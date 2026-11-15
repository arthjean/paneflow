use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, BoxShadow, ClickEvent, CursorStyle, Div, ElementId, Entity,
    FocusHandle, FontWeight, Hsla, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Role, SharedString, Size, Stateful,
    StatefulInteractiveElement, Styled, TextRun, Toggled, Window, canvas, deferred, div, font,
    prelude::*, px, size, svg,
};

use crate::settings::components::{select_menu_surface, with_alpha};
use crate::theme::UiColors;
use crate::ui_primitives::TooltipDelayExt;

pub(crate) const PANEL_RADIUS: Pixels = px(8.);
pub(crate) const PANEL_PADDING: Pixels = px(4.);
pub(crate) const PANEL_BORDER: Pixels = px(1.);
pub(crate) const ROW_HEIGHT: Pixels = px(28.);
pub(crate) const ROW_RADIUS: Pixels = px(4.);
pub(crate) const ROW_INSET: Pixels = px(8.);
pub(crate) const ROW_GAP: Pixels = px(8.);
pub(crate) const GLYPH_SIZE: Pixels = px(16.);
pub(crate) const CHECK_SIZE: Pixels = px(14.);
pub(crate) const LABEL_TEXT: Pixels = px(14.);
pub(crate) const SHORTCUT_TEXT: Pixels = px(12.);
pub(crate) const SEPARATOR_MARGIN: Pixels = px(4.);
pub(crate) const MAX_HEIGHT: Pixels = px(400.);
pub(crate) const FONT_FAMILY: &str = "Geist";
pub(crate) const EDGE_ALPHA: f32 = 0.10;
pub(crate) const SEPARATOR_ALPHA: f32 = 0.08;
pub(crate) const HIGHLIGHT_ALPHA: f32 = 0.06;
pub(crate) const DISABLED_OPACITY: f32 = 0.64;
const TRAILING_GAP: Pixels = px(16.);
const HEADING_TEXT: Pixels = px(12.);
const HEADING_HEIGHT: Pixels = px(28.);
const MIN_WIDTH: Pixels = px(160.);
const MAX_WIDTH: Pixels = px(320.);
const SUBMENU_GAP: Pixels = px(4.);
const SUBMENU_PRIORITY: usize = 6;
const GLYPH_ALPHA: f32 = 0.8;

pub(crate) type MenuElement = Rc<dyn Fn() -> AnyElement>;

#[derive(Clone)]
pub(crate) enum MenuGlyph {
    Icon(SharedString),
    Element(MenuElement),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
    None,
    Toggle(bool),
    Radio(bool),
}

pub(crate) struct MenuItem<A> {
    id: Option<SharedString>,
    glyph: Option<MenuGlyph>,
    label: SharedString,
    detail: Option<SharedString>,
    mnemonic: Option<char>,
    shortcut: Option<SharedString>,
    action: Option<A>,
    submenu: Option<ActionMenu<A>>,
    check: Check,
    keep_open: bool,
    tooltip: Option<&'static str>,
    trailing: Option<(Pixels, MenuElement)>,
    description: Option<SharedString>,
}

impl<A> MenuItem<A> {
    fn blank(label: SharedString) -> Self {
        Self {
            id: None,
            glyph: None,
            label,
            detail: None,
            mnemonic: None,
            shortcut: None,
            action: None,
            submenu: None,
            check: Check::None,
            keep_open: false,
            tooltip: None,
            trailing: None,
            description: None,
        }
    }

    pub(crate) fn new(label: impl Into<SharedString>, action: A) -> Self {
        Self {
            action: Some(action),
            ..Self::blank(label.into())
        }
    }

    pub(crate) fn disabled(label: impl Into<SharedString>) -> Self {
        Self::blank(label.into())
    }

    pub(crate) fn submenu(label: impl Into<SharedString>, children: ActionMenu<A>) -> Self {
        Self {
            submenu: Some(children),
            ..Self::blank(label.into())
        }
    }

    pub(crate) fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    pub(crate) fn icon(mut self, path: impl Into<SharedString>) -> Self {
        self.glyph = Some(MenuGlyph::Icon(path.into()));
        self
    }

    pub(crate) fn glyph(mut self, glyph: Option<MenuGlyph>) -> Self {
        self.glyph = glyph;
        self
    }

    pub(crate) fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(crate) fn mnemonic(mut self, key: char) -> Self {
        self.mnemonic = Some(key.to_ascii_uppercase());
        self
    }

    pub(crate) fn shortcut(mut self, shortcut: Option<SharedString>) -> Self {
        self.shortcut = shortcut;
        self
    }

    pub(crate) fn toggle(mut self, on: bool) -> Self {
        self.check = Check::Toggle(on);
        self
    }

    pub(crate) fn radio(mut self, selected: bool) -> Self {
        self.check = Check::Radio(selected);
        self
    }

    pub(crate) fn trailing(mut self, width: Pixels, element: MenuElement) -> Self {
        self.trailing = Some((width, element));
        self
    }

