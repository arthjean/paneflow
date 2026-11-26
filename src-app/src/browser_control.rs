use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use paneflow_api::methods::browser::TabOwner;
use paneflow_browser_client::{BrowserClient, HostConfig};
use paneflow_browser_control::{
    Approvals, CallContext, Control, ControlError, EvaluateRequest, Limits, SiteChoice, SiteRequest,
};
use paneflow_ipc_client::host_control::HostControl;
use serde_json::{Value, json};

const CLIENT_NAME: &str = "paneflow-window-browser";
const PROMPT_WAIT_MS: u64 = 25_000;
const PROMPT_CALL_DEADLINE: Duration = Duration::from_secs(40);
const SHOWN_SCRIPT_BYTES: usize = 12 * 1024;
const ALLOW_ONCE: &str = "allow_once";
const ALLOW_ALWAYS: &str = "allow_always";
const ALLOW: &str = "allow";
const DENY: &str = "deny";

pub(crate) struct Runtime {
    pub(crate) control: Control,
    pub(crate) client: Arc<BrowserClient>,
}

static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

pub(crate) fn runtime() -> Result<&'static Runtime, String> {
    RUNTIME
        .get_or_init(|| {
            let home = paneflow_home::paneflow_home()
                .ok_or("cannot resolve the Paneflow home; set PANEFLOW_HOME")?;
            Ok(start(home))
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(crate) fn control() -> Result<&'static Control, String> {
    runtime().map(|runtime| &runtime.control)
}

pub(crate) fn installed() -> bool {
    runtime().is_ok_and(|runtime| runtime.client.binary().is_file())
}

fn start(home: PathBuf) -> Runtime {
    let approvals = WorkerApprovals {
        endpoint: paneflow_home::serve_endpoint_path(&home),
        limits: Limits::default(),
    };
    let (client, events) = BrowserClient::new(HostConfig::for_home(home));
    let client = Arc::new(client);
    let control = Control::new(
        Arc::clone(&client) as Arc<dyn paneflow_browser_control::Engine>,
        events,
        Arc::new(approvals),
        Limits::default(),
    );
    Runtime { control, client }
}

pub(crate) fn answer(
    control: &Control,
    method: &str,
    params: Value,
    context: &CallContext,
) -> Value {
    match control.call(method, params, context) {
        Ok(value) => value,
        Err(error) => error_value(&error),
    }
}

pub(crate) fn error_value(error: &ControlError) -> Value {
    crate::app::ipc_handler::JsonRpcError {
        code: i32::try_from(error.code).unwrap_or(-32603),
        message: error.message.clone(),
        data: Some(json!({"reason": error.reason})),
    }
    .into_value()
}

struct WorkerApprovals {
    endpoint: PathBuf,
    limits: Limits,
}

fn agent_label(owner: &TabOwner) -> String {
    match owner {
        TabOwner::Agent {
            agent: Some(agent), ..
        } => agent.clone(),
        TabOwner::Agent { .. } => "An agent".to_string(),
        TabOwner::Human | TabOwner::Unclaimed => "The Paneflow browser".to_string(),
    }
}

fn origin_of(owner: &TabOwner) -> Option<Value> {
    match owner {
        TabOwner::Agent { session, agent } => Some(json!({
            "kind": "agent",
            "agent": agent,
            "session": session,
        })),
        TabOwner::Human | TabOwner::Unclaimed => None,
    }
}

fn choice(id: &str, title: &str) -> Value {
    json!({"id": id, "title": title})
}

pub(crate) fn site_prompt(request: &SiteRequest) -> Value {
    let mut params = json!({
        "kind": "pick",
        "title": format!("Allow {} to open {}?", agent_label(&request.owner), request.origin),
        "body": format!(
            "Browser tab {} asks to leave your local servers.\nURL: {}\nWorkspace profile: {}",
            request.tab.0, request.url, request.workspace
        ),
        "items": [
            choice(ALLOW_ONCE, "Allow once"),
            choice(ALLOW_ALWAYS, "Always allow for this workspace"),
            choice(DENY, "Deny"),
        ],
        "wait_ms": PROMPT_WAIT_MS,
    });
    if let Some(origin) = origin_of(&request.owner) {
        params["origin"] = origin;
    }
    params
}

pub(crate) fn evaluate_prompt(request: &EvaluateRequest) -> Value {
    let mut params = json!({
        "kind": "pick",
        "title": format!(
            "Allow {} to run scripts on {}?",
            agent_label(&request.owner),
            request.origin
        ),
        "body": format!(
            "Browser tab {} in workspace profile {} asks to run this script; allowing it lets this agent evaluate scripts in this workspace from now on.\n\n{}",
            request.tab.0,
            request.workspace,
            request.script
        ),
        "items": [choice(ALLOW, "Allow"), choice(DENY, "Deny")],
        "wait_ms": PROMPT_WAIT_MS,
    });
    if let Some(origin) = origin_of(&request.owner) {
        params["origin"] = origin;
    }
    params
}

impl WorkerApprovals {
    fn ask(&self, params: Value) -> Result<String, String> {
        let deadline = Instant::now() + self.limits.approval;
        let mut outcome = self.call("prompt.request", params)?;
        loop {
            match outcome["status"].as_str() {
                Some("answered") => {
                    return outcome["answer"]
                        .as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "the answer was not one of the choices".to_string());
                }
                Some("pending") if Instant::now() < deadline => {
                    let id = outcome["id"]
                        .as_u64()
                        .ok_or("the pending question carries no id")?;
                    outcome =
                        self.call("prompt.wait", json!({"id": id, "wait_ms": PROMPT_WAIT_MS}))?;
                }
                Some("pending") => return Err("the user did not answer in time".to_string()),
                Some("no_client") => {
                    return Err("no Paneflow window could show the question".to_string());
                }
                Some(status) => return Err(format!("the question ended with {status}")),
                None => return Err("the worker answered without a status".to_string()),
            }
        }
    }

    fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        HostControl::connect(&self.endpoint, CLIENT_NAME)
            .and_then(|mut control| {
                control.request_with_deadline(method, params, PROMPT_CALL_DEADLINE)
            })
            .map_err(|error| format!("the Paneflow worker cannot ask the user: {error}"))
    }
}

