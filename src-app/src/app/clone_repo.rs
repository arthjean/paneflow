use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, ParentElement, PathPromptOptions, SharedString, Styled, Window,
    deferred, div, prelude::*, px, svg,
};

use crate::PaneFlowApp;
use crate::settings::components::{
    MENU_MAX_HEIGHT, select_item_shaped, select_menu_surface, with_alpha,
};
use crate::ui_primitives::quick_pick::{
    FIELD_GAP, FIELD_INSET, ICON_GAP, ICON_SIZE, LABEL_SIZE, PANEL_PADDING, PANEL_RADIUS, ROW_GAP,
    ROW_HEIGHT, ROW_RADIUS, ROW_SPACING, ROW_TEXT_INSET,
};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill};
use crate::ui_primitives::{FilterFieldStyle, filter_field};
use crate::widgets::text_input::TextInput;

const CLONE_PANEL_WIDTH: f32 = 560.;
const CLONE_PANEL_TOP_MARGIN: f32 = 96.;
const TRAILING_SIZE: f32 = 13.;
const GITHUB_ICON: &str = "icons/brand-github.svg";
const CLONE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(600);
const CLONE_OUTPUT_CAP: u64 = 256 * 1024;
const GITHUB_LIST_DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);
const GITHUB_LIST_OUTPUT_CAP: u64 = 1024 * 1024;
const GITHUB_LIST_LIMIT: &str = "200";
const URL_PLACEHOLDER: &str = "Provide repository URL or pick a repository source.";
const GITHUB_PLACEHOLDER: &str = "Repository name (type to search)";
const GITHUB_SOURCE_LABEL: &str = "Clone from GitHub";
const GITHUB_SOURCE_GROUP: &str = "remote sources";
const GITHUB_CLI_HINT: &str =
    "Install the GitHub CLI and run gh auth login to list your repositories";
const CLONE_PHASES: [(&str, f32, f32); 6] = [
    ("Enumerating objects", 0.0, 0.02),
    ("Counting objects", 0.02, 0.05),
    ("Compressing objects", 0.05, 0.10),
    ("Receiving objects", 0.10, 0.80),
    ("Resolving deltas", 0.80, 0.95),
    ("Updating files", 0.95, 1.0),
];

const GH_SIGNED_OUT_MARKER: &str = "gh auth login";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloneTool {
    GitHubCli,
    Git,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CloneProgress {
    pub(crate) phase: &'static str,
    pub(crate) fraction: f32,
    pub(crate) detail: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitHubRepo {
    pub(crate) name_with_owner: String,
    pub(crate) url: String,
}

pub(crate) enum CloneSource {
    Url,
    GitHub {
        loading: bool,
        repos: Vec<GitHubRepo>,
    },
}

pub(crate) struct CloneRepoState {
    pub(crate) url_input: Entity<TextInput>,
    pub(crate) source: CloneSource,
    pub(crate) selected: usize,
    pub(crate) error: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CloneFailure {
    Canceled,
    Failed(String),
}

enum CloneRow {
    GitHubSource,
    Url(String),
    Repo(GitHubRepo),
}

pub(crate) fn reject_unsafe_clone_url(url: &str) -> Option<&'static str> {
    let trimmed = url.trim();
    if trimmed.starts_with('-') {
        return Some("A repository URL cannot start with a dash");
    }
    if trimmed
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("ext:"))
    {
        return Some("The ext transport runs arbitrary commands and is refused");
    }
    None
}

pub(crate) fn repo_name_from_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let tail = trimmed
        .rsplit(['/', ':', '\\'])
        .find(|segment| !segment.is_empty())?;
    let name = tail.strip_suffix(".git").unwrap_or(tail);
    if name.is_empty() || name == "." || name == ".." || name.starts_with('-') {
        return None;
    }
    Some(name.to_string())
}

