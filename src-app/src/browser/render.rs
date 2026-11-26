use std::rc::Rc;

use gpui::{
    AnyElement, Context, Corners, Div, HitboxBehavior, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window,
    canvas, deferred, div, prelude::FluentBuilder, px, size, svg,
};
use paneflow_api::methods::browser::TabOwner;
use paneflow_browser_proto::surface::{DialogKind, HistoryStep};

use super::inspector::DevToolsKey;
use super::view::{BrowserMenu, BrowserView, PageState, host_of};
use super::{address, omnibox};
use crate::settings::kit::{self, Ink, Size, Tone};
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};
use crate::ui_primitives::{AccessibleControlExt, TooltipDelayExt};

pub(crate) const CHROME_HEIGHT: f32 = 40.;
pub(crate) const CHROME_PADDING: f32 = 6.;
pub(crate) const CHROME_GAP: f32 = 2.;
pub(crate) const ADDRESS_HEIGHT: f32 = 28.;
pub(crate) const ADDRESS_RADIUS: f32 = 8.;
pub(crate) const ADDRESS_MIN_WIDTH: f32 = 120.;
pub(crate) const SUGGESTION_GAP: f32 = 4.;
pub(crate) const SUGGESTION_RADIUS: f32 = 12.;
pub(crate) const SUGGESTION_PADDING: f32 = 4.;
pub(crate) const SUGGESTION_ROW_HEIGHT: f32 = 28.;
pub(crate) const SUGGESTION_ROW_RADIUS: f32 = 8.;
pub(crate) const SUGGESTION_ROW_INSET: f32 = 4.;
pub(crate) const SUGGESTION_REMOVE_RADIUS: f32 = 5.;
pub(crate) const SUGGESTION_ICON_LEFT: f32 =
    ADDRESS_ICON_INSET + (ADDRESS_ICON_BOX - ADDRESS_ICON_SIZE) / 2. - SUGGESTION_PADDING;
pub(crate) const SUGGESTION_ICON_GAP: f32 =
    ADDRESS_ICON_INSET + ADDRESS_ICON_BOX + ADDRESS_TEXT_GAP
        - SUGGESTION_PADDING
        - SUGGESTION_ICON_LEFT
        - ADDRESS_ICON_SIZE;
pub(crate) const SUGGESTION_SHADOW_BLUR: f32 = 12.;
pub(crate) const SUGGESTION_PANEL_LIFT: f32 = 0.16;
pub(crate) const SUGGESTION_LINK_WHITE: f32 = 0.45;
pub(crate) const ADDRESS_PADDING: f32 = 10.;
pub(crate) const ADDRESS_TEXT_SIZE: f32 = 13.;
pub(crate) const ADDRESS_ICON_INSET: f32 = 2.;
pub(crate) const ADDRESS_ICON_BOX: f32 = 24.;
pub(crate) const ADDRESS_ICON_SIZE: f32 = 16.;
pub(crate) const ADDRESS_TEXT_GAP: f32 = 5.;
pub(crate) const ADDRESS_REST_OPACITY: f32 = 0.83;
pub(crate) const ADDRESS_FILL: (f32, f32) = (0.10, 0.08);
pub(crate) const ADDRESS_FILL_HOVER: (f32, f32) = (0.14, 0.12);
pub(crate) const STRIP_HEIGHT: f32 = 28.;
pub(crate) const MESSAGE_WIDTH: f32 = 360.;
pub(crate) const MESSAGE_ACTIONS_GAP: f32 = 8.;

pub(crate) fn popup_fill() -> gpui::Hsla {
    let ui = crate::theme::ui_colors();
    let light = ui.surface.l > 0.5;
    super::loader::mix(
        ui.surface.opacity(1.),
        ui.text.opacity(1.),
        if light { 0. } else { SUGGESTION_PANEL_LIFT },
    )
}

