use std::path::{Path, PathBuf};
use std::time::Duration;

pub const CONFINE_TIMEOUT: Duration = Duration::from_secs(2);

pub fn implicit_launch_cwd() -> PathBuf {
    resolve_implicit_launch_cwd(std::env::current_dir().ok(), dirs::home_dir())
}

fn resolve_implicit_launch_cwd(current: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    match current {
        Some(current) if is_filesystem_root(&current) => home.unwrap_or(current),
        Some(current) => current,
        None => home.unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string())),
    }
}

pub fn is_filesystem_root(path: &Path) -> bool {
    path.has_root() && path.parent().is_none()
}

pub fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    let stripped = path.to_str().and_then(|s| {
        s.strip_prefix(r"\\?\UNC\")
            .map(|rest| PathBuf::from(format!(r"\\{rest}")))
            .or_else(|| s.strip_prefix(r"\\?\").map(PathBuf::from))
    });
    stripped.unwrap_or(path)
}

pub fn canonical_dir(raw: &str) -> Result<PathBuf, String> {
    let expanded = expand_tilde(raw, dirs::home_dir().as_deref());
    let canonical = std::fs::canonicalize(&expanded)
        .map_err(|error| format!("cwd does not exist or is unreadable: {raw} ({error})"))?;
    let meta = std::fs::metadata(&canonical)
        .map_err(|error| format!("cwd metadata read failed for {raw}: {error}"))?;
    if !meta.is_dir() {
        return Err(format!("cwd is not a directory: {raw}"));
    }
    Ok(strip_verbatim_prefix(canonical))
}

fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    match raw {
        "~" => home.map_or_else(|| PathBuf::from(raw), Path::to_path_buf),
        _ => raw
            .strip_prefix("~/")
            .or_else(|| raw.strip_prefix("~\\"))
            .and_then(|rest| home.map(|home| home.join(rest)))
            .unwrap_or_else(|| PathBuf::from(raw)),
    }
}

pub fn run_bounded<T: Send + 'static>(
    timeout: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("paneflow-path-probe".to_owned())
        .spawn(move || {
            let _ = sender.send(work());
        })
        .ok()?;
    receiver.recv_timeout(timeout).ok()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpawnCwd {
    pub cwd: Option<PathBuf>,
    pub confine_to: Option<PathBuf>,
    pub fallback: Option<PathBuf>,
}

impl SpawnCwd {
    pub fn within(worktree: Option<&Path>, inherited: Option<PathBuf>) -> Self {
        match worktree {
            None => Self {
                cwd: inherited,
                ..Self::default()
            },
            Some(worktree) => Self {
                cwd: Some(inherited.unwrap_or_else(|| worktree.to_path_buf())),
                confine_to: Some(worktree.to_path_buf()),
                fallback: None,
            },
        }
    }

    pub fn needs_resolving(&self) -> bool {
        self.confine_to.is_some() || self.fallback.is_some()
    }

    pub fn resolve(self, timeout: Duration) -> Option<PathBuf> {
        self.resolve_with(timeout, is_inside_checkout, Path::is_dir)
    }

    fn resolve_with(
        self,
        timeout: Duration,
        inside: fn(&Path, &Path) -> bool,
        is_dir: fn(&Path) -> bool,
    ) -> Option<PathBuf> {
        match (self.cwd, self.confine_to, self.fallback) {
            (None, Some(worktree), _) => Some(worktree),
            (Some(cwd), Some(worktree), _) => {
                let root = worktree.clone();
                let kept = run_bounded(timeout, move || inside(&cwd, &root).then_some(cwd));
                Some(kept.flatten().unwrap_or(worktree))
            }
            (Some(cwd), None, Some(fallback)) => {
                let kept = run_bounded(timeout, move || is_dir(&cwd).then_some(cwd));
                Some(kept.flatten().unwrap_or(fallback))
            }
            (None, None, fallback) => fallback,
            (cwd, None, None) => cwd,
        }
    }
}

fn is_inside_checkout(cwd: &Path, checkout: &Path) -> bool {
    if cwd
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return false;
    }
    let canonical = |path: &Path| std::fs::canonicalize(path).map(strip_verbatim_prefix);
    match (canonical(cwd), canonical(checkout)) {
        (Ok(cwd), Ok(checkout)) => cwd.starts_with(checkout),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn platform_root() -> PathBuf {
        std::env::current_dir()
            .ok()
            .and_then(|path| path.ancestors().last().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(std::path::MAIN_SEPARATOR.to_string()))
    }

    fn child_of_root(name: &str) -> PathBuf {
        let mut path = platform_root();
        path.push(name);
        path
    }

    #[test]
    fn implicit_launch_cwd_uses_home_when_current_is_root() {
        let root = platform_root();
        let home = child_of_root("home");

        assert_eq!(
            resolve_implicit_launch_cwd(Some(root), Some(home.clone())),
            home
        );
    }

    #[test]
    fn implicit_launch_cwd_keeps_non_root_current() {
        let current = child_of_root("project");
        let home = child_of_root("home");

        assert_eq!(
            resolve_implicit_launch_cwd(Some(current.clone()), Some(home)),
            current
        );
    }

    #[test]
    fn implicit_launch_cwd_uses_home_when_current_is_missing() {
        let home = child_of_root("home");

        assert_eq!(resolve_implicit_launch_cwd(None, Some(home.clone())), home);
    }

    #[test]
    fn a_confinement_that_cannot_resolve_in_time_falls_back_to_the_worktree() {
        let spawn = SpawnCwd {
            cwd: Some(PathBuf::from("/mnt/dead/wt/src")),
            confine_to: Some(PathBuf::from("/mnt/dead/wt")),
            fallback: None,
        };
        let started = Instant::now();
        let cwd = spawn.resolve_with(
            Duration::from_millis(50),
            |_, _| {
                std::thread::sleep(Duration::from_secs(30));
                true
            },
            Path::is_dir,
        );
        assert_eq!(cwd, Some(PathBuf::from("/mnt/dead/wt")));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_missing_restored_cwd_falls_back_off_the_render_thread() {
        let tmp = tempfile::tempdir().unwrap();
        let fallback = tmp.path().join("workspace");
        std::fs::create_dir_all(&fallback).unwrap();
        let spawn = |cwd: PathBuf| SpawnCwd {
            cwd: Some(cwd),
            confine_to: None,
            fallback: Some(fallback.clone()),
        };
        assert_eq!(
            spawn(tmp.path().join("missing")).resolve(CONFINE_TIMEOUT),
            Some(fallback.clone()),
            "a surface cwd that is gone falls back to the workspace folder"
        );
        assert_eq!(
            spawn(tmp.path().to_path_buf()).resolve(CONFINE_TIMEOUT),
            Some(tmp.path().to_path_buf())
        );
        let stuck = spawn(tmp.path().to_path_buf()).resolve_with(
            Duration::from_millis(50),
            |_, _| true,
            |_| {
                std::thread::sleep(Duration::from_secs(30));
                true
            },
        );
        assert_eq!(stuck, Some(fallback));
    }

    #[test]
    fn canonical_dir_expands_home_and_refuses_what_is_not_a_directory() {
        let home = PathBuf::from(if cfg!(windows) {
            r"C:\Users\Arthur"
        } else {
            "/home/arthur"
        });
        assert_eq!(expand_tilde("~", Some(&home)), home);
        assert_eq!(
            expand_tilde("~/dev/backend", Some(&home)),
            home.join("dev/backend")
        );
        assert_eq!(
            expand_tilde("~\\dev\\backend", Some(&home)),
            home.join("dev\\backend")
        );
        assert_eq!(
            expand_tilde("rel/~not-home", Some(&home)),
            PathBuf::from("rel/~not-home")
        );

        let tmp = tempfile::tempdir().unwrap();
        let resolved = canonical_dir(tmp.path().to_str().unwrap()).unwrap();
        assert!(resolved.is_absolute() && resolved.is_dir());
        assert!(!resolved.to_string_lossy().starts_with(r"\\?\"));

        let missing = tmp.path().join("missing");
        let error = canonical_dir(missing.to_str().unwrap()).unwrap_err();
        assert!(error.contains("does not exist"), "{error}");

        let file = tmp.path().join("file");
        std::fs::write(&file, "").unwrap();
        let error = canonical_dir(file.to_str().unwrap()).unwrap_err();
        assert!(error.contains("not a directory"), "{error}");
    }

    #[test]
    fn a_bounded_call_gives_up_on_work_that_never_returns() {
        let started = Instant::now();
        let stuck = run_bounded(Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_secs(30));
            1
        });
        assert_eq!(stuck, None);
        assert!(started.elapsed() < Duration::from_secs(2));
        assert_eq!(run_bounded(Duration::from_secs(5), || 2), Some(2));
    }
}