pub(crate) fn parse_github_repos(stdout: &[u8]) -> Vec<GitHubRepo> {
    let parsed: serde_json::Value = match serde_json::from_slice(stdout) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    parsed
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|row| {
            let name_with_owner = row.get("nameWithOwner")?.as_str()?.to_string();
            let url = row.get("url")?.as_str()?.to_string();
            if name_with_owner.is_empty() || reject_unsafe_clone_url(&url).is_some() {
                return None;
            }
            Some(GitHubRepo {
                name_with_owner,
                url,
            })
        })
        .collect()
}

pub(crate) fn filter_github_repos(repos: &[GitHubRepo], query: &str) -> Vec<GitHubRepo> {
    let query = query.trim().to_lowercase();
    repos
        .iter()
        .filter(|repo| {
            query.is_empty()
                || query
                    .split_whitespace()
                    .all(|word| repo.name_with_owner.to_lowercase().contains(word))
        })
        .cloned()
        .collect()
}

fn source_row_matches(query: &str) -> bool {
    let haystack = GITHUB_SOURCE_LABEL.to_lowercase();
    query
        .split_whitespace()
        .all(|word| haystack.contains(&word.to_lowercase()))
}

pub(crate) fn parse_clone_progress(line: &str) -> Option<CloneProgress> {
    let line = line.trim();
    let line = line.strip_prefix("remote:").map_or(line, str::trim);
    let (label, rest) = line.split_once(':')?;
    let label = match label.trim() {
        "Checking out files" => "Updating files",
        other => other,
    };
    let (phase, low, high) = CLONE_PHASES
        .iter()
        .copied()
        .find(|(name, _, _)| *name == label)?;
    let rest = rest.trim();
    let percent = rest
        .split('%')
        .next()
        .and_then(|head| head.trim().parse::<f32>().ok())
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    let detail = rest
        .split_once(')')
        .map_or("", |(_, tail)| tail)
        .trim()
        .trim_start_matches(',')
        .trim()
        .trim_end_matches("done.")
        .trim()
        .trim_end_matches(',')
        .trim()
        .replace(" | ", " · ");
    Some(CloneProgress {
        phase,
        fraction: low + (high - low) * percent / 100.0,
        detail: (!detail.is_empty()).then_some(detail),
    })
}

fn split_progress_lines(carry: &mut Vec<u8>, chunk: &[u8], mut emit: impl FnMut(&str)) {
    for byte in chunk {
        if *byte == b'\r' || *byte == b'\n' {
            if !carry.is_empty() {
                emit(&String::from_utf8_lossy(carry));
                carry.clear();
            }
        } else {
            carry.push(*byte);
        }
    }
}

fn is_github_shorthand(target: &str) -> bool {
    let Some((owner, name)) = target.split_once('/') else {
        return false;
    };
    let segment_ok = |segment: &str| {
        !segment.is_empty()
            && segment != "."
            && segment != ".."
            && segment
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
    };
    segment_ok(owner) && segment_ok(name)
}

fn github_host_of(target: &str) -> bool {
    let lower = target.to_ascii_lowercase();
    let rest = lower
        .split_once("://")
        .map_or(lower.as_str(), |(_, rest)| rest);
    let rest = rest.rsplit_once('@').map_or(rest, |(_, rest)| rest);
    rest.strip_prefix("github.com")
        .is_some_and(|tail| tail.starts_with('/') || tail.starts_with(':'))
}

pub(crate) fn clone_tool_for(target: &str) -> CloneTool {
    let target = target.trim();
    if is_github_shorthand(target) || github_host_of(target) {
        CloneTool::GitHubCli
    } else {
        CloneTool::Git
    }
}

pub(crate) fn git_target_for(target: &str) -> String {
    let target = target.trim();
    if is_github_shorthand(target) {
        format!("https://github.com/{target}.git")
    } else {
        target.to_string()
    }
}

fn clone_command(
    tool: CloneTool,
    target: &str,
    destination: &Path,
    space_env: &[(String, String)],
) -> std::process::Command {
    let mut command = match tool {
        CloneTool::GitHubCli => {
            let mut command = std::process::Command::new("gh");
            command
                .arg("repo")
                .arg("clone")
                .arg(target)
                .arg(destination)
                .arg("--")
                .arg("--progress")
                .env("GH_PAGER", "")
                .env("GH_PROMPT_DISABLED", "1")
                .env("NO_COLOR", "1");
            command
        }
        CloneTool::Git => {
            let mut command = crate::git_command::git(
                crate::git_command::GitProfile::UserAction,
                ["clone", "--progress", "--"],
            );
            command.arg(git_target_for(target)).arg(destination);
            command
        }
    };
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.envs(space_env.iter().map(|(key, value)| (key, value)));
    command
}

