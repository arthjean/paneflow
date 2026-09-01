//! Whether a branch shown in the rail already has a pull request.
//!
//! The rail's other optional lines read files: `HEAD` for the branch, a
//! `git diff` for the counts. This one is a network answer, so it is cached
//! per `(repository, branch)`, refreshed on the same 30 s tick that refreshes
//! the git state, and only ever fetched while the `PR` switch is on.
//!
//! `gh` rather than `api.github.com` directly: it already holds the user's
//! credentials, follows GitHub Enterprise hosts, and resolves which repository
//! a directory belongs to. The cost is that this is GitHub-only - a GitLab or
//! Gitea checkout simply never gets an icon, which is also what happens when
//! `gh` is missing or logged out.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{Context, Hsla, Rgba};

use crate::PaneFlowApp;

/// How long a lookup stays good. Long enough that opening and closing tabs
/// does not re-query, short enough that a PR opened from the terminal shows up
/// while you are still in the same sitting.
const TTL: Duration = Duration::from_secs(300);
/// Wall-clock bound for one `gh` call, well under the 30 s tick.
const DEADLINE: Duration = Duration::from_secs(15);
const STDOUT_CAP: u64 = 256 * 1024;

/// A branch's pull request, in GitHub's own vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrState {
    Draft,
    Open,
    Merged,
    Closed,
}

impl PrState {
    /// GitHub's status colors, in Primer's light and dark values.
    ///
    /// Borrowed rather than mapped onto Paneflow's own palette: these four
    /// colors are what a pull request means to anyone who has used GitHub, and
    /// a green "merged" or a purple "open" would read as a different state
    /// rather than as a house style.
    pub(crate) fn color(self, ui: crate::theme::UiColors) -> Hsla {
        let light = ui.surface.l > 0.5;
        let hex = match (self, light) {
            (PrState::Open, true) => 0x1a7f37,
            (PrState::Open, false) => 0x3fb950,
            (PrState::Draft, true) => 0x59636e,
            (PrState::Draft, false) => 0x9198a1,
            (PrState::Merged, true) => 0x8250df,
            (PrState::Merged, false) => 0xab7df8,
            (PrState::Closed, true) => 0xcf222e,
            (PrState::Closed, false) => 0xf85149,
        };
        Hsla::from(Rgba {
            r: ((hex >> 16) & 0xFF) as f32 / 255.0,
            g: ((hex >> 8) & 0xFF) as f32 / 255.0,
            b: (hex & 0xFF) as f32 / 255.0,
            a: 1.0,
        })
    }
}

/// The pull request a branch is carried by, as the rail needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PullRequest {
    pub number: u64,
    pub state: PrState,
}

struct Cached {
    /// `None` means "asked, and this branch has no pull request" - a real
    /// answer worth caching, not a missing one.
    value: Option<PullRequest>,
    at: Instant,
}

/// Pull requests by `(repository root, branch)`.
#[derive(Default)]
pub(crate) struct PrStates {
    entries: HashMap<(String, String), Cached>,
    /// Lookups in flight, so a 30 s tick landing on a slow `gh` does not stack
    /// a second call on the same branch.
    inflight: HashSet<(String, String)>,
    /// Repositories `gh` cannot answer for: not installed, logged out, or not
    /// a GitHub remote. Asked once, never again this session - the answer does
    /// not change under us, and retrying every tick would spend a subprocess
    /// per repository forever.
    unavailable: HashSet<String>,
}

impl PrStates {
    fn key(repo_root: &str, branch: &str) -> (String, String) {
        (repo_root.to_string(), branch.to_string())
    }

    pub(crate) fn get(&self, repo_root: &str, branch: &str) -> Option<PullRequest> {
        self.entries
            .get(&Self::key(repo_root, branch))
            .and_then(|cached| cached.value)
    }

