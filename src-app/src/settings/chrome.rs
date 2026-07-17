//! Codex-style embedded settings chrome for `PaneFlowApp`.
//!
//! Two entry points, wired into the main window's `Render` impl (`main.rs`):
//! - [`PaneFlowApp::render_settings_nav`] - the grouped left-rail navigation
//!   (back-to-app button + search box + iconed sections), rendered in the
//!   sidebar slot in place of the mode rail while settings are open.
//! - [`PaneFlowApp::render_settings_content_panel`] - the right panel: a big
//!   page title plus the scrollable section body.
//!
//! Section bodies live in `settings::tabs::*`; this file owns the nav, the
//! panel shell, the scroll wrapper, and the section → title/body dispatch.
//!
//! Replaces the old standalone `SettingsWindow` (a separate GPUI window) and
//! the legacy inline `render_settings_page` (a nested mini-sidebar inside the
//! content area). One source of truth now: settings render inline, and the
//! app's own left rail becomes the settings nav.

use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, ParentElement, Point, SharedString, Styled,
    Window, div, prelude::*, px, svg,
};

use crate::ui_primitives::AnimatedHoverExt;
use crate::widgets::scrollbar;
use crate::{PaneFlowApp, SettingsSection};

/// Width of the settings nav rail. Wider than the app rails (Codex's settings
/// sidebar fits grouped, spelled-out section labels). Same units as
/// `SIDEBAR_WIDTH` (raw `f32`, wrapped in `px()` at the use site) so it can
/// feed `sidebar_px` for title-bar brand-slot alignment.
pub(crate) const SETTINGS_NAV_WIDTH: f32 = 260.;

/// Content-panel background - `ui.base` (`#181818`), the same opaque surface the
/// Review / Agents content panels use. Deliberately *lighter* than the `#141414`
/// rail/chrome so the rail-side corner masks (which paint the `#141414` chrome
/// tint over the panel's square corner) actually read as rounded - a content
/// fill equal to the mask color would show no rounding at all. The nav RAIL
/// instead uses `cockpit_chrome_background()` (the platform-aware material /
/// blur-veil treatment shared with the CLI / Review / Agents rails).
pub(crate) fn settings_chrome_bg() -> gpui::Hsla {
    crate::theme::ui_colors().base
}

/// One selectable section row in the nav.
struct NavItem {
    section: SettingsSection,
    label: &'static str,
    icon: &'static str,
    /// Extra lowercase search terms (the controls living on the page) so the
    /// nav search box finds a section by its *content*, not just its label -
    /// e.g. typing "theme", "cursor", or "shell" surfaces the right page.
    keywords: &'static [&'static str],
}

/// A labelled group of nav rows (Codex's "Personnel" / "Intégrations" …).
struct NavGroup {
    label: &'static str,
    items: &'static [NavItem],
}

/// The Codex-style grouped taxonomy. Render order = declaration order.
const NAV_GROUPS: &[NavGroup] = &[
    NavGroup {
        label: "Personal",
        items: &[
            NavItem {
                section: SettingsSection::General,
                label: "General",
                icon: "icons/settings.svg",
                keywords: &["window", "decorations", "mode", "shell", "default shell"],
            },
            NavItem {
                section: SettingsSection::Appearance,
                label: "Themes",
                icon: "icons/palette.svg",
                keywords: &["theme", "themes", "colors", "appearance"],
            },
            NavItem {
                section: SettingsSection::Shortcuts,
                label: "Keyboard Shortcuts",
                icon: "icons/bolt.svg",
                keywords: &["keyboard", "shortcuts", "keys", "bindings", "hotkey"],
            },
            NavItem {
                section: SettingsSection::Notifications,
                label: "Notifications",
                icon: "icons/bell.svg",
                keywords: &[
                    "notifications",
                    "native",
                    "os",
                    "system",
                    "bell",
                    "toast",
                    "agent",
                ],
            },
        ],
    },
    NavGroup {
        label: "Terminal",
        items: &[
            NavItem {
                section: SettingsSection::Terminal,
                label: "Terminal",
                icon: "icons/terminal.svg",
                keywords: &["cursor", "font", "font family", "font size"],
            },
            NavItem {
                section: SettingsSection::Workspaces,
                label: "Workspaces",
                icon: "icons/layout-grid.svg",
                keywords: &[
                    "workspace",
                    "workspaces",
                    "project",
                    "layout",
                    "pane",
                    "panes",
                    "flow",
                    "toml",
                    "agent",
                    "command",
                ],
            },
        ],
    },
    NavGroup {
        label: "Integrations",
        items: &[
            NavItem {
                section: SettingsSection::AiAgent,
                label: "AI Agent",
                icon: "icons/sparkles.svg",
                keywords: &[
                    "ai",
                    "agent",
                    "claude",
                    "codex",
                    "gemini",
                    "bypass",
                    "permissions",
                    "launcher",
                    "tab bar",
                ],
            },
            NavItem {
                section: SettingsSection::McpServers,
                label: "MCP Servers",
                icon: "icons/server.svg",
                keywords: &["mcp", "bridge", "server", "integration"],
            },
        ],
    },
];

