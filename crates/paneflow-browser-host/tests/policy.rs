#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{DEADLINE, FixtureServer, Harness, lan_address};
use paneflow_browser_client::{ClientEvent, TabStatus};
use paneflow_browser_proto::{
    CommandError, Event, NavigationRefusal, OriginDecision, Reply, TabId,
};
use serde_json::json;

fn refused_with(error: Result<Reply, CommandError>) -> NavigationRefusal {
    match error {
        Err(CommandError::NavigationRefused { refusal }) => refusal,
        other => panic!("expected a refused navigation, got {other:?}"),
    }
}

#[test]
fn loopback_pages_load_without_a_decision() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("loopback", &server.url("/app.html"));
    assert_eq!(harness.wait_loaded(tab, "/app.html"), 200);
    let localhost = format!("http://localhost:{}/form.html", server.port);
    assert_eq!(
        harness.client.navigate(tab, &localhost).wait(DEADLINE),
        Ok(Reply::Done)
    );
    assert_eq!(harness.wait_loaded(tab, "localhost"), 200);
    assert!(!harness.seen.iter().any(|event| matches!(
        event,
        ClientEvent::Browser(Event::OriginDecisionRequested { .. })
    )));
    harness.shutdown();
}

#[test]
fn engine_and_local_schemes_are_refused_naming_the_scheme() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("schemes", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");
    for (url, scheme) in [
        ("file:///etc/passwd", "file"),
        ("chrome://version", "chrome"),
        ("devtools://devtools/bundled/inspector.html", "devtools"),
        ("view-source:http://127.0.0.1/", "view-source"),
        ("javascript:alert(1)", "javascript"),
        ("data:text/html,<h1>top</h1>", "data"),
    ] {
        let refusal = refused_with(harness.client.navigate(tab, url).wait(DEADLINE));
        assert_eq!(
            refusal,
            NavigationRefusal::Scheme {
                scheme: scheme.to_string()
            }
        );
        assert!(refusal.to_string().contains(&format!("{scheme}:")));
        let (_, opened) = harness.client.open_tab(Harness::profile("schemes"), url);
        assert!(matches!(
            opened.wait(DEADLINE),
            Err(CommandError::NavigationRefused {
                refusal: NavigationRefusal::Scheme { .. }
            })
        ));
    }

    harness.evaluate(
        tab,
        "setTimeout(() => { location.href = 'file:///etc/hostname' }, 0); 1",
    );
    harness.wait_for("the page's own file: navigation to be refused", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::NavigationRefused { tab: t, refusal: NavigationRefusal::Scheme { scheme }, .. })
                if *t == tab && scheme == "file"
        ) || matches!(
            event,
            ClientEvent::Browser(Event::Console { tab: t, text, .. })
                if *t == tab && text.contains("Not allowed to load local resource: file:///etc/hostname")
        )
    });
    assert_eq!(
        harness.evaluate(tab, "location.href"),
        json!(server.url("/form.html"))
    );
    harness.shutdown();
}

#[test]
fn a_url_that_embeds_credentials_is_refused() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("credentials", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");
    let url = format!("http://user:secret@127.0.0.1:{}/app.html", server.port);
    assert_eq!(
        refused_with(harness.client.navigate(tab, &url).wait(DEADLINE)),
        NavigationRefusal::Credentials
    );
    let (_, opened) = harness
        .client
        .open_tab(Harness::profile("credentials"), &url);
    assert_eq!(
        refused_with(opened.wait(DEADLINE)),
        NavigationRefusal::Credentials
    );
    harness.shutdown();
}

fn decision_requested(harness: &mut Harness, tab: TabId, origin: &str) {
    harness.wait_for(&format!("a decision request for {origin}"), |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::OriginDecisionRequested { tab: t, origin: o, .. })
                if *t == tab && o == origin
        )
    });
}

fn placeholder_event(event: &ClientEvent, tab: TabId) -> bool {
    match event {
        ClientEvent::Browser(
            Event::LoadStarted { tab: t, url }
            | Event::Loaded { tab: t, url, .. }
            | Event::AddressChanged { tab: t, url },
        ) => *t == tab && url.starts_with("about:"),
        ClientEvent::Browser(Event::TitleChanged { tab: t, title }) => {
            *t == tab && title.starts_with("about:")
        }
        ClientEvent::Browser(Event::LoadingState { tab: t, .. }) => *t == tab,
        _ => false,
    }
}

