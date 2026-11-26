#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use paneflow_config::schema::{SessionId, WorkspaceId};
use paneflow_host::protocol::ClientHello;
use paneflow_host::{CreateSession, HostClient, ServerHandle, SessionHost};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const PATIENCE: Duration = Duration::from_secs(90);

const INIT_LUA: &str = "paneflow.action{ name = 'quick', title = 'Quick', agents = true,\n\
   run = function() return { done = true } end }\n\
 paneflow.action{ name = 'slow', title = 'Slow', agents = true,\n\
   run = function() paneflow.sleep(3) return 'slept' end }\n\
 paneflow.action{ name = 'secret', title = 'Secret', run = function() return 'hidden' end }\n";

struct Instance {
    home: tempfile::TempDir,
    scripts: tempfile::TempDir,
    core: Arc<SessionHost>,
    server: Option<ServerHandle>,
    worker: Option<paneflow_serve::worker::RunningWorker>,
    owner: HostClient,
    agent: PathBuf,
}

impl Instance {
    fn open(with_worker: bool) -> Self {
        let home = tempfile::tempdir().unwrap();
        for (name, contents) in [
            ("paneflow.json", "{}"),
            ("session.json", "{}"),
            ("window-state.json", "{}"),
        ] {
            std::fs::write(home.path().join(name), contents).unwrap();
        }
        std::fs::write(paneflow_home::init_lua_path_in(home.path()), INIT_LUA).unwrap();
        let endpoint = paneflow_home::host_endpoint_path(home.path());
        let core = SessionHost::open(home.path(), &endpoint).unwrap();
        let server = ServerHandle::spawn(Arc::clone(&core), endpoint.clone()).unwrap();
        let owner = HostClient::connect(&endpoint, &ClientHello::local("cli-relay-test")).unwrap();
        let worker = with_worker.then(|| paneflow_serve::open(home.path()).unwrap());
        let scripts = tempfile::tempdir().unwrap();
        let agent = scripts.path().join("claude");
        std::os::unix::fs::symlink("/bin/sh", &agent).unwrap();
        Self {
            home,
            scripts,
            core,
            server: Some(server),
            worker,
            owner,
            agent,
        }
    }

    fn home(&self) -> &Path {
        self.home.path()
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.scripts.path().join(name);
        let body = body.replace(
            "paneflow ",
            &format!("'{}' ", env!("CARGO_BIN_EXE_paneflow")),
        );
        std::fs::write(&path, body).unwrap();
        path
    }

    fn pane(&mut self, title: &str, workspace: &WorkspaceId, body: &str) -> SessionId {
        let script = self.script(&format!("{title}.sh"), &format!("{body}\nsleep 120\n"));
        self.start(title, workspace, &script)
    }

    fn agent_pane(&mut self, title: &str, workspace: &WorkspaceId, body: &str) -> SessionId {
        let inner = self.script(&format!("{title}-agent.sh"), body);
        let outer = format!(
            "'{}' '{}'\nsleep 120\n",
            self.agent.display(),
            inner.display()
        );
        let script = self.script(&format!("{title}.sh"), &outer);
        self.start(title, workspace, &script)
    }

    fn start(&mut self, title: &str, workspace: &WorkspaceId, script: &Path) -> SessionId {
        let mut env = BTreeMap::new();
        env.insert(
            paneflow_home::HOME_ENV.to_string(),
            self.home().display().to_string(),
        );
        let request = CreateSession {
            workspace: Some(workspace.clone()),
            shell: Some("/bin/sh".to_string()),
            args: vec![script.display().to_string()],
            env,
            cwd: Some(std::env::temp_dir().display().to_string()),
            cols: Some(240),
            rows: Some(60),
            title: Some(title.to_string()),
            ..CreateSession::default()
        };
        self.owner.create(&request).unwrap().manifest.session
    }

    fn text(&self, session: &SessionId) -> String {
        self.core
            .text(session)
            .map(|text| text.text.replace("\r\n", "\n"))
            .unwrap_or_default()
    }

