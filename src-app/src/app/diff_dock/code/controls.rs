use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    AnyElement, AppContext, Context, Entity, EventEmitter, FocusHandle, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, Window,
    deferred, div, prelude::FluentBuilder, px, svg,
};

use super::view::CodeView;
use crate::app::menu_icons as icon;
use crate::ui_primitives::action_menu::{ActionMenu, MenuHandlers, MenuItem};
use crate::ui_primitives::{AnimatedHoverExt, TooltipDelayExt, text_tooltip};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct EditorDisplay {
    pub(crate) minimap: bool,
    pub(crate) scrollbar: bool,
    pub(crate) word_wrap: bool,
    pub(crate) markdown_rendered: bool,
    pub(crate) csv_table: bool,
}

impl EditorDisplay {
    pub(crate) fn from_config(config: &paneflow_config::schema::EditorDisplayConfig) -> Self {
        Self {
            minimap: config.minimap_enabled(),
            scrollbar: config.scrollbar_enabled(),
            word_wrap: config.word_wrap_enabled(),
            markdown_rendered: config.markdown_rendered_enabled(),
            csv_table: config.csv_table_enabled(),
        }
    }

    pub(crate) fn to_config_value(self) -> serde_json::Value {
        serde_json::json!({
            "minimap": self.minimap,
            "scrollbar": self.scrollbar,
            "word_wrap": self.word_wrap,
            "markdown_rendered": self.markdown_rendered,
            "csv_table": self.csv_table,
        })
    }
}

static EDITOR_MINIMAP: AtomicBool = AtomicBool::new(false);
static EDITOR_SCROLLBAR: AtomicBool = AtomicBool::new(true);
static EDITOR_WORD_WRAP: AtomicBool = AtomicBool::new(false);
static EDITOR_MARKDOWN_RENDERED: AtomicBool = AtomicBool::new(false);
static EDITOR_CSV_TABLE: AtomicBool = AtomicBool::new(true);

pub(crate) fn set_editor_display(display: EditorDisplay) {
    EDITOR_MINIMAP.store(display.minimap, Ordering::Relaxed);
    EDITOR_SCROLLBAR.store(display.scrollbar, Ordering::Relaxed);
    EDITOR_WORD_WRAP.store(display.word_wrap, Ordering::Relaxed);
    EDITOR_MARKDOWN_RENDERED.store(display.markdown_rendered, Ordering::Relaxed);
    EDITOR_CSV_TABLE.store(display.csv_table, Ordering::Relaxed);
}

pub(crate) fn editor_display() -> EditorDisplay {
    EditorDisplay {
        minimap: EDITOR_MINIMAP.load(Ordering::Relaxed),
        scrollbar: EDITOR_SCROLLBAR.load(Ordering::Relaxed),
        word_wrap: EDITOR_WORD_WRAP.load(Ordering::Relaxed),
        markdown_rendered: EDITOR_MARKDOWN_RENDERED.load(Ordering::Relaxed),
        csv_table: EDITOR_CSV_TABLE.load(Ordering::Relaxed),
    }
}

pub(crate) struct EditorControls {
    open: bool,
    editor_focus: FocusHandle,
    word_wrap: bool,
}

#[derive(Clone, Copy)]
enum EditorControl {
    WordWrap,
    Minimap,
    Scrollbar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WordWrapToggled(pub(crate) bool);

impl EditorControls {
    pub(crate) fn attach(editor_focus: FocusHandle, cx: &mut Context<CodeView>) -> Entity<Self> {
        let controls = cx.new(|_| Self {
            open: false,
            editor_focus,
            word_wrap: editor_display().word_wrap,
        });
        cx.observe(&controls, |_, _, cx| cx.notify()).detach();
        cx.subscribe(
            &controls,
            |view: &mut CodeView, _, toggled: &WordWrapToggled, cx| {
                view.set_word_wrap(toggled.0, cx);
            },
        )
        .detach();
        controls
    }

    #[cfg(test)]
    pub(crate) fn word_wrap(&self) -> bool {
        self.word_wrap
    }

    #[cfg(test)]
    pub(crate) fn toggle_word_wrap_for_test(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.choose(EditorControl::WordWrap, window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        window.focus(&self.editor_focus, cx);
        cx.notify();
    }

    fn choose(&mut self, control: EditorControl, window: &mut Window, cx: &mut Context<Self>) {
        let mut display = editor_display();
        match control {
            EditorControl::WordWrap => {
                self.word_wrap = !self.word_wrap;
                cx.emit(WordWrapToggled(self.word_wrap));
                self.close(window, cx);
                return;
            }
            EditorControl::Minimap => display.minimap = !display.minimap,
            EditorControl::Scrollbar => display.scrollbar = !display.scrollbar,
        }
        cx.emit(display);
        self.close(window, cx);
    }

    fn menu(&self, cx: &mut Context<Self>) -> AnyElement {
        let ui = crate::theme::ui_colors();
        let display = editor_display();
        let menu = ActionMenu::new()
            .item(
                MenuItem::new("Word Wrap", EditorControl::WordWrap)
                    .id("code-editor-control-word-wrap")
                    .icon(icon::WORD_WRAP)
                    .mnemonic('w')
                    .toggle(self.word_wrap),
            )
            .item(
                MenuItem::new("Minimap", EditorControl::Minimap)
                    .id("code-editor-control-minimap")
                    .icon(icon::MINIMAP)
                    .mnemonic('m')
                    .toggle(display.minimap),
            )
            .item(
                MenuItem::new("Scrollbar", EditorControl::Scrollbar)
                    .id("code-editor-control-scrollbar")
                    .icon(icon::SCROLLBAR)
                    .mnemonic('s')
                    .toggle(display.scrollbar),
            );
        let handlers = MenuHandlers {
            activate: Rc::new(cx.listener(|this, control: &EditorControl, window, cx| {
                this.choose(*control, window, cx);
            })),
            dismiss: Rc::new(cx.listener(|this, _: &(), window, cx| this.close(window, cx))),
        };

        deferred(crate::ui_primitives::menu_reveal(
            "code-editor-controls-menu-reveal",
            div()
                .absolute()
                .top(px(32.))
                .right_0()
                .occlude()
                .child(menu.element("code-editor-controls-menu", handlers, ui)),
        ))
        .with_priority(3)
        .into_any_element()
    }
}

impl EventEmitter<EditorDisplay> for EditorControls {}
impl EventEmitter<WordWrapToggled> for EditorControls {}

impl Render for EditorControls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui = crate::theme::ui_colors();
        let hover = crate::app::constants::sidebar_tab_hover_background();
        let open = self.open;
        div()
            .id("code-editor-controls")
            .role(gpui::Role::Button)
            .aria_label("Editor Controls")
            .aria_expanded(open)
            .relative()
            .flex_none()
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .animated_hover_bg(
                if open {
                    hover
                } else {
                    gpui::transparent_black()
                },
                hover,
            )
            .delayed_tooltip(text_tooltip("Editor Controls"))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, window, cx| {
                if open {
                    this.close(window, cx);
                } else {
                    this.open = true;
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .child(
                svg()
                    .size(px(16.))
                    .path("icons/dock/sliders-horizontal.svg")
                    .text_color(if open { ui.text } else { ui.muted }),
            )
            .when(open, |button| button.child(self.menu(cx)))
    }
}
