use gpui::{
    AnyElement, ClickEvent, Context, CursorStyle, Entity, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseUpEvent, ParentElement,
    ScrollHandle, SharedString, Styled, WeakEntity, Window, deferred, div, prelude::*, px, svg,
};
use paneflow_config::schema::{ButtonCommand, PaneFlowConfig, TerminalSurfaceProfile};

use crate::PaneFlowApp;
use crate::agent_launcher::AgentLaunch;
use crate::app::workspace_ops::SurfaceLaunch;
use crate::layout::SplitDirection;
use crate::pane::Pane;
use crate::settings::components::{menu_divider_color, select_item, select_menu, with_alpha};
use crate::ui_primitives::squircle::squircle_fill;
use crate::ui_primitives::{ROW_RADIUS, squircle_skin};
use crate::widgets::text_input::TextInput;

pub(crate) const PALETTE_TAB_TITLE: &str = "New pane";

const PICKER_WIDTH: f32 = 260.0;

pub(crate) enum PalettePlacement {
    Tab {
        tab_id: u64,
    },
    Split {
        target: WeakEntity<Pane>,
        direction: SplitDirection,
    },
}

#[derive(Clone)]
enum BranchTarget {
    Branch(String),
    Checkout(std::path::PathBuf),
}

struct BranchOption {
    target: BranchTarget,
    label: String,
    selected: bool,
    needs_checkout: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum PresetSource {
    Shell,
    Agent(AgentLaunch),
    Custom(ButtonCommand),
}

#[derive(Debug, Clone)]
pub(crate) struct Preset {
    pub(crate) label: String,
    pub(crate) source: PresetSource,
}

impl Preset {
    fn icon_path(&self) -> SharedString {
        match &self.source {
            PresetSource::Shell => "icons/terminal.svg".into(),
            PresetSource::Agent(launch) => launch.agent().icon_path().into(),
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
        matches!(&self.source, PresetSource::Agent(launch) if launch.agent().icon_multicolor())
    }

    fn accent(&self) -> Option<u32> {
        match &self.source {
            PresetSource::Agent(launch) => launch.agent().accent(),
            _ => None,
        }
    }

    fn profile(&self) -> TerminalSurfaceProfile {
        match &self.source {
            PresetSource::Agent(_) => TerminalSurfaceProfile::Agent,
            _ => TerminalSurfaceProfile::Normal,
        }
    }

    fn command(&self, config: &PaneFlowConfig) -> Option<String> {
        match &self.source {
            PresetSource::Shell => None,
            PresetSource::Agent(launch) => Some(launch.launch_command(config)),
            PresetSource::Custom(button) => Some(button.command.clone()),
        }
    }

    fn env(&self) -> Option<std::collections::HashMap<String, String>> {
        match &self.source {
            PresetSource::Agent(launch) => launch.process_env(),
            _ => None,
        }
    }

    fn ensure_launchable(&self) -> Result<(), String> {
        match &self.source {
            PresetSource::Agent(launch) if !launch.is_installed() => Err(format!(
                "{} is not installed - install its CLI, or hide it in Settings > Agents",
                launch.agent().display_name()
            )),
            _ => Ok(()),
        }
    }
}

pub(crate) struct NewBranchDraft {
    pub(crate) name: Entity<TextInput>,
    pub(crate) base: Option<String>,
    pub(crate) base_open: bool,
    pub(crate) worktree: bool,
}

pub(crate) struct PanePaletteState {
    pub(crate) ws_id: u64,
    pub(crate) placement: PalettePlacement,
    pub(crate) selected: usize,
    pub(crate) error: Option<String>,
    pub(crate) restore_focus: Option<FocusHandle>,
    pub(crate) scroll: ScrollHandle,
    pub(crate) branch_picker_open: bool,
    pub(crate) new_branch: Option<NewBranchDraft>,
    pub(crate) pending_launch: Option<usize>,
}

impl PaneFlowApp {
    pub(crate) fn pane_palette_presets(&self, ws_idx: usize) -> Vec<Preset> {
        let mut presets = vec![Preset {
            label: "Terminal".to_string(),
            source: PresetSource::Shell,
        }];
        presets.extend(
            AgentLaunch::visible(&self.cached_config)
                .into_iter()
                .map(|launch| Preset {
                    label: launch.label(),
                    source: PresetSource::Agent(launch),
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

    fn pane_palette_ws_idx(&self) -> Option<usize> {
        let ws_id = self.pane_palette.as_ref()?.ws_id;
        self.workspaces.iter().position(|ws| ws.id == ws_id)
    }

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
            ws.sidebar_expanded = true;
        }

        self.spawn_worktree_listing(ws_idx, cx);
        self.pane_palette = Some(PanePaletteState {
            ws_id,
            placement: PalettePlacement::Tab { tab_id },
            selected: 0,
            error: None,
            restore_focus,
            scroll: ScrollHandle::new(),
            branch_picker_open: false,
            new_branch: None,
            pending_launch: None,
        });
        let tab_idx = self.workspaces[ws_idx].active_tab_idx();
        self.focus_workspace_tab(ws_idx, tab_idx, window, cx);
        window.focus(&self.pane_palette_focus, cx);
        cx.notify();
    }

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
            branch_picker_open: false,
            new_branch: None,
            pending_launch: None,
        });
        self.pending_palette_focus = true;
        cx.notify();
    }

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

