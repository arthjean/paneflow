#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unwrap_in_result,
    clippy::panic
)]

mod common;

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use common::{DEADLINE, FixtureServer, config, lan_address};
use paneflow_api::methods::browser::{
    ERR_DENIED, ERR_NOT_OWNER, ERR_STALE_REFERENCE, ERR_TIMED_OUT,
};
use paneflow_browser_client::BrowserClient;
use paneflow_browser_control::{
    Approvals, CallContext, Control, ControlError, EvaluateRequest, Identity, Limits, SiteChoice,
    SiteRequest,
};
use paneflow_browser_proto::{ProfileName, Reply, TabId};
use serde_json::{Value, json};

#[derive(Default)]
struct Script {
    sites: VecDeque<Result<SiteChoice, String>>,
    evaluations: VecDeque<Result<bool, String>>,
    site_requests: Vec<SiteRequest>,
    evaluate_requests: Vec<EvaluateRequest>,
    hold: bool,
}

#[derive(Default)]
struct Scripted {
    script: Mutex<Script>,
    asked: Condvar,
}

impl Scripted {
    fn answer_sites(&self, answers: impl IntoIterator<Item = Result<SiteChoice, String>>) {
        self.script.lock().unwrap().sites.extend(answers);
        self.asked.notify_all();
    }

    fn answer_evaluations(&self, answers: impl IntoIterator<Item = Result<bool, String>>) {
        self.script.lock().unwrap().evaluations.extend(answers);
    }

    fn hold(&self, hold: bool) {
        self.script.lock().unwrap().hold = hold;
        self.asked.notify_all();
    }

    fn site_requests(&self) -> Vec<SiteRequest> {
        self.script.lock().unwrap().site_requests.clone()
    }

    fn evaluate_requests(&self) -> Vec<EvaluateRequest> {
        self.script.lock().unwrap().evaluate_requests.clone()
    }