    /// Whether this branch is worth asking about right now: not already in
    /// flight, not answered recently, and in a repository `gh` can speak for.
    fn is_stale(&self, repo_root: &str, branch: &str) -> bool {
        if self.unavailable.contains(repo_root) {
            return false;
        }
        let key = Self::key(repo_root, branch);
        if self.inflight.contains(&key) {
            return false;
        }
        self.entries
            .get(&key)
            .is_none_or(|cached| cached.at.elapsed() > TTL)
    }

    fn store(&mut self, repo_root: &str, branch: &str, value: Option<PullRequest>) -> bool {
        let key = Self::key(repo_root, branch);
        self.inflight.remove(&key);
        let changed = self.entries.get(&key).map(|c| c.value) != Some(value);
        self.entries.insert(
            key,
            Cached {
                value,
                at: Instant::now(),
            },
        );
        changed
    }

    fn mark_unavailable(&mut self, repo_root: &str, branch: &str) {
        self.inflight.remove(&Self::key(repo_root, branch));
        self.unavailable.insert(repo_root.to_string());
    }
}

/// Read the pull request of one branch. Blocking: the caller runs it through
/// `smol::unblock`.
///
/// `Err` means `gh` could not answer at all - the repository is then dropped
/// for the session. `Ok(None)` is a real answer: no pull request here.
fn lookup(repo_root: &std::path::Path, branch: &str) -> Result<Option<PullRequest>, ()> {
    let mut cmd = std::process::Command::new("gh");
    // `gh` has no global directory flag - `-C` is rejected outright ("unknown
    // shorthand flag: 'C'"), which used to fail every lookup and blacklist the
    // repository for the session. It resolves the repository from the working
    // directory instead.
    cmd.current_dir(repo_root)
        .args([
            "pr",
            "list",
            "--head",
            branch,
            "--state",
            "all",
            "--json",
            "number,state,isDraft",
            // A branch can carry a history of closed attempts under one open
            // pull request; take enough of them to pick the one that counts.
            "--limit",
            "10",
        ])
        // `gh` prints a survey/update notice on a tty and reads config from
        // the environment; neither matters here, but a pager would hang.
        .env("GH_PAGER", "")
        .env("NO_COLOR", "1");
    let out = paneflow_process::run_with_timeout(cmd, DEADLINE, STDOUT_CAP).map_err(|_| ())?;
    if !out.status.success() {
        // The only trace of a repository being dropped for the session: the
        // caller turns this `Err` into a silent blacklist entry, so without a
        // line here a wrong invocation looks exactly like "no GitHub remote".
        log::debug!(
            "gh pr list failed in {}: {}",
            repo_root.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return Err(());
    }
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| ())?;
    Ok(pick(parsed.as_array().map_or(&[], Vec::as_slice)))
}

/// The pull request that describes the branch, out of everything `gh` returned.
///
/// Open beats merged beats closed: a branch whose old attempt was closed and
/// whose current one is open is, to the person looking at the rail, open. Ties
/// go to the highest number, which is the most recent.
fn pick(rows: &[serde_json::Value]) -> Option<PullRequest> {
    rows.iter()
        .filter_map(|row| {
            let number = row.get("number")?.as_u64()?;
            let draft = row.get("isDraft").and_then(serde_json::Value::as_bool) == Some(true);
            let state = match row.get("state")?.as_str()? {
                "OPEN" if draft => PrState::Draft,
                "OPEN" => PrState::Open,
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ => return None,
            };
            Some(PullRequest { number, state })
        })
        .max_by_key(|pr| {
            let rank = match pr.state {
                PrState::Open => 3,
                PrState::Draft => 2,
                PrState::Merged => 1,
                PrState::Closed => 0,
            };
            (rank, pr.number)
        })
}

impl PaneFlowApp {
    /// The pull request of a branch, if one has been read.
    pub(crate) fn pull_request_for(
        &self,
        repo_root: &std::path::Path,
        branch: &str,
    ) -> Option<PullRequest> {
        self.pr_states.get(&repo_root.to_string_lossy(), branch)
    }

