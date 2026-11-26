#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use paneflow_host::host::{AttemptOutcome, ResumeRequest};
use paneflow_host::manifest::{manifest_path, read_manifest};
use paneflow_host::{ClientHello, CreateSession, HostClient, SessionId, SessionSummary};
use paneflow_program_resume::RecordId;

const OUTCOME_DEADLINE: Duration = Duration::from_secs(30);
const FIXTURE_NAME: &str = "paneflow-session-fixture";
const EXACT_ARGV: [&str; 9] = [
    FIXTURE_NAME,
    "argv-dump",
    "$(reboot)",
    "a; b",
    "two words",
    "",
    "-n",
    "naïve",
    "*",
];

fn built(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_BIN_EXE_paneflow-host"))
        .parent()
        .unwrap()
        .join(name);
    assert!(
        path.is_file(),
        "{} is missing: build the workspace binaries first (cargo build --workspace, or run the suite with --workspace)",
        path.display()
    );
    path
}

fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + OUTCOME_DEADLINE;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn copy_executable(from: &Path, to: &Path) {
    std::fs::copy(from, to).unwrap();
}

struct Bench {
    home: tempfile::TempDir,
    tools: PathBuf,
    work: PathBuf,
    host: Option<std::process::Child>,
}

impl Bench {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let helpers = home
            .path()
            .join("cache")
            .join("bin")
            .join(paneflow_host::helpers::HELPER_VERSION);
        std::fs::create_dir_all(&helpers).unwrap();
        copy_executable(&built("paneflow-shim"), &helpers.join("paneflow-resume"));
        copy_executable(
            &built("paneflow-ai-hook"),
            &helpers.join("paneflow-ai-hook"),
        );
        let tools = home.path().join("tools");
        let work = home.path().join("work");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        copy_executable(
            Path::new(env!("CARGO_BIN_EXE_paneflow-session-fixture")),
            &tools.join(FIXTURE_NAME),
        );
        let mut bench = Self {
            home,
            tools,
            work,
            host: None,
        };
        bench.start_host();
        bench
    }

    fn start_host(&mut self) {
        let process = std::process::Command::new(env!("CARGO_BIN_EXE_paneflow-host"))
            .arg("--home")
            .arg(self.home.path())
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        self.host = Some(process);
    }

    fn kill_host(&mut self) {
        use std::os::unix::process::ExitStatusExt;
        let mut process = self.host.take().unwrap();
        let pid = libc::pid_t::try_from(process.id()).unwrap();
        assert_eq!(unsafe { libc::kill(pid, libc::SIGKILL) }, 0);
        let status = process.wait().unwrap();
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{status:?}");
    }

    fn client(&self) -> HostClient {
        let endpoint = paneflow_host::endpoint::host_endpoint_path(self.home.path());
        let hello = ClientHello::local("program-resume-launch-test");
        wait_for("the host answering", || {
            HostClient::connect(&endpoint, &hello).ok()
        })
    }

    fn create(&self, shell: &str, extra_env: &[(&str, &str)]) -> SessionId {
        let mut env = HashMap::new();
        let args = paneflow_launch::shell::setup_shell_integration(
            Some(&self.home.path().join("shell")),
            shell,
            &mut env,
        );
        let mut env: std::collections::BTreeMap<String, String> = env.into_iter().collect();
        env.insert("HOME".into(), self.home.path().display().to_string());
        env.insert(
            "PATH".into(),
            format!("{}:/usr/bin:/bin", self.tools.display()),
        );
        for (key, value) in extra_env {
            env.insert((*key).to_owned(), (*value).to_owned());
        }
        self.client()
            .create(&CreateSession {
                shell: Some(shell.to_owned()),
                args,
                env,
                cwd: Some(self.home.path().display().to_string()),
                cols: Some(200),
                rows: Some(50),
                ..CreateSession::default()
            })
            .unwrap()
            .manifest
            .session
    }

    fn type_line(&self, session: &SessionId, line: &str) {
        let mut client = self.client();
        let generation = client.inspect(session).unwrap().manifest.generation;
        client
            .input(session, generation, format!("{line}\n").as_bytes())
            .unwrap();
    }

    fn report_from_the_pane(&self, session: &SessionId, argv: &[&str]) -> RecordId {
        self.report_with(session, argv, "")
    }

    fn report_in_the_background(&self, session: &SessionId, argv: &[&str]) -> RecordId {
        self.report_with(session, argv, " < /dev/null &")
    }

    fn report_with(&self, session: &SessionId, argv: &[&str], suffix: &str) -> RecordId {
        self.type_line(
            session,
            &format!(
                "{FIXTURE_NAME} resume-report 'cwd:{}' 'report:{}'{suffix}",
                self.work.display(),
                argv.join(",")
            ),
        );
        let manifest = manifest_path(self.home.path(), session);
        wait_for("the record reaching the manifest", || {
            read_manifest(&manifest)
                .ok()?
                .program_resume
                .records()
                .first()
                .map(|record| record.id)
        })
    }

    fn summary(&self, session: &SessionId) -> SessionSummary {
        self.client().inspect(session).unwrap()
    }

    fn summary_until(
        &self,
        session: &SessionId,
        what: &str,
        mut done: impl FnMut(&SessionSummary) -> bool,
    ) -> SessionSummary {
        wait_for(what, || {
            let summary = self.summary(session);
            done(&summary).then_some(summary)
        })
    }

    fn screen_until(&self, session: &SessionId, marker: &str) -> String {
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let text = self
                .client()
                .text(session)
                .map(|text| text.text)
                .unwrap_or_default();
            if text.contains(marker) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "{marker:?} never reached the pane: {text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn lose_and_resume(&mut self, session: &SessionId, record: RecordId) -> SessionSummary {
        self.lose_then_resume(session, record, |_| {})
    }

    fn lose_then_resume(
        &mut self,
        session: &SessionId,
        record: RecordId,
        while_lost: impl FnOnce(&Self),
    ) -> SessionSummary {
        self.kill_host();
        while_lost(self);
        self.start_host();
        let lost = self.summary(session);
        assert!(!lost.live);
        let candidate = lost
            .resume_candidate
            .expect("a lost session offers its record");
        assert_eq!(candidate.record, record);
        assert_eq!(candidate.blocked, None);
        self.client()
            .restart_resuming(
                session,
                None,
                &ResumeRequest {
                    record: Some(record),
                    ..ResumeRequest::default()
                },
            )
            .unwrap()
    }

    fn dump(&self) -> (Vec<Vec<u8>>, u32) {
        let dump = wait_for("the resumed program's argv dump", || {
            std::fs::read(self.work.join("argv.pid")).ok()?;
            std::fs::read(self.work.join("argv.dump")).ok()
        });
        let pid = std::fs::read_to_string(self.work.join("argv.pid"))
            .unwrap()
            .parse()
            .unwrap();
        let mut elements: Vec<Vec<u8>> =
            dump.split(|byte| *byte == 0).map(<[u8]>::to_vec).collect();
        assert_eq!(elements.pop(), Some(Vec::new()), "the dump ends with a NUL");
        (elements, pid)
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        if let Some(mut process) = self.host.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }
}

