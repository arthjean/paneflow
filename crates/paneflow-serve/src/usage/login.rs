use std::path::Path;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use paneflow_agent_config::usage::accounts::UsageAgent;
use paneflow_process::{LineSession, ProcError, run_with_timeout, spawn_line_session};
use serde_json::{Value, json};

use super::probe::{ProbeProgram, agent_command, binary_name, grok_signed_in};

const LOGIN_LINE_CAP: usize = 64 * 1024;
const LOGIN_POLL: Duration = Duration::from_millis(200);
const LOGIN_DEADLINE: Duration = Duration::from_secs(15 * 60);
const LOGIN_EXIT_GRACE: Duration = Duration::from_secs(2);
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(30);
const CANCEL_DEADLINE: Duration = Duration::from_secs(5);
const STATUS_DEADLINE: Duration = Duration::from_secs(30);
const STATUS_STDOUT_CAP: u64 = 64 * 1024;
const CODEX_INIT_ID: u64 = 1;
const CODEX_LOGIN_ID: u64 = 2;
const CODEX_CANCEL_ID: u64 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginInput {
    Code(String),
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEnd {
    SignedIn,
    Canceled,
    Failed(String),
}

enum Wait {
    Line(Value),
    Canceled,
    Failed(String),
}

pub fn run_login(
    agent: UsageAgent,
    program: &ProbeProgram,
    config_dir: &Path,
    user_home: Option<&Path>,
    inputs: &Receiver<LoginInput>,
    on_link: impl FnMut(String),
) -> LoginEnd {
    let mut command = agent_command(agent, program, config_dir, user_home);
    match agent {
        UsageAgent::ClaudeCode => command.args(["auth", "login"]),
        UsageAgent::Codex => command.arg("app-server"),
        UsageAgent::Grok => command.arg("login"),
    };
    let mut session = match spawn_line_session(command, LOGIN_LINE_CAP) {
        Ok(session) => session,
        Err(error) => {
            return LoginEnd::Failed(format!("{} could not start: {error}", binary_name(agent)));
        }
    };
    let end = match agent {
        UsageAgent::ClaudeCode => cli_login(&mut session, inputs, on_link, "Claude Code"),
        UsageAgent::Grok => cli_login(&mut session, inputs, on_link, "Grok"),
        UsageAgent::Codex => codex_login(&mut session, inputs, on_link),
    };
    session.close(LOGIN_EXIT_GRACE);
    match (agent, end) {
        (UsageAgent::ClaudeCode, Ok(last_line)) => {
            match claude_signed_in(program, config_dir, user_home) {
                Some(true) => LoginEnd::SignedIn,
                _ => LoginEnd::Failed(
                    last_line.unwrap_or_else(|| "Claude Code ended the sign-in".to_string()),
                ),
            }
        }
        (UsageAgent::Grok, Ok(last_line)) => {
            match grok_signed_in(program, config_dir, user_home, STATUS_DEADLINE) {
                Ok(Some(true)) => LoginEnd::SignedIn,
                _ => LoginEnd::Failed(
                    last_line.unwrap_or_else(|| "Grok ended the sign-in".to_string()),
                ),
            }
        }
        (UsageAgent::Codex, Ok(_)) => LoginEnd::SignedIn,
        (_, Err(end)) => end,
    }
}

fn cli_login(
    session: &mut LineSession,
    inputs: &Receiver<LoginInput>,
    mut on_link: impl FnMut(String),
    name: &str,
) -> Result<Option<String>, LoginEnd> {
    let started = Instant::now();
    let mut linked = false;
    let mut last_line = None;
    loop {
        match inputs.try_recv() {
            Ok(LoginInput::Code(code)) => {
                if session.send_line(code.trim().as_bytes()).is_err() {
                    return Err(LoginEnd::Failed(format!("{name} stopped reading the code")));
                }
            }
            Ok(LoginInput::Cancel) | Err(TryRecvError::Disconnected) => {
                return Err(LoginEnd::Canceled);
            }
            Err(TryRecvError::Empty) => {}
        }
        if started.elapsed() > LOGIN_DEADLINE {
            return Err(LoginEnd::Failed("The sign-in timed out".to_string()));
        }
        match session.next_line(LOGIN_POLL) {
            Ok(Some(line)) => {
                let text = visible_text(&line);
                if !linked && let Some(link) = first_link(&text) {
                    linked = true;
                    on_link(link);
                }
                let text = text.trim();
                if !text.is_empty() {
                    last_line = Some(text.chars().take(256).collect());
                }
            }
            Err(ProcError::Timeout) => {}
            Ok(None) | Err(_) => return Ok(last_line),
        }
    }
}

fn claude_signed_in(
    program: &ProbeProgram,
    config_dir: &Path,
    user_home: Option<&Path>,
) -> Option<bool> {
    let mut command = agent_command(UsageAgent::ClaudeCode, program, config_dir, user_home);
    command.args(["auth", "status", "--json"]);
    let output = run_with_timeout(command, STATUS_DEADLINE, STATUS_STDOUT_CAP).ok()?;
    serde_json::from_slice::<Value>(&output.stdout)
        .ok()?
        .get("loggedIn")?
        .as_bool()
}

fn codex_login(
    session: &mut LineSession,
    inputs: &Receiver<LoginInput>,
    mut on_link: impl FnMut(String),
) -> Result<Option<String>, LoginEnd> {
    send(
        session,
        &json!({
            "jsonrpc": "2.0",
            "id": CODEX_INIT_ID,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "paneflow",
                    "title": "Paneflow",
                    "version": env!("CARGO_PKG_VERSION"),
                },
                "capabilities": {"experimentalApi": true},
            },
        }),
    )?;
    let deadline = Instant::now() + HANDSHAKE_DEADLINE;
    codex_result(&await_codex(session, inputs, deadline, |message| {
        reply_to(message, CODEX_INIT_ID)
    })?)?;
    send(session, &json!({"jsonrpc": "2.0", "method": "initialized"}))?;
    send(
        session,
        &json!({
            "jsonrpc": "2.0",
            "id": CODEX_LOGIN_ID,
            "method": "account/login/start",
            "params": {"type": "chatgpt"},
        }),
    )?;
    let started = await_codex(session, inputs, deadline, |message| {
        reply_to(message, CODEX_LOGIN_ID)
    })?;
    let started = codex_result(&started)?;
    let login_id = started
        .get("loginId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let link = started
        .get("authUrl")
        .and_then(Value::as_str)
        .filter(|link| link.starts_with("https://"))
        .ok_or_else(|| LoginEnd::Failed("Codex returned no sign-in page".to_string()))?;
    on_link(link.to_string());
    let completed = |message: &Value| {
        message.get("method").and_then(Value::as_str) == Some("account/login/completed")
            && message
                .pointer("/params/loginId")
                .and_then(Value::as_str)
                .is_none_or(|id| Some(id) == login_id.as_deref())
    };
    let waited = await_codex(session, inputs, Instant::now() + LOGIN_DEADLINE, completed);
    let message = match waited {
        Ok(message) => message,
        Err(end) => {
            let cancel = json!({
                "jsonrpc": "2.0",
                "id": CODEX_CANCEL_ID,
                "method": "account/login/cancel",
                "params": {"loginId": login_id},
            });
            if send(session, &cancel).is_ok() {
                let (_open, quiet) = std::sync::mpsc::channel();
                let _ = await_codex(session, &quiet, Instant::now() + CANCEL_DEADLINE, completed);
            }
            return Err(end);
        }
    };
    if message.pointer("/params/success").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    Err(LoginEnd::Failed(
        message
            .pointer("/params/error")
            .and_then(Value::as_str)
            .unwrap_or("Codex did not finish the sign-in")
            .chars()
            .take(256)
            .collect(),
    ))
}

