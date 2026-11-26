#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result,
    clippy::panic
)]

mod common;

use std::time::{Duration, Instant};

use common::{DEADLINE, FixtureServer, Harness};
use paneflow_browser_client::{BrowserFrame, ClientEvent};
use paneflow_browser_proto::surface::{
    Cursor, DialogKind, FileMode, Input, KeyKind, Modifiers, MouseButton, View,
};
use paneflow_browser_proto::{Event, Reply, TabId};
use serde_json::Value;

fn present(harness: &Harness, tab: TabId, view: Option<View>) {
    assert_eq!(
        harness.client.present(tab, view).wait(DEADLINE),
        Ok(Reply::Done)
    );
}

fn frame_where(
    harness: &Harness,
    tab: TabId,
    what: &str,
    mut done: impl FnMut(&BrowserFrame) -> bool,
) -> BrowserFrame {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(frame) = harness.client.take_frame(tab)
            && done(&frame)
        {
            return frame;
        }
        assert!(Instant::now() < deadline, "never received {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn rect(harness: &Harness, tab: TabId, selector: &str) -> (i32, i32, i32, i32) {
    let value = harness.evaluate(
        tab,
        &format!(
            "(() => {{ const r = document.querySelector('{selector}').getBoundingClientRect(); return [r.left, r.top, r.width, r.height]; }})()"
        ),
    );
    let number = |index: usize| value[index].as_f64().unwrap().round() as i32;
    (number(0), number(1), number(2), number(3))
}

fn center(harness: &Harness, tab: TabId, selector: &str) -> (i32, i32) {
    let (x, y, width, height) = rect(harness, tab, selector);
    (x + width / 2, y + height / 2)
}

fn click(harness: &Harness, tab: TabId, (x, y): (i32, i32)) {
    let modifiers = Modifiers::default();
    harness
        .client
        .input(tab, Input::MouseMove { x, y, modifiers });
    for pressed in [true, false] {
        harness.client.input(
            tab,
            Input::MouseButton {
                x,
                y,
                button: MouseButton::Left,
                pressed,
                clicks: 1,
                modifiers: if pressed {
                    Modifiers(Modifiers::LEFT_BUTTON)
                } else {
                    modifiers
                },
            },
        );
    }
}

fn type_text(harness: &Harness, tab: TabId, text: &str) {
    for unit in text.encode_utf16() {
        harness.client.input(
            tab,
            Input::Key {
                kind: KeyKind::Char,
                windows_key_code: i32::from(unit),
                native_key_code: 0,
                character: unit,
                unmodified_character: unit,
                modifiers: Modifiers::default(),
            },
        );
    }
}

fn open_presented(harness: &mut Harness, server: &FixtureServer, path: &str, view: View) -> TabId {
    let tab = harness.open("w-surface", &server.url(path));
    harness.wait_loaded(tab, path);
    present(harness, tab, Some(view));
    harness.client.input(tab, Input::Focus { focused: true });
    tab
}

fn distinct_pixels(frame: &BrowserFrame) -> usize {
    let mut seen = std::collections::BTreeSet::new();
    for pixel in frame.pixels.as_chunks::<4>().0.iter().step_by(97) {
        seen.insert([pixel[0], pixel[1], pixel[2]]);
    }
    seen.len()
}

#[test]
fn frames_arrive_through_shared_memory_at_the_view_size_and_device_scale() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(540, 600, 1.0),
    );
    let frame = frame_where(&harness, tab, "a 540 x 600 frame", |frame| {
        frame.width == 540 && frame.height == 600
    });
    assert_eq!(frame.pixels.len(), 540 * 600 * 4);
    assert!(
        distinct_pixels(&frame) > 1,
        "the page painted a single color"
    );

    present(&harness, tab, Some(View::new(540, 600, 2.0)));
    let doubled = frame_where(&harness, tab, "a frame at scale 2", |frame| {
        frame.width == 1080
    });
    assert_eq!((doubled.width, doubled.height), (1080, 1200));
    assert_eq!(harness.evaluate(tab, "window.devicePixelRatio"), 2);

    present(&harness, tab, Some(View::new(400, 300, 2.0)));
    let resized = frame_where(&harness, tab, "a frame at the new size", |frame| {
        frame.width != 1080
    });
    assert_eq!((resized.width, resized.height), (800, 600));
    assert_eq!(
        harness.evaluate(tab, "[innerWidth, innerHeight]"),
        serde_json::json!([400, 300])
    );
    harness.shutdown();
}

