#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::manifest::{manifest_path, read_manifest};
use paneflow_host::{CreateSession, SessionGeneration, SessionHost, SessionId, SessionManifest};
use paneflow_program_resume::{CwdSource, Record};

const OUTCOME_DEADLINE: Duration = Duration::from_secs(30);

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_paneflow-session-fixture"))
}

struct Host {
    home: tempfile::TempDir,
    host: Arc<SessionHost>,
}

impl Host {
    fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let endpoint = home.path().join("host.sock");
        let host = SessionHost::open(home.path(), &endpoint).unwrap();
        Self { home, host }
    }

    fn create(&self, shell: &str, args: Vec<String>, cwd: &Path) -> SessionId {
        self.host
            .create(CreateSession {
                shell: Some(shell.to_string()),
                args,
                cwd: Some(cwd.display().to_string()),
                cols: Some(200),
                rows: Some(50),
                ..CreateSession::default()
            })
            .unwrap()
            .manifest
            .session
    }

    fn shell(&self) -> SessionId {
        self.create("/bin/sh", Vec::new(), &std::env::temp_dir())
    }

    fn fixture_pane(&self, steps: &[&str]) -> SessionId {
        let mut args = vec!["resume-report".to_string()];
        args.extend(steps.iter().map(|step| (*step).to_string()));
        self.create(
            &fixture().display().to_string(),
            args,
            &std::env::temp_dir(),
        )
    }

    fn type_line(&self, session: &SessionId, line: &str) {
        self.host
            .input(session, None, format!("{line}\n").into_bytes())
            .unwrap();
    }

    fn run_fixture(&self, session: &SessionId, steps: &[&str]) {
        self.run_fixture_in(session, None, steps);
    }

    fn run_fixture_in(&self, session: &SessionId, directory: Option<&Path>, steps: &[&str]) {
        let quoted: Vec<String> = steps.iter().map(|step| format!("'{step}'")).collect();
        let enter = directory.map_or_else(String::new, |directory| {
            format!("cd '{}' && ", directory.display())
        });
        self.type_line(
            session,
            &format!(
                "{enter}'{}' resume-report {}",
                fixture().display(),
                quoted.join(" ")
            ),
        );
    }

    fn screen(&self, session: &SessionId) -> String {
        self.host.text(session).unwrap().text
    }

    fn screen_until(&self, session: &SessionId, marker: &str) -> String {
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let text = self.screen(session);
            if text.contains(marker) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "{marker} never appeared in the pane: {text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn records(&self, session: &SessionId) -> Vec<Record> {
        self.host
            .inspect(session)
            .unwrap()
            .manifest
            .program_resume
            .records()
            .to_vec()
    }

    fn records_until(
        &self,
        session: &SessionId,
        mut done: impl FnMut(&[Record]) -> bool,
    ) -> Vec<Record> {
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let records = self.records(session);
            if done(&records) {
                return records;
            }
            assert!(
                Instant::now() < deadline,
                "the records never reached the expected state: {records:?}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn on_disk(&self, session: &SessionId) -> SessionManifest {
        read_manifest(&manifest_path(self.home.path(), session)).unwrap()
    }

    fn on_disk_until(
        &self,
        session: &SessionId,
        mut done: impl FnMut(&SessionManifest) -> bool,
    ) -> SessionManifest {
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let manifest = self.on_disk(session);
            if done(&manifest) {
                return manifest;
            }
            assert!(
                Instant::now() < deadline,
                "the manifest on disk never reached the expected state: {:?}",
                manifest.program_resume
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn pane_token(&self, session: &SessionId) -> String {
        self.type_line(session, "printf 'tok<%s>\\n' \"$OSC7577_TOKEN\"");
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let text = self.screen(session);
            if let Some(token) = text
                .lines()
                .filter_map(|line| line.trim().strip_prefix("tok<"))
                .filter_map(|rest| rest.strip_suffix('>'))
                .next_back()
            {
                return token.to_string();
            }
            assert!(Instant::now() < deadline, "no token line: {text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.host.request_shutdown(true);
    }
}

fn argv(record: &Record) -> Vec<&str> {
    record.argv.iter().map(String::as_str).collect()
}

fn step_marker(index: usize, name: &str) -> String {
    format!("fixture step {index} {name}")
}

fn fixture_pid(screen: &str) -> u32 {
    screen
        .lines()
        .find_map(|line| line.split("pid=").nth(1))
        .and_then(|pid| pid.trim().parse().ok())
        .expect("the fixture names its pid")
}

#[test]
fn a_pane_shell_sees_a_43_character_url_safe_token_that_the_manifest_never_holds() {
    let host = Host::start();
    let session = host.shell();
    let token = host.pane_token(&session);
    assert_eq!(token.len(), 43, "{token}");
    assert!(
        token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'),
        "{token}"
    );
    let manifest = std::fs::read_to_string(manifest_path(host.home.path(), &session)).unwrap();
    assert!(!manifest.contains(&token));
    let summary = serde_json::to_string(&host.host.inspect(&session).unwrap()).unwrap();
    assert!(!summary.contains(&token));
    assert!(!summary.contains("OSC7577_TOKEN"));
}

#[test]
fn two_panes_get_two_tokens() {
    let host = Host::start();
    let first = host.shell();
    let second = host.shell();
    assert_ne!(host.pane_token(&first), host.pane_token(&second));
}

#[test]
fn a_restarted_generation_gets_a_new_token() {
    let host = Host::start();
    let session = host.shell();
    let before = host.pane_token(&session);
    host.host
        .stop(&session, Some(SessionGeneration::FIRST))
        .unwrap();
    let restarted = host
        .host
        .restart(&session, Some(SessionGeneration::FIRST))
        .unwrap();
    assert_ne!(restarted.manifest.generation, SessionGeneration::FIRST);
    let after = host.pane_token(&session);
    assert_eq!(after.len(), 43);
    assert_ne!(before, after);
}

#[test]
fn a_signed_report_ended_by_st_or_bel_becomes_a_record() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim,-S,/tmp/session.vim"]);
    let records = host.records_until(&session, |records| records.len() == 1);
    assert_eq!(argv(&records[0]), ["nvim", "-S", "/tmp/session.vim"]);

    let bel = host.shell();
    host.run_fixture(&bel, &["bel", "report:lazygit"]);
    let records = host.records_until(&bel, |records| records.len() == 1);
    assert_eq!(argv(&records[0]), ["lazygit"]);
}

#[test]
fn an_oversized_report_is_ignored_and_the_earlier_record_of_its_owner_stays() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim", "oversize:htop"]);
    host.screen_until(&session, &step_marker(1, "oversize"));
    let records = host.records_until(&session, |records| !records.is_empty());
    assert_eq!(records.len(), 1);
    assert_eq!(argv(&records[0]), ["nvim"]);
}

#[test]
fn a_report_cut_by_can_stores_nothing() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["can:nvim", "wait:200", "osc7400"]);
    host.screen_until(&session, &step_marker(2, "osc7400"));
    assert!(host.records(&session).is_empty());
}

