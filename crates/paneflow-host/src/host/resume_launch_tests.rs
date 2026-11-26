use std::time::{Duration, Instant};

use paneflow_program_resume::{
    Action, Admission, AttemptId, CwdSource, Outcome, RecordCwd, RecordId, Report, SandboxStamp,
};

use super::*;

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

struct Bench {
    home: tempfile::TempDir,
    host: Arc<SessionHost>,
    rcfile: PathBuf,
    work: PathBuf,
}

impl Bench {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let helpers = home
            .path()
            .join("cache")
            .join("bin")
            .join(crate::helpers::HELPER_VERSION);
        std::fs::create_dir_all(&helpers).unwrap();
        std::fs::write(helpers.join("paneflow-ai-hook"), b"").unwrap();
        std::fs::write(helpers.join("paneflow-resume"), b"").unwrap();
        let rcfile = home.path().join("stub-bashrc");
        std::fs::write(&rcfile, b"").unwrap();
        let work = home.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let host = SessionHost::open(home.path(), &home.path().join("host.sock")).unwrap();
        Self {
            home,
            host,
            rcfile,
            work,
        }
    }

    fn create(&self, shell: &str, args: Vec<String>) -> SessionId {
        self.host
            .create(CreateSession {
                shell: Some(shell.to_owned()),
                args,
                cwd: Some(self.home.path().display().to_string()),
                cols: Some(200),
                rows: Some(50),
                ..CreateSession::default()
            })
            .unwrap()
            .manifest
            .session
    }

    fn bash(&self) -> SessionId {
        self.create(
            "/bin/bash",
            vec!["--rcfile".into(), self.rcfile.display().to_string()],
        )
    }

    fn inject(
        &self,
        session: &SessionId,
        argv: &[&str],
        cwd: Option<&Path>,
        adjust: impl FnOnce(&mut Admission),
    ) -> RecordId {
        let mut admission = Admission {
            started_at: 1,
            ancestors: Vec::new(),
            first_process: false,
            fallback_cwd: RecordCwd {
                path: self.home.path().display().to_string(),
                source: CwdSource::Spawn,
            },
            sandbox: None,
        };
        adjust(&mut admission);
        let report = Report {
            pid: 4_000_000 + u32::try_from(self.records(session).len()).unwrap(),
            time: now_ms() / 1000,
            action: Action::Argv(argv.iter().map(|element| (*element).to_owned()).collect()),
            cwd: cwd.map(|cwd| cwd.display().to_string()),
        };
        let (manifest, durability) = self.handles(session);
        assert!(self.host.commit(
            &manifest,
            &durability,
            None,
            WriteClass::Critical,
            |record| {
                assert!(record.program_resume.apply(
                    Outcome::Accepted(report),
                    |_| Some(admission),
                    now_ms()
                ));
            }
        ));
        self.records(session).last().unwrap().id
    }

    fn handles(&self, session: &SessionId) -> (Arc<Mutex<SessionManifest>>, Durability) {
        let sessions = self.host.lock_sessions();
        let record = sessions.get(session).unwrap();
        (Arc::clone(&record.manifest), Arc::clone(&record.durability))
    }

    fn records(&self, session: &SessionId) -> Vec<paneflow_program_resume::Record> {
        self.host
            .inspect(session)
            .unwrap()
            .manifest
            .program_resume
            .records()
            .to_vec()
    }

    fn on_disk(&self, session: &SessionId) -> SessionManifest {
        crate::manifest::read_manifest(&crate::manifest::manifest_path(self.home.path(), session))
            .unwrap()
    }

    fn end(&self, session: &SessionId) {
        self.host.input(session, None, b"exit\n".to_vec()).unwrap();
        assert!(wait_until(|| self.host.inspect(session).is_ok_and(
            |summary| !summary.live
                && !summary.pending_launch
                && summary.descendants_unresolved == 0
                && matches!(summary.manifest.lifecycle, SessionLifecycle::Exited { .. })
        )));
    }

    fn screen_line(&self, session: &SessionId, command: &str, prefix: &str) -> String {
        self.host
            .input(session, None, format!("{command}\n").into_bytes())
            .unwrap();
        let mut found = None;
        assert!(wait_until(|| {
            let text = self
                .host
                .text(session)
                .map(|text| text.text)
                .unwrap_or_default();
            found = text
                .lines()
                .filter_map(|line| line.trim().strip_prefix(prefix))
                .filter_map(|rest| rest.strip_suffix('>'))
                .map(str::to_owned)
                .next_back();
            found.is_some()
        }));
        found.unwrap()
    }

    fn restart(&self, session: &SessionId, request: ResumeRequest) -> SessionSummary {
        self.host
            .restart_with(session, None, Some(&request))
            .unwrap()
    }

    fn handoffs(&self, session: &SessionId) -> Vec<PathBuf> {
        std::fs::read_dir(self.host.session_data_dir(session))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(crate::resume_launch::is_handoff_file)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let _ = self.host.request_shutdown(true);
    }
}