    pub(crate) fn describe(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub(crate) fn tooltip(mut self, text: &'static str) -> Self {
        self.tooltip = Some(text);
        self
    }

    pub(crate) fn keep_open(mut self) -> Self {
        self.keep_open = true;
        self
    }

    fn enabled(&self) -> bool {
        self.action.is_some() || self.submenu.is_some()
    }

    fn checked(&self) -> bool {
        matches!(self.check, Check::Toggle(true) | Check::Radio(true))
    }
}

pub(crate) enum MenuEntry<A> {
    Item(Box<MenuItem<A>>),
    Separator,
    Heading(SharedString),
}

pub(crate) struct ActionMenu<A> {
    entries: Vec<MenuEntry<A>>,
    fill: Option<Hsla>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MenuKey {
    Activate(usize),
    Highlight(usize),
    OpenSubmenu(usize),
    Ignored,
}

pub(crate) type MenuListener<E> = Rc<dyn Fn(&E, &mut Window, &mut App)>;

pub(crate) struct MenuHandlers<A> {
    pub(crate) activate: MenuListener<A>,
    pub(crate) dismiss: MenuListener<()>,
}

impl<A> Clone for MenuHandlers<A> {
    fn clone(&self) -> Self {
        Self {
            activate: self.activate.clone(),
            dismiss: self.dismiss.clone(),
        }
    }
}

impl<A: Clone + 'static> Default for ActionMenu<A> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Clone + 'static> ActionMenu<A> {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            fill: None,
        }
    }

    pub(crate) fn fill(mut self, fill: Hsla) -> Self {
        self.fill = Some(fill);
        self
    }

    pub(crate) fn item(mut self, item: MenuItem<A>) -> Self {
        self.entries.push(MenuEntry::Item(Box::new(item)));
        self
    }

    pub(crate) fn items(mut self, items: impl IntoIterator<Item = MenuItem<A>>) -> Self {
        self.entries.extend(
            items
                .into_iter()
                .map(|item| MenuEntry::Item(Box::new(item))),
        );
        self
    }

    pub(crate) fn separator(mut self) -> Self {
        if matches!(self.entries.last(), Some(MenuEntry::Item(_))) {
            self.entries.push(MenuEntry::Separator);
        }
        self
    }

    pub(crate) fn heading(mut self, label: impl Into<SharedString>) -> Self {
        self.entries.push(MenuEntry::Heading(label.into()));
        self
    }

    pub(crate) fn entries(&self) -> &[MenuEntry<A>] {
        let end = self
            .entries
            .iter()
            .rposition(|entry| !matches!(entry, MenuEntry::Separator))
            .map_or(0, |last| last + 1);
        &self.entries[..end]
    }

    fn item_at(&self, index: usize) -> Option<&MenuItem<A>> {
        match self.entries().get(index)? {
            MenuEntry::Item(item) => Some(item),
            _ => None,
        }
    }

