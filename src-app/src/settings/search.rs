use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui::{AnyElement, Div, IntoElement, ParentElement, Styled, div, prelude::*, px};

use crate::SettingsSection;
use crate::settings::components::{hairline, setting_card};
use crate::theme::UiColors;

const TRANSITION_SECONDS: f32 = 0.18;

pub(crate) struct SettingCopy {
    pub title: &'static str,
    pub description: &'static str,
}

pub(crate) const DEFAULT_EDITOR: SettingCopy = SettingCopy {
    title: "Default editor",
    description: "Default application for opening files and folders.",
};
pub(crate) const DEFAULT_SHELL: SettingCopy = SettingCopy {
    title: "Shell in the integrated terminal",
    description: "Choose which shell opens in new integrated terminals. Existing terminals keep \
                  their shell until restarted.",
};
pub(crate) const NATIVE_NOTIFICATIONS: SettingCopy = SettingCopy {
    title: "Native OS notifications",
    description: "Alert you when an agent needs attention or finishes while Paneflow is unfocused.",
};
pub(crate) const THEME_PRESET: SettingCopy = SettingCopy {
    title: "Preset",
    description: "Palette applied to the terminal grid and the app chrome.",
};
pub(crate) const REDUCE_MOTION: SettingCopy = SettingCopy {
    title: "Reduce motion",
    description: "Settle hover transitions and the sidebar slide instantly instead of animating \
                  them.",
};
#[cfg(target_os = "windows")]
pub(crate) const CHROME_MATERIAL: SettingCopy = SettingCopy {
    title: "Chrome material",
    description: "Let Mica show through the navigation card.",
};
#[cfg(target_os = "macos")]
pub(crate) const SIDEBAR_TRANSPARENCY: SettingCopy = SettingCopy {
    title: "Sidebar transparency",
    description: "Show the native macOS Sidebar material in the navigation card.",
};
pub(crate) const CURSOR_SHAPE: SettingCopy = SettingCopy {
    title: "Cursor shape",
    description: "Default shape before an application overrides it. Takes effect on the next new \
                  terminal.",
};
pub(crate) const CURSOR_COLOR: SettingCopy = SettingCopy {
    title: "Cursor color",
    description: "Overrides the cursor color from the active color scheme.",
};
pub(crate) const FONT_FAMILY: SettingCopy = SettingCopy {
    title: "Font family",
    description: "Choose the monospace font used by every terminal. Hot-reloads.",
};
pub(crate) const FONT_SIZE: SettingCopy = SettingCopy {
    title: "Font size",
    description: "Terminal font size in points (8-32). Hot-reloads.",
};
pub(crate) const LINE_HEIGHT: SettingCopy = SettingCopy {
    title: "Line height",
    description: "Terminal line-height multiplier (1.0-2.5). Hot-reloads.",
};
pub(crate) const CELL_WIDTH: SettingCopy = SettingCopy {
    title: "Cell width",
    description: "Terminal cell-width multiplier (0.3-2.0). Hot-reloads.",
};
pub(crate) const FONT_WEIGHT: SettingCopy = SettingCopy {
    title: "Font weight",
    description: "Controls terminal stroke thickness. Hot-reloads.",
};
pub(crate) const INTEGRATED_GLYPHS: SettingCopy = SettingCopy {
    title: "Integrated glyphs",
    description: "Draw block elements with Paneflow's built-in renderer instead of the font glyph.",
};
pub(crate) const COLOR_EMOJI: SettingCopy = SettingCopy {
    title: "Color emoji",
    description: "Render emoji in color when the platform font stack supports it.",
};
pub(crate) const SCROLLBAR: SettingCopy = SettingCopy {
    title: "Scrollbar",
    description: "Overlay scrollbar that appears while scrolling or hovering the right edge. \
                  Takes effect on the next new terminal.",
};
#[cfg(target_os = "windows")]
pub(crate) const ACRYLIC_MATERIAL: SettingCopy = SettingCopy {
    title: "Enable acrylic material",
    description: "Applies a translucent texture behind the terminal window.",
};
pub(crate) const WORKTREE_ROOT: SettingCopy = SettingCopy {
    title: "Worktree root",
    description: "Directory where Paneflow creates managed worktrees, one subdirectory per \
                  repository. Worktrees already created elsewhere stay where they are.",
};
pub(crate) const AUTO_REMOVE_WORKTREES: SettingCopy = SettingCopy {
    title: "Remove old worktrees automatically",
    description: "A managed worktree is removed when its workspace closes, and the oldest ones \
                  are trimmed past the keep limit. Uncommitted changes are saved as a snapshot \
                  first, and the branch is never deleted.",
};
pub(crate) const WORKTREE_KEEP_LIMIT: SettingCopy = SettingCopy {
    title: "Keep limit",
    description: "Number of managed worktrees to keep before the oldest unopened ones are \
                  removed.",
};
pub(crate) const MCP_BRIDGE: SettingCopy = SettingCopy {
    title: "Read your panes from your agents",
    description: "Registers the bundled paneflow-mcp bridge with every detected CLI agent \
                  (Claude Code, Codex, Gemini, opencode) so they can read other panes' output. \
                  Idempotent, backed up, and only touches the paneflow entry. Re-run after an \
                  update if a path goes stale.",
};
pub(crate) const CLAUDE_FULL_ACCESS: SettingCopy = SettingCopy {
    title: "Full access for Claude Code",
    description: "Edits any file and runs networked commands without asking. No protection \
                  against prompt injection.",
};
pub(crate) const AI_FREE_ACCESS: SettingCopy = SettingCopy {
    title: "AI free access",
    description: "Lets an agent auto-submit prompts to your other panes, without the \
                  PANEFLOW_IPC_SCRIPTING gate. Every write is logged.",
};
pub(crate) const INJECTION_FENCE: SettingCopy = SettingCopy {
    title: "Injection fence",
    description: "Marks peer-pane output as untrusted when an agent reads it, so a malicious \
                  repo cannot hijack it.",
};