#[test]
fn a_page_without_a_background_is_painted_on_opaque_white() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/form.html",
        View::new(320, 240, 1.0),
    );
    let frame = frame_where(&harness, tab, "a 320 x 240 frame", |frame| {
        frame.width == 320 && frame.height == 240
    });
    assert_eq!(
        frame.pixels.first_chunk::<4>(),
        Some(&[0xFF, 0xFF, 0xFF, 0xFF]),
        "the corner of a page with no background is not opaque white"
    );
    harness.shutdown();
}

#[test]
fn a_tab_that_is_not_displayed_produces_no_frames_and_keeps_its_last_size() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("w-surface", &server.url("/video.html"));
    harness.wait_loaded(tab, "/video.html");
    harness.evaluate_until(
        tab,
        "document.querySelector('video') && document.querySelector('video').currentTime > 0.3",
        |value| value == &Value::Bool(true),
    );
    assert_eq!(harness.client.frame_stats(tab).copied, 0);

    present(&harness, tab, Some(View::new(320, 240, 1.0)));
    frame_where(&harness, tab, "a first frame", |_| true);
    present(&harness, tab, None);
    let stopped = harness.client.frame_stats(tab).copied;
    let played = harness.evaluate(tab, "document.querySelector('video').currentTime");
    harness.evaluate_until(
        tab,
        &format!("document.querySelector('video').currentTime > {played} + 0.5"),
        |value| value == &Value::Bool(true),
    );
    assert_eq!(harness.client.frame_stats(tab).copied, stopped);
    assert!(harness.client.take_frame(tab).is_none());
    assert_eq!(
        harness.evaluate(tab, "[innerWidth, document.visibilityState]"),
        serde_json::json!([320, "visible"])
    );
    harness.shutdown();
}