fn send(session: &mut LineSession, message: &Value) -> Result<(), LoginEnd> {
    session
        .send_line(message.to_string().as_bytes())
        .map_err(|error| LoginEnd::Failed(format!("Codex stopped answering: {error}")))
}

fn reply_to(message: &Value, id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(id)
}

fn codex_result(message: &Value) -> Result<&Value, LoginEnd> {
    if let Some(error) = message.get("error") {
        return Err(LoginEnd::Failed(
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("Codex refused the sign-in")
                .chars()
                .take(256)
                .collect(),
        ));
    }
    message
        .get("result")
        .ok_or_else(|| LoginEnd::Failed("Codex sent an answer Paneflow cannot read".to_string()))
}

fn await_codex(
    session: &mut LineSession,
    inputs: &Receiver<LoginInput>,
    deadline: Instant,
    accept: impl Fn(&Value) -> bool,
) -> Result<Value, LoginEnd> {
    loop {
        match next_codex(session, inputs, deadline) {
            Wait::Line(message) if accept(&message) => return Ok(message),
            Wait::Line(_) => {}
            Wait::Canceled => return Err(LoginEnd::Canceled),
            Wait::Failed(reason) => return Err(LoginEnd::Failed(reason)),
        }
    }
}

fn next_codex(session: &mut LineSession, inputs: &Receiver<LoginInput>, deadline: Instant) -> Wait {
    loop {
        match inputs.try_recv() {
            Ok(LoginInput::Cancel) | Err(TryRecvError::Disconnected) => return Wait::Canceled,
            Ok(LoginInput::Code(_)) | Err(TryRecvError::Empty) => {}
        }
        if Instant::now() > deadline {
            return Wait::Failed("The sign-in timed out".to_string());
        }
        match session.next_line(LOGIN_POLL) {
            Ok(Some(line)) => {
                if let Ok(message) = serde_json::from_slice::<Value>(&line) {
                    return Wait::Line(message);
                }
            }
            Err(ProcError::Timeout) => {}
            Ok(None) | Err(_) => {
                return Wait::Failed("Codex stopped before the sign-in finished".to_string());
            }
        }
    }
}