pub(crate) const THEME_MODES: &[&str] = &["System", "Light", "Dark"];

const GENERAL_COPIES: &[&SettingCopy] = &[&DEFAULT_EDITOR, &DEFAULT_SHELL, &NATIVE_NOTIFICATIONS];
#[cfg(target_os = "windows")]
const APPEARANCE_COPIES: &[&SettingCopy] = &[&THEME_PRESET, &REDUCE_MOTION, &CHROME_MATERIAL];
#[cfg(target_os = "macos")]
const APPEARANCE_COPIES: &[&SettingCopy] = &[&THEME_PRESET, &REDUCE_MOTION, &SIDEBAR_TRANSPARENCY];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const APPEARANCE_COPIES: &[&SettingCopy] = &[&THEME_PRESET, &REDUCE_MOTION];
#[cfg(target_os = "windows")]
const TERMINAL_COPIES: &[&SettingCopy] = &[
    &CURSOR_SHAPE,
    &CURSOR_COLOR,
    &FONT_FAMILY,
    &FONT_SIZE,
    &LINE_HEIGHT,
    &CELL_WIDTH,
    &FONT_WEIGHT,
    &INTEGRATED_GLYPHS,
    &COLOR_EMOJI,
    &SCROLLBAR,
    &ACRYLIC_MATERIAL,
];
#[cfg(not(target_os = "windows"))]
const TERMINAL_COPIES: &[&SettingCopy] = &[
    &CURSOR_SHAPE,
    &CURSOR_COLOR,
    &FONT_FAMILY,
    &FONT_SIZE,
    &LINE_HEIGHT,
    &CELL_WIDTH,
    &FONT_WEIGHT,
    &INTEGRATED_GLYPHS,
    &COLOR_EMOJI,
    &SCROLLBAR,
];
const WORKTREES_COPIES: &[&SettingCopy] =
    &[&WORKTREE_ROOT, &AUTO_REMOVE_WORKTREES, &WORKTREE_KEEP_LIMIT];