#[test]
fn a_window_that_does_not_take_frames_holds_only_the_latest_one() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/video.html",
        View::new(320, 240, 1.0),
    );
    let deadline = Instant::now() + DEADLINE;
    while harness.client.frame_stats(tab).copied < 12 {
        assert!(
            Instant::now() < deadline,
            "the video never painted 12 frames"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let stats = harness.client.frame_stats(tab);
    let latest = harness.client.take_frame(tab).unwrap();
    assert!(latest.sequence >= stats.copied);
    assert!(stats.replaced_before_taken >= stats.copied - 1);
    assert!(
        harness
            .client
            .take_frame(tab)
            .is_none_or(|next| next.sequence > latest.sequence)
    );
    harness.shutdown();
}

fn submit_by_pointer(scale: f32) {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/form.html",
        View::new(540, 700, scale),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    click(&harness, tab, center(&harness, tab, "input[name=email]"));
    harness.evaluate_until(tab, "document.activeElement.name", |value| value == "email");
    type_text(&harness, tab, "ada@example.com");
    harness.evaluate_until(tab, "document.forms.f.email.value", |value| {
        value == "ada@example.com"
    });
    click(&harness, tab, center(&harness, tab, "button[type=submit]"));
    harness.wait_loaded(tab, "/submitted.html");
    let query = harness.evaluate(tab, "location.search");
    assert!(
        query.as_str().unwrap().contains("email=ada%40example.com"),
        "{query}"
    );
    harness.shutdown();
}

#[test]
fn a_click_at_the_submit_button_position_submits_the_form_at_scale_1() {
    submit_by_pointer(1.0);
}

#[test]
fn a_click_at_the_submit_button_position_submits_the_form_at_scale_2() {
    submit_by_pointer(2.0);
}

#[test]
fn a_shift_click_reaches_the_page_as_a_click_with_shift_held() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/form.html",
        View::new(540, 700, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    harness.evaluate(
        tab,
        "(() => { window.__clicks = []; addEventListener('click', (e) => { e.preventDefault(); window.__clicks.push(e.shiftKey); }, true); return true; })()",
    );
    let (x, y) = center(&harness, tab, "button[type=submit]");
    let shift = Modifiers::SHIFT;
    harness.client.input(
        tab,
        Input::MouseMove {
            x,
            y,
            modifiers: Modifiers(shift),
        },
    );
    for pressed in [true, false] {
        harness.client.input(
            tab,
            Input::MouseButton {
                x,
                y,
                button: MouseButton::Left,
                pressed,
                clicks: 1,
                modifiers: Modifiers(if pressed {
                    shift | Modifiers::LEFT_BUTTON
                } else {
                    shift
                }),
            },
        );
    }
    harness.evaluate_until(tab, "JSON.stringify(window.__clicks)", |value| {
        value == "[true]"
    });
    harness.shutdown();
}

#[test]
fn escape_as_the_window_sends_it_reaches_the_page_as_the_escape_key() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/form.html",
        View::new(540, 700, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    harness.evaluate(
        tab,
        "(() => { window.__keys = []; addEventListener('keydown', (e) => window.__keys.push(e.key + ':' + e.isTrusted), true); return true; })()",
    );
    for kind in [KeyKind::RawDown, KeyKind::Char] {
        harness.client.input(
            tab,
            Input::Key {
                kind,
                windows_key_code: 0x1B,
                native_key_code: 0,
                character: 27,
                unmodified_character: 27,
                modifiers: Modifiers::default(),
            },
        );
    }
    harness.evaluate_until(tab, "JSON.stringify(window.__keys)", |value| {
        value == "[\"Escape:true\"]"
    });
    harness.shutdown();
}

#[test]
fn a_committed_composition_lands_in_the_field_and_the_caret_is_reported() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(540, 600, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    click(&harness, tab, center(&harness, tab, "#field"));
    harness.evaluate_until(tab, "document.activeElement.id", |value| value == "field");
    harness.client.input(
        tab,
        Input::ImeSetComposition {
            text: "にほ".to_string(),
            selection_start: 2,
            selection_end: 2,
        },
    );
    let caret = harness.wait_for(
        "the composition caret",
        |event| matches!(event, ClientEvent::Browser(Event::ImeCaret { tab: t, .. }) if *t == tab),
    );
    let ClientEvent::Browser(Event::ImeCaret { caret, .. }) = caret else {
        unreachable!()
    };
    let (x, y, width, height) = rect(&harness, tab, "#field");
    assert!(
        caret.x >= x && caret.x <= x + width && caret.y >= y - 2 && caret.y <= y + height,
        "caret {caret:?} outside the field at {x},{y} {width}x{height}"
    );
    harness.client.input(
        tab,
        Input::ImeCommit {
            text: "日本".to_string(),
        },
    );
    harness.evaluate_until(tab, "document.getElementById('field').value", |value| {
        value == "日本"
    });
    harness.shutdown();
}

#[test]
fn the_page_cursor_is_reported_over_a_pointer_area_and_reset_outside_it() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(540, 600, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    let modifiers = Modifiers::default();
    harness.client.input(
        tab,
        Input::MouseMove {
            x: 50,
            y: 50,
            modifiers,
        },
    );
    harness.wait_for("the pointer cursor", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::Cursor {
                cursor: Cursor::Pointer,
                ..
            })
        )
    });
    harness.client.input(
        tab,
        Input::MouseMove {
            x: 400,
            y: 500,
            modifiers,
        },
    );
    harness.wait_for("the default cursor", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::Cursor {
                cursor: Cursor::Default,
                ..
            })
        )
    });
    harness.shutdown();
}

fn dialog(harness: &mut Harness) -> (u64, DialogKind, String, String) {
    match harness.wait_for("a page dialog", |event| {
        matches!(event, ClientEvent::Browser(Event::DialogOpened { .. }))
    }) {
        ClientEvent::Browser(Event::DialogOpened {
            dialog,
            kind,
            origin,
            message,
            ..
        }) => (dialog, kind, origin, message),
        _ => unreachable!(),
    }
}

#[test]
fn confirm_and_prompt_wait_for_the_window_and_the_page_receives_the_answer() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(540, 600, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);

    click(&harness, tab, center(&harness, tab, "#confirm"));
    let (id, kind, origin, message) = dialog(&mut harness);
    assert_eq!(kind, DialogKind::Confirm);
    assert_eq!(origin, format!("http://127.0.0.1:{}", server.port));
    assert_eq!(message, "Proceed?");
    assert_eq!(
        harness
            .client
            .answer_dialog(tab, id, true, None)
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.evaluate_until(tab, "window.answer", |value| value == &Value::Bool(true));

    click(&harness, tab, center(&harness, tab, "#prompt"));
    let (id, kind, _, message) = dialog(&mut harness);
    assert_eq!((kind, message.as_str()), (DialogKind::Prompt, "Name?"));
    harness
        .client
        .answer_dialog(tab, id, true, Some("Grace".to_string()))
        .wait(DEADLINE)
        .unwrap();
    harness.evaluate_until(tab, "window.answer", |value| value == "Grace");
    assert!(
        harness
            .client
            .answer_dialog(tab, id, true, None)
            .wait(DEADLINE)
            .is_err()
    );
    harness.shutdown();
}

