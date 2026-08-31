//! EP-005 (prd-cli-tab-hierarchy): the « New pane » preset picker.
//!
//! One entry point for the moment the user decides *what* to launch. The
//! picker is a pure view over catalogues that already exist - the default
//! shell, the agents made visible in Settings -> AI Agent
//! ([`TerminalAgent::visible`]), and the workspace's custom command buttons
//! ([`paneflow_config::schema::ButtonCommand`]). Nothing new is written to
//! `paneflow.json`: US-015 forbids a `presets` table, and every agent command
//! comes back from [`TerminalAgent::launch_command`] so the Claude bypass
//! setting keeps being honored instead of being reimplemented here.
//!
//! Shape: the picker is a *pane-sized card*, never an overlay, holding one
//! centered column of plain buttons - nothing folds open, nothing is filtered.
//! It appears in the slot the new surface is about to occupy: `New tab` and the
//! sidebar's `+` open a `New pane` tab and fill it, while the pane header's
//! split buttons show it in the half the split is about to create, next to the
//! panes that stay visible. Up / Down move the cursor, Enter launches, Escape
//! creates nothing and hands focus back.

use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseUpEvent, ParentElement,
    ScrollHandle, SharedString, Styled, WeakEntity, Window, deferred, div, prelude::*, px, svg,
};
use paneflow_config::schema::{ButtonCommand, PaneFlowConfig, TerminalSurfaceProfile};

use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::layout::SplitDirection;
use crate::pane::Pane;
use crate::settings::components::{select_item, select_menu, with_alpha};
use crate::ui_primitives::squircle::squircle_fill;
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};

/// Title of the surface the picker stands in for, until a preset renames it.
pub(crate) const PALETTE_TAB_TITLE: &str = "New pane";

/// Width of the picker's column: the preset buttons, the checkout select above
/// them, and that select's menu all share one edge.
const PICKER_WIDTH: f32 = 260.0;

/// Where the picked preset lands - and therefore where the card is drawn.
pub(crate) enum PalettePlacement {
    /// The picker is the whole content of the empty tab it just created.
    /// Picking fills that tab; Escape closes it again.
    Tab { tab_id: u64 },
    /// The picker stands in the half a split is about to create, beside the
    /// panes that stay on screen. Nothing is split until a preset is picked,
    /// so Escape leaves the tab exactly as it was.
    Split {
        target: WeakEntity<Pane>,
        direction: SplitDirection,
    },
}

/// What one row of the branch select points at.
#[derive(Clone)]
enum BranchTarget {
    /// A branch, checked out on demand if nothing holds it yet.
    Branch(String),
    /// A checkout with no branch to name it - bound directly, since there is
    /// no branch to resolve.
    Checkout(std::path::PathBuf),
}

/// One row of the branch select.
struct BranchOption {
    target: BranchTarget,
    label: String,
    selected: bool,
    /// Whether picking it has to create a worktree first.
    needs_checkout: bool,
}

/// The three catalogues the picker projects (US-015). No fourth source, and
/// no persistence of its own.
#[derive(Debug, Clone)]
pub(crate) enum PresetSource {
    /// The configured default shell, launched as a plain terminal surface.
    Shell,
    Agent(TerminalAgent),
    Custom(ButtonCommand),
}

/// One picker button.
#[derive(Debug, Clone)]
pub(crate) struct Preset {
    pub(crate) label: String,
    pub(crate) source: PresetSource,
}

impl Preset {
    fn icon_path(&self) -> SharedString {
        match &self.source {
            PresetSource::Shell => "icons/terminal.svg".into(),
            PresetSource::Agent(agent) => agent.icon_path().into(),
            PresetSource::Custom(button) => {
                if button.icon.is_empty() {
                    "icons/terminal.svg".into()
                } else {
                    SharedString::from(button.icon.clone())
                }
            }
        }
    }

    fn icon_multicolor(&self) -> bool {
        matches!(&self.source, PresetSource::Agent(agent) if agent.icon_multicolor())
    }