#[test]
fn an_unrelated_unknown_osc_changes_no_record_and_renders_nothing() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim", "osc7400"]);
    let screen = host.screen_until(&session, &step_marker(1, "osc7400"));
    let records = host.records(&session);
    assert_eq!(records.len(), 1);
    assert_eq!(argv(&records[0]), ["nvim"]);
    assert!(!screen.contains("status=busy"), "{screen}");
}

#[test]
fn a_forged_signature_stores_nothing() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["forged:nvim"]);
    host.screen_until(&session, &step_marker(0, "forged"));
    assert!(host.records(&session).is_empty());
}

#[test]
fn a_record_keeps_its_owner_start_time_and_the_parents_up_to_the_pane_shell() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim"]);
    let screen = host.screen_until(&session, &step_marker(0, "report"));
    let pid = fixture_pid(&screen);
    let record = host.records_until(&session, |records| records.len() == 1)[0].clone();
    let shell = host
        .host
        .inspect(&session)
        .unwrap()
        .manifest
        .process
        .unwrap();
    assert_eq!(record.owner.pid, pid);
    assert_eq!(
        Some(record.owner.started_at),
        paneflow_host::process::process_start_time(pid)
    );
    assert!(!record.first_process);
    let leader = record
        .ancestors
        .last()
        .expect("the chain ends at the shell");
    assert_eq!(leader.pid, shell.pid);
    assert_eq!(Some(leader.started_at), shell.started_at);
    assert!(record.ancestors.iter().all(|ancestor| ancestor.pid != pid));
}

#[test]
fn a_report_naming_a_dead_pid_is_rejected() {
    let host = Host::start();
    let session = host.shell();
    let mut dead = std::process::Command::new("true").spawn().unwrap();
    let pid = dead.id();
    dead.wait().unwrap();
    host.run_fixture(&session, &[&format!("pid:{pid}"), "report:nvim"]);
    host.screen_until(&session, &step_marker(1, "report"));
    assert!(host.records(&session).is_empty());
}

#[test]
fn a_report_from_a_process_that_left_the_pane_session_is_rejected() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["setsid:nvim"]);
    host.screen_until(&session, &step_marker(0, "setsid"));
    assert!(host.records(&session).is_empty());
}

