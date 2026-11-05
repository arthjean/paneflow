use std::collections::BTreeSet;
use std::path::Path;

use gix::bstr::BString;
use gix::config::Source;
use gix::discover::upwards;

use crate::Error;

const REPOSITORY_CONFIG_SOURCES: &[Source] = &[Source::Local, Source::Worktree];

pub(crate) fn discover(path: &Path, thread_limit: usize) -> Result<gix::Repository, Error> {
    let repo = gix::ThreadSafeRepository::discover_opts(
        path,
        upwards::Options::default(),
        open_options_by_trust(),
    )
    .map_err(|error| classify_discovery_error(path, error))?
    .to_thread_local();
    into_worktree_repository(path, repo, thread_limit)
}

pub(crate) fn submodule(work_dir: &Path, thread_limit: usize) -> Result<gix::Repository, Error> {
    let repo = gix::ThreadSafeRepository::open_opts(work_dir, open_options())
        .map_err(|error| Error::git("open the submodule repository", error))?
        .to_thread_local();
    into_worktree_repository(work_dir, repo, thread_limit)
}

fn open_options() -> gix::open::Options {
    gix::open::Options::default().bail_if_untrusted(true)
}

fn open_options_by_trust() -> gix::sec::trust::Mapping<gix::open::Options> {
    gix::sec::trust::Mapping {
        full: open_options(),
        reduced: open_options(),
    }
}

fn classify_discovery_error(path: &Path, error: gix::Error) -> Error {
    match error.downcast_any_ref::<upwards::Error>() {
        Some(
            upwards::Error::NoGitRepository { .. }
            | upwards::Error::NoGitRepositoryWithinCeiling { .. }
            | upwards::Error::NoGitRepositoryWithinFs { .. },
        ) => Error::NotARepository {
            path: path.to_path_buf(),
        },
        Some(upwards::Error::NoTrustedGitRepository { .. }) => Error::Untrusted {
            path: path.to_path_buf(),
        },
        _ if error.is_permission_denied() => Error::Untrusted {
            path: path.to_path_buf(),
        },
        _ => Error::git("open the repository", error),
    }
}

fn into_worktree_repository(
    path: &Path,
    mut repo: gix::Repository,
    thread_limit: usize,
) -> Result<gix::Repository, Error> {
    if repo.workdir().is_none() {
        return Err(Error::Bare {
            path: path.to_path_buf(),
        });
    }
    configure_probe(&mut repo, thread_limit)?;
    Ok(repo)
}

pub(crate) fn repository_filter_drivers(repo: &gix::Repository) -> BTreeSet<BString> {
    repo.config_snapshot()
        .plumbing()
        .sections_by_name("filter")
        .into_iter()
        .flatten()
        .filter(|section| REPOSITORY_CONFIG_SOURCES.contains(&section.meta().source))
        .filter_map(|section| section.header().subsection_name().map(ToOwned::to_owned))
        .collect()
}

fn configure_probe(repo: &mut gix::Repository, thread_limit: usize) -> Result<(), Error> {
    let drivers = repository_filter_drivers(repo);
    let mut config = repo.config_snapshot_mut();
    for driver in &drivers {
        while config
            .remove_section("filter", Some(driver.as_slice().into()))
            .is_some()
        {}
    }
    config
        .set_value(
            &gix::config::tree::Index::THREADS,
            thread_limit.max(1).to_string().as_str(),
        )
        .map_err(|error| Error::git("limit the index decoding threads", error))?;
    config
        .commit()
        .map_err(|error| Error::git("neutralize the repository filter drivers", error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::{Command, Output};

    use super::{classify_discovery_error, discover, open_options_by_trust};
    use crate::porcelain::FILES_STATUS_ARGS;
    use crate::Error;

    fn git(root: &Path, global: &Path, args: &[&str], foreign_owner: bool) -> Output {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", global)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE");
        if foreign_owner {
            command.env("GIT_TEST_ASSUME_DIFFERENT_OWNER", "1");
        }
        command.output().expect("git")
    }

    #[test]
    fn a_repository_with_reduced_trust_gives_no_status_like_the_cli_refusing_its_owner() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let global = tmp.path().join("gitconfig");
        std::fs::write(&global, "").expect("global config");
        let root = tmp.path().join("repo");
        std::fs::create_dir_all(&root).expect("repo dir");
        assert!(git(&root, &global, &["init", "-q"], false).status.success());

        let refused = git(&root, &global, FILES_STATUS_ARGS, true);
        assert!(!refused.status.success(), "the CLI refuses a foreign owner");
        assert!(
            String::from_utf8_lossy(&refused.stderr).contains("dubious ownership"),
            "{}",
            String::from_utf8_lossy(&refused.stderr)
        );

        let reduced = open_options_by_trust()
            .reduced
            .with(gix::sec::Trust::Reduced);
        let error = gix::ThreadSafeRepository::open_opts(&root, reduced)
            .map(|_| ())
            .expect_err("the reduced-trust options refuse the repository");
        let classified = classify_discovery_error(&root, error);
        assert!(
            matches!(classified, Error::Untrusted { .. }),
            "{classified:?}"
        );

        assert!(
            discover(&root, 1).is_ok(),
            "the same repository opens with the full trust of its owner"
        );
    }
}
