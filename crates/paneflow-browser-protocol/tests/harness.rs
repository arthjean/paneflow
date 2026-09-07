mod support;

use paneflow_browser_protocol::{write_message, MAX_MESSAGE_BYTES};
use serde_json::json;
use support::*;

#[test]
fn oversized_outgoing_message_never_emits_a_partial_frame() {
    let mut output = Vec::new();
    let result = write_message(&mut output, &"x".repeat(MAX_MESSAGE_BYTES));
    assert!(result.is_err());
    assert!(output.is_empty());
}

#[test]
fn absent_backend_preserves_terminal_capability_without_creating_a_browser() -> TestResult {
    let mut harness = Harness::new(false)?;
    let capability = harness.send(json!({"type": "capabilities"}))?;
    assert_eq!(capability["Ok"]["availability"], "absent");
    assert_eq!(capability["Ok"]["terminal_available"], true);
    assert_error(
        &harness.send(create("workspace", "session", "browser", "profile"))?,
        "unavailable",
    );
    assert_error(
        &harness.send(command(
            "state",
            &document("workspace", "session", "browser", 1),
        ))?,
        "unknown_identity",
    );
    Ok(())
}

#[test]
fn scope_comes_from_the_transport_and_never_falls_back_to_an_active_page() -> TestResult {
    let mut harness = Harness::new(true)?;
    let target = harness.create("browser")?;
    let capability = harness.send(json!({"type": "capabilities"}))?;
    assert_eq!(capability["Ok"]["availability"], "development");
    for foreign in [
        document("foreign", "session", "browser", 1),
        document("workspace", "foreign", "browser", 1),
    ] {
        assert_error(&harness.send(command("state", &foreign))?, "access_denied");
    }
    assert_error(
        &harness.send(create("foreign", "session", "other", "other"))?,
        "access_denied",
    );
    assert_error(
        &harness.send(command(
            "state",
            &document("workspace", "session", "unknown", 1),
        ))?,
        "unknown_identity",
    );
    assert_eq!(
        harness.send(command("state", &target))?["Ok"]["session"]["document"],
        target
    );
    Ok(())
}

#[test]
fn framing_rejects_oversize_truncation_unknown_fields_and_versions() -> TestResult {
    let mut cases = vec![
        (
            (MAX_MESSAGE_BYTES as u32 + 1).to_be_bytes().to_vec(),
            "too_large",
        ),
        (vec![0, 0], "invalid_message"),
        (vec![0, 0, 0, 4, b'{'], "invalid_message"),
    ];
    for (value, expected) in [
        (
            json!({"version": paneflow_browser_protocol::CONTRACT_VERSION + 1, "operation": "op", "command": {"type": "capabilities"}}),
            "incompatible_version",
        ),
        (
            json!({"version": paneflow_browser_protocol::CONTRACT_VERSION, "operation": "op", "workspace": "foreign", "command": {"type": "capabilities"}}),
            "invalid_message",
        ),
        (
            json!({"version": paneflow_browser_protocol::CONTRACT_VERSION, "operation": "op", "command": {"type": "not_a_command"}}),
            "invalid_message",
        ),
        (
            json!({"version": paneflow_browser_protocol::CONTRACT_VERSION, "operation": "", "command": {"type": "capabilities"}}),
            "invalid_message",
        ),
    ] {
        let mut bytes = Vec::new();
        write_message(&mut bytes, &value)?;
        cases.push((bytes, expected));
    }
    for (bytes, expected) in cases {
        let output = raw(bytes)?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr)?.trim(),
            format!("\"{expected}\"")
        );
    }
    let value = json!({"version": paneflow_browser_protocol::CONTRACT_VERSION, "operation": "op", "command": {"type": "capabilities"}});
    let mut bytes = Vec::new();
    write_message(&mut bytes, &value)?;
    bytes.resize(4 + MAX_MESSAGE_BYTES, b' ');
    bytes[..4].copy_from_slice(&(MAX_MESSAGE_BYTES as u32).to_be_bytes());
    assert!(raw(bytes)?.status.success());
    Ok(())
}