fn run_clone_command(
    command: std::process::Command,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(CloneProgress) + Send + 'static,
) -> Result<paneflow_process::BoundedOutput, paneflow_process::ProcError> {
    let mut carry = Vec::new();
    if command.get_program() == "git" {
        crate::git_command::record_spawn(&command);
    }
    paneflow_process::run_cancelable_tapping_stderr(
        command,
        CLONE_DEADLINE,
        CLONE_OUTPUT_CAP,
        move |chunk| {
            split_progress_lines(&mut carry, chunk, |line| {
                if let Some(progress) = parse_clone_progress(line) {
                    on_progress(progress);
                }
            });
        },
        cancel,
    )
}

fn gh_is_unusable(
    result: &Result<paneflow_process::BoundedOutput, paneflow_process::ProcError>,
) -> bool {
    match result {
        Err(paneflow_process::ProcError::Spawn(_)) => true,
        Ok(output) => {
            !output.status.success()
                && String::from_utf8_lossy(&output.stderr).contains(GH_SIGNED_OUT_MARKER)
        }
        Err(_) => false,
    }
}

pub(crate) fn run_clone(
    target: &str,
    destination: &Path,
    space_env: &[(String, String)],
    cancel: &AtomicBool,
    on_progress: impl FnMut(CloneProgress) + Send + Clone + 'static,
) -> Result<(), CloneFailure> {
    let tool = clone_tool_for(target);
    let mut result = run_clone_command(
        clone_command(tool, target, destination, space_env),
        cancel,
        on_progress.clone(),
    );
    if tool == CloneTool::GitHubCli && gh_is_unusable(&result) && !cancel.load(Ordering::Relaxed) {
        log::info!("gh is missing or signed out; cloning {target} with git");
        result = run_clone_command(
            clone_command(CloneTool::Git, target, destination, space_env),
            cancel,
            on_progress,
        );
    }
    let output = match result {
        Ok(output) => output,
        Err(paneflow_process::ProcError::Canceled) => return Err(CloneFailure::Canceled),
        Err(error) => return Err(CloneFailure::Failed(error.to_string())),
    };
    if output.status.success() {
        return Ok(());
    }
    Err(CloneFailure::Failed(clone_failure_message(
        &String::from_utf8_lossy(&output.stderr),
    )))
}

pub(crate) fn clone_failure_message(stderr: &str) -> String {
    let lines: Vec<&str> = stderr
        .split(['\r', '\n'])
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let prefixed = |prefix: &str| {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(prefix))
            .map(str::trim)
    };
    prefixed("error:")
        .or_else(|| prefixed("fatal:"))
        .or_else(|| {
            lines.iter().copied().rfind(|line| {
                !line.starts_with("Cloning into") && parse_clone_progress(line).is_none()
            })
        })
        .unwrap_or("git clone failed")
        .to_string()
}

fn list_github_repos(space_env: &[(String, String)]) -> Result<Vec<GitHubRepo>, String> {
    let mut command = std::process::Command::new("gh");
    command
        .envs(space_env.iter().map(|(key, value)| (key, value)))
        .args([
            "repo",
            "list",
            "--json",
            "nameWithOwner,url",
            "--limit",
            GITHUB_LIST_LIMIT,
        ])
        .env("GH_PAGER", "")
        .env("NO_COLOR", "1");
    let output =
        paneflow_process::run_with_timeout(command, GITHUB_LIST_DEADLINE, GITHUB_LIST_OUTPUT_CAP)
            .map_err(|_| GITHUB_CLI_HINT.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr
            .lines()
            .map(str::trim)
            .rfind(|line| !line.is_empty())
            .unwrap_or(GITHUB_CLI_HINT)
            .to_string();
        return Err(message);
    }
    Ok(parse_github_repos(&output.stdout))
}

