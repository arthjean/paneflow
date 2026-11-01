#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use paneflow_agent_config::usage::accounts::{UsageAgent, canonical_account_dir};
use paneflow_ipc_client::host_control::{HostControl, METHOD_AGENT_FOLLOW};
use paneflow_serve::protocol::METHOD_USAGE_REFRESH;
use paneflow_serve::usage::probe::refresh_params;
use serde_json::{Value, json};

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

fn iso(at: u64) -> String {
    chrono::DateTime::from_timestamp(i64::try_from(at).unwrap(), 0)
        .unwrap()
        .to_rfc3339()
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("usage-probe")
        .join(name)
}

fn write_lines(path: &Path, lines: &[Value]) -> PathBuf {
    let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(path, text).unwrap();
    path.to_path_buf()
}

#[cfg(unix)]
fn install_fake(bin: &Path, name: &str, replies: &[(&str, PathBuf)]) {
    use std::os::unix::fs::PermissionsExt;
    let record = bin.join(name);
    let mut script = format!(
        "#!/bin/sh\necho call >> '{calls}'\nenv > '{env}'\nprintf '%s\\n' \"$@\" > '{args}'\npwd -P > '{cwd}'\n",
        cwd = record.with_extension("cwd").display(),
        calls = record.with_extension("calls").display(),
        env = record.with_extension("env").display(),
        args = record.with_extension("args").display(),
    );
    script.push_str("while IFS= read -r l; do\n  case \"$l\" in\n");
    for (needle, reply) in replies {
        script.push_str(&format!("    *'{needle}'*) cat '{}' ;;\n", reply.display()));
    }
    script.push_str("  esac\ndone\n");
    let path = bin.join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(windows)]
fn install_fake(bin: &Path, name: &str, replies: &[(&str, PathBuf)]) {
    let record = bin.join(name);
    let mut script = format!(
        "Add-Content -LiteralPath '{calls}' -Value call\n\
         Get-ChildItem env: | ForEach-Object {{ \"$($_.Name)=$($_.Value)\" }} | Set-Content -Encoding UTF8 -LiteralPath '{env}'\n\
         $args | Set-Content -Encoding UTF8 -LiteralPath '{args}'\n\
         (Get-Location).ProviderPath | Set-Content -Encoding UTF8 -LiteralPath '{cwd}'\n",
        cwd = record.with_extension("cwd").display(),
        calls = record.with_extension("calls").display(),
        env = record.with_extension("env").display(),
        args = record.with_extension("args").display(),
    );
    script.push_str("while (($l = [Console]::In.ReadLine()) -ne $null) {\n");
    for (needle, reply) in replies {
        script.push_str(&format!(
            "  if ($l.Contains('{needle}')) {{ [Console]::Out.Write([IO.File]::ReadAllText('{}')); [Console]::Out.Flush() }}\n",
            reply.display()
        ));
    }
    script.push_str("}\n");
    std::fs::write(bin.join(format!("{name}.ps1")), script).unwrap();
    std::fs::write(
        bin.join(format!("{name}.cmd")),
        format!(
            "@powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"%~dp0{name}.ps1\" %*\r\n"
        ),
    )
    .unwrap();
}

fn recorded(bin: &Path, name: &str, kind: &str) -> String {
    std::fs::read_to_string(bin.join(name).with_extension(kind)).unwrap_or_default()
}

fn plan<'a>(usage: &'a Value, agent: &str, dir: &str) -> Option<&'a Value> {
    usage["plans"]
        .as_array()?
        .iter()
        .find(|plan| plan["agent"] == agent && plan["config_dir"] == dir)
}

fn wait_for_usage(follower: &mut HostControl, accept: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        let Ok(Some(line)) = follower.read_stream_line(Duration::from_secs(2)) else {
            continue;
        };
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "usage" && accept(&frame["usage"]) {
            return frame["usage"].clone();
        }
    }
    panic!("no matching usage frame within 60 s");
}