    fn enabled_indices(&self) -> Vec<usize> {
        self.entries()
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                MenuEntry::Item(item) if item.enabled() => Some(index),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn action_at(&self, index: usize) -> Option<A> {
        self.item_at(index)?.action.clone()
    }

    fn submenu_at(&self, index: usize) -> Option<&ActionMenu<A>> {
        self.item_at(index)?.submenu.as_ref()
    }

    fn keeps_open(&self, index: usize) -> bool {
        self.item_at(index).is_some_and(|item| item.keep_open)
    }

    fn activation_at(&self, index: usize) -> MenuKey {
        match self.item_at(index) {
            Some(item) if item.submenu.is_some() => MenuKey::OpenSubmenu(index),
            Some(item) if item.action.is_some() => MenuKey::Activate(index),
            _ => MenuKey::Ignored,
        }
    }

    pub(crate) fn handle_key(&self, key: &str, highlighted: Option<usize>) -> MenuKey {
        let enabled = self.enabled_indices();
        let (Some(&first), Some(&last)) = (enabled.first(), enabled.last()) else {
            return MenuKey::Ignored;
        };
        let position = highlighted.and_then(|index| enabled.iter().position(|&i| i == index));
        let moved = match key {
            "down" => Some(position.map_or(first, |p| enabled[(p + 1) % enabled.len()])),
            "up" => {
                Some(position.map_or(last, |p| enabled[(p + enabled.len() - 1) % enabled.len()]))
            }
            "home" => Some(first),
            "end" => Some(last),
            _ => None,
        };
        if let Some(index) = moved {
            return MenuKey::Highlight(index);
        }
        match key {
            "enter" | "space" => highlighted.map_or(MenuKey::Ignored, |i| self.activation_at(i)),
            "right" => match highlighted {
                Some(index) if self.submenu_at(index).is_some() => MenuKey::OpenSubmenu(index),
                _ => MenuKey::Ignored,
            },
            _ => self.mnemonic_target(key),
        }
    }

    fn mnemonic_target(&self, key: &str) -> MenuKey {
        let mut chars = key.chars();
        let (Some(ch), None) = (chars.next(), chars.next()) else {
            return MenuKey::Ignored;
        };
        if !ch.is_ascii_alphanumeric() {
            return MenuKey::Ignored;
        }
        let ch = ch.to_ascii_uppercase();
        self.entries()
            .iter()
            .position(|entry| {
                matches!(entry, MenuEntry::Item(item) if item.mnemonic == Some(ch) && item.enabled())
            })
            .map_or(MenuKey::Ignored, |index| self.activation_at(index))
    }

    fn content_height(&self) -> Pixels {
        self.entries()
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(_) => ROW_HEIGHT,
                MenuEntry::Heading(_) => HEADING_HEIGHT,
                MenuEntry::Separator => PANEL_BORDER + SEPARATOR_MARGIN * 2.,
            })
            .fold(px(0.), |total, height| total + height)
    }

    fn has_glyphs(&self) -> bool {
        self.entries()
            .iter()
            .any(|entry| matches!(entry, MenuEntry::Item(item) if item.glyph.is_some()))
    }

    fn has_checks(&self) -> bool {
        self.entries()
            .iter()
            .any(|entry| matches!(entry, MenuEntry::Item(item) if item.check != Check::None))
    }

    fn row_width(&self, item: &MenuItem<A>, window: &mut Window) -> Pixels {
        let mut width = measure(window, &item.label, LABEL_TEXT, FontWeight::NORMAL);
        if self.has_glyphs() {
            width += GLYPH_SIZE + ROW_GAP;
        }
        let mut parts = Vec::new();
        if let Some(detail) = &item.detail {
            parts.push(measure(window, detail, LABEL_TEXT, FontWeight::NORMAL));
        }
        if let Some(key) = item.mnemonic {
            let key = SharedString::from(key.to_string());
            parts.push(measure(window, &key, SHORTCUT_TEXT, FontWeight::MEDIUM));
        }
        if let Some(shortcut) = &item.shortcut {
            parts.push(measure(window, shortcut, SHORTCUT_TEXT, FontWeight::MEDIUM));
        }
        if let Some((width, _)) = &item.trailing {
            parts.push(*width);
        }
        if item.submenu.is_some() {
            parts.push(GLYPH_SIZE);
        }
        if self.has_checks() {
            parts.push(CHECK_SIZE);
        }
        if !parts.is_empty() {
            let gaps = ROW_GAP * (parts.len() - 1) as f32;
            width += TRAILING_GAP + gaps + parts.into_iter().fold(px(0.), |a, b| a + b);
        }
        width
    }

    pub(crate) fn size(&self, window: &mut Window) -> Size<Pixels> {
        let widest = self
            .entries()
            .iter()
            .map(|entry| match entry {
                MenuEntry::Item(item) => self.row_width(item, window),
                MenuEntry::Heading(label) => {
                    measure(window, label, HEADING_TEXT, FontWeight::MEDIUM)
                }
                MenuEntry::Separator => px(0.),
            })
            .fold(px(0.), |widest, width| widest.max(width));
        let chrome = (PANEL_PADDING + PANEL_BORDER) * 2.;
        let width = (widest + ROW_INSET * 2. + chrome).clamp(MIN_WIDTH, MAX_WIDTH);
        let height = (self.content_height() + chrome).min(MAX_HEIGHT);
        size(width.ceil(), height.ceil())
    }

    pub(crate) fn render(
        self,
        id: impl Into<ElementId>,
        handlers: MenuHandlers<A>,
        ui: UiColors,
        window: &mut Window,
        cx: &mut App,
    ) -> Stateful<Div> {
        let id: ElementId = id.into();
        let state = window.use_keyed_state((id.clone(), "state"), cx, MenuState::open);
        let snapshot = state.read(cx).snapshot();
        let menu = Rc::new(self);
        let width = menu.size(window).width;
        let submenu = snapshot
            .submenu
            .and_then(|index| Some((index, menu.submenu_at(index)?.size(window).width)))
            .map(|(index, child_width)| {
                let opens_left = snapshot.root_bounds.get().is_some_and(|bounds| {
                    bounds.right() + SUBMENU_GAP + child_width > window.viewport_size().width
                });
                let panel = render_panel(
                    &menu,
                    Some(index),
                    (id.clone(), "submenu"),
                    None,
                    &state,
                    &snapshot,
                    &handlers,
                    ui,
                    window,
                )
                .child(bounds_probe(snapshot.submenu_bounds.clone()));
                let attached =
                    attach_submenu(panel.into_any_element(), width, child_width, opens_left);
                (index, attached)
            });
        let key_menu = menu.clone();
        let key_state = state.clone();
        let key_handlers = handlers.clone();
        let out_state = state.clone();
        let out_handlers = handlers.clone();
        render_panel(
            &menu, None, id, submenu, &state, &snapshot, &handlers, ui, window,
        )
        .child(bounds_probe(snapshot.root_bounds.clone()))
        .track_focus(&snapshot.focus)
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            if handle_key(&key_menu, &key_state, &key_handlers, event, window, cx) {
                cx.stop_propagation();
            }
        })
        .on_mouse_down_out(move |event: &MouseDownEvent, window, cx| {
            let inside_submenu = out_state
                .read(cx)
                .submenu_bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&event.position));
            if inside_submenu {
                return;
            }
            restore_focus(&out_state, window, cx);
            (out_handlers.dismiss)(&(), window, cx);
        })
    }
}

