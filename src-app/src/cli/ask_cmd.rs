use std::time::Duration;

use clap::Args;
use paneflow_api::methods::prompt::{DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS, MAX_WAIT_MS};
use serde_json::{Map, Value, json};

use super::sliced_wait::{Budget, Clock, SystemClock};
use super::{CliError, EXIT_OK, EXIT_RUNTIME, EXIT_TIMEOUT};

pub(super) const DEFAULT_ASK_WAIT: Duration = Duration::from_secs(20);
pub(super) const ASKER_LABEL: &str = "paneflow ask";
pub(super) const NO_WINDOW: &str = "no Paneflow window can show the question";

#[derive(Args, Debug)]
pub(super) struct AskArgs {
    #[arg(
        value_name = "QUESTION",
        required_unless_present = "resume",
        help = "The question shown to the user in the Paneflow window"
    )]
    question: Option<String>,
    #[arg(
        long = "option",
        value_name = "TEXT",
        help = "A choice the user picks from; repeat it for each choice. Without it the user types a line"
    )]
    options: Vec<String>,
    #[arg(
        long,
        help = "Let the user pick several options; each picked option is printed on its own line"
    )]
    multiple: bool,
    #[arg(
        long,
        value_name = "DURATION",
        help = "How long to wait for the answer, such as 20s, 10m or 1h; 20s by default. Past it the CLI prints `pending <id>` and exits 4"
    )]
    timeout: Option<String>,
    #[arg(
        long,
        value_name = "ID",
        conflicts_with_all = ["question", "options", "multiple"],
        help = "Keep waiting for the question a previous `paneflow ask` printed as `pending <id>`"
    )]
    resume: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Printed {
    pub(super) stdout: Vec<String>,
    pub(super) code: i32,
}

fn env_id(name: &str) -> Option<u64> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
}

fn spec_timeout_secs(limit: Duration) -> u64 {
    limit
        .as_secs()
        .saturating_add(1)
        .clamp(DEFAULT_TIMEOUT_SECS, MAX_TIMEOUT_SECS)
}

pub(super) fn request_params(
    question: &str,
    options: &[String],
    multiple: bool,
    limit: Duration,
    wait_ms: u64,
    place: (Option<u64>, Option<u64>),
) -> Value {
    let mut origin = Map::new();
    origin.insert("kind".into(), json!("script"));
    origin.insert("file".into(), json!(ASKER_LABEL));
    if let Some(surface) = place.0 {
        origin.insert("surface_id".into(), json!(surface));
    }
    if let Some(workspace) = place.1 {
        origin.insert("workspace_id".into(), json!(workspace));
    }
    let mut params = Map::new();
    params.insert("title".into(), json!(question));
    if options.is_empty() {
        params.insert("kind".into(), json!("input"));
    } else {
        params.insert("kind".into(), json!("pick"));
        params.insert("items".into(), json!(options));
        if multiple {
            params.insert("multiple".into(), json!(true));
        }
    }
    params.insert("timeout".into(), json!(spec_timeout_secs(limit)));
    params.insert("origin".into(), Value::Object(origin));
    params.insert("wait_ms".into(), json!(wait_ms));
    Value::Object(params)
}

fn answer_lines(answer: &Value) -> Vec<String> {
    match answer {
        Value::String(text) => vec![text.clone()],
        Value::Array(ids) => ids
            .iter()
            .map(|id| id.as_str().map_or_else(|| id.to_string(), str::to_string))
            .collect(),
        Value::Object(other) => vec![
            other
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ],
        other => vec![other.to_string()],
    }
}

pub(super) fn printed(outcome: &Value) -> Result<Printed, CliError> {
    let status = outcome["status"].as_str().unwrap_or_default();
    let refused = |message: &str| Err(CliError::runtime(message.to_string()));
    match status {
        "answered" => Ok(Printed {
            stdout: answer_lines(&outcome["answer"]),
            code: EXIT_OK,
        }),
        "pending" => Ok(Printed {
            stdout: vec![format!("pending {}", outcome["id"])],
            code: EXIT_TIMEOUT,
        }),
        "rate_limited" => Ok(Printed {
            stdout: vec!["rate_limited".to_string()],
            code: EXIT_RUNTIME,
        }),
        "no_client" => refused(NO_WINDOW),
        "cancelled" => refused("the user dismissed the question"),
        "timeout" => refused("the question expired before anyone answered it"),
        "busy" => {
            refused("too many questions already wait in Paneflow; ask again once one is answered")
        }
        "unsupported" => refused("the Paneflow window cannot show this question"),
        "failed" => refused(outcome["detail"].as_str().unwrap_or("the question failed")),
        other => Err(CliError::runtime(format!(
            "prompt.request returned an unknown status '{other}'"
        ))),
    }
}