#[test]
fn bounded_url_title_and_input_accept_the_limit_and_reject_limit_plus_one() -> TestResult {
    let mut harness = Harness::new(true)?;
    let prefix = "https://example.test/";
    let mut create_at_limit = create("workspace", "session", "browser", "profile");
    create_at_limit["url"] = format!("{prefix}{}", "a".repeat(8192 - prefix.len())).into();
    create_at_limit["title"] = "é".repeat(512).into();
    let target = harness.send(create_at_limit.clone())?["Ok"]["session"]["document"].clone();
    assert!(target.is_object());
    create_at_limit["browser"] = "too-long-url".into();
    create_at_limit["url"] = format!("{prefix}{}", "a".repeat(8193 - prefix.len())).into();
    assert_error(&harness.send(create_at_limit)?, "too_large");
    let mut bad_title = create("workspace", "session", "too-long-title", "profile");
    bad_title["title"] = "é".repeat(513).into();
    assert_error(&harness.send(bad_title)?, "too_large");
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://user:secret@example.test/",
        "https://example.test/\n",
    ] {
        let mut invalid = create("workspace", "session", "invalid", "profile");
        invalid["url"] = url.into();
        assert_error(&harness.send(invalid)?, "invalid_url");
    }
    harness.send(command("start", &target))?;
    let mut input = operation(&target, false);
    input["text"] = "a".repeat(65536).into();
    assert_eq!(harness.send(input.clone())?["Ok"]["type"], "accepted");
    input["text"] = "a".repeat(65537).into();
    assert_error(&harness.send(input)?, "too_large");
    Ok(())
}

#[test]
fn profiles_are_shared_inside_one_workspace_and_isolated_across_workspaces() -> TestResult {
    let replies = batch(vec![
        (
            "one",
            "first",
            vec![create("one", "first", "one-a", "profile-one")],
        ),
        (
            "one",
            "second",
            vec![
                create("one", "second", "one-b", "profile-one"),
                create("one", "second", "one-c", "profile-two"),
            ],
        ),
        (
            "two",
            "first",
            vec![
                create("two", "first", "two-a", "profile-one"),
                create("two", "first", "two-b", "profile-two"),
                command("state", &document("one", "first", "one-a", 1)),
            ],
        ),
    ])?;
    assert_eq!(replies[0]["Ok"]["type"], "state");
    assert_eq!(replies[1]["Ok"]["type"], "state");
    assert_error(&replies[2], "access_denied");
    assert_error(&replies[3], "access_denied");
    assert_eq!(replies[4]["Ok"]["type"], "state");
    assert_error(&replies[5], "access_denied");
    Ok(())
}

#[test]
fn descriptor_and_live_page_limits_are_global_and_do_not_remove_existing_pages() -> TestResult {
    let sessions: Vec<_> = (0..9).map(|index| format!("session-{index}")).collect();
    let mut channels = Vec::new();
    for (index, session) in sessions.iter().enumerate() {
        let mut commands = Vec::new();
        for browser in 0..8 {
            commands.push(create(
                "workspace",
                session,
                &format!("browser-{index}-{browser}"),
                "profile",
            ));
        }
        if index == 0 {
            commands.push(create("workspace", session, "ninth-in-session", "profile"));
        }
        channels.push(("workspace", session.as_str(), commands));
    }
    let replies = batch(channels)?;
    assert_eq!(
        replies
            .iter()
            .filter(|reply| reply.get("Ok").is_some())
            .count(),
        64
    );
    assert_eq!(
        replies
            .iter()
            .filter(|reply| reply["Err"] == "limit_reached")
            .count(),
        9
    );
    let mut first = Vec::new();
    for browser in 0..8 {
        first.push(create(
            "workspace",
            "one",
            &format!("browser-{browser}"),
            "profile",
        ));
        first.push(command(
            "start",
            &document(
                "workspace",
                "one",
                &format!("browser-{browser}"),
                browser + 1,
            ),
        ));
    }
    let second = vec![
        create("workspace", "two", "ninth", "profile"),
        command("start", &document("workspace", "two", "ninth", 9)),
        json!({"type": "capabilities"}),
    ];
    let asleep = document("workspace", "one", "browser-0", 1);
    let ninth = document("workspace", "two", "ninth", 9);
    let replies = batch(vec![
        ("workspace", "one", first),
        ("workspace", "two", second),
        (
            "workspace",
            "one",
            vec![command("sleep", &asleep), command("sleep", &asleep)],
        ),
        (
            "workspace",
            "two",
            vec![
                command("start", &ninth),
                command("sleep", &ninth),
                command("state", &ninth),
            ],
        ),
    ])?;
    assert_eq!(
        replies[..17]
            .iter()
            .filter(|reply| reply.get("Ok").is_some())
            .count(),
        17
    );
    assert_error(&replies[17], "limit_reached");
    assert_eq!(replies[18]["Ok"]["terminal_available"], true);
    assert_eq!(replies[19]["Ok"]["session"]["state"], "dormant");
    assert_eq!(replies[20]["Ok"]["session"]["state"], "dormant");
    assert_eq!(replies[21]["Ok"]["session"]["state"], "hidden");
    assert_eq!(replies[22]["Ok"]["session"]["state"], "dormant");
    assert_eq!(replies[23]["Ok"]["session"]["document"]["generation"], 9);
    Ok(())
}