const MCP_COPIES: &[&SettingCopy] = &[&MCP_BRIDGE];
const AGENTS_COPIES: &[&SettingCopy] = &[&CLAUDE_FULL_ACCESS, &AI_FREE_ACCESS, &INJECTION_FENCE];

const GENERAL_HEADERS: &[&str] = &["Defaults", "Notifications"];
#[cfg(target_os = "windows")]
const APPEARANCE_HEADERS: &[&str] = &["Theme", "Preferences", "Windows", "System", "Light", "Dark"];
#[cfg(target_os = "macos")]
const APPEARANCE_HEADERS: &[&str] = &["Theme", "Preferences", "macOS", "System", "Light", "Dark"];
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
const APPEARANCE_HEADERS: &[&str] = &["Theme", "Preferences", "System", "Light", "Dark"];
#[cfg(target_os = "windows")]
const TERMINAL_HEADERS: &[&str] = &["Cursor", "Display", "Window"];
#[cfg(not(target_os = "windows"))]
const TERMINAL_HEADERS: &[&str] = &["Cursor", "Display"];
const AGENTS_HEADERS: &[&str] = &["Agents", "Profiles", "Permissions"];
const MCP_HEADERS: &[&str] = &["MCP bridge"];
const WORKSPACES_HEADERS: &[&str] = &["Workspace templates"];
const WORKTREES_HEADERS: &[&str] = &["Storage and cleanup", "Managed worktrees", "Snapshots"];

fn section_copies(section: SettingsSection) -> &'static [&'static SettingCopy] {
    match section {
        SettingsSection::General => GENERAL_COPIES,
        SettingsSection::Appearance => APPEARANCE_COPIES,
        SettingsSection::Terminal => TERMINAL_COPIES,
        SettingsSection::Worktrees => WORKTREES_COPIES,
        SettingsSection::McpServers => MCP_COPIES,
        SettingsSection::Agents => AGENTS_COPIES,
        SettingsSection::Workspaces | SettingsSection::Shortcuts => &[],
    }
}

fn section_headers(section: SettingsSection) -> &'static [&'static str] {
    match section {
        SettingsSection::General => GENERAL_HEADERS,
        SettingsSection::Appearance => APPEARANCE_HEADERS,
        SettingsSection::Terminal => TERMINAL_HEADERS,
        SettingsSection::Agents => AGENTS_HEADERS,
        SettingsSection::McpServers => MCP_HEADERS,
        SettingsSection::Workspaces => WORKSPACES_HEADERS,
        SettingsSection::Worktrees => WORKTREES_HEADERS,
        SettingsSection::Shortcuts => &[],
    }
}

pub(crate) fn normalize(query: &str) -> String {
    query.trim().to_lowercase()
}

pub(crate) fn text_matches(text: &str, query: &str) -> bool {
    query.is_empty() || text.to_lowercase().contains(query)
}

fn copy_matches(copy: &SettingCopy, query: &str) -> bool {
    text_matches(copy.title, query) || text_matches(copy.description, query)
}

pub(crate) fn section_matches(section: SettingsSection, query: &str) -> bool {
    section_copies(section)
        .iter()
        .any(|copy| copy_matches(copy, query))
        || section_headers(section)
            .iter()
            .any(|header| text_matches(header, query))
}

#[derive(Default)]
pub(crate) struct SearchMotion {
    query: String,
    started: Option<Instant>,
    eased: f32,
    rows: HashMap<String, RowState>,
    heights: HashMap<String, f32>,
}

struct RowState {
    from: f32,
    amount: f32,
}

impl SearchMotion {
    fn begin(&mut self, query: &str, now: Instant) -> bool {
        if self.query != query {
            self.query = query.to_string();
            for row in self.rows.values_mut() {
                row.from = row.amount;
            }
            self.started = (!crate::ui_primitives::reduce_motion()).then_some(now);
        }
        self.eased = match self.started {
            Some(started) => {
                let progress = now.duration_since(started).as_secs_f32() / TRANSITION_SECONDS;
                if progress >= 1. {
                    self.started = None;
                    1.
                } else {
                    crate::ui_primitives::ease_out_cubic(progress)
                }
            }
            None => 1.,
        };
        self.started.is_some()
    }

