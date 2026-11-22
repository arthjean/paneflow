#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result
)]

mod pane;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const PANE_TEST: &str = "pane_client";
const PATIENCE: Duration = Duration::from_secs(30);

const INIT_LUA: &str = "paneflow.action{ name = 'deploy', title = 'Deploy', agents = true,\n\
   run = function(ctx) return { origin = ctx.origin } end }\n\
 paneflow.action{ name = 'secret', title = 'Secret', run = function() return 'hidden' end }\n\
 paneflow.action{ name = 'slow', title = 'Slow', agents = true, run = function() paneflow.sleep(1) return 'slept' end }\n\
 paneflow.action{ name = 'confirm', title = 'Confirm', agents = true, run = function()\n\
   local answer, reason = paneflow.ui.input{ title = 'Type yes' }\n\
   return answer or reason end }\n";

fn agent_named_copy(dir: &Path) -> PathBuf {
    let me = std::env::current_exe().unwrap();
    #[cfg(windows)]
    {
        let copy = dir.join("claude.exe");
        std::fs::copy(&me, &copy).unwrap();
        copy
    }
    #[cfg(unix)]
    {
        let link = dir.join("claude");
        std::os::unix::fs::symlink(&me, &link).unwrap();
        link
    }
}

fn act_as_caller(home: &Path) {
    let mut control = pane::connect(home, "agent-caller");
    for (label, name) in [("secret", "secret"), ("deploy", "deploy")] {
        let answer = pane::worker_call(
            &mut control,
            "action.invoke",
            json!({"name": name, "ctx": {"origin": "cli"}}),
        );
        pane::report(label, &answer);
    }
    let asked = pane::worker_call(
        &mut control,
        "prompt.request",
        json!({"kind": "input", "title": "Ship it?", "wait_ms": 0, "origin": {"kind": "script", "file": "ship.lua"}}),
    );
    pane::report("prompt", &asked);
    let confirm = pane::worker_call(
        &mut control,
        "action.invoke",
        json!({"name": "confirm", "wait_ms": 1_000, "ctx": {"origin": "cli"}}),
    );
    pane::report("confirm", &confirm);
}

