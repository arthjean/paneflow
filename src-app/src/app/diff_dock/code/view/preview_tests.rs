use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, TestAppContext, VisualTestContext};

use super::preview::{SHOW_MARKDOWN_SOURCE, SHOW_RENDERED_MARKDOWN};
use super::*;
use crate::app::diff_dock::preview::FilePreview;
use crate::app::diff_dock::preview::large_text::{
    COPY_REFUSED, INDEXING_LABEL, LARGE_TEXT_MAX_BYTES, LargeTextView, LineIndex, PAGES_KEPT,
    index_file,
};
use crate::app::diff_dock::preview::table::{SHOW_SOURCE, SHOW_TABLE};

struct Opened<'a> {
    _dir: tempfile::TempDir,
    path: PathBuf,
    view: Entity<CodeView>,
    cx: &'a mut VisualTestContext,
    events: Rc<RefCell<Vec<CodeViewEvent>>>,
}

fn open_file<'a>(cx: &'a mut TestAppContext, name: &str, bytes: &[u8]) -> Opened<'a> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join(name);
    std::fs::write(&path, bytes).expect("write fixture");
    open_path(cx, dir, path)
}

fn open_path<'a>(cx: &'a mut TestAppContext, dir: tempfile::TempDir, path: PathBuf) -> Opened<'a> {
    super::disk_sync::WATCHER_OFF.with(|off| off.set(true));
    let spawn_path = path.clone();
    let (view, cx) = cx.add_window_view(move |_window, cx| CodeView::new(spawn_path, cx));
    cx.simulate_resize(size(px(800.), px(500.)));
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, event: &CodeViewEvent, _| {
            sink.borrow_mut().push(event.clone());
        })
        .detach();
    });
    cx.run_until_parked();
    frame(&view, cx);
    Opened {
        _dir: dir,
        path,
        view,
        cx,
        events,
    }
}

fn frame(view: &Entity<CodeView>, cx: &mut VisualTestContext) {
    view.update(cx, |_, cx| cx.notify());
    cx.run_until_parked();
}

fn settle(opened: &mut Opened<'_>) {
    for _ in 0..4 {
        frame(&opened.view, opened.cx);
    }
}

fn stop_watching(opened: &mut Opened<'_>) {
    opened.view.update(opened.cx, |view, _| {
        if let Some(bridge) = view._watch_bridge.take() {
            *bridge.lock().expect("bridge lock") = None;
        }
        view._watcher = None;
    });
}

fn touch(opened: &mut Opened<'_>, bytes: &[u8]) {
    std::fs::write(&opened.path, bytes).expect("rewrite");
    let stamp = FileStamp::read(&opened.path);
    opened
        .view
        .update(opened.cx, |view, cx| view.preview_disk_changed(stamp, cx));
    settle(opened);
}

fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    ::image::RgbaImage::from_pixel(width, height, ::image::Rgba([10, 20, 30, 255]))
        .write_to(&mut bytes, ::image::ImageFormat::Png)
        .expect("encode png");
    bytes.into_inner()
}

#[gpui::test]
fn a_binary_file_opens_on_the_no_preview_screen_with_its_three_actions(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "tool.exe", b"MZ\0\0\x90\0garbage");
    opened.view.read_with(opened.cx, |view, _| {
        let Some(FilePreview::NoPreview(screen)) = view.preview() else {
            panic!("a binary lands on the no-preview screen");
        };
        assert_eq!(screen.size, Some(13));
        assert!(screen.reason.as_deref().unwrap_or("").contains("binary"));
        assert!(!view.shows_text_position());
        assert!(view.mode_toggle_label().is_none());
    });
    for selector in [
        "no-preview-open-default",
        "no-preview-reveal",
        "no-preview-open-with",
    ] {
        assert!(
            opened.cx.debug_bounds(selector).is_some(),
            "{selector} is on screen"
        );
    }
    stop_watching(&mut opened);
}

