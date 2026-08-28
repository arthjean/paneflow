//! Workspace tab - the level that owns a pane layout tree.
//!
//! US-001 (prd-cli-tab-hierarchy): a workspace no longer owns a single
//! `LayoutTree`; it owns a list of [`Tab`], each carrying the tree the
//! workspace used to carry, plus the zoom `saved_layout` that used to live at
//! the workspace level. Split, zoom and focus mechanics are unchanged - they
//! now operate one level down.

use gpui::{App, Entity, Window};
use paneflow_config::schema::{LayoutNode, TabTitleSource};

use crate::layout::LayoutTree;
use crate::pane::Pane;

/// Monotonic tab ID counter. Process-local and never persisted: the IPC
/// surface addresses surfaces by `surface_id`, never by tab index or tab id
/// (FR-07), so a stable in-memory identity is all the UI needs.
static NEXT_TAB_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_tab_id() -> u64 {
    NEXT_TAB_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// One working composition inside a workspace: a title plus the pane layout
/// tree, with the zoom bookkeeping that belongs to it.
pub struct Tab {
    /// Unique tab identifier, assigned at construction.
    pub id: u64,
    /// User-facing title. Empty means "unnamed" - the sidebar derives a
    /// fallback label (US-009).
    ///
    /// Private on purpose: [`Self::set_title`] is the ONE way a title changes,
    /// and it is where "a name a human typed is never overwritten" is
    /// enforced. A `pub` field would put that rule back in the hands of every
    /// caller, which is how such rules quietly stop holding.
    title: String,
    /// Who wrote [`Self::title`]. See [`TabTitleSource`].
    title_source: TabTitleSource,
    /// Pane layout tree. `None` for an empty tab (every pane closed).
    pub root: Option<LayoutTree>,
    /// Saved layout tree while zoomed. `Some(tree)` means this tab is zoomed
    /// and `root` holds only the zoomed pane as a single Leaf.
    pub saved_layout: Option<LayoutTree>,
}

impl Tab {
    /// Create a tab holding `root`, with a freshly allocated id.
    ///
    /// The title is [`TabTitleSource::Preset`], the weakest rank: every
    /// in-process construction site is Paneflow naming the tab itself (a
    /// preset label, the palette placeholder, an empty string), and all of it
    /// is meant to be replaced by a name that describes the work. A human's
    /// title only ever arrives through [`Self::set_title`], or from the
    /// restore path via [`Self::restored`].
    pub fn new(title: impl Into<String>, root: Option<LayoutTree>) -> Self {
        Self {
            id: next_tab_id(),
            title: title.into(),
            title_source: TabTitleSource::Preset,
            root,
            saved_layout: None,
        }
    }

    /// Rebuild a tab from a session snapshot, carrying the persisted title
    /// provenance across the restart. Without this, every restored tab would
    /// look app-named and the first prompt of the next session would erase a
    /// name the user typed days ago.
    pub fn restored(
        title: impl Into<String>,
        title_source: TabTitleSource,
        root: Option<LayoutTree>,
    ) -> Self {
        Self {
            title_source,
            ..Self::new(title, root)
        }
    }

    /// Create an untitled tab with no pane. Used to honour the "a workspace
    /// always has at least one tab" invariant (FR-01) when the last tab is
    /// closed.
    pub fn empty() -> Self {
        Self::new(String::new(), None)
    }

    /// The tab's stored title. Empty means unnamed; the sidebar derives the
    /// visible fallback (US-009).
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn title_source(&self) -> TabTitleSource {
        self.title_source
    }

    /// Whether this title belongs to the user and is therefore frozen against
    /// every automatic naming path.
    pub fn title_is_user_owned(&self) -> bool {
        self.title_source == TabTitleSource::User
    }

    /// Whether an automatic name should still be looked for. See
    /// [`TabTitleSource::is_settled`].
    pub fn title_is_settled(&self) -> bool {
        self.title_source.is_settled()
    }

    /// The single write path for a tab title. Returns whether anything
    /// changed, so callers persist the session only on a real delta.
    ///
    /// Rules, in order:
    /// 1. Precedence is [`TabTitleSource::yields_to`]'s to decide, and only
    ///    its. Checking it here rather than at each call site is what keeps
    ///    "a name a human typed is never overwritten" true of code not yet
    ///    written.
    /// 2. An automatic title never *clears* one. A CLI that generated nothing
    ///    must leave the tab as it is, not blank it.
    /// 3. Titles are stored trimmed, and a write that changes nothing reports
    ///    `false` - every turn of an agent would otherwise schedule a session
    ///    save of identical bytes.
    pub fn set_title(&mut self, title: &str, source: TabTitleSource) -> bool {
        let title = title.trim();
        if title.is_empty() || !self.title_source.yields_to(source) {
            return false;
        }
        if self.title == title && self.title_source == source {
            return false;
        }
        self.title.clear();
        self.title.push_str(title);
        self.title_source = source;
        true
    }

    /// Hand a named tab back to auto-naming (the tab menu's "Reset name").
    /// Returns whether anything changed.
    ///
    /// The text is deliberately kept: dropping it would flash the tab back to
    /// its "Tab 3" fallback until the next turn produced a name. Only the
    /// ownership changes - and it drops all the way to `Preset`, so the very
    /// next thing the agent does can name the tab, rather than the reset only
    /// taking effect once a *better* rank comes along.
    pub fn unlock_title(&mut self) -> bool {
        let changed = self.title_source != TabTitleSource::Preset;
        self.title_source = TabTitleSource::Preset;
        changed
    }

    pub fn is_zoomed(&self) -> bool {
        self.saved_layout.is_some()
    }

    /// Leave zoom, restoring the saved tree. Returns the pane that was zoomed.
    pub fn exit_zoom(&mut self, cx: &mut App) -> Option<Entity<Pane>> {
        let zoomed_pane = self.root.as_ref().and_then(|root| root.first_leaf());
        let saved = self.saved_layout.take()?;
        self.root = Some(saved);
        if let Some(pane) = &zoomed_pane {
            pane.update(cx, |pane, _| {
                pane.zoomed = false;
            });
        }
        zoomed_pane
    }

    pub fn pane_count(&self) -> usize {
        self.root.as_ref().map_or(0, |r| r.leaf_count())
    }

    /// Whether this tab can take one more pane.
    ///
    /// US-003 (prd-cli-tab-hierarchy): `MAX_PANES` bounds a *tab*, not a
    /// workspace. Every create site - keyboard split, drop-to-split, launch
    /// pad, IPC `surface.split` - gates on this single predicate so the cap
    /// cannot drift between them.
    pub fn can_add_pane(&self) -> bool {
        self.pane_count() < crate::layout::MAX_PANES
    }

    pub fn contains_pane(&self, pane: &Entity<Pane>) -> bool {
        self.root
            .as_ref()
            .is_some_and(|root| root.contains_leaf(pane))
            || self
                .saved_layout
                .as_ref()
                .is_some_and(|saved| saved.contains_leaf(pane))
    }

    pub fn any_pane(&self, f: &mut impl FnMut(&Entity<Pane>) -> bool) -> bool {
        if let Some(root) = &self.root
            && root.any_leaf(f)
        {
            return true;
        }
        if let Some(saved) = &self.saved_layout
            && saved.any_leaf(f)
        {
            return true;
        }
        false
    }

    /// Every pane of this tab, the zoom-saved tree included, in traversal
    /// order and without duplicates.
    /// Terminal surface ids this tab owns.
    ///
    /// The same walk the sidebar does to attribute a session to a tab row
    /// (`tab_row_sessions`), so an unread completion and a session badge can
    /// never disagree about which row speaks for a surface.
    pub fn surface_ids(&self, cx: &gpui::App) -> std::collections::HashSet<u64> {
        let mut ids = std::collections::HashSet::new();
        for pane in self.collect_panes() {
            for terminal in pane.read(cx).terminals() {
                ids.insert(terminal.entity_id().as_u64());
            }
        }
        ids
    }

    pub fn collect_panes(&self) -> Vec<Entity<Pane>> {
        let mut panes = Vec::new();
        if let Some(root) = &self.root {
            panes.extend(root.collect_leaves());
        }
        if let Some(saved) = &self.saved_layout {
            for pane in saved.collect_leaves() {
                if !panes.contains(&pane) {
                    panes.push(pane);
                }
            }
        }
        panes
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut App) {
        if let Some(root) = &self.root {
            root.focus_first(window, cx);
        }
    }

    /// Serialize this tab's layout to a `LayoutNode`.
    ///
    /// When zoomed, serializes the saved (un-zoomed) layout so the full pane
    /// arrangement is captured rather than just the single zoomed pane.
    pub fn serialize(&self, cx: &App) -> Option<LayoutNode> {
        let tree = self.saved_layout.as_ref().or(self.root.as_ref())?;
        Some(tree.serialize(cx))
    }

    /// Serialize this tab for session persistence without terminal output,
    /// which must remain local to the current process.
    pub fn serialize_without_scrollback(&self, cx: &App) -> Option<LayoutNode> {
        let tree = self.saved_layout.as_ref().or(self.root.as_ref())?;
        Some(tree.serialize_without_scrollback(cx))
    }
}

/// The title-precedence rule, tested at the one place that enforces it.
#[cfg(test)]
mod tests {
    use super::*;

    fn tab(title: &str) -> Tab {
        Tab::new(title, None)
    }

    /// The ladder each rank climbs: a preset label gives way to the first
    /// prompt's placeholder, which gives way to the title the CLI generates,
    /// which gives way to a human.
    #[test]
    fn each_rank_replaces_the_ones_below_it() {
        let mut tab = tab("Claude Code");
        assert_eq!(tab.title_source(), TabTitleSource::Preset);

        assert!(tab.set_title("fix the flaky worktree test", TabTitleSource::Prompt));
        assert_eq!(tab.title(), "fix the flaky worktree test");

        assert!(tab.set_title("Worktree test deflake", TabTitleSource::Generated));
        assert_eq!(tab.title(), "Worktree test deflake");

        assert!(tab.set_title("sprint 3", TabTitleSource::User));
        assert_eq!(tab.title(), "sprint 3");
    }

    #[test]
    fn a_user_title_is_never_overwritten_by_an_automatic_one() {
        let mut tab = tab("Claude Code");
        assert!(tab.set_title("sprint 3", TabTitleSource::User));

        for source in [
            TabTitleSource::Preset,
            TabTitleSource::Prompt,
            TabTitleSource::Generated,
        ] {
            assert!(!tab.set_title("something else", source), "{source:?}");
        }
        assert_eq!(tab.title(), "sprint 3");
        assert!(tab.title_is_user_owned());
    }

    /// The bug this ranking exists for: a session is torn down a few seconds
    /// after each turn, so every new turn used to look like a first one and
    /// rename the tab after whatever was just typed. A tab keeps naming the
    /// work it was opened for.
    #[test]
    fn a_later_prompt_does_not_replace_the_first_ones_placeholder() {
        let mut tab = tab("Claude Code");
        assert!(tab.set_title("fix the flaky worktree test", TabTitleSource::Prompt));

        assert!(!tab.set_title("yes go ahead", TabTitleSource::Prompt));
        assert_eq!(tab.title(), "fix the flaky worktree test");
    }

    /// A generated title is live intent, not a one-shot: a CLI that
    /// regenerates its session title has a better one.
    #[test]
    fn a_generated_title_replaces_an_earlier_generated_title() {
        let mut tab = tab("Claude Code");
        assert!(tab.set_title("Worktree test deflake", TabTitleSource::Generated));
        assert!(tab.set_title("Release checksum job", TabTitleSource::Generated));
        assert_eq!(tab.title(), "Release checksum job");
    }

    #[test]
    fn a_user_title_can_be_replaced_by_another_user_title() {
        let mut tab = tab("");
        assert!(tab.set_title("sprint 3", TabTitleSource::User));
        assert!(tab.set_title("sprint 4", TabTitleSource::User));
        assert_eq!(tab.title(), "sprint 4");
    }

    /// A preset label never replaces one: the empty tab being filled already
    /// carries whatever it should.
    #[test]
    fn a_preset_label_does_not_replace_a_preset_label() {
        let mut tab = tab("Claude Code");
        assert!(!tab.set_title("Codex", TabTitleSource::Preset));
        assert_eq!(tab.title(), "Claude Code");
    }

    /// A CLI that generated nothing must leave the tab alone rather than blank
    /// it - the sidebar would fall back to "Tab 2" out of nowhere.
    #[test]
    fn an_automatic_title_never_clears_one() {
        let mut tab = tab("OpenCode");
        assert!(!tab.set_title("", TabTitleSource::Generated));
        assert!(!tab.set_title("   \n ", TabTitleSource::Prompt));
        assert_eq!(tab.title(), "OpenCode");
    }

    /// Every turn of an agent would otherwise schedule a session write of
    /// identical bytes.
    #[test]
    fn an_unchanged_title_reports_no_change() {
        let mut tab = tab("Codex");
        assert!(tab.set_title("Deflake the tests", TabTitleSource::Generated));
        assert!(!tab.set_title("Deflake the tests", TabTitleSource::Generated));
        assert!(!tab.set_title("  Deflake the tests  ", TabTitleSource::Generated));
        assert!(tab.set_title("Deflake the tests", TabTitleSource::User));
    }

    #[test]
    fn titles_are_stored_trimmed() {
        let mut tab = tab("");
        assert!(tab.set_title("  fix the flaky test \n", TabTitleSource::User));
        assert_eq!(tab.title(), "fix the flaky test");
    }

    /// Only the top two ranks stop the search for a better name. A tab still
    /// wearing a preset label or a placeholder is one worth reading a
    /// transcript for.
    #[test]
    fn only_a_generated_or_user_title_settles_a_tab() {
        let mut tab = tab("Claude Code");
        assert!(!tab.title_is_settled());

        assert!(tab.set_title("fix the flaky test", TabTitleSource::Prompt));
        assert!(!tab.title_is_settled());

        assert!(tab.set_title("Test deflake", TabTitleSource::Generated));
        assert!(tab.title_is_settled());

        assert!(tab.set_title("sprint 3", TabTitleSource::User));
        assert!(tab.title_is_settled());
    }

    /// Reset name drops all the way to the weakest rank, so the very next
    /// thing the agent does can name the tab. Dropping only one rank would
    /// leave a reset tab waiting on a title better than the one it had.
    #[test]
    fn unlock_title_reopens_naming_and_keeps_the_text() {
        let mut tab = tab("Claude Code");
        assert!(tab.set_title("sprint 3", TabTitleSource::User));

        assert!(tab.unlock_title());
        assert_eq!(tab.title(), "sprint 3");
        assert!(!tab.title_is_user_owned());
        assert!(!tab.title_is_settled());
        assert!(tab.set_title("fix the flaky test", TabTitleSource::Prompt));
        assert_eq!(tab.title(), "fix the flaky test");
    }

    #[test]
    fn unlock_title_on_a_preset_named_tab_reports_no_change() {
        assert!(!tab("Claude Code").unlock_title());
    }

    /// A blank rename is the inline editor being dismissed, not a request to
    /// erase the name - `commit_rename` guards this too, doubly on purpose.
    #[test]
    fn a_blank_user_title_is_refused() {
        let mut tab = tab("Codex");
        assert!(!tab.set_title("  ", TabTitleSource::User));
        assert_eq!(tab.title(), "Codex");
        assert!(!tab.title_is_user_owned());
    }

    #[test]
    fn a_freshly_built_tab_is_app_named() {
        assert!(!tab("Claude Code").title_is_user_owned());
        assert!(!Tab::empty().title_is_user_owned());
        assert!(Tab::restored("sprint 3", TabTitleSource::User, None).title_is_user_owned());
    }
}
