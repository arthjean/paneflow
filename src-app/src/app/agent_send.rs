use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Entity, Focusable, IntoElement, Pixels, Point, SharedString,
    WeakEntity, Window, px, rgb,
};

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::app::diff_dock::code::view::CodeView;
use crate::app::files_tree::workspace_relative_path;
use crate::app::sidebar::agents::{agent_candidates, order_agents};
use crate::terminal::{Prefill, TerminalView};
use crate::ui_primitives::action_menu::{ActionMenu, MenuGlyph, MenuItem};
use crate::workspace::Workspace;

pub(crate) const SEND_TO_AGENT: &str = "Send to Agent";
pub(crate) const NO_AGENT: &str = "No agent in this workspace";
pub(crate) const AGENT_GONE: &str = "This agent's terminal is no longer open";
pub(crate) const COMMENT_MAX_CHARS: usize = 16_000;
pub(crate) const QUOTED_MAX_CHARS: usize = 32_000;
pub(crate) const TRUNCATION_MARK: &str = "… truncated …";
const SEND_ICON: &str = "icons/menu/terminal.svg";
const LOGO_SIZE: f32 = 14.0;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentTarget {
    pub(crate) surface_id: u64,
    pub(crate) tool: TerminalAgent,
    pub(crate) title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MentionStyle {
    AtQuoted,
    AtEscaped,
    Plain,
}

fn mention_style(tool: TerminalAgent) -> MentionStyle {
    if tool == TerminalAgent::ClaudeCode || tool == TerminalAgent::Codex {
        MentionStyle::AtQuoted
    } else if tool == TerminalAgent::Gemini {
        MentionStyle::AtEscaped
    } else {
        MentionStyle::Plain
    }
}

fn double_quoted(path: &str) -> String {
    if path.chars().any(char::is_whitespace) || path.contains('"') {
        format!("\"{}\"", path.replace('"', "\\\""))
    } else {
        path.to_owned()
    }
}

fn backslash_escaped(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for ch in path.chars() {
        if ch.is_whitespace() {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

pub(crate) fn file_mention(tool: TerminalAgent, root: &Path, path: &Path) -> String {
    let relative = workspace_relative_path(root, path);
    match mention_style(tool) {
        MentionStyle::AtQuoted => format!("@{} ", double_quoted(&relative)),
        MentionStyle::AtEscaped => format!("@{} ", backslash_escaped(&relative)),
        MentionStyle::Plain => format!("{} ", double_quoted(&relative)),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RangeComment {
    pub(crate) path: PathBuf,
    pub(crate) first_line: usize,
    pub(crate) last_line: usize,
    pub(crate) text: String,
    pub(crate) quoted: String,
}

pub(crate) fn range_label(first_line: usize, last_line: usize) -> String {
    if last_line > first_line {
        format!("L{first_line}-L{last_line}")
    } else {
        format!("L{first_line}")
    }
}

fn truncated(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        None => text.to_owned(),
        Some((cut, _)) => format!("{}\n{TRUNCATION_MARK}", text[..cut].trim_end_matches('\n')),
    }
}

fn code_fence(body: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for ch in body.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

pub(crate) fn comment_prompt(root: &Path, comment: &RangeComment) -> String {
    let relative = workspace_relative_path(root, &comment.path);
    let location = if comment.last_line > comment.first_line {
        format!("{relative}:{}-{}", comment.first_line, comment.last_line)
    } else {
        format!("{relative}:{}", comment.first_line)
    };
    let text = truncated(comment.text.trim(), COMMENT_MAX_CHARS);
    let quoted = truncated(comment.quoted.trim_end_matches('\n'), QUOTED_MAX_CHARS);
    let language = comment
        .path
        .extension()
        .map(|extension| extension.to_string_lossy().into_owned())
        .unwrap_or_default();
    let fence = code_fence(&quoted);
    format!("{location}\n\n{text}\n\n{fence}{language}\n{quoted}\n{fence}")
}

pub(crate) fn workspace_agent_targets(
    workspaces: &[Workspace],
    workspace_idx: usize,
    cx: &App,
) -> Vec<AgentTarget> {
    order_agents(agent_candidates(workspaces, None, cx), false)
        .into_iter()
        .filter(|candidate| candidate.workspace_idx == workspace_idx)
        .filter_map(|candidate| {
            let workspace = workspaces.get(candidate.workspace_idx)?;
            let session = workspace.agent_sessions.get(&candidate.session_key)?;
            let tab = workspace.tabs().get(candidate.tab_idx)?;
            let title = if tab.title().trim().is_empty() {
                session.tool.display_name().to_string()
            } else {
                tab.title().to_string()
            };
            Some(AgentTarget {
                surface_id: candidate.surface_id,
                tool: session.tool,
                title,
            })
        })
        .collect()
}

fn agent_logo(tool: TerminalAgent) -> MenuGlyph {
    MenuGlyph::Element(Rc::new(move || {
        let ui = crate::theme::ui_colors();
        let tint = tool
            .accent()
            .map_or(ui.text, |tint| gpui::Hsla::from(rgb(tint)));
        crate::settings::components::render_logo(
            tool.icon_path(),
            tool.icon_multicolor(),
            px(LOGO_SIZE),
            tint,
        )
        .into_any_element()
    }))
}

pub(crate) fn agent_choices<A: Clone + 'static>(
    targets: &[AgentTarget],
    remembered: Option<u64>,
    action: impl Fn(u64) -> A,
) -> ActionMenu<A> {
    let mut ordered: Vec<&AgentTarget> = targets.iter().collect();
    if let Some(position) = ordered
        .iter()
        .position(|target| Some(target.surface_id) == remembered)
    {
        let kept = ordered.remove(position);
        ordered.insert(0, kept);
    }
    ActionMenu::new().items(ordered.into_iter().map(|target| {
        MenuItem::new(
            SharedString::from(target.title.clone()),
            action(target.surface_id),
        )
        .id(format!("send-to-agent-{}", target.surface_id))
        .glyph(Some(agent_logo(target.tool)))
        .detail(target.tool.display_name())
    }))
}

pub(crate) fn send_to_agent_item<A: Clone + 'static>(
    targets: &[AgentTarget],
    action: impl Fn(u64) -> A,
) -> MenuItem<A> {
    let item = match targets {
        [] => MenuItem::disabled(NO_AGENT),
        [only] => MenuItem::new(SEND_TO_AGENT, action(only.surface_id)),
        many => MenuItem::submenu(SEND_TO_AGENT, agent_choices(many, None, action)),
    };
    item.icon(SEND_ICON).mnemonic('s')
}

#[derive(Clone)]
pub(crate) struct CommentOffer {
    origin: WeakEntity<CodeView>,
    comment: RangeComment,
    position: Point<Pixels>,
}

impl PaneFlowApp {
    pub(crate) fn agent_targets(&self, cx: &App) -> Vec<AgentTarget> {
        self.shown_workspace_idx()
            .map(|idx| workspace_agent_targets(&self.workspaces, idx, cx))
            .unwrap_or_default()
    }

    pub(crate) fn offer_comment_to_agents(
        &mut self,
        origin: WeakEntity<CodeView>,
        comment: RangeComment,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let targets = self.agent_targets(cx);
        match targets.as_slice() {
            [] => self.show_toast(NO_AGENT, cx),
            [only] => {
                let root = self.diff_tab_root();
                self.send_comment_to_agent(only.surface_id, origin, &root, &comment, window, cx);
            }
            _ => {
                self.dismiss_transient_surfaces();
                self.comment_offer = Some(CommentOffer {
                    origin,
                    comment,
                    position,
                });
                cx.notify();
            }
        }
    }

    pub(crate) fn render_comment_agent_menu(
        &self,
        offer: CommentOffer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let menu = agent_choices(
            &self.agent_targets(cx),
            self.remembered_comment_agent(),
            |surface_id| surface_id,
        );
        let handlers = self.menu_handlers(
            cx,
            move |this, surface_id: &u64, window, cx| {
                this.comment_offer = None;
                let root = this.diff_tab_root();
                this.send_comment_to_agent(
                    *surface_id,
                    offer.origin.clone(),
                    &root,
                    &offer.comment,
                    window,
                    cx,
                );
            },
            |this| this.comment_offer = None,
        );
        crate::app::sidebar::context_menu::render_context_menu(
            SharedString::from("comment-agent-menu"),
            menu,
            offer.position,
            handlers,
            window,
            cx,
        )
    }

    pub(crate) fn remembered_comment_agent(&self) -> Option<u64> {
        let workspace = self.active_workspace()?;
        self.comment_agents.get(&workspace.id).copied()
    }

    pub(crate) fn send_file_to_agent(
        &mut self,
        surface_id: u64,
        root: &Path,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self
            .agent_targets(cx)
            .into_iter()
            .find(|target| target.surface_id == surface_id)
        else {
            self.show_toast(AGENT_GONE, cx);
            return;
        };
        let text = file_mention(target.tool, root, path);
        self.prefill_agent(surface_id, &text, None, window, cx);
    }

    pub(crate) fn send_comment_to_agent(
        &mut self,
        surface_id: u64,
        origin: WeakEntity<CodeView>,
        root: &Path,
        comment: &RangeComment,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(workspace) = self.active_workspace() {
            self.comment_agents.insert(workspace.id, surface_id);
        }
        let text = comment_prompt(root, comment);
        self.prefill_agent(surface_id, &text, Some(origin), window, cx);
    }

    pub(crate) fn prefill_agent(
        &mut self,
        surface_id: u64,
        text: &str,
        origin: Option<WeakEntity<CodeView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self.terminal_for_surface(surface_id) else {
            self.show_toast(AGENT_GONE, cx);
            return;
        };
        self.prefill_terminal(&terminal, text, origin, window, cx);
    }

    pub(crate) fn prefill_terminal(
        &mut self,
        terminal: &Entity<TerminalView>,
        text: &str,
        origin: Option<WeakEntity<CodeView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let app = cx.entity().downgrade();
        let later_origin = origin.clone();
        let later_terminal = terminal.downgrade();
        let outcome = terminal.update(cx, |terminal, cx| {
            terminal.prefill(
                text,
                move |outcome, window, cx| {
                    let Some(terminal) = later_terminal.upgrade() else {
                        return;
                    };
                    let _ = app.update(cx, |app, cx| {
                        app.terminal_prefill_settled(&terminal, later_origin, outcome, window, cx);
                    });
                },
                window,
                cx,
            )
        });
        match outcome {
            Ok(Prefill::Confirming) => {}
            Ok(Prefill::Inserted) => {
                self.terminal_prefill_settled(terminal, origin, Ok(()), window, cx);
            }
            Err(reason) => {
                self.terminal_prefill_settled(terminal, origin, Err(reason), window, cx);
            }
        }
    }

    pub(crate) fn terminal_prefill_settled(
        &mut self,
        terminal: &Entity<TerminalView>,
        origin: Option<WeakEntity<CodeView>>,
        outcome: Result<(), &'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(()) => {
                if let Some(view) = origin.and_then(|origin| origin.upgrade()) {
                    view.update(cx, |view, cx| view.comment_delivered(cx));
                }
                let surface_id = terminal.read(cx).surface_id();
                if self.terminal_for_surface(surface_id).is_some() {
                    self.focus_agent_surface(surface_id, window, cx);
                } else {
                    let focus = terminal.read(cx).focus_handle(cx);
                    window.focus(&focus, cx);
                }
            }
            Err(reason) => self.show_toast(reason, cx),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_primitives::action_menu::MenuEntry;

    fn target(surface_id: u64, tool: TerminalAgent, title: &str) -> AgentTarget {
        AgentTarget {
            surface_id,
            tool,
            title: title.to_owned(),
        }
    }

    fn labels<A: Clone + 'static>(menu: &ActionMenu<A>) -> Vec<String> {
        menu.entries()
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.label_for_test().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_three_confirmed_runtimes_get_an_at_mention_and_the_others_the_plain_path() {
        let root = Path::new("/w");
        let path = Path::new("/w/src/main.rs");
        for tool in [
            TerminalAgent::ClaudeCode,
            TerminalAgent::Codex,
            TerminalAgent::Gemini,
        ] {
            assert_eq!(file_mention(tool, root, path), "@src/main.rs ", "{tool:?}");
        }
        for tool in [
            TerminalAgent::Opencode,
            TerminalAgent::Pi,
            TerminalAgent::Hermes,
        ] {
            assert_eq!(file_mention(tool, root, path), "src/main.rs ", "{tool:?}");
        }
    }

    #[test]
    fn a_file_outside_the_root_is_mentioned_by_its_absolute_path() {
        assert_eq!(
            file_mention(
                TerminalAgent::ClaudeCode,
                Path::new("/w"),
                Path::new("/etc/hosts")
            ),
            "@/etc/hosts "
        );
    }

    #[test]
    fn a_path_with_a_space_is_quoted_by_the_rule_of_its_runtime() {
        let root = Path::new("/w");
        let path = Path::new("/w/my docs/a b.md");
        assert_eq!(
            file_mention(TerminalAgent::ClaudeCode, root, path),
            "@\"my docs/a b.md\" "
        );
        assert_eq!(
            file_mention(TerminalAgent::Codex, root, path),
            "@\"my docs/a b.md\" "
        );
        assert_eq!(
            file_mention(TerminalAgent::Gemini, root, path),
            "@my\\ docs/a\\ b.md "
        );
        assert_eq!(
            file_mention(TerminalAgent::Opencode, root, path),
            "\"my docs/a b.md\" "
        );
    }

    #[test]
    fn a_mention_never_ends_with_a_line_break() {
        for tool in TerminalAgent::all() {
            let text = file_mention(tool, Path::new("/w"), Path::new("/w/a b.rs"));
            assert!(text.ends_with(' '), "{tool:?}");
            assert!(!text.contains(['\r', '\n']), "{tool:?}");
        }
    }

    #[test]
    fn the_menu_entry_is_disabled_without_an_agent_and_direct_with_one() {
        let none = ActionMenu::new().item(send_to_agent_item::<u64>(&[], |id| id));
        assert_eq!(labels(&none), [NO_AGENT]);
        assert!(!none.enabled_at_for_test(0));

        let one = [target(7, TerminalAgent::ClaudeCode, "review")];
        let menu = ActionMenu::new().item(send_to_agent_item(&one, |id| id));
        assert_eq!(labels(&menu), [SEND_TO_AGENT]);
        assert!(menu.enabled_at_for_test(0));
        assert_eq!(menu.action_at(0), Some(7));
    }

    #[test]
    fn several_agents_are_listed_in_the_order_they_were_given() {
        let many = [
            target(3, TerminalAgent::Codex, "fix tests"),
            target(9, TerminalAgent::ClaudeCode, "review"),
        ];
        let menu = ActionMenu::new().item(send_to_agent_item(&many, |id| id));
        assert_eq!(labels(&menu), [SEND_TO_AGENT]);
        assert_eq!(menu.action_at(0), None);
        let choices = agent_choices(&many, None, |id| id);
        assert_eq!(labels(&choices), ["fix tests", "review"]);
        assert_eq!(choices.action_at(0), Some(3));
        assert_eq!(choices.action_at(1), Some(9));
    }

    #[test]
    fn the_remembered_agent_leads_the_list() {
        let many = [
            target(3, TerminalAgent::Codex, "fix tests"),
            target(9, TerminalAgent::ClaudeCode, "review"),
        ];
        let choices = agent_choices(&many, Some(9), |id| id);
        assert_eq!(labels(&choices), ["review", "fix tests"]);
        let stale = agent_choices(&many, Some(42), |id| id);
        assert_eq!(labels(&stale), ["fix tests", "review"]);
    }

    fn comment(first: usize, last: usize, text: &str, quoted: &str) -> RangeComment {
        RangeComment {
            path: PathBuf::from("/w/src/lib.rs"),
            first_line: first,
            last_line: last,
            text: text.to_owned(),
            quoted: quoted.to_owned(),
        }
    }

    #[test]
    fn a_comment_prompt_carries_the_location_the_comment_and_the_quoted_lines() {
        let prompt = comment_prompt(
            Path::new("/w"),
            &comment(12, 15, "rename this\n", "fn a() {}\nfn b() {}\n"),
        );
        assert_eq!(
            prompt,
            "src/lib.rs:12-15\n\nrename this\n\n```rs\nfn a() {}\nfn b() {}\n```"
        );
        let single = comment_prompt(Path::new("/w"), &comment(12, 12, "why?", "let a = 1;"));
        assert!(single.starts_with("src/lib.rs:12\n\nwhy?\n\n```rs\n"));
        assert!(!single.ends_with('\n'));
    }

    #[test]
    fn a_long_comment_and_long_quoted_lines_are_cut_at_their_own_limits() {
        let text = "x".repeat(COMMENT_MAX_CHARS + 50);
        let quoted = "q".repeat(QUOTED_MAX_CHARS + 50);
        let prompt = comment_prompt(Path::new("/w"), &comment(1, 1, &text, &quoted));
        assert_eq!(
            prompt.chars().filter(|c| *c == 'x').count(),
            COMMENT_MAX_CHARS
        );
        assert_eq!(
            prompt.chars().filter(|c| *c == 'q').count(),
            QUOTED_MAX_CHARS
        );
        assert_eq!(prompt.matches(TRUNCATION_MARK).count(), 2);
    }

    #[test]
    fn quoted_backticks_cannot_close_the_fence() {
        let prompt = comment_prompt(Path::new("/w"), &comment(1, 1, "x", "a ``` b ```` c"));
        assert!(prompt.contains("\n`````rs\na ``` b ```` c\n`````"));
    }

    #[test]
    fn the_range_label_names_one_line_or_both_ends() {
        assert_eq!(range_label(12, 12), "L12");
        assert_eq!(range_label(12, 15), "L12-L15");
    }
}
