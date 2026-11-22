mod view;

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use gpui::Context;
use paneflow_api::methods::prompt::{
    OriginKind, PromptAnswer, PromptKind, PromptOrigin, PromptSpec, RequestedFrame,
};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

use crate::PaneFlowApp;

pub(crate) use view::ShownPrompt;

const CLIENT_NAME: &str = "paneflow-window-prompts";
const THREAD_NAME: &str = "paneflow-prompts";
const REPLY_THREAD_NAME: &str = "paneflow-prompt-reply";
const RECONNECT_DELAY: Duration = Duration::from_secs(2);
const STREAM_SILENCE: Duration = Duration::from_secs(10);
const FRAMES_PER_TICK: usize = 32;
pub(crate) const SHOWN_KINDS: [PromptKind; 2] = [PromptKind::Input, PromptKind::Pick];

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AnswerToken(String);

impl AnswerToken {
    fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AnswerToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AnswerToken(..)")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PendingPrompt {
    pub(crate) id: u64,
    pub(crate) spec: Arc<PromptSpec>,
    pub(crate) origin: Option<PromptOrigin>,
    pub(crate) asked_at: Instant,
    pub(crate) token: Option<AnswerToken>,
}

pub(crate) fn waited(requested_at_ms: u64, now_ms: u64) -> Duration {
    Duration::from_millis(now_ms.saturating_sub(requested_at_ms))
}

pub(crate) fn plain_text(raw: &str) -> String {
    let mut text = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() != Some(&'\n') {
                    text.push('\n');
                }
            }
            '\n' => text.push('\n'),
            '\t' => text.push(' '),
            c if c.is_control() => {}
            c => text.push(c),
        }
    }
    crate::markdown::strip_bidi_zero_width(text)
}

fn displayable(mut spec: PromptSpec) -> PromptSpec {
    spec.title = plain_text(&spec.title);
    if let Some(body) = spec.body.as_mut() {
        *body = plain_text(body);
    }
    for text in [&mut spec.placeholder, &mut spec.value, &mut spec.confirm]
        .into_iter()
        .flatten()
    {
        *text = plain_text(text).replace('\n', " ");
    }
    spec
}

impl PendingPrompt {
    pub(crate) fn from_frame(frame: RequestedFrame, now_ms: u64) -> Self {
        let waited = waited(frame.requested_at_ms, now_ms);
        Self {
            id: frame.id,
            spec: Arc::new(displayable(frame.spec)),
            origin: frame.origin,
            asked_at: Instant::now()
                .checked_sub(waited)
                .unwrap_or_else(Instant::now),
            token: frame.token.map(AnswerToken),
        }
    }

    pub(crate) fn raised_by_agent(&self) -> bool {
        self.origin
            .as_ref()
            .is_some_and(|origin| origin.kind == OriginKind::Agent)
    }

    pub(crate) fn surface_id(&self) -> Option<u64> {
        self.origin.as_ref().and_then(|origin| origin.surface_id)
    }