pub fn visible_text(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    let mut visible = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if !c.is_control() || c == '\t' {
                visible.push(c);
            }
            continue;
        }
        match chars.next() {
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        if chars.peek() == Some(&'\\') {
                            chars.next();
                        }
                        break;
                    }
                }
            }
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    visible
}

pub fn first_link(text: &str) -> Option<String> {
    let start = text.find("https://")?;
    let link: String = text[start..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    (link.len() > "https://".len()).then_some(link)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_link_is_read_through_terminal_hyperlinks() {
        let line = b"\x1b]8;;https://claude.ai/oauth/authorize?code=true\x07https://claude.ai/oauth/authorize?code=true\x1b]8;;\x07";
        let text = visible_text(line);
        assert_eq!(text, "https://claude.ai/oauth/authorize?code=true");
        assert_eq!(
            first_link(&text).as_deref(),
            Some("https://claude.ai/oauth/authorize?code=true")
        );
        assert_eq!(
            visible_text(b"\x1b[1mbold\x1b[0m \x1b]0;title\x1b\\x"),
            "bold x"
        );
        assert_eq!(first_link("visit http://example.com"), None);
        assert_eq!(first_link("https://"), None);
    }

    #[cfg(unix)]
    mod process {
        use std::path::PathBuf;
        use std::sync::mpsc;

        use super::super::*;

        struct Fake {
            dir: tempfile::TempDir,
            program: ProbeProgram,
        }

        fn fake(script: &str) -> Fake {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("fake.sh");
            std::fs::write(
                &path,
                script.replace("$DIR", &dir.path().display().to_string()),
            )
            .unwrap();
            Fake {
                program: ProbeProgram {
                    program: PathBuf::from("/bin/sh"),
                    leading_args: vec![path.into_os_string()],
                    work_dir: dir.path().to_path_buf(),
                },
                dir,
            }
        }

        const CLAUDE: &str = r#"
case "$*" in
  "auth login")
    printf '%s\n' "$CLAUDE_CONFIG_DIR" > "$DIR/config_dir"
    printf 'Opening browser to sign in\nvisit:\n\033]8;;https://claude.ai/oauth/authorize?x=1\007https://claude.ai/oauth/authorize?x=1\033]8;;\007\nPaste code here if prompted > '
    read -r code
    printf '%s' "$code" > "$DIR/code"
    echo "Login successful."
    ;;
  "auth status --json")
    if [ -f "$DIR/code" ]; then echo '{"loggedIn": true}'; else echo '{"loggedIn": false}'; exit 1; fi
    ;;
esac
"#;

        #[test]
        fn claude_signs_in_with_the_pasted_code_and_confirms_through_auth_status() {
            let fake = fake(CLAUDE);
            let config = fake.dir.path().join("account");
            std::fs::create_dir(&config).unwrap();
            let (sender, inputs) = mpsc::channel();
            let mut links = Vec::new();
            let end = run_login(
                UsageAgent::ClaudeCode,
                &fake.program,
                &config,
                None,
                &inputs,
                |link| {
                    links.push(link);
                    sender
                        .send(LoginInput::Code(" abc#state \n".into()))
                        .unwrap();
                },
            );
            assert_eq!(end, LoginEnd::SignedIn);
            assert_eq!(links, ["https://claude.ai/oauth/authorize?x=1"]);
            let code = std::fs::read_to_string(fake.dir.path().join("code")).unwrap();
            assert_eq!(code, "abc#state");
            let dir = std::fs::read_to_string(fake.dir.path().join("config_dir")).unwrap();
            assert_eq!(dir.trim(), config.display().to_string());
        }

        #[test]
        fn a_canceled_claude_sign_in_stops_the_login_and_reports_no_failure() {
            let fake = fake(CLAUDE);
            let (sender, inputs) = mpsc::channel();
            let end = run_login(
                UsageAgent::ClaudeCode,
                &fake.program,
                fake.dir.path(),
                None,
                &inputs,
                |_| sender.send(LoginInput::Cancel).unwrap(),
            );
            assert_eq!(end, LoginEnd::Canceled);
            let code = std::fs::read_to_string(fake.dir.path().join("code")).unwrap_or_default();
            assert!(code.is_empty(), "no code reaches a canceled login");
        }

        const GROK: &str = r#"
case "$*" in
  "login")
    printf '%s\n' "$GROK_HOME" > "$DIR/grok_home"
    printf '\nTo sign in, open this URL in your browser:\n\n  https://accounts.x.ai/oauth2/device?user_code=AAAA-BBBB\n\nWaiting for authorization...\n'
    if [ -f "$DIR/refuse" ]; then echo "Authorization was denied."; exit 1; fi
    touch "$DIR/signed"
    ;;
  "models")
    if [ -f "$DIR/signed" ]; then echo "You are logged in with grok.com."; else echo "You are not authenticated."; fi
    ;;