#[test]
fn operations_enforce_workspace_global_and_per_browser_mutation_limits() -> TestResult {
    let workspaces: Vec<_> = (0..5).map(|index| format!("workspace-{index}")).collect();
    let mut channels = Vec::new();
    for (index, workspace) in workspaces.iter().enumerate() {
        let target = document(
            workspace,
            "session",
            &format!("browser-{index}"),
            index as u64 + 1,
        );
        let mut commands = vec![
            create(
                workspace,
                "session",
                &format!("browser-{index}"),
                &format!("profile-{index}"),
            ),
            command("start", &target),
        ];
        commands.extend((0..5).map(|_| operation(&target, false)));
        channels.push((workspace.as_str(), "session", commands));
    }
    let replies = batch(channels)?;
    assert_eq!(
        replies
            .iter()
            .filter(|reply| reply["Ok"]["type"] == "accepted")
            .count(),
        16
    );
    assert_eq!(
        replies
            .iter()
            .filter(|reply| reply["Err"] == "busy")
            .count(),
        9
    );
    let mut harness = Harness::new(true)?;
    let target = harness.create("browser")?;
    harness.send(command("start", &target))?;
    let accepted = harness.send_as(operation(&target, true), "request")?;
    assert_eq!(accepted["Ok"]["type"], "accepted");
    let pending = accepted["Ok"]["operation"].clone();
    assert_error(&harness.send(operation(&target, true))?, "busy");
    assert_eq!(
        harness.send(operation(&target, false))?["Ok"]["type"],
        "accepted"
    );
    let completion = json!({"type": "complete_operation", "document": target, "pending": pending});
    assert_eq!(harness.send(completion.clone())?["Ok"]["type"], "completed");
    assert_error(&harness.send(completion.clone())?, "unknown_identity");
    let replacement = harness.send_as(operation(&target, true), "request")?;
    assert_eq!(replacement["Ok"]["type"], "accepted");
    assert_ne!(replacement["Ok"]["operation"], pending);
    assert_error(&harness.send(completion)?, "unknown_identity");
    Ok(())
}

#[test]
fn presentation_visibility_does_not_own_document_lifetime() -> TestResult {
    let mut harness = Harness::new(true)?;
    let target = harness.create("browser")?;
    assert_error(
        &harness.send(present(&target, 1, 640, true))?,
        "unavailable",
    );
    harness.send(command("start", &target))?;
    assert_eq!(
        harness.send(present(&target, 1, 640, true))?["Ok"]["session"]["state"],
        "visible"
    );
    let hidden = harness.send(present(&target, 1, 640, false))?;
    assert_eq!(hidden["Ok"]["session"]["document"], target);
    assert_eq!(hidden["Ok"]["session"]["state"], "hidden");
    assert_error(&harness.send(frame(&target, 1, 0, 1))?, "unavailable");
    assert_eq!(
        harness.send(present(&target, 1, 640, true))?["Ok"]["session"]["document"],
        target
    );
    Ok(())
}