    fn until(&self, session: &SessionId, needle: &str) -> String {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let text = self.text(session);
            if text.contains(needle) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "{needle} never appeared in the pane:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn release(&self, name: &str) {
        std::fs::write(self.scripts.path().join(name), b"").unwrap();
    }

    fn released(&self, name: &str) -> String {
        format!(
            "while [ ! -f '{}' ]; do sleep 0.1; done",
            self.scripts.path().join(name).display()
        )
    }

    fn window(&self) -> HostControl {
        let mut follower =
            HostControl::connect(&paneflow_home::serve_endpoint_path(self.home()), "window")
                .unwrap();
        let header = follower
            .request("prompt.follow", json!({"kinds": ["input", "pick"]}))
            .unwrap();
        assert_eq!(header["type"], "following");
        follower
    }

    fn reply(&self, shown: &Value, answer: Value) {
        HostControl::connect(&paneflow_home::serve_endpoint_path(self.home()), "reply")
            .unwrap()
            .request(
                "prompt.reply",
                json!({"id": shown["id"], "token": shown["token"], "answer": answer}),
            )
            .unwrap();
    }

    fn stop(mut self) {
        if let Some(worker) = self.worker.take() {
            worker.stop();
        }
        let _ = self.core.request_shutdown(true);
        if let Some(server) = self.server.take() {
            let _ = server.stop();
        }
    }
}

fn next_requested(follower: &mut HostControl, title: &str) -> Value {
    let deadline = Instant::now() + PATIENCE;
    loop {
        assert!(Instant::now() < deadline, "{title} was never asked");
        let line = follower.read_stream_line(PATIENCE).unwrap().unwrap();
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["type"] == "requested" && frame["spec"]["title"] == title {
            return frame;
        }
    }
}

fn after<'t>(text: &'t str, marker: &str) -> &'t str {
    text.split(marker).nth(1).unwrap_or_default()
}

fn code(text: &str, name: &str) -> Option<i32> {
    after(text, &format!("{name}="))
        .split_whitespace()
        .next()
        .and_then(|code| code.parse().ok())
}

fn unwrapped(text: &str) -> String {
    text.lines().map(str::trim_end).collect::<Vec<_>>().join("")
}

fn outside(home: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow"));
    command
        .args(args)
        .env(paneflow_home::HOME_ENV, home)
        .env_remove("PANEFLOW_SOCKET_PATH")
        .env_remove("PANEFLOW_ALLOW_SOCKET_OVERRIDE");
    for key in paneflow_host::env::PANE_CONTEXT_ENV {
        command.env_remove(key);
    }
    command.output().unwrap()
}

fn skipped_by_environment() -> bool {
    std::env::var_os("PANEFLOW_IPC_SCRIPTING").is_some()
        || std::env::var_os("PANEFLOW_IPC_ORCHESTRATION").is_some()
}

fn approvals(owner_endpoint: &Path) -> Receiver<Value> {
    let (frames, received) = channel();
    let mut watcher =
        HostClient::connect(owner_endpoint, &ClientHello::local("cli-relay-window")).unwrap();
    std::thread::spawn(move || {
        let _ = watcher.follow_approvals(|frame| frames.send(frame.clone()).is_ok());
    });
    received
}

fn pending_request(frames: &Receiver<Value>, source: &SessionId) -> Value {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = frames.recv_timeout(left).expect("an approval request");
        if let Some(request) = frame["pending"].as_array().and_then(|list| {
            list.iter()
                .find(|request| request["source_session"] == source.to_string())
        }) {
            return request.clone();
        }
    }
}

