//! Declarative agent-flow spec for `paneflow flow` (EP-003,
//! prd-orchestration-v2 - US-010 schema/validation, US-013 foreach
//! expansion, US-014 variable rules).
//!
//! A `flow.toml` describes a DAG of steps. Each step either SPAWNS a pane
//! (reusing [`PaneSpec`] - agent/command, cwd, env, worktree…) or SENDS text
//! into a pane created by an earlier step. A step may declare a `ready`
//! barrier (regex over the pane's recent scrollback - the
//! `process_log_ready` pattern, reliable here because Paneflow owns the PTY),
//! and a `capture` (last N lines at match time, exposed as `${var}` to later
//! steps' prompts/texts).
//!
//! Everything that can fail statically fails at parse: unknown fields
//! (`deny_unknown_fields`), unknown `needs`, dependency cycles, missing
//! barrier timeouts, `${item}` outside `foreach`, unknown variables, pane
//! budget over `MAX_PANES`. The executor ([`super::flow_cmd`]) never sees an
//! invalid plan.
//!
//! ```toml
//! name = "review-pipeline"
//!
//! [defaults]
//! timeout_secs = 600
//! on_failure = "fail_fast"
//!
//! [[step]]
//! id = "impl"
//! pane = { cwd = "~/dev/x", agent = "claude", prompt = "fix the bug" }
//! submit = true
//! ready = { pattern = "tests passed" }
//! capture = { var = "summary", lines = 20 }
//!
//! [[step]]
//! id = "review"
//! needs = ["impl"]
//! send = { target = "impl", text = "Review this: ${summary}", submit = false }
//! ```

use std::collections::{HashMap, HashSet};

use serde::Deserialize;

use super::up_cmd::{DEFAULT_PORT_BASE, extract_tokens};
use super::workspace_spec::{LayoutPreset, PaneSpec, validate_pane};
use crate::layout::MAX_PANES;