fn exact_argv() -> Vec<Vec<u8>> {
    EXACT_ARGV
        .iter()
        .map(|element| element.as_bytes().to_vec())
        .collect()
}

fn resume_exactly_in(shell: &str) -> Vec<Vec<u8>> {
    let mut bench = Bench::new();
    let session = bench.create(shell, &[]);
    let record = bench.report_from_the_pane(&session, &EXACT_ARGV);
    let restarted = bench.lose_and_resume(&session, record);
    let attempt = restarted
        .resume_attempt
        .expect("the restart names its attempt");
    assert_eq!(attempt.outcome, AttemptOutcome::Prepared);
    let (dump, pid) = bench.dump();
    let summary = bench.summary_until(
        &session,
        "the record moving to the resumed program",
        |summary| {
            summary
                .manifest
                .program_resume
                .records()
                .first()
                .is_some_and(|record| record.owner.pid == pid)
        },
    );
    let candidate = summary.resume_candidate.unwrap();
    assert!(
        !candidate.settled,
        "a started attempt stays unsettled until input or a report"
    );
    assert_eq!(
        summary.resume_attempt.unwrap().outcome,
        AttemptOutcome::Started
    );
    dump
}

#[test]
fn a_killed_host_resumes_the_program_in_bash_with_its_exact_argv_and_hands_it_the_record() {
    let mut bench = Bench::new();
    let session = bench.create("/bin/bash", &[]);
    let record = bench.report_from_the_pane(&session, &EXACT_ARGV);
    let restarted = bench.lose_and_resume(&session, record);
    let attempt = restarted.resume_attempt.clone().unwrap();
    assert_eq!(attempt.outcome, AttemptOutcome::Prepared);
    assert_eq!(attempt.record, Some(record));
    assert_eq!(restarted.manifest.cwd, bench.work.display().to_string());

    let (dump, pid) = bench.dump();
    assert_eq!(dump, exact_argv());
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    assert!(cmdline.starts_with(b"paneflow-session-fixture\0argv-dump\0"));

    let summary = bench.summary_until(
        &session,
        "the record moving to the resumed program",
        |summary| {
            summary
                .manifest
                .program_resume
                .records()
                .first()
                .is_some_and(|record| record.owner.pid == pid)
        },
    );
    assert_eq!(
        summary.resume_attempt.as_ref().unwrap().outcome,
        AttemptOutcome::Started
    );
    assert!(!summary.resume_candidate.as_ref().unwrap().settled);

    let on_disk = read_manifest(&manifest_path(bench.home.path(), &session)).unwrap();
    assert!(
        on_disk
            .launch
            .env
            .keys()
            .all(|key| key != "PANEFLOW_RESUME_HANDOFF" && key != "OSC7577_TOKEN"),
        "{:?}",
        on_disk.launch.env.keys().collect::<Vec<_>>()
    );
    assert!(
        std::fs::read_dir(bench.home.path().join("sessions").join(session.as_str()))
            .map(|entries| entries
                .flatten()
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".handoff")))
            .unwrap_or(true),
        "the launcher deleted the handoff"
    );

    bench.kill_host();
    bench.start_host();
    let after_crash = bench.summary(&session);
    let candidate = after_crash.resume_candidate.unwrap();
    assert_eq!(candidate.owner_pid, pid);
    assert!(
        !candidate.settled,
        "a host killed during a started attempt leaves it unsettled"
    );
}

