use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use paneflow_lsp::{ClientEvent, LanguageClient, OpenDocument, ServerKey, ServerSpec, ServerState};
use ropey::Rope;
use serde_json::Value;

use crate::fake_server::{CAPS_ENV, LOG_ENV, SCENARIO_ENV};

pub const OUTCOME_DEADLINE: Duration = Duration::from_secs(60);

pub struct Fixture {
    pub _dir: tempfile::TempDir,
    pub root: PathBuf,
    pub log: PathBuf,
    pub client: LanguageClient,
    pub events: Receiver<ClientEvent>,
}

pub fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = paneflow_lsp::trust::canonical_root(dir.path())
        .unwrap()
        .join("project");
    std::fs::create_dir_all(&root).unwrap();
    let log = dir.path().join("fake.log");
    let (sender, events) = mpsc::channel();
    let sender = Mutex::new(sender);
    let client = LanguageClient::start(Arc::new(move |event| {
        let _ = sender.lock().unwrap().send(event);
    }))
    .unwrap();
    Fixture {
        _dir: dir,
        root,
        log,
        client,
        events,
    }
}

impl Fixture {
    pub fn key(&self, server_id: &str) -> ServerKey {
        ServerKey {
            server_id: server_id.to_string(),
            root: self.root.clone(),
        }
    }

    pub fn spec(&self, scenario: &str, capabilities: Option<Value>) -> ServerSpec {
        let mut env = vec![
            (SCENARIO_ENV.to_string(), scenario.to_string()),
            (LOG_ENV.to_string(), self.log.to_string_lossy().into_owned()),
        ];
        if let Some(capabilities) = capabilities {
            env.push((CAPS_ENV.to_string(), capabilities.to_string()));
        }
        ServerSpec {
            command: std::env::current_exe()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            args: Vec::new(),
            env,
            initialization_options: None,
            settings: Value::Null,
        }
    }

    pub fn document(&self, name: &str, text: &str) -> OpenDocument {
        OpenDocument {
            path: self.root.join(name),
            language_id: "rust".to_string(),
            text: Rope::from_str(text),
        }
    }

    pub fn entries(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect()
    }

    pub fn wait_for_log(&self, what: &str, done: impl Fn(&[Value]) -> bool) -> Vec<Value> {
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let entries = self.entries();
            if done(&entries) {
                return entries;
            }
            assert!(
                Instant::now() < deadline,
                "the fake server log never showed {what}: {entries:#?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn wait_for_event(
        &self,
        what: &str,
        matches: impl Fn(&ClientEvent) -> bool,
    ) -> ClientEvent {
        let deadline = Instant::now() + OUTCOME_DEADLINE;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(event) if matches(&event) => return event,
                Ok(_) => {}
                Err(_) => panic!("the client never reported {what}"),
            }
        }
    }

    pub fn wait_for_state(
        &self,
        key: &ServerKey,
        wanted: impl Fn(&ServerState) -> bool,
    ) -> ServerState {
        match self.wait_for_event("the expected server state", |event| {
            matches!(event, ClientEvent::State { key: at, state } if at == key && wanted(state))
        }) {
            ClientEvent::State { state, .. } => state,
            _ => unreachable!(),
        }
    }
}

pub fn messages<'a>(entries: &'a [Value], method: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    entries
        .iter()
        .filter(|entry| entry["event"] == "message")
        .map(|entry| &entry["message"])
        .filter(move |message| message["method"] == method)
}

pub fn count(entries: &[Value], method: &str) -> usize {
    messages(entries, method).count()
}

pub fn spawns(entries: &[Value]) -> Vec<u64> {
    entries
        .iter()
        .filter(|entry| entry["event"] == "spawn")
        .filter_map(|entry| entry["pid"].as_u64())
        .collect()
}

pub fn method_order(entries: &[Value]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| entry["event"] == "message")
        .filter_map(|entry| entry["message"]["method"].as_str().map(str::to_string))
        .collect()
}

pub fn executable_runs(program: &Path) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

struct CapturingLogger(Mutex<Vec<String>>);

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if record.level() == log::Level::Debug {
            self.0.lock().unwrap().push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

fn logger() -> &'static CapturingLogger {
    static LOGGER: std::sync::OnceLock<&'static CapturingLogger> = std::sync::OnceLock::new();
    LOGGER.get_or_init(|| {
        let logger: &'static CapturingLogger =
            Box::leak(Box::new(CapturingLogger(Mutex::new(Vec::new()))));
        let _ = log::set_logger(logger);
        log::set_max_level(log::LevelFilter::Debug);
        logger
    })
}

pub fn capture_debug_logs() {
    logger();
}

pub fn debug_lines_containing(text: &str) -> usize {
    logger()
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.contains(text))
        .count()
}
