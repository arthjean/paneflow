use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{Context, Hsla, Rgba};

use crate::PaneFlowApp;

const TTL: Duration = Duration::from_secs(300);
const DEADLINE: Duration = Duration::from_secs(15);
const STDOUT_CAP: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrState {
    Draft,
    Open,
    Merged,
    Closed,
}

impl PrState {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PullRequest {
    pub number: u64,
    pub state: PrState,
}

struct Cached {
    value: Option<PullRequest>,
    at: Instant,
}

#[derive(Default)]
pub(crate) struct PrStates {
    entries: HashMap<(String, String), Cached>,
    inflight: HashSet<(String, String)>,
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

fn lookup(repo_root: &std::path::Path, branch: &str) -> Result<Option<PullRequest>, ()> {
    let mut cmd = std::process::Command::new("gh");
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
            "--limit",
            "10",
        ])
        .env("GH_PAGER", "")
        .env("NO_COLOR", "1");
    let out = paneflow_process::run_with_timeout(cmd, DEADLINE, STDOUT_CAP).map_err(|_| ())?;
    if !out.status.success() {
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
    pub(crate) fn pull_request_for(
        &self,
        repo_root: &std::path::Path,
        branch: &str,
    ) -> Option<PullRequest> {
        self.pr_states.get(&repo_root.to_string_lossy(), branch)
    }

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