    fn accent(&self) -> Option<u32> {
        match &self.source {
            PresetSource::Agent(agent) => agent.accent(),
            _ => None,
        }
    }

    fn profile(&self) -> TerminalSurfaceProfile {
        match &self.source {
            PresetSource::Agent(_) => TerminalSurfaceProfile::Agent,
            _ => TerminalSurfaceProfile::Normal,
        }
    }

    /// The line written to the new terminal, or `None` for a bare shell.
    fn command(&self, config: &PaneFlowConfig) -> Option<String> {
        match &self.source {
            PresetSource::Shell => None,
            PresetSource::Agent(agent) => Some(agent.launch_command(config)),
            PresetSource::Custom(button) => Some(button.command.clone()),
        }
    }

    /// `Err` carries the readable refusal a not-installed agent must produce
    /// instead of an empty terminal (US-015 AC4).
    fn ensure_launchable(&self) -> Result<(), String> {
        match &self.source {
            PresetSource::Agent(agent) if !agent.is_installed() => Err(format!(
                "{} is not installed - install its CLI, or hide it in Settings > AI Agent",
                agent.display_name()
            )),
            _ => Ok(()),
        }
    }
}

/// Live picker state, owned by `PaneFlowApp`. `None` = closed.
pub(crate) struct PanePaletteState {
    /// Workspace the preset lands in, by stable id (survives reorders).
    pub(crate) ws_id: u64,
    pub(crate) placement: PalettePlacement,
    /// Keyboard cursor into the preset list.
    pub(crate) selected: usize,
    /// Last refusal, shown under the buttons (US-015 AC4).
    pub(crate) error: Option<String>,
    /// Focus to hand back when the picker goes away (US-014 AC5). `None` for
    /// a split placement, which hands focus back to its target pane instead.
    pub(crate) restore_focus: Option<FocusHandle>,
    pub(crate) scroll: ScrollHandle,
    /// Whether the branch menu is up over the presets. Closed on open, so the
    /// picker always comes up on its presets.
    pub(crate) branch_picker_open: bool,
}

impl PaneFlowApp {
    /// Build the picker catalogue for `ws_idx` (US-015): Terminal first, then
    /// the visible agents in `TerminalAgent::ALL` order, then the workspace's
    /// custom commands. A workspace without custom commands simply ends after
    /// the agents - there is no empty section.
    pub(crate) fn pane_palette_presets(&self, ws_idx: usize) -> Vec<Preset> {
        let mut presets = vec![Preset {
            label: "Terminal".to_string(),
            source: PresetSource::Shell,
        }];
        presets.extend(
            TerminalAgent::visible(&self.cached_config)
                .into_iter()
                .map(|agent| Preset {
                    label: agent.display_name().to_string(),
                    source: PresetSource::Agent(agent),
                }),
        );
        if let Some(ws) = self.workspaces.get(ws_idx) {
            presets.extend(ws.custom_buttons.iter().map(|button| Preset {
                label: button.name.clone(),
                source: PresetSource::Custom(button.clone()),
            }));
        }
        presets
    }

    /// Re-resolve the picker's workspace by id (it may have been reordered or
    /// closed while the picker was open).
    fn pane_palette_ws_idx(&self) -> Option<usize> {
        let ws_id = self.pane_palette.as_ref()?.ws_id;
        self.workspaces.iter().position(|ws| ws.id == ws_id)
    }

    /// Open a `New pane` tab in `ws_idx` and make the preset picker its
    /// content. Entry points: the `New tab` action and the sidebar folder
    /// row's hover `+` (US-010 AC).
    ///
    /// The picker is the tab's surface, not an overlay: creating a tab and
    /// choosing what runs in it are one gesture, so the choice is made in the
    /// place the result will appear rather than over the panes it hides.
    pub(crate) fn open_pane_palette(
        &mut self,
        ws_idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws) = self.workspaces.get(ws_idx) else {
            return;
        };
        let ws_id = ws.id;
        self.commit_rename(cx);
        self.dismiss_transient_surfaces();
        let restore_focus = window.focused(cx);