#[test]
fn write_outside_a_pane_exits_1_before_any_request() {
    let home = tempfile::tempdir().unwrap();
    let output = outside(home.path(), &["write", "reviewer", "hello"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "paneflow: paneflow write needs the session of the calling pane; run it from a Paneflow pane\n"
    );
    assert!(output.stdout.is_empty());
}

#[test]
fn write_relays_through_the_pair_approval_and_reports_every_refusal() {
    if skipped_by_environment() {
        return;
    }
    let mut instance = Instance::open(false);
    let own = WorkspaceId::new();
    let theirs = WorkspaceId::new();
    let target = instance.pane(
        "target-pane",
        &own,
        "printf '\\033[?2004h'\necho target-ready",
    );
    instance.until(&target, "target-ready");
    let foreign = instance.pane("foreign-pane", &theirs, "echo foreign-ready");
    instance.until(&foreign, "foreign-ready");

    let lonely = instance.pane(
        "lonely-pane",
        &own,
        "paneflow write target-pane hello\necho \"rc_lonely=$?\"",
    );
    let text = instance.until(&lonely, "rc_lonely=");
    assert!(text.contains("no_window"), "{text}");
    assert_eq!(code(&text, "rc_lonely"), Some(1), "{text}");

    let frames = approvals(&paneflow_home::host_endpoint_path(instance.home()));
    let body = format!(
        "paneflow write target-pane 'hello from the writer'\necho \"rc_pending=$?\"\n\
         {wait}\n\
         paneflow write target-pane 'hello from the writer' --submit --json\necho \"rc_written=$?\"\n\
         paneflow write target-pane 'plain line'\necho \"rc_plain=$?\"\n\
         paneflow write target-pane \"$(head -c 17000 /dev/zero | tr '\\0' a)\"\necho \"rc_big=$?\"\n\
         paneflow write writer-pane self\necho \"rc_self=$?\"\n\
         paneflow write foreign-pane over\necho \"rc_foreign=$?\"\n\
         paneflow write foreign-pane over --scope all\necho \"rc_foreign_all=$?\"\n\
         echo writer-done",
        wait = instance.released("allowed"),
    );
    let writer = instance.pane("writer-pane", &own, &body);
    let text = instance.until(&writer, "rc_pending=");
    let request = pending_request(&frames, &writer);
    assert!(
        text.contains(&format!("approval_pending {}", request["id"])),
        "{text}"
    );
    assert_eq!(code(&text, "rc_pending"), Some(0), "{text}");
    assert_eq!(request["source_session"], writer.to_string());
    assert_eq!(request["target_session"], target.to_string());
    assert_eq!(request["source"], "writer-pane");
    assert_eq!(request["target"], "target-pane");
    assert!(
        !instance.text(&target).contains("hello from the writer"),
        "nothing is written before the human allows the pair"
    );

    let mut decider = HostClient::connect(
        &paneflow_home::host_endpoint_path(instance.home()),
        &ClientHello::local("cli-relay-decider"),
    )
    .unwrap();
    decider
        .decide_approval(request["id"].as_u64().unwrap(), true)
        .unwrap();
    instance.release("allowed");
    let text = instance.until(&writer, "writer-done");

    let written = after(&text, "rc_pending=");
    let reply_start = written
        .find('{')
        .unwrap_or_else(|| panic!("a JSON reply:\n{text}"));
    let reply_end = written.find("rc_written=").expect("the written exit code");
    let reply: Value = serde_json::from_str(&written[reply_start..reply_end]).unwrap();
    assert_eq!(reply["status"], "written", "{text}");
    assert_eq!(reply["submitted"], true, "{text}");
    assert_eq!(code(&text, "rc_written"), Some(0), "{text}");
    assert!(
        after(&text, "rc_written=").lines().nth(1) == Some("written"),
        "{text}"
    );
    assert_eq!(code(&text, "rc_plain"), Some(0), "{text}");

    let target_text = instance.until(&target, "plain line");
    let joined = unwrapped(&target_text);
    assert!(
        joined.contains("[Paneflow: message from writer-pane, surface "),
        "{target_text}"
    );
    assert!(joined.contains("hello from the writer"), "{target_text}");

    assert_eq!(code(&text, "rc_big"), Some(1), "{text}");
    assert!(
        unwrapped(after(&text, "rc_plain=")).contains("pane.write relays at most 16384 bytes"),
        "{text}"
    );
    assert_eq!(code(&text, "rc_self"), Some(1), "{text}");
    assert!(
        after(&text, "rc_big=").contains("cannot write into its own pane"),
        "{text}"
    );
    assert_eq!(code(&text, "rc_foreign"), Some(1), "{text}");
    assert!(
        unwrapped(after(&text, "rc_self=")).contains("ipc.orchestration"),
        "{text}"
    );
    assert_eq!(code(&text, "rc_foreign_all"), Some(1), "{text}");
    assert!(
        unwrapped(after(&text, "rc_foreign=")).contains("ipc.orchestration"),
        "{text}"
    );
    assert!(
        !instance.text(&foreign).contains("over"),
        "{}",
        instance.text(&foreign)
    );

    instance.stop();
}

#[test]
fn ask_shows_the_question_from_the_agent_and_prints_the_answer() {
    if skipped_by_environment() {
        return;
    }
    let mut instance = Instance::open(true);
    let mut window = instance.window();
    let workspace = WorkspaceId::new();

    let lingering = instance.agent_pane(
        "lingering-pane",
        &workspace,
        "paneflow ask 'Anyone there?'\necho \"rc_default=$?\"",
    );
    let body = "paneflow ask 'Deploy now?' --option yes --option no --timeout 60s\necho \"rc_pick=$?\"\n\
                sleep 1.2\n\
                paneflow ask 'Branch name?' --timeout 60s\necho \"rc_input=$?\"\n\
                sleep 1.2\n\
                out=$(paneflow ask 'Later?' --timeout 1s); rc=$?\necho \"left:$out rc_pending=$rc\"\n\
                paneflow ask --resume \"${out#pending }\" --timeout 60s\necho \"rc_resume=$?\"\n\
                echo asker-done";
    let asker = instance.agent_pane("asker-pane", &workspace, body);

    let pick = next_requested(&mut window, "Deploy now?");
    assert_eq!(pick["spec"]["kind"], "pick");
    assert_eq!(pick["spec"]["items"], json!(["yes", "no"]));
    assert_eq!(pick["origin"]["kind"], "agent", "{pick}");
    assert_eq!(pick["origin"]["agent"], "Claude Code", "{pick}");
    assert_eq!(pick["origin"]["session"], asker.to_string(), "{pick}");
    instance.reply(&pick, json!("yes"));
    let text = instance.until(&asker, "rc_pick=");
    assert_eq!(code(&text, "rc_pick"), Some(0), "{text}");
    assert!(text.lines().any(|line| line == "yes"), "{text}");

    let input = next_requested(&mut window, "Branch name?");
    assert_eq!(input["spec"]["kind"], "input");
    instance.reply(&input, json!("feature/relay"));
    let text = instance.until(&asker, "rc_input=");
    assert_eq!(code(&text, "rc_input"), Some(0), "{text}");
    assert!(text.lines().any(|line| line == "feature/relay"), "{text}");

    let later = next_requested(&mut window, "Later?");
    let text = instance.until(&asker, "rc_pending=");
    assert_eq!(code(&text, "rc_pending"), Some(4), "{text}");
    assert!(
        text.contains(&format!("left:pending {} ", later["id"])),
        "{text}"
    );

    let intruder = instance.agent_pane(
        "intruder-pane",
        &workspace,
        &format!(
            "paneflow ask --resume {} --timeout 1s\necho \"rc_foreign=$?\"",
            later["id"]
        ),
    );
    let refused = instance.until(&intruder, "rc_foreign=");
    assert_eq!(code(&refused, "rc_foreign"), Some(1), "{refused}");
    assert!(
        unwrapped(&refused).contains(&format!("no prompt with id {}", later["id"])),
        "{refused}"
    );

    instance.reply(&later, json!("tomorrow"));
    let text = instance.until(&asker, "asker-done");
    assert_eq!(code(&text, "rc_resume"), Some(0), "{text}");
    assert!(
        after(&text, "rc_pending=")
            .lines()
            .any(|line| line == "tomorrow"),
        "{text}"
    );

    let text = instance.until(&lingering, "rc_default=");
    assert_eq!(code(&text, "rc_default"), Some(4), "{text}");
    assert!(
        text.lines().any(|line| line.starts_with("pending ")),
        "{text}"
    );

    let burst = instance.agent_pane(
        "burst-pane",
        &workspace,
        "for n in 1 2 3 4 5 6 7 8; do ( out=$(paneflow ask \"burst $n\" --timeout 0s); echo \"burst:$out rc=$?\" ) & done\nwait\necho burst-done",
    );
    let text = instance.until(&burst, "burst-done");
    let limited = text.matches("burst:rate_limited rc=1").count();
    assert!(limited >= 1, "{text}");
    assert_eq!(
        limited + text.matches("rc=4").count(),
        8,
        "every ask is either pending or rate limited: {text}"
    );

    instance.stop();
}

#[test]
fn ask_with_no_window_or_a_bad_flag_exits_1() {
    if skipped_by_environment() {
        return;
    }
    let home = tempfile::tempdir().unwrap();
    let output = outside(home.path(), &["ask", "Which?", "--multiple"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--option"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut instance = Instance::open(true);
    let pane = instance.pane(
        "unseen-pane",
        &WorkspaceId::new(),
        "paneflow ask 'Anyone?'\necho \"rc_unseen=$?\"",
    );
    let text = instance.until(&pane, "rc_unseen=");
    assert_eq!(code(&text, "rc_unseen"), Some(1), "{text}");
    assert!(
        text.contains("no Paneflow window can show the question"),
        "{text}"
    );
    instance.stop();
}

#[test]
fn do_wait_hands_back_a_run_id_and_resume_prints_the_result() {
    if skipped_by_environment() {
        return;
    }
    let mut instance = Instance::open(true);
    let body = "paneflow do quick --wait 10s\necho \"rc_quick=$?\"\n\
                sleep 1.2\n\
                out=$(paneflow do slow --wait 1s); rc=$?\necho \"left:$out rc_running=$rc\"\n\
                paneflow do --resume \"${out#running }\"\necho \"rc_resume=$?\"\n\
                sleep 1.2\n\
                paneflow do slow --wait 60s\necho \"rc_long=$?\"\n\
                paneflow do secret --wait 1s\necho \"rc_secret_wait=$?\"\n\
                paneflow do secret\necho \"rc_secret=$?\"\n\
                paneflow do --resume no-such-run --wait 1s\necho \"rc_unknown=$?\"\n\
                sleep 1.2\n\
                paneflow do quick\necho \"rc_plain=$?\"\n\
                echo runner-done";
    let runner = instance.agent_pane("runner-pane", &WorkspaceId::new(), body);
    let text = instance.until(&runner, "runner-done");

    assert_eq!(code(&text, "rc_quick"), Some(0), "{text}");
    assert!(text.contains("{\"done\":true}"), "{text}");
    assert_eq!(code(&text, "rc_running"), Some(4), "{text}");
    assert!(text.contains("left:running "), "{text}");
    assert_eq!(code(&text, "rc_resume"), Some(0), "{text}");
    assert!(
        after(&text, "rc_running=").lines().nth(1) == Some("\"slept\""),
        "{text}"
    );
    assert_eq!(code(&text, "rc_long"), Some(0), "{text}");
    assert!(after(&text, "rc_resume=").contains("\"slept\""), "{text}");
    assert_eq!(code(&text, "rc_secret_wait"), Some(1), "{text}");
    assert_eq!(code(&text, "rc_secret"), Some(1), "{text}");
    let with_wait = after(&text, "rc_long=")
        .lines()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    let without = after(&text, "rc_secret_wait=")
        .lines()
        .nth(1)
        .unwrap_or_default()
        .to_string();
    assert!(with_wait.contains("no action named 'secret'"), "{text}");
    assert_eq!(with_wait, without, "{text}");
    assert_eq!(code(&text, "rc_unknown"), Some(1), "{text}");
    assert!(after(&text, "rc_secret=").contains("no-such-run"), "{text}");
    assert_eq!(code(&text, "rc_plain"), Some(0), "{text}");
    assert!(
        after(&text, "rc_unknown=").contains("{\"done\":true}"),
        "{text}"
    );

    instance.stop();
}
