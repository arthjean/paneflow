use std::sync::Arc;
use std::time::{Duration, Instant};

use paneflow_host::host::AttemptOutcome;
use paneflow_program_resume::RecordId;

use super::*;
use crate::terminal::program_resume::{ResumePolicy, ResumeState, launcher_input, restore_state};
use crate::terminal::types::ShellQuoting;
use crate::terminal::view::conversation::{
    AttemptVerdict, attempt_verdict, conversation_resume_request, handed_off_attempt,
    typed_resume_input,
};

const DEADLINE: Duration = Duration::from_secs(30);
const DUMP: &str = "paneflow-argv-dump";
const CLAUDE_ID: &str = "3922faec-860a-47b1-8f2d-e6b9488c467c";
const BASH: &str = "/bin/bash";
const REPORTER: &str = "#!/bin/sh\n\
printf '%s %s\\n' \"$$\" \"$OSC7577_TOKEN\" > \"$1/owner.tmp\"\n\
mv \"$1/owner.tmp\" \"$1/owner\"\n\
while [ ! -f \"$1/sequence\" ]; do sleep 0.05; done\n\
cat \"$1/sequence\"\n\
exec sleep 600\n";
const DUMPER: &str = "#!/bin/sh\n\
printf '%s\\0' \"$@\" > argv.dump.tmp\n\
mv argv.dump.tmp argv.dump\n\
printf '%s' \"$$\" > argv.pid\n\
exec sleep 600\n";

fn write_script(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "{what} never happened");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn policy(mode: &str) -> ResumePolicy {
    let config: paneflow_config::schema::PaneFlowConfig =
        serde_json::from_value(serde_json::json!({"terminal": {"program_resume": {"mode": mode}}}))
            .unwrap();
    ResumePolicy::from_config(&config)
}

struct Bench {
    home: tempfile::TempDir,
    endpoint: PathBuf,
    tools: PathBuf,
    work: PathBuf,
    host: Option<(Arc<paneflow_host::SessionHost>, paneflow_host::ServerHandle)>,
}