#[test]
fn a_file_chooser_waits_for_the_window_and_the_page_gets_only_the_chosen_file() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(540, 600, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    let chosen = tempfile::Builder::new()
        .prefix("receipt-")
        .suffix(".txt")
        .tempfile()
        .unwrap();
    std::fs::write(chosen.path(), b"paid").unwrap();
    click(&harness, tab, center(&harness, tab, "#files"));
    let request = match harness.wait_for("a file chooser", |event| {
        matches!(event, ClientEvent::Browser(Event::FileChooserOpened { .. }))
    }) {
        ClientEvent::Browser(Event::FileChooserOpened { request, mode, .. }) => {
            assert_eq!(mode, FileMode::Open);
            request
        }
        _ => unreachable!(),
    };
    harness
        .client
        .answer_files(
            tab,
            request,
            Some(vec![chosen.path().display().to_string()]),
        )
        .wait(DEADLINE)
        .unwrap();
    let name = chosen.path().file_name().unwrap().to_str().unwrap();
    harness.evaluate_until(
        tab,
        "Array.from(document.getElementById('files').files).map(f => f.name)",
        |value| value == &serde_json::json!([name]),
    );
    harness.shutdown();
}

#[test]
fn an_open_select_list_is_drawn_below_its_control_inside_the_page_and_answers_keys() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(540, 600, 1.0),
    );
    let (x, y, width, height) = rect(&harness, tab, "#pick");
    let before = frame_where(&harness, tab, "a first frame", |_| true);
    let below = |frame: &BrowserFrame| {
        let px = (x + width / 2) as usize;
        let py = (y + height + 12) as usize;
        let offset = (py * frame.width as usize + px) * 4;
        frame.pixels[offset..offset + 3].to_vec()
    };
    let background = below(&before);
    click(&harness, tab, center(&harness, tab, "#pick"));
    let opened = frame_where(&harness, tab, "a frame with the open list", |frame| {
        frame.width == 540 && below(frame) != background
    });
    assert_eq!(opened.height, 600);
    for key in [0x28, 0x0D] {
        for kind in [KeyKind::RawDown, KeyKind::Up] {
            harness.client.input(
                tab,
                Input::Key {
                    kind,
                    windows_key_code: key,
                    native_key_code: 0,
                    character: 0,
                    unmodified_character: 0,
                    modifiers: Modifiers::default(),
                },
            );
        }
    }
    harness.evaluate_until(tab, "document.getElementById('pick').value", |value| {
        value == "us"
    });
    frame_where(&harness, tab, "a frame with the list closed", |frame| {
        below(frame) == background
    });
    harness.shutdown();
}

#[test]
fn a_link_that_opens_a_new_window_is_reported_and_not_opened_by_the_engine() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(540, 600, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    click(&harness, tab, center(&harness, tab, "#popup"));
    match harness.wait_for("a new window request", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::NewWindowRequested { .. })
        )
    }) {
        ClientEvent::Browser(Event::NewWindowRequested {
            tab: opener,
            url,
            user_gesture,
        }) => {
            assert_eq!(opener, tab);
            assert_eq!(url, server.url("/test/popup-target"));
            assert!(user_gesture);
        }
        _ => unreachable!(),
    }
    assert_eq!(harness.client.tabs(), vec![tab]);
    assert_eq!(harness.evaluate(tab, "location.pathname"), "/test/surface");
    harness.shutdown();
}

#[test]
fn a_frame_too_large_to_share_is_reported_and_the_tab_stays_drivable() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(6000, 6000, 1.0),
    );
    match harness.wait_for("the presentation failure", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::PresentationFailed { .. })
        )
    }) {
        ClientEvent::Browser(Event::PresentationFailed {
            tab: failed,
            reason,
        }) => {
            assert_eq!(failed, tab);
            assert!(reason.contains("shared memory"), "{reason}");
        }
        _ => unreachable!(),
    }
    assert!(harness.client.take_frame(tab).is_none());
    assert_eq!(harness.evaluate(tab, "document.title"), "Surface");
    present(&harness, tab, Some(View::new(320, 240, 1.0)));
    frame_where(&harness, tab, "a frame after the failure", |frame| {
        frame.width == 320
    });
    harness.shutdown();
}