/// Human page title shown as the content H1.
pub(crate) fn section_title(section: SettingsSection) -> &'static str {
    match section {
        SettingsSection::General => "General",
        SettingsSection::Appearance => "Themes",
        SettingsSection::Shortcuts => "Keyboard Shortcuts",
        SettingsSection::Terminal => "Terminal",
        SettingsSection::Notifications => "Notifications",
        SettingsSection::AiAgent => "AI Agent",
        SettingsSection::McpServers => "MCP Servers",
        SettingsSection::Workspaces => "Workspaces",
    }
}

impl PaneFlowApp {
    /// The grouped settings navigation rail (sidebar slot while settings open).
    pub(crate) fn render_settings_nav(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let theme = crate::theme::active_theme();
        let active = self.settings_section.unwrap_or(SettingsSection::General);
        let query = self.settings_search_input.read(cx).value().to_lowercase();
        let nav_hover_bg = crate::app::constants::sidebar_tab_hover_background();

        // ── Back-to-app row ─────────────────────────────────────────────
        let back = div()
            .id("settings-back")
            .mx(px(8.))
            .mb(px(6.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .animated_hover_bg(nav_hover_bg.opacity(0.0), nav_hover_bg)
            .on_click(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.close_settings(cx);
                cx.notify();
            }))
            .child(
                svg()
                    .size(px(14.))
                    .flex_none()
                    .path("icons/arrow_left.svg")
                    .text_color(ui.muted),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::NORMAL)
                    .text_color(ui.muted)
                    .child("Back to app"),
            );

        // ── Search box ──────────────────────────────────────────────────
        let search = self.render_settings_search(ui, window, cx);