    fn wait_for_site_requests(&self, count: usize) {
        let deadline = std::time::Instant::now() + DEADLINE;
        let mut script = self.script.lock().unwrap();
        while script.site_requests.len() < count {
            assert!(
                std::time::Instant::now() < deadline,
                "no site request arrived"
            );
            script = self
                .asked
                .wait_timeout(script, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }
}

impl Approvals for Scripted {
    fn site(&self, request: &SiteRequest) -> Result<SiteChoice, String> {
        let mut script = self.script.lock().unwrap();
        script.site_requests.push(request.clone());
        self.asked.notify_all();
        let deadline = std::time::Instant::now() + DEADLINE;
        loop {
            if !script.hold
                && let Some(answer) = script.sites.pop_front()
            {
                return answer;
            }
            if std::time::Instant::now() >= deadline {
                return Err("the test never answered".to_string());
            }
            script = self
                .asked
                .wait_timeout(script, Duration::from_millis(50))
                .unwrap()
                .0;
        }
    }

    fn evaluate(&self, request: &EvaluateRequest) -> Result<bool, String> {
        let mut script = self.script.lock().unwrap();
        script.evaluate_requests.push(request.clone());
        script
            .evaluations
            .pop_front()
            .unwrap_or_else(|| Err("the test never answered".to_string()))
    }
}

struct Rig {
    _home: tempfile::TempDir,
    client: BrowserClient,
    control: Control,
    approvals: Arc<Scripted>,
}

fn rig_with(limits: Limits) -> Rig {
    let home = tempfile::tempdir().unwrap();
    let (client, events) = BrowserClient::new(config(home.path()));
    let approvals = Arc::new(Scripted::default());
    let control = Control::new(
        Arc::new(client.clone()),
        events,
        Arc::clone(&approvals) as Arc<dyn Approvals>,
        limits,
    );
    Rig {
        _home: home,
        client,
        control,
        approvals,
    }
}

fn rig() -> Rig {
    rig_with(Limits::default())
}

impl Drop for Rig {
    fn drop(&mut self) {
        if let Some(pid) = self.client.host_pid() {
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
}

const WORKSPACE: &str = "0b6d6713-5706-4af6-8061-06e40f757cb9";
const OTHER_WORKSPACE: &str = "7c0e3a11-9a2b-4d4e-8f00-3d3f0c1b2a99";
const AGENT: &str = "a9e52660-0000-4000-8000-000000000001";
const OTHER_AGENT: &str = "a9e52660-0000-4000-8000-000000000002";

fn agent(session: &str, workspace: &str) -> CallContext {
    let workspace = ProfileName::new(workspace).unwrap();
    CallContext {
        identity: Identity::Agent {
            session: session.to_string(),
            agent: Some("Claude Code".to_string()),
        },
        workspace: Some(workspace.clone()),
        visible: Some(workspace),
        live_sessions: Some([AGENT, OTHER_AGENT].iter().map(|s| s.to_string()).collect()),
    }
}

fn human(workspace: &str) -> CallContext {
    CallContext {
        identity: Identity::Human,
        workspace: Some(ProfileName::new(workspace).unwrap()),
        visible: None,
        live_sessions: None,
    }
}

impl Rig {
    fn call(&self, method: &str, params: Value, context: &CallContext) -> Value {
        match self.control.call(method, params.clone(), context) {
            Ok(value) => value,
            Err(error) => panic!("{method} {params}: {error:?}"),
        }
    }

    fn fail(&self, method: &str, params: Value, context: &CallContext) -> ControlError {
        match self.control.call(method, params.clone(), context) {
            Ok(value) => panic!("{method} {params} should fail, got {value}"),
            Err(error) => error,
        }
    }

    fn open(&self, url: &str, context: &CallContext) -> u64 {
        self.call("browser.open", json!({"url": url}), context)["tab"]
            .as_u64()
            .unwrap()
    }

    fn page_eval(&self, tab: u64, expression: &str) -> Value {
        match self
            .client
            .devtools(
                TabId(tab),
                "Runtime.evaluate",
                json!({"expression": expression, "returnByValue": true}),
            )
            .wait(DEADLINE)
        {
            Ok(Reply::DevTools(result)) => result["result"]["value"].clone(),
            other => panic!("{expression}: {other:?}"),
        }
    }

    fn snapshot(&self, tab: u64, context: &CallContext) -> Value {
        self.call("browser.snapshot", json!({"tab": tab}), context)
    }

    fn reference(&self, snapshot: &Value, line_part: &str) -> String {
        let tree = snapshot["tree"].as_str().unwrap();
        let line = tree
            .lines()
            .find(|line| line.contains(line_part))
            .unwrap_or_else(|| panic!("no line with {line_part} in\n{tree}"));
        let start = line.find("[ref=").unwrap() + 5;
        line[start..line[start..].find(']').unwrap() + start].to_string()
    }
}

#[test]
fn an_agent_finds_the_planted_console_error_and_failing_request_with_no_port_open() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let opened = rig.call("browser.open", json!({"url": server.url("/")}), &me);
    assert_eq!(opened["state"], "loaded");
    assert_eq!(opened["http_status"], 200);
    assert_eq!(opened["owner"]["session"], AGENT);
    let tab = opened["tab"].as_u64().unwrap();
    rig.call(
        "browser.wait_for",
        json!({"tab": tab, "network_idle": true}),
        &me,
    );

    let snapshot = rig.snapshot(tab, &me);
    assert_eq!(snapshot["url"], server.url("/"));
    assert_eq!(snapshot["title"], "Fixture shop");
    assert_eq!(snapshot["loading"], false);
    let tree = snapshot["tree"].as_str().unwrap();
    assert!(tree.starts_with("RootWebArea \"Fixture shop\""), "{tree}");
    assert!(tree.contains("text \"Total: unknown\""), "{tree}");
    assert!(tree.contains("button \"Refresh cart\" [ref=e"), "{tree}");
    let console = snapshot["console"].as_array().unwrap();
    assert!(
        console.iter().any(|entry| entry["level"] == "error"
            && entry["text"].as_str().unwrap().contains("TypeError")
            && entry["line"] == 14),
        "{console:#?}"
    );
    let network = snapshot["network"].as_array().unwrap();
    assert!(
        network
            .iter()
            .any(|entry| entry["url"] == server.url("/api/prices")
                && entry["status"] == 500
                && entry["resource_type"] == "Fetch"),
        "{network:#?}"
    );

    let history = rig.call("browser.network", json!({"tab": tab}), &me);
    let prices = history["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["url"] == server.url("/api/prices"))
        .expect("the failing request is in the history");
    assert_eq!(prices["method"], "GET");
    assert_eq!(prices["status"], 500);
    assert!(prices.get("body").is_none() && prices.get("response").is_none());

    let again = rig.snapshot(tab, &me);
    assert!(again["console"].as_array().unwrap().is_empty());
    assert!(again["network"].as_array().unwrap().is_empty());

    let pid = rig.client.host_pid().unwrap();
    let inodes = common::listening_tcp_inodes();
    for process in common::process_tree(pid) {
        assert!(!common::command_line(process).contains("remote-debugging"));
        assert!(common::socket_inodes(process).is_disjoint(&inodes));
    }
}

#[test]
fn only_the_owner_drives_a_tab_and_every_tab_lists_its_owner() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let other = agent(OTHER_AGENT, WORKSPACE);
    let person = human(WORKSPACE);
    let mine = rig.open(&server.url("/"), &me);
    let theirs = rig.open(&server.url("/form.html"), &other);
    let humans = rig.open(&server.url("/form.html"), &person);
    let elsewhere = rig.open(&server.url("/"), &agent(OTHER_AGENT, OTHER_WORKSPACE));

    let listed = rig.call("browser.list", json!({}), &me);
    let tabs = listed["tabs"].as_array().unwrap();
    let row = |tab: u64| tabs.iter().find(|row| row["tab"] == tab).cloned();
    assert_eq!(row(mine).unwrap()["drivable"], true);
    assert_eq!(row(mine).unwrap()["owner"]["kind"], "agent");
    assert_eq!(row(theirs).unwrap()["drivable"], false);
    assert_eq!(row(theirs).unwrap()["owner"]["session"], OTHER_AGENT);
    assert_eq!(row(humans).unwrap()["owner"]["kind"], "human");
    assert_eq!(row(humans).unwrap()["drivable"], false);
    assert!(
        row(elsewhere).is_none(),
        "another workspace's tab is listed"
    );
    assert!(tabs.iter().all(|row| row["presentation"] == "none"));

    for (method, params) in [
        (
            "browser.navigate",
            json!({"tab": humans, "url": server.url("/")}),
        ),
        ("browser.snapshot", json!({"tab": humans})),
        ("browser.close", json!({"tab": theirs})),
    ] {
        let refused = rig.fail(method, params, &me);
        assert_eq!(refused.code, ERR_NOT_OWNER, "{method}");
        assert!(
            refused.message.contains("owned by the human") || refused.message.contains(OTHER_AGENT),
            "{}",
            refused.message
        );
    }
    assert_eq!(
        rig.page_eval(humans, "location.pathname"),
        json!("/form.html")
    );
}

#[test]
fn navigate_reload_back_forward_and_close_report_the_final_url() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/"), &me);
    let moved = rig.call(
        "browser.navigate",
        json!({"tab": tab, "url": server.url("/form.html")}),
        &me,
    );
    assert_eq!(moved["url"], server.url("/form.html"));
    assert_eq!(moved["state"], "loaded");
    let reloaded = rig.call("browser.reload", json!({"tab": tab}), &me);
    assert_eq!(reloaded["url"], server.url("/form.html"));
    let back = rig.call("browser.back", json!({"tab": tab}), &me);
    assert_eq!(back["url"], server.url("/"));
    let forward = rig.call("browser.forward", json!({"tab": tab}), &me);
    assert_eq!(forward["url"], server.url("/form.html"));
    let no_later = rig.fail("browser.forward", json!({"tab": tab}), &me);
    assert!(
        no_later.message.contains("no later page"),
        "{}",
        no_later.message
    );
    let closed = rig.call("browser.close", json!({"tab": tab}), &me);
    assert_eq!(closed["url"], server.url("/form.html"));
    assert_eq!(closed["closed"], true);
    let listed = rig.call("browser.list", json!({}), &me);
    assert!(listed["tabs"].as_array().unwrap().is_empty());
}

#[test]
fn a_tab_of_an_ended_session_is_unclaimed_until_an_agent_claims_it() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let other = agent(OTHER_AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/"), &other);
    let mine = rig.open(&server.url("/"), &me);
    let refused = rig.fail("browser.claim", json!({"tab": tab}), &me);
    assert_eq!(refused.code, ERR_NOT_OWNER);
    assert!(refused.message.contains(OTHER_AGENT), "{}", refused.message);

    let mut after = me.clone();
    after.live_sessions = Some([AGENT.to_string()].into_iter().collect::<HashSet<_>>());
    let listed = rig.call("browser.list", json!({}), &after);
    let row = listed["tabs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["tab"] == tab)
        .cloned()
        .unwrap();
    assert_eq!(row["owner"]["kind"], "unclaimed");
    let unclaimed = rig.fail("browser.snapshot", json!({"tab": tab}), &after);
    assert!(
        unclaimed.message.contains("browser.claim"),
        "{}",
        unclaimed.message
    );
    let claimed = rig.call("browser.claim", json!({"tab": tab}), &after);
    assert_eq!(claimed["owner"]["session"], AGENT);
    assert_eq!(claimed["drivable"], true);
    rig.snapshot(tab, &after);
    let not_free = rig.fail("browser.claim", json!({"tab": mine}), &other);
    assert_eq!(not_free.code, ERR_NOT_OWNER);
}

#[test]
fn a_snapshot_hides_passwords_states_its_truncation_and_reports_a_loading_page() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/form.html"), &me);
    let form = rig.snapshot(tab, &me);
    let rendered = form.to_string();
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(
        form["tree"]
            .as_str()
            .unwrap()
            .contains("textbox \"Password\" value=<hidden>"),
        "{}",
        form["tree"]
    );
    assert_eq!(form["dropped_bytes"], 0);
    assert_eq!(
        rig.page_eval(tab, "document.visibilityState"),
        json!("visible")
    );

    let big = rig.call(
        "browser.navigate",
        json!({"tab": tab, "url": server.url("/test/big")}),
        &me,
    );
    assert_eq!(big["state"], "loaded");
    let snapshot = rig.snapshot(tab, &me);
    let tree = snapshot["tree"].as_str().unwrap();
    assert!(tree.len() <= paneflow_browser_proto::limits::SNAPSHOT_TEXT_BYTES);
    assert!(
        snapshot["dropped_bytes"].as_u64().unwrap() > 10_000,
        "{}",
        snapshot["dropped_bytes"]
    );
    assert!(tree.contains("Button number 0"));
    assert!(!tree.contains("Button number 2999"));

    let control = rig.control.clone();
    let slow_url = server.url("/test/slow");
    let mover = me.clone();
    let navigating = std::thread::spawn(move || {
        control.call(
            "browser.navigate",
            json!({"tab": tab, "url": slow_url}),
            &mover,
        )
    });
    let deadline = std::time::Instant::now() + DEADLINE;
    let loading = loop {
        let snapshot = rig.snapshot(tab, &me);
        if snapshot["loading"] == true
            && snapshot["tree"].as_str().unwrap().contains("Arrived first")
        {
            break snapshot;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "never saw the slow page loading"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(!loading["tree"].as_str().unwrap().contains("Arrived last"));
    let finished = navigating.join().unwrap().unwrap();
    assert_eq!(finished["state"], "loaded");
    let done = rig.snapshot(tab, &me);
    assert_eq!(done["loading"], false);
    assert!(done["tree"].as_str().unwrap().contains("Arrived last"));
}

#[test]
fn actions_hit_the_referenced_element_and_stale_hidden_or_disabled_ones_are_refused() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/form.html"), &me);
    let snapshot = rig.snapshot(tab, &me);
    let email = rig.reference(&snapshot, "textbox \"Email\"");
    let country = rig.reference(&snapshot, "combobox \"Country\"");
    let disabled = rig.reference(&snapshot, "checkbox \"Disabled option\"");
    let save = rig.reference(&snapshot, "button \"Save for later\"");
    let submit = rig.reference(&snapshot, "button \"Place order\"");

    let typed = rig.call(
        "browser.act",
        json!({"tab": tab, "ref": email, "action": "type", "text": "ada@example.com"}),
        &me,
    );
    assert_eq!(typed["navigated"], false);
    assert_eq!(
        rig.page_eval(tab, "document.querySelector('[name=email]').value"),
        json!("ada@example.com")
    );
    rig.call(
        "browser.act",
        json!({"tab": tab, "ref": country, "action": "select", "value": "Japan"}),
        &me,
    );
    assert_eq!(
        rig.page_eval(tab, "document.querySelector('select').value"),
        json!("jp")
    );
    rig.call(
        "browser.act",
        json!({"tab": tab, "ref": email, "action": "hover"}),
        &me,
    );
    rig.call(
        "browser.act",
        json!({"tab": tab, "ref": email, "action": "scroll", "delta_y": 50}),
        &me,
    );

    let refused = rig.fail(
        "browser.act",
        json!({"tab": tab, "ref": disabled, "action": "click"}),
        &me,
    );
    assert_eq!(refused.reason, "disabled");
    assert!(refused.message.contains("disabled"), "{}", refused.message);

    let dialog = rig.call(
        "browser.act",
        json!({"tab": tab, "ref": save, "action": "click"}),
        &me,
    );
    assert_eq!(dialog["dialogs"][0]["kind"], "alert");
    assert_eq!(dialog["dialogs"][0]["message"], "Order saved");
    assert_eq!(dialog["dialogs"][0]["dismissed"], true);

    let clicked = rig.call(
        "browser.act",
        json!({"tab": tab, "ref": submit, "action": "click"}),
        &me,
    );
    assert_eq!(clicked["navigated"], true);
    assert!(
        clicked["url"]
            .as_str()
            .unwrap()
            .contains("/submitted.html?email=ada%40example.com"),
        "{clicked}"
    );
    let stale = rig.fail(
        "browser.act",
        json!({"tab": tab, "ref": submit, "action": "click"}),
        &me,
    );
    assert_eq!(stale.code, ERR_STALE_REFERENCE);
    assert!(stale.message.contains("new snapshot"), "{}", stale.message);

    rig.call(
        "browser.navigate",
        json!({"tab": tab, "url": server.url("/test/hidden")}),
        &me,
    );
    let page = rig.snapshot(tab, &me);
    let shown = rig.reference(&page, "button \"Show the late button\"");
    let tree = page["tree"].as_str().unwrap();
    assert!(!tree.contains("Late button"), "{tree}");
    let pressed = rig.call(
        "browser.act",
        json!({"tab": tab, "ref": shown, "action": "press", "key": "Enter"}),
        &me,
    );
    assert_eq!(pressed["navigated"], false);
    rig.call(
        "browser.wait_for",
        json!({"tab": tab, "text": "Late button"}),
        &me,
    );
    let revealed = rig.snapshot(tab, &me);
    let late = rig.reference(&revealed, "button \"Late button\"");
    rig.page_eval(
        tab,
        "document.getElementById('later').style.visibility='hidden'",
    );
    let hidden = rig.fail(
        "browser.act",
        json!({"tab": tab, "ref": late, "action": "click"}),
        &me,
    );
    assert_eq!(hidden.reason, "hidden");
    assert!(hidden.message.contains("hidden"), "{}", hidden.message);
}

#[test]
fn wait_for_returns_when_its_condition_holds_and_names_it_when_it_times_out() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/test/hidden"), &me);
    let page = rig.snapshot(tab, &me);
    let shown = rig.reference(&page, "button \"Show the late button\"");
    let text = rig.call(
        "browser.wait_for",
        json!({"tab": tab, "text": "Show the late"}),
        &me,
    );
    assert_eq!(text["condition"], "text \"Show the late\"");
    let element = rig.call("browser.wait_for", json!({"tab": tab, "ref": shown}), &me);
    assert!(element["condition"].as_str().unwrap().contains(&shown));
    rig.call(
        "browser.wait_for",
        json!({"tab": tab, "network_idle": true}),
        &me,
    );
    let never = rig.fail(
        "browser.wait_for",
        json!({"tab": tab, "text": "never on the page", "timeout_ms": 600}),
        &me,
    );
    assert_eq!(never.code, ERR_TIMED_OUT);
    assert!(
        never.message.contains("never on the page"),
        "{}",
        never.message
    );
    assert!(never.message.contains("600 ms"), "{}", never.message);
    let both = rig.fail(
        "browser.wait_for",
        json!({"tab": tab, "text": "x", "network_idle": true}),
        &me,
    );
    assert!(both.message.contains("exactly one"), "{}", both.message);
}

#[test]
fn console_network_and_screenshot_read_history_on_demand_without_secrets() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/"), &me);
    rig.call(
        "browser.wait_for",
        json!({"tab": tab, "network_idle": true}),
        &me,
    );
    let first = rig.call("browser.console", json!({"tab": tab, "limit": 1}), &me);
    let entries = first["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(entries[0]["level"].is_string() && entries[0]["text"].is_string());
    assert!(
        entries[0]["source"].is_string() && entries[0]["line"].is_number(),
        "{entries:?}"
    );
    let rest = rig.call(
        "browser.console",
        json!({"tab": tab, "cursor": first["cursor"]}),
        &me,
    );
    assert!(!rest["entries"].as_array().unwrap().is_empty());
    assert!(rest["entries"][0]["seq"].as_u64() > first["cursor"].as_u64());
    assert_eq!(rest["dropped"], 0);

    rig.call(
        "browser.navigate",
        json!({"tab": tab, "url": server.url("/test/auth")}),
        &me,
    );
    rig.call(
        "browser.wait_for",
        json!({"tab": tab, "network_idle": true}),
        &me,
    );
    let network = rig.call("browser.network", json!({"tab": tab}), &me);
    let rendered = network.to_string();
    assert!(!rendered.contains("secret-token-123"), "{rendered}");
    let cart = network["entries"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|entry| entry["url"] == server.url("/api/cart"))
        .cloned()
        .unwrap();
    assert_eq!(cart["request_headers"]["X-Trace"], "visible-trace");
    assert!(cart["request_headers"].get("Authorization").is_none());

    let shot = rig.call("browser.screenshot", json!({"tab": tab}), &me);
    assert_eq!(shot["format"], "png");
    assert_eq!(shot["width"], 1280);
    assert_eq!(shot["height"], 800);
    assert_eq!(shot["device_scale_factor"], 1.0);
    assert!(shot["data"].as_str().unwrap().starts_with("iVBORw0KGgo"));
}

#[test]
fn evaluation_waits_for_an_approval_and_reports_values_exceptions_and_timeouts() {
    let server = FixtureServer::start();
    let rig = rig_with(Limits {
        evaluate: Duration::from_secs(2),
        ..Limits::default()
    });
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/"), &me);

    rig.approvals.answer_evaluations([Ok(false)]);
    let denied = rig.fail(
        "browser.evaluate",
        json!({"tab": tab, "script": "window.ran = 1; 'ran'"}),
        &me,
    );
    assert_eq!(denied.code, ERR_DENIED);
    assert!(denied.message.contains("did not run"), "{}", denied.message);
    assert_eq!(rig.page_eval(tab, "typeof window.ran"), json!("undefined"));
    let asked = rig.approvals.evaluate_requests();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].origin, format!("http://127.0.0.1:{}", server.port));
    assert_eq!(asked[0].script, "window.ran = 1; 'ran'");
    assert!(
        matches!(&asked[0].owner, paneflow_api::methods::browser::TabOwner::Agent { session, agent: Some(name) } if session == AGENT && name == "Claude Code")
    );