#[derive(IntoElement)]
pub(crate) struct ActionMenuElement<A: Clone + 'static> {
    menu: ActionMenu<A>,
    id: ElementId,
    handlers: MenuHandlers<A>,
    ui: UiColors,
}

impl<A: Clone + 'static> ActionMenu<A> {
    pub(crate) fn element(
        self,
        id: impl Into<ElementId>,
        handlers: MenuHandlers<A>,
        ui: UiColors,
    ) -> ActionMenuElement<A> {
        ActionMenuElement {
            menu: self,
            id: id.into(),
            handlers,
            ui,
        }
    }
}

impl<A: Clone + 'static> RenderOnce for ActionMenuElement<A> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        self.menu
            .render(self.id, self.handlers, self.ui, window, cx)
    }
}

type BoundsCell = Rc<Cell<Option<Bounds<Pixels>>>>;

struct MenuState {
    focus: FocusHandle,
    restore: Option<FocusHandle>,
    highlighted: Option<usize>,
    submenu: Option<usize>,
    submenu_highlighted: Option<usize>,
    root_bounds: BoundsCell,
    submenu_bounds: BoundsCell,
}

#[derive(Clone)]
struct Snapshot {
    focus: FocusHandle,
    highlighted: Option<usize>,
    submenu: Option<usize>,
    submenu_highlighted: Option<usize>,
    root_bounds: BoundsCell,
    submenu_bounds: BoundsCell,
}

impl MenuState {
    fn open(window: &mut Window, cx: &mut gpui::Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let restore = window.focused(cx);
        let target = focus.clone();
        window.defer(cx, move |window, cx| window.focus(&target, cx));
        Self {
            focus,
            restore,
            highlighted: None,
            submenu: None,
            submenu_highlighted: None,
            root_bounds: Rc::default(),
            submenu_bounds: Rc::default(),
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            focus: self.focus.clone(),
            highlighted: self.highlighted,
            submenu: self.submenu,
            submenu_highlighted: self.submenu_highlighted,
            root_bounds: self.root_bounds.clone(),
            submenu_bounds: self.submenu_bounds.clone(),
        }
    }
}

fn restore_focus(state: &Entity<MenuState>, window: &mut Window, cx: &mut App) {
    let restore = state.read(cx).restore.clone();
    if let Some(handle) = restore {
        window.focus(&handle, cx);
    }
}

fn run_item<A: Clone + 'static>(
    menu: &ActionMenu<A>,
    index: usize,
    state: &Entity<MenuState>,
    handlers: &MenuHandlers<A>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(action) = menu.action_at(index) else {
        return;
    };
    if !menu.keeps_open(index) {
        restore_focus(state, window, cx);
    }
    (handlers.activate)(&action, window, cx);
}