        // ── Section list (scrollable, filtered by the search query) ─────
        let mut list = div()
            .id("settings-nav-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .pb(px(8.));

        let mut any_match = false;
        for group in NAV_GROUPS {
            let items: Vec<&NavItem> = group
                .items
                .iter()
                .filter(|it| {
                    query.is_empty()
                        || it.label.to_lowercase().contains(&query)
                        || it.keywords.iter().any(|k| k.contains(query.as_str()))
                })
                .collect();
            if items.is_empty() {
                continue;
            }
            any_match = true;
            list = list.child(
                div()
                    .px(px(14.))
                    .pt(px(14.))
                    .pb(px(6.))
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.muted)
                    .child(group.label),
            );
            for it in items {
                let section = it.section;
                let is_active = section == active;
                // Every section row renders in full-strength text (white),
                // active or not - Codex keeps all labels at one legible color
                // and signals the active row through the pill fill + the medium
                // font weight alone, not a muted/bright color split.
                let fg = ui.text;
                let row = div()
                    .id(SharedString::from(format!("settings-nav-{}", it.label)))
                    .mx(px(8.))
                    .my(px(1.))
                    .px(px(8.))
                    .py(px(6.))
                    .rounded(px(8.))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.))
                    .child(svg().size(px(15.)).flex_none().path(it.icon).text_color(fg))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .font_weight(FontWeight::NORMAL)
                            .text_color(fg)
                            .truncate()
                            .child(it.label),
                    );
                let row = if is_active {
                    row.bg(crate::app::constants::sidebar_tab_active_background())
                        .into_any_element()
                } else {
                    row.animated_hover_bg(nav_hover_bg.opacity(0.0), nav_hover_bg)
                        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                            this.select_settings_section(section, window, cx);
                        }))
                        .into_any_element()
                };
                list = list.child(row);
            }
        }

        if !any_match {
            list = list.child(
                div()
                    .px(px(14.))
                    .pt(px(14.))
                    .text_size(px(12.))
                    .text_color(ui.muted)
                    .child("No matching settings"),
            );
        }

        div()
            .id("settings-nav")
            .w(px(SETTINGS_NAV_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            // Same platform-aware rail treatment as the CLI / Review / Agents
            // sidebars: optional native material on Windows, platform default
            // on macOS, and a blur veil on Linux when the compositor advertises
            // it. Keeps the settings rail visually identical to the other rails.
            .bg(crate::app::constants::cockpit_chrome_background(
                theme.title_bar_background,
                window.is_window_active(),
                self.cached_config.cockpit_chrome_material_enabled(),
            ))
            .pt(px(6.))
            .child(back)
            .child(div().px(px(8.)).pb(px(10.)).child(search))
            .child(list)
    }

    /// The nav search field - a real single-line `TextInput` (cursor, arrow
    /// keys, clipboard, mouse selection), read from `value()` at render to
    /// filter the section list. Mirrors the agents-sidebar filter recipe.
    fn render_settings_search(
        &self,
        ui: crate::theme::UiColors,
        _window: &Window,
        cx: &mut Context<Self>,
        // Returns a concrete `AnyElement` (not `impl IntoElement`) so the
        // value does not capture `cx`'s borrow under edition-2024 RPIT - the
        // nav loop reborrows `cx` for its per-row `on_click` listeners.
    ) -> AnyElement {
        // Codex-style search pill: a filled `ui.subtle` gray, borderless, and
        // fully inert - nothing changes on focus or hover. Matches the select
        // triggers so the settings chrome reads as one system; the blinking
        // caret is the only focus cue.
        div()
            .id("settings-search")
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(ui.subtle)
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.))
            .cursor_text()
            // Two-stage Escape (keyboard parity with the Back button): clear the
            // query if any, otherwise - already empty - close settings outright.
            // Cursor movement / Delete / Ctrl+A,C,V,X / mouse selection are
            // handled inside the focused TextInput.
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _window, cx| {
                if ev.keystroke.key == "escape" {
                    if this.settings_search_input.read(cx).value().is_empty() {
                        this.close_settings(cx);
                    } else {
                        this.settings_search_input.update(cx, |inp, cx| {
                            inp.clear(cx);
                        });
                    }
                    cx.notify();
                    cx.stop_propagation();
                }
            }))
            // Clicking outside drops focus so the caret disappears.
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this
                    .settings_search_input
                    .read(cx)
                    .focus_handle
                    .is_focused(window)
                {
                    window.blur();
                    cx.notify();
                }
            }))
            .child(
                svg()
                    .size(px(13.))
                    .flex_none()
                    .path("icons/tool_search.svg")
                    .text_color(ui.muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(px(12.))
                    .text_color(ui.text)
                    .child(self.settings_search_input.clone()),
            )
            .into_any_element()
    }

    /// The right content panel: the section H1 title + the scrollable body.
    pub(crate) fn render_settings_content_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let section = self.settings_section.unwrap_or(SettingsSection::General);

        let body = match section {
            SettingsSection::General => self.render_general_content(cx).into_any_element(),
            SettingsSection::Appearance => self.render_appearance_content(cx).into_any_element(),
            SettingsSection::Shortcuts => self.render_shortcuts_content(cx).into_any_element(),
            SettingsSection::Terminal => self.render_terminal_content(cx).into_any_element(),
            SettingsSection::Notifications => {
                self.render_notifications_content(cx).into_any_element()
            }
            SettingsSection::AiAgent => self.render_ai_agent_content(cx).into_any_element(),
            SettingsSection::McpServers => self.render_mcp_servers_content(cx).into_any_element(),
            SettingsSection::Workspaces => self.render_workspaces_content(cx).into_any_element(),
        };

        let ipc_banner = self.ipc_status.is_disabled().then(|| {
            use crate::widgets::callout::{Callout, CalloutIcon, CalloutSeverity};
            div().pb(px(16.)).child(
                Callout::new(CalloutSeverity::Warning, "IPC offline")
                    .icon(CalloutIcon::TriangleAlert)
                    .description("External clients (paneflow-ai-hook) will not connect.")
                    .render(),
            )
        });

        let title = div()
            .pb(px(20.))
            .text_size(px(26.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ui.text)
            .child(section_title(section));

        let column = div()
            .flex()
            .flex_col()
            .child(title)
            .when_some(ipc_banner, |d, b| d.child(b))
            .child(body)
            .into_any_element();

        div()
            .id("settings-panel")
            .track_focus(&self.settings_focus)
            .on_key_down(cx.listener(Self::handle_settings_key_down))
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .bg(settings_chrome_bg())
            .child(self.render_settings_scroll(column, cx))
    }

    /// Scrollable content area + visible scrollbar overlay. Centers a
    /// max-width reading column (Codex's settings content is a centered
    /// column, not full-bleed). Drag state lives on `PaneFlowApp`
    /// (`settings_scroll` / `settings_drag`).
    fn render_settings_scroll(
        &self,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let inner = div()
            .id("settings-content")
            .absolute()
            .top_0()
            .right_0()
            .bottom_0()
            .left_0()
            .min_h_0()
            .pr(scrollbar::SCROLLBAR_GUTTER)
            .bg(settings_chrome_bg())
            .overflow_y_scroll()
            .track_scroll(&self.settings_scroll)
            .flex()
            .flex_col()
            .items_start()
            .child(
                div()
                    .w_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .max_w(px(700.))
                    .mx_auto()
                    .px(px(28.))
                    .pt(px(28.))
                    .pb(px(72.))
                    .child(content),
            );

        let bar = scrollbar::render(
            &self.settings_scroll,
            crate::theme::ui_colors(),
            None,
            "settings-scrollbar-track",
            "settings-scrollbar-thumb",
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                if let Some(off) =
                    scrollbar::track_click_offset(&this.settings_scroll, ev.position.y)
                {
                    this.settings_scroll.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }),
            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.settings_drag =
                    Some(scrollbar::begin_drag(&this.settings_scroll, ev.position.y));
                cx.stop_propagation();
            }),
        );

        div()
            .id("settings-content-wrapper")
            .relative()
            .flex_1()
            .flex()
            .flex_col()
            .min_h_0()
            .on_scroll_wheel(cx.listener(|_, _, _, cx| cx.notify()))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if let Some(drag) = this.settings_drag
                    && let Some(off) =
                        scrollbar::drag_offset(&this.settings_scroll, &drag, ev.position.y)
                {
                    this.settings_scroll.set_offset(Point::new(px(0.), px(off)));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.settings_drag.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(inner)
            .when_some(bar, |d, sb| d.child(sb))
    }

    /// Switch the active settings section, resetting any per-page ephemeral UI
    /// (font picker, terminal dropdowns, in-progress shortcut recording) so a
    /// popover never lingers across a nav change. Warms the MCP status when
    /// the MCP page is opened.
    pub(crate) fn select_settings_section(
        &mut self,
        section: SettingsSection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.settings_section = Some(section);
        self.reset_settings_scroll();
        self.font_dropdown_open = false;
        self.font_search.clear();
        self.terminal_dropdown = None;
        self.general_dropdown = None;
        self.workspace_template_dropdown = None;
        self.workspace_template_detail_open = false;
        if self.recording_shortcut_idx.is_some() {
            self.recording_shortcut_idx = None;
            let config = paneflow_config::loader::load_config();
            crate::keybindings::apply_keybindings(cx, &config.shortcuts);
        }
        if section == SettingsSection::McpServers {
            self.refresh_mcp_status(cx);
        }
        if section == SettingsSection::Workspaces {
            self.sync_workspace_template_inputs(cx);
        }
        self.settings_focus.focus(window, cx);
        cx.notify();
    }
}
