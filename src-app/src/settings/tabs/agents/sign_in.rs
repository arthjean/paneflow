use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use gpui::{
    AnyElement, ClickEvent, Context, InteractiveElement, IntoElement, KeyDownEvent, ParentElement,
    SharedString, Styled, div, px,
};
use paneflow_agent_config::usage::accounts::{UsageAgent, account_dir_key, canonical_account_dir};
use paneflow_config::schema::SpaceId;
use paneflow_serve::usage::login::{LoginEnd, LoginInput, run_login};
use paneflow_serve::usage::probe::{ProbeProgram, probe_work_dir, resolve_binary};

use super::{input_box_sized, usage_agent};
use crate::PaneFlowApp;
use crate::agent_launcher::TerminalAgent;
use crate::settings::components::{ModalKey, modal_key};
use crate::settings::kit::{self, Ink, Size, Text, Tone};
use crate::ui_primitives::AccessibleControlExt as _;

const CODE_FIELD_HEIGHT: f32 = 28.;
const NEW_ACCOUNT_TITLE: &str = "New account";
const DISCARD_ATTEMPTS: u32 = 5;
const DISCARD_RETRY: Duration = Duration::from_millis(300);

pub(crate) struct NewAccount {
    pub(crate) dir: PathBuf,
    pub(crate) marker_dir: Option<PathBuf>,
    pub(crate) space: Option<(SpaceId, Option<String>)>,
}

pub(crate) struct AgentSignIn {
    pub(crate) agent: TerminalAgent,
    pub(crate) key: String,
    link: Option<String>,
    code_open: bool,
    inputs: mpsc::Sender<LoginInput>,
    new_account: Option<NewAccount>,
    discard: Arc<AtomicBool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SignInProgress {
    pub(crate) linked: bool,
    pub(crate) code_open: bool,
}

enum SignInEvent {
    Link(String),
    Ended(LoginEnd),
}

fn login_program(agent: UsageAgent) -> Result<(ProbeProgram, Option<PathBuf>), String> {
    let paneflow_home = paneflow_home::paneflow_home()
        .ok_or_else(|| "the Paneflow home directory cannot be resolved".to_string())?;
    let binary = resolve_binary(agent, &paneflow_home).map_err(|error| error.to_string())?;
    let work_dir = probe_work_dir(&paneflow_home).map_err(|error| error.to_string())?;
    Ok((ProbeProgram::binary(binary, work_dir), dirs::home_dir()))
}

fn discards_new_account(discard: bool, end: &LoginEnd) -> bool {
    discard && *end != LoginEnd::SignedIn
}

fn remove_new_account_folders(folders: &[PathBuf]) {
    for folder in folders {
        remove_folder(folder);
    }
}

fn remove_folder(folder: &Path) {
    for attempt in 1..=DISCARD_ATTEMPTS {
        match std::fs::remove_dir_all(folder) {
            Ok(()) => return,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(error) if attempt == DISCARD_ATTEMPTS => {
                log::warn!(
                    "could not remove the canceled account folder {}: {error}",
                    folder.display()
                );
            }
            Err(_) => std::thread::sleep(DISCARD_RETRY),
        }
    }
}

fn ended_message(agent: TerminalAgent, end: &LoginEnd) -> Option<String> {
    match end {
        LoginEnd::SignedIn => Some(format!(
            "Signed in to {}. Paneflow is reading the account.",
            agent.display_name()
        )),
        LoginEnd::Canceled => None,
        LoginEnd::Failed(reason) => Some(format!("The sign-in did not finish: {reason}")),
    }
}

impl PaneFlowApp {
    pub(crate) fn start_agent_sign_in(
        &mut self,
        agent: TerminalAgent,
        dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.begin_agent_sign_in(agent, dir, None, cx);
    }

