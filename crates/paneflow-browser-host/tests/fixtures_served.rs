#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{FIXTURES, FixtureServer, Harness, fixtures_dir, http_get};

#[test]
fn the_tracked_fixtures_are_the_four_of_the_prd() {
    for fixture in FIXTURES {
        assert!(fixtures_dir().join(fixture).is_file(), "{fixture}");
    }
    let app = std::fs::read_to_string(fixtures_dir().join("app.html")).unwrap();
    assert!(app.contains("/api/prices"));
    assert!(app.contains("#grand-total"));
    let form = std::fs::read_to_string(fixtures_dir().join("form.html")).unwrap();
    for control in [
        "<select",
        "type=\"password\"",
        "type=\"file\"",
        "type=\"submit\"",
    ] {
        assert!(form.contains(control), "{control}");
    }
    let video = std::fs::read_to_string(fixtures_dir().join("video.html")).unwrap();
    assert!(video.contains("<video") && fixtures_dir().join("clip.webm").is_file());
    let injection = std::fs::read_to_string(fixtures_dir().join("injection.html")).unwrap();
    assert!(injection.contains("https://example.com/"));
}

#[test]
fn a_loopback_server_serves_each_fixture_and_the_planted_failure() {
    let server = FixtureServer::start();
    for fixture in FIXTURES {
        let (status, body) = http_get(server.port, &format!("/{fixture}"));
        assert_eq!(status, 200, "{fixture}");
        assert_eq!(body, std::fs::read(fixtures_dir().join(fixture)).unwrap());
    }
    assert_eq!(http_get(server.port, "/clip.webm").0, 200);
    assert_eq!(http_get(server.port, "/api/prices").0, 500);
    assert_eq!(http_get(server.port, "/api/cart").0, 200);
}

#[test]
fn the_host_loads_each_fixture_from_the_loopback_server() {
    let server = FixtureServer::start();
    let mut harness = Harness::new();
    for fixture in FIXTURES {
        let tab = harness.open("fixtures", &server.url(&format!("/{fixture}")));
        assert_eq!(harness.wait_loaded(tab, fixture), 200, "{fixture}");
    }
    harness.shutdown();
}