#[test]
fn the_first_typed_input_after_a_resume_settles_its_attempt() {
    let mut bench = Bench::new();
    let session = bench.create("/bin/bash", &[]);
    let record = bench.report_from_the_pane(&session, &EXACT_ARGV);
    bench.lose_and_resume(&session, record);
    let (_, pid) = bench.dump();
    bench.summary_until(&session, "the started report", |summary| {
        summary
            .resume_attempt
            .as_ref()
            .map(|attempt| attempt.outcome)
            == Some(AttemptOutcome::Started)
            && summary
                .manifest
                .program_resume
                .records()
                .first()
                .is_some_and(|record| record.owner.pid == pid)
    });
    bench.type_line(&session, "");
    let settled = bench.summary_until(&session, "the attempt settling on input", |summary| {
        summary
            .resume_candidate
            .as_ref()
            .is_some_and(|candidate| candidate.settled)
    });
    assert_eq!(
        settled.resume_attempt.unwrap().outcome,
        AttemptOutcome::Settled
    );
}

#[test]
fn a_resumed_owner_that_reports_again_settles_the_attempt_under_its_new_pid() {
    let mut bench = Bench::new();
    let session = bench.create("/bin/bash", &[]);
    let work = bench.work.display().to_string();
    let reported = [
        FIXTURE_NAME,
        "resume-report",
        &format!("cwd:{work}"),
        "report:again",
    ];
    let record = bench.report_from_the_pane(&session, &reported);
    bench.lose_and_resume(&session, record);
    let settled = bench.summary_until(&session, "the resumed owner's own report", |summary| {
        summary
            .manifest
            .program_resume
            .records()
            .iter()
            .any(|record| record.argv == ["again"] && record.settled())
    });
    let owner = settled.manifest.program_resume.records()[0].owner.pid;
    assert_eq!(
        settled
            .resume_attempt
            .as_ref()
            .and_then(|attempt| attempt.pid),
        Some(owner)
    );
    assert_eq!(
        settled.resume_attempt.unwrap().outcome,
        AttemptOutcome::Settled
    );
}

#[test]
fn a_program_missing_from_path_drops_its_record_and_leaves_a_plain_shell() {
    let mut bench = Bench::new();
    let session = bench.create("/bin/bash", &[]);
    let record = bench.report_from_the_pane(&session, &["paneflow-no-such-program", "--flag"]);
    bench.lose_and_resume(&session, record);
    let failed = bench.summary_until(&session, "the failed attempt", |summary| {
        summary
            .resume_attempt
            .as_ref()
            .map(|attempt| attempt.outcome)
            == Some(AttemptOutcome::Failed)
    });
    let attempt = failed.resume_attempt.unwrap();
    assert_eq!(attempt.reason.as_deref(), Some("not_found"));
    assert_eq!(attempt.program.as_deref(), Some("paneflow-no-such-program"));
    assert!(failed.manifest.program_resume.is_empty());
    assert!(failed.resume_candidate.is_none());
    assert!(failed.live);
    bench.screen_until(
        &session,
        "paneflow-resume: paneflow-no-such-program: not found",
    );
    bench.type_line(&session, "echo plain-$((40+2))");
    bench.screen_until(&session, "plain-42");
}