fn check(args: &AskArgs) -> Result<(), CliError> {
    if args.multiple && args.options.is_empty() {
        return Err(CliError::runtime(
            "--multiple needs the choices to pick from: pass --option for each one",
        ));
    }
    Ok(())
}

pub(super) fn ask(
    call: &mut dyn FnMut(&str, Value) -> Result<Value, CliError>,
    clock: &dyn Clock,
    args: &AskArgs,
    place: (Option<u64>, Option<u64>),
) -> Result<Printed, CliError> {
    check(args)?;
    let limit = args
        .timeout
        .as_deref()
        .map(super::do_cmd::parse_duration)
        .transpose()?
        .unwrap_or(DEFAULT_ASK_WAIT);
    let budget = Budget::new(clock, Some(limit), MAX_WAIT_MS);
    let (mut outcome, mut id) = match (args.resume, args.question.as_deref()) {
        (Some(id), _) => (
            call(
                "prompt.wait",
                json!({"id": id, "wait_ms": budget.slice_ms()}),
            )?,
            Some(id),
        ),
        (None, Some(question)) => (
            call(
                "prompt.request",
                request_params(
                    question,
                    &args.options,
                    args.multiple,
                    limit,
                    budget.slice_ms(),
                    place,
                ),
            )?,
            None,
        ),
        (None, None) => {
            return Err(CliError::runtime(
                "paneflow ask needs a question, or --resume <id>",
            ));
        }
    };
    while outcome["status"] == "pending" && !budget.spent() {
        let Some(pending) = outcome["id"].as_u64().or(id) else {
            break;
        };
        id = Some(pending);
        outcome = call(
            "prompt.wait",
            json!({"id": pending, "wait_ms": budget.slice_ms()}),
        )?;
    }
    if outcome["status"] == "pending" && outcome.get("id").is_none_or(Value::is_null) {
        outcome["id"] = json!(id);
    }
    printed(&outcome)
}

pub(super) fn run(args: &AskArgs) -> Result<i32, CliError> {
    check(args)?;
    let owners = super::router::Owners::from_environment()?;
    let clock = SystemClock::start();
    let printed = ask(
        &mut |method, params| owners.call(method, params),
        &clock,
        args,
        (
            env_id("PANEFLOW_SURFACE_ID"),
            env_id("PANEFLOW_WORKSPACE_ID"),
        ),
    )?;
    for line in &printed.stdout {
        println!("{line}");
    }
    Ok(printed.code)
}

#[cfg(test)]
mod tests {
    use super::super::sliced_wait::fake::FakeClock;
    use super::*;

    fn args(question: Option<&str>, options: &[&str], timeout: Option<&str>) -> AskArgs {
        AskArgs {
            question: question.map(str::to_string),
            options: options.iter().map(|option| (*option).to_string()).collect(),
            multiple: false,
            timeout: timeout.map(str::to_string),
            resume: None,
        }
    }