impl PaneFlowApp {
    pub(crate) fn open_clone_repo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.clone_repo.is_some() {
            return;
        }
        self.dismiss_transient_surfaces();
        let url_input = cx.new(|cx| TextInput::new("", URL_PLACEHOLDER, cx));
        let focus = url_input.read(cx).focus_handle.clone();
        self.clone_repo = Some(CloneRepoState {
            url_input,
            source: CloneSource::Url,
            selected: 0,
            error: None,
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    pub(crate) fn handle_clone_repository(
        &mut self,
        _: &crate::CloneRepository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_clone_repo(window, cx);
    }

    pub(crate) fn close_clone_repo(&mut self, cx: &mut Context<Self>) {
        self.clone_repo = None;
        cx.notify();
    }

    fn clone_repo_set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        if let Some(clone) = self.clone_repo.as_mut() {
            if let CloneSource::GitHub { loading, .. } = &mut clone.source {
                *loading = false;
            }
            clone.error = Some(message.into());
            cx.notify();
        }
    }

    fn clone_repo_rows(&self, cx: &Context<Self>) -> Vec<CloneRow> {
        let Some(clone) = self.clone_repo.as_ref() else {
            return Vec::new();
        };
        let query = clone.url_input.read(cx).value();
        let query = query.trim();
        match &clone.source {
            CloneSource::Url => {
                if query.is_empty() {
                    vec![CloneRow::GitHubSource]
                } else if source_row_matches(query) {
                    vec![CloneRow::GitHubSource, CloneRow::Url(query.to_string())]
                } else {
                    vec![CloneRow::Url(query.to_string())]
                }
            }
            CloneSource::GitHub { repos, .. } => filter_github_repos(repos, query)
                .into_iter()
                .map(CloneRow::Repo)
                .collect(),
        }
    }

    fn clone_repo_move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let len = self.clone_repo_rows(cx).len();
        let Some(clone) = self.clone_repo.as_mut() else {
            return;
        };
        if len == 0 {
            clone.selected = 0;
        } else {
            let current = clone.selected.min(len - 1) as isize;
            clone.selected = (current + delta).rem_euclid(len as isize) as usize;
        }
        cx.notify();
    }

    fn clone_repo_pick_github(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clone) = self.clone_repo.as_mut() else {
            return;
        };
        let url_input = cx.new(|cx| TextInput::new("", GITHUB_PLACEHOLDER, cx));
        let focus = url_input.read(cx).focus_handle.clone();
        clone.url_input = url_input;
        clone.source = CloneSource::GitHub {
            loading: true,
            repos: Vec::new(),
        };
        clone.selected = 0;
        clone.error = None;
        window.focus(&focus, cx);
        cx.notify();

        let space_env =
            crate::space_env::subprocess_env(&self.cached_config, self.active_space.as_ref());
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let result = smol::unblock(move || list_github_repos(&space_env)).await;
            let _ = this.update(cx, |app, cx| {
                let Some(clone) = app.clone_repo.as_mut() else {
                    return;
                };
                let CloneSource::GitHub { loading, repos } = &mut clone.source else {
                    return;
                };
                *loading = false;
                match result {
                    Ok(listed) => *repos = listed,
                    Err(message) => clone.error = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clone_repo_back_to_sources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clone) = self.clone_repo.as_mut() else {
            return;
        };
        let url_input = cx.new(|cx| TextInput::new("", URL_PLACEHOLDER, cx));
        let focus = url_input.read(cx).focus_handle.clone();
        clone.url_input = url_input;
        clone.source = CloneSource::Url;
        clone.selected = 0;
        clone.error = None;
        window.focus(&focus, cx);
        cx.notify();
    }

    fn clone_repo_activate(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.clone_repo.is_none() {
            return;
        }
        let rows = self.clone_repo_rows(cx);
        let Some(row) = rows.into_iter().nth(idx) else {
            return;
        };
        match row {
            CloneRow::GitHubSource => self.clone_repo_pick_github(window, cx),
            CloneRow::Url(url) => self.clone_repo_start(url, window, cx),
            CloneRow::Repo(repo) => self.clone_repo_start(repo.url, window, cx),
        }
    }

    pub(crate) fn clone_repo_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(clone) = self.clone_repo.as_ref() else {
            return;
        };
        let typed = clone.url_input.read(cx).value().trim().to_string();
        let selected = clone.selected;
        if matches!(clone.source, CloneSource::Url) && !typed.is_empty() {
            self.clone_repo_start(typed, window, cx);
            return;
        }
        self.clone_repo_activate(selected, window, cx);
    }