impl Bench {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        crate::ai_hooks::extract::extract_binaries_in_home(home.path()).unwrap();
        let tools = home.path().join("tools");
        let work = home.path().join("work");
        std::fs::create_dir_all(&tools).unwrap();
        std::fs::create_dir_all(&work).unwrap();
        write_script(&tools.join("paneflow-report"), REPORTER);
        write_script(&tools.join(DUMP), DUMPER);
        let endpoint = paneflow_host::endpoint::host_endpoint_path(home.path());
        let mut bench = Self {
            home,
            endpoint,
            tools,
            work,
            host: None,
        };
        bench.start();
        bench
    }

    fn start(&mut self) {
        let host = paneflow_host::SessionHost::open(self.home.path(), &self.endpoint).unwrap();
        let server =
            paneflow_host::ServerHandle::spawn(Arc::clone(&host), self.endpoint.clone()).unwrap();
        self.host = Some((host, server));
    }

    fn lose_the_host(&mut self) {
        let (host, server) = self.host.take().unwrap();
        let _ = host.request_shutdown(true);
        server.stop().unwrap();
        wait_for("the old host releasing its state", || {
            (Arc::strong_count(&host) == 1).then_some(())
        });
        drop(host);
        self.start();
    }

    fn client(&self) -> HostClient {
        wait_for("the host answering", || connect(&self.endpoint).ok())
    }

    fn target(&self) -> HostEndpoint {
        HostEndpoint {
            home: self.home.path().to_path_buf(),
            endpoint: self.endpoint.clone(),
        }
    }

    fn create_bash(&self) -> SessionId {
        let mut env = HashMap::new();
        let args = paneflow_launch::shell::setup_shell_integration(
            Some(&self.home.path().join("shell")),
            BASH,
            &mut env,
        );
        let mut env: std::collections::BTreeMap<String, String> = env.into_iter().collect();
        env.insert("HOME".into(), self.home.path().display().to_string());
        env.insert(
            "PATH".into(),
            format!("{}:/usr/bin:/bin", self.tools.display()),
        );
        self.client()
            .create(&CreateSession {
                shell: Some(BASH.to_owned()),
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

    fn conversation(&self) -> paneflow_config::schema::AgentSessionRef {
        write_script(&self.tools.join("claude"), DUMPER);
        paneflow_config::schema::AgentSessionRef {
            runtime: "com.anthropic.claude-code".to_string(),
            id: CLAUDE_ID.to_string(),
            cwd: Some(self.work.display().to_string()),
        }
    }

    fn create_resuming(
        &self,
        shell: &str,
        integration: bool,
        resume: paneflow_host::host::ResumeRequest,
    ) -> HostedAttachment {
        let mut env = HashMap::new();
        let extra_args = if integration {
            paneflow_launch::shell::setup_shell_integration(
                Some(&self.home.path().join("shell")),
                shell,
                &mut env,
            )
        } else {
            Vec::new()
        };
        env.insert("HOME".into(), self.home.path().display().to_string());
        env.insert(
            "PATH".into(),
            format!("{}:/usr/bin:/bin", self.tools.display()),
        );
        let mut request = self.request(SessionIntent::Create, &SessionId::new());
        request.params.shell = shell.to_owned();
        request.params.extra_args = extra_args;
        request.params.env = env;
        request.params.cwd = self.home.path().to_path_buf();
        request.resume = Some(resume);
        match resolve_at(request, self.target()) {
            Ok(ResolveOutcome::Attached(hosted, _)) => *hosted,
            Ok(ResolveOutcome::Ended(end, _)) => panic!("the create ended: {:?}", end.kind),
            Ok(ResolveOutcome::Starting) => panic!("the create is still starting"),
            Err(error) => panic!("the create failed: {error}"),
        }
    }

    fn verdict(&self, hosted: &HostedAttachment) -> AttemptVerdict {
        let attempt = handed_off_attempt(hosted.resume_attempt.as_ref())
            .expect("the create prepared a conversation handoff");
        wait_for("the launcher reporting the conversation attempt", || {
            let report = resume_attempt_at(&self.endpoint, &hosted.attachment.session).ok()?;
            match attempt_verdict(report.as_ref(), &attempt) {
                AttemptVerdict::Pending => None,
                verdict => Some(verdict),
            }
        })
    }

    fn type_bytes(&self, session: &SessionId, bytes: &[u8]) {
        let mut client = self.client();
        let generation = client.inspect(session).unwrap().manifest.generation;
        client.input(session, generation, bytes).unwrap();
    }

    fn report(&self, session: &SessionId, argv: &[&str]) -> RecordId {
        let exchange = self
            .home
            .path()
            .join(format!("report-{}", SessionId::new()));
        std::fs::create_dir(&exchange).unwrap();
        self.type_bytes(
            session,
            format!("paneflow-report '{}'\n", exchange.as_path().display()).as_bytes(),
        );
        let owner = wait_for("the reporter naming its pid and token", || {
            std::fs::read_to_string(exchange.as_path().join("owner")).ok()
        });
        let (pid, token) = owner.trim().split_once(' ').unwrap();
        let report = paneflow_program_resume::Report {
            pid: pid.parse().unwrap(),
            time: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            action: paneflow_program_resume::Action::Argv(
                argv.iter().map(|element| (*element).to_string()).collect(),
            ),
            cwd: Some(self.work.display().to_string()),
        };
        let token = paneflow_program_resume::Token::new(token).unwrap();
        let body = paneflow_program_resume::signed_body(&report, &token);
        let staged = exchange.as_path().join("sequence.tmp");
        std::fs::write(&staged, paneflow_program_resume::osc_sequence(&body)).unwrap();
        std::fs::rename(staged, exchange.as_path().join("sequence")).unwrap();
        wait_for("the record reaching the host", || {
            self.client()
                .inspect(session)
                .ok()?
                .manifest
                .program_resume
                .records()
                .first()
                .map(|record| record.id)
        })
    }

    fn request(&self, intent: SessionIntent, session: &SessionId) -> AttachRequest {
        AttachRequest {
            intent,
            session: session.clone(),
            workspace: None,
            params: SpawnParams {
                shell: BASH.into(),
                shell_quoting: ShellQuoting::Posix,
                extra_args: Vec::new(),
                env: Default::default(),
                cwd: std::env::temp_dir(),
                cols: 80,
                rows: 24,
                profile: paneflow_config::schema::TerminalSurfaceProfile::Normal,
            },
            appearance: crate::terminal::ghostty_session::current_host_appearance(),
            cell: None,
            route: None,
            resume: None,
        }
    }

    fn restore(&self, session: &SessionId, policy: &ResumePolicy) -> (HostLinkEnd, ResumeState) {
        let Ok(ResolveOutcome::Ended(end, _)) = resolve_at(
            self.request(SessionIntent::Reattach, session),
            self.target(),
        ) else {
            panic!("a session of the lost host ends its restored pane");
        };
        assert!(end.kind.lost_by_the_host(), "{:?}", end.kind);
        let state = restore_state(policy, &end);
        (end, state)
    }

    fn recover(
        &self,
        session: &SessionId,
        end: &HostLinkEnd,
        state: &ResumeState,
    ) -> HostedAttachment {
        let Some((intent, false)) = end.restore_recovery(None) else {
            panic!("{:?} restarts under the same id", end.kind);
        };
        let mut request = self.request(intent, session);
        request.resume = match intent {
            SessionIntent::Restart { expected: Some(_) } => state.restart_request(),
            _ => None,
        };
        match resolve_at(request, self.target()) {
            Ok(ResolveOutcome::Attached(hosted, _)) => *hosted,
            Ok(ResolveOutcome::Ended(end, _)) => panic!("the restart ended: {:?}", end.kind),
            Ok(ResolveOutcome::Starting) => panic!("the restart is still starting"),
            Err(error) => panic!("the restart failed: {error}"),
        }
    }

    fn dump(&self) -> Vec<String> {
        let dump = wait_for("the resumed program's argv dump", || {
            std::fs::read(self.work.join("argv.pid")).ok()?;
            std::fs::read(self.work.join("argv.dump")).ok()
        });
        let mut elements: Vec<String> = dump
            .split(|byte| *byte == 0)
            .map(|element| String::from_utf8(element.to_vec()).unwrap())
            .collect();
        assert_eq!(
            elements.pop().as_deref(),
            Some(""),
            "the dump ends with a NUL"
        );
        elements
    }

    fn settle(&self, session: &SessionId, mut state: ResumeState) -> ResumeState {
        wait_for("the launcher reporting its attempt", || {
            let report = resume_attempt_at(&self.endpoint, session).ok()??;
            state.settled_outcome(&report).then_some(())
        });
        let report = resume_attempt_at(&self.endpoint, session).unwrap();
        state = state.after_attempt(report.as_ref());
        state
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        if let Some((host, server)) = self.host.take() {
            let _ = host.request_shutdown(true);
            let _ = server.stop();
        }
    }
}

#[test]
fn auto_mode_restarts_a_lost_pane_with_its_record_and_the_program_starts() {
    let mut bench = Bench::new();
    let session = bench.create_bash();
    let argv = [DUMP, "two words", "naïve", "", "-n"];
    let record = bench.report(&session, &argv);
    bench.lose_the_host();

    let (end, state) = bench.restore(&session, &policy("auto"));
    assert_eq!(
        state.restart_request().and_then(|request| request.record),
        Some(record)
    );
    let hosted = bench.recover(&session, &end, &state);
    let attempt = hosted
        .resume_attempt
        .clone()
        .expect("the restart names its attempt");
    assert_eq!(attempt.outcome, AttemptOutcome::Prepared);
    assert_eq!(attempt.record, Some(record));
    let state = state.after_attempt(hosted.resume_attempt.as_ref());
    assert!(state.awaited_attempt().is_some());
    assert_eq!(bench.dump(), argv[1..]);
    assert_eq!(bench.settle(&session, state), ResumeState::None);
}

#[test]
fn ask_mode_offers_then_the_typed_launcher_resumes_the_exact_argv() {
    let mut bench = Bench::new();
    let session = bench.create_bash();
    let argv = [DUMP, "$(reboot)", "a; b", "*"];
    let record = bench.report(&session, &argv);
    bench.lose_the_host();

    let (end, state) = bench.restore(&session, &policy("ask"));
    assert!(state.offers_resume(), "{state:?}");
    assert_eq!(state.restart_request(), None);
    let hosted = bench.recover(&session, &end, &state);
    assert_eq!(
        hosted.resume_attempt, None,
        "the plain restart prepared nothing"
    );
    let state = state.after_attempt(None);
    assert!(state.offers_resume(), "the banner stays on the plain shell");
    assert!(
        !std::fs::exists(bench.work.join("argv.dump")).unwrap(),
        "nothing ran before the user accepted"
    );

    let prepared = prepare_resume_at(&bench.endpoint, &session, record).unwrap();
    assert_eq!(prepared.record, Some(record));
    let typed = launcher_input(
        ShellQuoting::Posix,
        &prepared.launcher,
        &prepared.handoff,
        false,
    );
    let text = String::from_utf8(typed.clone()).unwrap();
    for element in &argv[1..] {
        assert!(!text.contains(element), "{element} was typed: {text}");
    }
    bench.type_bytes(&session, &typed);
    assert_eq!(bench.dump(), argv[1..]);
}

#[test]
fn a_dismissed_record_does_not_come_back_on_the_next_restore() {
    let mut bench = Bench::new();
    let session = bench.create_bash();
    let record = bench.report(&session, &[DUMP]);
    bench.lose_the_host();
    let (end, state) = bench.restore(&session, &policy("ask"));
    assert_eq!(state.dismissed_record(), Some(record));
    bench.recover(&session, &end, &state);

    discard_resume_at(&bench.endpoint, &session, state.dismissed_record().unwrap()).unwrap();
    let summary = bench.client().inspect(&session).unwrap();
    assert!(summary.resume_candidate.is_none());
    let manifest = paneflow_host::manifest::manifest_path(bench.home.path(), &session);
    wait_for("the next host reading no record on disk", || {
        paneflow_host::manifest::read_manifest(&manifest)
            .ok()?
            .program_resume
            .is_empty()
            .then_some(())
    });
    let next = restore_state(
        &policy("ask"),
        &end.with_resume_candidate(summary.resume_candidate),
    );
    assert_eq!(next, ResumeState::None);
}

#[test]
fn a_program_missing_at_resume_fails_with_its_name_and_drops_the_record() {
    let mut bench = Bench::new();
    let session = bench.create_bash();
    bench.report(&session, &["paneflow-missing-tool", "--flag"]);
    bench.lose_the_host();

    let (end, state) = bench.restore(&session, &policy("auto"));
    let hosted = bench.recover(&session, &end, &state);
    let state = state.after_attempt(hosted.resume_attempt.as_ref());
    let failed = bench.settle(&session, state);
    assert_eq!(
        failed.banner_message().as_deref(),
        Some("Could not resume `paneflow-missing-tool`: not found")
    );
    wait_for("the failed record leaving the session", || {
        bench
            .client()
            .inspect(&session)
            .unwrap()
            .manifest
            .program_resume
            .is_empty()
            .then_some(())
    });
}

fn resume_a_lost_claude_conversation_in(shell: &str) {
    let bench = Bench::new();
    let recorded = bench.conversation();
    let resume = conversation_resume_request(
        &recorded,
        &paneflow_config::schema::PaneFlowConfig::default(),
    )
    .expect("Claude resumes by id");
    let argv = resume.argv.clone().expect("the request carries the argv");
    assert_eq!(argv[..3], ["claude", "--resume", CLAUDE_ID]);
    let hosted = bench.create_resuming(shell, true, resume);
    let session = hosted.attachment.session.clone();
    assert_eq!(bench.dump(), argv[1..]);
    assert_eq!(bench.verdict(&hosted), AttemptVerdict::Started);
    let report = resume_attempt_at(&bench.endpoint, &session)
        .unwrap()
        .unwrap();
    assert_eq!(
        report.outcome,
        AttemptOutcome::Started,
        "no session.input reached the host, so the attempt never settled on input"
    );
}

#[test]
fn a_lost_claude_conversation_is_created_with_its_handoff_and_starts_without_a_keystroke() {
    resume_a_lost_claude_conversation_in(BASH);
}

#[test]
fn zsh_resumes_a_lost_claude_conversation_through_the_same_handoff() {
    if let Some(zsh) = ["/usr/bin/zsh", "/bin/zsh"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
    {
        resume_a_lost_claude_conversation_in(zsh);
    }
}

#[test]
fn a_shell_without_integration_refuses_the_handoff_so_the_desktop_keeps_typing() {
    let bench = Bench::new();
    let resume = conversation_resume_request(
        &bench.conversation(),
        &paneflow_config::schema::PaneFlowConfig::default(),
    )
    .unwrap();
    let hosted = bench.create_resuming(BASH, false, resume);
    let report = hosted
        .resume_attempt
        .clone()
        .expect("the refusal is reported");
    assert_eq!(report.outcome, AttemptOutcome::Refused);
    assert_eq!(report.reason.as_deref(), Some("shell_unsupported"));
    assert_eq!(handed_off_attempt(Some(&report)), None);
    assert!(
        !std::fs::exists(bench.work.join("argv.dump")).unwrap(),
        "nothing ran from a handoff the shell cannot consume"
    );
}

#[test]
fn the_offered_conversation_types_only_the_launcher_and_handoff_paths() {
    let bench = Bench::new();
    let session = bench.create_bash();
    let resume = conversation_resume_request(
        &bench.conversation(),
        &paneflow_config::schema::PaneFlowConfig::default(),
    )
    .unwrap();
    let argv = resume.argv.clone().unwrap();
    let command = argv.join(" ");
    let prepared = prepare_command_resume_at(
        &bench.endpoint,
        &session,
        argv.clone(),
        bench.work.display().to_string(),
    )
    .unwrap();
    assert_eq!(prepared.record, None);
    let typed = typed_resume_input(ShellQuoting::Posix, Ok(&prepared), &command);
    let text = String::from_utf8(typed.clone()).unwrap();
    assert!(
        !text.contains(CLAUDE_ID),
        "the conversation id was typed: {text}"
    );
    bench.type_bytes(&session, &typed);
    assert_eq!(bench.dump(), argv[1..]);
}

#[test]
fn a_conversation_whose_agent_is_missing_reports_a_failed_attempt() {
    let bench = Bench::new();
    let resume = paneflow_host::host::ResumeRequest {
        argv: Some(vec![
            "paneflow-missing-agent".to_string(),
            "--resume".to_string(),
            CLAUDE_ID.to_string(),
        ]),
        cwd: Some(bench.work.display().to_string()),
        ..paneflow_host::host::ResumeRequest::default()
    };
    let hosted = bench.create_resuming(BASH, true, resume);
    assert_eq!(
        bench.verdict(&hosted),
        AttemptVerdict::Failed("could not start paneflow-missing-agent: not found".to_string())
    );
}