#[test]
fn a_report_from_the_pane_first_process_is_kept_and_marked_first_process() {
    let host = Host::start();
    let session = host.fixture_pane(&["report:nvim"]);
    let record = host.records_until(&session, |records| records.len() == 1)[0].clone();
    assert!(record.first_process);
    assert!(record.resume_input().first_process);
    assert_eq!(
        Some(record.owner.pid),
        host.host
            .inspect(&session)
            .unwrap()
            .manifest
            .process
            .map(|process| process.pid)
    );
}

#[test]
fn a_report_without_cwd_takes_the_owner_directory_from_the_os_never_from_osc_7() {
    let host = Host::start();
    let session = host.shell();
    let directory = tempfile::tempdir().unwrap();
    let canonical = directory.path().canonicalize().unwrap();
    host.type_line(&session, "printf '\\033]7;file:///osc7/claims/this\\007'");
    host.run_fixture_in(&session, Some(&canonical), &["report:nvim"]);
    let record = host.records_until(&session, |records| records.len() == 1)[0].clone();
    assert_eq!(record.cwd_source, CwdSource::Owner);
    assert_eq!(Path::new(&record.cwd), canonical);
}

#[test]
fn a_report_cwd_key_wins_over_the_owner_directory() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["cwd:/declared/by/report", "report:nvim"]);
    let record = host.records_until(&session, |records| records.len() == 1)[0].clone();
    assert_eq!(record.cwd_source, CwdSource::Report);
    assert_eq!(record.cwd, "/declared/by/report");
}

#[test]
fn records_reach_the_manifest_on_disk_and_a_clear_removes_them_there() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim,-S,x.vim", "await", "clear"]);
    let on_disk = host.on_disk_until(&session, |manifest| !manifest.program_resume.is_empty());
    assert_eq!(
        argv(&on_disk.program_resume.records()[0]),
        ["nvim", "-S", "x.vim"]
    );
    host.type_line(&session, "");
    host.screen_until(&session, &step_marker(2, "clear"));
    host.on_disk_until(&session, |manifest| manifest.program_resume.is_empty());
    assert!(host.records(&session).is_empty());
}

#[test]
fn reset_alternate_screen_erase_and_prompt_marks_leave_records_alone() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim", "noise", "wait:100", "osc7400"]);
    host.screen_until(&session, &step_marker(3, "osc7400"));
    let records = host.records(&session);
    assert_eq!(records.len(), 1);
    assert_eq!(argv(&records[0]), ["nvim"]);
}

#[test]
fn an_owner_that_exits_on_its_own_loses_its_record_two_seconds_later() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim", "await", "exit"]);
    host.records_until(&session, |records| records.len() == 1);
    host.type_line(&session, "");
    let marked = host.records_until(&session, |records| {
        records.is_empty() || records[0].exited_at_ms.is_some()
    });
    let exited_at = marked
        .first()
        .and_then(|record| record.exited_at_ms)
        .expect("the exit is marked before the record goes");
    host.records_until(&session, <[Record]>::is_empty);
    let removed_at = paneflow_host::manifest::now_ms();
    assert!(
        removed_at >= exited_at + paneflow_program_resume::EXIT_GRACE_MS,
        "removed {} ms after the exit",
        removed_at - exited_at
    );
    host.on_disk_until(&session, |manifest| manifest.program_resume.is_empty());
}

#[test]
fn a_forced_shutdown_keeps_the_records_of_the_programs_it_ends() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim"]);
    host.on_disk_until(&session, |manifest| {
        manifest.program_resume.records().len() == 1
    });
    host.host.request_shutdown(true).unwrap();
    std::thread::sleep(Duration::from_millis(
        paneflow_program_resume::EXIT_GRACE_MS + 500,
    ));
    let on_disk = host.on_disk(&session);
    assert_eq!(on_disk.program_resume.records().len(), 1);
    assert_eq!(argv(&on_disk.program_resume.records()[0]), ["nvim"]);
}

#[test]
fn closing_a_pane_discards_its_records() {
    let host = Host::start();
    let session = host.shell();
    host.run_fixture(&session, &["report:nvim"]);
    host.on_disk_until(&session, |manifest| !manifest.program_resume.is_empty());
    let stopped = host.host.stop(&session, None).unwrap();
    assert!(stopped.manifest.program_resume.is_empty());
    assert!(host.records(&session).is_empty());
    host.on_disk_until(&session, |manifest| manifest.program_resume.is_empty());
}

#[test]
fn stop_everything_and_quit_discards_every_record() {
    let host = Host::start();
    let sessions = [host.shell(), host.shell()];
    for session in &sessions {
        host.run_fixture(session, &["report:nvim"]);
        host.on_disk_until(session, |manifest| !manifest.program_resume.is_empty());
    }
    for session in &sessions {
        host.host.stop(session, None).unwrap();
    }
    host.host.request_shutdown(false).unwrap();
    for session in &sessions {
        assert!(host.on_disk(session).program_resume.is_empty());
    }
}