pub(crate) fn popup_panel(ink: Ink) -> Div {
    let light = crate::theme::ui_colors().surface.l > 0.5;
    let panel = popup_fill();
    div()
        .rounded(px(SUGGESTION_RADIUS))
        .bg(panel)
        .border_1()
        .border_color(ink.edge)
        .shadow(vec![gpui::BoxShadow {
            color: gpui::black().opacity(if light { 0.12 } else { 0.32 }),
            offset: gpui::point(px(0.), px(4.)),
            blur_radius: px(SUGGESTION_SHADOW_BLUR),
            spread_radius: px(0.),
            inset: false,
        }])
        .text_color(ink.primary)
        .occlude()
}

#[derive(Clone)]
enum MenuChoice {
    Show(paneflow_browser_proto::TabId),
    HandTo(usize),
    Annotate(u64),
    StopAnnotating,
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_focus(window);
        let ui = crate::theme::ui_colors();
        let ink = Ink::of(ui);
        let state = self.page_state();
        self.takes_input = state.takes_input();
        if self.inspects.is_some() {
            return div()
                .id("browser-inspector")
                .size_full()
                .flex()
                .flex_col()
                .overflow_hidden()
                .child(self.render_page(state, ink, window, cx));
        }
        let page = self.render_page(state, ink, window, cx);
        let body = self.render_docked(page, ink, cx);
        div()
            .id("browser-view")
            .size_full()
            .text_color(ink.primary)
            .flex()
            .flex_col()
            .overflow_hidden()
            .key_context("Browser")
            .on_action(cx.listener(|this, _: &crate::ToggleDevTools, window, cx| {
                this.devtools_shortcut(DevToolsKey::Toggle, window, cx);
            }))
            .on_action(
                cx.listener(|this, _: &crate::ShowDevToolsConsole, window, cx| {
                    this.devtools_shortcut(DevToolsKey::Console, window, cx);
                }),
            )
            .on_action(cx.listener(|this, _: &crate::InspectElement, window, cx| {
                this.devtools_shortcut(DevToolsKey::Inspect, window, cx);
            }))
            .on_action(cx.listener(|this, _: &crate::FontSizeIncrease, _, cx| this.zoom_by(1, cx)))
            .on_action(cx.listener(|this, _: &crate::FontSizeDecrease, _, cx| this.zoom_by(-1, cx)))
            .on_action(cx.listener(|this, _: &crate::FontSizeReset, _, cx| this.zoom_by(0, cx)))
            .child(self.render_chrome(ink, window, cx))
            .children(self.render_strip(ink, cx))
            .child(body)
            .children(self.render_menu(ui, cx))
            .children(self.render_suggestions(ink, window, cx))
            .children(self.render_site_info(ink, cx))
    }
}

