use super::*;
use crate::app::diff_dock::preview::archive::ArchiveView;
use crate::app::diff_dock::preview::image::ImageView;
use crate::app::diff_dock::preview::large_text::{LargeTextView, PreviewEvent};
use crate::app::diff_dock::preview::no_preview::{
    ClickHandler, NoPreviewActions, render_no_preview,
};
use crate::app::diff_dock::preview::table::{SHOW_SOURCE, SHOW_TABLE, TableView};
use crate::app::diff_dock::preview::{
    FileOpenOutcome, FilePreview, NoPreview, OpenedFile, PreviewKind, open_file_blocking,
};
use crate::app::files_sidebar::open_with;
use crate::markdown::{MAX_INPUT_BYTES, MarkdownView};

pub(crate) const SHOW_RENDERED_MARKDOWN: &str = "Show rendered markdown";
pub(crate) const SHOW_MARKDOWN_SOURCE: &str = "Show markdown source";
pub(crate) const MARKDOWN_TOO_LARGE: &str = "File too large to render";
pub(crate) const TABLE_TOO_LARGE: &str = "File too large to show as a table";

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum CodeViewEvent {
    Toast(SharedString),
    OpenWithMenu {
        path: PathBuf,
        position: Point<Pixels>,
    },
    DisplayChanged(super::super::controls::EditorDisplay),
    SendComment {
        comment: crate::app::agent_send::RangeComment,
        position: Point<Pixels>,
        window: gpui::AnyWindowHandle,
    },
}

impl gpui::EventEmitter<CodeViewEvent> for CodeView {}

pub(super) struct RenderedViews {
    pub(super) markdown: Option<(gpui::Entity<MarkdownView>, u64)>,
    pub(super) table: Option<gpui::Entity<TableView>>,
}

impl RenderedViews {
    pub(super) fn none() -> Self {
        Self {
            markdown: None,
            table: None,
        }
    }
}

pub(super) fn rendered_by_default(kind: PreviewKind) -> bool {
    let display = super::super::controls::editor_display();
    match kind {
        PreviewKind::Markdown => display.markdown_rendered,
        PreviewKind::Table(_) => display.csv_table,
        _ => false,
    }
}

impl CodeView {
    #[cfg(test)]
    pub(crate) fn preview(&self) -> Option<&FilePreview> {
        match &self.state {
            CodeLoadState::Preview(preview) => Some(preview),
            _ => None,
        }
    }

    pub(crate) fn preview_active(&self) -> bool {
        matches!(self.state, CodeLoadState::Preview(_))
    }

    pub(crate) fn shows_text_position(&self) -> bool {
        self.state.document().is_some() && !self.rendered
    }

    #[cfg(test)]
    pub(crate) fn shows_rendered(&self) -> bool {
        self.rendered
    }

    pub(super) fn spawn_open(&mut self, reuse: bool, cx: &mut Context<Self>) {
        let generation = self.slot.begin();
        let syntax = DiffSyntax::from_theme(&crate::theme::active_theme());
        self.theme_generation = crate::theme::theme_generation();
        let path = self.path.clone();
        spawn_blocking_then(
            cx,
            move || {
                let stamp = FileStamp::read(&path);
                (open_file_blocking(&path, syntax), stamp)
            },
            move |view: &mut Self, (outcome, stamp): (FileOpenOutcome, Option<FileStamp>), cx| {
                if !view.slot.accept(generation) {
                    return;
                }
                view.apply_opened(outcome, stamp, reuse, cx);
                if !reuse {
                    view.start_watcher(cx);
                }
                cx.notify();
            },
        );
    }

    fn apply_opened(
        &mut self,
        outcome: FileOpenOutcome,
        stamp: Option<FileStamp>,
        reuse: bool,
        cx: &mut Context<Self>,
    ) {
        match outcome {
            Ok(OpenedFile::Code(loaded)) => {
                if reuse {
                    self.reset_for_reopen();
                }
                self.indent = loaded.indent;
                self.buffer_stamp = loaded.stamp;
                self.disk_stamp = loaded.stamp;
                self.state = CodeLoadState::Ready(loaded);
                self.start_initial_parse(cx);
                self.start_lexical(cx);
                self.start_base_load(cx);
            }
            Ok(opened) => {
                self.disk_stamp = stamp;
                self.buffer_stamp = stamp;
                self.install_preview(opened, reuse, cx);
            }
            Err(err) => self.state = CodeLoadState::Failed(err),
        }
    }

    fn reset_for_reopen(&mut self) {
        self.selection = CodeSelection::default();
        self.history.clear();
        self.saved_mark = edit::HistoryMark::default();
        self.scroll
            .install_map(super::super::rows::RowMap::uniform(0), None);
    }