    pub(super) fn begin_agent_sign_in(
        &mut self,
        agent: TerminalAgent,
        dir: PathBuf,
        new_account: Option<NewAccount>,
        cx: &mut Context<Self>,
    ) {
        let Some(usage) = usage_agent(agent) else {
            return;
        };
        if self.agent_sign_in.is_some() {
            self.show_toast("Finish or cancel the sign-in in progress first", cx);
            return;
        }
        let key = account_dir_key(&canonical_account_dir(&dir));
        self.usage
            .forget_account_attempts(|(attempted, dir_key)| *attempted == usage && *dir_key == key);
        let (inputs, receiver) = mpsc::channel();
        let (events, event_rx) = smol::channel::unbounded();
        let discard = Arc::new(AtomicBool::new(false));
        let thread_discard = discard.clone();
        let folders: Vec<PathBuf> = new_account
            .iter()
            .flat_map(|account| {
                std::iter::once(account.dir.clone()).chain(account.marker_dir.clone())
            })
            .collect();
        let spawned = std::thread::Builder::new()
            .name("paneflow-agent-sign-in".to_string())
            .spawn(move || {
                let end = match login_program(usage) {
                    Ok((program, user_home)) => run_login(
                        usage,
                        &program,
                        &dir,
                        user_home.as_deref(),
                        &receiver,
                        |link| {
                            let _ = events.try_send(SignInEvent::Link(link));
                        },
                    ),
                    Err(reason) => LoginEnd::Failed(reason),
                };
                if discards_new_account(thread_discard.load(Ordering::SeqCst), &end) {
                    remove_new_account_folders(&folders);
                }
                let _ = events.try_send(SignInEvent::Ended(end));
            });
        if let Err(error) = spawned {
            self.show_toast(format!("Could not start the sign-in: {error}"), cx);
            return;
        }
        self.agent_profile_menu = None;
        self.agent_sign_in_code_input
            .update(cx, |input, cx| input.clear(cx));
        self.agent_sign_in = Some(AgentSignIn {
            agent,
            key: key.clone(),
            link: None,
            code_open: false,
            inputs,
            new_account,
            discard,
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = event_rx.recv().await {
                let applied = this.update(cx, |this, cx| this.apply_sign_in_event(&key, event, cx));
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply_sign_in_event(&mut self, key: &str, event: SignInEvent, cx: &mut Context<Self>) {
        let Some(sign_in) = self
            .agent_sign_in
            .as_mut()
            .filter(|sign_in| sign_in.key == key)
        else {
            return;
        };
        match event {
            SignInEvent::Link(link) => {
                if sign_in.agent != TerminalAgent::ClaudeCode {
                    self.open_sign_in_page(&link, cx);
                }
                if let Some(sign_in) = self.agent_sign_in.as_mut() {
                    sign_in.link = Some(link);
                }
            }
            SignInEvent::Ended(end) => {
                let agent = sign_in.agent;
                self.agent_sign_in = None;
                self.agent_sign_in_code_input
                    .update(cx, |input, cx| input.clear(cx));
                if let Some(usage) = usage_agent(agent) {
                    self.usage.forget_account_attempts(|(attempted, dir_key)| {
                        *attempted == usage && dir_key == key
                    });
                }
                self.refresh_usage_inventory(true, cx);
                self.check_agent_accounts(cx);
                if let Some(message) = ended_message(agent, &end) {
                    self.show_toast(message, cx);
                }
            }
        }
        cx.notify();
    }

    fn open_sign_in_page(&mut self, link: &str, cx: &mut Context<Self>) {
        if let Err(error) = crate::external_open::open_url(link) {
            self.show_toast(crate::external_open::open_url_failure_message(&error), cx);
        }
    }

    pub(crate) fn reopen_sign_in_page(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.agent_sign_in.as_mut() else {
            return;
        };
        let Some(link) = sign_in.link.clone() else {
            return;
        };
        if sign_in.agent == TerminalAgent::ClaudeCode {
            sign_in.code_open = true;
        }
        self.open_sign_in_page(&link, cx);
        cx.notify();
    }

    pub(crate) fn cancel_agent_sign_in(&mut self, cx: &mut Context<Self>) {
        let Some(sign_in) = self.agent_sign_in.take() else {
            return;
        };
        if sign_in.new_account.is_some() {
            sign_in.discard.store(true, Ordering::SeqCst);
        }
        let _ = sign_in.inputs.send(LoginInput::Cancel);
        self.agent_sign_in_code_input
            .update(cx, |input, cx| input.clear(cx));
        if let Some(account) = sign_in.new_account {
            self.remove_new_account(sign_in.agent, &sign_in.key, account, cx);
        }
        self.check_agent_accounts(cx);
        cx.notify();
    }

    pub(crate) fn submit_sign_in_code(&mut self, cx: &mut Context<Self>) {
        let code = self.agent_sign_in_code_input.read(cx).value();
        let code = code.trim();
        if code.is_empty() {
            return;
        }
        let Some(sign_in) = self.agent_sign_in.as_mut() else {
            return;
        };
        if sign_in
            .inputs
            .send(LoginInput::Code(code.to_string()))
            .is_ok()
        {
            sign_in.code_open = false;
        }
        self.agent_sign_in_code_input
            .update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    pub(crate) fn sign_in_progress(
        &self,
        agent: TerminalAgent,
        key: &str,
    ) -> Option<SignInProgress> {
        self.agent_sign_in
            .as_ref()
            .filter(|sign_in| sign_in.agent == agent && sign_in.key == key)
            .map(|sign_in| SignInProgress {
                linked: sign_in.link.is_some(),
                code_open: sign_in.code_open,
            })
    }

    pub(crate) fn signing_in(&self, agent: TerminalAgent, key: &str) -> bool {
        self.agent_sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.agent == agent && sign_in.key == key)
    }

    pub(crate) fn pending_sign_in_without_row(&self, agent: TerminalAgent, keys: &[&str]) -> bool {
        self.agent_sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.agent == agent && !keys.contains(&sign_in.key.as_str()))
    }

    pub(crate) fn render_sign_in_row(
        &self,
        agent: TerminalAgent,
        title: Option<&str>,
        ink: Ink,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(sign_in) = self.agent_sign_in.as_ref() else {
            return div().into_any_element();
        };
        let tag = agent.tag();
        let title = title.unwrap_or(NEW_ACCOUNT_TITLE);
        let detail = if sign_in.link.is_some() {
            "Finish signing in in your browser"
        } else {
            "Starting the sign-in…"
        };
        let mut controls = div().flex().flex_row().items_center().gap(px(8.));
        if sign_in.link.is_some() {
            controls = controls.child(
                kit::button(
                    SharedString::from(format!("agent-sign-in-open-{tag}")),
                    "Open the page",
                    Tone::Ghost,
                    Size::Small,
                    ink,
                )
                .on_activate(cx.listener(|this, _: &ClickEvent, _w, cx| {
                    this.reopen_sign_in_page(cx);
                })),
            );
        }
        controls = controls.child(
            kit::button(
                SharedString::from(format!("agent-sign-in-cancel-{tag}")),
                "Cancel",
                Tone::Secondary,
                Size::Small,
                ink,
            )
            .on_activate(cx.listener(|this, _: &ClickEvent, _w, cx| {
                this.cancel_agent_sign_in(cx);
            })),
        );
        let row = kit::row(
            ink,
            Some(kit::avatar(title)),
            Text::new(title.to_string()).line(kit::line(ink, detail)),
            Some(controls.into_any_element()),
        );
        if !sign_in.code_open {
            return row.into_any_element();
        }
        let ui = crate::theme::ui_colors();
        let code_field = div()
            .id(SharedString::from(format!("agent-sign-in-code-{tag}")))
            .flex_1()
            .min_w_0()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _w, cx| {
                if matches!(modal_key(event), Some(ModalKey::Confirm)) {
                    this.submit_sign_in_code(cx);
                    cx.stop_propagation();
                }
            }))
            .child(input_box_sized(
                self.agent_sign_in_code_input.clone(),
                true,
                CODE_FIELD_HEIGHT,
                ui,
            ));
        let typed = !self
            .agent_sign_in_code_input
            .read(cx)
            .value()
            .trim()
            .is_empty();
        div()
            .flex()
            .flex_col()
            .child(row)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.))
                    .pl(px(kit::COLUMN_INSET))
                    .pr(px(kit::ROW_INSET))
                    .pb(px(12.))
                    .child(code_field)
                    .child(
                        kit::button_with(
                            SharedString::from(format!("agent-sign-in-code-send-{tag}")),
                            "Continue",
                            Tone::Primary,
                            Size::Small,
                            false,
                            typed,
                            ink,
                        )
                        .on_activate(cx.listener(
                            |this, _: &ClickEvent, _w, cx| {
                                this.submit_sign_in_code(cx);
                            },
                        )),
                    ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_canceled_or_failed_sign_in_discards_a_new_account() {
        assert!(discards_new_account(true, &LoginEnd::Canceled));
        assert!(discards_new_account(
            true,
            &LoginEnd::Failed("closed".to_string())
        ));
        assert!(
            !discards_new_account(true, &LoginEnd::SignedIn),
            "a sign-in that finished as Cancel was pressed keeps its folder"
        );
        assert!(
            !discards_new_account(false, &LoginEnd::Canceled),
            "closing the app mid sign-in is not a Cancel"
        );
    }

    #[test]
    fn removing_a_new_account_folder_takes_its_contents_and_tolerates_a_missing_one() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path();
        let account = root.join(".claude-3");
        std::fs::create_dir_all(account.join("plugins")).unwrap();
        std::fs::write(account.join("settings.json"), "{}").unwrap();
        let sibling = root.join(".claude");
        std::fs::create_dir_all(&sibling).unwrap();
        remove_new_account_folders(&[account.clone(), root.join("missing")]);
        assert!(!account.exists());
        assert!(sibling.exists(), "only the named folder goes");
    }

    #[test]
    fn only_a_finished_or_failed_sign_in_speaks_up() {
        let claude = TerminalAgent::ClaudeCode;
        assert_eq!(ended_message(claude, &LoginEnd::Canceled), None);
        assert_eq!(
            ended_message(claude, &LoginEnd::Failed("timed out".into())).as_deref(),
            Some("The sign-in did not finish: timed out")
        );
        assert!(
            ended_message(claude, &LoginEnd::SignedIn)
                .is_some_and(|message| message.starts_with("Signed in to Claude Code"))
        );
    }
}