    pub(crate) fn pending_split_palette(&self) -> Option<(Entity<Pane>, SplitDirection)> {
        let palette = self.pane_palette.as_ref()?;
        let PalettePlacement::Split { target, direction } = &palette.placement else {
            return None;
        };
        let target = target.upgrade()?;
        (self.active_workspace()?.id == palette.ws_id).then_some((target, *direction))
    }

    fn discard_pane_palette(&mut self, cx: &mut Context<Self>) {
        self.pane_palette = None;
        cx.notify();
    }

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

    pub(crate) fn pane_palette_launch(
        &mut self,
        idx: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ws_idx) = self.pane_palette_ws_idx() else {
            self.pane_palette_set_error("This project is no longer open", cx);
            return;
        };
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
        if self.pane_palette_create_branch_then_launch(idx, cx) {
            return;
        }
        let launch = SurfaceLaunch {
            command: preset.command(&self.cached_config),
            env: preset.env(),
        };
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
                self.discard_pane_palette(cx);
                self.open_tab_with_surface(ws_idx, title, profile, launch, window, cx);
            }
            Some((target, direction)) => {
                let Some(target) = target.upgrade() else {
                    self.pane_palette_set_error("That pane no longer exists", cx);
                    return;
                };
                match self.split_with_target(target, direction, profile, launch, window, cx) {
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
        let base_open = self.pane_palette.as_ref().is_some_and(|palette| {
            palette
                .new_branch
                .as_ref()
                .is_some_and(|draft| draft.base_open)
        });
        let draft_open = self
            .pane_palette
            .as_ref()
            .is_some_and(|palette| palette.new_branch.is_some());
        match event.keystroke.key.as_str() {
            "escape" if picker_open => {
                if let Some(palette) = self.pane_palette.as_mut() {
                    palette.branch_picker_open = false;
                }
                cx.notify();
            }
            "escape" if base_open => {
                if let Some(draft) = self
                    .pane_palette
                    .as_mut()
                    .and_then(|palette| palette.new_branch.as_mut())
                {
                    draft.base_open = false;
                }
                cx.notify();
            }
            "escape" if draft_open => self.pane_palette_close_new_branch(window, cx),
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

    pub(crate) fn pane_palette_open_new_branch(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = self.pane_palette.as_ref() else {
            return;
        };
        if palette.new_branch.is_some() {
            return;
        }
        let base = self
            .pane_palette_tab(palette)
            .map(|(ws_idx, _)| self.workspace_checkout_label(ws_idx))
            .filter(|label| !label.is_empty() && label != "Project root");
        let name = cx.new(|cx| TextInput::new("", "Branch name (optional)", cx));
        let focus = name.read(cx).focus_handle.clone();
        if let Some(palette) = self.pane_palette.as_mut() {
            palette.branch_picker_open = false;
            palette.error = None;
            palette.new_branch = Some(NewBranchDraft {
                name,
                base,
                base_open: false,
                worktree: self.cached_config.worktrees.new_branches_use_worktrees(),
            });
        }
        window.focus(&focus, cx);
        cx.notify();
    }

    fn pane_palette_close_new_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(palette) = self.pane_palette.as_mut() {
            palette.new_branch = None;
            palette.error = None;
        }
        window.focus(&self.pane_palette_focus, cx);
        cx.notify();
    }

    fn pane_palette_create_branch_then_launch(
        &mut self,
        preset_idx: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(palette) = self.pane_palette.as_ref() else {
            return false;
        };
        let Some(draft) = palette.new_branch.as_ref() else {
            return false;
        };
        let Some((ws_idx, tab_idx)) = self.pane_palette_tab(palette) else {
            return false;
        };
        let branch = draft.name.read(cx).value().trim().to_string();
        let base = draft.base.clone();
        let worktree = draft.worktree;
        if let Some(palette) = self.pane_palette.as_mut() {
            palette.error = None;
        }
        if worktree {
            self.create_branch_for_tab(ws_idx, tab_idx, branch, base, Some(preset_idx), cx);
        } else {
            self.switch_checkout_for_tab(ws_idx, tab_idx, branch, base, Some(preset_idx), cx);
        }
        true
    }

    pub(crate) fn pane_palette_branch_created(
        &mut self,
        tab_id: u64,
        launch_preset: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(palette) = self.pane_palette.as_mut() else {
            return;
        };
        if !matches!(palette.placement, PalettePlacement::Tab { tab_id: id } if id == tab_id) {
            return;
        }
        palette.new_branch = None;
        palette.error = None;
        palette.pending_launch = launch_preset;
        cx.notify();
    }

    pub(crate) fn pane_palette_branch_failed(
        &mut self,
        tab_id: u64,
        message: String,
        cx: &mut Context<Self>,
    ) {
        let owns_tab = self.pane_palette.as_ref().is_some_and(|palette| {
            matches!(palette.placement, PalettePlacement::Tab { tab_id: id } if id == tab_id)
        });
        if owns_tab {
            self.pane_palette_set_error(message, cx);
        } else {
            self.show_toast(message, cx);
        }
    }

    pub(crate) fn take_pane_palette_pending_launch(&mut self) -> Option<usize> {
        self.pane_palette
            .as_mut()
            .and_then(|palette| palette.pending_launch.take())
    }

    fn pane_palette_select(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(palette) = self.pane_palette.as_mut() {
            palette.selected = idx;
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

        let draft_open = palette.new_branch.is_some();
        let mut title = div()
            .flex_none()
            .relative()
            .w(px(PICKER_WIDTH))
            .pb(px(14.))
            .flex()
            .justify_center()
            .text_size(px(13.))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(ui.text)
            .child(if draft_open {
                "New branch"
            } else {
                PALETTE_TAB_TITLE
            });
        if draft_open {
            title = title.child(
                div()
                    .id("palette-new-branch-back")
                    .absolute()
                    .left(px(0.))
                    .top(px(-2.))
                    .size(px(20.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(with_alpha(ui.text, 0.06)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.pane_palette_close_new_branch(window, cx);
                        cx.stop_propagation();
                    }))
                    .child(
                        svg()
                            .size(px(12.))
                            .path("icons/chevron-left.svg")
                            .text_color(ui.muted),
                    ),
            );
        }

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

    fn render_palette_branch_row(
        &self,
        palette: &PanePaletteState,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (ws_idx, tab_idx) = self.pane_palette_tab(palette)?;
        let ws = self.workspaces.get(ws_idx)?;
        let root = ws.repo_root.clone()?;
        let bound = ws.tabs().get(tab_idx)?.worktree.clone();
        let listing = self.workspace_worktree_listing(ws_idx);
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
                needs_checkout: !listing
                    .iter()
                    .any(|entry| entry.branch.as_deref() == Some(branch.as_str())),
            })
            .collect();
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

        let current = self.branch_checkout_pending.clone().or_else(|| {
            options
                .iter()
                .find(|option| option.selected)
                .map(|option| option.label.clone())
        });
        let current = current
            .or(on_branch)
            .unwrap_or_else(|| self.workspace_checkout_label(ws_idx));
        if let Some(draft) = palette.new_branch.as_ref() {
            return Some(self.render_new_branch_form(draft, ws_idx, ui, cx));
        }
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
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                        if let Some(palette) = this.pane_palette.as_mut() {
                            palette.branch_picker_open = false;
                            cx.notify();
                        }
                    }),
                );
            menu = menu
                .child(
                    select_item("palette-branch-new", false, ui)
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.pane_palette_open_new_branch(window, cx);
                            cx.stop_propagation();
                        }))
                        .child(
                            svg()
                                .size(px(12.))
                                .flex_none()
                                .path("icons/plus.svg")
                                .text_color(ui.text),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(ui.text)
                                .child("New branch…"),
                        ),
                )
                .child(
                    div()
                        .h(px(1.))
                        .mx(px(8.))
                        .my(px(4.))
                        .bg(menu_divider_color(ui)),
                );
            for option in options {
                menu =
                    menu.child(self.render_palette_branch_option(option, ws_idx, tab_idx, ui, cx));
            }
            trigger.child(
                deferred(crate::ui_primitives::menu_reveal(
                    "palette-branch-menu-reveal",
                    menu,
                ))
                .with_priority(3),
            )
        });

        Some(crate::ui_primitives::menu_reveal(
            "palette-branch-row-reveal",
            div().flex_none().pb(px(10.)).child(trigger),
        ))
    }

    fn render_new_branch_form(
        &self,
        draft: &NewBranchDraft,
        ws_idx: usize,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let base_label = draft.base.clone().unwrap_or_else(|| "HEAD".to_string());
        let base_open = draft.base_open;
        let worktree = draft.worktree;
        let branches: Vec<String> = self.workspace_branches(ws_idx).to_vec();

        let branch_field = div()
            .h(px(28.))
            .w(px(PICKER_WIDTH))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .bg(ui.subtle)
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
                    .text_size(px(11.))
                    .text_color(ui.text)
                    .child(draft.name.clone()),
            )
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        let mut base_trigger = div()
            .id("palette-new-branch-base")
            .relative()
            .h(px(28.))
            .w(px(PICKER_WIDTH))
            .mt(px(4.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .when(base_open, |row| row.bg(with_alpha(ui.text, 0.05)))
            .hover(|style| style.bg(with_alpha(ui.text, 0.05)))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.))
                    .text_color(ui.muted)
                    .child("from"),
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
                    .child(base_label.clone()),
            )
            .child(
                svg()
                    .size(px(11.))
                    .flex_none()
                    .path("icons/chevron-down.svg")
                    .text_color(ui.muted),
            )
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                if let Some(draft) = this
                    .pane_palette
                    .as_mut()
                    .and_then(|palette| palette.new_branch.as_mut())
                {
                    draft.base_open = !base_open;
                    cx.notify();
                }
            }));
        if base_open {
            let mut menu = select_menu("palette-new-branch-base-menu", ui)
                .absolute()
                .top(px(32.))
                .left(px(0.))
                .w(px(PICKER_WIDTH))
                .occlude()
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _: &MouseUpEvent, _w, cx| {
                        if let Some(draft) = this
                            .pane_palette
                            .as_mut()
                            .and_then(|palette| palette.new_branch.as_mut())
                        {
                            draft.base_open = false;
                            cx.notify();
                        }
                    }),
                );
            for branch in branches {
                let selected = draft.base.as_deref() == Some(branch.as_str());
                let pick = branch.clone();
                menu = menu.child(
                    select_item(
                        SharedString::from(format!("palette-new-branch-base-{branch}")),
                        selected,
                        ui,
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                        if let Some(draft) = this
                            .pane_palette
                            .as_mut()
                            .and_then(|palette| palette.new_branch.as_mut())
                        {
                            draft.base = Some(pick.clone());
                            draft.base_open = false;
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(ui.text)
                            .child(branch),
                    )
                    .child(div().w(px(13.)).flex_none().child(if selected {
                        svg()
                            .size(px(13.))
                            .path("icons/check.svg")
                            .text_color(ui.text)
                            .into_any_element()
                    } else {
                        div().size(px(13.)).into_any_element()
                    })),
                );
            }
            base_trigger = base_trigger.child(
                deferred(crate::ui_primitives::menu_reveal(
                    "palette-new-branch-base-menu-reveal",
                    menu,
                ))
                .with_priority(3),
            );
        }

        let worktree_row = div()
            .id("palette-new-branch-worktree")
            .h(px(28.))
            .w(px(PICKER_WIDTH))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .rounded(px(6.))
            .hover(|style| style.bg(with_alpha(ui.text, 0.05)))
            .cursor(CursorStyle::PointingHand)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _: &ClickEvent, _w, cx| {
                if let Some(draft) = this
                    .pane_palette
                    .as_mut()
                    .and_then(|palette| palette.new_branch.as_mut())
                {
                    draft.worktree = !worktree;
                }
                let mut next = this.cached_config.worktrees.clone();
                next.for_new_branches = Some(!worktree);
                this.persist_worktrees(next, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(11.))
                    .text_color(ui.text)
                    .child("Worktree"),
            )
            .child(
                div()
                    .flex_none()
                    .text_size(px(10.))
                    .text_color(ui.muted)
                    .child(if worktree {
                        "own folder"
                    } else {
                        "switch this checkout"
                    }),
            )
            .child(compact_toggle(worktree, ui));

        let form = div()
            .flex_none()
            .w(px(PICKER_WIDTH))
            .pb(px(4.))
            .flex()
            .flex_col()
            .child(branch_field)
            .child(base_trigger)
            .child(worktree_row)
            .child(
                div()
                    .h(px(1.))
                    .mx(px(2.))
                    .mt(px(12.))
                    .mb(px(10.))
                    .bg(menu_divider_color(ui)),
            )
            .child(
                div()
                    .pb(px(4.))
                    .px(px(8.))
                    .text_size(px(10.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(ui.muted)
                    .child("Open with"),
            );
        crate::ui_primitives::menu_reveal("palette-new-branch-form-reveal", form)
    }

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

fn compact_toggle(on: bool, ui: crate::theme::UiColors) -> impl IntoElement {
    let track_bg = if on {
        gpui::Hsla::from(gpui::rgb(0x339cff))
    } else {
        with_alpha(ui.muted, 0.30)
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .w(px(26.))
        .h(px(16.))
        .rounded_full()
        .px(px(2.))
        .bg(track_bg)
        .when(on, |track| track.justify_end())
        .child(div().size(px(12.)).rounded_full().bg(gpui::white()))
}