    fn install_preview(&mut self, opened: OpenedFile, reuse: bool, cx: &mut Context<Self>) {
        let current = match &self.state {
            CodeLoadState::Preview(preview) if reuse => Some(preview),
            _ => None,
        };
        let preview = match (opened, current) {
            (OpenedFile::LargeText { .. }, Some(FilePreview::LargeText(view))) => {
                view.update(cx, |view, cx| view.reindex(cx));
                FilePreview::LargeText(view.clone())
            }
            (OpenedFile::Image(probe), Some(FilePreview::Image(view))) => {
                view.update(cx, |view, cx| view.reload(probe, cx));
                FilePreview::Image(view.clone())
            }
            (OpenedFile::Archive { size, .. }, Some(FilePreview::Archive(view))) => {
                view.update(cx, |view, cx| view.reload(size, cx));
                FilePreview::Archive(view.clone())
            }
            (OpenedFile::LargeText { size }, _) => {
                let path = self.path.clone();
                let focus = self.focus.clone();
                let view = cx.new(|cx| LargeTextView::new(path, size, focus, cx));
                self.subscribe_preview(&view, cx);
                FilePreview::LargeText(view)
            }
            (OpenedFile::Image(probe), _) => {
                let path = self.path.clone();
                let focus = self.focus.clone();
                let view = cx.new(|cx| ImageView::new(path, probe, focus, cx));
                self.subscribe_preview(&view, cx);
                FilePreview::Image(view)
            }
            (OpenedFile::Archive { format, size }, _) => {
                let path = self.path.clone();
                let focus = self.focus.clone();
                let view = cx.new(|cx| ArchiveView::new(path, format, size, focus, cx));
                self.subscribe_preview(&view, cx);
                FilePreview::Archive(view)
            }
            (OpenedFile::NoPreview(screen), _) => FilePreview::NoPreview(screen),
            (OpenedFile::Code(_), _) => return,
        };
        self.state = CodeLoadState::Preview(preview);
    }

    fn subscribe_preview<V: gpui::EventEmitter<PreviewEvent>>(
        &mut self,
        view: &gpui::Entity<V>,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe(
            view,
            |this: &mut Self, _, event: &PreviewEvent, cx| match event {
                PreviewEvent::Refused(screen) => {
                    this.state = CodeLoadState::Preview(FilePreview::NoPreview(screen.clone()));
                    cx.notify();
                }
                PreviewEvent::Toast(message) => cx.emit(CodeViewEvent::Toast(message.clone())),
            },
        )
        .detach();
    }

    pub(super) fn preview_disk_changed(
        &mut self,
        stamp: Option<FileStamp>,
        cx: &mut Context<Self>,
    ) {
        if stamp == self.disk_stamp {
            return;
        }
        self.disk_stamp = stamp;
        if stamp.is_none() {
            self.state = CodeLoadState::Failed(CodeLoadError::NotFound);
            cx.notify();
            return;
        }
        self.spawn_open(true, cx);
    }

