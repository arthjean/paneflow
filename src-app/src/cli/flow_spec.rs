use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::up_cmd::{DEFAULT_PORT_BASE, extract_tokens};
use super::workspace_spec::{LayoutPreset, PaneSpec, validate_pane};
use crate::layout::MAX_PANES;

pub const MAX_CAPTURE_LINES: u64 = 500;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowSpec {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub layout: LayoutPreset,
    #[serde(default)]
    pub port_base: Option<u16>,
    #[serde(default)]
    pub defaults: FlowDefaults,
    #[serde(default, rename = "step")]
    pub steps: Vec<StepSpec>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowDefaults {
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub on_failure: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepSpec {
    pub id: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub foreach: Option<Vec<String>>,
    #[serde(default)]
    pub pane: Option<PaneSpec>,
    #[serde(default)]
    pub send: Option<SendSpec>,
    #[serde(default)]
    pub ready: Option<ReadySpec>,
    #[serde(default)]
    pub capture: Option<CaptureSpec>,
    #[serde(default)]
    pub submit: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendSpec {
    pub target: String,
    pub text: String,
    #[serde(default)]
    pub submit: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadySpec {
    pub pattern: String,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSpec {
    pub var: String,
    pub lines: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnFailure {
    #[default]
    FailFast,
    Continue,
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub id: String,
    pub group: String,
    pub needs: Vec<String>,
    pub action: UnitAction,
    pub ready: Option<(String, u64)>,
    pub capture: Option<(String, u64)>,
    pub submit: bool,
}

#[derive(Debug, Clone)]
pub enum UnitAction {
    Spawn(Box<SpawnUnit>),
    Send { target: String, text: String },
}

#[derive(Debug, Clone)]
pub struct SpawnUnit {
    pub pane: PaneSpec,
    pub name: String,
}

#[derive(Debug)]
pub struct FlowPlan {
    pub name: String,
    pub layout: LayoutPreset,
    pub port_base: u16,
    pub on_failure: OnFailure,
    pub units: Vec<Unit>,
}

impl FlowPlan {
    pub fn requires_submit(&self) -> bool {
        self.units.iter().any(|u| u.submit)
    }
}

pub fn load(src: &str) -> Result<FlowPlan, String> {
    let spec: FlowSpec = toml::from_str(src).map_err(|e| e.to_string())?;
    validate(&spec)?;
    expand(&spec)
}

fn validate(spec: &FlowSpec) -> Result<(), String> {
    if spec.steps.is_empty() {
        return Err("flow has no [[step]]".to_string());
    }

    let mut ids = HashSet::new();
    for step in &spec.steps {
        if step.id.is_empty() {
            return Err("a [[step]] has an empty `id`".to_string());
        }
        if step.id.contains(['[', ']']) {
            return Err(format!(
                "step '{}': `id` must not contain brackets",
                step.id
            ));
        }
        if !ids.insert(step.id.as_str()) {
            return Err(format!("duplicate step id '{}'", step.id));
        }
    }

    if let Some(policy) = spec.defaults.on_failure.as_deref()
        && !matches!(policy, "fail_fast" | "continue")
    {
        return Err(format!(
            "defaults.on_failure must be \"fail_fast\" or \"continue\", got '{policy}'"
        ));
    }

    let mut vars: HashMap<&str, &StepSpec> = HashMap::new();
    for step in &spec.steps {
        if let Some(cap) = &step.capture {
            if cap.var.is_empty()
                || !cap
                    .var
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
            {
                return Err(format!(
                    "step '{}': capture var '{}' must be [A-Za-z0-9_]+",
                    step.id, cap.var
                ));
            }
            if cap.lines == 0 || cap.lines > MAX_CAPTURE_LINES {
                return Err(format!(
                    "step '{}': capture.lines must be 1..={MAX_CAPTURE_LINES}",
                    step.id
                ));
            }
            if step.ready.is_none() {
                return Err(format!(
                    "step '{}': `capture` requires `ready` (captured at match time)",
                    step.id
                ));
            }
            if let Some(prev) = vars.insert(cap.var.as_str(), step) {
                return Err(format!(
                    "capture var '{}' declared by both '{}' and '{}'",
                    cap.var, prev.id, step.id
                ));
            }
        }
    }

    let mut spawn_units = 0usize;
    let mut has_root_spawn = false;
    for step in &spec.steps {
        match (&step.pane, &step.send) {
            (Some(_), Some(_)) => {
                return Err(format!(
                    "step '{}': set either `pane` or `send`, not both",
                    step.id
                ));
            }
            (None, None) => {
                return Err(format!("step '{}': needs a `pane` or a `send`", step.id));
            }
            _ => {}
        }

        for need in &step.needs {
            if need == &step.id {
                return Err(format!("step '{}': depends on itself", step.id));
            }
            if !ids.contains(need.as_str()) {
                let known: Vec<&str> = ids.iter().copied().collect();
                return Err(format!(
                    "step '{}': unknown dependency '{need}' (known: {})",
                    step.id,
                    known.join(", ")
                ));
            }
        }

        if let Some(ready) = &step.ready {
            if ready.pattern.is_empty() {
                return Err(format!("step '{}': ready.pattern is empty", step.id));
            }
            regex::Regex::new(&ready.pattern.replace("${item}", "x"))
                .map_err(|e| format!("step '{}': invalid ready.pattern: {e}", step.id))?;
            if ready.timeout_secs.or(spec.defaults.timeout_secs).is_none() {
                return Err(format!(
                    "step '{}': `ready` needs `timeout_secs` (own or [defaults])",
                    step.id
                ));
            }
        }

        if let Some(items) = &step.foreach {
            if items.is_empty() {
                return Err(format!("step '{}': `foreach` is empty", step.id));
            }
            let mut seen = HashSet::new();
            for item in items {
                if !seen.insert(item.as_str()) {
                    return Err(format!(
                        "step '{}': duplicate foreach item '{item}'",
                        step.id
                    ));
                }
            }
        }

        if let Some(pane) = &step.pane {
            validate_pane(0, pane)
                .map_err(|e| format!("step '{}': {}", step.id, e.replace("pane 0: ", "")))?;
            spawn_units += step.foreach.as_ref().map_or(1, Vec::len);
            if step.needs.is_empty() {
                has_root_spawn = true;
            }
        }

        if let Some(send) = &step.send {
            if send.target.is_empty() {
                return Err(format!("step '{}': send.target is empty", step.id));
            }
            if step.needs.is_empty() {
                return Err(format!(
                    "step '{}': a `send` step needs at least one dependency",
                    step.id
                ));
            }
            if step.submit.is_some() {
                return Err(format!(
                    "step '{}': put `submit` inside `send` for send steps",
                    step.id
                ));
            }
        }

        validate_tokens(spec, step, &vars)?;
    }

    if spawn_units > MAX_PANES {
        return Err(format!(
            "flow spawns {spawn_units} panes, exceeds MAX_PANES ({MAX_PANES})"
        ));
    }
    detect_cycles(spec)?;
    if !has_root_spawn {
        return Err(
            "flow needs at least one `pane` step without `needs` (the workspace bootstrap)"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_tokens(
    spec: &FlowSpec,
    step: &StepSpec,
    vars: &HashMap<&str, &StepSpec>,
) -> Result<(), String> {
    let is_foreach = step.foreach.is_some();

    let mut item_only: Vec<(&str, &str)> = Vec::new();
    let mut runtime: Vec<(&str, &str)> = Vec::new();

    if let Some(pane) = &step.pane {
        for (label, value) in [
            ("pane.cwd", pane.cwd.as_deref()),
            ("pane.name", pane.name.as_deref()),
            ("pane.worktree", pane.worktree.as_deref()),
        ] {
            if let Some(v) = value {
                item_only.push((label, v));
            }
        }
        if let Some(env) = &pane.env {
            for v in env.values() {
                item_only.push(("pane.env", v));
            }
        }
        if let Some(p) = pane.prompt.as_deref() {
            runtime.push(("pane.prompt", p));
        }
    }
    if let Some(send) = &step.send {
        item_only.push(("send.target", &send.target));
        runtime.push(("send.text", &send.text));
    }
    if let Some(ready) = &step.ready {
        item_only.push(("ready.pattern", &ready.pattern));
    }

    for (label, value) in &item_only {
        for token in extract_tokens(value).map_err(|e| err_in(step, label, &e))? {
            if token == "item" {
                if !is_foreach {
                    return Err(err_in(step, label, "`${item}` outside a `foreach` step"));
                }
            } else {
                if *label == "pane.env" && token == "port_offset" {
                    continue;
                }
                let supported = if *label == "pane.env" {
                    "`${item}` or `${port_offset}`"
                } else {
                    "`${item}`"
                };
                return Err(err_in(
                    step,
                    label,
                    &format!("only {supported} is allowed here, got '${{{token}}}'"),
                ));
            }
        }
    }

    for (label, value) in &runtime {
        for token in extract_tokens(value).map_err(|e| err_in(step, label, &e))? {
            if token == "item" {
                if !is_foreach {
                    return Err(err_in(step, label, "`${item}` outside a `foreach` step"));
                }
                continue;
            }
            let (var, suffix) = match token.split_once('.') {
                Some((v, s)) => (v, Some(s)),
                None => (token, None),
            };
            let Some(owner) = vars.get(var) else {
                return Err(err_in(
                    step,
                    label,
                    &format!("unknown variable '${{{token}}}' (no step captures '{var}')"),
                ));
            };
            if *label == "pane.prompt" && !step.submit.unwrap_or(false) {
                return Err(err_in(
                    step,
                    label,
                    &format!(
                        "capture variable '${{{token}}}' in a non-submitting prompt \
                         would be prefilled verbatim; set `submit = true` on this \
                         step or move the reference to a `send` step"
                    ),
                ));
            }
            match (&owner.foreach, suffix) {
                (Some(items), Some(s)) => {
                    if !items.iter().any(|i| i == s) {
                        return Err(err_in(
                            step,
                            label,
                            &format!(
                                "'${{{token}}}': '{s}' is not a foreach item of step '{}'",
                                owner.id
                            ),
                        ));
                    }
                }
                (Some(_), None) => {
                    return Err(err_in(
                        step,
                        label,
                        &format!(
                            "step '{}' captures '{var}' per foreach item; use `${{{var}.<item>}}`",
                            owner.id
                        ),
                    ));
                }
                (None, Some(_)) => {
                    return Err(err_in(
                        step,
                        label,
                        &format!("'{var}' is not a foreach capture; use `${{{var}}}`"),
                    ));
                }
                (None, None) => {}
            }
            if !step_depends_on(spec, step.id.as_str(), owner.id.as_str()) {
                return Err(err_in(
                    step,
                    label,
                    &format!(
                        "capture variable '${{{token}}}' is produced by step '{}' but \
                         that step is not a transitive dependency",
                        owner.id
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn err_in(step: &StepSpec, label: &str, msg: &str) -> String {
    format!("step '{}': {label}: {msg}", step.id)
}

fn step_depends_on(spec: &FlowSpec, step_id: &str, dependency: &str) -> bool {
    let index: HashMap<&str, &StepSpec> = spec.steps.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut seen = HashSet::new();

    fn visit<'a>(
        id: &'a str,
        dependency: &str,
        index: &HashMap<&'a str, &'a StepSpec>,
        seen: &mut HashSet<&'a str>,
    ) -> bool {
        if !seen.insert(id) {
            return false;
        }
        let Some(step) = index.get(id) else {
            return false;
        };
        step.needs
            .iter()
            .any(|need| need == dependency || visit(need, dependency, index, seen))
    }

    visit(step_id, dependency, &index, &mut seen)
}

fn detect_cycles(spec: &FlowSpec) -> Result<(), String> {
    let index: HashMap<&str, &StepSpec> = spec.steps.iter().map(|s| (s.id.as_str(), s)).collect();
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        White,
        Grey,
        Black,
    }
    let mut marks: HashMap<&str, Mark> = spec
        .steps
        .iter()
        .map(|s| (s.id.as_str(), Mark::White))
        .collect();

    fn visit<'a>(
        id: &'a str,
        index: &HashMap<&'a str, &'a StepSpec>,
        marks: &mut HashMap<&'a str, Mark>,
        path: &mut Vec<&'a str>,
    ) -> Result<(), String> {
        match marks[id] {
            Mark::Black => return Ok(()),
            Mark::Grey => {
                let start = path.iter().position(|p| *p == id).unwrap_or(0);
                let mut cycle: Vec<&str> = path[start..].to_vec();
                cycle.push(id);
                return Err(format!("dependency cycle: {}", cycle.join(" → ")));
            }
            Mark::White => {}
        }
        marks.insert(id, Mark::Grey);
        path.push(id);
        for need in &index[id].needs {
            visit(need.as_str(), index, marks, path)?;
        }
        path.pop();
        marks.insert(id, Mark::Black);
        Ok(())
    }

    let mut path = Vec::new();
    for step in &spec.steps {
        visit(step.id.as_str(), &index, &mut marks, &mut path)?;
    }
    Ok(())
}

fn expand(spec: &FlowSpec) -> Result<FlowPlan, String> {
    let mut units = Vec::new();
    let mut pane_names = HashSet::new();
    for step in &spec.steps {
        let items: Vec<Option<&str>> = match &step.foreach {
            Some(items) => items.iter().map(|i| Some(i.as_str())).collect(),
            None => vec![None],
        };
        for item in items {
            let unit = instantiate(spec, step, item)?;
            if let UnitAction::Spawn(s) = &unit.action
                && !pane_names.insert(s.name.clone())
            {
                return Err(format!(
                    "duplicate pane name '{}' (unit '{}'); pane names must be \
                     unique so `send.target` resolves",
                    s.name, unit.id
                ));
            }
            units.push(unit);
        }
    }
    Ok(FlowPlan {
        name: spec.name.clone().unwrap_or_else(|| "flow".to_string()),
        layout: spec.layout,
        port_base: spec.port_base.unwrap_or(DEFAULT_PORT_BASE),
        on_failure: match spec.defaults.on_failure.as_deref() {
            Some("continue") => OnFailure::Continue,
            _ => OnFailure::FailFast,
        },
        units,
    })
}

fn instantiate(spec: &FlowSpec, step: &StepSpec, item: Option<&str>) -> Result<Unit, String> {
    let sub = |s: &str| -> String {
        match item {
            Some(it) => s.replace("${item}", it),
            None => s.to_string(),
        }
    };
    let id = match item {
        Some(it) => format!("{}[{}]", step.id, it),
        None => step.id.clone(),
    };

    let action = if let Some(pane) = &step.pane {
        let mut pane = pane.clone();
        pane.cwd = pane.cwd.map(|v| sub(&v));
        pane.prompt = pane.prompt.map(|v| sub(&v));
        pane.name = pane.name.map(|v| sub(&v));
        pane.worktree = pane.worktree.map(|v| sub(&v));
        pane.env = pane
            .env
            .map(|env| env.into_iter().map(|(k, v)| (k, sub(&v))).collect());
        validate_pane(0, &pane)
            .map_err(|e| format!("unit '{}': {}", id, e.replace("pane 0: ", "")))?;
        let name = pane.name.clone().unwrap_or_else(|| match item {
            Some(it) => format!("{}-{}", step.id, it),
            None => step.id.clone(),
        });
        pane.name = Some(name.clone());
        UnitAction::Spawn(Box::new(SpawnUnit { pane, name }))
    } else {
        let send = step.send.as_ref().expect("validated: pane XOR send");
        UnitAction::Send {
            target: sub(&send.target),
            text: sub(&send.text),
        }
    };

    let ready = match step.ready.as_ref() {
        Some(r) => {
            let timeout = r
                .timeout_secs
                .or(spec.defaults.timeout_secs)
                .expect("validated: ready has a timeout");
            let pattern = sub(&r.pattern);
            regex::Regex::new(&pattern).map_err(|e| {
                format!("unit '{id}': ready.pattern invalid after ${{item}} substitution: {e}")
            })?;
            Some((pattern, timeout))
        }
        None => None,
    };
    let capture = step.capture.as_ref().map(|c| {
        let key = match item {
            Some(it) => format!("{}.{}", c.var, it),
            None => c.var.clone(),
        };
        (key, c.lines)
    });
    let submit = step
        .submit
        .or(step.send.as_ref().and_then(|s| s.submit))
        .unwrap_or(false);

    Ok(Unit {
        id,
        group: step.id.clone(),
        needs: step.needs.clone(),
        action,
        ready,
        capture,
        submit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(extra: &str) -> String {
        format!(
            "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"root\"\npane = {{ command = \"true\" }}\n{extra}"
        )
    }

    #[test]
    fn demo_review_pipeline_flow_is_valid() {
        let src = include_str!("../../../examples/review-pipeline.flow.toml");
        let plan = load(src).expect("the demo flow.toml must be a valid flow");
        assert_eq!(plan.name, "review-pipeline");
        let ids: Vec<&str> = plan.units.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, vec!["impl", "review"]);
        assert!(plan.requires_submit(), "the demo exercises the submit gate");
    }

    #[test]
    fn loads_a_minimal_flow() {
        let plan = load(&minimal("")).expect("valid");
        assert_eq!(plan.units.len(), 1);
        assert_eq!(plan.on_failure, OnFailure::FailFast);
        assert!(!plan.requires_submit());
    }

    #[test]
    fn rejects_unknown_dependency_listing_known_ids() {
        let err = load(&minimal(
            "[[step]]\nid = \"b\"\nneeds = [\"nope\"]\nsend = { target = \"root\", text = \"x\" }\n",
        ))
        .unwrap_err();
        assert!(err.contains("unknown dependency 'nope'"), "got: {err}");
        assert!(err.contains("root"), "lists known ids: {err}");
    }

    #[test]
    fn rejects_a_cycle_naming_it() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"root\"\npane = { command = \"true\" }\n\n[[step]]\nid = \"a\"\nneeds = [\"b\"]\nsend = { target = \"root\", text = \"x\" }\n\n[[step]]\nid = \"b\"\nneeds = [\"a\"]\nsend = { target = \"root\", text = \"x\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("dependency cycle"), "got: {err}");
        assert!(err.contains("a") && err.contains("b"), "names it: {err}");
    }

    #[test]
    fn ready_without_any_timeout_is_an_error() {
        let src = "[[step]]\nid = \"root\"\npane = { command = \"true\" }\nready = { pattern = \"done\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("timeout_secs"), "got: {err}");
    }

    #[test]
    fn foreach_expands_with_item_substitution_and_fan_in_groups() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"shard\"\nforeach = [\"api\", \"ui\"]\npane = { command = \"true\", prompt = \"fix ${item}\", env = { NAME = \"${item}\", PORT = \"${port_offset}\" } }\nready = { pattern = \"done ${item}\" }\ncapture = { var = \"out\", lines = 5 }\n";
        let plan = load(src).expect("valid");
        assert_eq!(plan.units.len(), 2);
        assert_eq!(plan.units[0].id, "shard[api]");
        assert_eq!(plan.units[0].group, "shard");
        let UnitAction::Spawn(s) = &plan.units[0].action else {
            panic!("spawn");
        };
        assert_eq!(s.pane.prompt.as_deref(), Some("fix api"));
        let env = s.pane.env.as_ref().expect("env");
        assert_eq!(env["NAME"], "api");
        assert_eq!(
            env["PORT"], "${port_offset}",
            "port substitution happens in flow_cmd with the same allocator as `up`"
        );
        assert_eq!(s.name, "shard-api");
        assert_eq!(plan.units[0].ready.as_ref().unwrap().0, "done api");
        assert_eq!(plan.units[0].capture.as_ref().unwrap().0, "out.api");
        assert_eq!(plan.units[1].capture.as_ref().unwrap().0, "out.ui");
    }

    #[test]
    fn empty_foreach_is_an_error() {
        let err = load(&minimal(
            "[[step]]\nid = \"s\"\nforeach = []\nneeds = [\"root\"]\npane = { command = \"true\" }\n",
        ))
        .unwrap_err();
        assert!(err.contains("`foreach` is empty"), "got: {err}");
    }

    #[test]
    fn item_outside_foreach_is_an_error() {
        let err = load(&minimal(
            "[[step]]\nid = \"s\"\nneeds = [\"root\"]\npane = { command = \"true\", prompt = \"x ${item}\" }\n",
        ))
        .unwrap_err();
        assert!(err.contains("${item}"), "got: {err}");
        assert!(err.contains("foreach"), "got: {err}");
    }

    #[test]
    fn unknown_variable_is_an_error() {
        let err = load(&minimal(
            "[[step]]\nid = \"s\"\nneeds = [\"root\"]\nsend = { target = \"root\", text = \"${nope}\" }\n",
        ))
        .unwrap_err();
        assert!(err.contains("unknown variable"), "got: {err}");

        let err = load(
            "[[step]]\nid = \"root\"\npane = { command = \"true\", env = { X = \"${typo}\" } }\n",
        )
        .unwrap_err();
        assert!(err.contains("port_offset"), "got: {err}");
    }

    #[test]
    fn capture_ref_must_be_reachable_through_needs() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"root\"\npane = { command = \"true\" }\n\n[[step]]\nid = \"producer\"\nneeds = [\"root\"]\npane = { command = \"true\" }\nready = { pattern = \"ok\" }\ncapture = { var = \"sum\", lines = 1 }\n\n[[step]]\nid = \"consumer\"\nneeds = [\"root\"]\nsend = { target = \"root\", text = \"${sum}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("not a transitive dependency"), "got: {err}");
    }

    #[test]
    fn plain_ref_to_a_foreach_capture_is_an_error() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"shard\"\nforeach = [\"api\", \"ui\"]\npane = { command = \"true\" }\nready = { pattern = \"ok\" }\ncapture = { var = \"out\", lines = 5 }\n\n[[step]]\nid = \"merge\"\nneeds = [\"shard\"]\nsend = { target = \"shard-api\", text = \"all: ${out}\" }\n";
        let err = load(src).unwrap_err();
        assert!(
            err.contains("${out.<item>}") || err.contains("out.<item>"),
            "got: {err}"
        );
        let ok = src.replace("${out}", "${out.api} ${out.ui}");
        load(&ok).expect("suffixed refs are valid");
    }

    #[test]
    fn suffixed_ref_to_unknown_item_is_an_error() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"shard\"\nforeach = [\"api\"]\npane = { command = \"true\" }\nready = { pattern = \"ok\" }\ncapture = { var = \"out\", lines = 5 }\n\n[[step]]\nid = \"merge\"\nneeds = [\"shard\"]\nsend = { target = \"shard-api\", text = \"${out.db}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("'db' is not a foreach item"), "got: {err}");
    }

    #[test]
    fn pane_budget_is_enforced_statically() {
        let items: Vec<String> = (0..MAX_PANES + 1).map(|i| format!("\"i{i}\"")).collect();
        let src = format!(
            "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"s\"\nforeach = [{}]\npane = {{ command = \"true\" }}\n",
            items.join(", ")
        );
        let err = load(&src).unwrap_err();
        assert!(err.contains("MAX_PANES"), "got: {err}");
    }

    #[test]
    fn send_step_requires_a_dependency_and_a_root_spawn_exists() {
        let err =
            load("[[step]]\nid = \"s\"\nsend = { target = \"x\", text = \"y\" }\n").unwrap_err();
        assert!(err.contains("needs at least one dependency"), "got: {err}");
        let err = load(
            "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"a\"\nneeds = [\"b\"]\npane = { command = \"true\" }\n\n[[step]]\nid = \"b\"\npane = { command = \"true\" }\nneeds = [\"a\"]\n",
        )
        .unwrap_err();
        assert!(err.contains("cycle"), "got: {err}");
    }

    #[test]
    fn capture_requires_ready_and_bounded_lines() {
        let err = load(&minimal(
            "[[step]]\nid = \"s\"\nneeds = [\"root\"]\npane = { command = \"true\" }\ncapture = { var = \"v\", lines = 5 }\n",
        ))
        .unwrap_err();
        assert!(err.contains("requires `ready`"), "got: {err}");
        let err = load(
            "[[step]]\nid = \"root\"\npane = { command = \"true\" }\nready = { pattern = \"x\", timeout_secs = 5 }\ncapture = { var = \"v\", lines = 0 }\n",
        )
        .unwrap_err();
        assert!(err.contains("capture.lines"), "got: {err}");
    }

    #[test]
    fn expanded_item_cannot_smuggle_a_flag_branch_past_validation() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"s\"\nforeach = [\"-evil\"]\npane = { cwd = \"/tmp\", command = \"true\", worktree = \"${item}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("must not start with '-'"), "got: {err}");
        let src = src.replace("-evil", "..");
        let err = load(&src).unwrap_err();
        assert!(err.contains("filesystem-safe"), "got: {err}");
    }

    #[test]
    fn expanded_item_cannot_break_the_ready_regex() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"s\"\nforeach = [\"(\"]\npane = { command = \"true\" }\nready = { pattern = \"done ${item}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("after ${item} substitution"), "got: {err}");
    }

    #[test]
    fn capture_var_in_non_submitting_prompt_is_rejected() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"root\"\npane = { command = \"true\" }\nready = { pattern = \"ok\" }\ncapture = { var = \"sum\", lines = 5 }\n\n[[step]]\nid = \"next\"\nneeds = [\"root\"]\npane = { command = \"true\", prompt = \"use ${sum}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("submit = true"), "got: {err}");
        let ok = src.replace(
            "pane = { command = \"true\", prompt = \"use ${sum}\" }",
            "pane = { command = \"true\", prompt = \"use ${sum}\" }\nsubmit = true",
        );
        load(&ok).expect("submitting prompt may consume captures");
    }

    #[test]
    fn duplicate_pane_names_after_expansion_are_refused() {
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"a\"\npane = { command = \"true\", name = \"same\" }\n\n[[step]]\nid = \"b\"\npane = { command = \"true\", name = \"same\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("duplicate pane name 'same'"), "got: {err}");
    }

    #[test]
    fn submit_flags_propagate_to_requires_submit() {
        let plan = load(&minimal(
            "[[step]]\nid = \"s\"\nneeds = [\"root\"]\nsend = { target = \"root\", text = \"go\", submit = true }\n",
        ))
        .expect("valid");
        assert!(plan.requires_submit());
    }
}
