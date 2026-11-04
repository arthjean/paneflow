#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use portable_pty::PtySize;

const CONCURRENT_SPAWNS: usize = 200;

const SIZE: PtySize = PtySize {
    rows: 24,
    cols: 80,
    pixel_width: 0,
    pixel_height: 0,
};

fn spawn_started_child() -> Child {
    let mut child = Command::new("sh")
        .args(["-c", "echo ready; exec sleep 600"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("sh starts");
    let mut line = String::new();
    BufReader::new(child.stdout.take().expect("stdout is piped"))
        .read_line(&mut line)
        .expect("the child reports that it started");
    assert_eq!(line, "ready\n");
    child
}

fn pty_descriptors(child: &Child) -> BTreeSet<PathBuf> {
    std::fs::read_dir(format!("/proc/{}/fd", child.id()))
        .expect("the child's descriptor table is readable")
        .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
        .filter(|target| target.starts_with("/dev/pts") || target == Path::new("/dev/ptmx"))
        .collect()
}

fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn a_process_spawned_while_ptys_open_inherits_neither_end() {
    drop(paneflow_host::pty::open(SIZE).expect("a PTY opens"));
    let control = spawn_started_child();
    let inherited_before = pty_descriptors(&control);
    stop(control);

    let opening = Arc::new(AtomicBool::new(true));
    let opener = {
        let opening = Arc::clone(&opening);
        std::thread::spawn(move || {
            let mut opened = 0_usize;
            while opening.load(Ordering::Relaxed) {
                drop(paneflow_host::pty::open(SIZE).expect("a PTY opens"));
                opened += 1;
            }
            opened
        })
    };
    let mut leaks = Vec::new();
    for _ in 0..CONCURRENT_SPAWNS {
        let child = spawn_started_child();
        let leaked: BTreeSet<PathBuf> = pty_descriptors(&child)
            .difference(&inherited_before)
            .cloned()
            .collect();
        stop(child);
        if !leaked.is_empty() {
            leaks.push(leaked);
        }
    }
    opening.store(false, Ordering::Relaxed);
    let opened = opener.join().expect("the opener thread finishes");

    assert!(opened > 0, "no PTY opened while the children were spawned");
    assert!(
        leaks.is_empty(),
        "{} of {CONCURRENT_SPAWNS} children spawned while {opened} PTYs opened inherited one: {leaks:?}",
        leaks.len()
    );
}