#[cfg(unix)]
#[gpui::test]
fn a_named_pipe_opens_on_the_no_preview_screen_without_blocking(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("events.fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(&path)
        .status()
        .is_ok_and(|status| status.success());
    assert!(made, "mkfifo {}", path.display());
    let mut opened = open_path(cx, dir, path);
    opened.view.read_with(opened.cx, |view, _| {
        let Some(FilePreview::NoPreview(screen)) = view.preview() else {
            panic!("a pipe lands on the no-preview screen");
        };
        assert!(
            screen
                .reason
                .as_deref()
                .unwrap_or("")
                .contains("special file")
        );
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_file_deleted_before_it_opens_offers_a_retry_that_reopens_it(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("gone.rs");
    let mut opened = open_path(cx, dir, path.clone());
    opened.view.read_with(opened.cx, |view, _| {
        let message = view.state.error_message().expect("failed");
        assert!(message.starts_with("File no longer exists"), "{message}");
        assert!(view.state.is_retriable());
    });
    std::fs::write(&path, "fn main() {}\n").expect("recreate");
    opened.view.update(opened.cx, |view, cx| {
        let path = view.path.clone();
        view.open(path, cx);
    });
    settle(&mut opened);
    opened.view.read_with(opened.cx, |view, _| {
        assert!(view.document().is_some(), "Retry opened the file");
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn an_image_opens_in_the_viewer_and_keeps_its_zoom_across_a_disk_change(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "logo.png", &png_bytes(40, 20));
    let image = opened
        .view
        .read_with(opened.cx, |view, _| match view.preview() {
            Some(FilePreview::Image(image)) => image.clone(),
            _ => panic!("a png opens in the image viewer"),
        });
    settle(&mut opened);
    image.read_with(opened.cx, |image, _| {
        assert!(!image.is_loading());
        assert_eq!(image.dimensions(), Some((40, 20)));
        assert_eq!(image.zoom_mode(), None, "the viewer starts on Fit");
        assert_eq!(image.scale(), 1.0, "Fit never passes 100%");
    });
    for selector in [
        "image-zoom-out",
        "image-zoom-in",
        "image-zoom-fit",
        "image-zoom-actual",
    ] {
        assert!(opened.cx.debug_bounds(selector).is_some(), "{selector}");
    }
    image.update(opened.cx, |image, cx| image.zoom_by_step(true, cx));
    image.read_with(opened.cx, |image, _| {
        assert_eq!(image.zoom_mode(), Some(1.25))
    });

    touch(&mut opened, &png_bytes(80, 40));
    image.read_with(opened.cx, |image, _| {
        assert_eq!(image.dimensions(), Some((80, 40)), "the new pixels loaded");
        assert_eq!(
            image.zoom_mode(),
            Some(1.25),
            "the zoom survived the reload"
        );
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_large_picture_pans_when_dragged_past_the_area(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "wide.png", &png_bytes(3000, 2000));
    settle(&mut opened);
    let image = opened
        .view
        .read_with(opened.cx, |view, _| match view.preview() {
            Some(FilePreview::Image(image)) => image.clone(),
            _ => panic!("image viewer"),
        });
    image.update(opened.cx, |image, cx| {
        image.set_zoom(Some(1.0), cx);
        image.set_viewport_for_test(size(px(400.), px(300.)));
        let before = image.scroll_offset();
        image.pan_by(120.0, 80.0, cx);
        let after = image.scroll_offset();
        assert!(
            after.x > before.x && after.y > before.y,
            "{before:?} -> {after:?}"
        );
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_corrupt_image_says_it_could_not_be_loaded(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "broken.png", b"\x89PNG\r\n\x1a\nnope");
    settle(&mut opened);
    opened
        .view
        .read_with(opened.cx, |view, cx| match view.preview() {
            Some(FilePreview::Image(image)) => assert!(image.read(cx).has_failed()),
            _ => panic!("the image viewer reports the failure"),
        });
    stop_watching(&mut opened);
}

#[gpui::test]
fn an_oversized_image_lands_on_the_no_preview_screen_before_any_decode(cx: &mut TestAppContext) {
    let header = crate::app::diff_dock::preview::image::tests::png_header(20_000, 20_000);
    let mut opened = open_file(cx, "huge.png", &header);
    opened.view.read_with(opened.cx, |view, _| {
        let Some(FilePreview::NoPreview(screen)) = view.preview() else {
            panic!("refused from the header");
        };
        assert!(screen.reason.as_deref().unwrap_or("").contains("16,384"));
    });
    stop_watching(&mut opened);
}

fn large_text(lines: usize, width: usize) -> Vec<u8> {
    let mut text = Vec::with_capacity(lines * (width + 1));
    for n in 0..lines {
        let line = format!("{n:0width$}");
        text.extend_from_slice(line.as_bytes());
        text.push(b'\n');
    }
    text
}

fn large_view(opened: &Opened<'_>) -> Entity<LargeTextView> {
    opened
        .view
        .read_with(opened.cx, |view, _| match view.preview() {
            Some(FilePreview::LargeText(large)) => large.clone(),
            _ => panic!("a large text file opens in the large viewer"),
        })
}

#[gpui::test]
fn a_large_log_opens_read_only_indexes_and_scrolls_to_its_last_line(cx: &mut TestAppContext) {
    let lines = 220_000;
    let bytes = large_text(lines, 63);
    assert!(bytes.len() > super::super::load::MAX_FILE_BYTES);
    let mut opened = open_file(cx, "run.log", &bytes);
    let large = large_view(&opened);
    settle(&mut opened);
    large.read_with(opened.cx, |large, _| {
        assert!(large.is_indexed());
        assert_eq!(large.line_count(), lines);
        assert_eq!(
            large.status_label(),
            format!(
                "220,000 lines · {}",
                crate::app::diff_dock::preview::format_size(bytes.len() as u64)
            )
        );
        assert!(large.retained_index_bytes() <= 8 * (lines + 1));
    });
    opened.view.read_with(opened.cx, |view, _| {
        assert!(
            !view.shows_text_position(),
            "no caret position for the read-only viewer"
        );
    });
    large.update(opened.cx, |large, cx| large.scroll_to_line(f64::MAX, cx));
    settle(&mut opened);
    large.read_with(opened.cx, |large, _| {
        let top = large.top_line();
        assert!(top > (lines - 40) as f64, "scrolled to the end, top {top}");
        assert!(large.cached_pages() <= PAGES_KEPT);
    });
    stop_watching(&mut opened);
}

#[test]
fn the_first_screen_is_ready_after_sixteen_kilobytes_while_the_index_runs() {
    let index = std::sync::Mutex::new(LineIndex::default());
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("run.log");
    std::fs::write(&path, large_text(200_000, 63)).expect("write");
    let mut first_page = None;
    index_file(&path, &index, &cancel, || {
        if first_page.is_none() {
            let index = index.lock().expect("lock");
            first_page = Some((index.line_count(), index.is_complete()));
        }
    })
    .expect("index");
    let (lines, complete) = first_page.expect("an early progress report");
    assert!(
        lines >= 60,
        "a full screen of lines is ready after 16 KB ({lines})"
    );
    assert!(!complete, "and the index is still running");
    assert_eq!(INDEXING_LABEL, "Indexing…");
}

#[gpui::test]
fn copying_a_selection_reads_it_from_disk_and_refuses_past_sixteen_megabytes(
    cx: &mut TestAppContext,
) {
    let bytes = large_text(300_000, 63);
    let mut opened = open_file(cx, "big.log", &bytes);
    let large = large_view(&opened);
    settle(&mut opened);
    large.update(opened.cx, |large, cx| {
        large.select_lines(2, 4);
        large.copy_selection(cx);
    });
    settle(&mut opened);
    let copied = opened
        .cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .expect("clipboard text");
    assert_eq!(copied, format!("{:063}\n{:063}\n{:063}\n", 2, 3, 4));

    large.update(opened.cx, |large, cx| {
        large.select_lines(0, 299_999);
        large.copy_selection(cx);
    });
    settle(&mut opened);
    assert!(
        opened
            .events
            .borrow()
            .contains(&CodeViewEvent::Toast(COPY_REFUSED.into())),
        "a 19 MB copy is refused with a toast"
    );
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_rewritten_log_is_reindexed_and_keeps_its_position(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "grow.log", &large_text(200_000, 63));
    let large = large_view(&opened);
    settle(&mut opened);
    large.update(opened.cx, |large, cx| large.scroll_to_line(150_000.0, cx));
    touch(&mut opened, &large_text(260_000, 63));
    let same = large_view(&opened);
    assert_eq!(same.entity_id(), large.entity_id(), "the viewer is kept");
    large.read_with(opened.cx, |large, _| {
        assert!(large.is_indexed());
        assert_eq!(large.line_count(), 260_000);
        assert_eq!(large.top_line(), 150_000.0);
    });
    touch(&mut opened, &large_text(170_000, 63));
    large.read_with(opened.cx, |large, _| {
        assert_eq!(large.line_count(), 170_000);
        assert!(
            large.top_line() < 170_000.0,
            "a vanished line clamps the position"
        );
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn closing_the_viewer_stops_the_index(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "close.log", &large_text(200_000, 63));
    let token = large_view(&opened).read_with(opened.cx, |large, _| large.cancel_token());
    let other = opened.path.with_file_name("other.rs");
    std::fs::write(&other, "fn main() {}\n").expect("write");
    opened
        .view
        .update(opened.cx, |view, cx| view.open(other, cx));
    settle(&mut opened);
    assert!(
        token.load(std::sync::atomic::Ordering::Relaxed),
        "releasing the viewer cancels its index"
    );
}

#[gpui::test]
fn a_log_with_a_null_byte_past_the_sniff_window_ends_on_the_no_preview_screen(
    cx: &mut TestAppContext,
) {
    let mut bytes = large_text(200_000, 63);
    let at = bytes.len() - 100;
    bytes[at] = 0;
    let mut opened = open_file(cx, "corrupt.log", &bytes);
    settle(&mut opened);
    opened.view.read_with(opened.cx, |view, _| {
        let Some(FilePreview::NoPreview(screen)) = view.preview() else {
            panic!("the index refuses the null byte");
        };
        assert!(screen.reason.as_deref().unwrap_or("").contains("null byte"));
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_text_file_past_the_viewer_limit_lands_on_the_no_preview_screen(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("huge.log");
    let file = std::fs::File::create(&path).expect("create");
    file.set_len(LARGE_TEXT_MAX_BYTES + 1).expect("sparse file");
    let mut opened = open_path(cx, dir, path);
    opened.view.read_with(opened.cx, |view, _| {
        let Some(FilePreview::NoPreview(screen)) = view.preview() else {
            panic!("past 256 MB nothing is indexed");
        };
        assert!(screen.reason.as_deref().unwrap_or("").contains("256 MB"));
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn markdown_opens_as_source_and_renders_the_unsaved_text_on_toggle(cx: &mut TestAppContext) {
    let saved = super::super::controls::editor_display();
    let mut opened = open_file(cx, "PLAN.md", b"# Plan\n\nFirst step.\n");
    opened.view.read_with(opened.cx, |view, _| {
        assert!(view.document().is_some());
        assert!(!view.shows_rendered(), "source is the default");
        assert_eq!(view.mode_toggle_label(), Some(SHOW_RENDERED_MARKDOWN));
    });
    opened.view.update(opened.cx, |view, cx| {
        let end = view.document().expect("ready").len_bytes();
        view.splice_all(
            &[(end..end, "Unsaved line.\n".to_string())],
            CodeSelection::at(end),
            EditGroup::Atomic,
            cx,
        );
        assert!(view.is_dirty());
        view.toggle_rendered(cx);
    });
    settle(&mut opened);
    let markdown = opened.view.read_with(opened.cx, |view, _| {
        assert!(view.shows_rendered());
        assert_eq!(view.mode_toggle_label(), Some(SHOW_MARKDOWN_SOURCE));
        view.rendered_views
            .markdown
            .as_ref()
            .expect("rendered")
            .0
            .clone()
    });
    let text = markdown
        .read_with(opened.cx, |markdown, _| markdown.rendered_text())
        .expect("parsed");
    assert!(text.contains("Unsaved line."), "{text}");
    assert!(super::super::controls::editor_display().markdown_rendered);
    assert!(
        opened
            .events
            .borrow()
            .iter()
            .any(|event| matches!(event, CodeViewEvent::DisplayChanged(display) if display.markdown_rendered)),
        "the choice is written to the configuration"
    );

    opened.view.update(opened.cx, |view, cx| {
        view.reveal(FileReveal::line(2), cx);
        assert!(!view.shows_rendered(), "a requested line shows the source");
    });
    assert!(
        super::super::controls::editor_display().markdown_rendered,
        "without changing the preference"
    );
    super::super::controls::set_editor_display(saved);
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_markdown_past_the_render_limit_keeps_its_source_and_says_so(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "huge.md", &large_text(200_000, 63));
    opened.view.update(opened.cx, |view, cx| {
        assert!(matches!(view.preview(), Some(FilePreview::LargeText(_))));
        assert_eq!(view.mode_toggle_label(), Some(SHOW_RENDERED_MARKDOWN));
        view.toggle_rendered(cx);
        assert!(!view.shows_rendered());
    });
    assert!(
        opened
            .events
            .borrow()
            .contains(&CodeViewEvent::Toast("File too large to render".into()))
    );
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_csv_opens_as_a_table_and_toggles_to_its_source(cx: &mut TestAppContext) {
    let saved = super::super::controls::editor_display();
    let mut opened = open_file(cx, "data.csv", b"name;age\nada;36\nlin;41\n");
    settle(&mut opened);
    let table = opened.view.read_with(opened.cx, |view, _| {
        assert!(view.shows_rendered(), "the table is the default");
        assert_eq!(view.mode_toggle_label(), Some(SHOW_SOURCE));
        view.rendered_views.table.clone().expect("table view")
    });
    let parsed = table
        .read_with(opened.cx, |table, _| table.table())
        .expect("parsed off the render thread");
    assert_eq!(parsed.rows.len(), 3);
    assert_eq!(
        parsed.columns, 2,
        "the consistent semicolon is the separator"
    );
    opened
        .view
        .update(opened.cx, |view, cx| view.toggle_rendered(cx));
    opened.view.read_with(opened.cx, |view, _| {
        assert!(!view.shows_rendered());
        assert_eq!(view.mode_toggle_label(), Some(SHOW_TABLE));
        assert!(view.shows_text_position());
    });
    assert!(!super::super::controls::editor_display().csv_table);
    super::super::controls::set_editor_display(saved);
    stop_watching(&mut opened);
}

#[gpui::test]
fn a_csv_past_ten_megabytes_opens_as_large_text(cx: &mut TestAppContext) {
    let saved = super::super::controls::editor_display();
    assert!(saved.csv_table, "the table is the default");
    let mut opened = open_file(cx, "big.csv", &large_text(200_000, 63));
    opened.view.update(opened.cx, |view, cx| {
        assert!(matches!(view.preview(), Some(FilePreview::LargeText(_))));
        assert_eq!(view.mode_toggle_label(), Some(SHOW_TABLE));
        view.toggle_rendered(cx);
    });
    assert!(
        super::super::controls::editor_display().csv_table,
        "a toggle that cannot show the table leaves the preference alone"
    );
    assert!(opened.events.borrow().contains(&CodeViewEvent::Toast(
        "File too large to show as a table".into()
    )));
    super::super::controls::set_editor_display(saved);
    stop_watching(&mut opened);
}

#[gpui::test]
fn an_archive_lists_its_entries_and_walks_into_a_folder(cx: &mut TestAppContext) {
    let mut builder = tar::Builder::new(Vec::new());
    for (name, data) in [("pkg/a.txt", &b"alpha"[..]), ("top.txt", b"t")] {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, name, data)
            .expect("append");
    }
    let bytes = builder.into_inner().expect("tar");
    let mut opened = open_file(cx, "build.tar", &bytes);
    settle(&mut opened);
    let archive = opened
        .view
        .read_with(opened.cx, |view, _| match view.preview() {
            Some(FilePreview::Archive(archive)) => archive.clone(),
            _ => panic!("a tar opens in the archive viewer"),
        });
    archive.update(opened.cx, |archive, cx| {
        assert!(!archive.is_loading());
        let names: Vec<_> = archive.rows().into_iter().map(|row| row.name).collect();
        assert_eq!(names, vec!["pkg".to_string(), "top.txt".to_string()]);
        archive.enter("pkg".to_string(), cx);
        assert_eq!(archive.dir(), ["pkg".to_string()]);
        let names: Vec<_> = archive.rows().into_iter().map(|row| row.name).collect();
        assert_eq!(names, vec!["a.txt".to_string()]);
        archive.back(cx);
        assert!(archive.dir().is_empty());
    });
    stop_watching(&mut opened);
}

#[gpui::test]
fn an_editor_tab_whose_file_turns_binary_reopens_on_the_no_preview_screen(cx: &mut TestAppContext) {
    let mut opened = open_file(cx, "notes.txt", b"plain text\n");
    opened
        .view
        .read_with(opened.cx, |view, _| assert!(view.document().is_some()));
    std::fs::write(&opened.path, b"now\0binary").expect("rewrite");
    opened.view.update(opened.cx, |view, cx| {
        view.disk_unreadable(CodeLoadError::Binary, false, cx);
    });
    settle(&mut opened);
    opened.view.read_with(opened.cx, |view, _| {
        assert!(matches!(view.preview(), Some(FilePreview::NoPreview(_))));
    });
    stop_watching(&mut opened);
}