#[test]
fn input_sent_after_the_host_died_is_dropped_and_the_tab_reports_the_crash() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/test/surface",
        View::new(320, 240, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    let pid = harness.host_pid();
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    harness.wait_for(
        "the crash",
        |event| matches!(event, ClientEvent::Browser(Event::Crashed { tab: t, .. }) if *t == tab),
    );
    type_text(&harness, tab, "lost");
    click(&harness, tab, (10, 10));
    assert!(harness.client.host_pid().is_none());
    assert!(harness.client.take_frame(tab).is_none());
    assert!(matches!(
        harness.client.tab_status(tab),
        Some(paneflow_browser_client::TabStatus::Crashed { .. })
    ));
}

fn history_state(harness: &mut Harness, tab: TabId, back: bool) -> bool {
    let event = harness.wait_for(&format!("tab {} settled, back {back}", tab.0), |event| {
        matches!(event, ClientEvent::Browser(Event::LoadingState { tab: t, loading: false, can_go_back, .. })
            if *t == tab && *can_go_back == back)
    });
    matches!(event, ClientEvent::Browser(Event::LoadingState { can_go_forward, .. }) if can_go_forward)
}

#[test]
fn going_back_reloads_the_previous_page_and_never_reaches_the_blank_placeholder() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(&mut harness, &server, "/app.html", View::new(400, 300, 1.0));
    assert!(
        !history_state(&mut harness, tab, false),
        "a new tab has no history"
    );
    assert_eq!(
        harness
            .client
            .navigate(tab, server.url("/form.html"))
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_loaded(tab, "/form.html");
    history_state(&mut harness, tab, true);
    harness.seen.clear();
    let form = frame_where(&harness, tab, "a frame of the form", |_| true);
    while harness.client.take_frame(tab).is_some() {}

    assert_eq!(
        harness
            .client
            .history(tab, paneflow_browser_proto::surface::HistoryStep::Back)
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_for("the address of the previous page", |event| {
        matches!(event, ClientEvent::Browser(Event::AddressChanged { tab: t, url }) if *t == tab && url.contains("/app.html"))
    });
    assert!(
        history_state(&mut harness, tab, false),
        "back on the first page: nothing before it, the next page ahead"
    );
    assert_eq!(
        harness.evaluate(tab, "location.pathname"),
        Value::from("/app.html")
    );
    frame_where(&harness, tab, "a frame of the previous page", |frame| {
        frame.pixels != form.pixels
    });
}

#[test]
fn the_first_page_after_a_blank_start_replaces_it_in_the_history() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("w-surface", "about:blank");
    harness.wait_loaded(tab, "about:blank");
    assert_eq!(
        harness
            .client
            .navigate(tab, server.url("/app.html"))
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_loaded(tab, "/app.html");
    assert!(!history_state(&mut harness, tab, false));
    assert_eq!(harness.evaluate(tab, "history.length"), Value::from(1));
    assert_eq!(
        harness
            .client
            .navigate(tab, server.url("/form.html"))
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_loaded(tab, "/form.html");
    history_state(&mut harness, tab, true);
    assert_eq!(
        harness.evaluate(tab, "history.length"),
        Value::from(2),
        "later pages are added, not replaced"
    );
}

#[test]
fn the_annotator_keeps_the_arrow_over_text_fields_instead_of_a_theme_crosshair() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/form.html",
        View::new(540, 700, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    let (x, y) = center(&harness, tab, "input[name=email]");
    let modifiers = Modifiers::default();
    harness
        .client
        .input(tab, Input::MouseMove { x, y, modifiers });
    harness.wait_for("the text cursor over the field", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::Cursor {
                cursor: Cursor::Text,
                ..
            })
        )
    });
    let call = |method: &str, params: Value| {
        let Ok(Reply::DevTools(result)) =
            harness.client.devtools(tab, method, params).wait(DEADLINE)
        else {
            panic!("{method} failed");
        };
        result
    };
    call("Runtime.enable", serde_json::json!({}));
    let tree = call("Page.getFrameTree", serde_json::json!({}));
    let frame = tree["frameTree"]["frame"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        "Runtime.addBinding",
        serde_json::json!({"name": "__paneflowAnnotate", "executionContextName": "annotator"}),
    );
    let world = call(
        "Page.createIsolatedWorld",
        serde_json::json!({"frameId": frame, "worldName": "annotator"}),
    );
    let context = world["executionContextId"].as_i64().unwrap();
    let script = include_str!("../../../src-app/src/browser/annotate/annotator.js");
    let installed = call(
        "Runtime.evaluate",
        serde_json::json!({"expression": script, "contextId": context}),
    );
    assert!(installed.get("exceptionDetails").is_none(), "{installed}");
    call(
        "Runtime.evaluate",
        serde_json::json!({"expression": "window.__paneflowAnnotator.start(1, {accent: '#3b82f6', dark: true})", "contextId": context}),
    );
    harness.client.input(
        tab,
        Input::MouseMove {
            x: x + 3,
            y: y + 1,
            modifiers,
        },
    );
    let reported = harness.wait_for(
        "the cursor while annotating",
        |event| matches!(event, ClientEvent::Browser(Event::Cursor { tab: t, .. }) if *t == tab),
    );
    assert!(
        matches!(
            reported,
            ClientEvent::Browser(Event::Cursor {
                cursor: Cursor::Default,
                ..
            })
        ),
        "{reported:?}"
    );
    harness.shutdown();
}

