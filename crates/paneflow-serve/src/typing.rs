use std::path::PathBuf;
use std::time::Duration;

use paneflow_config::schema::SessionId;
use paneflow_host::HostClient;
use paneflow_host::protocol::ClientHello;

const LAUNCH_FLOOR: Duration = Duration::from_millis(700);
const LAUNCH_MAX: Duration = Duration::from_millis(4000);
const LAUNCH_POLL: Duration = Duration::from_millis(100);
const PREFILL_FLOOR: Duration = Duration::from_millis(1800);
const PREFILL_MAX: Duration = Duration::from_millis(8000);
const PREFILL_POLL: Duration = Duration::from_millis(200);
const HOST_CLIENT: &str = "paneflow-serve-typing";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Typed {
    pub session: SessionId,
    pub command: Option<String>,
    pub prompt: Option<String>,
}

impl Typed {
    pub fn new(
        session: SessionId,
        command: Option<String>,
        prompt: Option<String>,
    ) -> Option<Self> {
        let command = command.filter(|text| !text.is_empty());
        let prompt = prompt.filter(|text| !text.is_empty());
        (command.is_some() || prompt.is_some()).then_some(Self {
            session,
            command,
            prompt,
        })
    }
}

pub fn prompt_input(prompt: &str) -> String {
    if paneflow_ipc_client::send_text::text_contains_submit_byte(prompt) {
        paneflow_ipc_client::send_text::bracketed_paste_frame(prompt)
    } else {
        prompt.to_string()
    }
}

pub fn type_when_settled(core_endpoint: PathBuf, typed: Typed) {
    let spawned = std::thread::Builder::new()
        .name("paneflow-serve-typing".into())
        .spawn(move || {
            if let Err(error) = type_now(&core_endpoint, &typed) {
                log::warn!(
                    "paneflow-serve: the launch input for session {} was not typed: {error}",
                    typed.session
                );
            }
        });
    if let Err(error) = spawned {
        log::warn!("paneflow-serve: cannot start the typing thread: {error}");
    }
}

fn type_now(core_endpoint: &std::path::Path, typed: &Typed) -> Result<(), String> {
    let mut client = HostClient::connect(core_endpoint, &ClientHello::local(HOST_CLIENT))
        .map_err(|error| error.to_string())?;
    if let Some(command) = &typed.command {
        let settled = wait_for_settle(
            &mut client,
            &typed.session,
            LAUNCH_FLOOR,
            LAUNCH_MAX,
            LAUNCH_POLL,
        );
        if !settled {
            log::warn!(
                "paneflow-serve: session {} still producing output after {LAUNCH_MAX:?}; launch command sent best-effort",
                typed.session
            );
        }
        let mut bytes = command.as_bytes().to_vec();
        bytes.push(b'\r');
        write(&mut client, &typed.session, &bytes)?;
    }
    if let Some(prompt) = &typed.prompt {
        let settled = wait_for_settle(
            &mut client,
            &typed.session,
            PREFILL_FLOOR,
            PREFILL_MAX,
            PREFILL_POLL,
        );
        if !settled {
            log::warn!(
                "paneflow-serve: session {} still producing output after {PREFILL_MAX:?}; prompt prefilled best-effort",
                typed.session
            );
        }
        write(&mut client, &typed.session, prompt_input(prompt).as_bytes())?;
    }
    Ok(())
}

fn write(client: &mut HostClient, session: &SessionId, bytes: &[u8]) -> Result<(), String> {
    let generation = client
        .inspect(session)
        .map_err(|error| error.to_string())?
        .manifest
        .generation;
    client
        .input(session, generation, bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn output_mark(client: &mut HostClient, session: &SessionId) -> Option<String> {
    client.text(session).ok().map(|reply| reply.text)
}

fn wait_for_settle(
    client: &mut HostClient,
    session: &SessionId,
    floor: Duration,
    max: Duration,
    poll: Duration,
) -> bool {
    std::thread::sleep(floor);
    let mut last = output_mark(client, session);
    let mut waited = floor;
    while waited < max {
        std::thread::sleep(poll);
        waited += poll;
        let now = output_mark(client, session);
        if now == last {
            return true;
        }
        last = now;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_to_type_schedules_nothing() {
        let session = SessionId::new();
        assert_eq!(Typed::new(session.clone(), None, Some(String::new())), None);
        let typed = Typed::new(session, Some("claude".into()), None).expect("a command");
        assert_eq!(typed.prompt, None);
    }

    #[test]
    fn a_prompt_with_a_newline_is_pasted_so_it_does_not_submit_early() {
        assert_eq!(prompt_input("review"), "review");
        let pasted = prompt_input("line one\nline two");
        assert!(pasted.starts_with("\u{1b}[200~"), "{pasted:?}");
    }
}