#[test]
fn a_tab_opened_on_a_public_origin_reports_only_the_held_site_until_it_is_decided() {
    let Some(lan) = lan_address() else {
        panic!("this test needs a non-loopback interface address");
    };
    let server = FixtureServer::start_on_all_interfaces();
    let mut harness = Harness::new();
    let external = server.url_on(lan, "/app.html");
    let origin = format!("http://{lan}:{}", server.port);
    let tab = harness.open("held-at-open", &external);
    decision_requested(&mut harness, tab, &origin);
    assert!(matches!(
        harness.client.tab_status(tab),
        Some(TabStatus::AwaitingDecision { origin: o, .. }) if o == origin
    ));
    harness.evaluate(tab, "document.readyState");
    harness.seen.extend(harness.events.try_iter());
    let before_decision: Vec<&ClientEvent> = harness
        .seen
        .iter()
        .filter(|event| placeholder_event(event, tab))
        .collect();
    assert!(
        before_decision.is_empty(),
        "the blank page the tab starts on was reported: {before_decision:#?}"
    );
    assert_eq!(
        harness
            .client
            .decide_origin(tab, &origin, OriginDecision::AllowOnce)
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    assert_eq!(harness.wait_loaded(tab, &origin), 200);
    assert_eq!(
        harness.evaluate(tab, "history.length"),
        json!(1),
        "Back must not lead to the blank page the tab started on"
    );
    harness.shutdown();
}

#[test]
fn a_tab_a_person_drives_opens_a_public_origin_and_frames_it_without_a_decision() {
    let Some(lan) = lan_address() else {
        panic!("this test needs a non-loopback interface address");
    };
    let server = FixtureServer::start_on_all_interfaces();
    let mut harness = Harness::new();
    let external = server.url_on(lan, "/app.html");
    let origin = format!("http://{lan}:{}", server.port);
    let (tab, opened) =
        harness
            .client
            .open_tab_guarded(Harness::profile("unguarded"), &external, false);
    assert_eq!(opened.wait(DEADLINE), Ok(Reply::Done));
    assert_eq!(harness.wait_loaded(tab, &origin), 200);
    let framed = harness.evaluate(
        tab,
        &format!(
            "new Promise(done => {{ const frame = document.createElement('iframe'); \
             frame.onload = () => done(frame.contentWindow.location.href); \
             frame.src = '{}'; document.body.appendChild(frame); }})",
            server.url_on(lan, "/form.html")
        ),
    );
    assert_eq!(framed, json!(server.url_on(lan, "/form.html")));

    assert_eq!(
        harness.client.guard(tab, true).wait(DEADLINE),
        Ok(Reply::Done)
    );
    let local = server.url("/form.html");
    assert_eq!(
        harness.client.navigate(tab, &local).wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_loaded(tab, "/form.html");
    let held = server.url_on(lan, "/app.html?held");
    assert_eq!(
        harness.client.navigate(tab, &held).wait(DEADLINE),
        Ok(Reply::Done)
    );
    decision_requested(&mut harness, tab, &origin);
    assert_eq!(
        harness.client.guard(tab, false).wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_for("the held site dropped, never loaded", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::NavigationRefused {
                tab: t,
                refusal: NavigationRefusal::OriginUndecided { origin: o },
                ..
            }) if *t == tab && *o == origin
        )
    });
    assert_eq!(harness.evaluate(tab, "location.href"), json!(local));
    harness.shutdown();
}