    struct Scripted<'c> {
        clock: &'c FakeClock,
        answer_at_ms: u64,
        calls: Vec<(String, Value)>,
    }

    impl Scripted<'_> {
        fn call(&mut self, method: &str, params: Value) -> Result<Value, CliError> {
            self.calls.push((method.to_string(), params.clone()));
            let wait = params["wait_ms"].as_u64().unwrap_or(0);
            let now = self.clock.now_ms();
            if now + wait >= self.answer_at_ms {
                self.clock.advance_ms(self.answer_at_ms.saturating_sub(now));
                return Ok(json!({"status": "answered", "id": 7, "answer": "yes"}));
            }
            self.clock.advance_ms(wait);
            Ok(json!({"status": "pending", "id": 7}))
        }
    }

    #[test]
    fn an_answer_after_forty_seconds_arrives_through_waits_of_at_most_twenty_five_seconds() {
        let clock = FakeClock::default();
        let mut scripted = Scripted {
            clock: &clock,
            answer_at_ms: 40_000,
            calls: Vec::new(),
        };
        let printed = ask(
            &mut |method, params| scripted.call(method, params),
            &clock,
            &args(Some("Deploy now?"), &["yes", "no"], Some("60s")),
            (None, None),
        )
        .unwrap();
        assert_eq!(
            printed,
            Printed {
                stdout: vec!["yes".into()],
                code: EXIT_OK
            }
        );
        let methods: Vec<&str> = scripted.calls.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(methods, ["prompt.request", "prompt.wait"]);
        assert!(
            scripted
                .calls
                .iter()
                .all(|(_, params)| params["wait_ms"].as_u64().unwrap() <= MAX_WAIT_MS),
            "{:?}",
            scripted.calls
        );
        assert_eq!(scripted.calls[1].1["id"], 7);
        assert_eq!(scripted.calls[0].1["kind"], "pick");
        assert_eq!(scripted.calls[0].1["items"], json!(["yes", "no"]));
        assert_eq!(
            scripted.calls[0].1["origin"],
            json!({"kind": "script", "file": ASKER_LABEL})
        );
    }

    #[test]
    fn without_a_timeout_the_question_is_left_pending_after_twenty_seconds() {
        let clock = FakeClock::default();
        let mut scripted = Scripted {
            clock: &clock,
            answer_at_ms: u64::MAX,
            calls: Vec::new(),
        };
        let printed = ask(
            &mut |method, params| scripted.call(method, params),
            &clock,
            &args(Some("Branch name?"), &[], None),
            (None, None),
        )
        .unwrap();
        assert_eq!(
            printed,
            Printed {
                stdout: vec!["pending 7".into()],
                code: EXIT_TIMEOUT
            }
        );
        assert_eq!(clock.now_ms(), 20_000);
        assert_eq!(scripted.calls.len(), 1);
        assert_eq!(scripted.calls[0].1["kind"], "input");
        assert_eq!(scripted.calls[0].1["wait_ms"], 20_000);
    }

    #[test]
    fn a_resumed_question_waits_on_its_id_and_prints_the_answer() {
        let clock = FakeClock::default();
        let mut scripted = Scripted {
            clock: &clock,
            answer_at_ms: 30_000,
            calls: Vec::new(),
        };
        let mut resumed = args(None, &[], Some("45s"));
        resumed.resume = Some(7);
        let printed = ask(
            &mut |method, params| scripted.call(method, params),
            &clock,
            &resumed,
            (None, None),
        )
        .unwrap();
        assert_eq!(printed.stdout, ["yes"]);
        assert!(
            scripted
                .calls
                .iter()
                .all(|(method, params)| method == "prompt.wait" && params["id"] == 7)
        );
    }

    #[test]
    fn multiple_without_options_is_refused_before_any_request() {
        let mut asked = args(Some("Which?"), &[], None);
        asked.multiple = true;
        let mut called = false;
        let refused = ask(
            &mut |_, _| {
                called = true;
                Ok(json!({}))
            },
            &FakeClock::default(),
            &asked,
            (None, None),
        )
        .unwrap_err();
        assert!(!called);
        assert_eq!(refused.code, EXIT_RUNTIME);
        assert!(refused.message.contains("--option"), "{}", refused.message);
    }

    #[test]
    fn each_outcome_prints_or_fails_as_documented() {
        assert_eq!(
            printed(&json!({"status": "answered", "answer": ["a", "b"]})).unwrap(),
            Printed {
                stdout: vec!["a".into(), "b".into()],
                code: EXIT_OK
            }
        );
        assert_eq!(
            printed(&json!({"status": "answered", "answer": {"text": "typed"}}))
                .unwrap()
                .stdout,
            ["typed"]
        );
        assert_eq!(
            printed(&json!({"status": "rate_limited"})).unwrap(),
            Printed {
                stdout: vec!["rate_limited".into()],
                code: EXIT_RUNTIME
            }
        );
        let refused = printed(&json!({"status": "no_client"})).unwrap_err();
        assert_eq!(refused.code, EXIT_RUNTIME);
        assert_eq!(refused.message, NO_WINDOW);
    }

    #[test]
    fn a_long_wait_keeps_the_question_open_at_least_as_long() {
        let params = request_params(
            "q",
            &[],
            false,
            Duration::from_secs(1800),
            25_000,
            (Some(3), None),
        );
        assert_eq!(params["timeout"], 1801);
        assert_eq!(params["origin"]["surface_id"], 3);
        assert_eq!(
            request_params("q", &[], false, DEFAULT_ASK_WAIT, 0, (None, None))["timeout"],
            DEFAULT_TIMEOUT_SECS
        );
    }
}