impl BrowserView {
    fn render_chrome(&mut self, ink: Ink, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = self.snapshot.clone();
        let can_back = snapshot.as_ref().is_some_and(|s| s.can_go_back);
        let can_forward = snapshot.as_ref().is_some_and(|s| s.can_go_forward);
        let loading = snapshot.as_ref().is_some_and(|s| s.loading) || self.opening;
        let nav = |id: &'static str, label: &'static str, icon: &'static str, enabled: bool| {
            kit::icon_button(id, label, icon, ink).when(!enabled, |button| button.opacity(0.4))
        };
        let back = nav("browser-back", "Back", "icons/chevron-left.svg", can_back).when(
            can_back,
            |button| {
                button.on_click(cx.listener(|this, _, _, cx| this.history(HistoryStep::Back, cx)))
            },
        );
        let forward = nav(
            "browser-forward",
            "Forward",
            "icons/chevron-right.svg",
            can_forward,
        )
        .when(can_forward, |button| {
            button.on_click(cx.listener(|this, _, _, cx| this.history(HistoryStep::Forward, cx)))
        });
        let (reload_label, reload_icon) = if loading {
            ("Stop loading", "icons/close.svg")
        } else {
            ("Reload", "icons/refresh.svg")
        };
        let reload = kit::icon_button("browser-reload", reload_label, reload_icon, ink)
            .on_click(cx.listener(|this, _, _, cx| this.reload_or_stop(cx)));
        let editing = self.address.read(cx).focus_handle.is_focused(window);
        let shown = (!editing && self.address_error.is_none())
            .then(|| address::display(&self.url()))
            .flatten();
        let light = crate::theme::ui_colors().surface.l > 0.5;
        let fill = |alpha: (f32, f32)| ink.primary.opacity(if light { alpha.1 } else { alpha.0 });
        let entity = cx.entity();
        let accent = ink.accent;
        let loader = canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let reduced = crate::ui_primitives::reduce_motion();
                let frame = entity
                    .update(cx, |view, _| {
                        view.omnibox.bounds = Some(bounds);
                        view.loader.frame(std::time::Instant::now(), reduced)
                    })
                    .filter(|_| !editing);
                if let Some(frame) = frame {
                    super::loader::paint(bounds, ADDRESS_RADIUS, frame, accent, window);
                }
                if !reduced && entity.read(cx).loader.is_active() {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .inset_0();
        let standing = self.standing();
        let site_open = self.site_info.is_some();
        let site_icon = shown.as_ref().zip(standing).map(|(_, standing)| {
            let entity = cx.entity();
            div()
                .id("browser-site-icon")
                .relative()
                .flex_none()
                .ml(px(ADDRESS_ICON_INSET))
                .size(px(ADDRESS_ICON_BOX))
                .rounded(px(ADDRESS_RADIUS - ADDRESS_ICON_INSET))
                .flex()
                .items_center()
                .justify_center()
                .cursor(gpui::CursorStyle::PointingHand)
                .when(site_open, |icon| icon.bg(fill(ADDRESS_FILL_HOVER)))
                .hover(|icon| icon.bg(fill(ADDRESS_FILL_HOVER)))
                .accessible_control(gpui::accesskit::Role::Button, "View site information")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| this.toggle_site_info(window, cx)))
                .child(
                    canvas(
                        move |bounds, _, cx| {
                            entity.update(cx, |view, _| view.site_icon_bounds = Some(bounds));
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(
                    svg()
                        .path(standing.icon())
                        .size(px(ADDRESS_ICON_SIZE))
                        .text_color(standing.color(ink)),
                )
        });
        let field = match &shown {
            Some(shown) => div()
                .id("browser-address-display")
                .flex_1()
                .min_w_0()
                .h_full()
                .pl(px(ADDRESS_TEXT_GAP))
                .pr(px(ADDRESS_PADDING))
                .flex()
                .items_center()
                .overflow_hidden()
                .whitespace_nowrap()
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        let focus = this.address.read(cx).focus_handle.clone();
                        window.focus(&focus, cx);
                        let url = this.url();
                        this.show_in_field(url, true, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .flex_none()
                        .text_color(ink.primary)
                        .child(shown.host.clone()),
                )
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(ink.primary.opacity(ADDRESS_REST_OPACITY))
                        .child(shown.rest.clone()),
                )
                .into_any_element(),
            None => div()
                .flex_1()
                .min_w_0()
                .px(px(ADDRESS_PADDING))
                .child(self.address.clone())
                .into_any_element(),
        };
        let zoom_mark = self
            .site_zoom(cx)
            .filter(|(_, percent, default)| percent != default)
            .map(|(_, percent, default)| {
                div()
                    .flex_none()
                    .pr(px(ADDRESS_ICON_INSET))
                    .child(
                        kit::button(
                            "browser-zoom",
                            format!("{percent}%"),
                            Tone::Ghost,
                            Size::Small,
                            ink,
                        )
                        .debug_selector(move || format!("browser-zoom-{percent}-of-{default}"))
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_by(0, cx))),
                    )
                    .into_any_element()
            });
        let address = div()
            .id("browser-address")
            .relative()
            .flex_1()
            .min_w(px(ADDRESS_MIN_WIDTH))
            .h(px(ADDRESS_HEIGHT))
            .rounded(px(ADDRESS_RADIUS))
            .bg(fill(ADDRESS_FILL))
            .hover(|field| field.bg(fill(ADDRESS_FILL_HOVER)))
            .flex()
            .items_center()
            .text_size(px(ADDRESS_TEXT_SIZE))
            .when(self.address_error.is_some(), |field| {
                field.border_1().border_color(ink.danger)
            })
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => {
                        this.submit_address(cx);
                        if this.address_error.is_none() {
                            window.focus(&this.focus, cx);
                        }
                        cx.stop_propagation();
                    }
                    "escape" => {
                        if !this.close_suggestions(cx) {
                            this.reset_address(cx);
                            window.focus(&this.focus, cx);
                        }
                        cx.stop_propagation();
                    }
                    "up" | "down" => {
                        let step = if event.keystroke.key == "up" { -1 } else { 1 };
                        if this.move_selection(step, cx) {
                            cx.stop_propagation();
                        }
                    }
                    "delete"
                        if event.keystroke.modifiers.shift
                            && this.forget_row(this.omnibox.selected, cx) =>
                    {
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .children(site_icon)
            .child(field)
            .children(zoom_mark)
            .child(loader);
        let owner = self.render_owner(ink, cx);
        let tabs = kit::icon_button("browser-tabs", "Browser tabs", "icons/list.svg", ink)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(BrowserMenu::Tabs, cx)));
        div()
            .flex_none()
            .h(px(CHROME_HEIGHT))
            .px(px(CHROME_PADDING))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(CHROME_GAP))
            .child(back)
            .child(forward)
            .child(reload)
            .child(div().w(px(4.)))
            .child(address)
            .children(owner)
            .child(tabs)
            .into_any_element()
    }

    fn render_owner(&self, ink: Ink, cx: &mut Context<Self>) -> Option<AnyElement> {
        let snapshot = self.snapshot.as_ref()?;
        let badge = match &snapshot.owner {
            TabOwner::Agent { agent, .. } => {
                let name = agent.clone().unwrap_or_else(|| "An agent".to_string());
                let label = SharedString::from(format!("{name} drives this tab"));
                Some(
                    kit::badge(ink, label.clone())
                        .id("browser-owner-badge")
                        .flex_shrink_1()
                        .min_w_0()
                        .truncate()
                        .delayed_tooltip(crate::ui_primitives::text_tooltip(label))
                        .into_any_element(),
                )
            }
            TabOwner::Human | TabOwner::Unclaimed => None,
        };
        let annotating = self.annotating();
        let pointer = kit::icon_button(
            "browser-pointer",
            if annotating {
                "Annotating elements"
            } else {
                "Annotate or hand to an agent"
            },
            "icons/mouse-pointer-2.svg",
            ink,
        )
        .when(annotating, |button| button.bg(ink.accent.opacity(0.22)))
        .on_click(cx.listener(|this, _, _, cx| this.toggle_menu(BrowserMenu::Pointer, cx)))
        .into_any_element();
        Some(
            div()
                .flex_shrink_1()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(CHROME_GAP))
                .pl(px(4.))
                .children(badge)
                .child(pointer)
                .into_any_element(),
        )
    }

    fn render_strip(&self, ink: Ink, cx: &mut Context<Self>) -> Option<AnyElement> {
        let strip = |text: String, color| {
            div()
                .flex_none()
                .min_h(px(STRIP_HEIGHT))
                .px(px(12.))
                .py(px(4.))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.))
                .text_size(px(12.))
                .text_color(color)
                .child(div().flex_1().min_w_0().child(text))
        };
        if let Some(reason) = &self.address_error {
            return Some(strip(reason.clone(), ink.danger).into_any_element());
        }
        let snapshot = self.snapshot.as_ref()?;
        if let Some(refusal) = &snapshot.refusal {
            return Some(
                strip(
                    format!(
                        "Paneflow did not open this address: {refusal}. The page stays as it was."
                    ),
                    ink.danger,
                )
                .into_any_element(),
            );
        }
        if snapshot.taken_over_from.is_some() && matches!(snapshot.owner, TabOwner::Human) {
            return Some(
                strip(
                    "You took over this tab. The agent's next action is refused until you hand it back.".to_string(),
                    ink.secondary,
                )
                .into_any_element(),
            );
        }
        if let Some(held) = &snapshot.held {
            return Some(
                strip(
                    format!("Waiting for your approval to open {}.", held.origin),
                    ink.secondary,
                )
                .into_any_element(),
            );
        }
        if let Some(failure) = &snapshot.failure {
            return Some(
                strip(format!("The page did not load: {failure}"), ink.danger)
                    .child(
                        kit::button(
                            "browser-strip-reload",
                            "Reload",
                            Tone::Secondary,
                            Size::Small,
                            ink,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.reload_or_stop(cx))),
                    )
                    .into_any_element(),
            );
        }
        None
    }

    fn render_page(
        &mut self,
        state: PageState,
        ink: Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let image = self.image.clone();
        let frame = self.frame_size;
        let cursor = self.cursor;
        let focus = self.focus.clone();
        let painted = state == PageState::Page || matches!(state, PageState::Crashed(_));
        let surface = canvas(
            {
                let entity = entity.clone();
                move |bounds, window, cx| {
                    let scale = window.scale_factor();
                    entity.update(cx, |view, _| view.place(bounds, scale));
                    window.insert_hitbox(bounds, HitboxBehavior::Normal)
                }
            },
            move |bounds, hitbox, window, cx| {
                entity.update(cx, |view, _| view.drop_retired(window));
                if let Some(image) = image.filter(|_| painted) {
                    let scale = window.scale_factor().max(0.5);
                    let image_bounds = gpui::Bounds {
                        origin: bounds.origin,
                        size: size(px(frame.0 as f32 / scale), px(frame.1 as f32 / scale)),
                    };
                    let _ = window.paint_image(
                        bounds,
                        image_bounds,
                        Corners::default(),
                        image,
                        0,
                        false,
                    );
                }
                window.set_cursor_style(cursor, &hitbox);
                let accepts = {
                    let view = entity.read(cx);
                    view.tab.is_some() && view.takes_input
                };
                if accepts {
                    let handler = gpui::ElementInputHandler::new(bounds, entity.clone());
                    window.handle_input(&focus, handler, cx);
                }
            },
        )
        .size_full();
        let overlay = self.render_state(state, ink, cx);
        let dialog = self.render_dialog(ink, window, cx);
        div()
            .id("browser-page")
            .key_context("BrowserPage")
            .track_focus(&self.focus)
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .bg(self
                .backing
                .map_or(crate::theme::ui_colors().surface, gpui::Hsla::from))
            .on_mouse_down(MouseButton::Left, cx.listener(BrowserView::mouse_down))
            .on_mouse_down(MouseButton::Middle, cx.listener(BrowserView::mouse_down))
            .on_mouse_down(MouseButton::Right, cx.listener(BrowserView::mouse_down))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, event, _, cx| this.mouse_up(event, cx)),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, _| {
                let inside = this
                    .page_bounds
                    .is_some_and(|bounds| bounds.contains(&event.position));
                this.mouse_move(event, inside);
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, _| {
                if !*hovered {
                    this.mouse_left();
                }
            }))
            .on_scroll(cx.listener(|this, event, _, cx| this.scroll(event, cx)))
            .on_key_down(cx.listener(|this, event, _, cx| this.key_down(event, cx)))
            .on_key_up(cx.listener(|this, event, _, cx| this.key_up(event, cx)))
            .child(surface)
            .children(overlay)
            .children(dialog)
            .into_any_element()
    }

    fn render_state(
        &self,
        state: PageState,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let message = |title: String, body: String| {
            div()
                .max_w(px(MESSAGE_WIDTH))
                .px(px(24.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(8.))
                .text_center()
                .child(
                    div()
                        .text_size(px(13.))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(ink.primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(ink.secondary)
                        .child(body),
                )
        };
        let actions = || {
            div()
                .pt(px(MESSAGE_ACTIONS_GAP))
                .flex()
                .flex_row()
                .justify_center()
                .gap(px(8.))
        };
        let column = match state {
            PageState::Page => return None,
            PageState::Empty => message(
                "Open a page".to_string(),
                "Type a local address such as localhost:3000 in the address field.".to_string(),
            ),
            PageState::Opening => message(
                "Opening the page".to_string(),
                format!(
                    "Paneflow is starting the browser engine for {}.",
                    self.url()
                ),
            ),
            PageState::NotInstalled => message(
                "The browser engine is not installed".to_string(),
                "Paneflow needs its browser engine to show pages. No browser process was started."
                    .to_string(),
            ),
            PageState::Refused(reason) => {
                message("Paneflow did not open this address".to_string(), reason)
            }
            PageState::Failed(reason) => message("The page could not open".to_string(), reason),
            PageState::Crashed(reason) => message(
                "The page crashed".to_string(),
                format!("{reason}. Reload to start the page again."),
            )
            .child(
                actions().child(
                    kit::button(
                        "browser-crash-reload",
                        "Reload",
                        Tone::Primary,
                        Size::Medium,
                        ink,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.reload_or_stop(cx))),
                ),
            ),
            PageState::PresentationFailed(reason) => message(
                "Paneflow cannot show this page".to_string(),
                format!("{reason}. Agents can still drive this tab."),
            ),
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(crate::theme::ui_colors().surface)
                .child(column)
                .into_any_element(),
        )
    }

    fn render_dialog(
        &self,
        ink: Ink,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let open = self.dialog.as_ref()?;
        let dialog = &open.dialog;
        let title = match dialog.kind {
            DialogKind::Alert | DialogKind::Confirm | DialogKind::Prompt => {
                format!("{} says", dialog.origin)
            }
        };
        let cancel = (dialog.kind != DialogKind::Alert).then(|| {
            kit::button(
                "browser-dialog-cancel",
                "Cancel",
                Tone::Secondary,
                Size::Small,
                ink,
            )
            .on_click(cx.listener(|this, _, _, cx| this.answer_dialog(false, cx)))
            .into_any_element()
        });
        let ok = kit::button("browser-dialog-ok", "OK", Tone::Primary, Size::Small, ink)
            .on_click(cx.listener(|this, _, _, cx| this.answer_dialog(true, cx)))
            .into_any_element();
        if let Some(text) = &open.text
            && !text.read(cx).focus_handle.contains_focused(window, cx)
            && self.focus.is_focused(window)
        {
            let handle = text.read(cx).focus_handle.clone();
            window.defer(cx, move |window, cx| window.focus(&handle, cx));
        }
        let field: Option<Div> = open.text.as_ref().map(|text| {
            div()
                .h(px(ADDRESS_HEIGHT))
                .px(px(10.))
                .rounded(px(ADDRESS_RADIUS))
                .bg(ink.well)
                .flex()
                .items_center()
                .text_size(px(12.))
                .child(text.clone())
        });
        let body = div()
            .w(px(MESSAGE_WIDTH))
            .p(px(16.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .rounded(px(12.))
            .bg(crate::settings::components::select_menu_surface(
                crate::theme::ui_colors(),
            ))
            .border_1()
            .border_color(ink.edge)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "enter" => this.answer_dialog(true, cx),
                    "escape" => this.answer_dialog(false, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(ink.primary)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(ink.secondary)
                    .child(SharedString::from(dialog.message.clone())),
            )
            .children(field)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.))
                    .children(cancel)
                    .child(ok),
            );
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_start()
                .justify_center()
                .pt(px(24.))
                .bg(ink.scrim)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(body)
                .into_any_element(),
        )
    }

    fn render_suggestions(
        &self,
        ink: Ink,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let bounds = self.omnibox.bounds?;
        let editing = self.address.read(cx).focus_handle.is_focused(window);
        if !editing || !self.omnibox.open || self.omnibox.suggestions.rows.is_empty() {
            return None;
        }
        let ui = crate::theme::ui_colors();
        let light = ui.surface.l > 0.5;
        let link = if light {
            ink.accent
        } else {
            super::loader::mix(ink.accent, gpui::white(), SUGGESTION_LINK_WHITE)
        };
        let selected_fill = ink.primary.opacity(if light { 0.08 } else { 0.10 });
        let bold = gpui::HighlightStyle {
            font_weight: Some(gpui::FontWeight::BOLD),
            ..gpui::HighlightStyle::default()
        };
        let marked = |text: &str, marks: &[std::ops::Range<usize>]| {
            gpui::StyledText::new(SharedString::from(text.to_string())).with_highlights(
                marks
                    .iter()
                    .filter(|range| range.end <= text.len())
                    .map(|range| (range.clone(), bold)),
            )
        };
        let rows = self
            .omnibox
            .suggestions
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let selected = index == self.omnibox.selected;
                let (icon, title, detail): (&str, Option<AnyElement>, Option<AnyElement>) =
                    match &row.kind {
                        omnibox::RowKind::Search { query, engine } => (
                            "icons/tool_search.svg",
                            Some(div().child(query.clone()).into_any_element()),
                            Some(
                                div()
                                    .text_color(ink.secondary)
                                    .child(format!("Search {engine}"))
                                    .into_any_element(),
                            ),
                        ),
                        omnibox::RowKind::Typed => (
                            "icons/world.svg",
                            Some(div().child(row.shown.clone()).into_any_element()),
                            None,
                        ),
                        omnibox::RowKind::DevServer { label } => (
                            "icons/server.svg",
                            Some(
                                marked(label.as_deref().unwrap_or("Dev server"), &row.title_marks)
                                    .into_any_element(),
                            ),
                            Some(
                                div()
                                    .text_color(link)
                                    .child(marked(&row.shown, &row.shown_marks))
                                    .into_any_element(),
                            ),
                        ),
                        omnibox::RowKind::Page { title } => (
                            "icons/world.svg",
                            title
                                .as_deref()
                                .map(|title| marked(title, &row.title_marks).into_any_element()),
                            Some(
                                div()
                                    .text_color(link)
                                    .child(marked(&row.shown, &row.shown_marks))
                                    .into_any_element(),
                            ),
                        ),
                    };
                let separated = title.is_some() && detail.is_some();
                let remove = (row.removable() && selected).then(|| {
                    div()
                        .id(("browser-suggestion-forget", index))
                        .flex_none()
                        .size(px(SUGGESTION_ROW_HEIGHT - 8.))
                        .rounded(px(SUGGESTION_REMOVE_RADIUS))
                        .flex()
                        .items_center()
                        .justify_center()
                        .hover(|button| button.bg(ink.primary.opacity(0.12)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.forget_row(index, cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(
                            svg()
                                .path("icons/close.svg")
                                .size(px(12.))
                                .text_color(ink.secondary),
                        )
                });
                div()
                    .id(("browser-suggestion", index))
                    .h(px(SUGGESTION_ROW_HEIGHT))
                    .pl(px(SUGGESTION_ICON_LEFT))
                    .pr(px(SUGGESTION_ROW_INSET))
                    .rounded(px(SUGGESTION_ROW_RADIUS))
                    .flex()
                    .items_center()
                    .gap(px(SUGGESTION_ICON_GAP))
                    .when(selected, |row| row.bg(selected_fill))
                    .cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _, _, cx| {
                        if this.omnibox.selected != index {
                            this.omnibox.selected = index;
                            cx.notify();
                        }
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            this.open_row(index, cx);
                            window.focus(&this.focus, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(
                        svg()
                            .path(icon)
                            .flex_none()
                            .size(px(ADDRESS_ICON_SIZE))
                            .text_color(ink.secondary),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .children(title.map(|title| {
                                div()
                                    .flex_none()
                                    .max_w(gpui::relative(0.6))
                                    .truncate()
                                    .child(title)
                            }))
                            .when(separated, |line| {
                                line.child(div().flex_none().text_color(ink.secondary).child(" - "))
                            })
                            .children(
                                detail.map(|detail| div().min_w_0().truncate().child(detail)),
                            ),
                    )
                    .children(remove)
                    .into_any_element()
            })
            .collect::<Vec<_>>();
        let popup = popup_panel(ink)
            .id("browser-suggestions")
            .w(bounds.size.width)
            .p(px(SUGGESTION_PADDING))
            .text_size(px(ADDRESS_TEXT_SIZE))
            .text_color(ink.primary)
            .occlude()
            .flex()
            .flex_col()
            .children(rows);
        Some(
            deferred(
                gpui::anchored()
                    .position(gpui::point(
                        bounds.origin.x,
                        bounds.origin.y + bounds.size.height + px(SUGGESTION_GAP),
                    ))
                    .child(popup),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }

    fn render_menu(
        &self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let menu = self.menu?;
        let actions: ActionMenu<MenuChoice> = match menu {
            BrowserMenu::Tabs => {
                let items: Vec<MenuItem<MenuChoice>> = if self.listed.is_empty() {
                    vec![MenuItem::disabled("No browser tabs in this workspace")]
                } else {
                    self.listed
                        .iter()
                        .map(|tab| {
                            let label = tab
                                .title
                                .clone()
                                .filter(|title| !title.trim().is_empty())
                                .or_else(|| host_of(&tab.url))
                                .unwrap_or_else(|| tab.url.clone());
                            let owner = match &tab.owner {
                                TabOwner::Agent { agent, .. } => {
                                    agent.clone().unwrap_or_else(|| "An agent".to_string())
                                }
                                TabOwner::Human => "You".to_string(),
                                TabOwner::Unclaimed => "Unclaimed".to_string(),
                            };
                            MenuItem::new(label, MenuChoice::Show(tab.tab)).detail(owner)
                        })
                        .collect()
                };
                ActionMenu::new().heading("Browser tabs").items(items)
            }
            BrowserMenu::Pointer => {
                let annotate = match self.targets.as_slice() {
                    _ if self.annotating() => {
                        MenuItem::new("Stop annotating", MenuChoice::StopAnnotating)
                    }
                    [] => MenuItem::disabled("No agent to annotate for"),
                    [only] => MenuItem::new(
                        format!("Annotate for {}", only.title),
                        MenuChoice::Annotate(only.surface_id),
                    ),
                    targets => MenuItem::submenu(
                        "Annotate for",
                        crate::app::agent_send::agent_choices(targets, None, MenuChoice::Annotate),
                    ),
                }
                .icon("icons/mouse-pointer-2.svg");
                let agent_owned = self
                    .owner()
                    .is_some_and(|owner| matches!(owner, TabOwner::Agent { .. }));
                let hand = (!agent_owned).then(|| {
                    if self.agents.is_empty() {
                        MenuItem::disabled("No agent to hand this tab to")
                    } else {
                        MenuItem::submenu(
                            "Hand to agent",
                            ActionMenu::new().items(self.agents.iter().enumerate().map(
                                |(index, agent)| {
                                    MenuItem::new(agent.label.clone(), MenuChoice::HandTo(index))
                                },
                            )),
                        )
                    }
                    .icon("icons/chat-bot.svg")
                });
                ActionMenu::new().items(std::iter::once(annotate).chain(hand))
            }
        };
        let handlers = MenuHandlers {
            activate: Rc::new(
                cx.listener(|this, choice: &MenuChoice, window, cx| match choice {
                    MenuChoice::Show(tab) => this.show_listed(*tab, cx),
                    MenuChoice::HandTo(index) => {
                        if let Some(agent) = this.agents.get(*index).cloned() {
                            this.hand_to(&agent, cx);
                        }
                    }
                    MenuChoice::Annotate(surface_id) => {
                        let ink = Ink::of(crate::theme::ui_colors());
                        this.annotate_for(*surface_id, ink, window, cx);
                    }
                    MenuChoice::StopAnnotating => {
                        this.menu = None;
                        this.stop_annotating(cx);
                    }
                }),
            ),
            dismiss: Rc::new(cx.listener(|this, _: &(), _, cx| {
                this.menu = None;
                cx.notify();
            })),
        };
        let element = actions.element("browser-menu", handlers, ui);
        let placed = div()
            .absolute()
            .top(px(CHROME_HEIGHT))
            .right(px(CHROME_PADDING))
            .occlude()
            .child(element);
        Some(deferred(placed).with_priority(3).into_any_element())
    }
}