#[test]
fn a_public_origin_is_held_until_a_decision_and_each_decision_applies() {
    let Some(lan) = lan_address() else {
        panic!("this test needs a non-loopback interface address");
    };
    let server = FixtureServer::start_on_all_interfaces();
    let mut harness = Harness::new();
    let tab = harness.open("decisions", &server.url("/form.html"));
    harness.wait_loaded(tab, "/form.html");
    let external = server.url_on(lan, "/app.html");
    let origin = format!("http://{lan}:{}", server.port);

    assert_eq!(
        harness.client.navigate(tab, &external).wait(DEADLINE),
        Ok(Reply::Done)
    );
    decision_requested(&mut harness, tab, &origin);
    assert!(matches!(
        harness.client.tab_status(tab),
        Some(TabStatus::AwaitingDecision { origin: o, .. }) if o == origin
    ));
    assert_eq!(
        harness.evaluate(tab, "location.href"),
        json!(server.url("/form.html"))
    );

    assert_eq!(
        harness
            .client
            .navigate(tab, server.url("/form.html?moved-on"))
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_loaded(tab, "moved-on");
    assert_eq!(
        harness
            .client
            .decide_origin(tab, &origin, OriginDecision::AllowOnce)
            .wait(DEADLINE),
        Err(CommandError::NoPendingDecision {
            tab,
            origin: origin.clone()
        })
    );
    assert_eq!(
        harness.client.navigate(tab, &external).wait(DEADLINE),
        Ok(Reply::Done)
    );
    decision_requested(&mut harness, tab, &origin);

    assert_eq!(
        harness
            .client
            .decide_origin(tab, &origin, OriginDecision::Deny)
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_for("the denial", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::NavigationRefused { refusal: NavigationRefusal::OriginDenied { origin: o }, .. })
                if *o == origin
        )
    });
    assert_eq!(
        harness.evaluate(tab, "location.href"),
        json!(server.url("/form.html?moved-on"))
    );

    assert_eq!(
        harness.client.navigate(tab, &external).wait(DEADLINE),
        Ok(Reply::Done)
    );
    decision_requested(&mut harness, tab, &origin);
    assert_eq!(
        harness
            .client
            .decide_origin(tab, &origin, OriginDecision::AllowOnce)
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    assert_eq!(harness.wait_loaded(tab, &origin), 200);

    harness.evaluate(
        tab,
        &format!(
            "setTimeout(() => {{ location.href = '{}' }}, 0); 1",
            server.url("/form.html")
        ),
    );
    harness.wait_loaded(tab, "127.0.0.1");
    harness.evaluate(
        tab,
        &format!("setTimeout(() => {{ location.href = '{external}' }}, 0); 1"),
    );
    decision_requested(&mut harness, tab, &origin);
    assert_eq!(
        harness
            .client
            .decide_origin(tab, &origin, OriginDecision::AllowAlways)
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    assert_eq!(harness.wait_loaded(tab, &origin), 200);

    assert_eq!(
        harness
            .client
            .navigate(tab, server.url("/form.html"))
            .wait(DEADLINE),
        Ok(Reply::Done)
    );
    harness.wait_loaded(tab, "127.0.0.1");
    assert_eq!(
        harness.client.navigate(tab, &external).wait(DEADLINE),
        Ok(Reply::Done)
    );
    assert_eq!(harness.wait_loaded(tab, &origin), 200);

    assert_eq!(
        harness
            .client
            .decide_origin(
                tab,
                "https://never-asked.example",
                OriginDecision::AllowAlways
            )
            .wait(DEADLINE),
        Err(CommandError::NoPendingDecision {
            tab,
            origin: "https://never-asked.example".to_string()
        })
    );
    harness.shutdown();
}

#[test]
fn camera_microphone_geolocation_and_notifications_are_denied() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("permissions", &server.url("/test/permissions"));
    harness.wait_loaded(tab, "/test/permissions");
    let results = harness.evaluate_until(tab, "JSON.stringify(window.results)", |value| {
        let text = value.as_str().unwrap_or_default();
        ["geolocation", "notifications", "media"]
            .iter()
            .all(|key| text.contains(key))
    });
    let results: serde_json::Value = serde_json::from_str(results.as_str().unwrap()).unwrap();
    assert!(
        results["geolocation"]
            .as_str()
            .unwrap()
            .starts_with("denied"),
        "{results}"
    );
    assert_eq!(results["notifications"], "denied", "{results}");
    assert!(
        results["media"].as_str().unwrap().starts_with("denied"),
        "{results}"
    );

    let policy = harness.evaluate_until(
        tab,
        "Promise.all(['geolocation', 'notifications', 'camera', 'microphone'].map(name => navigator.permissions.query({name}).then(status => status.state)))",
        |value| value.is_array(),
    );
    assert_eq!(
        policy,
        serde_json::json!(["denied", "denied", "denied", "denied"]),
        "the profile does not block every site permission by policy"
    );
    harness.shutdown();
}

#[test]
fn a_download_started_by_a_page_is_cancelled_and_reported() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("downloads", &server.url("/test/download"));
    harness.wait_loaded(tab, "/test/download");
    harness.evaluate(tab, "document.getElementById('get').click(); 1");
    harness.wait_for("the cancelled download", |event| {
        matches!(
            event,
            ClientEvent::Browser(Event::DownloadCancelled { tab: t, url })
                if *t == tab && url.ends_with("/test/file.bin")
        )
    });
    let user_home = harness.home.as_ref().unwrap().path().join("user");
    let downloaded: Vec<_> = walk(&user_home)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("file"))
        })
        .collect();
    assert!(downloaded.is_empty(), "{downloaded:?}");
    harness.shutdown();
}

fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn addresses_past_2048_characters_load_in_frames_pages_and_commands() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    let tab = harness.open("long-urls", &server.url("/app.html"));
    assert_eq!(harness.wait_loaded(tab, "/app.html"), 200);
    let long = server.url(&format!("/test/popup-target?q={}", "a".repeat(2300)));
    harness.evaluate(
        tab,
        &format!(
            "(() => {{ const f = document.createElement('iframe'); f.id = 'long'; f.src = {}; document.body.appendChild(f); return true; }})()",
            json!(long)
        ),
    );
    harness.evaluate_until(
        tab,
        "(() => { try { return document.getElementById('long').contentDocument.title; } catch (e) { return String(e); } })()",
        |title| title == "Popup target",
    );
    harness.evaluate(tab, &format!("location.href = {}", json!(long)));
    assert_eq!(harness.wait_loaded(tab, "popup-target"), 200);
    let commanded = server.url(&format!("/test/popup-target?r={}", "b".repeat(5000)));
    assert_eq!(
        harness.client.navigate(tab, &commanded).wait(DEADLINE),
        Ok(Reply::Done)
    );
    assert_eq!(harness.wait_loaded(tab, "r=bbbb"), 200);
    assert!(
        !harness
            .seen
            .iter()
            .any(|event| matches!(event, ClientEvent::Browser(Event::NavigationRefused { .. }))),
        "{:#?}",
        harness.seen
    );
    harness.shutdown();
}