#[test]
fn a_function_an_alias_and_a_builtin_named_like_the_program_never_run_instead_of_it() {
    let mut bench = Bench::new();
    std::fs::write(
        bench.home.path().join(".bashrc"),
        format!(
            "shopt -s expand_aliases\n{FIXTURE_NAME}() {{ echo function > '{work}/hijacked'; }}\nalias {FIXTURE_NAME}=\"echo alias > '{work}/hijacked'\"\ntrue() {{ echo function > '{work}/hijacked'; }}\n",
            work = bench.work.display()
        ),
    )
    .unwrap();
    copy_executable(
        Path::new(env!("CARGO_BIN_EXE_paneflow-session-fixture")),
        &bench.tools.join("true"),
    );
    let session = bench.create("/bin/bash", &[]);
    bench.type_line(
        &session,
        "unset -f true paneflow-session-fixture; unalias -a",
    );
    let record = bench.report_from_the_pane(&session, &EXACT_ARGV);
    bench.lose_and_resume(&session, record);
    let (dump, _) = bench.dump();
    assert_eq!(dump, exact_argv());
    assert!(!bench.work.join("hijacked").exists());

    std::fs::remove_file(bench.work.join("argv.dump")).unwrap();
    std::fs::remove_file(bench.work.join("argv.pid")).unwrap();
    let session = bench.create("/bin/bash", &[]);
    bench.type_line(
        &session,
        "unset -f true paneflow-session-fixture; unalias -a",
    );
    let record = bench.report_from_the_pane(&session, &["true", "argv-dump", "builtin"]);
    bench.lose_and_resume(&session, record);
    let (dump, _) = bench.dump();
    assert_eq!(
        dump,
        [b"true".to_vec(), b"argv-dump".to_vec(), b"builtin".to_vec()]
    );
    assert!(!bench.work.join("hijacked").exists());
}