    pub(crate) fn mode_toggle_label(&self) -> Option<&'static str> {
        let available = self.state.document().is_some()
            || matches!(
                self.state,
                CodeLoadState::Preview(FilePreview::LargeText(_))
            );
        if !available {
            return None;
        }
        Some(match (self.kind, self.shows_rendered_view()) {
            (PreviewKind::Markdown, false) => SHOW_RENDERED_MARKDOWN,
            (PreviewKind::Markdown, true) => SHOW_MARKDOWN_SOURCE,
            (PreviewKind::Table(_), false) => SHOW_TABLE,
            (PreviewKind::Table(_), true) => SHOW_SOURCE,
            _ => return None,
        })
    }

    fn shows_rendered_view(&self) -> bool {
        self.rendered && self.renderable()
    }

    fn renderable(&self) -> bool {
        self.state
            .document()
            .is_some_and(|doc| doc.len_bytes() <= MAX_INPUT_BYTES)
    }

    pub(crate) fn toggle_rendered(&mut self, cx: &mut Context<Self>) {
        let want = !self.shows_rendered_view();
        if want && !self.renderable() {
            let message = if matches!(self.kind, PreviewKind::Markdown) {
                MARKDOWN_TOO_LARGE
            } else {
                TABLE_TOO_LARGE
            };
            cx.emit(CodeViewEvent::Toast(message.into()));
            return;
        }
        self.rendered = want;
        let mut display = super::super::controls::editor_display();
        match self.kind {
            PreviewKind::Markdown => display.markdown_rendered = want,
            PreviewKind::Table(_) => display.csv_table = want,
            _ => return,
        }
        super::super::controls::set_editor_display(display);
        cx.emit(CodeViewEvent::DisplayChanged(display));
        cx.notify();
    }

    pub(crate) fn render_mode_toggle(
        &mut self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let label = self.mode_toggle_label()?;
        let icon = if self.shows_rendered_view() {
            "icons/code.svg"
        } else {
            "icons/eye.svg"
        };
        let hover = crate::app::constants::sidebar_tab_hover_background();
        Some(
            div()
                .id("code-view-mode-toggle")
                .role(gpui::Role::Button)
                .aria_label(label)
                .flex_none()
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .cursor(CursorStyle::PointingHand)
                .hover(move |style| style.bg(hover))
                .tooltip(crate::ui_primitives::text_tooltip(label))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    this.toggle_rendered(cx);
                    cx.stop_propagation();
                }))
                .child(gpui::svg().size(px(16.)).path(icon).text_color(ui.muted))
                .into_any_element(),
        )
    }

    pub(super) fn render_rendered(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.rendered {
            return None;
        }
        let doc = self.state.document()?;
        let revision = doc.revision();
        match self.kind {
            PreviewKind::Markdown => {
                let stale = self
                    .rendered_views
                    .markdown
                    .as_ref()
                    .is_none_or(|(_, seen)| *seen != revision);
                if stale {
                    let text = doc.slice_string(0..doc.len_bytes());
                    let view = match self.rendered_views.markdown.take() {
                        Some((view, _)) => view,
                        None => {
                            let path = self.path.clone();
                            cx.new(|cx| MarkdownView::embedded(path, cx))
                        }
                    };
                    view.update(cx, |view, cx| view.set_source(text, cx));
                    self.rendered_views.markdown = Some((view, revision));
                }
                let view = self.rendered_views.markdown.as_ref()?.0.clone();
                Some(
                    div()
                        .id("code-view-rendered-markdown")
                        .track_focus(&self.focus)
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(view)
                        .into_any_element(),
                )
            }
            PreviewKind::Table(delimiter) => {
                let focus = self.focus.clone();
                let view = self
                    .rendered_views
                    .table
                    .get_or_insert_with(|| cx.new(|_| TableView::new(focus)))
                    .clone();
                if view.read(cx).needs(revision) {
                    let text = doc.slice_string(0..doc.len_bytes());
                    view.update(cx, |view, cx| view.load(text, delimiter, revision, cx));
                }
                Some(
                    div()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .child(view)
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    fn no_preview_actions(&self, cx: &mut Context<Self>) -> NoPreviewActions {
        let open_path = self.path.clone();
        let reveal_path = self.path.clone();
        let menu_path = self.path.clone();
        let launch = |task: gpui::Task<Result<(), String>>, cx: &mut Context<Self>| {
            cx.spawn(async move |this, cx| {
                if let Err(message) = task.await {
                    log::warn!("{message}");
                    let _ = this.update(cx, |_: &mut Self, cx| {
                        cx.emit(CodeViewEvent::Toast(message.into()));
                    });
                }
            })
            .detach();
        };
        let open_default = cx.listener(move |_, _: &ClickEvent, _, cx| {
            let task = cx
                .background_executor()
                .spawn(open_with::open_with_default_app(open_path.clone()));
            launch(task, cx);
        });
        let reveal = cx.listener(move |_, _: &ClickEvent, _, cx| {
            let task = cx
                .background_executor()
                .spawn(open_with::reveal_in_file_manager(reveal_path.clone()));
            launch(task, cx);
        });
        let has_editors = !crate::app::sidebar::context_menu::installed_editors().is_empty();
        let open_with_menu = cx.listener(move |_, event: &ClickEvent, _, cx| {
            cx.emit(CodeViewEvent::OpenWithMenu {
                path: menu_path.clone(),
                position: event.position(),
            });
            cx.stop_propagation();
        });
        NoPreviewActions {
            open_default: Box::new(open_default),
            reveal: Box::new(reveal),
            open_with: has_editors.then(|| Box::new(open_with_menu) as ClickHandler),
        }
    }

    pub(super) fn render_preview(
        &mut self,
        ui: crate::theme::UiColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let CodeLoadState::Preview(preview) = &self.state else {
            return None;
        };
        let (body, owns_focus) = match preview {
            FilePreview::NoPreview(screen) => {
                let screen: NoPreview = screen.clone();
                let actions = self.no_preview_actions(cx);
                (render_no_preview(&self.path, &screen, actions, ui), false)
            }
            FilePreview::LargeText(view) => (view.clone().into_any_element(), true),
            FilePreview::Image(view) => (view.clone().into_any_element(), true),
            FilePreview::Archive(view) => (view.clone().into_any_element(), true),
        };
        let frame = div().id("code-view-preview");
        let frame = if owns_focus {
            frame
        } else {
            frame.track_focus(&self.focus)
        };
        Some(
            frame
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .child(body)
                .into_any_element(),
        )
    }
}
