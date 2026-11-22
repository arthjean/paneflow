#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::Duration;

use paneflow_api::Owner;
use paneflow_ipc_client::host_control::HostControl;
use paneflow_ipc_client::line_wire::{LineRead, Wire};
use paneflow_serve::protocol::{
    ERR_HANDSHAKE_REQUIRED, ERR_INVALID_PARAMS, ERR_METHOD_NOT_FOUND, MAX_CONTROL_FRAME_BYTES,
};
use serde_json::{Value, json};

fn read_frame(wire: &mut Wire) -> Option<Value> {
    match wire.read_line(Duration::from_secs(5)).expect("readable") {
        LineRead::Line(line) => Some(serde_json::from_str(&line).expect("a JSON frame")),
        LineRead::Eof => None,
        LineRead::Idle => panic!("no frame within the deadline"),
        LineRead::TooLong => panic!("an oversized frame"),
    }
}

fn raw_request(wire: &mut Wire, id: u64, method: &str, params: Value) -> Value {
    wire.write_json(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
        .expect("the request is written");
    read_frame(wire).expect("the worker answers")
}

#[test]
fn a_connection_that_skips_the_hello_is_refused_and_closed() {
    let home = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut wire = Wire::connect(&endpoint, MAX_CONTROL_FRAME_BYTES).expect("connects");
    let refused = raw_request(&mut wire, 1, "worker.status", json!({}));
    assert_eq!(refused["error"]["code"], ERR_HANDSHAKE_REQUIRED);
    assert!(read_frame(&mut wire).is_none(), "the connection is closed");
    worker.stop();
}

#[test]
fn system_describe_on_the_worker_lists_exactly_the_worker_methods() {
    let home = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut control = HostControl::connect(&endpoint, "worker-registry-test").expect("connects");

    let described = control
        .request("system.describe", json!({}))
        .expect("the worker describes its methods");
    let mut listed: Vec<String> = described["methods"]
        .as_array()
        .expect("methods")
        .iter()
        .map(|method| {
            assert_eq!(method["owner"], "worker", "{method}");
            assert_eq!(method["callable"], true, "{method}");
            assert!(method["params"].is_object() && method["result"].is_object());
            method["name"].as_str().unwrap().to_owned()
        })
        .collect();
    let catalog = paneflow_api::catalog().unwrap();
    let mut expected: Vec<String> = catalog
        .owned_by(Owner::Worker)
        .map(|descriptor| descriptor.name().to_owned())
        .collect();
    listed.sort();
    expected.sort();
    assert_eq!(listed, expected);
    assert!(expected.contains(&"events.follow".to_owned()));

    let window_method = control
        .request("system.describe", json!({"method": "surface.read"}))
        .expect("any declared method is described");
    assert_eq!(window_method["callable"], false);
    worker.stop();
}

#[test]
fn a_mistyped_parameter_and_an_unknown_method_are_refused_with_their_codes() {
    let home = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut wire = Wire::connect(&endpoint, MAX_CONTROL_FRAME_BYTES).expect("connects");
    let hello = raw_request(&mut wire, 1, "worker.hello", json!({}));
    assert!(hello["result"]["pid"].as_u64().is_some());

    let mistyped = raw_request(&mut wire, 2, "agent.activity_log", json!({"limit": "ten"}));
    assert_eq!(mistyped["error"]["code"], ERR_INVALID_PARAMS);
    assert!(
        mistyped["error"]["message"]
            .as_str()
            .unwrap()
            .contains("limit"),
        "{mistyped}"
    );

    let unknown = raw_request(&mut wire, 3, "worker.teleport", json!({}));
    assert_eq!(unknown["error"]["code"], ERR_METHOD_NOT_FOUND);
    assert_eq!(
        unknown["error"]["message"],
        "method not handled by the worker: worker.teleport"
    );

    let still_open = raw_request(&mut wire, 4, "worker.status", json!({}));
    assert!(still_open["result"]["pid"].as_u64().is_some());
    worker.stop();
}

#[test]
fn a_caller_outside_every_pane_is_external_until_it_attaches_as_the_window() {
    let home = tempfile::tempdir().unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a host opens");
    let core_server =
        paneflow_host::ServerHandle::spawn(std::sync::Arc::clone(&core), core_endpoint)
            .expect("the host serves");
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());

    let mut other = HostControl::connect(&endpoint, "worker-identity-test").expect("connects");
    other
        .request("layout.attach", json!({"client": "dashboard"}))
        .expect("attaches");
    assert_eq!(
        other.request("system.whoami", json!({})).unwrap(),
        json!({"principal": "external"})
    );

    let mut window = HostControl::connect(&endpoint, "worker-identity-test").expect("connects");
    assert_eq!(
        window.request("system.whoami", json!({})).unwrap(),
        json!({"principal": "external"})
    );
    window
        .request("layout.attach", json!({"client": "window"}))
        .expect("attaches");
    assert_eq!(
        window.request("system.whoami", json!({})).unwrap(),
        json!({"principal": "window"})
    );
    worker.stop();
    core_server.stop().unwrap();
}