    fn clone_repo_start(&mut self, url: String, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(reason) = reject_unsafe_clone_url(&url) {
            self.clone_repo_set_error(reason, cx);
            return;
        }
        let Some(name) = repo_name_from_url(&url) else {
            self.clone_repo_set_error("Enter a repository URL first", cx);
            return;
        };

        if self.clone_job_is_running() {
            self.clone_repo_set_error("A clone is already in progress", cx);
            return;
        }

        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Clone into".into()),
        });
        if let Some(clone) = self.clone_repo.as_mut() {
            clone.error = None;
        }
        cx.notify();

        cx.spawn_in(window, async move |this: gpui::WeakEntity<Self>, cx| {
            let picked = match receiver.await {
                Ok(Ok(Some(mut paths))) if !paths.is_empty() => paths.pop(),
                _ => None,
            };
            let Some(parent) = picked else {
                return;
            };
            let destination = parent.join(&name);
            let exists = {
                let probe = destination.clone();
                smol::unblock(move || probe.exists()).await
            };
            let _ = this.update_in(cx, |app, window, cx| {
                if exists {
                    app.clone_repo_set_error(
                        format!("{} already exists", destination.display()),
                        cx,
                    );
                    return;
                }
                app.begin_clone_job(url, name, destination, window, cx);
            });
        })
        .detach();
    }

    pub(crate) fn handle_clone_repo_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event.keystroke.key.as_str() {
            "escape" => {
                let in_github = self
                    .clone_repo
                    .as_ref()
                    .is_some_and(|clone| matches!(clone.source, CloneSource::GitHub { .. }));
                if in_github {
                    self.clone_repo_back_to_sources(window, cx);
                } else {
                    self.close_clone_repo(cx);
                }
            }
            "enter" => self.clone_repo_confirm(window, cx),
            "up" => self.clone_repo_move_selection(-1, cx),
            "down" => self.clone_repo_move_selection(1, cx),
            _ => {}
        }
    }

    fn render_clone_row(
        &self,
        idx: usize,
        row: &CloneRow,
        selected: bool,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (label, trailing): (String, Option<&'static str>) = match row {
            CloneRow::GitHubSource => (GITHUB_SOURCE_LABEL.to_string(), Some(GITHUB_SOURCE_GROUP)),
            CloneRow::Url(url) => (format!("Clone {url}"), Some("repository URL")),
            CloneRow::Repo(repo) => (repo.name_with_owner.clone(), None),
        };
        let label_color = if selected {
            crate::theme::on_selection_color()
        } else {
            ui.text
        };
        let muted_color = if selected {
            crate::theme::on_selection_muted()
        } else {
            ui.muted
        };
        select_item_shaped(
            SharedString::from(format!("clone-repo-row-{idx}")),
            selected,
            ui,
            crate::theme::selection_color(),
            ROW_RADIUS,
        )
        .h(px(ROW_HEIGHT))
        .px(px(ROW_TEXT_INSET))
        .gap(px(ROW_GAP))
        .text_size(px(LABEL_SIZE))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.clone_repo_activate(idx, window, cx);
            cx.stop_propagation();
        }))
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(ICON_GAP))
                .child(
                    svg()
                        .flex_none()
                        .size(px(ICON_SIZE))
                        .path(GITHUB_ICON)
                        .text_color(with_alpha(label_color, 0.8)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .overflow_x_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(label_color)
                        .child(label),
                ),
        )
        .when_some(trailing, |row, group| {
            row.child(
                div()
                    .relative()
                    .flex_none()
                    .text_size(px(TRAILING_SIZE))
                    .text_color(muted_color)
                    .child(group),
            )
        })
        .into_any_element()
    }

    fn render_clone_field(
        &self,
        url_input: Entity<TextInput>,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let focus = url_input.read(cx).focus_handle.clone();
        let focused = focus.is_focused(window);
        let has_query = !url_input.read(cx).value().is_empty();
        let clear_target = url_input.clone();
        filter_field(
            "clone-repo-field",
            "clone-repo-field-clear",
            "Repository URL",
            ui,
            FilterFieldStyle::palette(),
            focused,
            has_query,
            true,
            None,
            url_input,
            move |_: &ClickEvent, window: &mut Window, cx: &mut gpui::App| {
                cx.stop_propagation();
                clear_target.update(cx, |input, cx| input.clear(cx));
                let focus = clear_target.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
            },
        )
        .flex_none()
        .mx(px(FIELD_INSET))
        .mb(px(FIELD_GAP))
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&focus, cx);
            cx.stop_propagation();
        })
        .into_any_element()
    }

    pub(crate) fn render_clone_repo(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(clone) = self.clone_repo.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let rows = self.clone_repo_rows(cx);
        let selected = clone.selected.min(rows.len().saturating_sub(1));
        let loading = matches!(clone.source, CloneSource::GitHub { loading: true, .. });
        let url_input = clone.url_input.clone();
        let error = clone.error.clone();

        let mut list = div()
            .id("clone-repo-list")
            .flex()
            .flex_col()
            .gap(px(ROW_SPACING))
            .max_h(MENU_MAX_HEIGHT)
            .overflow_y_scroll();

        let status_row = |text: String, color: gpui::Hsla| {
            div()
                .min_h(px(ROW_HEIGHT))
                .px(px(ROW_TEXT_INSET))
                .py(px(6.))
                .flex()
                .items_center()
                .text_size(px(LABEL_SIZE))
                .text_color(color)
                .child(text)
        };

        if let Some(error) = error {
            list = list.child(status_row(error, ui.vc_deleted));
        } else if loading {
            list = list.child(status_row(
                "Loading your GitHub repositories…".to_string(),
                ui.muted,
            ));
        } else if rows.is_empty() {
            list = list.child(status_row("No matching repository".to_string(), ui.muted));
        } else {
            for (idx, row) in rows.iter().enumerate() {
                list = list.child(self.render_clone_row(idx, row, idx == selected, ui, cx));
            }
        }

        let panel = div()
            .id("clone-repo")
            .relative()
            .child(squircle_fill(PANEL_RADIUS, select_menu_surface(ui)))
            .child(squircle_border(
                PANEL_RADIUS,
                px(1.),
                with_alpha(ui.border, 0.6),
            ))
            .flex()
            .flex_col()
            .p(px(PANEL_PADDING))
            .w(px(CLONE_PANEL_WIDTH))
            .occlude()
            .track_focus(&self.clone_repo_focus)
            .on_key_down(cx.listener(Self::handle_clone_repo_key_down))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_clone_repo(cx);
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .child(self.render_clone_field(url_input, ui, window, cx))
            .child(list);

        deferred(
            div()
                .id("clone-repo-backdrop")
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(CLONE_PANEL_TOP_MARGIN))
                .child(crate::ui_primitives::menu_reveal(
                    "clone-repo-reveal",
                    panel,
                )),
        )
        .with_priority(7)
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(command: &std::process::Command, key: &str) -> Option<String> {
        command
            .get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value.map(|value| value.to_string_lossy().into_owned()))
    }

    #[test]
    fn a_clone_runs_under_the_active_space_allowlist_for_gh_and_git() {
        const GH_DIR: &str = if cfg!(windows) {
            r"C:\work\gh"
        } else {
            "/work/gh"
        };
        let mut config = paneflow_config::schema::PaneFlowConfig::default();
        let work = paneflow_config::schema::SpaceId::new();
        config.spaces.push(paneflow_config::schema::SpaceConfig {
            id: work.clone(),
            name: "Work".to_string(),
            color: None,
            avatar: None,
            env: [
                ("GIT_SSH_COMMAND", "ssh -i ~/.ssh/work"),
                ("GH_CONFIG_DIR", GH_DIR),
                ("NOT_ALLOWLISTED", "leak"),
            ]
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        });
        let space_env = crate::space_env::subprocess_env(&config, Some(&work));
        let destination = std::path::Path::new("/tmp/clone-target");
        for tool in [CloneTool::GitHubCli, CloneTool::Git] {
            let command = clone_command(tool, "owner/repo", destination, &space_env);
            assert_eq!(env_of(&command, "GH_CONFIG_DIR").as_deref(), Some(GH_DIR));
            assert_eq!(
                env_of(&command, "GIT_SSH_COMMAND").as_deref(),
                Some("ssh -i ~/.ssh/work")
            );
            assert_eq!(env_of(&command, "NOT_ALLOWLISTED"), None);
            let default = clone_command(
                tool,
                "owner/repo",
                destination,
                &crate::space_env::subprocess_env(&config, None),
            );
            assert_eq!(env_of(&default, "GH_CONFIG_DIR"), None);
            assert_eq!(
                default.get_envs().count(),
                clone_command(tool, "owner/repo", destination, &[])
                    .get_envs()
                    .count(),
                "the default space clones with exactly today's env"
            );
        }
    }

    #[test]
    fn repo_name_reads_an_ssh_remote() {
        assert_eq!(
            repo_name_from_url("git@github.com:arthjean/paneflow.git"),
            Some("paneflow".to_string())
        );
    }

    #[test]
    fn repo_name_reads_an_https_remote_with_a_trailing_slash() {
        assert_eq!(
            repo_name_from_url("https://github.com/arthjean/paneflow/"),
            Some("paneflow".to_string())
        );
    }

    #[test]
    fn repo_name_rejects_an_empty_url() {
        assert_eq!(repo_name_from_url("   "), None);
    }

    #[test]
    fn repo_name_rejects_a_traversal_tail() {
        assert_eq!(repo_name_from_url("https://example.com/repo/.."), None);
        assert_eq!(repo_name_from_url("https://example.com/repo/."), None);
    }

    #[test]
    fn repo_name_rejects_a_windows_traversal_tail() {
        assert_eq!(repo_name_from_url("C:\\repos\\.."), None);
    }

    #[test]
    fn an_option_shaped_url_is_refused() {
        assert!(reject_unsafe_clone_url("--upload-pack=touch /tmp/pwned").is_some());
        assert!(reject_unsafe_clone_url("  -c core.pager=sh").is_some());
    }

    #[test]
    fn the_ext_transport_is_refused_in_any_case() {
        assert!(reject_unsafe_clone_url("ext::sh -c whoami").is_some());
        assert!(reject_unsafe_clone_url("EXT::sh -c whoami").is_some());
    }

    #[test]
    fn an_ordinary_remote_is_accepted() {
        assert!(reject_unsafe_clone_url("git@github.com:arthjean/paneflow.git").is_none());
        assert!(reject_unsafe_clone_url("https://github.com/arthjean/paneflow").is_none());
    }

    #[test]
    fn github_targets_clone_through_gh_and_the_rest_through_git() {
        assert_eq!(clone_tool_for("arthjean/paneflow"), CloneTool::GitHubCli);
        assert_eq!(
            clone_tool_for("https://github.com/arthjean/paneflow"),
            CloneTool::GitHubCli
        );
        assert_eq!(
            clone_tool_for("git@github.com:arthjean/paneflow.git"),
            CloneTool::GitHubCli
        );
        assert_eq!(
            clone_tool_for("ssh://git@github.com/arthjean/paneflow.git"),
            CloneTool::GitHubCli
        );
        assert_eq!(
            clone_tool_for("https://gitlab.com/arthjean/paneflow"),
            CloneTool::Git
        );
        assert_eq!(
            clone_tool_for("https://github.com.evil.example/x/y"),
            CloneTool::Git
        );
        assert_eq!(clone_tool_for("C:\\repos\\paneflow"), CloneTool::Git);
        assert_eq!(clone_tool_for("../paneflow"), CloneTool::Git);
        assert_eq!(clone_tool_for("paneflow"), CloneTool::Git);
    }

    #[test]
    fn the_git_fallback_expands_a_shorthand_to_an_https_remote() {
        assert_eq!(
            git_target_for("arthjean/paneflow"),
            "https://github.com/arthjean/paneflow.git"
        );
        assert_eq!(
            git_target_for("git@github.com:arthjean/paneflow.git"),
            "git@github.com:arthjean/paneflow.git"
        );
    }

    #[test]
    fn a_failed_checkout_reports_the_first_error_line() {
        let stderr = "Cloning into 'ghostty'...\rReceiving objects: 100% (10/10), done.\nerror: invalid path 'src/a:b.txt'\nfatal: unable to checkout working tree\nwarning: Clone succeeded, but checkout failed.\nand retry with 'git restore --source=HEAD :/'\n";
        assert_eq!(clone_failure_message(stderr), "invalid path 'src/a:b.txt'");
    }

    #[test]
    fn a_refused_remote_reports_the_fatal_line() {
        assert_eq!(
            clone_failure_message(
                "Cloning into 'x'...\nfatal: repository 'https://example.com/x/' not found\n"
            ),
            "repository 'https://example.com/x/' not found"
        );
        assert_eq!(clone_failure_message(""), "git clone failed");
    }

    #[test]
    fn progress_weights_received_objects_into_the_wide_band() {
        let progress =
            parse_clone_progress("Receiving objects:  50% (555/1110), 2.3 MiB | 1.2 MiB/s")
                .unwrap();
        assert_eq!(progress.phase, "Receiving objects");
        assert!((progress.fraction - 0.45).abs() < 0.001);
        assert_eq!(progress.detail.as_deref(), Some("2.3 MiB · 1.2 MiB/s"));
    }

    #[test]
    fn progress_reads_remote_phases_and_drops_the_done_marker() {
        let progress =
            parse_clone_progress("remote: Compressing objects: 100% (500/500), done.").unwrap();
        assert_eq!(progress.phase, "Compressing objects");
        assert!((progress.fraction - 0.10).abs() < 0.001);
        assert_eq!(progress.detail, None);
        assert_eq!(
            parse_clone_progress("remote: Enumerating objects: 1234, done.")
                .unwrap()
                .fraction,
            0.0
        );
        assert!(parse_clone_progress("Cloning into 'paneflow'...").is_none());
    }

    #[test]
    fn progress_lines_split_on_carriage_returns_across_chunks() {
        let mut carry = Vec::new();
        let mut seen = Vec::new();
        split_progress_lines(
            &mut carry,
            b"Resolving deltas:  10% (1/10)\rResolving del",
            |line| seen.push(line.to_string()),
        );
        split_progress_lines(&mut carry, b"tas: 100% (10/10), done.\n", |line| {
            seen.push(line.to_string())
        });
        assert_eq!(
            seen,
            vec![
                "Resolving deltas:  10% (1/10)".to_string(),
                "Resolving deltas: 100% (10/10), done.".to_string(),
            ]
        );
    }

    #[test]
    fn github_rows_parse_name_and_url() {
        let rows = parse_github_repos(
            br#"[{"nameWithOwner":"arthjean/paneflow","url":"https://github.com/arthjean/paneflow"},{"nameWithOwner":"x/y","url":"--upload-pack=sh"}]"#,
        );
        assert_eq!(
            rows,
            vec![GitHubRepo {
                name_with_owner: "arthjean/paneflow".to_string(),
                url: "https://github.com/arthjean/paneflow".to_string(),
            }]
        );
    }

    #[test]
    fn github_rows_filter_on_every_query_word() {
        let repos = parse_github_repos(
            br#"[{"nameWithOwner":"arthjean/paneflow","url":"a"},{"nameWithOwner":"arthjean/paneflow-web","url":"b"}]"#,
        );
        let hits = filter_github_repos(&repos, "pane web");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name_with_owner, "arthjean/paneflow-web");
        assert_eq!(filter_github_repos(&repos, "").len(), 2);
    }
}