fn handle_key<A: Clone + 'static>(
    menu: &ActionMenu<A>,
    state: &Entity<MenuState>,
    handlers: &MenuHandlers<A>,
    event: &KeyDownEvent,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let modifiers = event.keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
        return false;
    }
    let key = event.keystroke.key.as_str();
    let snapshot = state.read(cx).snapshot();
    if let Some(open) = snapshot.submenu
        && let Some(child) = menu.submenu_at(open)
    {
        if matches!(key, "escape" | "left") {
            state.update(cx, |state, cx| {
                state.submenu = None;
                state.submenu_highlighted = None;
                state.highlighted = Some(open);
                cx.notify();
            });
            return true;
        }
        return match child.handle_key(key, snapshot.submenu_highlighted) {
            MenuKey::Activate(index) => {
                run_item(child, index, state, handlers, window, cx);
                true
            }
            MenuKey::Highlight(index) => {
                state.update(cx, |state, cx| {
                    state.submenu_highlighted = Some(index);
                    cx.notify();
                });
                true
            }
            MenuKey::OpenSubmenu(_) | MenuKey::Ignored => key.chars().count() == 1,
        };
    }
    if key == "escape" {
        restore_focus(state, window, cx);
        (handlers.dismiss)(&(), window, cx);
        return true;
    }
    match menu.handle_key(key, snapshot.highlighted) {
        MenuKey::Activate(index) => {
            run_item(menu, index, state, handlers, window, cx);
            true
        }
        MenuKey::Highlight(index) => {
            state.update(cx, |state, cx| {
                state.highlighted = Some(index);
                cx.notify();
            });
            true
        }
        MenuKey::OpenSubmenu(index) => {
            let first =
                menu.submenu_at(index)
                    .and_then(|child| match child.handle_key("home", None) {
                        MenuKey::Highlight(first) => Some(first),
                        _ => None,
                    });
            state.update(cx, |state, cx| {
                state.highlighted = Some(index);
                state.submenu = Some(index);
                state.submenu_highlighted = first;
                cx.notify();
            });
            true
        }
        MenuKey::Ignored => key.chars().count() == 1,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one panel renderer serves the root menu and its submenu"
)]
fn render_panel<A: Clone + 'static>(
    root: &Rc<ActionMenu<A>>,
    submenu_of: Option<usize>,
    id: impl Into<ElementId>,
    attached: Option<(usize, AnyElement)>,
    state: &Entity<MenuState>,
    snapshot: &Snapshot,
    handlers: &MenuHandlers<A>,
    ui: UiColors,
    window: &mut Window,
) -> Stateful<Div> {
    let id: ElementId = id.into();
    let menu: &ActionMenu<A> = submenu_of
        .and_then(|index| root.submenu_at(index))
        .unwrap_or(root);
    let in_submenu = submenu_of.is_some();
    let highlighted = if in_submenu {
        snapshot.submenu_highlighted
    } else {
        snapshot.highlighted
    };
    let panel_size = menu.size(window);
    let has_glyphs = menu.has_glyphs();
    let has_checks = menu.has_checks();
    let (open_index, mut open_child) = match attached {
        Some((index, child)) => (Some(index), Some(child)),
        None => (None, None),
    };
    let mut list = div()
        .id((id.clone(), "list"))
        .flex()
        .flex_col()
        .p(PANEL_PADDING)
        .max_h(MAX_HEIGHT - PANEL_BORDER * 2.)
        .overflow_y_scroll();
    for (index, entry) in menu.entries().iter().enumerate() {
        list = match entry {
            MenuEntry::Separator => list.child(menu_separator(ui)),
            MenuEntry::Heading(label) => list.child(menu_heading(label.clone(), ui)),
            MenuEntry::Item(item) => {
                let open = open_index == Some(index);
                let row_id = match &item.id {
                    Some(id) => ElementId::Name(id.clone()),
                    None => ElementId::from(index),
                };
                let layout = RowLayout {
                    highlighted: highlighted == Some(index) || open,
                    reserve_glyph: has_glyphs,
                    reserve_check: has_checks,
                };
                let row = render_row(row_id, item, layout, ui);
                let row = wire_row(row, root, submenu_of, index, state, handlers);
                match open.then(|| open_child.take()).flatten() {
                    Some(child) => list.child(row.relative().child(child)),
                    None => list.child(row),
                }
            }
        };
    }
    let keyboard_highlight = highlighted.is_some();
    let clear_state = state.clone();
    panel_shell(div().id(id), ui)
        .when_some(menu.fill, |panel, fill| panel.bg(fill))
        .w(panel_size.width)
        .role(Role::Menu)
        .on_mouse_move(move |_: &MouseMoveEvent, _, cx| {
            if keyboard_highlight {
                clear_state.update(cx, |state, cx| {
                    if in_submenu {
                        state.submenu_highlighted = None;
                    } else {
                        state.highlighted = None;
                    }
                    cx.notify();
                });
            }
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(list)
}

fn wire_row<A: Clone + 'static>(
    row: Stateful<Div>,
    root: &Rc<ActionMenu<A>>,
    submenu_of: Option<usize>,
    index: usize,
    state: &Entity<MenuState>,
    handlers: &MenuHandlers<A>,
) -> Stateful<Div> {
    let menu: &ActionMenu<A> = submenu_of
        .and_then(|parent| root.submenu_at(parent))
        .unwrap_or(root);
    let has_submenu = menu.submenu_at(index).is_some();
    let row = match submenu_of {
        Some(_) => row,
        None => {
            let hover_state = state.clone();
            row.on_hover(move |hovered, _, cx| {
                if !*hovered {
                    return;
                }
                hover_state.update(cx, |state, cx| {
                    let next = has_submenu.then_some(index);
                    if state.submenu != next {
                        state.submenu = next;
                        state.submenu_highlighted = None;
                        cx.notify();
                    }
                });
            })
        }
    };
    if has_submenu {
        let click_state = state.clone();
        return row.on_click(move |_: &ClickEvent, _, cx| {
            cx.stop_propagation();
            click_state.update(cx, |state, cx| {
                state.submenu = Some(index);
                cx.notify();
            });
        });
    }
    if menu.action_at(index).is_none() {
        return row;
    }
    let root = root.clone();
    let state = state.clone();
    let handlers = handlers.clone();
    row.on_click(move |_: &ClickEvent, window, cx| {
        cx.stop_propagation();
        let menu: &ActionMenu<A> = submenu_of
            .and_then(|parent| root.submenu_at(parent))
            .unwrap_or(&root);
        run_item(menu, index, &state, &handlers, window, cx);
    })
}

pub(crate) fn panel_shell(panel: Stateful<Div>, ui: UiColors) -> Stateful<Div> {
    panel
        .relative()
        .flex()
        .flex_col()
        .rounded(PANEL_RADIUS)
        .border(PANEL_BORDER)
        .border_color(with_alpha(ui.text, EDGE_ALPHA))
        .bg(select_menu_surface(ui))
        .shadow(panel_shadow(ui))
        .font_family(FONT_FAMILY)
}

pub(crate) fn panel_shadow(ui: UiColors) -> Vec<BoxShadow> {
    let shadow = if ui.surface.l > 0.5 {
        BoxShadow::new(px(0.), px(16.), gpui::black().opacity(0.55))
            .blur_radius(px(40.))
            .spread_radius(px(-18.))
    } else {
        BoxShadow::new(px(0.), px(18.), gpui::black().opacity(0.8))
            .blur_radius(px(44.))
            .spread_radius(px(-18.))
    };
    vec![shadow]
}

pub(crate) fn menu_separator(ui: UiColors) -> Div {
    div()
        .flex_none()
        .mx(ROW_INSET)
        .my(SEPARATOR_MARGIN)
        .h(PANEL_BORDER)
        .bg(with_alpha(ui.text, SEPARATOR_ALPHA))
}

pub(crate) fn menu_heading(label: SharedString, ui: UiColors) -> Div {
    div()
        .flex_none()
        .h(HEADING_HEIGHT)
        .px(ROW_INSET)
        .flex()
        .items_center()
        .text_size(HEADING_TEXT)
        .font_weight(FontWeight::MEDIUM)
        .text_color(ui.muted)
        .child(label)
}

fn attach_submenu(
    panel: AnyElement,
    parent_width: Pixels,
    width: Pixels,
    opens_left: bool,
) -> AnyElement {
    let inset = PANEL_PADDING + PANEL_BORDER;
    let left = if opens_left {
        -(inset + SUBMENU_GAP + width)
    } else {
        parent_width - inset + SUBMENU_GAP
    };
    deferred(
        div()
            .absolute()
            .top(-inset)
            .left(left)
            .occlude()
            .child(panel),
    )
    .priority(SUBMENU_PRIORITY)
    .into_any_element()
}

fn bounds_probe(bounds: BoundsCell) -> impl IntoElement {
    canvas(
        move |painted, _, _| bounds.set(Some(painted)),
        |_, _, _, _| {},
    )
    .absolute()
    .size_full()
}

struct RowLayout {
    highlighted: bool,
    reserve_glyph: bool,
    reserve_check: bool,
}

fn render_row<A>(
    id: ElementId,
    item: &MenuItem<A>,
    layout: RowLayout,
    ui: UiColors,
) -> Stateful<Div> {
    let glyph = match item.glyph.clone() {
        Some(MenuGlyph::Icon(path)) => Some(
            svg()
                .size(GLYPH_SIZE)
                .flex_none()
                .path(path)
                .text_color(with_alpha(ui.muted, GLYPH_ALPHA))
                .into_any_element(),
        ),
        Some(MenuGlyph::Element(render)) => Some(
            div()
                .size(GLYPH_SIZE)
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(render())
                .into_any_element(),
        ),
        None if layout.reserve_glyph => Some(div().size(GLYPH_SIZE).flex_none().into_any_element()),
        None => None,
    };
    let role = match item.check {
        Check::None => Role::MenuItem,
        Check::Toggle(_) => Role::MenuItemCheckBox,
        Check::Radio(_) => Role::MenuItemRadio,
    };
    let checked = item.checked();
    let selector = item.id.clone();
    let row = div()
        .id(id)
        .when_some(selector, |row, selector| {
            row.debug_selector(move || selector.to_string())
        })
        .flex_none()
        .h(ROW_HEIGHT)
        .px(ROW_INSET)
        .flex()
        .flex_row()
        .items_center()
        .gap(ROW_GAP)
        .rounded(ROW_RADIUS)
        .when(layout.highlighted, |row| {
            row.bg(with_alpha(ui.text, HIGHLIGHT_ALPHA))
        })
        .text_size(LABEL_TEXT)
        .text_color(ui.text)
        .cursor(CursorStyle::Arrow)
        .role(role)
        .aria_label(
            item.description
                .clone()
                .unwrap_or_else(|| item.label.clone()),
        )
        .when(item.check != Check::None, |row| {
            row.aria_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            })
        })
        .when(item.submenu.is_some(), |row| {
            row.aria_expanded(layout.highlighted)
        })
        .when_some(item.tooltip, |row, text| {
            row.delayed_tooltip(crate::ui_primitives::text_tooltip(text))
        })
        .children(glyph)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_x_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(item.label.clone()),
        )
        .when_some(item.detail.clone(), |row, detail| {
            row.child(
                div()
                    .flex_none()
                    .whitespace_nowrap()
                    .text_color(ui.muted)
                    .child(detail),
            )
        })
        .when_some(item.trailing.clone(), |row, (_, render)| {
            row.child(div().flex_none().child(render()))
        })
        .when_some(item.mnemonic, |row, key| {
            row.child(trailing_hint(key.to_string(), ui))
        })
        .when_some(item.shortcut.clone(), |row, shortcut| {
            row.child(trailing_hint(shortcut, ui))
        })
        .when(item.submenu.is_some(), |row| {
            row.child(
                svg()
                    .size(GLYPH_SIZE)
                    .flex_none()
                    .path("icons/chevron-right.svg")
                    .text_color(with_alpha(ui.muted, GLYPH_ALPHA)),
            )
        })
        .when(layout.reserve_check, |row| {
            row.child(div().size(CHECK_SIZE).flex_none().when(checked, |slot| {
                slot.child(
                    svg()
                        .size(CHECK_SIZE)
                        .path("icons/menu/check.svg")
                        .text_color(ui.text),
                )
            }))
        });
    if !item.enabled() {
        row.opacity(DISABLED_OPACITY)
    } else if layout.highlighted {
        row
    } else {
        row.hover(|style| style.bg(with_alpha(ui.text, HIGHLIGHT_ALPHA)))
    }
}