        let tab = crate::workspace::Tab::new(PALETTE_TAB_TITLE, None);
        let tab_id = tab.id;
        let opened = self
            .workspaces
            .get_mut(ws_idx)
            .is_some_and(|ws| ws.open_tab(tab));
        if !opened {
            self.show_toast("Tab limit reached for this workspace", cx);
            return;
        }
        if let Some(ws) = self.workspaces.get_mut(ws_idx) {
            // A tab created from a collapsed folder row must be visible.
            ws.sidebar_expanded = true;
        }

        // The tab exists before any preset is picked, which is what lets the
        // worktree be chosen BEFORE the agent's process spawns - the ordering
        // Cursor and the Codex app both rely on. Refresh the repository's
        // worktree list now so the header row is populated by the time the eye
        // reaches it.
        self.spawn_worktree_listing(ws_idx, cx);
        self.pane_palette = Some(PanePaletteState {
            ws_id,
            placement: PalettePlacement::Tab { tab_id },
            selected: 0,
            error: None,
            restore_focus,
            scroll: ScrollHandle::new(),
            branch_picker_open: false,
        });
        let tab_idx = self.workspaces[ws_idx].active_tab_idx();
        self.focus_workspace_tab(ws_idx, tab_idx, window, cx);
        // The card owns the keyboard: there is no text field to hand focus to.
        window.focus(&self.pane_palette_focus, cx);
        cx.notify();
    }

    /// Show the picker in the slot a split would create, instead of dropping a
    /// bare shell there. Entry point: the pane header's split buttons
    /// (`PaneEvent::Split`).
    ///
    /// No `Window` here - the pane-event subscriber has none - so focus is
    /// claimed on the next frame through `pending_palette_focus`, the same
    /// deferral `pending_pane_focus` uses for a drop-to-split.
    pub(crate) fn open_split_palette(
        &mut self,
        target: Entity<Pane>,
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) {
        let ws_id = target.read(cx).workspace_id;
        self.commit_rename(cx);
        self.dismiss_transient_surfaces();
        self.pane_palette = Some(PanePaletteState {
            ws_id,
            placement: PalettePlacement::Split {
                target: target.downgrade(),
                direction,
            },
            selected: 0,
            error: None,
            restore_focus: None,
            scroll: ScrollHandle::new(),
            // A split fills an existing tab, whose binding already governs
            // where its panes start: no worktree row, nothing to unfold.
            branch_picker_open: false,
        });
        self.pending_palette_focus = true;
        cx.notify();
    }

    /// Drop a split picker whose slot is no longer on screen - the target pane
    /// was closed, or the user switched tab or project. Without this the state
    /// would survive invisibly and re-appear on the way back, holding focus in
    /// the meantime.
    pub(crate) fn prune_stale_split_palette(&mut self, cx: &mut Context<Self>) {
        let Some(palette) = self.pane_palette.as_ref() else {
            return;
        };
        let PalettePlacement::Split { target, .. } = &palette.placement else {
            return;
        };
        let ws_id = palette.ws_id;
        let visible = target.upgrade().is_some_and(|target| {
            self.active_workspace().is_some_and(|ws| {
                ws.id == ws_id
                    && ws
                        .active_tab()
                        .root
                        .as_ref()
                        .is_some_and(|root| root.contains_leaf(&target))
            })
        });
        if !visible {
            self.pane_palette = None;
            cx.notify();
        }
    }

    /// Target pane and direction of a pending split picker in the *active*
    /// tab, so the layout tree can draw the picker at that pane's slot. `None`
    /// when the picker is closed or owns a whole tab.
    pub(crate) fn pending_split_palette(&self) -> Option<(Entity<Pane>, SplitDirection)> {
        let palette = self.pane_palette.as_ref()?;
        let PalettePlacement::Split { target, direction } = &palette.placement else {
            return None;
        };
        let target = target.upgrade()?;
        (self.active_workspace()?.id == palette.ws_id).then_some((target, *direction))
    }

    /// Drop the picker state without touching its tab. Used by the launch
    /// path, which is about to fill that very tab.
    fn discard_pane_palette(&mut self, cx: &mut Context<Self>) {
        self.pane_palette = None;
        cx.notify();
    }

    /// Escape path: nothing is created, and focus goes back to the element
    /// that had it before the picker opened (US-014 AC5). A tab picker closes
    /// its own tab; a split picker only disappears, since it never split
    /// anything.
    pub(crate) fn close_pane_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(palette) = self.pane_palette.take() else {
            return;
        };
        match &palette.placement {
            PalettePlacement::Tab { tab_id } => {
                let position = self
                    .workspaces
                    .iter()
                    .position(|ws| ws.id == palette.ws_id)
                    .and_then(|ws_idx| {
                        self.workspaces[ws_idx]
                            .tabs()
                            .iter()
                            .position(|tab| tab.id == *tab_id)
                            .map(|tab_idx| (ws_idx, tab_idx))
                    });
                if let Some((ws_idx, tab_idx)) = position {
                    self.close_workspace_tab(ws_idx, tab_idx, window, cx);
                }
            }
            PalettePlacement::Split { target, .. } => {
                if let Some(target) = target.upgrade() {
                    target.read(cx).focus_handle(cx).focus(window, cx);
                }
            }
        }
        if let Some(handle) = palette.restore_focus {
            window.focus(&handle, cx);
        }
        cx.notify();
    }

    fn pane_palette_set_error(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        if let Some(palette) = self.pane_palette.as_mut() {
            palette.error = Some(message.into());
            cx.notify();
        }
    }

    /// Launch the preset at `idx` where the picker stands.
    fn pane_palette_launch(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ws_idx) = self.pane_palette_ws_idx() else {
            self.pane_palette_set_error("This project is no longer open", cx);
            return;
        };
        // A pane started now would spawn in the checkout being left behind: the
        // binding is what decides its cwd, and it is not settled yet.
        if let Some(branch) = self.branch_checkout_pending.clone() {
            self.pane_palette_set_error(format!("Checking out {branch}..."), cx);
            return;
        }
        let Some(preset) = self.pane_palette_presets(ws_idx).get(idx).cloned() else {
            return;
        };
        if let Err(message) = preset.ensure_launchable() {
            self.pane_palette_set_error(message, cx);
            return;
        }
        let command = preset.command(&self.cached_config);
        let profile = preset.profile();
        let title = preset.label.clone();
        let placement = match self.pane_palette.as_ref() {
            Some(palette) => match &palette.placement {
                PalettePlacement::Tab { .. } => None,
                PalettePlacement::Split { target, direction } => Some((target.clone(), *direction)),
            },
            None => return,
        };

        match placement {
            None => {
                // The picker *is* this tab, so the preset fills it in place -
                // dropping the state first, otherwise closing the picker would
                // close the tab that is about to receive the pane.
                self.discard_pane_palette(cx);
                self.open_tab_with_surface(ws_idx, title, profile, command, window, cx);
            }
            Some((target, direction)) => {
                let Some(target) = target.upgrade() else {
                    self.pane_palette_set_error("That pane no longer exists", cx);
                    return;
                };
                match self.split_with_target(
                    target,
                    direction,
                    profile,
                    command.as_deref(),
                    window,
                    cx,
                ) {
                    // The refusal (the `MAX_PANES` cap in particular) stays
                    // inside the picker, and the tab is left untouched.
                    Err(message) => self.pane_palette_set_error(message, cx),
                    Ok(()) => self.discard_pane_palette(cx),
                }
            }
        }
    }

    pub(crate) fn handle_pane_palette_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let len = self
            .pane_palette_ws_idx()
            .map_or(0, |ws_idx| self.pane_palette_presets(ws_idx).len());
        let selected = self.pane_palette.as_ref().map_or(0, |p| p.selected);
        let picker_open = self
            .pane_palette
            .as_ref()
            .is_some_and(|palette| palette.branch_picker_open);
        match event.keystroke.key.as_str() {
            // The checkout menu is a layer over the list, so Escape folds it
            // first and only discards the palette once nothing sits above it.
            "escape" if picker_open => {
                if let Some(palette) = self.pane_palette.as_mut() {
                    palette.branch_picker_open = false;
                }
                cx.notify();
            }
            "escape" => self.close_pane_palette(window, cx),
            "enter" => {
                if selected < len {
                    self.pane_palette_launch(selected, window, cx);
                }
            }
            "up" if selected > 0 && selected < len => {
                self.pane_palette_select(selected - 1, cx);
            }
            "down" if selected + 1 < len => {
                self.pane_palette_select(selected + 1, cx);
            }
            _ => {}
        }
    }

    fn pane_palette_select(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(palette) = self.pane_palette.as_mut() {
            palette.selected = idx;
            // Keep the keyboard cursor inside the viewport: the column is
            // taller than its `max_h` as soon as a few agents are visible.
            palette.scroll.scroll_to_item(idx);
            cx.notify();
        }
    }

    pub(crate) fn render_pane_palette(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(palette) = self.pane_palette.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let presets = self
            .pane_palette_ws_idx()
            .map(|ws_idx| self.pane_palette_presets(ws_idx))
            .unwrap_or_default();

        let title = div()
            .flex_none()
            .pb(px(14.))
            .text_size(px(13.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(ui.text)
            .child(PALETTE_TAB_TITLE);

        let mut buttons = div()
            .id("pane-palette-list")
            .flex()
            .flex_col()
            .gap(px(2.))
            .w(px(PICKER_WIDTH))
            .max_h(px(420.))
            .overflow_y_scroll()
            .track_scroll(&palette.scroll);
        for (idx, preset) in presets.iter().enumerate() {
            buttons = buttons.child(self.render_pane_palette_button(idx, preset, palette, ui, cx));
        }

        let mut column = div().flex().flex_col().items_center().child(title);
        if let Some(row) = self.render_palette_branch_row(palette, ui, cx) {
            column = column.child(row);
        }
        column = column.child(buttons);
        if let Some(error) = &palette.error {
            column = column.child(
                div()
                    .pt(px(10.))
                    .max_w(px(PICKER_WIDTH))
                    .text_size(px(11.))
                    .text_color(ui.vc_deleted)
                    .child(error.clone()),
            );
        }

        // A full-size pane card, filled the way `Pane::render` fills one (a
        // superellipse under the subtree). No hairline: the card is a chooser,
        // not a live surface, so it stays flat until a preset turns it into a
        // real pane.
        div()
            .id("pane-palette")
            .size_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.pane_palette_focus)
            .on_key_down(cx.listener(Self::handle_pane_palette_key_down))
            .child(squircle_fill(
                crate::app::constants::PANE_CARD_RADIUS,
                crate::theme::active_theme().background,
            ))
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .child(column),
            )
            .into_any_element()
    }

    /// The palette's tab, as `(ws_idx, tab_idx)`. `None` for a split placement,
    /// which fills an existing pane rather than a tab of its own.
    fn pane_palette_tab(&self, palette: &PanePaletteState) -> Option<(usize, usize)> {
        let PalettePlacement::Tab { tab_id } = &palette.placement else {
            return None;
        };
        let ws_idx = self.pane_palette_ws_idx()?;
        let tab_idx = self.workspaces[ws_idx]
            .tabs()
            .iter()
            .position(|tab| tab.id == *tab_id)?;
        Some((ws_idx, tab_idx))
    }

    /// The tab's branch, as a compact select above the presets.
    ///
    /// Branches, not worktrees: a worktree is only how git gives a second
    /// branch a directory of its own, and what the user is choosing between is
    /// the branch. Picking one that has no checkout yet makes it - see
    /// [`PaneFlowApp::bind_tab_to_branch`] - so the choice reads the way it
    /// does on a forge, where switching branch is one click and where the
    /// directory lives is not the user's problem.
    ///
    /// A control, not a second list: the options live in a floating menu
    /// anchored under the trigger, so opening it never pushes the presets down
    /// and its rows never read as another column of launch buttons. Folding
    /// them into the flow did exactly that - `select_item` cards stacked under
    /// a bare text header looked like the agent buttons they sat above, and
    /// the card was suddenly two lists deep.
    ///
    /// Here rather than in the tab's context menu because of the ordering every
    /// product that does this respects: the branch is chosen BEFORE the
    /// agent's process starts. T3 Code prepares the worktree and writes it on
    /// the thread before dispatching the first turn
    /// (`apps/server/src/ws.ts`), and rebinding a live session there costs a
    /// kill and a restart of the provider process
    /// (`ProviderCommandReactor.ts`, `cwdChanged`). A PTY cannot be resumed
    /// that way, so Paneflow makes the choice while the tab is still empty and
    /// leaves running panes alone afterwards.
    ///
    /// Only for a tab placement: a split lands in an existing tab, whose own
    /// binding already governs where its panes start.
    fn render_palette_branch_row(
        &self,
        palette: &PanePaletteState,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (ws_idx, tab_idx) = self.pane_palette_tab(palette)?;
        let ws = self.workspaces.get(ws_idx)?;
        // Outside a repository there is nothing to switch between.
        let root = ws.repo_root.clone()?;
        let bound = ws.tabs().get(tab_idx)?.worktree.clone();
        let listing = self.workspace_worktree_listing(ws_idx);
        // The branch the tab works on today: the one its worktree holds, or
        // the repository's own when it is unbound.
        let on_branch = match bound.as_ref() {
            Some(path) => listing
                .iter()
                .find(|entry| entry.path == *path)
                .and_then(|entry| entry.branch.clone()),
            None => Some(self.workspace_checkout_label(ws_idx)),
        };

        let mut options: Vec<BranchOption> = self
            .workspace_branches(ws_idx)
            .iter()
            .map(|branch| BranchOption {
                target: BranchTarget::Branch(branch.clone()),
                label: branch.clone(),
                selected: on_branch.as_deref() == Some(branch.as_str()),
                // A branch git already has a directory for costs nothing to
                // switch to; the others are checked out on the spot.
                needs_checkout: !listing
                    .iter()
                    .any(|entry| entry.branch.as_deref() == Some(branch.as_str())),
            })
            .collect();
        // Detached checkouts have no branch to be listed under, and dropping
        // them would strand a tab bound to one - the Codex app creates every
        // worktree of its own that way.
        options.extend(
            listing
                .iter()
                .filter(|entry| entry.branch.is_none() && entry.path != root)
                .map(|entry| BranchOption {
                    label: crate::workspace::worktree::checkout_label(None, &entry.path, &root),
                    target: BranchTarget::Checkout(entry.path.clone()),
                    selected: bound.as_deref() == Some(entry.path.as_path()),
                    needs_checkout: false,
                }),
        );

        // What the trigger names: the branch being checked out while git works,
        // then whatever the tab settled on.
        let current = self.branch_checkout_pending.clone().or_else(|| {
            options
                .iter()
                .find(|option| option.selected)
                .map(|option| option.label.clone())
        });
        let current = current
            .or(on_branch)
            .unwrap_or_else(|| self.workspace_checkout_label(ws_idx));
        let open = palette.branch_picker_open;
        let fill = with_alpha(ui.text, 0.05);

        let trigger = squircle_skin(
            div()
                .id("palette-branch")
                .flex_none()
                .h(px(28.))
                .px(px(8.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.))
                .w(px(PICKER_WIDTH)),
            "palette-branch-group",
            ROW_RADIUS,
            open.then_some(fill),
            Some(fill),
        )
        .cursor(CursorStyle::PointingHand)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // Toggle the render-time snapshot, not the live flag: the menu's
        // `on_mouse_up_out` fires on this same release and has already cleared
        // it, so a live toggle would reopen what the press was closing.
        .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
            if let Some(palette) = this.pane_palette.as_mut() {
                palette.branch_picker_open = !open;
                cx.notify();
            }
        }))
        .child(
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/git-branch-sidebar.svg")
                .text_color(ui.muted),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_size(px(11.))
                .text_color(ui.text)
                .child(current),
        )
        .child(
            svg()
                .size(px(11.))
                .flex_none()
                .path("icons/chevron-down.svg")
                .text_color(ui.muted),
        )
        .when(open, |trigger| {
            let mut menu = select_menu("palette-branch-menu", ui)
                .absolute()
                .top(px(32.))
                .left(px(0.))
                .w(px(PICKER_WIDTH))
                .occlude()
                // Dismiss on release for the reason the sidebar's popover does:
                // the capture-phase `on_mouse_down_out` would close the menu
                // before a row's own click could bubble.
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                        if let Some(palette) = this.pane_palette.as_mut() {
                            palette.branch_picker_open = false;
                            cx.notify();
                        }
                    }),
                );
            for option in options {
                menu =
                    menu.child(self.render_palette_branch_option(option, ws_idx, tab_idx, ui, cx));
            }
            trigger.child(deferred(menu).with_priority(3))
        });

        Some(
            div()
                .flex_none()
                .pb(px(10.))
                .child(trigger)
                .into_any_element(),
        )
    }

    /// One branch in the select's menu: its name, a check when the tab is on
    /// it, and otherwise a hint when picking it will have to make a checkout.
    fn render_palette_branch_option(
        &self,
        option: BranchOption,
        ws_idx: usize,
        tab_idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let BranchOption {
            target,
            label,
            selected,
            needs_checkout,
        } = option;
        let id = SharedString::from(format!("palette-branch-{label}"));
        select_item(id, selected, ui)
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                match target.clone() {
                    BranchTarget::Branch(branch) => {
                        this.bind_tab_to_branch(ws_idx, tab_idx, branch, cx)
                    }
                    BranchTarget::Checkout(path) => {
                        this.set_tab_worktree(ws_idx, tab_idx, Some(path), cx)
                    }
                }
                if let Some(palette) = this.pane_palette.as_mut() {
                    palette.branch_picker_open = false;
                }
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_x_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_color(ui.text)
                    .child(label),
            )
            // One trailing slot, reserved either way so the names stay on one
            // left edge: the check for where the tab is, and for a branch with
            // no directory yet the icon for the one this will make.
            .child(div().w(px(13.)).flex_none().child(if selected {
                svg()
                    .size(px(13.))
                    .path("icons/check.svg")
                    .text_color(ui.text)
                    .into_any_element()
            } else if needs_checkout {
                svg()
                    .size(px(13.))
                    .path("icons/folder-plus.svg")
                    .text_color(with_alpha(ui.muted, 0.7))
                    .into_any_element()
            } else {
                div().size(px(13.)).into_any_element()
            }))
            .into_any_element()
    }

    /// One plain preset button: icon, label, and nothing that folds open.
    fn render_pane_palette_button(
        &self,
        idx: usize,
        preset: &Preset,
        palette: &PanePaletteState,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let launchable = preset.ensure_launchable().is_ok();
        let icon_path = preset.icon_path();
        let icon = if preset.icon_multicolor() {
            gpui::img(icon_path)
                .size(px(14.))
                .flex_none()
                .into_any_element()
        } else {
            svg()
                .size(px(14.))
                .flex_none()
                .path(icon_path)
                .text_color(preset.accent().map_or(ui.text, |c| gpui::rgb(c).into()))
                .into_any_element()
        };

        let mut button = select_item(
            SharedString::from(format!("pane-palette-row-{idx}")),
            idx == palette.selected,
            ui,
        )
        .cursor(CursorStyle::PointingHand)
        .gap(px(8.))
        .h(px(34.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.pane_palette_launch(idx, window, cx);
            cx.stop_propagation();
        }))
        .child(icon)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(if launchable { ui.text } else { ui.muted })
                .child(preset.label.clone()),
        );

        if !launchable {
            button = button.child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child("not installed"),
            );
        }

        button.into_any_element()
    }
}