#[test]
fn frame_pools_bound_live_handles_and_require_exact_acknowledgements() -> TestResult {
    let mut harness = Harness::new(true)?;
    let target = harness.create("browser")?;
    harness.send(command("start", &target))?;
    harness.send(present(&target, 1, 640, true))?;
    for buffer in 0..3 {
        assert_eq!(
            harness.send(frame(&target, 1, buffer, u64::from(buffer) + 1))?["Ok"]["type"],
            "frame_accepted"
        );
    }
    assert_error(&harness.send(frame(&target, 1, 3, 4))?, "invalid_frame");
    assert_error(&harness.send(frame(&target, 1, 0, 4))?, "invalid_frame");
    assert_error(
        &harness.send(present(&target, 1, 800, true))?,
        "stale_generation",
    );
    harness.send(present(&target, 2, 800, true))?;
    for buffer in 0..3 {
        assert_eq!(
            harness.send(frame(&target, 2, buffer, u64::from(buffer) + 1))?["Ok"]["type"],
            "frame_accepted"
        );
    }
    assert_error(&harness.send(present(&target, 3, 900, true))?, "busy");
    assert_error(&harness.send(frame(&target, 1, 0, 4))?, "stale_generation");
    assert_error(&harness.send(command("close", &target))?, "busy");
    assert_error(&harness.send(release(&target, 1, 0, 99))?, "invalid_frame");
    for buffer in 0..3 {
        assert_eq!(
            harness.send(release(&target, 1, buffer, u64::from(buffer) + 1))?["Ok"]["type"],
            "frame_released"
        );
    }
    assert_error(&harness.send(release(&target, 1, 0, 1))?, "invalid_frame");
    assert_eq!(
        harness.send(present(&target, 3, 900, true))?["Ok"]["type"],
        "state"
    );
    Ok(())
}

#[test]
fn navigation_retires_old_frames_and_recreated_identity_never_reuses_a_generation() -> TestResult {
    let mut harness = Harness::new(true)?;
    let old = harness.create("browser")?;
    harness.send(command("start", &old))?;
    harness.send(present(&old, 1, 640, true))?;
    harness.send(frame(&old, 1, 0, 1))?;
    let accepted = harness.send_as(operation(&old, true), "old-request")?;
    let pending = accepted["Ok"]["operation"].clone();
    let navigation = harness
        .send(json!({"type": "navigate", "document": old, "url": "https://example.test/next"}))?;
    let current = navigation["Ok"]["session"]["document"].clone();
    assert_ne!(current["generation"], old["generation"]);
    assert_error(&harness.send(command("state", &old))?, "stale_generation");
    assert_error(&harness.send(frame(&old, 1, 0, 2))?, "stale_generation");
    assert_error(
        &harness
            .send(json!({"type": "complete_operation", "document": old, "pending": pending}))?,
        "stale_generation",
    );
    harness.send(present(&current, 1, 640, true))?;
    assert_eq!(
        harness.send(frame(&current, 1, 0, 1))?["Ok"]["type"],
        "frame_accepted"
    );
    assert_eq!(
        harness.send(release(&old, 1, 0, 1))?["Ok"]["type"],
        "frame_released"
    );
    assert_error(&harness.send(frame(&current, 1, 0, 2))?, "invalid_frame");
    harness.send(release(&current, 1, 0, 1))?;
    harness.send(command("close", &current))?;
    let recreated = harness.create("browser")?;
    assert_ne!(recreated["generation"], current["generation"]);
    assert_error(
        &harness.send(command("state", &current))?,
        "stale_generation",
    );
    Ok(())
}

#[test]
fn scale_changes_are_generation_changes_and_input_needs_a_live_page() -> TestResult {
    let mut harness = Harness::new(true)?;
    let target = harness.create("browser")?;
    let click = json!({"type": "input", "document": target, "input": {"type": "mouse_button", "x": 10, "y": 10, "button": "left", "down": true, "clicks": 1, "modifiers": 16}});
    assert_error(&harness.send(click.clone())?, "unavailable");
    harness.send(command("start", &target))?;
    assert_eq!(harness.send(click)?["Ok"]["type"], "input_accepted");
    let bad_click = json!({"type": "input", "document": target, "input": {"type": "mouse_button", "x": 10, "y": 10, "button": "left", "down": true, "clicks": 4, "modifiers": 16}});
    assert_error(&harness.send(bad_click)?, "invalid_message");
    let foreign_modifier = json!({"type": "input", "document": target, "input": {"type": "key", "kind": "char", "key_code": 65, "native_key_code": 38, "character": 97, "unmodified_character": 97, "modifiers": 1}});
    assert_error(&harness.send(foreign_modifier)?, "invalid_message");
    let scaled = |generation: u64, scale: u32| json!({"type": "present", "document": target, "presentation": {"mounted": true, "visible": true, "width": 640, "height": 480, "generation": generation, "scale_percent": scale}});
    assert_eq!(
        harness.send(scaled(1, 100))?["Ok"]["session"]["presentation"]["scale_percent"],
        100
    );
    assert_eq!(
        harness.send(frame(&target, 1, 0, 1))?["Ok"]["type"],
        "frame_accepted"
    );
    assert_error(&harness.send(scaled(1, 200))?, "stale_generation");
    assert_eq!(
        harness.send(scaled(2, 200))?["Ok"]["session"]["presentation"]["scale_percent"],
        200
    );
    assert_error(&harness.send(frame(&target, 1, 1, 2))?, "stale_generation");
    assert_error(&harness.send(scaled(3, 25))?, "invalid_frame");
    assert_eq!(
        harness.send(present(&target, 3, 640, true))?["Ok"]["session"]["presentation"]
            ["scale_percent"],
        100
    );
    Ok(())
}