    /// Every `(repository, branch)` the rail is currently showing: each
    /// workspace's own branch, plus the branch of every bound tab.
    fn pr_probe_targets(&self) -> Vec<(std::path::PathBuf, String)> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for ws in &self.workspaces {
            let Some(repo_root) = ws.repo_root.clone() else {
                continue;
            };
            let mut push = |branch: &str, out: &mut Vec<_>| {
                if branch.is_empty() {
                    return;
                }
                if seen.insert((repo_root.clone(), branch.to_string())) {
                    out.push((repo_root.clone(), branch.to_string()));
                }
            };
            if ws.is_git_repo {
                push(&ws.git_branch, &mut out);
            }
            for tab in ws.tabs() {
                if let Some(git) = self.tab_checkout_git(tab) {
                    push(&git.branch.clone(), &mut out);
                }
            }
        }
        out
    }

    /// Refresh the pull requests of every branch the rail shows, off the render
    /// thread. A no-op while the switch is off: nothing is drawn from it, so
    /// nothing justifies the calls.
    pub(crate) fn refresh_pull_requests(&mut self, cx: &mut Context<Self>) {
        if !self.cached_config.sidebar_show.pr_enabled() {
            return;
        }
        let stale: Vec<(std::path::PathBuf, String)> = self
            .pr_probe_targets()
            .into_iter()
            .filter(|(repo_root, branch)| {
                self.pr_states
                    .is_stale(&repo_root.to_string_lossy(), branch)
            })
            .collect();
        for (repo_root, branch) in stale {
            self.pr_states
                .inflight
                .insert(PrStates::key(&repo_root.to_string_lossy(), &branch));
            cx.spawn(
                async move |this: gpui::WeakEntity<Self>, cx: &mut gpui::AsyncApp| {
                    let read = smol::unblock({
                        let repo_root = repo_root.clone();
                        let branch = branch.clone();
                        move || lookup(&repo_root, &branch)
                    })
                    .await;
                    let _ = cx.update(|cx| {
                        this.update(cx, |app: &mut Self, cx: &mut Context<Self>| {
                            let key = repo_root.to_string_lossy();
                            match read {
                                Ok(value) => {
                                    if app.pr_states.store(&key, &branch, value) {
                                        cx.notify();
                                    }
                                }
                                Err(()) => app.pr_states.mark_unavailable(&key, &branch),
                            }
                        })
                    });
                },
            )
            .detach();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PrState, pick};

    fn row(number: u64, state: &str, draft: bool) -> serde_json::Value {
        serde_json::json!({ "number": number, "state": state, "isDraft": draft })
    }

    #[test]
    fn an_open_pull_request_outranks_a_closed_attempt() {
        // The rail answers "is this branch in review?", so a branch whose
        // first attempt was closed and whose second is open reads as open,
        // whatever order `gh` listed them in.
        let picked = pick(&[row(12, "OPEN", false), row(40, "CLOSED", false)]).unwrap();
        assert_eq!(picked.number, 12);
        assert_eq!(picked.state, PrState::Open);
    }

    #[test]
    fn a_draft_is_its_own_state_and_the_newest_wins_a_tie() {
        assert_eq!(pick(&[row(3, "OPEN", true)]).unwrap().state, PrState::Draft);
        let picked = pick(&[row(7, "MERGED", false), row(9, "MERGED", false)]).unwrap();
        assert_eq!(picked.number, 9, "the most recent of equals is the answer");
    }

    #[test]
    fn no_rows_is_an_answer_and_junk_is_not_a_crash() {
        assert!(pick(&[]).is_none());
        assert!(pick(&[serde_json::json!({ "number": 1 })]).is_none());
        assert!(pick(&[row(1, "SOMETHING_NEW", false)]).is_none());
    }
}