struct ServedHost {
    home: tempfile::TempDir,
    process: std::process::Child,
    log: PathBuf,
}

impl ServedHost {
    fn spawn() -> Self {
        let home = tempfile::tempdir().unwrap();
        let log = home.path().join("host.log");
        let process = std::process::Command::new(env!("CARGO_BIN_EXE_paneflow-host"))
            .arg("--home")
            .arg(home.path())
            .arg("serve")
            .env("OSC7577_TOKEN", "inherited")
            .env("RUST_LOG", "debug")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        Self { home, process, log }
    }

    fn client(&self) -> paneflow_host::HostClient {
        let endpoint = paneflow_host::endpoint::host_endpoint_path(self.home.path());
        let hello = paneflow_host::ClientHello::local("program-resume-test");
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            match paneflow_host::HostClient::connect(&endpoint, &hello) {
                Ok(client) => return client,
                Err(error) => assert!(
                    Instant::now() < deadline,
                    "the host never answered: {error}"
                ),
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn cli(&self, args: &[&str]) -> String {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_paneflow-host"))
            .arg("--home")
            .arg(self.home.path())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn screen_until(
        &self,
        client: &mut paneflow_host::HostClient,
        session: &SessionId,
        mut found: impl FnMut(&str) -> Option<String>,
    ) -> String {
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let text = client.text(session).unwrap().text;
            if let Some(value) = found(&text) {
                return value;
            }
            assert!(Instant::now() < deadline, "never shown: {text}");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for ServedHost {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[test]
fn a_served_host_hides_the_token_from_its_log_and_cli_and_its_records_survive_a_kill() {
    let mut served = ServedHost::spawn();
    let mut client = served.client();
    let summary = client
        .create(&CreateSession {
            shell: Some("/bin/sh".to_string()),
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(200),
            rows: Some(50),
            ..CreateSession::default()
        })
        .unwrap();
    let session = summary.manifest.session.clone();
    let generation = summary.manifest.generation;
    client
        .input(
            &session,
            generation,
            b"printf 'tok<%s>\\n' \"$OSC7577_TOKEN\"\n",
        )
        .unwrap();
    let token = served.screen_until(&mut client, &session, |text| {
        text.lines()
            .filter_map(|line| line.trim().strip_prefix("tok<"))
            .filter_map(|rest| rest.strip_suffix('>'))
            .next_back()
            .map(str::to_string)
    });
    assert_ne!(token, "inherited");
    assert_eq!(token.len(), 43);

    let command = format!(
        "'{}' resume-report 'report:nvim,a\u{202E}b,-S'\n",
        fixture().display()
    );
    client
        .input(&session, generation, command.as_bytes())
        .unwrap();
    served.screen_until(&mut client, &session, |text| {
        text.contains("fixture step 0 report").then(String::new)
    });
    let manifest_file = manifest_path(served.home.path(), &session);
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    while read_manifest(&manifest_file)
        .unwrap()
        .program_resume
        .is_empty()
    {
        assert!(Instant::now() < deadline, "the record never reached disk");
        std::thread::sleep(Duration::from_millis(50));
    }

    let inspected = served.cli(&["session", "inspect", session.as_str()]);
    assert!(!inspected.contains(&token));
    assert!(!inspected.contains('\u{202E}'), "{inspected}");
    let parsed: serde_json::Value = serde_json::from_str(&inspected).unwrap();
    let record = &parsed["program_resume"]["records"][0];
    assert_eq!(
        record["argv"],
        serde_json::json!(["nvim", "a\\u{202E}b", "-S"])
    );
    assert!(
        record["cwd"]
            .as_str()
            .is_some_and(|cwd| cwd.starts_with('/'))
    );
    assert!(record["owner"]["pid"].as_u64().is_some());
    assert!(record["accepted_at_ms"].as_u64().is_some());
    assert!(
        !std::fs::read_to_string(&manifest_file)
            .unwrap()
            .contains(&token)
    );

    drop(client);
    served.process.kill().unwrap();
    served.process.wait().unwrap();
    let survived = read_manifest(&manifest_file).unwrap();
    assert_eq!(survived.program_resume.records().len(), 1);
    assert_eq!(
        argv(&survived.program_resume.records()[0]),
        ["nvim", "a\u{202E}b", "-S"]
    );
    let log = std::fs::read_to_string(&served.log).unwrap();
    assert!(log.contains("OSC 7577 accepted"), "{log}");
    assert!(!log.contains(&token));
}
