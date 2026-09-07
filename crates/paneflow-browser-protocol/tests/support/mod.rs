use std::error::Error;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use paneflow_browser_protocol::write_message;
use serde_json::{json, Value};

pub type TestResult = Result<(), Box<dyn Error>>;

pub struct Harness {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    next_operation: u64,
}

impl Harness {
    pub fn new(deterministic: bool) -> Result<Self, Box<dyn Error>> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow-browser-harness"));
        command.args(["workspace", "session"]);
        if deterministic {
            command.arg("--deterministic");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().ok_or("missing child stdin")?;
        let output = child.stdout.take().ok_or("missing child stdout")?;
        Ok(Self {
            child,
            input: Some(input),
            output,
            next_operation: 1,
        })
    }

    pub fn send(&mut self, command: Value) -> Result<Value, Box<dyn Error>> {
        let operation = format!("operation-{}", self.next_operation);
        self.next_operation += 1;
        self.send_as(command, &operation)
    }

    pub fn send_as(&mut self, command: Value, operation: &str) -> Result<Value, Box<dyn Error>> {
        let envelope = json!({"version": paneflow_browser_protocol::CONTRACT_VERSION, "operation": operation, "command": command});
        write_message(self.input.as_mut().ok_or("closed stdin")?, &envelope)?;
        let mut header = [0; 4];
        self.output.read_exact(&mut header)?;
        let mut reply = vec![0; u32::from_be_bytes(header) as usize];
        self.output.read_exact(&mut reply)?;
        let reply: Value = serde_json::from_slice(&reply)?;
        assert_eq!(
            reply["version"],
            paneflow_browser_protocol::CONTRACT_VERSION
        );
        assert_eq!(reply["operation"], operation);
        Ok(reply["result"].clone())
    }

    pub fn create(&mut self, browser: &str) -> Result<Value, Box<dyn Error>> {
        let reply = self.send(create("workspace", "session", browser, "profile"))?;
        assert_eq!(reply["Ok"]["type"], "state", "{reply}");
        Ok(reply["Ok"]["session"]["document"].clone())
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn create(workspace: &str, session: &str, browser: &str, profile: &str) -> Value {
    json!({"type": "create", "owner": {"workspace": workspace, "session": session}, "browser": browser, "profile": profile, "url": "https://example.test/", "title": "Example"})
}

pub fn document(workspace: &str, session: &str, browser: &str, generation: u64) -> Value {
    json!({"owner": {"workspace": workspace, "session": session}, "browser": browser, "generation": generation})
}

pub fn command(kind: &str, document: &Value) -> Value {
    json!({"type": kind, "document": document})
}

pub fn present(document: &Value, generation: u64, width: u32, visible: bool) -> Value {
    json!({"type": "present", "document": document, "presentation": {"mounted": visible, "visible": visible, "width": width, "height": 480, "generation": generation}})
}

pub fn frame(document: &Value, pool: u64, buffer: u8, sequence: u64) -> Value {
    json!({"type": "frame", "document": document, "contract_version": paneflow_browser_protocol::CONTRACT_VERSION, "pool_generation": pool, "buffer": buffer, "sequence": sequence})
}

pub fn release(document: &Value, pool: u64, buffer: u8, sequence: u64) -> Value {
    json!({"type": "release_frame", "document": document, "pool_generation": pool, "buffer": buffer, "sequence": sequence})
}

pub fn operation(document: &Value, mutation: bool) -> Value {
    json!({"type": "begin_operation", "document": document, "mutation": mutation, "text": ""})
}

pub fn assert_error(reply: &Value, error: &str) {
    assert_eq!(reply["Err"], error, "{reply}");
}

pub fn raw(bytes: Vec<u8>) -> Result<Output, Box<dyn Error>> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_paneflow-browser-harness"))
        .args(["workspace", "session", "--deterministic"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut input = child.stdin.take().ok_or("missing child stdin")?;
    let writer = std::thread::spawn(move || input.write_all(&bytes));
    let output = child.wait_with_output()?;
    let _ = writer.join().map_err(|_| "writer thread failed")?;
    Ok(output)
}

struct FixtureDirectory(PathBuf);

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

pub fn batch(channels: Vec<(&str, &str, Vec<Value>)>) -> Result<Vec<Value>, Box<dyn Error>> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let directory = FixtureDirectory(std::env::temp_dir().join(format!(
        "paneflow-browser-protocol-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )));
    fs::create_dir(&directory.0)?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_paneflow-browser-harness"));
    command.arg("--batch");
    for (index, (workspace, session, commands)) in channels.into_iter().enumerate() {
        let path = directory.0.join(format!("{index}.frames"));
        let mut file = File::create(&path)?;
        for (number, value) in commands.into_iter().enumerate() {
            write_message(
                &mut file,
                &json!({"version": paneflow_browser_protocol::CONTRACT_VERSION, "operation": format!("channel-{index}-op-{number}"), "command": value}),
            )?;
        }
        command.args([workspace, session]).arg(path);
    }
    let output = command.arg("--deterministic").output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut bytes = output.stdout.as_slice();
    let mut replies = Vec::new();
    while !bytes.is_empty() {
        let mut header = [0; 4];
        bytes.read_exact(&mut header)?;
        let mut body = vec![0; u32::from_be_bytes(header) as usize];
        bytes.read_exact(&mut body)?;
        let reply: Value = serde_json::from_slice(&body)?;
        replies.push(reply["result"].clone());
    }
    Ok(replies)
}