    fn amount(&mut self, key: &str, visible: bool) -> f32 {
        let to = if visible { 1. } else { 0. };
        let eased = self.eased;
        let row = self.rows.entry(key.to_string()).or_insert(RowState {
            from: to,
            amount: to,
        });
        row.amount = row.from + (to - row.from) * eased;
        row.amount
    }

    pub(crate) fn reset(&mut self) {
        self.rows.clear();
        self.heights.clear();
        self.started = None;
    }
}

struct Frame {
    query: String,
    motion: Rc<RefCell<SearchMotion>>,
}

thread_local! {
    static FRAME: RefCell<Option<Frame>> = const { RefCell::new(None) };
}

pub(crate) fn begin_frame(query: String, motion: Rc<RefCell<SearchMotion>>, now: Instant) -> bool {
    let animating = motion.borrow_mut().begin(&query, now);
    FRAME.with(|frame| *frame.borrow_mut() = Some(Frame { query, motion }));
    animating
}

pub(crate) fn end_frame() {
    FRAME.with(|frame| *frame.borrow_mut() = None);
}

pub(crate) fn active_query() -> String {
    FRAME.with(|frame| {
        frame
            .borrow()
            .as_ref()
            .map(|frame| frame.query.clone())
            .unwrap_or_default()
    })
}

fn active_motion() -> Option<Rc<RefCell<SearchMotion>>> {
    FRAME.with(|frame| frame.borrow().as_ref().map(|frame| frame.motion.clone()))
}

fn amount_for(key: &str, visible: bool) -> f32 {
    active_motion().map_or(1., |motion| motion.borrow_mut().amount(key, visible))
}