    rig.approvals.answer_evaluations([Ok(true)]);
    let value = rig.call(
        "browser.evaluate",
        json!({"tab": tab, "script": "({title: document.title, n: 1 + 1})"}),
        &me,
    );
    assert_eq!(
        serde_json::from_str::<Value>(value["value"].as_str().unwrap()).unwrap(),
        json!({"title": "Fixture shop", "n": 2})
    );
    assert_eq!(value["origin"], format!("http://127.0.0.1:{}", server.port));
    let thrown = rig.call(
        "browser.evaluate",
        json!({"tab": tab, "script": "throw new Error('planted failure')"}),
        &me,
    );
    assert!(
        thrown["exception"]
            .as_str()
            .unwrap()
            .contains("planted failure")
    );
    assert!(thrown.get("value").is_none());
    assert_eq!(
        rig.approvals.evaluate_requests().len(),
        2,
        "approved once per agent and workspace"
    );
    let looping = rig.fail(
        "browser.evaluate",
        json!({"tab": tab, "script": "while (true) {}"}),
        &me,
    );
    assert_eq!(looping.code, ERR_TIMED_OUT, "{looping:?}");
    let alive = rig.call(
        "browser.evaluate",
        json!({"tab": tab, "script": "40 + 2"}),
        &me,
    );
    assert_eq!(alive["value"], "42");