esac
"#;

        #[test]
        fn grok_signs_in_through_its_own_login_and_confirms_through_its_model_list() {
            let fake = fake(GROK);
            let config = fake.dir.path().join("account");
            std::fs::create_dir(&config).unwrap();
            let (_sender, inputs) = mpsc::channel();
            let mut links = Vec::new();
            let end = run_login(
                UsageAgent::Grok,
                &fake.program,
                &config,
                None,
                &inputs,
                |link| links.push(link),
            );
            assert_eq!(end, LoginEnd::SignedIn);
            assert_eq!(
                links,
                ["https://accounts.x.ai/oauth2/device?user_code=AAAA-BBBB"]
            );
            let home = std::fs::read_to_string(fake.dir.path().join("grok_home")).unwrap();
            assert_eq!(home.trim(), config.display().to_string());
            let probed = crate::usage::probe::probe(
                UsageAgent::Grok,
                &fake.program,
                &config,
                None,
                STATUS_DEADLINE,
                0,
            )
            .unwrap();
            assert_eq!(
                probed,
                crate::usage::probe::ProbeOutcome::IdentityOnly {
                    agent_version: None,
                    identity: crate::usage::probe::AccountIdentity {
                        email: None,
                        plan: Some("Grok account".to_string()),
                    },
                },
                "a signed-in folder with no readable identity still counts as signed in"
            );
        }

        #[test]
        fn a_refused_grok_sign_in_reports_what_grok_said_and_probes_as_signed_out() {
            let fake = fake(GROK);
            std::fs::write(fake.dir.path().join("refuse"), "").unwrap();
            let (_sender, inputs) = mpsc::channel();
            let end = run_login(
                UsageAgent::Grok,
                &fake.program,
                fake.dir.path(),
                None,
                &inputs,
                |_| {},
            );
            assert_eq!(
                end,
                LoginEnd::Failed("Authorization was denied.".to_string())
            );
            let probed = crate::usage::probe::probe(
                UsageAgent::Grok,
                &fake.program,
                fake.dir.path(),
                None,
                STATUS_DEADLINE,
                0,
            )
            .unwrap();
            assert_eq!(
                probed,
                crate::usage::probe::ProbeOutcome::IdentityOnly {
                    agent_version: None,
                    identity: crate::usage::probe::AccountIdentity::default(),
                }
            );
        }

        fn codex(login_reply: &str, cancel_reply: &str) -> Fake {
            fake(&format!(
                r#"
printf '%s\n' "$*" > "$DIR/args"
while IFS= read -r l; do
  case "$l" in
    *'"initialize"'*) echo '{{"id":1,"result":{{"userAgent":"paneflow/0.165.0"}}}}' ;;
    *'account/login/start'*) printf '%s\n' {login_reply} ;;
    *'account/login/cancel'*) printf '%s\n' {cancel_reply} ;;
  esac