fn record(id: RecordId) -> ResumeRequest {
    ResumeRequest {
        record: Some(id),
        ..ResumeRequest::default()
    }
}

fn refusal(summary: &SessionSummary) -> Option<&str> {
    summary
        .resume_attempt
        .as_ref()
        .filter(|attempt| attempt.outcome == AttemptOutcome::Refused)
        .and_then(|attempt| attempt.reason.as_deref())
}

fn refused(result: Result<impl std::fmt::Debug, HostError>) -> ResumeRefusal {
    match result {
        Err(HostError::ResumeRefused(refusal)) => refusal,
        other => panic!("expected a resume refusal, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn a_restart_with_a_record_writes_a_private_handoff_and_spawns_the_shell_in_the_record_directory() {
    use std::os::unix::fs::PermissionsExt;
    let bench = Bench::new();
    let session = bench.bash();
    let id = bench.inject(
        &session,
        &["nvim", "-S", "a b.vim"],
        Some(&bench.work),
        |_| {},
    );
    bench.end(&session);
    let lost = bench.host.inspect(&session).unwrap();
    let candidate = lost.resume_candidate.unwrap();
    assert_eq!(candidate.record, id);
    assert_eq!(candidate.argv, ["nvim", "-S", "a b.vim"]);
    assert_eq!(candidate.cwd_source, CwdSource::Report);
    assert!(candidate.settled);
    assert_eq!(candidate.blocked, None);

    let restarted = bench.restart(&session, record(id));
    let attempt = restarted.resume_attempt.clone().unwrap();
    assert_eq!(attempt.outcome, AttemptOutcome::Prepared);
    assert_eq!(attempt.record, Some(id));
    assert_eq!(restarted.manifest.cwd, bench.work.display().to_string());
    assert_eq!(
        restarted.manifest.launch.args,
        ["--rcfile".to_owned(), bench.rcfile.display().to_string()]
    );

    let handoffs = bench.handoffs(&session);
    assert_eq!(handoffs.len(), 1);
    let handoff = &handoffs[0];
    assert_eq!(
        std::fs::metadata(handoff).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let decoded =
        paneflow_resume_handoff::decode(&std::fs::read_to_string(handoff).unwrap()).unwrap();
    assert_eq!(decoded.attempt, attempt.attempt.unwrap().to_string());
    assert_eq!(decoded.argv, ["nvim", "-S", "a b.vim"]);
    assert_eq!(decoded.cwd, bench.work.display().to_string());
    assert_eq!(
        decoded.cwd_source,
        paneflow_resume_handoff::CwdSource::Report
    );

    let seen = bench.screen_line(
        &session,
        "printf 'h<%s>\\n' \"$PANEFLOW_RESUME_HANDOFF\"",
        "h<",
    );
    assert_eq!(Path::new(&seen), handoff.as_path());
    let pwd = bench.screen_line(&session, "printf 'd<%s>\\n' \"$PWD\"", "d<");
    assert_eq!(
        std::fs::canonicalize(pwd).unwrap(),
        std::fs::canonicalize(&bench.work).unwrap()
    );

    let on_disk = bench.on_disk(&session);
    assert!(!on_disk.launch.env.contains_key("PANEFLOW_RESUME_HANDOFF"));
    let manifest_text =
        std::fs::read_to_string(crate::manifest::manifest_path(bench.home.path(), &session))
            .unwrap();
    assert!(!manifest_text.contains(&handoff.display().to_string()));
    assert!(!on_disk.program_resume.records()[0].settled());
}

#[cfg(unix)]
#[test]
fn a_restart_refused_for_its_record_still_starts_a_plain_shell_and_names_the_reason() {
    let bench = Bench::new();
    let gone = bench.home.path().join("deleted");
    std::fs::create_dir_all(&gone).unwrap();

    let session = bench.bash();
    let id = bench.inject(&session, &["nvim"], Some(&gone), |_| {});
    std::fs::remove_dir(&gone).unwrap();
    bench.end(&session);
    assert_eq!(
        bench
            .host
            .inspect(&session)
            .unwrap()
            .resume_candidate
            .unwrap()
            .blocked,
        Some(ResumeRefusal::CwdMissing)
    );
    let restarted = bench.restart(&session, record(id));
    assert_eq!(refusal(&restarted), Some("cwd_missing"));
    assert!(restarted.live || restarted.pending_launch);
    assert!(bench.handoffs(&session).is_empty());
    assert_eq!(bench.records(&session).len(), 1, "a blocked record is kept");

    let plain = bench.create("/bin/sh", Vec::new());
    let id = bench.inject(&plain, &["nvim"], Some(&bench.work), |_| {});
    bench.end(&plain);
    assert_eq!(
        bench
            .host
            .inspect(&plain)
            .unwrap()
            .resume_candidate
            .unwrap()
            .blocked,
        Some(ResumeRefusal::ShellUnsupported)
    );
    assert_eq!(
        refusal(&bench.restart(&plain, record(id))),
        Some("shell_unsupported")
    );

    let sandboxed = bench.bash();
    let id = bench.inject(&sandboxed, &["nvim"], Some(&bench.work), |admission| {
        admission.sandbox = Some(SandboxStamp {
            root: "/elsewhere".to_owned(),
            unix_scope: false,
        });
    });
    bench.end(&sandboxed);
    assert_eq!(
        bench
            .host
            .inspect(&sandboxed)
            .unwrap()
            .resume_candidate
            .unwrap()
            .blocked,
        Some(ResumeRefusal::SandboxChanged)
    );
    assert_eq!(
        refusal(&bench.restart(&sandboxed, record(id))),
        Some("sandbox_changed")
    );

    let first = bench.bash();
    let id = bench.inject(&first, &["nvim"], None, |admission| {
        admission.first_process = true;
    });
    bench.end(&first);
    assert_eq!(
        bench
            .host
            .inspect(&first)
            .unwrap()
            .resume_candidate
            .unwrap()
            .blocked,
        Some(ResumeRefusal::FirstProcess)
    );
    assert_eq!(
        refusal(&bench.restart(&first, record(id))),
        Some("first_process")
    );
}

#[cfg(unix)]
#[test]
fn an_unsettled_record_is_resumed_only_when_the_restart_confirms_it() {
    let bench = Bench::new();
    let session = bench.bash();
    let id = bench.inject(&session, &["nvim"], Some(&bench.work), |_| {});
    let (manifest, durability) = bench.handles(&session);
    bench.host.commit(
        &manifest,
        &durability,
        None,
        WriteClass::Critical,
        |current| {
            current
                .program_resume
                .begin_attempt(id, AttemptId::from_bytes([7; 16]))
                .unwrap();
        },
    );
    bench.end(&session);
    let candidate = bench
        .host
        .inspect(&session)
        .unwrap()
        .resume_candidate
        .unwrap();
    assert!(!candidate.settled);
    assert_eq!(candidate.blocked, None);
    let restarted = bench.restart(&session, record(id));
    assert_eq!(refusal(&restarted), Some("unsettled"));
    assert!(bench.handoffs(&session).is_empty());

    bench.end(&session);
    let confirmed = bench.restart(
        &session,
        ResumeRequest {
            record: Some(id),
            confirmed: true,
            ..ResumeRequest::default()
        },
    );
    assert_eq!(
        confirmed.resume_attempt.unwrap().outcome,
        AttemptOutcome::Prepared
    );
    assert_eq!(bench.handoffs(&session).len(), 1);
}

#[cfg(unix)]
#[test]
fn a_live_session_prepares_a_handoff_for_its_record_or_for_a_catalog_command() {
    let bench = Bench::new();
    let session = bench.bash();
    let id = bench.inject(&session, &["lazygit"], Some(&bench.work), |_| {});
    let prepared = bench.host.prepare_resume(&session, &record(id)).unwrap();
    assert_eq!(
        Path::new(&prepared.launcher),
        crate::resume_launch::launcher_path(bench.host.helper_dir().unwrap())
    );
    assert!(Path::new(&prepared.launcher).is_absolute());
    assert_eq!(prepared.record, Some(id));
    let decoded =
        paneflow_resume_handoff::decode(&std::fs::read_to_string(&prepared.handoff).unwrap())
            .unwrap();
    assert_eq!(decoded.argv, ["lazygit"]);
    assert_eq!(decoded.attempt, prepared.attempt.to_string());

    let catalog = bench
        .host
        .prepare_resume(
            &session,
            &ResumeRequest {
                argv: Some(vec!["claude".into(), "--resume".into(), "abc".into()]),
                cwd: Some(bench.work.display().to_string()),
                ..ResumeRequest::default()
            },
        )
        .unwrap();
    assert_eq!(catalog.record, None);
    let decoded =
        paneflow_resume_handoff::decode(&std::fs::read_to_string(&catalog.handoff).unwrap())
            .unwrap();
    assert_eq!(decoded.argv, ["claude", "--resume", "abc"]);
    assert_eq!(decoded.cwd, bench.work.display().to_string());
    assert_eq!(
        decoded.cwd_source,
        paneflow_resume_handoff::CwdSource::Report
    );
    assert!(
        !Path::new(&prepared.handoff).exists(),
        "one pending handoff per session"
    );

    assert_eq!(
        refused(bench.host.prepare_resume(&session, &record(id))),
        ResumeRefusal::Unsettled
    );
    assert!(
        bench
            .host
            .prepare_resume(
                &session,
                &ResumeRequest {
                    record: Some(id),
                    confirmed: true,
                    ..ResumeRequest::default()
                },
            )
            .is_ok()
    );
    assert_eq!(
        refused(bench.host.prepare_resume(
            &session,
            &ResumeRequest {
                argv: Some(vec!["./relative".into()]),
                cwd: Some("/".into()),
                ..ResumeRequest::default()
            }
        )),
        ResumeRefusal::InvalidCommand
    );
}

#[cfg(unix)]
#[test]
fn a_create_with_a_catalog_resume_starts_its_shell_with_that_handoff() {
    let bench = Bench::new();
    let summary = bench
        .host
        .create(CreateSession {
            shell: Some("/bin/bash".into()),
            args: vec!["--rcfile".into(), bench.rcfile.display().to_string()],
            cwd: Some(bench.home.path().display().to_string()),
            cols: Some(200),
            rows: Some(50),
            resume: Some(ResumeRequest {
                argv: Some(vec!["codex".into(), "resume".into(), "id-1".into()]),
                cwd: Some(bench.work.display().to_string()),
                ..ResumeRequest::default()
            }),
            ..CreateSession::default()
        })
        .unwrap();
    let session = summary.manifest.session.clone();
    assert_eq!(
        summary.resume_attempt.unwrap().outcome,
        AttemptOutcome::Prepared
    );
    let seen = bench.screen_line(
        &session,
        "printf 'h<%s>\\n' \"$PANEFLOW_RESUME_HANDOFF\"",
        "h<",
    );
    let decoded =
        paneflow_resume_handoff::decode(&std::fs::read_to_string(&seen).unwrap()).unwrap();
    assert_eq!(decoded.argv, ["codex", "resume", "id-1"]);
    assert_eq!(
        bench.host.inspect(&session).unwrap().manifest.cwd,
        bench.work.display().to_string()
    );
}

#[cfg(unix)]
#[test]
fn attempt_reports_move_settle_drop_or_block_the_record_and_unknown_ids_are_refused() {
    let bench = Bench::new();
    let session = bench.bash();
    let shell_pid = bench
        .screen_line(&session, "printf 'p<%s>\\n' \"$$\"", "p<")
        .parse::<u32>()
        .unwrap();

    let id = bench.inject(&session, &["nvim"], Some(&bench.work), |_| {});
    let prepared = bench.host.prepare_resume(&session, &record(id)).unwrap();
    assert_eq!(
        refused(bench.host.resume_attempt(
            &session,
            &AttemptId::from_bytes([9; 16]),
            ReportedAttempt::Started { pid: shell_pid }
        )),
        ResumeRefusal::UnknownAttempt
    );
    let started = bench
        .host
        .resume_attempt(
            &session,
            &prepared.attempt,
            ReportedAttempt::Started { pid: shell_pid },
        )
        .unwrap();
    assert_eq!(started.outcome, AttemptOutcome::Started);
    let moved = bench.records(&session);
    assert_eq!(moved[0].owner.pid, shell_pid);
    assert!(!moved[0].settled());
    assert!(!bench.on_disk(&session).program_resume.records()[0].settled());
    bench.host.input(&session, None, b" ".to_vec()).unwrap();
    assert!(bench.records(&session)[0].settled());

    let dropped = bench.inject(&session, &["gone-tool"], Some(&bench.work), |_| {});
    let prepared = bench
        .host
        .prepare_resume(&session, &record(dropped))
        .unwrap();
    let failed = bench
        .host
        .resume_attempt(
            &session,
            &prepared.attempt,
            ReportedAttempt::Failed(paneflow_program_resume::AttemptFailure::NotFound),
        )
        .unwrap();
    assert_eq!(failed.reason.as_deref(), Some("not_found"));
    assert!(
        bench
            .records(&session)
            .iter()
            .all(|record| record.id != dropped)
    );

    let missing = bench.home.path().join("vanishing");
    std::fs::create_dir_all(&missing).unwrap();
    let blocked = bench.inject(&session, &["nvim", "-S"], Some(&missing), |_| {});
    let prepared = bench
        .host
        .prepare_resume(&session, &record(blocked))
        .unwrap();
    std::fs::remove_dir(&missing).unwrap();
    bench
        .host
        .resume_attempt(
            &session,
            &prepared.attempt,
            ReportedAttempt::Failed(paneflow_program_resume::AttemptFailure::CwdMissing),
        )
        .unwrap();
    let summary = bench.host.inspect(&session).unwrap();
    assert!(summary.manifest.program_resume.get(blocked).is_some());
    let candidate = summary.resume_candidate.unwrap();
    assert_eq!(candidate.record, blocked);
    assert_eq!(candidate.blocked, Some(ResumeRefusal::CwdMissing));
}

#[cfg(unix)]
#[test]
fn discarding_a_record_removes_it_from_the_store_and_the_manifest_and_an_unknown_id_changes_nothing()
 {
    let bench = Bench::new();
    let session = bench.bash();
    let kept = bench.inject(&session, &["htop"], None, |_| {});
    let discarded = bench.inject(&session, &["nvim"], None, |_| {});
    let summary = bench.host.discard_resume(&session, discarded).unwrap();
    assert!(summary.manifest.program_resume.get(discarded).is_none());
    assert!(
        bench
            .on_disk(&session)
            .program_resume
            .get(discarded)
            .is_none()
    );
    assert!(bench.on_disk(&session).program_resume.get(kept).is_some());
    let before = bench.on_disk(&session).program_resume;
    assert_eq!(
        refused(bench.host.discard_resume(&session, RecordId::from(999))),
        ResumeRefusal::UnknownRecord
    );
    assert_eq!(bench.on_disk(&session).program_resume, before);
}