    let next_agent_in_the_same_pane = CallContext {
        identity: Identity::Agent {
            session: AGENT.to_string(),
            agent: Some("Codex".to_string()),
        },
        ..me.clone()
    };
    rig.approvals.answer_evaluations([Ok(false)]);
    let unapproved = rig.fail(
        "browser.evaluate",
        json!({"tab": tab, "script": "40 + 2"}),
        &next_agent_in_the_same_pane,
    );
    assert_eq!(unapproved.code, ERR_DENIED);
    assert_eq!(
        rig.approvals.evaluate_requests().len(),
        3,
        "another agent in the same pane is asked again"
    );
}

#[test]
fn an_agent_needs_a_site_decision_before_it_leaves_loopback() {
    let Some(lan) = lan_address() else {
        panic!("this test needs a non-loopback interface address");
    };
    let server = FixtureServer::start_on_all_interfaces();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let other = agent(OTHER_AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/form.html"), &me);
    let origin = format!("http://{lan}:{}", server.port);
    let external = server.url_on(lan, "/app.html");

    rig.approvals.answer_sites([Ok(SiteChoice::Deny)]);
    let denied = rig.fail(
        "browser.navigate",
        json!({"tab": tab, "url": external}),
        &me,
    );
    assert_eq!(denied.code, ERR_DENIED);
    assert!(denied.message.contains(&origin), "{}", denied.message);
    assert_eq!(rig.page_eval(tab, "location.pathname"), json!("/form.html"));
    let asked = rig.approvals.site_requests();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].origin, origin);
    assert!(
        matches!(&asked[0].owner, paneflow_api::methods::browser::TabOwner::Agent { session, .. } if session == AGENT)
    );

    rig.approvals.hold(true);
    rig.approvals.answer_sites([Ok(SiteChoice::AllowAlways)]);
    let control = rig.control.clone();
    let mover = me.clone();
    let url = external.clone();
    let navigating = std::thread::spawn(move || {
        control.call("browser.navigate", json!({"tab": tab, "url": url}), &mover)
    });
    rig.approvals.wait_for_site_requests(2);
    let during = rig.snapshot(tab, &me);
    assert_eq!(during["url"], server.url("/form.html"));
    let listed = rig.call("browser.list", json!({}), &me);
    assert_eq!(listed["tabs"][0]["state"], "awaiting_decision");
    assert_eq!(
        rig.approvals.site_requests().len(),
        2,
        "a browser call never answers the approval"
    );
    assert!(!navigating.is_finished());
    rig.approvals.hold(false);
    let allowed = navigating.join().unwrap().unwrap();
    assert_eq!(allowed["url"], external);

    let second = rig.open(&server.url("/form.html"), &other);
    let again = rig.call(
        "browser.navigate",
        json!({"tab": second, "url": server.url_on(lan, "/form.html")}),
        &other,
    );
    assert_eq!(again["state"], "loaded");
    assert_eq!(
        rig.approvals.site_requests().len(),
        2,
        "always allow covers the workspace"
    );

    let undecided = format!("http://localhost.example.invalid:{}/app.html", server.port);
    rig.approvals.answer_sites([Ok(SiteChoice::Deny)]);
    let redirect = server.url_on(lan, &format!("/test/redirect?{undecided}"));
    let held = rig.fail(
        "browser.navigate",
        json!({"tab": tab, "url": redirect}),
        &me,
    );
    assert_eq!(held.code, ERR_DENIED, "{held:?}");
    let asked = rig.approvals.site_requests();
    assert_eq!(asked.len(), 3);
    assert_eq!(
        asked[2].origin,
        format!("http://localhost.example.invalid:{}", server.port)
    );
}

