#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use serde_json::{Value, json};

fn next_frame(follower: &mut HostControl) -> Value {
    let line = follower
        .read_stream_line(Duration::from_secs(5))
        .expect("the follow stream stays readable")
        .expect("the follow stream stays open");
    serde_json::from_str(&line).expect("a JSON frame")
}

#[cfg(target_os = "linux")]
fn follow_thread_context_switches() -> u64 {
    std::fs::read_dir("/proc/self/task")
        .expect("the task list")
        .flatten()
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|comm| comm.trim() == "paneflow-serve-conn")
        })
        .filter_map(|task| std::fs::read_to_string(task.path().join("status")).ok())
        .flat_map(|status| {
            status
                .lines()
                .filter(|line| line.contains("ctxt_switches:"))
                .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
                .collect::<Vec<_>>()
        })
        .sum()
}

#[test]
fn an_idle_follower_sleeps_until_its_keepalive_and_ends_promptly_on_shutdown() {
    let home = tempfile::tempdir().unwrap();
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut follower =
        HostControl::connect(&endpoint, "worker-follow-test").expect("a Controller connects");
    let header = follower
        .request(METHOD_AGENT_FOLLOW, json!({}))
        .expect("agent.follow");
    assert_eq!(header["following"], true);
    let followed_at = Instant::now();

    #[cfg(target_os = "linux")]
    let switches_before = follow_thread_context_switches();
    let mut previous = followed_at;
    let mut keepalives = 0;
    while keepalives < 2 {
        let frame = next_frame(&mut follower);
        if frame["type"] == "keepalive" {
            let gap = previous.elapsed();
            assert!(
                gap >= Duration::from_millis(1_900) && gap <= Duration::from_millis(2_300),
                "a keepalive leaves at most 2 s after the last frame, took {gap:?}"
            );
            keepalives += 1;
        }
        previous = Instant::now();
    }
    #[cfg(target_os = "linux")]
    {
        let elapsed = followed_at.elapsed().as_secs_f64();
        let per_ten_seconds =
            (follow_thread_context_switches() - switches_before) as f64 * 10.0 / elapsed;
        assert!(
            per_ten_seconds <= 25.0,
            "an idle follower wakes at most 25 times in 10 s, measured {per_ten_seconds:.1}"
        );
    }

    std::thread::sleep(Duration::from_millis(700));
    let stopping = Instant::now();
    worker.worker().request_shutdown();
    let end = loop {
        let frame = next_frame(&mut follower);
        if frame["type"] != "keepalive" {
            break frame;
        }
    };
    let ended_in = stopping.elapsed();
    assert_eq!(end["type"], "end", "{end}");
    assert!(
        ended_in < Duration::from_millis(600),
        "a stopping worker ends its followers within 600 ms, took {ended_in:?}"
    );
    drop(follower);
    worker.stop();
}