fn collapse(key: &str, amount: f32, content: AnyElement) -> AnyElement {
    let Some(motion) = active_motion() else {
        return content;
    };
    let measured = motion.borrow().heights.get(key).copied();
    let measure_key = key.to_string();
    let measure_into = motion.clone();
    let natural = div()
        .relative()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .child(content)
        .child(
            gpui::canvas(
                move |bounds, _, _| {
                    measure_into
                        .borrow_mut()
                        .heights
                        .insert(measure_key, f32::from(bounds.size.height));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        );
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .overflow_hidden()
        .opacity(amount)
        .when(amount < 1., |wrapper| match measured {
            Some(height) => wrapper.h(px(height * amount)),
            None if amount <= 0. => wrapper.h(px(0.)),
            None => wrapper,
        })
        .child(natural)
        .into_any_element()
}

pub(crate) struct SearchCard {
    ui: UiColors,
    rows: Vec<(f32, AnyElement)>,
    any_match: bool,
}

impl SearchCard {
    pub(crate) fn new(ui: UiColors) -> Self {
        Self {
            ui,
            rows: Vec::new(),
            any_match: false,
        }
    }

    pub(crate) fn row(mut self, copy: &SettingCopy, element: impl IntoElement) -> Self {
        let matched = copy_matches(copy, &active_query());
        let amount = amount_for(copy.title, matched);
        self.any_match |= matched;
        self.rows.push((
            amount,
            collapse(copy.title, amount, element.into_any_element()),
        ));
        self
    }

    pub(crate) fn fixed(mut self, element: impl IntoElement) -> Self {
        self.rows.push((1., element.into_any_element()));
        self
    }

    fn finish(self) -> Div {
        let mut card = setting_card(self.ui);
        let mut previous: Option<f32> = None;
        for (amount, element) in self.rows {
            if let Some(above) = previous {
                let visible = above.min(amount);
                card = card.child(
                    div()
                        .w_full()
                        .h(px(visible))
                        .overflow_hidden()
                        .opacity(visible)
                        .child(hairline(self.ui)),
                );
            }
            card = card.child(element);
            previous = Some(amount);
        }
        card
    }
}

pub(crate) struct Block {
    label: &'static str,
    matched: bool,
    inner: Div,
}

impl Block {
    pub(crate) fn new(label: &'static str) -> Self {
        Self {
            label,
            matched: text_matches(label, &active_query()),
            inner: div().flex().flex_col(),
        }
    }

    pub(crate) fn top_gap(mut self, gap: f32) -> Self {
        self.inner = self.inner.mt(px(gap));
        self
    }

    pub(crate) fn gap(mut self, gap: f32) -> Self {
        self.inner = self.inner.gap(px(gap));
        self
    }

    pub(crate) fn child(mut self, element: impl IntoElement) -> Self {
        self.inner = self.inner.child(element);
        self
    }

    pub(crate) fn region(
        mut self,
        key: &'static str,
        texts: &[&str],
        element: impl IntoElement,
    ) -> Self {
        let query = active_query();
        let matched = texts.iter().any(|text| text_matches(text, &query));
        self.matched |= matched;
        let amount = amount_for(key, matched);
        self.inner = self
            .inner
            .child(collapse(key, amount, element.into_any_element()));
        self
    }

    pub(crate) fn card(mut self, card: SearchCard) -> Self {
        self.matched |= card.any_match;
        self.inner = self.inner.child(card.finish());
        self
    }

    pub(crate) fn finish(self) -> AnyElement {
        let key = format!("block:{}", self.label);
        let amount = amount_for(&key, self.matched);
        collapse(&key, amount, self.inner.into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn query_matches_setting_copy_across_title_and_description() {
        assert!(section_matches(SettingsSection::General, "opening files"));
        assert!(section_matches(SettingsSection::Terminal, "hot-reloads"));
        assert!(section_matches(SettingsSection::Worktrees, "snapshots"));
        assert!(!section_matches(SettingsSection::McpServers, "worktree"));
    }

    #[test]
    fn every_section_matches_the_empty_query() {
        for section in [
            SettingsSection::General,
            SettingsSection::Appearance,
            SettingsSection::Terminal,
            SettingsSection::Agents,
            SettingsSection::McpServers,
            SettingsSection::Workspaces,
            SettingsSection::Worktrees,
        ] {
            assert!(section_matches(section, ""), "{section:?}");
        }
    }

    #[test]
    fn search_motion_eases_rows_toward_their_target_and_settles() {
        let mut motion = SearchMotion::default();
        let start = Instant::now();
        assert!(!motion.begin("", start));
        assert_eq!(motion.amount("row", true), 1.);
        assert!(motion.begin("zzz", start));
        assert!(motion.begin("zzz", start + Duration::from_millis(90)));
        let mid = motion.amount("row", false);
        assert!(mid > 0. && mid < 1., "{mid}");
        assert!(!motion.begin("zzz", start + Duration::from_millis(200)));
        assert_eq!(motion.amount("row", false), 0.);
        assert!(motion.begin("", start + Duration::from_millis(210)));
        assert!(motion.begin("", start + Duration::from_millis(300)));
        let rising = motion.amount("row", true);
        assert!(rising > 0. && rising < 1., "{rising}");
    }

    #[test]
    fn search_motion_retargets_from_the_current_amount() {
        let mut motion = SearchMotion::default();
        let start = Instant::now();
        motion.begin("", start);
        motion.amount("row", true);
        motion.begin("a", start);
        motion.begin("a", start + Duration::from_millis(90));
        let partial = motion.amount("row", false);
        assert!(partial > 0. && partial < 1., "{partial}");
        motion.begin("ab", start + Duration::from_millis(90));
        assert_eq!(motion.amount("row", true), partial);
        motion.begin("ab", start + Duration::from_millis(180));
        let rising = motion.amount("row", true);
        assert!(rising > partial && rising < 1., "{rising}");
    }

    #[test]
    fn rows_first_seen_during_a_query_start_at_their_target() {
        let mut motion = SearchMotion::default();
        motion.begin("abc", Instant::now());
        assert_eq!(motion.amount("hidden", false), 0.);
        assert_eq!(motion.amount("shown", true), 1.);
    }
}