pub(crate) fn trailing_hint(text: impl Into<SharedString>, ui: UiColors) -> Div {
    div()
        .flex_none()
        .whitespace_nowrap()
        .text_size(SHORTCUT_TEXT)
        .font_weight(FontWeight::MEDIUM)
        .text_color(ui.muted)
        .child(text.into())
}

fn measure(window: &mut Window, text: &SharedString, size: Pixels, weight: FontWeight) -> Pixels {
    let mut face = font(FONT_FAMILY);
    face.weight = weight;
    let runs = [TextRun {
        len: text.len(),
        font: face,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    window
        .text_system()
        .shape_line(text.clone(), size, &runs, None)
        .width
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{
        Context, FocusHandle, InteractiveElement, IntoElement, Modifiers, ParentElement, Render,
        Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
    };

    use super::{ActionMenu, MenuEntry, MenuHandlers, MenuItem, MenuKey};

    struct Harness {
        open: bool,
        picked: Rc<RefCell<Vec<&'static str>>>,
        outer: FocusHandle,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let root = div().track_focus(&self.outer).size_full().relative();
            if !self.open {
                return root;
            }
            let menu = ActionMenu::new()
                .item(MenuItem::new("Rename", "rename").id("rename").mnemonic('r'))
                .item(
                    MenuItem::submenu(
                        "Move",
                        ActionMenu::new().item(MenuItem::new("Left", "left").id("left")),
                    )
                    .id("move")
                    .mnemonic('m'),
                );
            let picked = self.picked.clone();
            let handlers = MenuHandlers {
                activate: Rc::new(cx.listener(move |this, action: &&'static str, _, cx| {
                    picked.borrow_mut().push(action);
                    this.open = false;
                    cx.notify();
                })),
                dismiss: Rc::new(cx.listener(|this, _: &(), _, cx| {
                    this.open = false;
                    cx.notify();
                })),
            };
            root.child(
                div()
                    .absolute()
                    .left(px(20.))
                    .top(px(20.))
                    .child(menu.element("menu", handlers, crate::theme::ui_colors())),
            )
        }
    }

    fn harness(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<Harness>,
        Rc<RefCell<Vec<&'static str>>>,
        &mut VisualTestContext,
    ) {
        let picked = Rc::new(RefCell::new(Vec::new()));
        let shared = picked.clone();
        let (view, cx) = cx.add_window_view(move |_, cx| Harness {
            open: false,
            picked: shared,
            outer: cx.focus_handle(),
        });
        cx.simulate_resize(size(px(800.), px(600.)));
        cx.update(|window, cx| {
            let outer = view.read(cx).outer.clone();
            window.focus(&outer, cx);
        });
        redraw(cx);
        view.update(cx, |view, cx| {
            view.open = true;
            cx.notify();
        });
        redraw(cx);
        (view, picked, cx)
    }

    fn redraw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }

    fn outer_focused(view: &gpui::Entity<Harness>, cx: &mut VisualTestContext) -> bool {
        cx.update(|window, cx| view.read(cx).outer.is_focused(window))
    }

    #[gpui::test]
    fn an_open_menu_takes_focus_runs_a_mnemonic_and_gives_focus_back(cx: &mut TestAppContext) {
        let (view, picked, cx) = harness(cx);
        assert!(!outer_focused(&view, cx), "the open menu holds focus");
        cx.simulate_keystrokes("r");
        redraw(cx);
        assert_eq!(*picked.borrow(), vec!["rename"]);
        assert!(!view.read_with(cx, |view, _| view.open));
        assert!(outer_focused(&view, cx), "focus returns to where it was");
    }

    #[gpui::test]
    fn a_submenu_item_outside_the_parent_menu_reaches_its_handler(cx: &mut TestAppContext) {
        let (view, picked, cx) = harness(cx);
        let parent = cx.debug_bounds("move").expect("submenu row is painted");
        cx.simulate_click(parent.center(), Modifiers::default());
        redraw(cx);
        let target = cx.debug_bounds("left").expect("submenu item is painted");
        assert!(
            target.left() > parent.right(),
            "the submenu sits beside its parent"
        );
        cx.simulate_click(target.center(), Modifiers::default());
        redraw(cx);
        assert_eq!(*picked.borrow(), vec!["left"]);
        assert!(!view.read_with(cx, |view, _| view.open));
    }

    #[gpui::test]
    fn the_keyboard_walks_into_a_submenu_and_back(cx: &mut TestAppContext) {
        let (view, picked, cx) = harness(cx);
        cx.simulate_keystrokes("m");
        redraw(cx);
        assert!(cx.debug_bounds("left").is_some(), "m opens the submenu");
        cx.simulate_keystrokes("left");
        redraw(cx);
        assert!(cx.debug_bounds("left").is_none(), "left closes it");
        cx.simulate_keystrokes("m enter");
        redraw(cx);
        assert_eq!(*picked.borrow(), vec!["left"]);
        assert!(!view.read_with(cx, |view, _| view.open));
    }

    #[gpui::test]
    fn escape_and_a_press_outside_dismiss_without_running_anything(cx: &mut TestAppContext) {
        let (view, picked, cx) = harness(cx);
        cx.simulate_keystrokes("escape");
        redraw(cx);
        assert!(!view.read_with(cx, |view, _| view.open));
        assert!(outer_focused(&view, cx));

        view.update(cx, |view, cx| {
            view.open = true;
            cx.notify();
        });
        redraw(cx);
        cx.simulate_click(point(px(700.), px(500.)), Modifiers::default());
        redraw(cx);
        assert!(picked.borrow().is_empty());
        assert!(!view.read_with(cx, |view, _| view.open));
    }

    fn sample() -> ActionMenu<&'static str> {
        ActionMenu::new()
            .item(MenuItem::new("Rename", "rename").mnemonic('r'))
            .separator()
            .item(MenuItem::disabled("Unavailable").mnemonic('u'))
            .item(MenuItem::new("Copy path", "copy").mnemonic('p'))
            .separator()
            .separator()
            .item(
                MenuItem::submenu(
                    "Move",
                    ActionMenu::new().item(MenuItem::new("Left", "left")),
                )
                .mnemonic('m'),
            )
            .item(MenuItem::new("Close", "close"))
            .separator()
    }

    #[test]
    fn separators_never_double_up_or_trail() {
        let menu = sample();
        assert_eq!(menu.entries().len(), 7);
        assert!(matches!(menu.entries().last(), Some(MenuEntry::Item(_))));
        let empty: ActionMenu<&str> = ActionMenu::new().separator();
        assert!(empty.entries().is_empty());
    }

    #[test]
    fn mnemonics_are_case_insensitive_and_skip_disabled_items() {
        let menu = sample();
        assert_eq!(menu.handle_key("p", None), MenuKey::Activate(3));
        assert_eq!(menu.handle_key("R", None), MenuKey::Activate(0));
        assert_eq!(menu.handle_key("u", None), MenuKey::Ignored);
        assert_eq!(menu.handle_key("q", None), MenuKey::Ignored);
        assert_eq!(menu.action_at(3), Some("copy"));
    }

    #[test]
    fn a_submenu_opens_from_its_mnemonic_enter_or_right() {
        let menu = sample();
        assert_eq!(menu.handle_key("m", None), MenuKey::OpenSubmenu(5));
        assert_eq!(menu.handle_key("enter", Some(5)), MenuKey::OpenSubmenu(5));
        assert_eq!(menu.handle_key("right", Some(5)), MenuKey::OpenSubmenu(5));
        assert_eq!(menu.handle_key("right", Some(3)), MenuKey::Ignored);
    }

    #[test]
    fn arrows_walk_enabled_items_and_wrap() {
        let menu = sample();
        assert_eq!(menu.handle_key("down", None), MenuKey::Highlight(0));
        assert_eq!(menu.handle_key("down", Some(0)), MenuKey::Highlight(3));
        assert_eq!(menu.handle_key("down", Some(3)), MenuKey::Highlight(5));
        assert_eq!(menu.handle_key("down", Some(6)), MenuKey::Highlight(0));
        assert_eq!(menu.handle_key("up", None), MenuKey::Highlight(6));
        assert_eq!(menu.handle_key("up", Some(0)), MenuKey::Highlight(6));
        assert_eq!(menu.handle_key("home", Some(6)), MenuKey::Highlight(0));
    }

    #[test]
    fn enter_activates_only_a_highlighted_enabled_item() {
        let menu = sample();
        assert_eq!(menu.handle_key("enter", Some(3)), MenuKey::Activate(3));
        assert_eq!(menu.handle_key("enter", Some(2)), MenuKey::Ignored);
        assert_eq!(menu.handle_key("enter", None), MenuKey::Ignored);
    }
}