    pub(crate) fn origin_line_in(&self, pane_title: Option<&str>) -> String {
        let Some(origin) = self.origin.as_ref() else {
            return "Asked by a script".to_string();
        };
        let pane = pane_title
            .map(|title| plain_text(title).replace('\n', " "))
            .filter(|title| !title.trim().is_empty());
        match (origin.kind, pane) {
            (OriginKind::Agent, Some(pane)) => {
                let agent = origin.agent.as_deref().map_or_else(
                    || "an agent".to_string(),
                    |agent| plain_text(agent).replace('\n', " "),
                );
                format!("Asked by {agent} in {}", pane.trim())
            }
            _ => plain_text(&origin.label()).replace('\n', " "),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum PromptFrame {
    Requested(Box<PendingPrompt>),
    Ended(u64),
    Reset,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum StreamLine {
    Frame(PromptFrame),
    Quiet,
    End,
}

pub(crate) fn parse_line(line: &str, now_ms: u64) -> StreamLine {
    let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
        return StreamLine::Quiet;
    };
    match value["type"].as_str() {
        Some("requested") => match serde_json::from_value::<RequestedFrame>(value) {
            Ok(frame) => StreamLine::Frame(PromptFrame::Requested(Box::new(
                PendingPrompt::from_frame(frame, now_ms),
            ))),
            Err(error) => {
                log::warn!("paneflow: the worker sent a prompt the window cannot read: {error}");
                StreamLine::Quiet
            }
        },
        Some("ended") => value["id"].as_u64().map_or(StreamLine::Quiet, |id| {
            StreamLine::Frame(PromptFrame::Ended(id))
        }),
        Some("end") => StreamLine::End,
        _ => StreamLine::Quiet,
    }
}

pub(crate) fn next_prompt(
    pending: &[PendingPrompt],
    activated: Option<u64>,
    shown: Option<u64>,
) -> Option<&PendingPrompt> {
    if let Some(prompt) = activated
        .filter(|id| Some(*id) != shown)
        .and_then(|id| pending.iter().find(|prompt| prompt.id == id))
    {
        return Some(prompt);
    }
    pending
        .iter()
        .filter(|prompt| Some(prompt.id) != shown)
        .min_by_key(|prompt| (prompt.raised_by_agent(), prompt.asked_at, prompt.id))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

fn open_stream(endpoint: &Path) -> Result<HostControl, String> {
    let mut control = HostControl::connect(endpoint, CLIENT_NAME)?;
    let id = control
        .write_request("prompt.follow", json!({"kinds": SHOWN_KINDS}))
        .map_err(|error| error.to_string())?;
    loop {
        let line = control
            .read_stream_line(STREAM_SILENCE)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "the worker closed the prompt stream".to_string())?;
        let envelope: Value =
            serde_json::from_str(line.trim()).map_err(|error| error.to_string())?;
        if envelope["id"].as_u64() != Some(id) {
            continue;
        }
        if envelope["error"].is_object() {
            return Err(envelope["error"]["message"]
                .as_str()
                .unwrap_or("the worker refused prompt.follow")
                .to_string());
        }
        return Ok(control);
    }
}

fn follow(endpoint: &Path, frames: &Sender<PromptFrame>, wake: &crate::app::wake::AppWake) {
    let send = |frame: PromptFrame| {
        let sent = frames.send(frame).is_ok();
        wake.notify();
        sent
    };
    loop {
        let mut delivered = false;
        match open_stream(endpoint) {
            Ok(mut control) => {
                while let Ok(Some(line)) = control.read_stream_line(STREAM_SILENCE) {
                    match parse_line(&line, now_ms()) {
                        StreamLine::Frame(frame) => {
                            delivered = true;
                            if !send(frame) {
                                return;
                            }
                        }
                        StreamLine::Quiet => {}
                        StreamLine::End => break,
                    }
                }
            }
            Err(error) => log::debug!("paneflow: cannot follow the worker's prompts: {error}"),
        }
        if delivered && !send(PromptFrame::Reset) {
            return;
        }
        std::thread::sleep(RECONNECT_DELAY);
    }
}

pub(crate) fn spawn_prompt_thread(
    wake: crate::app::wake::AppWake,
) -> Option<Receiver<PromptFrame>> {
    let endpoint = paneflow_home::serve_endpoint_path_for_current_home()?;
    let (tx, rx) = channel();
    let spawned = std::thread::Builder::new()
        .name(THREAD_NAME.into())
        .spawn(move || follow(&endpoint, &tx, &wake));
    match spawned {
        Ok(_) => Some(rx),
        Err(error) => {
            log::warn!("paneflow: cannot start the prompt follower: {error}");
            None
        }
    }
}

pub(crate) fn reply_params(
    id: u64,
    token: Option<&AnswerToken>,
    answer: Option<PromptAnswer>,
) -> Value {
    let mut params = match answer {
        Some(answer) => json!({"id": id, "answer": answer}),
        None => json!({"id": id, "cancel": true}),
    };
    if let (Some(token), Some(map)) = (token, params.as_object_mut()) {
        map.insert("token".into(), Value::String(token.expose().to_string()));
    }
    params
}

pub(crate) fn send_reply(id: u64, token: Option<AnswerToken>, answer: Option<PromptAnswer>) {
    let Some(endpoint) = paneflow_home::serve_endpoint_path_for_current_home() else {
        return;
    };
    let params = reply_params(id, token.as_ref(), answer);
    let spawned = std::thread::Builder::new()
        .name(REPLY_THREAD_NAME.into())
        .spawn(move || {
            let replied = HostControl::connect(&endpoint, CLIENT_NAME)
                .and_then(|mut control| control.request("prompt.reply", params));
            if let Err(error) = replied {
                log::debug!("paneflow: the worker did not take the reply to prompt {id}: {error}");
            }
        });
    if let Err(error) = spawned {
        log::warn!("paneflow: cannot send the reply to prompt {id}: {error}");
    }
}

#[derive(Default)]
pub(crate) struct PromptsView {
    frames: Option<Receiver<PromptFrame>>,
    pub(crate) pending: Vec<PendingPrompt>,
    pub(crate) activated: Option<u64>,
    pub(crate) shown: Option<ShownPrompt>,
    pub(crate) return_focus: Option<crate::FocusReturn>,
    warned_icons: HashSet<String>,
}

impl PromptsView {
    pub(crate) fn apply(&mut self, frame: PromptFrame) -> bool {
        match frame {
            PromptFrame::Requested(prompt) => {
                if self.pending.iter().any(|known| known.id == prompt.id) {
                    return false;
                }
                self.pending.push(*prompt);
                true
            }
            PromptFrame::Ended(id) => {
                let known = self.pending.len();
                self.pending.retain(|prompt| prompt.id != id);
                if self.activated == Some(id) {
                    self.activated = None;
                }
                if self
                    .shown
                    .as_ref()
                    .is_some_and(|shown| shown.prompt.id == id)
                {
                    self.close_shown();
                }
                known != self.pending.len()
            }
            PromptFrame::Reset => {
                self.pending.clear();
                self.activated = None;
                self.close_shown();
                true
            }
        }
    }

    pub(crate) fn close_shown(&mut self) -> Option<PendingPrompt> {
        let shown = self.shown.take()?;
        if self.return_focus.is_none() {
            self.return_focus = Some(shown.return_focus);
        }
        Some(shown.prompt)
    }

    pub(crate) fn shown_id(&self) -> Option<u64> {
        self.shown.as_ref().map(|shown| shown.prompt.id)
    }

    pub(crate) fn warn_unknown_icon(&mut self, name: &str) {
        if self.warned_icons.insert(name.to_string()) {
            log::warn!(
                "paneflow: the prompt icon '{name}' is not one of {}; the item is drawn without an icon",
                paneflow_api::methods::prompt::ICONS.join(", ")
            );
        }
    }

    pub(crate) fn surface_agent_prompt(&self, surface_id: u64) -> Option<u64> {
        self.pending
            .iter()
            .filter(|prompt| prompt.raised_by_agent() && prompt.surface_id() == Some(surface_id))
            .min_by_key(|prompt| (prompt.asked_at, prompt.id))
            .map(|prompt| prompt.id)
    }

    pub(crate) fn surface_prompt(&self, surface_id: u64) -> Option<u64> {
        self.pending
            .iter()
            .filter(|prompt| prompt.surface_id() == Some(surface_id))
            .min_by_key(|prompt| (prompt.asked_at, prompt.id))
            .map(|prompt| prompt.id)
    }
}

impl PaneFlowApp {
    pub(crate) fn start_prompt_stream(&mut self) {
        if self.prompts.frames.is_none() {
            self.prompts.frames = spawn_prompt_thread(self.app_wake.clone());
        }
    }

    pub(crate) fn process_prompt_frames(&mut self, cx: &mut Context<Self>) {
        let mut frames = Vec::new();
        if let Some(receiver) = self.prompts.frames.as_ref() {
            while frames.len() < FRAMES_PER_TICK {
                let Ok(frame) = receiver.try_recv() else {
                    break;
                };
                frames.push(frame);
            }
        }
        if frames.len() == FRAMES_PER_TICK {
            self.app_wake.notify();
        }
        let mut changed = false;
        for frame in frames {
            changed |= self.prompts.apply(frame);
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn prompt_origin_line(&self, prompt: &PendingPrompt, cx: &gpui::App) -> String {
        let pane_title = prompt
            .raised_by_agent()
            .then(|| prompt.surface_id())
            .flatten()
            .and_then(|surface_id| self.surface_pane_title(surface_id, cx));
        prompt.origin_line_in(pane_title.as_deref())
    }

    pub(crate) fn activate_surface_prompt(&mut self, surface_id: u64, cx: &mut Context<Self>) {
        let Some(id) = self.prompts.surface_prompt(surface_id) else {
            return;
        };
        self.prompts.activated = Some(id);
        if self.prompts.shown_id().is_some_and(|shown| shown != id) {
            self.prompts.close_shown();
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_api::methods::prompt::FrameKind;

    fn spec(title: &str) -> PromptSpec {
        PromptSpec {
            kind: PromptKind::Input,
            title: title.into(),
            body: None,
            placeholder: None,
            value: None,
            confirm: None,
            timeout: None,
            items: None,
            multiple: None,
            allow_other: None,
        }
    }

    fn prompt(id: u64, kind: OriginKind, waited_secs: u64, surface: Option<u64>) -> PendingPrompt {
        PendingPrompt {
            id,
            spec: Arc::new(spec("t")),
            origin: Some(PromptOrigin {
                kind,
                title: None,
                name: None,
                file: None,
                agent: None,
                surface_id: surface,
                workspace_id: None,
                session: None,
            }),
            asked_at: Instant::now()
                .checked_sub(Duration::from_secs(waited_secs))
                .unwrap(),
            token: None,
        }
    }

    #[test]
    fn the_activated_prompt_comes_first_then_the_users_oldest_then_the_agents_oldest() {
        let pending = vec![
            prompt(1, OriginKind::Agent, 90, None),
            prompt(2, OriginKind::Script, 10, None),
            prompt(3, OriginKind::Action, 30, None),
            prompt(4, OriginKind::Agent, 120, None),
        ];
        assert_eq!(next_prompt(&pending, None, None).map(|p| p.id), Some(3));
        assert_eq!(next_prompt(&pending, None, Some(3)).map(|p| p.id), Some(2));
        assert_eq!(
            next_prompt(&pending, Some(1), Some(3)).map(|p| p.id),
            Some(1)
        );
        let agents = vec![pending[0].clone(), pending[3].clone()];
        assert_eq!(next_prompt(&agents, None, None).map(|p| p.id), Some(4));
        assert_eq!(next_prompt(&[], None, None), None);
    }

    #[test]
    fn a_requested_frame_carries_its_waiting_time_and_an_ended_frame_closes_the_shown_prompt() {
        let line = serde_json::to_string(&RequestedFrame {
            kind: FrameKind::Requested,
            id: 7,
            spec: spec("Name"),
            origin: None,
            requested_at_ms: 1_000,
            remaining_ms: 299_000,
            token: None,
        })
        .unwrap();
        let StreamLine::Frame(PromptFrame::Requested(prompt)) = parse_line(&line, 61_000) else {
            panic!("a requested frame");
        };
        assert_eq!(waited(1_000, 61_000), Duration::from_secs(60));
        assert_eq!(
            waited(61_000, 1_000),
            Duration::ZERO,
            "a clock step back waits zero"
        );
        assert_eq!(prompt.id, 7);
        assert_eq!(prompt.origin_line_in(None), "Asked by a script");
        assert_eq!(
            parse_line(r#"{"type":"ended","id":7,"reason":"answered"}"#, 0),
            StreamLine::Frame(PromptFrame::Ended(7))
        );
        assert_eq!(parse_line(r#"{"type":"keepalive"}"#, 0), StreamLine::Quiet);
        assert_eq!(
            parse_line(r#"{"type":"end","reason":"stop"}"#, 0),
            StreamLine::End
        );

        let mut view = PromptsView::default();
        assert!(view.apply(PromptFrame::Requested(prompt.clone())));
        assert!(
            !view.apply(PromptFrame::Requested(prompt)),
            "a repeat is ignored"
        );
        assert!(view.apply(PromptFrame::Ended(7)));
        assert!(view.pending.is_empty());
    }

    #[test]
    fn the_window_replies_with_the_token_of_its_frame_and_never_prints_it() {
        let line = serde_json::to_string(&RequestedFrame {
            kind: FrameKind::Requested,
            id: 9,
            spec: spec("Allow?"),
            origin: None,
            requested_at_ms: 0,
            remaining_ms: 1,
            token: Some("ab".repeat(16)),
        })
        .unwrap();
        let StreamLine::Frame(PromptFrame::Requested(prompt)) = parse_line(&line, 0) else {
            panic!("a requested frame");
        };
        assert!(!format!("{prompt:?}").contains(&"ab".repeat(16)));
        assert_eq!(
            reply_params(
                9,
                prompt.token.as_ref(),
                Some(PromptAnswer::Text("yes".into()))
            ),
            json!({"id": 9, "answer": "yes", "token": "ab".repeat(16)})
        );
        assert_eq!(
            reply_params(9, prompt.token.as_ref(), None),
            json!({"id": 9, "cancel": true, "token": "ab".repeat(16)})
        );
        assert_eq!(
            reply_params(3, None, None),
            json!({"id": 3, "cancel": true})
        );
    }

    #[test]
    fn a_permission_input_keeps_its_line_breaks_and_loses_its_controls_and_bidi_marks() {
        let mut permission = spec("Bash");
        permission.body =
            Some("command:\r\nrm -rf\u{202e}gpj.\u{1b}[2J build\ndescription: x\u{7}".into());
        let line = serde_json::to_string(&RequestedFrame {
            kind: FrameKind::Requested,
            id: 5,
            spec: permission,
            origin: None,
            requested_at_ms: 0,
            remaining_ms: 1,
            token: None,
        })
        .unwrap();
        let StreamLine::Frame(PromptFrame::Requested(prompt)) = parse_line(&line, 0) else {
            panic!("a requested frame");
        };
        assert_eq!(
            prompt.spec.body.as_deref(),
            Some("command:\nrm -rfgpj.[2J build\ndescription: x")
        );
    }

    #[test]
    fn a_surface_resolves_to_its_oldest_prompt() {
        let view = PromptsView {
            pending: vec![
                prompt(1, OriginKind::Action, 5, Some(9)),
                prompt(2, OriginKind::Action, 50, Some(9)),
                prompt(3, OriginKind::Action, 500, None),
            ],
            ..PromptsView::default()
        };
        assert_eq!(view.surface_prompt(9), Some(2));
        assert_eq!(view.surface_prompt(4), None);
    }

    #[test]
    fn an_agent_question_is_shown_as_plain_text_with_its_controls_and_bidi_marks_stripped() {
        let line = serde_json::to_string(&RequestedFrame {
            kind: FrameKind::Requested,
            id: 8,
            spec: spec("Delete\u{202e}gpj.exe\u{1b}[31m the\r\nbranch\u{200b}?\u{7}\tnow"),
            origin: None,
            requested_at_ms: 0,
            remaining_ms: 1,
            token: None,
        })
        .unwrap();
        let StreamLine::Frame(PromptFrame::Requested(prompt)) = parse_line(&line, 0) else {
            panic!("a requested frame");
        };
        assert_eq!(prompt.spec.title, "Deletegpj.exe[31m the\nbranch? now");
        assert_eq!(plain_text("a\rb\u{2066}c\u{85}d"), "a\nbcd");
    }

    fn agent_origin(agent: Option<&str>) -> PromptOrigin {
        PromptOrigin {
            kind: OriginKind::Agent,
            title: None,
            name: None,
            file: None,
            agent: agent.map(str::to_string),
            surface_id: Some(9),
            workspace_id: Some(1),
            session: Some("s".into()),
        }
    }

    #[test]
    fn an_agent_origin_names_the_agent_and_its_pane_title_and_no_other_origin_does() {
        let mut asked = prompt(1, OriginKind::Agent, 0, Some(9));
        asked.origin = Some(agent_origin(Some("Claude Code")));
        assert_eq!(
            asked.origin_line_in(Some("api \u{202e}tests")),
            "Asked by Claude Code in api tests"
        );
        assert_eq!(asked.origin_line_in(None), "Asked by Claude Code in pane 9");
        asked.origin = Some(agent_origin(None));
        assert_eq!(
            asked.origin_line_in(Some("web")),
            "Asked by an agent in web"
        );

        let mut action = prompt(2, OriginKind::Action, 0, Some(9));
        if let Some(origin) = action.origin.as_mut() {
            origin.title = Some("New branch".into());
        }
        assert_eq!(
            action.origin_line_in(Some("web")),
            "Asked by the action New branch"
        );
        let script = prompt(3, OriginKind::Script, 0, Some(9));
        assert_eq!(script.origin_line_in(Some("web")), "Asked by a script");
        for line in [
            action.origin_line_in(Some("web")),
            script.origin_line_in(Some("web")),
        ] {
            assert!(!line.contains(" in web"), "{line}");
        }
    }

    #[test]
    fn a_surface_resolves_its_agent_question_apart_from_its_other_prompts() {
        let view = PromptsView {
            pending: vec![
                prompt(1, OriginKind::Action, 500, Some(9)),
                prompt(2, OriginKind::Agent, 5, Some(9)),
                prompt(3, OriginKind::Agent, 50, Some(4)),
            ],
            ..PromptsView::default()
        };
        assert_eq!(view.surface_agent_prompt(9), Some(2));
        assert_eq!(view.surface_agent_prompt(4), Some(3));
        assert_eq!(view.surface_agent_prompt(7), None);
        assert_eq!(view.surface_prompt(9), Some(1));
    }
}
