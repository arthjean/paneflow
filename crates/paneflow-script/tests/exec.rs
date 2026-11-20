#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;

use paneflow_script::SystemClock;
use support::{FakeBackend, Harness, harness_with, returned};

fn real_time() -> Harness {
    harness_with(FakeBackend::new(), |options| {
        options.clock = Arc::new(SystemClock::new());
    })
}

#[test]
fn a_program_that_does_not_exist_returns_nil_and_a_message() {
    let value = returned(real_time().run(
        "local r, message = paneflow.exec({ 'paneflow-no-such-program-anywhere' }) return { r = r == nil, message = message }",
    ));
    assert_eq!(value["r"], true);
    assert!(
        value["message"]
            .as_str()
            .unwrap()
            .contains("paneflow-no-such-program-anywhere"),
        "{value}"
    );
}

#[cfg(windows)]
#[test]
fn a_non_zero_exit_is_a_result_on_windows() {
    let value = returned(real_time().run(
        "local r = paneflow.exec({ 'cmd', '/C', 'echo out & exit 3' }) return { code = r.code, ok = r.ok, out = r.stdout }",
    ));
    assert_eq!(value["code"], 3);
    assert_eq!(value["ok"], false);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use paneflow_script::Level;
    use serde_json::json;

    fn pid_alive(pid: u64) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    }

    fn eventually_gone(pid: u64) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if !pid_alive(pid) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_non_zero_exit_is_a_result_with_every_field() {
        let value = returned(
            real_time()
                .run("return paneflow.exec({ 'sh', '-c', 'echo out; echo err >&2; exit 3' })"),
        );
        assert_eq!(
            value,
            json!({"code": 3, "stdout": "out\n", "stderr": "err\n", "timed_out": false, "ok": false})
        );
        let value = returned(real_time().run("return paneflow.exec({ 'true' }).ok"));
        assert_eq!(value, true);
    }

    #[test]
    fn options_are_honored_and_an_unknown_one_logs_one_warning() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        let harness = real_time();
        let source = format!(
            "local where = paneflow.exec({{ 'pwd' }}, {{ cwd = '{}' }}) \
             local env = paneflow.exec({{ 'sh', '-c', 'echo ${{PANEFLOW_SCRIPT_A-unset}} ${{HOME-unset}}' }}, {{ env = {{ PANEFLOW_SCRIPT_A = 'set', HOME = false }} }}) \
             local cleared = paneflow.exec({{ '/usr/bin/env' }}, {{ clear_env = true, env = {{ ONLY = '1' }} }}) \
             local fed = paneflow.exec({{ 'cat' }}, {{ stdin = 'from stdin' }}) \
             local quiet = paneflow.exec({{ 'sh', '-c', 'echo hidden; echo shown >&2' }}, {{ stdout = false }}) \
             local raw = paneflow.exec({{ 'printf', 'a\\r\\nb' }}, {{ text = false }}) \
             local text = paneflow.exec({{ 'printf', 'a\\r\\nb' }}) \
             local odd = paneflow.exec({{ 'true' }}, {{ colour = 'red', shell = true }}) \
             return {{ cwd = where.stdout, env = env.stdout, cleared = cleared.stdout, fed = fed.stdout, quiet_out = quiet.stdout == nil, quiet_err = quiet.stderr, raw = raw.stdout, text = text.stdout, odd = odd.ok }}",
            cwd.display()
        );
        let value = returned(harness.run(&source));
        assert_eq!(value["cwd"], format!("{}\n", cwd.display()));
        assert_eq!(value["env"], "set unset\n");
        assert_eq!(value["cleared"], "ONLY=1\n");
        assert_eq!(value["fed"], "from stdin");
        assert_eq!(value["quiet_out"], true);
        assert_eq!(value["quiet_err"], "shown\n");
        assert_eq!(value["raw"], "a\r\nb");
        assert_eq!(value["text"], "a\nb");
        assert_eq!(value["odd"], true);
        let warnings: Vec<String> = harness
            .sink
            .logs()
            .into_iter()
            .filter(|(level, _)| *level == Level::Warn)
            .map(|(_, message)| message)
            .collect();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("colour, shell"), "{warnings:?}");
    }

    #[test]
    fn no_shell_interprets_the_argument_list() {
        let value = returned(
            real_time()
                .run("return paneflow.exec({ 'echo', '$HOME', '*', '; ls', '$(id)' }).stdout"),
        );
        assert_eq!(value, "$HOME * ; ls $(id)\n");
    }

    #[test]
    fn a_timeout_terminates_the_whole_process_tree() {
        let value = returned(real_time().run(
            "local r = paneflow.exec({ 'sh', '-c', 'sleep 3000 & echo $!; wait' }, { timeout = 0.3 }) \
             return { timed_out = r.timed_out, ok = r.ok, grandchild = tonumber(r.stdout:match('%d+')) }",
        ));
        assert_eq!(value["timed_out"], true);
        assert_eq!(value["ok"], false);
        assert!(eventually_gone(value["grandchild"].as_u64().unwrap()));
    }

    #[test]
    fn a_started_command_has_a_handle_and_streams_output_as_it_arrives() {
        let harness = real_time();
        let recorded = harness.recorder();
        let value = returned(harness.run(
            "local h = paneflow.exec.start({ 'cat' }, { stdout = function(chunk) record(chunk) end }) \
             local running = h:running() \
             assert(h:write('one\\n')) \
             paneflow.sleep(0.2) \
             assert(h:write('two\\n')) \
             h:close_stdin() \
             local r = h:wait(20) \
             local sleeper = paneflow.exec.start({ 'sleep', '30' }) \
             local killed = sleeper:kill('TERM') \
             local s = sleeper:wait(20) \
             return { running = running, pid = type(h.pid), code = r.code, after = h:running(), killed = killed, signal = s.signal }",
        ));
        assert_eq!(
            value,
            json!({"running": true, "pid": "number", "code": 0, "after": false, "killed": true, "signal": 15})
        );
        let chunks: String = recorded
            .borrow()
            .iter()
            .map(|chunk| chunk.as_str().unwrap().to_string())
            .collect();
        assert_eq!(chunks, "one\ntwo\n");
        assert!(recorded.borrow().len() >= 2, "{:?}", recorded.borrow());
    }

    #[test]
    fn a_command_is_stopped_when_its_scope_ends_unless_detached() {
        let harness = real_time();
        let recorded = harness.recorder();
        let value = returned(harness.run(
            "local from_timer \
             paneflow.after(0, function() from_timer = paneflow.exec.start({ 'sleep', '30' }) end) \
             paneflow.sleep(0.1) \
             local stopped = from_timer:wait(20) \
             local kept = paneflow.exec.start({ 'sleep', '30' }) \
             local detached = paneflow.exec.start({ 'sleep', '30' }, { detach = true }) \
             record(kept.pid) record(detached.pid) \
             return { timer_signal = stopped.signal }",
        ));
        assert_eq!(value, json!({"timer_signal": 15}));
        let kept = recorded.borrow()[0].as_u64().unwrap();
        let detached = recorded.borrow()[1].as_u64().unwrap();
        assert!(
            eventually_gone(kept),
            "the top-level command outlived its scope"
        );
        assert!(pid_alive(detached), "the detached command was stopped");
        let _ = std::process::Command::new("kill")
            .arg(detached.to_string())
            .status();
    }

    #[test]
    fn a_command_that_ignores_the_termination_signal_is_killed_when_the_script_stops() {
        let harness = real_time();
        let recorded = harness.recorder();
        let outcome = harness.run(
            "local stubborn = paneflow.exec.start({ 'sh', '-c', 'trap \"\" TERM; echo ready; while :; do sleep 0.2; done' }, \
               { stdout = function(chunk) record(stubborn_pid) paneflow.stop(2) end }) \
             stubborn_pid = stubborn.pid \
             paneflow.sleep(30)",
        );
        assert_eq!(outcome, paneflow_script::Outcome::Stopped(2));
        let stubborn = recorded.borrow()[0].as_u64().unwrap();
        assert!(
            eventually_gone(stubborn),
            "a command that ignores SIGTERM outlived the script"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_detached_command_writes_to_no_pipe_the_script_would_close() {
        let harness = real_time();
        let recorded = harness.recorder();
        returned(harness.run(
            "local detached = paneflow.exec.start({ 'sleep', '30' }, { detach = true }) \
             record(detached.pid) return true",
        ));
        let detached = recorded.borrow()[0].as_u64().unwrap();
        for descriptor in [1, 2] {
            let target = std::fs::read_link(format!("/proc/{detached}/fd/{descriptor}")).unwrap();
            assert_eq!(target, std::path::Path::new("/dev/null"));
        }
        let _ = std::process::Command::new("kill")
            .arg(detached.to_string())
            .status();
    }

    #[test]
    fn a_timer_runs_while_a_command_is_waiting_for_output() {
        let harness = real_time();
        let recorded = harness.recorder();
        returned(harness.run(
            "paneflow.after(0.05, function() record('timer') end) \
             paneflow.exec({ 'sleep', '0.5' }) record('exec') return true",
        ));
        assert_eq!(*recorded.borrow(), [json!("timer"), json!("exec")]);
    }
}
