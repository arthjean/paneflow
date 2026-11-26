use std::time::{Duration, Instant};

use paneflow_program_resume::{
    Action, Admission, CwdSource, Outcome, ProcessStamp, Record, RecordCwd, Report,
};

use super::*;
#[cfg(unix)]
use crate::program_resume::{PaneOrigin, PaneToken, ReportSequence};

const OUTCOME_DEADLINE: Duration = Duration::from_secs(30);

fn wait_until(mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

fn shell_request() -> CreateSession {
    #[cfg(windows)]
    let (shell, args) = (
        Some("cmd.exe".to_string()),
        vec!["/Q".to_string(), "/D".to_string()],
    );
    #[cfg(unix)]
    let (shell, args) = (Some("/bin/sh".to_string()), Vec::new());
    CreateSession {
        shell,
        args,
        cwd: Some(std::env::temp_dir().display().to_string()),
        cols: Some(200),
        rows: Some(50),
        ..CreateSession::default()
    }
}

fn open_host(home: &Path) -> Arc<SessionHost> {
    SessionHost::open(home, &home.join("host.sock")).unwrap()
}

fn handles(host: &SessionHost, session: &SessionId) -> (Arc<Mutex<SessionManifest>>, Durability) {
    let sessions = host.lock_sessions();
    let record = sessions.get(session).unwrap();
    (Arc::clone(&record.manifest), Arc::clone(&record.durability))
}

fn records(host: &SessionHost, session: &SessionId) -> Vec<Record> {
    host.inspect(session)
        .unwrap()
        .manifest
        .program_resume
        .records()
        .to_vec()
}

#[cfg(unix)]
fn on_disk(host: &SessionHost, session: &SessionId) -> SessionManifest {
    crate::manifest::read_manifest(&crate::manifest::manifest_path(host.home(), session)).unwrap()
}

fn command_report(pid: u32, argv: &[&str]) -> Report {
    Report {
        pid,
        time: now_ms() / 1000,
        action: Action::Argv(argv.iter().map(|element| (*element).to_owned()).collect()),
        cwd: None,
    }
}

fn admission(started_at: u64) -> Admission {
    Admission {
        started_at,
        ancestors: Vec::new(),
        first_process: false,
        fallback_cwd: RecordCwd {
            path: "/".into(),
            source: CwdSource::Spawn,
        },
        sandbox: None,
    }
}

fn inject(host: &SessionHost, session: &SessionId, report: Report, admitted: Admission) {
    let (manifest, durability) = handles(host, session);
    assert!(host.commit(
        &manifest,
        &durability,
        None,
        WriteClass::Metadata,
        |record| {
            let _ = record.program_resume.apply(
                Outcome::Accepted(report),
                |_| Some(admitted),
                now_ms(),
            );
        }
    ));
}

#[cfg(unix)]
fn own_origin() -> PaneOrigin {
    let pid = std::process::id();
    PaneOrigin {
        leader: ProcessStamp {
            pid,
            started_at: crate::process::process_start_time(pid).unwrap(),
        },
        process_session: paneflow_peer::process_session(pid),
        spawn_cwd: "/".into(),
        sandbox: None,
    }
}

#[cfg(unix)]
fn ingest(host: &SessionHost, session: &SessionId, token: &PaneToken, report: &Report) {
    let (manifest, durability) = handles(host, session);
    let generation = manifest.lock().unwrap().generation;
    let body = paneflow_program_resume::signed_body(report, token.token());
    host.ingest_program_resume(
        &manifest,
        &durability,
        generation,
        token,
        ReportSequence {
            content: format!("7577;{body}").into_bytes(),
            truncated: false,
            origin: own_origin(),
        },
    );
}

#[cfg(unix)]
fn sleeper() -> (std::process::Child, ProcessStamp) {
    let child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let pid = child.id();
    let started_at = crate::process::process_start_time(pid).unwrap();
    (child, ProcessStamp { pid, started_at })
}

fn dead_process() -> ProcessStamp {
    #[cfg(windows)]
    let mut child = std::process::Command::new("cmd")
        .args(["/C", "exit 0"])
        .spawn()
        .unwrap();
    #[cfg(unix)]
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let pid = child.id();
    let started_at = crate::process::process_start_time(pid).unwrap_or(1);
    child.wait().unwrap();
    ProcessStamp { pid, started_at }
}

#[cfg(unix)]
#[test]
fn a_report_from_a_reused_pid_replaces_the_record_of_the_process_that_had_it() {
    let home = tempfile::tempdir().unwrap();
    let host = open_host(home.path());
    let session = host.create(shell_request()).unwrap().manifest.session;
    let (mut child, real) = sleeper();
    inject(
        &host,
        &session,
        command_report(real.pid, &["previous"]),
        admission(real.started_at - 1),
    );
    let stale = records(&host, &session);
    assert_eq!(stale.len(), 1);
    assert_eq!(stale[0].owner.started_at, real.started_at - 1);
    let token = PaneToken::generate().unwrap();
    ingest(
        &host,
        &session,
        &token,
        &command_report(real.pid, &["nvim"]),
    );
    let replaced = records(&host, &session);
    assert_eq!(replaced.len(), 1);
    assert_eq!(replaced[0].argv, ["nvim"]);
    assert_eq!(replaced[0].owner, real);
    assert_ne!(replaced[0].id, stale[0].id);
    let _ = child.kill();
    let _ = child.wait();
    host.stop(&session, None).unwrap();
}

#[cfg(unix)]
#[test]
fn two_reports_before_the_writer_runs_become_one_metadata_write_from_no_runtime_file_write() {
    let home = tempfile::tempdir().unwrap();
    let host = open_host(home.path());
    let session = host.create(shell_request()).unwrap().manifest.session;
    let (_, durability) = handles(&host, &session);
    assert!(wait_until(|| host.persistence.report().queued_jobs == 0));
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    host.persistence.spawn_exclusive(move || {
        let _ = started_tx.send(());
        let _ = release_rx.recv();
        Ok(())
    });
    started_rx.recv_timeout(OUTCOME_DEADLINE).unwrap();
    let token = PaneToken::generate().unwrap();
    let pid = std::process::id();
    ingest(&host, &session, &token, &command_report(pid, &["first"]));
    ingest(&host, &session, &token, &command_report(pid, &["second"]));
    assert_eq!(records(&host, &session)[0].argv, ["second"]);
    assert_eq!(host.persistence.report().queued_jobs, 1);
    assert!(on_disk(&host, &session).program_resume.is_empty());
    let written = durability.written_revision();
    release_tx.send(()).unwrap();
    assert!(wait_until(|| {
        on_disk(&host, &session)
            .program_resume
            .records()
            .first()
            .is_some_and(|record| record.argv == ["second"])
    }));
    assert_eq!(durability.written_revision(), written + 2);
    host.stop(&session, None).unwrap();
}

#[test]
fn an_owner_gone_before_its_watch_is_marked_exited_and_removed_after_the_grace() {
    let home = tempfile::tempdir().unwrap();
    let host = open_host(home.path());
    let session = host.create(shell_request()).unwrap().manifest.session;
    let owner = dead_process();
    inject(
        &host,
        &session,
        command_report(owner.pid, &["nvim"]),
        admission(owner.started_at),
    );
    let (manifest, durability) = handles(&host, &session);
    host.watch_resume_owner(&session, &manifest, &durability, owner);
    assert!(wait_until(|| records(&host, &session)
        .first()
        .is_none_or(|record| record.exited_at_ms.is_some())));
    let exited_at = records(&host, &session)
        .first()
        .and_then(|record| record.exited_at_ms)
        .expect("the record is marked before it is removed");
    assert!(wait_until(|| records(&host, &session).is_empty()));
    assert!(now_ms() >= exited_at + paneflow_program_resume::EXIT_GRACE_MS);
    assert!(wait_until(|| host.watched_resume_owners() == 0));
    host.stop(&session, None).unwrap();
}

#[cfg(unix)]
#[test]
fn a_host_that_dies_inside_the_exit_grace_leaves_the_record_on_disk() {
    let home = tempfile::tempdir().unwrap();
    let host = open_host(home.path());
    let session = host.create(shell_request()).unwrap().manifest.session;
    let (mut child, owner) = sleeper();
    let token = PaneToken::generate().unwrap();
    ingest(
        &host,
        &session,
        &token,
        &command_report(owner.pid, &["nvim"]),
    );
    assert!(wait_until(|| !on_disk(&host, &session)
        .program_resume
        .is_empty()));
    assert_eq!(host.watched_resume_owners(), 1);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(wait_until(|| records(&host, &session)
        .first()
        .is_some_and(|record| record.exited_at_ms.is_some())));
    host.persistence.stop_writer();
    assert!(wait_until(|| records(&host, &session).is_empty()));
    let next_host_reads = on_disk(&host, &session);
    assert_eq!(next_host_reads.program_resume.records().len(), 1);
    assert_eq!(next_host_reads.program_resume.records()[0].argv, ["nvim"]);
    assert_eq!(
        next_host_reads.program_resume.records()[0].exited_at_ms,
        None
    );
    let _ = host.request_shutdown(true);
}

#[cfg(windows)]
#[test]
fn a_windows_pane_never_sees_the_token_while_conpty_is_unproven() {
    let home = tempfile::tempdir().unwrap();
    let host = open_host(home.path());
    let session = host.create(shell_request()).unwrap().manifest.session;
    host.input(&session, None, b"echo tok[%OSC7577_TOKEN%]\r\n".to_vec())
        .unwrap();
    assert!(wait_until(|| host.text(&session).is_ok_and(|text| text
        .text
        .contains("tok[%OSC7577_TOKEN%]")
        && text
            .text
            .lines()
            .any(|line| line.trim() == "tok[%OSC7577_TOKEN%]"))));
    host.stop(&session, None).unwrap();
}