impl Approvals for WorkerApprovals {
    fn site(&self, request: &SiteRequest) -> Result<SiteChoice, String> {
        match self.ask(site_prompt(request))?.as_str() {
            ALLOW_ONCE => Ok(SiteChoice::AllowOnce),
            ALLOW_ALWAYS => Ok(SiteChoice::AllowAlways),
            _ => Ok(SiteChoice::Deny),
        }
    }

    fn evaluate(&self, request: &EvaluateRequest) -> Result<bool, String> {
        if request.script.len() > SHOWN_SCRIPT_BYTES {
            return Err(format!(
                "the script is {} bytes and the user is shown at most {SHOWN_SCRIPT_BYTES}; ask first with a shorter script",
                request.script.len()
            ));
        }
        Ok(self.ask(evaluate_prompt(request))? == ALLOW)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use paneflow_browser_control::Identity;
    use paneflow_browser_proto::{ProfileName, TabId};

    fn agent_owner() -> TabOwner {
        TabOwner::Agent {
            session: "a9e52660-0000-4000-8000-000000000001".into(),
            agent: Some("Claude Code".into()),
        }
    }

    #[test]
    fn a_site_question_names_the_agent_and_the_origin_and_offers_three_choices() {
        let prompt = site_prompt(&SiteRequest {
            tab: TabId(3),
            owner: agent_owner(),
            workspace: ProfileName::new("w-1").unwrap(),
            origin: "https://example.com".into(),
            url: "https://example.com/collect".into(),
        });
        assert_eq!(
            prompt["title"],
            "Allow Claude Code to open https://example.com?"
        );
        assert!(
            prompt["body"]
                .as_str()
                .unwrap()
                .contains("https://example.com/collect")
        );
        let ids: Vec<&str> = prompt["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, [ALLOW_ONCE, ALLOW_ALWAYS, DENY]);
        assert_eq!(prompt["origin"]["kind"], "agent");
        assert_eq!(
            prompt["origin"]["session"],
            "a9e52660-0000-4000-8000-000000000001"
        );
        let request: paneflow_api::methods::prompt::RequestParams =
            serde_json::from_value(prompt).expect("a valid prompt.request");
        paneflow_api::methods::prompt::check_spec(&request.spec()).expect("a valid spec");
    }

    fn evaluation(script: String) -> EvaluateRequest {
        EvaluateRequest {
            tab: TabId(1),
            owner: agent_owner(),
            workspace: ProfileName::new("w-1").unwrap(),
            origin: "http://127.0.0.1:5173".into(),
            script,
        }
    }

    #[test]
    fn an_evaluation_question_shows_the_agent_the_origin_and_the_whole_script() {
        let script = format!("{}document.title", " ".repeat(SHOWN_SCRIPT_BYTES - 14));
        let prompt = evaluate_prompt(&evaluation(script.clone()));
        assert_eq!(
            prompt["title"],
            "Allow Claude Code to run scripts on http://127.0.0.1:5173?"
        );
        let body = prompt["body"].as_str().unwrap();
        assert!(body.ends_with(&script), "{body}");
        let request: paneflow_api::methods::prompt::RequestParams =
            serde_json::from_value(prompt).expect("a valid prompt.request");
        paneflow_api::methods::prompt::check_spec(&request.spec()).expect("within the body limit");
    }

    #[test]
    fn a_script_too_long_to_show_is_refused_without_asking() {
        let home = tempfile::tempdir().unwrap();
        let approvals = WorkerApprovals {
            endpoint: home.path().join("no-worker"),
            limits: Limits::default(),
        };
        let hidden = format!(
            "document.title;{}fetch('/steal')",
            " ".repeat(SHOWN_SCRIPT_BYTES)
        );
        let refused = approvals.evaluate(&evaluation(hidden)).unwrap_err();
        assert!(refused.contains("shorter script"), "{refused}");
        let shown = approvals
            .evaluate(&evaluation("document.title".into()))
            .unwrap_err();
        assert!(shown.contains("cannot ask the user"), "{shown}");
    }

    #[test]
    fn a_real_worker_takes_both_questions_and_says_when_no_window_can_show_them() {
        let home = tempfile::tempdir().unwrap();
        let running = paneflow_serve::open(home.path()).unwrap();
        let approvals = WorkerApprovals {
            endpoint: paneflow_home::serve_endpoint_path(home.path()),
            limits: Limits::default(),
        };
        let site = approvals.site(&SiteRequest {
            tab: TabId(1),
            owner: agent_owner(),
            workspace: ProfileName::new("w-1").unwrap(),
            origin: "https://example.com".into(),
            url: "https://example.com/".into(),
        });
        assert_eq!(
            site,
            Err("no Paneflow window could show the question".to_string())
        );
        let evaluate = approvals.evaluate(&EvaluateRequest {
            tab: TabId(1),
            owner: agent_owner(),
            workspace: ProfileName::new("w-1").unwrap(),
            origin: "http://127.0.0.1:5173".into(),
            script: "document.title".into(),
        });
        assert_eq!(
            evaluate,
            Err("no Paneflow window could show the question".to_string())
        );
        running.stop();
    }

    #[test]
    fn a_missing_engine_is_reported_as_not_installed() {
        let home = tempfile::tempdir().unwrap();
        let mut config = HostConfig::for_home(home.path().to_path_buf());
        config.binary = home.path().join("absent").join("paneflow-browser-host");
        let (client, events) = BrowserClient::new(config);
        let control = Control::new(
            Arc::new(client),
            events,
            Arc::new(paneflow_browser_control::Refuse),
            Limits::default(),
        );
        let context = CallContext {
            identity: Identity::Human,
            workspace: Some(ProfileName::new("w-1").unwrap()),
            visible: None,
            live_sessions: None,
        };
        let value = answer(
            &control,
            "browser.open",
            json!({"url": "http://127.0.0.1:5173/"}),
            &context,
        );
        let error = &value[crate::app::ipc_handler::JSONRPC_ERROR_KEY];
        assert_eq!(
            error["code"],
            paneflow_api::methods::browser::ERR_NOT_INSTALLED
        );
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("The browser engine is not installed"),
            "{error}"
        );
        assert_eq!(error["data"]["reason"], "not_installed");
    }
}