fn installed(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain([PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .map(|path| path.display().to_string())
}

#[test]
fn zsh_delivers_the_same_exact_argv_as_bash() {
    if let Some(zsh) = installed("zsh") {
        assert_eq!(resume_exactly_in(&zsh), exact_argv());
    }
}

#[test]
fn fish_delivers_the_same_exact_argv_after_a_config_that_never_sees_the_handoff() {
    let Some(fish) = installed("fish") else {
        return;
    };
    let mut bench = Bench::new();
    let config = bench.home.path().join(".config").join("fish");
    std::fs::create_dir_all(&config).unwrap();
    let seen = bench.home.path().join("seen-by-config");
    let user_tools = bench.home.path().join("user-tools");
    std::fs::create_dir_all(&user_tools).unwrap();
    std::fs::write(
        config.join("config.fish"),
        format!(
            "for name in PANEFLOW_RESUME_HANDOFF __paneflow_resume_handoff __paneflow_resume_launcher\n    set -q $name; and echo $name >> '{seen}'\nend\necho checked >> '{seen}'\nset -gx PATH '{tools}' $PATH\n",
            seen = seen.display(),
            tools = user_tools.display()
        ),
    )
    .unwrap();
    let session = bench.create(&fish, &[]);
    let record = bench.report_from_the_pane(&session, &EXACT_ARGV);
    bench.lose_and_resume(&session, record);
    let (dump, pid) = bench.dump();
    assert_eq!(dump, exact_argv());
    let checked = std::fs::read_to_string(&seen).unwrap();
    assert!(
        checked.lines().all(|line| line == "checked"),
        "config.fish saw a resume variable: {checked}"
    );
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    let path = environ
        .split(|byte| *byte == 0)
        .find_map(|entry| entry.strip_prefix(b"PATH="))
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .unwrap_or_default();
    assert!(
        path.split(':')
            .any(|entry| entry == user_tools.display().to_string()),
        "the resumed program inherits the PATH config.fish set: {path}"
    );
}

#[test]
fn resume_calls_from_outside_the_window_or_outside_the_pane_are_refused() {
    let bench = Bench::new();
    let session = bench.create("/bin/bash", &[]);
    let record = bench.report_in_the_background(&session, &["nvim", "-S", "s.vim"]);
    let endpoint = paneflow_host::endpoint::host_endpoint_path(bench.home.path());

    let mut outsider = bench.client();
    let refused = outsider
        .call(
            "session.resume_attempt",
            serde_json::json!({"session": session, "attempt": "0123456789abcdef0123456789abcdef", "outcome": "failed", "reason": "not_found"}),
        )
        .unwrap_err();
    assert!(
        matches!(refused, paneflow_host::HostClientError::Rpc { .. }),
        "{refused}"
    );
    assert_eq!(
        bench.summary(&session).manifest.program_resume.records()[0].id,
        record
    );

    let other = bench.create("/bin/bash", &[]);
    let other_record = bench.report_in_the_background(&other, &["htop"]);
    let prepared = bench
        .client()
        .prepare_resume(
            &other,
            &ResumeRequest {
                record: Some(other_record),
                ..ResumeRequest::default()
            },
        )
        .unwrap();
    let other_owner = bench.summary(&other).manifest.program_resume.records()[0]
        .owner
        .pid;
    bench.type_line(
        &session,
        &format!(
            "{FIXTURE_NAME} call host '{}' control session.resume_attempt '{}' < /dev/null &",
            endpoint.display(),
            serde_json::json!({"session": other, "attempt": prepared.attempt.to_string(), "outcome": "started", "pid": std::process::id()})
        ),
    );
    let screen = bench.screen_until(&session, "call done");
    assert!(screen.contains("call error="), "{screen}");
    assert!(!screen.contains("call ok="), "{screen}");
    let untouched = bench.summary(&other);
    assert_eq!(
        untouched.manifest.program_resume.records()[0].owner.pid,
        other_owner
    );
    assert_eq!(
        untouched.resume_attempt.unwrap().outcome,
        AttemptOutcome::Prepared
    );

    for (index, (method, params)) in [
        (
            "session.prepare_resume",
            serde_json::json!({"session": session, "record": record.get(), "confirmed": true}),
        ),
        (
            "session.create",
            serde_json::json!({"shell": "/bin/bash", "resume": {"argv": ["nvim"], "cwd": "/"}}),
        ),
        (
            "session.discard_resume",
            serde_json::json!({"session": session, "record": record.get()}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        bench.type_line(
            &session,
            &format!(
                "{FIXTURE_NAME} call host '{}' control {method} '{params}' < /dev/null &",
                endpoint.display()
            ),
        );
        let answers = wait_for(&format!("the answer to {method}"), || {
            let text = bench.client().text(&session).ok()?.text;
            let answers: Vec<String> = text.split("call done").map(str::to_owned).collect();
            (answers.len() > index + 2).then_some(answers)
        });
        assert!(
            answers[index + 1].contains("call reason=principal_refused"),
            "{method}: {}",
            answers[index + 1]
        );
    }
    assert_eq!(
        bench.summary(&session).manifest.program_resume.records()[0].id,
        record
    );
}

fn self_reporting_argv(work: &str) -> [String; 4] {
    [
        FIXTURE_NAME.to_owned(),
        "resume-report".to_owned(),
        format!("cwd:{work}"),
        format!("report:{FIXTURE_NAME}"),
    ]
}

fn cmdline_of(pid: u32) -> Vec<String> {
    let raw = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let mut elements: Vec<String> = raw
        .split(|byte| *byte == 0)
        .map(|element| String::from_utf8_lossy(element).into_owned())
        .collect();
    if elements.last().is_some_and(String::is_empty) {
        elements.pop();
    }
    elements
}

#[test]
fn a_sigkilled_host_resumes_the_reporting_fixture_whose_new_report_settles_it_under_its_new_pid() {
    let mut bench = Bench::new();
    let session = bench.create("/bin/bash", &[]);
    let reported = self_reporting_argv(&bench.work.display().to_string());
    let reported_refs: Vec<&str> = reported.iter().map(String::as_str).collect();
    let record = bench.report_from_the_pane(&session, &reported_refs);
    let first_owner = bench.summary(&session).manifest.program_resume.records()[0]
        .owner
        .pid;
    assert_eq!(
        cmdline_of(first_owner)[..2],
        [FIXTURE_NAME, "resume-report"]
    );

    let restarted = bench.lose_and_resume(&session, record);
    let attempt = restarted.resume_attempt.unwrap();
    assert_eq!(attempt.outcome, AttemptOutcome::Prepared);
    assert_eq!(attempt.record, Some(record));

    let settled = bench.summary_until(&session, "the resumed fixture's own report", |summary| {
        summary
            .resume_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.outcome == AttemptOutcome::Settled)
            && summary
                .manifest
                .program_resume
                .records()
                .iter()
                .any(|record| record.argv == [FIXTURE_NAME] && record.settled())
    });
    let records = settled.manifest.program_resume.records();
    assert_eq!(records.len(), 1, "{records:?}");
    let owner = records[0].owner.pid;
    assert_ne!(
        owner, first_owner,
        "the record moved to the resumed process"
    );
    assert_eq!(settled.resume_attempt.unwrap().pid, Some(owner));
    assert_eq!(
        cmdline_of(owner),
        reported,
        "the resumed fixture runs with the argv it reported"
    );
    assert!(settled.resume_candidate.unwrap().settled);
}

#[test]
fn a_fixture_removed_from_path_while_the_host_is_dead_drops_its_record_and_leaves_a_plain_shell() {
    let mut bench = Bench::new();
    let session = bench.create("/bin/bash", &[]);
    let reported = self_reporting_argv(&bench.work.display().to_string());
    let reported_refs: Vec<&str> = reported.iter().map(String::as_str).collect();
    let record = bench.report_from_the_pane(&session, &reported_refs);

    bench.lose_then_resume(&session, record, |bench| {
        std::fs::remove_file(bench.tools.join(FIXTURE_NAME)).unwrap();
    });
    let failed = bench.summary_until(&session, "the failed attempt", |summary| {
        summary
            .resume_attempt
            .as_ref()
            .map(|attempt| attempt.outcome)
            == Some(AttemptOutcome::Failed)
    });
    let attempt = failed.resume_attempt.unwrap();
    assert_eq!(attempt.reason.as_deref(), Some("not_found"));
    assert_eq!(attempt.program.as_deref(), Some(FIXTURE_NAME));
    assert!(failed.manifest.program_resume.is_empty());
    assert!(failed.resume_candidate.is_none());
    assert!(failed.live);
    let manifest = manifest_path(bench.home.path(), &session);
    wait_for("the dropped record leaving the manifest on disk", || {
        read_manifest(&manifest)
            .ok()?
            .program_resume
            .is_empty()
            .then_some(())
    });
    bench.screen_until(
        &session,
        &format!("paneflow-resume: {FIXTURE_NAME}: not found"),
    );
    bench.type_line(&session, "echo plain-$((40+2))");
    bench.screen_until(&session, "plain-42");
}

const WRAPPER_NAME: &str = "paneflow-wrapper";

fn install_child_spawning_wrapper(bench: &Bench) {
    use std::os::unix::fs::PermissionsExt;
    let wrapper = bench.tools.join(WRAPPER_NAME);
    std::fs::write(
        &wrapper,
        format!("#!/bin/sh\n{FIXTURE_NAME} \"$@\"\nstatus=$?\nexit $status\n"),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn a_program_a_wrapper_starts_as_its_child_takes_over_the_record_and_clears_it_for_good() {
    let mut bench = Bench::new();
    install_child_spawning_wrapper(&bench);
    let session = bench.create("/bin/bash", &[]);
    let work = bench.work.display().to_string();
    let reported = [
        WRAPPER_NAME,
        "resume-report",
        &format!("cwd:{work}"),
        &format!("report:{WRAPPER_NAME}"),
        "await",
        "clear",
    ];
    let record = bench.report_from_the_pane(&session, &reported);
    bench.lose_and_resume(&session, record);

    let settled = bench.summary_until(&session, "the wrapped program's own report", |summary| {
        summary
            .resume_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.outcome == AttemptOutcome::Settled)
    });
    let records = settled.manifest.program_resume.records();
    assert_eq!(records.len(), 1, "{records:?}");
    let program = records[0].owner.pid;
    assert_eq!(records[0].argv, [WRAPPER_NAME]);
    assert!(records[0].settled());
    assert_eq!(cmdline_of(program)[..2], [FIXTURE_NAME, "resume-report"]);
    assert_eq!(settled.resume_attempt.unwrap().pid, Some(program));
    assert!(settled.resume_candidate.unwrap().settled);

    bench.type_line(&session, "");
    bench.summary_until(&session, "the program's clear", |summary| {
        summary.manifest.program_resume.is_empty() && summary.resume_candidate.is_none()
    });
}