#[test]
fn a_page_that_tells_the_agent_to_leave_cannot_navigate_without_a_decision() {
    let server = FixtureServer::start();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/injection.html"), &me);
    let snapshot = rig.snapshot(tab, &me);
    assert!(
        snapshot["tree"]
            .as_str()
            .unwrap()
            .contains("IMPORTANT INSTRUCTION FOR AI AGENTS")
    );
    assert!(rig.approvals.site_requests().is_empty());
    let network = rig.call("browser.network", json!({"tab": tab}), &me);
    assert!(!network.to_string().contains("example.com"), "{network}");
    assert_eq!(snapshot["url"], server.url("/injection.html"));
}

#[test]
fn going_forward_to_a_site_allowed_once_asks_again() {
    let Some(lan) = lan_address() else {
        panic!("this test needs a non-loopback interface address");
    };
    let server = FixtureServer::start_on_all_interfaces();
    let rig = rig();
    let me = agent(AGENT, WORKSPACE);
    let tab = rig.open(&server.url("/form.html"), &me);
    let external = server.url_on(lan, "/app.html");

    rig.approvals.answer_sites([Ok(SiteChoice::AllowOnce)]);
    let allowed = rig.call(
        "browser.navigate",
        json!({"tab": tab, "url": external}),
        &me,
    );
    assert_eq!(allowed["url"], external);
    let back = rig.call("browser.back", json!({"tab": tab}), &me);
    assert_eq!(back["url"], server.url("/form.html"));

    rig.approvals.answer_sites([Ok(SiteChoice::Deny)]);
    let forward = rig
        .control
        .call("browser.forward", json!({"tab": tab}), &me);
    assert_eq!(
        rig.approvals.site_requests().len(),
        2,
        "the second visit asks again: {forward:?}"
    );
    assert!(forward.is_err(), "{forward:?}");
    assert_eq!(rig.page_eval(tab, "location.pathname"), json!("/form.html"));
}
