use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui::Hsla;
use paneflow_git::Freshness;
use paneflow_git::engine::{Probe, Selector};
use paneflow_git::porcelain::{self, Record};

use crate::git_monitors::{GitMonitors, Keep};
use crate::theme::UiColors;
use crate::workspace::git::git_stdout;

const GIT_STATUS_DEADLINE: Duration = Duration::from_secs(10);
const GIT_STATUS_STDOUT_CAP: u64 = 4 * 1024 * 1024;
const PORCELAIN_RECORD_FRAMING: u64 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TrackedSummary {
    pub added: u32,
    pub modified: u32,
    pub deleted: u32,
}

impl std::ops::AddAssign for TrackedSummary {
    fn add_assign(&mut self, rhs: Self) {
        self.added += rhs.added;
        self.modified += rhs.modified;
        self.deleted += rhs.deleted;
    }
}

impl std::ops::Add for TrackedSummary {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self {
        self += rhs;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitSummary {
    pub index: TrackedSummary,
    pub worktree: TrackedSummary,
    pub untracked: u32,
    pub conflict: u32,
}

impl std::ops::AddAssign for GitSummary {
    fn add_assign(&mut self, rhs: Self) {
        self.index += rhs.index;
        self.worktree += rhs.worktree;
        self.untracked += rhs.untracked;
        self.conflict += rhs.conflict;
    }
}

impl GitSummary {
    pub(crate) fn is_unchanged(self) -> bool {
        self == Self::default()
    }

