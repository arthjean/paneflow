#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::channel;
use std::time::Duration;

use serde_json::{Value, json};

const ROOT_OWNED_RUNTIME_DIR: &str = "/";

fn directory_is_writable(path: &Path) -> bool {
    tempfile::tempfile_in(path).is_ok()
}

#[test]
fn a_root_owned_xdg_runtime_dir_keeps_ipc_reachable_from_an_external_terminal() {
    if directory_is_writable(Path::new(ROOT_OWNED_RUNTIME_DIR)) {
        return;
    }
    let user_home = tempfile::tempdir().expect("user home");
    let tmpdir = tempfile::tempdir().expect("tmpdir");
    let server_env = paneflow_home::EndpointEnv {
        user_home: Some(user_home.path().to_path_buf()),
        xdg_runtime_dir: Some(ROOT_OWNED_RUNTIME_DIR.into()),
        tmpdir: Some(tmpdir.path().as_os_str().to_owned()),
        ..paneflow_home::EndpointEnv::default()
    };
    let endpoint = paneflow_home::ipc_endpoint_in(&server_env).expect("the GUI resolves a socket");
    assert!(
        endpoint.path.starts_with(tmpdir.path()),
        "{} must fall back to TMPDIR",
        endpoint.path.display()
    );
    assert!(endpoint.owned_parent);
    let parent = endpoint.path.parent().expect("socket parent");
    std::fs::create_dir_all(parent).expect("owned socket parent");
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700)).expect("0700");
    let listener = UnixListener::bind(&endpoint.path).expect("the GUI binds the fallback socket");

    let (seen_tx, seen) = channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).unwrap_or(0) == 0 {
                continue;
            }
            let request: Value = serde_json::from_str(line.trim()).expect("JSON-RPC request");
            let method = request["method"].as_str().unwrap_or_default().to_string();
            let result = match method.as_str() {
                "surface.list" => json!({ "surfaces": [{ "surface_id": 7, "name": "agent" }] }),
                "surface.send_text" => json!({ "surface_id": 7, "bytes": 5 }),
                _ => json!({}),
            };
            let reply = json!({ "jsonrpc": "2.0", "id": request["id"], "result": result });
            let _ = writeln!(stream, "{reply}");
            let _ = seen_tx.send((method, request["params"].clone()));
        }
    });

    let output = Command::new(env!("CARGO_BIN_EXE_paneflow"))
        .args(["send", "7", "hello"])
        .env("HOME", user_home.path())
        .env("XDG_RUNTIME_DIR", ROOT_OWNED_RUNTIME_DIR)
        .env("TMPDIR", tmpdir.path())
        .env_remove(paneflow_home::HOME_ENV)
        .env_remove(paneflow_home::SOCKET_PATH_ENV)
        .env_remove(paneflow_home::ALLOW_SOCKET_OVERRIDE_ENV)
        .env_remove("PANEFLOW_HOST_ENDPOINT")
        .output()
        .expect("the paneflow CLI runs");
    assert!(
        output.status.success(),
        "paneflow send failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut methods = Vec::new();
    while let Ok((method, params)) = seen.recv_timeout(Duration::from_secs(5)) {
        if method == "surface.send_text" {
            assert_eq!(params["text"], "hello");
            methods.push(method);
            break;
        }
        methods.push(method);
    }
    assert_eq!(
        methods.last().map(String::as_str),
        Some("surface.send_text")
    );
}