fn run_as_agent(home: &Path, mode: &str) {
    let dir = tempfile::tempdir().unwrap();
    let agent = agent_named_copy(dir.path());
    let status = std::process::Command::new(&agent)
        .args([PANE_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(pane::MODE_ENV, mode)
        .env(pane::HOME_ENV, home)
        .status()
        .unwrap();
    assert!(status.success(), "the agent-named child failed: {status}");
}

fn wait_on_own_run(home: &Path) {
    let start = |control: &mut HostControl| {
        pane::worker_call(
            control,
            "action.invoke",
            json!({"name": "slow", "wait_ms": 50, "ctx": {"origin": "cli"}}),
        )
    };
    let mut control = pane::connect(home, "agent-runs");
    let first = start(&mut control);
    pane::report("started", &first);
    let run_id = first.unwrap()["run_id"].clone();
    let mut again = pane::connect(home, "agent-runs-again");
    let done = pane::worker_call(
        &mut again,
        "action.wait",
        json!({"run_id": run_id, "wait_ms": 25_000}),
    );
    pane::report("done", &done);
    let taken = pane::worker_call(&mut again, "action.wait", json!({"run_id": run_id}));
    pane::report("taken", &taken);
    let left = start(&mut control);
    pane::report("left", &left);
    let asked = pane::worker_call(
        &mut control,
        "prompt.request",
        json!({"kind": "input", "title": "Later?", "wait_ms": 0}),
    );
    pane::report("asked", &asked);
}

#[test]
fn pane_client() {
    let Some((mode, home)) = pane::pane_mode() else {
        return;
    };
    match mode.as_str() {
        "through-agent" => run_as_agent(&home, "caller"),
        "caller" => {
            act_as_caller(&home);
            return;
        }
        "own-runs" => run_as_agent(&home, "agent-runs"),
        "agent-runs" => {
            wait_on_own_run(&home);
            return;
        }
        mode if mode.starts_with("wait:") => {
            let (run_id, prompt_id) = mode["wait:".len()..].split_once(':').unwrap();
            let mut control = pane::connect(&home, "other-pane");
            let run = pane::worker_call(
                &mut control,
                "action.wait",
                json!({"run_id": run_id, "wait_ms": 0}),
            );
            pane::report("run", &run);
            let prompt = pane::worker_call(
                &mut control,
                "prompt.wait",
                json!({"id": prompt_id.parse::<u64>().unwrap(), "wait_ms": 0}),
            );
            pane::report("prompt", &prompt);
        }
        "typed" => act_as_caller(&home),
        _ => panic!("unknown pane client mode {mode}"),
    }
    println!("{}", pane::DONE);
}

fn loaded(home: &Path) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let status = pane::worker_call(
            &mut pane::connect(home, "agent-callers-status"),
            "config.status",
            json!({}),
        )
        .unwrap();
        if status["loaded_at_ms"].is_u64() {
            assert_eq!(status["problems"], json!([]), "{status}");
            return;
        }
        assert!(Instant::now() < deadline, "init.lua never loaded: {status}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn window(home: &Path) -> HostControl {
    let mut follower =
        HostControl::connect(&paneflow_home::serve_endpoint_path(home), "window").unwrap();
    let header = follower
        .request("prompt.follow", json!({"kinds": ["input", "pick"]}))
        .unwrap();
    assert_eq!(header["type"], "following");
    follower
}

fn next_requested(follower: &mut HostControl) -> Value {
    loop {
        let line = follower.read_stream_line(PATIENCE).unwrap().unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "requested" {
            return frame;
        }
    }
}

fn wait_until_ended(follower: &mut HostControl, id: u64) {
    loop {
        let line = follower.read_stream_line(PATIENCE).unwrap().unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "ended" && frame["id"] == id {
            return;
        }
    }
}

fn dismiss(home: &Path, shown: &Value) {
    pane::worker_call(
        &mut pane::connect(home, "window-reply"),
        "prompt.reply",
        json!({"id": shown["id"], "token": shown["token"], "cancel": true}),
    )
    .unwrap();
}

fn instance() -> pane::Instance {
    let instance = pane::Instance::start(
        &json!({"shell_integration": false, "ipc": {"scripting": false, "orchestration": false}}),
        Some(INIT_LUA),
    );
    loaded(instance.home());
    instance
}

#[test]
fn a_process_whose_parent_is_an_agent_acts_as_that_agent_without_any_hook() {
    let instance = instance();
    let mut follower = window(instance.home());
    let lines = instance.run_in_pane(PANE_TEST, "through-agent");
    let secret = pane::labelled(&lines, "secret");
    assert!(
        secret["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no action named 'secret'"),
        "an agent never reaches an unmarked action, whatever origin it claims: {secret}"
    );
    let deploy = pane::labelled(&lines, "deploy");
    assert_eq!(deploy["ok"]["result"]["origin"], "agent", "{deploy}");
    let prompt = pane::labelled(&lines, "prompt");
    assert_eq!(prompt["ok"]["status"], "pending", "{prompt}");
    let shown = next_requested(&mut follower);
    assert_eq!(shown["origin"]["kind"], "agent", "{shown}");
    assert_eq!(shown["origin"]["agent"], "Claude Code", "{shown}");
    dismiss(instance.home(), &shown);
    let confirm = pane::labelled(&lines, "confirm");
    assert_eq!(confirm["ok"]["status"], "running", "{confirm}");
    let asked_by_run = next_requested(&mut follower);
    assert_eq!(asked_by_run["origin"]["kind"], "action", "{asked_by_run}");
    assert_eq!(
        asked_by_run["origin"]["agent"], "Claude Code",
        "the prompt of a run the agent started names it: {asked_by_run}"
    );
    dismiss(instance.home(), &asked_by_run);
}

#[test]
fn a_command_the_user_types_in_the_pane_shell_runs_every_action_as_before() {
    let instance = instance();
    let mut follower = window(instance.home());
    let lines = instance.run_in_pane(PANE_TEST, "typed");
    assert_eq!(
        pane::labelled(&lines, "secret")["ok"]["result"],
        "hidden",
        "{lines:?}"
    );
    let deploy = pane::labelled(&lines, "deploy");
    assert_eq!(deploy["ok"]["result"]["origin"], "cli", "{deploy}");
    let shown = next_requested(&mut follower);
    assert_eq!(shown["origin"]["kind"], "script", "{shown}");
    dismiss(instance.home(), &shown);
    let asked_by_run = next_requested(&mut follower);
    assert_eq!(asked_by_run["origin"]["kind"], "action", "{asked_by_run}");
    assert!(asked_by_run["origin"]["agent"].is_null(), "{asked_by_run}");
    dismiss(instance.home(), &asked_by_run);
}

#[test]
fn only_the_agent_that_started_a_run_or_asked_a_prompt_reads_its_outcome() {
    let instance = instance();
    let mut follower = window(instance.home());
    let lines = instance.run_in_pane(PANE_TEST, "own-runs");
    let started = pane::labelled(&lines, "started");
    assert_eq!(started["ok"]["status"], "running", "{started}");
    assert_eq!(
        pane::labelled(&lines, "done")["ok"],
        json!({"result": "slept", "status": "done"}),
        "the asker resumes its run from a new connection"
    );
    let taken = pane::labelled(&lines, "taken");
    assert!(
        taken["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("no action run"),
        "{taken}"
    );
    let left = pane::labelled(&lines, "left")["ok"]["run_id"]
        .as_str()
        .unwrap()
        .to_string();
    let asked = pane::labelled(&lines, "asked")["ok"]["id"]
        .as_u64()
        .unwrap();
    let shown = next_requested(&mut follower);
    assert_eq!(shown["id"], asked);
    let _cancelled_or_already_ended = pane::worker_call(
        &mut pane::connect(instance.home(), "window-reply"),
        "prompt.reply",
        json!({"id": shown["id"], "token": shown["token"], "cancel": true}),
    );
    wait_until_ended(&mut follower, asked);
    let outside = pane::worker_call(
        &mut pane::connect(instance.home(), "outside"),
        "action.wait",
        json!({"run_id": left, "wait_ms": 0}),
    )
    .unwrap_err();
    assert!(
        outside["message"]
            .as_str()
            .unwrap()
            .starts_with("no action run"),
        "a caller with no pane session is not the asker: {outside}"
    );
    let lines = instance.run_in_pane(PANE_TEST, &format!("wait:{left}:{asked}"));
    let run = pane::labelled(&lines, "run");
    assert!(
        run["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("no action run"),
        "another pane is refused even with the right run_id: {run}"
    );
    let prompt = pane::labelled(&lines, "prompt");
    assert!(
        prompt["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .starts_with("no prompt with id"),
        "another pane cannot read an ended agent prompt: {prompt}"
    );
}