const DEEP_ELEMENTS: &str = "(() => { const all = []; const walk = (root) => { for (const el of root.querySelectorAll('*')) { all.push(el); if (el.shadowRoot) walk(el.shadowRoot); } }; walk(document); return all; })()";
const DEEP_TEXT: &str = "(() => { const walk = (node) => [...node.childNodes].map((child) => (child.shadowRoot ? walk(child.shadowRoot) : '') + (child.nodeType === 3 ? child.data : walk(child))).join(' '); return [walk(document)]; })()";

#[test]
fn an_inspector_opens_off_screen_beside_its_page_and_closes_with_it() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = open_presented(
        &mut harness,
        &server,
        "/form.html",
        View::new(540, 700, 1.0),
    );
    frame_where(&harness, tab, "a first frame", |_| true);
    let (inspector, ticket) = harness.client.inspect(tab);
    assert_eq!(ticket.wait(DEADLINE), Ok(Reply::Done));
    present(&harness, inspector, Some(View::new(480, 700, 1.0)));
    harness.evaluate_until(inspector, "location.href", |value| {
        value
            .as_str()
            .is_some_and(|url| url.starts_with("devtools://devtools/"))
    });
    harness.evaluate_until(
        inspector,
        "(() => { const walk = (node) => [...node.childNodes].map((child) => (child.shadowRoot ? walk(child.shadowRoot) : '') + (child.nodeType === 3 ? child.data : walk(child))).join(' '); return walk(document).includes('DOCTYPE'); })()",
        |value| value == &Value::Bool(true),
    );
    let frame = frame_where(
        &harness,
        inspector,
        "an inspector frame with its interface",
        |frame| frame.width == 480 && distinct_pixels(frame) > 6,
    );
    assert_eq!(frame.height, 700);
    harness.evaluate_until(inspector, "navigator.language", |value| value == "en-US");
    let agent = harness.evaluate(inspector, "navigator.userAgent");
    assert!(
        agent
            .as_str()
            .is_some_and(|agent| agent.contains("Chrome/")),
        "{agent}"
    );
    let text = harness.evaluate(inspector, &format!("{DEEP_TEXT}.join(' ')"));
    assert!(
        !text
            .as_str()
            .unwrap_or_default()
            .contains("DevTools is now available"),
        "the language bar stays hidden"
    );
    harness
        .client
        .input(inspector, Input::Focus { focused: true });
    let more_tabs = harness.evaluate_until(
        inspector,
        &format!("(() => {{ const el = {DEEP_ELEMENTS}.find((e) => /more tabs/i.test(e.getAttribute('aria-label') || '')); if (!el) return null; const r = el.getBoundingClientRect(); return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)]; }})()"),
        Value::is_array,
    );
    let at = |index: usize| {
        more_tabs[index]
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or_default()
    };
    click(&harness, inspector, (at(0), at(1)));
    harness.evaluate_until(
        inspector,
        &format!("{DEEP_ELEMENTS}.some((e) => e.classList.contains('soft-context-menu'))"),
        |value| value == &Value::Bool(true),
    );
    let (second, again) = harness.client.inspect(tab);
    assert!(
        matches!(
            again.wait(DEADLINE),
            Err(paneflow_browser_proto::CommandError::Rejected { .. })
        ),
        "a second inspector for tab {} ({}) is refused",
        tab.0,
        second.0
    );
    assert_eq!(
        harness.client.close_tab(tab).wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_for("the inspector to close with its page", |event| {
        matches!(event, ClientEvent::Browser(Event::Closed { tab: closed }) if *closed == inspector)
    });
    harness.shutdown();
}
