#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
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
fn worker_thread_switches() -> std::collections::BTreeMap<u64, u64> {
    std::fs::read_dir("/proc/self/task")
        .expect("the task list")
        .flatten()
        .filter(|task| {
            std::fs::read_to_string(task.path().join("comm"))
                .is_ok_and(|comm| comm.starts_with("paneflow-serve-"))
        })
        .filter_map(|task| {
            let tid = task.file_name().to_str()?.parse::<u64>().ok()?;
            let status = std::fs::read_to_string(task.path().join("status")).ok()?;
            let switches = status
                .lines()
                .filter(|line| line.contains("ctxt_switches:"))
                .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
                .sum();
            Some((tid, switches))
        })
        .collect()
}

#[test]
fn an_idle_worker_sleeps_until_a_frame_or_deadline_and_ends_promptly_on_shutdown() {
    let home = tempfile::tempdir().unwrap();
    let core_endpoint = paneflow_home::host_endpoint_path(home.path());
    let core = paneflow_host::SessionHost::open(home.path(), &core_endpoint).expect("a core opens");
    let core_server = paneflow_host::ServerHandle::spawn(Arc::clone(&core), core_endpoint)
        .expect("the core serves its endpoint");
    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let connected_by = Instant::now() + Duration::from_secs(5);
    while worker.worker().snapshot_frame()["core_connected"] != true {
        assert!(Instant::now() < connected_by, "the worker follows the core");
        std::thread::sleep(Duration::from_millis(20));
    }
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut follower =
        HostControl::connect(&endpoint, "worker-follow-test").expect("a Controller connects");
    let header = follower
        .request(METHOD_AGENT_FOLLOW, json!({}))
        .expect("agent.follow");
    assert_eq!(header["following"], true);

    #[cfg(target_os = "linux")]
    let followed_at = Instant::now();
    #[cfg(target_os = "linux")]
    let switches_before = worker_thread_switches();
    let mut keepalives = 0;
    while keepalives < 4 {
        if next_frame(&mut follower)["type"] == "keepalive" {
            keepalives += 1;
        }
    }
    #[cfg(target_os = "linux")]
    {
        let elapsed = followed_at.elapsed().as_secs_f64();
        let switches_after = worker_thread_switches();
        let per_thread: Vec<f64> = switches_after
            .iter()
            .map(|(tid, after)| {
                let before = switches_before.get(tid).copied().unwrap_or(0);
                (after - before) as f64 * 10.0 / elapsed
            })
            .collect();
        assert!(
            per_thread.len() >= 4,
            "the accept, core link, pump and connection threads are all counted: {per_thread:?}"
        );
        assert!(
            per_thread
                .iter()
                .all(|per_ten_seconds| *per_ten_seconds <= 10.0),
            "no idle worker thread wakes more than 10 times in 10 s, measured {per_thread:?}"
        );
    }

    let stopping = Instant::now();
    worker.worker().request_shutdown();
    let end = next_frame(&mut follower);
    let ended_in = stopping.elapsed();
    assert_eq!(
        end["type"], "end",
        "a stopping worker ends its follower at once, before the next keepalive is due: {end}"
    );
    assert_eq!(end["reason"], "the worker is stopping", "{end}");
    drop(follower);

    let drain_deadline = paneflow_serve::DRAIN_WAIT.saturating_sub(Duration::from_secs(1));
    let (stopped, stopped_signal) = std::sync::mpsc::channel();
    let stopping = Instant::now();
    let stopper = std::thread::spawn(move || {
        worker.stop();
        let _ = stopped.send(stopping.elapsed());
    });
    let stopped_in = stopped_signal
        .recv_timeout(drain_deadline)
        .unwrap_or_else(|_| {
            panic!(
                "a stopping worker joins its threads without waiting out its {:?} drain",
                paneflow_serve::DRAIN_WAIT
            )
        });
    stopper.join().expect("the stopping thread finishes");
    eprintln!(
        "worker follow: the end frame came {ended_in:?} after shutdown, stop took {stopped_in:?}"
    );
    if let Some(out) = std::env::var_os("PANEFLOW_GATE_WORKER_STOP_OUT") {
        std::fs::write(
            out,
            serde_json::to_vec_pretty(&json!({
                "end_ms": ended_in.as_secs_f64() * 1000.0,
                "stop_ms": stopped_in.as_secs_f64() * 1000.0,
            }))
            .unwrap(),
        )
        .unwrap();
    }
    core_server.stop().unwrap();
    drop(core);
}