#[test]
fn opening_the_bubble_probes_each_requested_account_through_its_real_binary_once_per_floor() {
    let home = tempfile::tempdir().unwrap();
    let agents = tempfile::tempdir().unwrap();
    let bin = agents.path().join("bin");
    let claude_dir = agents.path().join("claude-work");
    let codex_dir = agents.path().join("codex-work");
    for dir in [&bin, &claude_dir, &codex_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    let now = now_secs();
    let claude_usage = write_lines(
        &agents.path().join("claude-usage.jsonl"),
        &[json!({"type": "control_response", "response": {
            "subtype": "success",
            "request_id": "paneflow-get-usage",
            "response": {
                "session": {"total_cost_usd": 0, "model_usage": {}},
                "subscription_type": "max",
                "rate_limits_available": true,
                "rate_limits": {
                    "five_hour": {"utilization": 12, "resets_at": iso(now + 2 * 3600)},
                    "seven_day": {"utilization": 67, "resets_at": iso(now + 3 * 86_400)},
                    "seven_day_opus": null
                },
                "behaviors": null
            }
        }})],
    );
    let codex_limits = write_lines(
        &agents.path().join("codex-limits.jsonl"),
        &[
            json!({"method": "account/updated", "params": {"authMode": "chatgpt", "planType": "free"}}),
            json!({"id": 2, "result": {"rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 3, "windowDurationMins": 43_200, "resetsAt": now + 20 * 86_400},
                "secondary": null,
                "planType": "free"
            }}}),
        ],
    );
    install_fake(
        &bin,
        "claude",
        &[
            ("\"initialize\"", fixture("claude-2.1.316-initialize.jsonl")),
            ("\"get_usage\"", claude_usage),
        ],
    );
    install_fake(
        &bin,
        "codex",
        &[
            ("\"initialize\"", fixture("codex-0.166.0-initialize.jsonl")),
            ("account/rateLimits/read", codex_limits),
        ],
    );
    let path = std::env::var_os("PATH").unwrap_or_default();
    let searched =
        std::env::join_paths(std::iter::once(bin.clone()).chain(std::env::split_paths(&path)))
            .unwrap();
    unsafe {
        std::env::set_var(paneflow_home::HOME_ENV, home.path());
        std::env::set_var("PATH", searched);
    }

    let worker = paneflow_serve::open(home.path()).expect("the worker takes the home");
    let endpoint = paneflow_home::serve_endpoint_path(home.path());
    let mut follower = HostControl::connect(&endpoint, "worker-usage-probe-follower").unwrap();
    follower.request(METHOD_AGENT_FOLLOW, json!({})).unwrap();
    let mut control = HostControl::connect(&endpoint, "worker-usage-probe-control").unwrap();
    let claude = canonical_account_dir(&claude_dir);
    let codex = canonical_account_dir(&codex_dir);
    let params = refresh_params([
        (UsageAgent::ClaudeCode, claude.as_path()),
        (UsageAgent::Codex, codex.as_path()),
    ]);

    let asked = Instant::now();
    control
        .request(METHOD_USAGE_REFRESH, params.clone())
        .unwrap();
    assert!(
        asked.elapsed() < Duration::from_secs(5),
        "the refresh answers before the probes finish"
    );
    let claude_key = claude.display().to_string();
    let codex_key = codex.display().to_string();
    let usage = wait_for_usage(&mut follower, |usage| {
        plan(usage, "claude-code", &claude_key).is_some_and(|plan| plan["primary"].is_object())
            && plan(usage, "codex", &codex_key).is_some_and(|plan| plan["primary"].is_object())
    });
    let claude_plan = plan(&usage, "claude-code", &claude_key).unwrap();
    assert_eq!(claude_plan["state"], "reported");
    assert_eq!(claude_plan["primary"]["used_percent"], 12.0);
    assert_eq!(claude_plan["secondary"]["used_percent"], 67.0);
    assert_eq!(claude_plan["secondary"]["window_secs"], 7 * 86_400);
    assert_eq!(claude_plan["agent_version"], "2.1.316");
    let codex_plan = plan(&usage, "codex", &codex_key).unwrap();
    assert_eq!(codex_plan["state"], "reported");
    assert_eq!(codex_plan["primary"]["used_percent"], 3.0);
    assert_eq!(codex_plan["primary"]["window_secs"], 43_200 * 60);
    assert_eq!(codex_plan["agent_version"], "0.166.0");

    let claude_env = recorded(&bin, "claude", "env");
    assert!(
        !claude_env
            .lines()
            .any(|line| line.trim_start_matches('\u{feff}').starts_with("PANEFLOW_")),
        "{claude_env}"
    );
    assert!(
        claude_env
            .lines()
            .any(|line| line == format!("CLAUDE_CONFIG_DIR={claude_key}")),
        "{claude_env}"
    );
    assert!(claude_env.contains("ENABLE_CLAUDEAI_MCP_SERVERS=false"));
    let claude_args = recorded(&bin, "claude", "args");
    for expected in [
        "-p",
        "stream-json",
        "--no-session-persistence",
        "--strict-mcp-config",
    ] {
        assert!(
            claude_args
                .lines()
                .any(|line| line.trim_start_matches('\u{feff}') == expected),
            "{expected} in {claude_args}"
        );
    }
    assert!(claude_args.contains("disableAllHooks"), "{claude_args}");
    let codex_env = recorded(&bin, "codex", "env");
    assert!(
        codex_env
            .lines()
            .any(|line| line == format!("CODEX_HOME={codex_key}")),
        "{codex_env}"
    );
    assert!(
        recorded(&bin, "codex", "args")
            .lines()
            .any(|line| line.trim_start_matches('\u{feff}') == "app-server")
    );

    control.request(METHOD_USAGE_REFRESH, params).unwrap();
    std::thread::sleep(Duration::from_millis(1_500));
    assert_eq!(recorded(&bin, "claude", "calls").lines().count(), 1);
    assert_eq!(recorded(&bin, "codex", "calls").lines().count(), 1);
    let private = canonical_account_dir(&paneflow_home::usage_dir_in(home.path()).join("probe"));
    for name in ["claude", "codex"] {
        let cwd = recorded(&bin, name, "cwd");
        assert_eq!(
            canonical_account_dir(Path::new(cwd.trim().trim_start_matches('\u{feff}'))),
            private,
            "{name} runs from the private probe directory"
        );
    }

    drop(follower);
    drop(control);
    worker.stop();
}