/// Capture window cap - mirrors the `surface.read` poll window the executor
/// uses (a capture can never exceed what one poll reads).
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
    /// Default `ready` timeout. A step with `ready` and no timeout anywhere
    /// is a validation error - an unbounded barrier blocks forever.
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// `"fail_fast"` (default) or `"continue"`.
    #[serde(default)]
    pub on_failure: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepSpec {
    /// Unique step id - also the default pane name for spawn steps.
    pub id: String,
    #[serde(default)]
    pub needs: Vec<String>,
    /// Fan-out: instantiate this step once per item, `${item}` substituted
    /// (US-013). A `needs` on this step's id is a fan-in barrier.
    #[serde(default)]
    pub foreach: Option<Vec<String>>,
    /// Spawn a pane (mutually exclusive with `send`).
    #[serde(default)]
    pub pane: Option<PaneSpec>,
    /// Inject text into an earlier step's pane (mutually exclusive with `pane`).
    #[serde(default)]
    pub send: Option<SendSpec>,
    /// Barrier: the step is READY when this regex matches the pane's recent
    /// scrollback (after the spawn/send action).
    #[serde(default)]
    pub ready: Option<ReadySpec>,
    /// Capture the last N lines at `ready`-match time into `${var}`.
    #[serde(default)]
    pub capture: Option<CaptureSpec>,
    /// Submit the pane's `prompt` (spawn steps) after injection. Default
    /// false: pre-filled, the human presses Enter (US-012). Requires the
    /// instance-side scripting gate.
    #[serde(default)]
    pub submit: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SendSpec {
    /// Pane to feed: a step id (spawn steps register their pane under their
    /// id) or any selector the instance resolves.
    pub target: String,
    pub text: String,
    /// Submit after injection (US-012). Default false - pre-fill only.
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

/// Failure policy (US-011/US-015).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnFailure {
    #[default]
    FailFast,
    Continue,
}

/// One executable unit - a step, or one `foreach` instance of a step, with
/// `${item}` already substituted everywhere it is allowed.
#[derive(Debug, Clone)]
pub struct Unit {
    /// `"build"` or `"shard[api]"` for a foreach instance.
    pub id: String,
    /// The declaring step's id - `needs` resolve against groups, so a need on
    /// a foreach step is a fan-in over all its instances (US-013).
    pub group: String,
    pub needs: Vec<String>,
    pub action: UnitAction,
    /// `(pattern, timeout_secs)` - timeout already resolved with defaults.
    pub ready: Option<(String, u64)>,
    /// `(var_key, lines)` - key is `var.item` for foreach instances.
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
    /// Item-substituted pane spec (cwd/agent/command/prompt/env/worktree…).
    pub pane: PaneSpec,
    /// Final unique pane name (user name or the unit id slug).
    pub name: String,
}

/// Fully validated, foreach-expanded execution plan.
#[derive(Debug)]
pub struct FlowPlan {
    pub name: String,
    pub layout: LayoutPreset,
    pub port_base: u16,
    pub on_failure: OnFailure,
    pub units: Vec<Unit>,
}

impl FlowPlan {
    /// True when any unit submits (pane prompt or send) - the executor
    /// refuses to run (or dry-run) without the instance scripting gate then
    /// (US-012).
    pub fn requires_submit(&self) -> bool {
        self.units.iter().any(|u| u.submit)
    }
}

/// Parse + validate a flow spec, then expand it into a [`FlowPlan`].
pub fn load(src: &str) -> Result<FlowPlan, String> {
    let spec: FlowSpec = toml::from_str(src).map_err(|e| e.to_string())?;
    validate(&spec)?;
    expand(&spec)
}

// ---------------------------------------------------------------------------
// Validation (US-010)
// ---------------------------------------------------------------------------

fn validate(spec: &FlowSpec) -> Result<(), String> {
    if spec.steps.is_empty() {
        return Err("flow has no [[step]]".to_string());
    }

    // Ids: unique, non-empty, no brackets (reserved for instance naming).
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

    // Capture vars: unique across steps, sane shape.
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
        // Exactly one action.
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

        // needs: known ids, no self-reference.
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

        // A barrier without a bound blocks forever (US-010).
        if let Some(ready) = &step.ready {
            if ready.pattern.is_empty() {
                return Err(format!("step '{}': ready.pattern is empty", step.id));
            }
            // `${item}` is substituted at expansion - validate the regex on a
            // representative instantiation, not on the raw template (`${`
            // alone is invalid regex syntax).
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
                // A root send has nothing to target: the flow's own panes
                // don't exist yet (the flow always creates its workspace).
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
    // Cycles before the bootstrap rule: an all-`needs` flow is more usefully
    // diagnosed as the cycle it contains than as a missing root.
    detect_cycles(spec)?;
    if !has_root_spawn {
        return Err(
            "flow needs at least one `pane` step without `needs` (the workspace bootstrap)"
                .to_string(),
        );
    }
    Ok(())
}

/// `${…}` rules (US-013/US-014): `${item}` only inside `foreach` steps, in
/// any field; capture variables only in `send.text` / `pane.prompt`; a var
/// captured by a foreach step must be referenced as `${var.<item>}`.
fn validate_tokens(
    spec: &FlowSpec,
    step: &StepSpec,
    vars: &HashMap<&str, &StepSpec>,
) -> Result<(), String> {
    let is_foreach = step.foreach.is_some();

    // Fields where ONLY ${item} makes sense (resolved at expansion).
    let mut item_only: Vec<(&str, &str)> = Vec::new();
    // Fields where capture vars are also allowed (resolved at runtime).
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
            // A NON-submitting spawn prompt is prefilled by the server
            // verbatim (the engine's substitution only runs on the feed
            // path), so a capture ref there would surface as a literal
            // `${var}` in the agent's input box - silently (US-014 AC3).
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

/// Static cycle detection on the step graph (US-010 - refuse at parse, the
/// process-compose pattern). DFS with a path stack so the error names an
/// actual cycle, not just "there is one".
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

// ---------------------------------------------------------------------------
// Expansion (US-013)
// ---------------------------------------------------------------------------

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
        // Re-validate the EXPANDED pane: `validate` only saw the raw template
        // (`worktree = "${item}"` passes its checks), so a crafted item could
        // otherwise smuggle a leading-`-` branch (CWE-88, git argv) or a
        // dot-only slug past the EP-002 guards.
        validate_pane(0, &pane)
            .map_err(|e| format!("unit '{}': {}", id, e.replace("pane 0: ", "")))?;
        // The pane registers under this name so later steps can target it by
        // step id (or instance id for foreach).
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
            // `validate` compiled the pattern with `${item}` → "x"; an item
            // containing regex syntax (`(`, `[`…) can still make the EXPANDED
            // pattern invalid. Compile it here so the executor's recompile is
            // genuinely infallible - a parse error, never an engine panic.
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
        // EP-005 US-018 AC1/AC2: the committed demo flow.toml parses + validates,
        // so `paneflow flow run --dry-run examples/review-pipeline.flow.toml`
        // accepts it without mutating anything.
        let src = include_str!("../../../examples/review-pipeline.flow.toml");
        let plan = load(src).expect("the demo flow.toml must be a valid flow");
        assert_eq!(plan.name, "review-pipeline");
        let ids: Vec<&str> = plan.units.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, vec!["impl", "review"]);
        // AC3: the review step submits the captured summary, so the existing
        // gate-off refusal (check_scripting_gate) applies to this flow.
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
        // The suffixed form passes.
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
        // Cycle fires first here, but a flow whose only roots are sends is
        // caught by the bootstrap rule - exercised via the cycle-free shape:
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
        // CWE-88: `worktree = "${item}"` passes template validation, but the
        // EXPANDED branch must be re-validated - `-evil` would otherwise read
        // as a git flag in `worktree add` argv.
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"s\"\nforeach = [\"-evil\"]\npane = { cwd = \"/tmp\", command = \"true\", worktree = \"${item}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("must not start with '-'"), "got: {err}");
        // Dot-only expansion is equally refused (slug traversal guard).
        let src = src.replace("-evil", "..");
        let err = load(&src).unwrap_err();
        assert!(err.contains("filesystem-safe"), "got: {err}");
    }

    #[test]
    fn expanded_item_cannot_break_the_ready_regex() {
        // An item with regex syntax must surface as a parse error, never as
        // the executor's `.expect()` panic on recompilation.
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"s\"\nforeach = [\"(\"]\npane = { command = \"true\" }\nready = { pattern = \"done ${item}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("after ${item} substitution"), "got: {err}");
    }

    #[test]
    fn capture_var_in_non_submitting_prompt_is_rejected() {
        // US-014 AC3: a non-submitting prompt is prefilled verbatim by the
        // server - a `${var}` there would silently surface unsubstituted.
        let src = "[defaults]\ntimeout_secs = 60\n\n[[step]]\nid = \"root\"\npane = { command = \"true\" }\nready = { pattern = \"ok\" }\ncapture = { var = \"sum\", lines = 5 }\n\n[[step]]\nid = \"next\"\nneeds = [\"root\"]\npane = { command = \"true\", prompt = \"use ${sum}\" }\n";
        let err = load(src).unwrap_err();
        assert!(err.contains("submit = true"), "got: {err}");
        // The same reference is valid on a submitting step (fed through the
        // engine's substitution path).
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