#[test]
fn navigation_commands_need_a_live_page_and_validate_zoom() -> TestResult {
    let mut harness = Harness::new(true)?;
    let target = harness.create("browser")?;
    let back = json!({"type": "history", "document": target, "direction": "back"});
    assert_error(&harness.send(back.clone())?, "unavailable");
    harness.send(command("start", &target))?;
    assert_eq!(harness.send(back)?["Ok"]["type"], "navigation_accepted");
    assert_eq!(
        harness.send(json!({"type": "reload", "document": target, "ignore_cache": true}))?["Ok"]
            ["type"],
        "navigation_accepted"
    );
    assert_eq!(
        harness.send(command("stop", &target))?["Ok"]["type"],
        "navigation_accepted"
    );
    assert_eq!(
        harness.send(json!({"type": "mute", "document": target, "muted": true}))?["Ok"]["type"],
        "navigation_accepted"
    );
    assert_eq!(
        harness.send(json!({"type": "zoom", "document": target, "percent": 500}))?["Ok"]["type"],
        "navigation_accepted"
    );
    assert_error(
        &harness.send(json!({"type": "zoom", "document": target, "percent": 501}))?,
        "invalid_message",
    );
    assert_error(
        &harness.send(json!({"type": "zoom", "document": target, "percent": 24}))?,
        "invalid_message",
    );
    let stale = document("workspace", "session", "browser", 99);
    assert_error(
        &harness.send(json!({"type": "history", "document": stale, "direction": "forward"}))?,
        "stale_generation",
    );
    Ok(())
}

#[test]
fn ime_events_are_bounded_and_need_a_live_page() -> TestResult {
    let mut harness = Harness::new(true)?;
    let target = harness.create("browser")?;
    let composition = |text: &str, cursor: u32| json!({"type": "input", "document": target, "input": {"type": "ime_composition", "text": text, "cursor": cursor}});
    assert_error(&harness.send(composition("か", 1))?, "unavailable");
    harness.send(command("start", &target))?;
    assert_eq!(
        harness.send(composition("か", 1))?["Ok"]["type"],
        "input_accepted"
    );
    assert_error(&harness.send(composition("か", 2))?, "invalid_message");
    assert_error(&harness.send(composition("a\u{7}", 0))?, "invalid_message");
    let commit = json!({"type": "input", "document": target, "input": {"type": "ime_commit", "text": "漢字"}});
    assert_eq!(harness.send(commit)?["Ok"]["type"], "input_accepted");
    let cancel = json!({"type": "input", "document": target, "input": {"type": "ime_cancel"}});
    assert_eq!(harness.send(cancel)?["Ok"]["type"], "input_accepted");
    for input in [
        json!({"type":"ime_composition","text":"日本","cursor":2,"selection_start":0,"replacement":[4,7]}),
        json!({"type":"ime_commit","text":"","replacement":[4,7]}),
        json!({"type":"ime_finish"}),
    ] {
        assert_eq!(
            harness.send(json!({"type":"input","document":target,"input":input}))?["Ok"]["type"],
            "input_accepted"
        );
    }
    for input in [
        json!({"type":"ime_composition","text":"日","cursor":1,"selection_start":2}),
        json!({"type":"ime_commit","text":"日","replacement":[7,4]}),
        json!({"type":"ime_commit","text":"日","replacement":[0,4294967295_u32]}),
    ] {
        assert_error(
            &harness.send(json!({"type":"input","document":target,"input":input}))?,
            "invalid_message",
        );
    }
    let oversized = json!({"type": "input", "document": target, "input": {"type": "ime_commit", "text": "x".repeat(64 * 1024 + 1)}});
    assert_error(&harness.send(oversized)?, "invalid_message");
    Ok(())
}