    fn from_porcelain_code(code: [u8; 2]) -> Self {
        match &code {
            b"??" => Self {
                untracked: 1,
                ..Self::default()
            },
            b"!!" => Self::default(),
            b"AA" | b"DD" | [_, b'U'] | [b'U', _] => Self {
                conflict: 1,
                ..Self::default()
            },
            [index, worktree] => Self {
                index: tracked_from_code(*index),
                worktree: tracked_from_code(*worktree),
                ..Self::default()
            },
        }
    }
}

fn tracked_from_code(code: u8) -> TrackedSummary {
    match code {
        b'M' | b'T' => TrackedSummary {
            modified: 1,
            ..TrackedSummary::default()
        },
        b'A' => TrackedSummary {
            added: 1,
            ..TrackedSummary::default()
        },
        b'D' => TrackedSummary {
            deleted: 1,
            ..TrackedSummary::default()
        },
        _ => TrackedSummary::default(),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GitStatuses {
    entries: HashMap<PathBuf, GitSummary>,
}

impl GitStatuses {
    pub(crate) fn summary(&self, path: &Path) -> GitSummary {
        self.entries.get(path).copied().unwrap_or_default()
    }

    pub(crate) fn parse(root: &Path, prefix: &str, stdout: &[u8]) -> Self {
        let records = stdout
            .split(|byte| *byte == 0)
            .filter(|record| record.len() >= 4 && record[2] == b' ')
            .map(|record| ([record[0], record[1]], &record[3..]));
        Self::from_records(root, prefix.as_bytes(), records)
    }

    fn from_snapshot(
        root: &Path,
        snapshot: &paneflow_git::Snapshot,
        stdout_cap: u64,
    ) -> Option<Self> {
        if snapshot.truncated {
            return None;
        }
        let records = porcelain::records(snapshot);
        let in_scope = |record: &&Record| record.path.starts_with(&snapshot.prefix);
        let porcelain_bytes: u64 = records
            .iter()
            .filter(in_scope)
            .map(|record| record.path.len() as u64 + PORCELAIN_RECORD_FRAMING)
            .sum();
        if porcelain_bytes > stdout_cap {
            return None;
        }
        Some(Self::from_records(
            root,
            &snapshot.prefix,
            records
                .iter()
                .filter(in_scope)
                .map(|record| (record.code, record.path.as_slice())),
        ))
    }

    fn from_records<'a>(
        root: &Path,
        prefix: &[u8],
        records: impl Iterator<Item = ([u8; 2], &'a [u8])>,
    ) -> Self {
        let mut entries: HashMap<PathBuf, GitSummary> = HashMap::new();
        for (code, path) in records {
            let summary = GitSummary::from_porcelain_code(code);
            if summary.is_unchanged() {
                continue;
            }
            let Some(relative) = path.strip_prefix(prefix) else {
                continue;
            };
            let Ok(relative) = std::str::from_utf8(relative) else {
                continue;
            };
            let absolute = root.join(relative);
            for ancestor in absolute.ancestors().take_while(|dir| dir.starts_with(root)) {
                *entries.entry(ancestor.to_path_buf()).or_default() += summary;
            }
        }
        Self { entries }
    }
}

pub(crate) fn read(root: &Path) -> Option<GitStatuses> {
    read_through(Selector::global(), root, |root| {
        read_with_gix(root, GIT_STATUS_STDOUT_CAP)
    })
}

fn read_through(
    selector: &Selector,
    root: &Path,
    gix: impl FnOnce(&Path) -> Result<Option<GitStatuses>, paneflow_git::Error>,
) -> Option<GitStatuses> {
    selector.run(
        Probe::FilesStatus,
        root,
        || gix(root),
        || read_with_cli(root, GIT_STATUS_STDOUT_CAP),
    )
}

fn read_with_gix(root: &Path, stdout_cap: u64) -> Result<Option<GitStatuses>, paneflow_git::Error> {
    read_with_gix_through(GitMonitors::global(), Selector::global(), root, stdout_cap)
}

fn read_with_gix_through(
    monitors: &GitMonitors,
    selector: &Selector,
    root: &Path,
    stdout_cap: u64,
) -> Result<Option<GitStatuses>, paneflow_git::Error> {
    let probed = monitors.with_snapshot(
        selector,
        root,
        &paneflow_git::Options::files_status(),
        Freshness::Converging,
        Keep::Alive,
        |snapshot, _| Ok(snapshot),
    );
    match probed {
        Ok(snapshot) => Ok(GitStatuses::from_snapshot(root, &snapshot, stdout_cap)),
        Err(paneflow_git::Error::NotARepository { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

fn read_with_cli(root: &Path, stdout_cap: u64) -> Option<GitStatuses> {
    let prefix = git_stdout(
        root,
        &["rev-parse", "--show-prefix"],
        GIT_STATUS_DEADLINE,
        GIT_STATUS_STDOUT_CAP,
    )?;
    let prefix = String::from_utf8_lossy(&prefix)
        .trim_end_matches(['\n', '\r'])
        .to_string();
    let stdout = git_stdout(
        root,
        porcelain::FILES_STATUS_ARGS,
        GIT_STATUS_DEADLINE,
        stdout_cap,
    )?;
    Some(GitStatuses::parse(root, &prefix, &stdout))
}

pub(crate) fn label_color(summary: GitSummary, ui: UiColors) -> Option<Hsla> {
    let tracked = summary.index + summary.worktree;
    if summary.conflict > 0 {
        Some(ui.vc_conflict)
    } else if tracked.deleted > 0 {
        Some(ui.vc_deleted)
    } else if tracked.modified > 0 {
        Some(ui.vc_modified)
    } else if tracked.added > 0 || summary.untracked > 0 {
        Some(ui.vc_added)
    } else {
        None
    }
}

pub(crate) fn status_indicator(summary: GitSummary, ui: UiColors) -> Option<(&'static str, Hsla)> {
    if summary.conflict > 0 {
        return Some(("!", ui.vc_conflict));
    }
    if summary.untracked > 0 {
        return Some(("U", ui.vc_added));
    }
    if summary.worktree.deleted > 0 {
        return Some(("D", ui.vc_deleted));
    }
    if summary.worktree.modified > 0 {
        return Some(("M", ui.vc_modified));
    }
    if summary.index.deleted > 0 {
        return Some(("D", ui.vc_deleted));
    }
    if summary.index.modified > 0 {
        return Some(("M", ui.vc_modified));
    }
    if summary.index.added > 0 {
        return Some(("A", ui.vc_added));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_fixture as fixture;

    fn ui() -> UiColors {
        crate::theme::ui_colors_with(&crate::theme::paneflow_dark())
    }

    fn porcelain(records: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for record in records {
            out.extend_from_slice(record.as_bytes());
            out.push(0);
        }
        out
    }

    fn statuses(records: &[&str]) -> GitStatuses {
        GitStatuses::parse(Path::new("/repo"), "", &porcelain(records))
    }

    fn committed_repo(root: &Path) {
        fixture::committed_repo(
            root,
            &[("src/app/row.rs", "one\n"), ("README.md", "readme\n")],
        );
    }

    #[test]
    fn porcelain_codes_map_to_tracked_index_and_worktree_sides() {
        let modified_index = GitSummary::from_porcelain_code(*b"M ");
        assert_eq!(modified_index.index.modified, 1);
        assert_eq!(modified_index.worktree.modified, 0);

        let modified_worktree = GitSummary::from_porcelain_code(*b" M");
        assert_eq!(modified_worktree.worktree.modified, 1);
        assert_eq!(modified_worktree.index.modified, 0);

        assert_eq!(GitSummary::from_porcelain_code(*b"A ").index.added, 1);
        assert_eq!(GitSummary::from_porcelain_code(*b" D").worktree.deleted, 1);
        assert_eq!(GitSummary::from_porcelain_code(*b"T ").index.modified, 1);
    }

    #[test]
    fn untracked_conflicted_and_ignored_codes_follow_zed() {
        assert_eq!(GitSummary::from_porcelain_code(*b"??").untracked, 1);
        assert_eq!(GitSummary::from_porcelain_code(*b"UU").conflict, 1);
        assert_eq!(GitSummary::from_porcelain_code(*b"AA").conflict, 1);
        assert_eq!(GitSummary::from_porcelain_code(*b"DD").conflict, 1);
        assert_eq!(GitSummary::from_porcelain_code(*b"AU").conflict, 1);
        assert!(GitSummary::from_porcelain_code(*b"!!").is_unchanged());
        assert!(GitSummary::from_porcelain_code(*b"  ").is_unchanged());
    }

    #[test]
    fn renamed_and_copied_codes_carry_no_summary() {
        assert!(GitSummary::from_porcelain_code(*b"R ").is_unchanged());
        assert!(GitSummary::from_porcelain_code(*b"C ").is_unchanged());
    }

    #[test]
    fn summaries_roll_up_into_every_ancestor_directory() {
        let statuses = statuses(&[" M src/app/row.rs", "?? src/app/new.rs", "D  docs/old.md"]);

        assert_eq!(
            statuses
                .summary(Path::new("/repo/src/app/row.rs"))
                .worktree
                .modified,
            1
        );
        let app = statuses.summary(Path::new("/repo/src/app"));
        assert_eq!(app.worktree.modified, 1);
        assert_eq!(app.untracked, 1);
        let src = statuses.summary(Path::new("/repo/src"));
        assert_eq!(src.worktree.modified, 1);
        assert_eq!(src.untracked, 1);
        assert_eq!(statuses.summary(Path::new("/repo/docs")).index.deleted, 1);
        let root = statuses.summary(Path::new("/repo"));
        assert_eq!(root.worktree.modified, 1);
        assert_eq!(root.untracked, 1);
        assert_eq!(root.index.deleted, 1);
    }

    #[test]
    fn unchanged_paths_have_no_summary() {
        let statuses = statuses(&[" M src/app/row.rs"]);
        assert!(
            statuses
                .summary(Path::new("/repo/src/app/view.rs"))
                .is_unchanged()
        );
        assert!(statuses.summary(Path::new("/repo/docs")).is_unchanged());
    }

    #[test]
    fn a_subdirectory_root_strips_the_repository_prefix() {
        let statuses = GitStatuses::parse(
            Path::new("/repo/src-app"),
            "src-app/",
            &porcelain(&[" M src-app/src/main.rs"]),
        );

        assert_eq!(
            statuses
                .summary(Path::new("/repo/src-app/src/main.rs"))
                .worktree
                .modified,
            1
        );
        assert_eq!(
            statuses
                .summary(Path::new("/repo/src-app/src"))
                .worktree
                .modified,
            1
        );
    }

    #[test]
    fn malformed_records_are_skipped() {
        let statuses = statuses(&["", "M", " M"]);
        assert!(statuses.summary(Path::new("/repo")).is_unchanged());
    }

    #[test]
    fn read_maps_a_real_repository_onto_absolute_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        std::fs::write(root.join("src").join("app").join("row.rs"), "two\n").expect("edit");
        std::fs::write(root.join("src").join("app").join("new.rs"), "new\n").expect("new file");

        let statuses = read(root).expect("statuses of a real repository");

        assert_eq!(
            statuses
                .summary(&root.join("src").join("app").join("row.rs"))
                .worktree
                .modified,
            1
        );
        assert_eq!(
            statuses
                .summary(&root.join("src").join("app").join("new.rs"))
                .untracked,
            1
        );
        let app = statuses.summary(&root.join("src").join("app"));
        assert_eq!(app.worktree.modified, 1);
        assert_eq!(app.untracked, 1);
        assert_eq!(statuses.summary(&root.join("src")).untracked, 1);
        assert!(statuses.summary(&root.join("README.md")).is_unchanged());
    }

    #[test]
    fn read_from_a_subdirectory_scopes_to_that_subtree() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        std::fs::write(root.join("src").join("app").join("row.rs"), "two\n").expect("edit");
        std::fs::write(root.join("README.md"), "changed\n").expect("readme edit");

        let statuses = read(&root.join("src")).expect("statuses of a subdirectory");

        assert_eq!(
            statuses
                .summary(&root.join("src").join("app").join("row.rs"))
                .worktree
                .modified,
            1
        );
        assert!(statuses.summary(&root.join("README.md")).is_unchanged());
    }

    #[test]
    fn read_outside_a_repository_yields_no_statuses() {
        let dir = tempfile::tempdir().expect("tempdir");
        if fixture::git_succeeds(dir.path(), &["rev-parse", "--git-dir"]) {
            return;
        }
        std::fs::write(dir.path().join("loose.txt"), "loose\n").expect("loose file");

        assert!(read(dir.path()).is_none());
    }

    #[test]
    fn a_subdirectory_whose_name_starts_with_a_space_keeps_its_statuses() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        let spaced = root.join(" spaced");
        std::fs::create_dir(&spaced).expect("spaced directory");
        std::fs::write(spaced.join("file.txt"), "one\n").expect("file");
        fixture::commit_all(root, "spaced");
        std::fs::write(spaced.join("file.txt"), "two\n").expect("edit");

        let statuses = read(&spaced).expect("statuses of the spaced directory");

        assert_eq!(
            statuses.summary(&spaced.join("file.txt")).worktree.modified,
            1
        );
    }

    fn spawns_on_this_thread() -> u64 {
        use crate::work_counters::{counted_on_this_thread, git_profile_counter};
        counted_on_this_thread(git_profile_counter(true))
            + counted_on_this_thread(git_profile_counter(false))
    }

    fn gix_statuses(root: &Path) -> Option<GitStatuses> {
        read_with_gix(root, GIT_STATUS_STDOUT_CAP).expect("gix statuses")
    }

    fn cli_statuses(root: &Path) -> Option<GitStatuses> {
        read_with_cli(root, GIT_STATUS_STDOUT_CAP)
    }

    fn assert_the_global_selector_prefers_gix() {
        assert_eq!(
            paneflow_git::engine::Selector::global().preferred(),
            paneflow_git::engine::Engine::Gix,
            "run the Files sidebar probe tests without PANEFLOW_GIT_ENGINE=cli"
        );
    }

    #[test]
    fn gix_and_cli_statuses_are_equal_on_every_oracle_fixture() {
        assert_gix_and_cli_statuses_are_equal_on_every_oracle_fixture();
    }

    #[test]
    fn gix_and_cli_statuses_are_equal_on_every_oracle_fixture_in_sha256_repositories() {
        paneflow_git::support::with_object_format(
            paneflow_git::support::SHA256,
            assert_gix_and_cli_statuses_are_equal_on_every_oracle_fixture,
        );
    }

    fn assert_gix_and_cli_statuses_are_equal_on_every_oracle_fixture() {
        use paneflow_git::oracle::{Probe, Skip, base, layouts, linked, submodules};
        paneflow_git::support::isolate_process();
        let tmp = tempfile::tempdir().expect("tempdir");
        let matrices = [
            ("base", base::matrix(), base::MAY_SKIP, &[][..]),
            ("layouts", layouts::matrix(), &[][..], &[][..]),
            ("linked", linked::matrix(), &[][..], &[][..]),
            (
                "submodules",
                submodules::matrix(Probe::Files),
                &[][..],
                submodules::MUST_FAIL,
            ),
        ];
        let mut compared = 0usize;
        let mut skipped = 0usize;
        let mut total = 0usize;
        let mut failures = Vec::new();
        for (matrix, fixtures, may_skip, must_fail) in matrices {
            for fixture in fixtures {
                total += 1;
                let root = tmp.path().join(matrix).join(fixture.name).join("repo");
                let cwd = match (fixture.build)(&root) {
                    Ok(cwd) => cwd,
                    Err(Skip(reason)) if may_skip.contains(&fixture.name) => {
                        eprintln!("oracle: SKIPPED fixture {}: {reason}", fixture.name);
                        skipped += 1;
                        continue;
                    }
                    Err(Skip(reason)) => {
                        failures.push(format!(
                            "fixture {} was skipped but is not allowed to skip: {reason}",
                            fixture.name
                        ));
                        continue;
                    }
                };
                let cli = cli_statuses(&cwd);
                let gix = read_with_gix(&cwd, GIT_STATUS_STDOUT_CAP);
                let expected_to_fail = must_fail.contains(&fixture.name);
                match (cli, gix) {
                    (Some(cli), Ok(Some(gix))) if !expected_to_fail => {
                        if cli != gix {
                            failures
                                .push(format!("fixture {}: cli {cli:?} gix {gix:?}", fixture.name));
                        }
                    }
                    (None, Err(_)) if expected_to_fail => {}
                    (cli, gix) => failures.push(format!(
                        "fixture {}: expected both engines to {}, got cli {cli:?} gix {gix:?}",
                        fixture.name,
                        if expected_to_fail { "fail" } else { "answer" }
                    )),
                }
                compared += 1;
            }
        }
        assert!(compared > 0);
        assert_eq!(
            compared + skipped,
            total,
            "every fixture is compared or skipped by name"
        );
        assert!(
            failures.is_empty(),
            "the Files sidebar statuses differ between the git CLI and gix ({compared} fixtures):\n{}",
            failures.join("\n")
        );
    }

    #[test]
    fn a_refresh_on_the_gix_path_spawns_no_git_process() {
        use paneflow_git::engine::{Counters, Engine, Probe, Selector};
        assert_the_global_selector_prefers_gix();
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        std::fs::write(root.join("README.md"), "changed\n").expect("edit");
        let gix_probes = |counters: &Counters| counters.probes(Probe::FilesStatus, Engine::Gix);
        let probes_before = gix_probes(&Selector::global().counters());

        let before = spawns_on_this_thread();
        let statuses = read(root).expect("statuses");
        let spawned = spawns_on_this_thread() - before;

        assert_eq!(spawned, 0, "a gix refresh spawns no git process");
        assert_eq!(
            statuses.summary(&root.join("README.md")).worktree.modified,
            1
        );
        assert!(gix_probes(&Selector::global().counters()) > probes_before);
    }

    #[test]
    fn a_failed_gix_refresh_returns_the_cli_statuses_and_counts_the_fallback() {
        use paneflow_git::engine::{Engine, Fallback, Probe, Selector};
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        std::fs::write(root.join("README.md"), "changed\n").expect("edit");
        let selector = Selector::new(Engine::Gix);

        let statuses = read_through(&selector, root, |_| {
            Err(paneflow_git::Error::Git {
                context: "compute the status",
                message: "injected failure".to_string(),
            })
        });

        assert_eq!(statuses, cli_statuses(root));
        assert_eq!(
            statuses
                .expect("the CLI statuses")
                .summary(&root.join("README.md"))
                .worktree
                .modified,
            1
        );
        let counters = selector.counters();
        assert_eq!(counters.fallbacks(Fallback::Error), 1);
        assert_eq!(counters.probes(Probe::FilesStatus, Engine::Cli), 1);
        assert_eq!(counters.probes(Probe::FilesStatus, Engine::Gix), 0);
    }

    #[test]
    fn a_refresh_outside_a_repository_spawns_no_git_process_and_counts_no_fallback() {
        use paneflow_git::engine::{Engine, Fallback, Probe, Selector};
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::write(root.join("notes.txt"), "plain folder\n").expect("file");
        assert_eq!(
            cli_statuses(root),
            None,
            "the CLI sees no repository either"
        );
        let selector = Selector::new(Engine::Gix);

        let before = spawns_on_this_thread();
        let statuses = read_through(&selector, root, |root| {
            read_with_gix(root, GIT_STATUS_STDOUT_CAP)
        });
        let spawned = spawns_on_this_thread() - before;

        assert_eq!(statuses, None);
        assert_eq!(spawned, 0, "a folder outside any repository spawns no git");
        let counters = selector.counters();
        assert_eq!(counters.fallbacks(Fallback::Error), 0);
        assert_eq!(counters.probes(Probe::FilesStatus, Engine::Gix), 1);
        assert_eq!(counters.probes(Probe::FilesStatus, Engine::Cli), 0);
    }

    #[test]
    fn the_forced_cli_answers_a_refresh_without_asking_gix() {
        use paneflow_git::engine::{Engine, Fallback, Selector};
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        std::fs::write(root.join("README.md"), "changed\n").expect("edit");
        let selector = Selector::new(Engine::Cli);
        let asked_gix = std::cell::Cell::new(false);

        let before = spawns_on_this_thread();
        let statuses = read_through(&selector, root, |_| {
            asked_gix.set(true);
            Ok(None)
        });

        assert!(!asked_gix.get());
        assert!(spawns_on_this_thread() - before > 0);
        assert_eq!(statuses, gix_statuses(root));
        assert_eq!(selector.counters().fallbacks(Fallback::Forced), 1);
    }

    #[test]
    fn a_status_past_the_stdout_cap_yields_no_statuses_on_both_engines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        for index in 0..8 {
            std::fs::write(root.join(format!("untracked-{index}.txt")), "new\n").expect("file");
        }
        let porcelain_len = fixture::output(root, porcelain::FILES_STATUS_ARGS)
            .stdout
            .len() as u64;
        assert!(porcelain_len > 100, "{porcelain_len}");

        assert_eq!(read_with_cli(root, porcelain_len - 1), None);
        assert_eq!(read_with_gix(root, porcelain_len - 1).expect("gix"), None);
        let cli = read_with_cli(root, porcelain_len).expect("cli statuses at the cap");
        let gix = read_with_gix(root, porcelain_len)
            .expect("gix")
            .expect("gix statuses at the cap");
        assert_eq!(cli, gix);
        assert_eq!(gix.summary(root).untracked, 8);
    }

    #[test]
    fn a_truncated_snapshot_yields_no_statuses() {
        let snapshot = paneflow_git::Snapshot {
            head: paneflow_git::Head::Unborn,
            prefix: Vec::new(),
            entries: vec![paneflow_git::Entry {
                path: b"new.txt".to_vec(),
                untracked: true,
                ..paneflow_git::Entry::default()
            }],
            sides: std::collections::BTreeMap::new(),
            truncated: true,
            submodules_opened: 0,
        };
        assert_eq!(
            GitStatuses::from_snapshot(Path::new("/repo"), &snapshot, GIT_STATUS_STDOUT_CAP),
            None
        );
        let complete = paneflow_git::Snapshot {
            truncated: false,
            ..snapshot
        };
        let statuses =
            GitStatuses::from_snapshot(Path::new("/repo"), &complete, GIT_STATUS_STDOUT_CAP)
                .expect("statuses");
        assert_eq!(statuses.summary(Path::new("/repo/new.txt")).untracked, 1);
    }

    #[test]
    fn a_subdirectory_root_gives_equal_statuses_on_both_engines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        committed_repo(root);
        std::fs::write(root.join("src").join("app").join("row.rs"), "two\n").expect("edit");
        std::fs::write(root.join("src").join("new.rs"), "new\n").expect("new file");
        std::fs::write(root.join("README.md"), "changed\n").expect("readme edit");

        for scope in [
            root.to_path_buf(),
            root.join("src"),
            root.join("src").join("app"),
        ] {
            let gix = gix_statuses(&scope).expect("gix statuses");
            assert_eq!(
                Some(&gix),
                cli_statuses(&scope).as_ref(),
                "{}",
                scope.display()
            );
        }
        let app = gix_statuses(&root.join("src").join("app")).expect("gix statuses");
        assert!(app.summary(&root.join("src").join("new.rs")).is_unchanged());
        assert_eq!(
            app.summary(&root.join("src").join("app")).worktree.modified,
            1
        );
    }

    #[test]
    fn a_truncating_agent_write_never_fails_while_the_files_status_probe_runs() {
        assert_the_files_status_race_never_fails_a_write(crate::git_race::Repository::Small);
    }

    #[test]
    fn a_truncating_agent_write_never_fails_while_the_files_status_probe_runs_in_a_large_repository()
     {
        assert_the_files_status_race_never_fails_a_write(crate::git_race::Repository::Large);
    }

    #[test]
    fn a_truncating_agent_write_never_fails_while_the_files_status_probe_runs_in_a_sha256_repository()
     {
        assert_the_files_status_race_never_fails_a_write(crate::git_race::Repository::Sha256);
    }

    fn assert_the_files_status_race_never_fails_a_write(repository: crate::git_race::Repository) {
        use crate::git_race::{RACED_FILE, Raced, Scenario, race_in};
        assert_the_global_selector_prefers_gix();
        let outcome = race_in(
            repository,
            Scenario::StatDirty,
            Raced::Tracked,
            |root: &Path| {
                let before = spawns_on_this_thread();
                let summary = read(root)
                    .map(|statuses| statuses.summary(&root.join(RACED_FILE)))
                    .ok_or_else(|| "the Files sidebar probe returned no statuses".to_string())?;
                match spawns_on_this_thread() - before {
                    0 => Ok(summary),
                    spawned => Err(format!(
                        "the probe fell back to the CLI and spawned {spawned} git processes"
                    )),
                }
            },
        );
        outcome.assert_no_failed_write();
        assert!(
            outcome
                .answers
                .iter()
                .any(|summary| summary.worktree.modified == 1),
            "a probe still sees the rewritten file: {:?}",
            outcome.answers
        );
    }

    #[test]
    fn label_color_orders_conflict_deleted_modified_created() {
        let ui = ui();
        let conflict = GitSummary {
            conflict: 1,
            worktree: TrackedSummary {
                modified: 1,
                ..TrackedSummary::default()
            },
            ..GitSummary::default()
        };
        assert_eq!(label_color(conflict, ui), Some(ui.vc_conflict));

        let deleted = GitSummary {
            index: TrackedSummary {
                deleted: 1,
                ..TrackedSummary::default()
            },
            worktree: TrackedSummary {
                modified: 1,
                ..TrackedSummary::default()
            },
            ..GitSummary::default()
        };
        assert_eq!(label_color(deleted, ui), Some(ui.vc_deleted));

        let modified = GitSummary {
            worktree: TrackedSummary {
                modified: 1,
                ..TrackedSummary::default()
            },
            untracked: 1,
            ..GitSummary::default()
        };
        assert_eq!(label_color(modified, ui), Some(ui.vc_modified));

        let created = GitSummary {
            untracked: 1,
            ..GitSummary::default()
        };
        assert_eq!(label_color(created, ui), Some(ui.vc_added));
        assert_eq!(label_color(GitSummary::default(), ui), None);
    }

    #[test]
    fn status_indicator_prefers_the_worktree_side_over_the_index() {
        let ui = ui();
        let both = GitSummary {
            index: TrackedSummary {
                added: 1,
                ..TrackedSummary::default()
            },
            worktree: TrackedSummary {
                modified: 1,
                ..TrackedSummary::default()
            },
            ..GitSummary::default()
        };
        assert_eq!(status_indicator(both, ui), Some(("M", ui.vc_modified)));

        let staged = GitSummary {
            index: TrackedSummary {
                added: 1,
                ..TrackedSummary::default()
            },
            ..GitSummary::default()
        };
        assert_eq!(status_indicator(staged, ui), Some(("A", ui.vc_added)));

        let untracked = GitSummary {
            untracked: 1,
            ..GitSummary::default()
        };
        assert_eq!(status_indicator(untracked, ui), Some(("U", ui.vc_added)));

        let conflict = GitSummary {
            conflict: 1,
            ..GitSummary::default()
        };
        assert_eq!(status_indicator(conflict, ui), Some(("!", ui.vc_conflict)));
        assert_eq!(status_indicator(GitSummary::default(), ui), None);
    }
}