done
"#
            ))
        }

        const STARTED: &str = r#"'{"id":2,"result":{"type":"chatgpt","loginId":"L1","authUrl":"https://auth.openai.com/oauth/authorize?x=1"}}'"#;

        #[test]
        fn codex_reports_the_page_to_open_and_the_completed_sign_in() {
            let fake = codex(
                &format!(
                    r#"{STARTED} '{{"method":"account/login/completed","params":{{"loginId":"other","success":false}}}}' '{{"method":"account/login/completed","params":{{"loginId":"L1","success":true}}}}'"#
                ),
                "''",
            );
            let (_sender, inputs) = mpsc::channel();
            let mut links = Vec::new();
            let end = run_login(
                UsageAgent::Codex,
                &fake.program,
                fake.dir.path(),
                None,
                &inputs,
                |link| links.push(link),
            );
            assert_eq!(end, LoginEnd::SignedIn);
            assert_eq!(links, ["https://auth.openai.com/oauth/authorize?x=1"]);
            let args = std::fs::read_to_string(fake.dir.path().join("args")).unwrap();
            assert_eq!(args.trim(), "app-server");
        }

        #[test]
        fn a_canceled_codex_sign_in_asks_codex_to_stop_its_login() {
            let fake = codex(
                STARTED,
                r#"'{"id":3,"result":{"status":"canceled"}}' '{"method":"account/login/completed","params":{"loginId":"L1","success":false,"error":"Login was not completed"}}'"#,
            );
            let (sender, inputs) = mpsc::channel();
            let end = run_login(
                UsageAgent::Codex,
                &fake.program,
                fake.dir.path(),
                None,
                &inputs,
                |_| sender.send(LoginInput::Cancel).unwrap(),
            );
            assert_eq!(end, LoginEnd::Canceled);
        }

        #[test]
        fn a_failed_codex_sign_in_carries_codex_reason() {
            let fake = codex(
                &format!(
                    r#"{STARTED} '{{"method":"account/login/completed","params":{{"loginId":"L1","success":false,"error":"Login server error"}}}}'"#
                ),
                "''",
            );
            let (_sender, inputs) = mpsc::channel();
            let end = run_login(
                UsageAgent::Codex,
                &fake.program,
                fake.dir.path(),
                None,
                &inputs,
                |_| {},
            );
            assert_eq!(end, LoginEnd::Failed("Login server error".to_string()));
        }
    }
}
