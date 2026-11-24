use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub(crate) use paneflow_launch::cwd::run_bounded;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirProbe {
    Dir,
    Missing,
    Unresolved,
}

pub(crate) async fn probe_dirs(
    paths: Vec<PathBuf>,
    timeout: Duration,
    is_dir: fn(&Path) -> bool,
) -> Vec<(PathBuf, DirProbe)> {
    let deadline = Instant::now() + timeout;
    let probes: Vec<(PathBuf, smol::Task<bool>)> = paths
        .into_iter()
        .map(|path| {
            let probed = path.clone();
            (path, smol::unblock(move || is_dir(&probed)))
        })
        .collect();
    let mut states = Vec::with_capacity(probes.len());
    for (path, probe) in probes {
        let state = smol::future::or(
            async {
                if probe.await {
                    DirProbe::Dir
                } else {
                    DirProbe::Missing
                }
            },
            async {
                smol::Timer::at(deadline).await;
                DirProbe::Unresolved
            },
        )
        .await;
        states.push((path, state));
    }
    states
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stalls_on_stuck(path: &Path) -> bool {
        if path.ends_with("stuck") {
            std::thread::sleep(Duration::from_secs(30));
        }
        path.is_dir()
    }

    #[test]
    fn a_path_that_blocks_30_s_is_left_unresolved_within_the_deadline() {
        let tmp = tempfile::tempdir().unwrap();
        let live = tmp.path().to_path_buf();
        let stuck = tmp.path().join("stuck");
        let missing = tmp.path().join("missing");
        std::fs::create_dir_all(&stuck).unwrap();

        let started = Instant::now();
        let states = smol::block_on(probe_dirs(
            vec![live.clone(), stuck.clone(), missing.clone()],
            Duration::from_millis(300),
            stalls_on_stuck,
        ));

        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(
            states,
            vec![
                (live, DirProbe::Dir),
                (stuck, DirProbe::Unresolved),
                (missing, DirProbe::Missing),
            ]
        );
    }
}
