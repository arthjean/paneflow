//! `paneflow flow run <file>` - the local-first agent-orchestration engine
//! (EP-003, prd-orchestration-v2 - US-011 executor, US-012 gated feeds,
//! US-013 fan-out/fan-in, US-014 captures, US-015 reporting).
//!
//! The engine lives in the CLI process and drives the running instance
//! through public IPC only: `workspace.up` bootstraps the flow's own
//! workspace with the root spawn steps, later spawn steps arrive via the
//! spawn-capable `surface.split`, feeds go through `surface.send_text`
//! (double-gated when submitting), and every `ready` barrier is a
//! `surface.read` poll - the same machinery as `paneflow wait`. Ctrl-C stops
//! the ORCHESTRATION: panes and agents always survive the engine (FR-06).
//!
//! Scheduling is a single-threaded tick loop (no threads, no async): each
//! tick advances the settling/polling units and starts every unit whose
//! dependencies are READY. Wall-clock resolution is `TICK` (500 ms), with
//! settling based on `output_generation` instead of text diffs.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use paneflow_config::schema::PaneFlowConfig;
use paneflow_ipc_client::IpcTransport;
use regex::Regex;
use serde_json::{Value, json};

use super::flow_spec::{self, FlowPlan, OnFailure, Unit, UnitAction};
use super::up_cmd::{self, WorktreePlan};
use super::{CliError, EXIT_OK, EXIT_RUNTIME, EXIT_TIMEOUT};

/// Scheduler tick. 500 ms = the documented `paneflow wait` poll cycle (NFR:
/// barrier polls reuse that cadence, not a faster one) - scheduling latency
/// is irrelevant against steps that run for minutes.
const TICK: Duration = Duration::from_millis(500);
/// Barrier poll window - mirrors `paneflow wait` (bounded under the client's
/// response cap; new output lands at the tail).
const READ_WINDOW_LINES: u64 = 500;
/// Settle detection before a feed: floor + stability mirror the server-side
/// prefill constants (US-010 of cli-agent-orchestration), using the public
/// `surface.read.output_generation` signal so a large scrollback does not need
/// to be string-compared on every tick.
const SETTLE_FLOOR: Duration = Duration::from_millis(1800);
const SETTLE_MAX: Duration = Duration::from_millis(8000);
const SETTLE_WINDOW_LINES: u64 = 50;
/// `surface.send_text` payload cap (server-enforced); substituted texts are
/// truncated to fit, with an explicit marker (US-014).
const MAX_SEND_LEN: usize = 64 * 1024;

/// EP-004 US-014 (agent-control-plane): the settle/bailout decision. Fire when
/// the pane output has been stable across >=2 reads past the floor, OR
/// unconditionally at `max` (the bailout for output that never settles, e.g. a
/// spinner). Pure, so the bailout is unit-tested without an 8 s wall-clock wait.
fn settle_fire(elapsed: Duration, stable: u8, floor: Duration, max: Duration) -> bool {
    elapsed >= max || (stable >= 2 && elapsed >= floor)
}

/// `paneflow flow run <file> [--dry-run] [--json]`.
pub fn run(
    client: &impl IpcTransport,
    file: &str,
    dry_run: bool,
    json_out: bool,
) -> Result<i32, CliError> {
    let src = std::fs::read_to_string(file)
        .map_err(|e| CliError::runtime(format!("cannot read '{file}': {e}")))?;
    let plan = flow_spec::load(&src).map_err(CliError::runtime)?;

    let config = paneflow_config::loader::load_config();
    let runs = prepare_runs(&plan, &config, up_cmd::port_is_free)?;

    if !dry_run {
        check_orchestration_gate(client, dry_run)?;
    }

    // US-012: a submitting flow is refused up-front - run AND dry-run - when
    // the instance gate is off. Never a silent downgrade to non-submitted.
    if plan.requires_submit() {
        check_scripting_gate(client, dry_run)?;
    }

    if dry_run {
        super::print_json(&dry_run_plan(&plan, &runs))?;
        return Ok(EXIT_OK);
    }

    Engine {
        client,
        on_failure: plan.on_failure,
        name: plan.name.clone(),
        layout: plan.layout.as_ipc(),
        runs,
        vars: HashMap::new(),
        started: Instant::now(),
        json_out,
        split_count: 0,
        anchor: None,
    }
    .execute()
}

/// Probe `system.capabilities` for the orchestration gate. An older server
/// without the field falls through to the runtime `-32601` translation; an
/// unreachable instance only degrades to a warning under `--dry-run` (the
/// plan itself needs no instance).
fn check_orchestration_gate(client: &impl IpcTransport, dry_run: bool) -> Result<(), CliError> {
    match client.call("system.capabilities", json!({})) {
        Ok(caps) => {
            let orchestration = caps.get("orchestration").and_then(Value::as_bool);
            let scripting = caps.get("scripting").and_then(Value::as_bool);
            match (orchestration, scripting) {
                (Some(true), _) | (_, Some(true)) | (None, _) => Ok(()),
                (Some(false), _) => Err(CliError::runtime(
                    "this flow creates panes with commands/prompts: relaunch Paneflow with \
                     PANEFLOW_IPC_ORCHESTRATION=1",
                )),
            }
        }
        Err(e) if dry_run => {
            eprintln!(
                "paneflow: instance unreachable ({e}); cannot verify the orchestration gate \
                 this flow requires"
            );
            Ok(())
        }
        Err(e) => Err(CliError::runtime(e)),
    }
}

/// Probe `system.capabilities` for the scripting gate. An older server
/// without the field falls through to the runtime `-32601` translation; an
/// unreachable instance only degrades to a warning under `--dry-run` (the
/// plan itself needs no instance).
fn check_scripting_gate(client: &impl IpcTransport, dry_run: bool) -> Result<(), CliError> {
    match client.call("system.capabilities", json!({})) {
        Ok(caps) => match caps.get("scripting").and_then(Value::as_bool) {
            Some(true) | None => Ok(()),
            Some(false) => Err(CliError::runtime(
                "this flow submits prompts: relaunch Paneflow with PANEFLOW_IPC_SCRIPTING=1 \
                 (or drop the `submit = true` flags)",
            )),
        },
        Err(e) if dry_run => {
            eprintln!(
                "paneflow: instance unreachable ({e}); cannot verify the scripting gate \
                 this flow requires"
            );
            Ok(())
        }
        Err(e) => Err(CliError::runtime(e)),
    }
}

fn dry_run_plan(plan: &FlowPlan, runs: &[UnitRun]) -> Value {
    let units: Vec<Value> = runs
        .iter()
        .map(|r| {
            let (kind, detail) = match &r.unit.action {
                UnitAction::Spawn(s) => (
                    "spawn",
                    json!({
                        "name": s.name,
                        "cwd": r.worktree.as_ref().map_or_else(
                            || s.pane.cwd.clone().unwrap_or_default(),
                            |w| w.path.to_string_lossy().into_owned(),
                        ),
                        "command": r.command,
                        "prompt": s.pane.prompt,
                        "env": s.pane.env,
                        "worktree": r.worktree.as_ref().map(|w| w.managed_json()),
                    }),
                ),
                UnitAction::Send { target, text } => {
                    ("send", json!({ "target": target, "text": text }))
                }
            };
            json!({
                "id": r.unit.id,
                "kind": kind,
                "needs": r.unit.needs,
                "ready": r.unit.ready.as_ref().map(|(p, t)| json!({
                    "pattern": p, "timeout_secs": t
                })),
                "capture": r.unit.capture.as_ref().map(|(v, l)| json!({
                    "var": v, "lines": l
                })),
                "submit": r.unit.submit,
                "detail": detail,
            })
        })
        .collect();
    json!({
        "flow": plan.name,
        "layout": plan.layout.as_ipc(),
        "on_failure": match plan.on_failure {
            OnFailure::FailFast => "fail_fast",
            OnFailure::Continue => "continue",
        },
        "units": units,
    })
}

fn prepare_runs(
    plan: &FlowPlan,
    config: &PaneFlowConfig,
    port_is_free: impl Fn(u16) -> bool,
) -> Result<Vec<UnitRun>, CliError> {
    // Resolve agent launch commands (PATH-checked), worktree plans, and
    // `${port_offset}` env values for every spawn unit - atomic: any failure
    // aborts before side effects.
    let mut commands: Vec<Option<String>> = Vec::with_capacity(plan.units.len());
    let mut worktree_plans: Vec<Option<WorktreePlan>> = Vec::with_capacity(plan.units.len());
    let mut port_refs: Vec<bool> = Vec::with_capacity(plan.units.len());
    for (i, unit) in plan.units.iter().enumerate() {
        let (command, worktree, port_ref) = match &unit.action {
            UnitAction::Spawn(s) => (
                up_cmd::resolve_command(i, &s.pane, config)?,
                up_cmd::plan_worktree(i, &s.pane)?,
                up_cmd::validate_pane_env_tokens(i, &s.pane)?,
            ),
            UnitAction::Send { .. } => (None, None, false),
        };
        commands.push(command);
        worktree_plans.push(worktree);
        port_refs.push(port_ref);
    }
    // Same static dedup as `up`: two units on one worktree path would fail
    // at the second `git worktree add` MID-FLOW otherwise (non-atomic).
    up_cmd::check_worktree_conflicts(&mut worktree_plans)?;
    let port_offsets = up_cmd::allocate_port_offsets(&port_refs, plan.port_base, port_is_free)?;

    Ok(plan
        .units
        .iter()
        .cloned()
        .zip(commands.into_iter().zip(worktree_plans).zip(port_offsets))
        .map(|(unit, ((command, worktree), port_offset))| {
            UnitRun::new(substitute_unit_env(unit, port_offset), command, worktree)
        })
        .collect())
}

fn substitute_unit_env(mut unit: Unit, port_offset: Option<u16>) -> Unit {
    if let UnitAction::Spawn(s) = &mut unit.action {
        s.pane.env = up_cmd::substitute_env(s.pane.env.as_ref(), port_offset);
    }
    unit
}

// ---------------------------------------------------------------------------
// Executor state machine (US-011)
// ---------------------------------------------------------------------------

enum State {
    Pending,
    /// Waiting for the target pane's output to settle before feeding text
    /// (send units, and spawn units submitting their prompt).
    Settling {
        last_generation: Option<u64>,
        stable: u8,
        since: Instant,
    },
    /// `ready` barrier poll.
    Polling {
        deadline: Instant,
        re: Regex,
        baseline: Option<ReadSnapshot>,
    },
    Ready,
    Failed {
        timeout: bool,
    },
    Skipped,
}

impl State {
    fn label(&self) -> &'static str {
        match self {
            State::Pending => "PENDING",
            State::Settling { .. } | State::Polling { .. } => "RUNNING",
            State::Ready => "READY",
            State::Failed { .. } => "FAILED",
            State::Skipped => "SKIPPED",
        }
    }

    fn is_terminal(&self) -> bool {
        matches!(self, State::Ready | State::Failed { .. } | State::Skipped)
    }
}

struct UnitRun {
    unit: Unit,
    command: Option<String>,
    worktree: Option<WorktreePlan>,
    state: State,
    surface_id: Option<u64>,
    started: Option<Instant>,
    finished: Option<Instant>,
    error: Option<String>,
}

impl UnitRun {
    fn new(unit: Unit, command: Option<String>, worktree: Option<WorktreePlan>) -> Self {
        Self {
            unit,
            command,
            worktree,
            state: State::Pending,
            surface_id: None,
            started: None,
            finished: None,
            error: None,
        }
    }
}

#[derive(Clone, Debug)]
struct ReadSnapshot {
    text: String,
    output_generation: Option<u64>,
}

/// What a barrier/settle poll saw.
enum Read {
    Snapshot(ReadSnapshot),
    Gone,
}

struct Engine<'c, T: IpcTransport> {
    client: &'c T,
    on_failure: OnFailure,
    name: String,
    layout: &'static str,
    runs: Vec<UnitRun>,
    vars: HashMap<String, String>,
    started: Instant,
    json_out: bool,
    split_count: usize,
    /// First wave-0 surface - later spawns split off it.
    anchor: Option<u64>,
}

impl<T: IpcTransport> Engine<'_, T> {
    fn execute(mut self) -> Result<i32, CliError> {
        // Ctrl-C stops the orchestration, never the panes (US-015): the
        // handler flips a flag the tick loop reads, so we exit through the
        // normal partial-report path instead of being killed mid-print.
        let interrupted = Arc::new(AtomicBool::new(false));
        {
            let flag = interrupted.clone();
            // A second Ctrl-C falls back to the default disposition via the
            // handler being a no-op flag set - best-effort; failure to
            // install (exotic env) is not fatal to the flow.
            let _ = ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst));
        }

        self.bootstrap_wave0()?;

        let aborted: Option<String> = loop {
            if interrupted.load(Ordering::SeqCst) {
                break Some("interrupted (panes left running)".to_string());
            }
            if let Err(e) = self.progress() {
                break Some(e);
            }
            if self.on_failure == OnFailure::FailFast && self.any_failed() {
                self.skip_pending_after_fail_fast();
                break None;
            }
            self.propagate_and_schedule()?;
            if self.on_failure == OnFailure::FailFast && self.any_failed() {
                // Fail-fast: stop orchestrating NOW. In-flight units stay
                // alive in their panes (never killed); pending ones are
                // skipped for the report.
                self.skip_pending_after_fail_fast();
                break None;
            }
            if self.runs.iter().all(|r| r.state.is_terminal()) {
                break None;
            }
            std::thread::sleep(TICK);
        };

        self.report(aborted)
    }

    /// Spawn every root unit in one `workspace.up` call: the flow gets its
    /// own workspace, panes are created together under the layout preset,
    /// and the response's `surface_ids` map back to the units in order.
    fn bootstrap_wave0(&mut self) -> Result<(), CliError> {
        let roots: Vec<usize> = (0..self.runs.len())
            .filter(|&i| self.runs[i].unit.needs.is_empty())
            .collect();
        let mut panes = Vec::with_capacity(roots.len());
        let mut created_worktrees: Vec<&WorktreePlan> = Vec::new();
        for &i in &roots {
            if let Some(plan) = &self.runs[i].worktree {
                if let Err(err) = up_cmd::execute_worktree_plan(plan) {
                    up_cmd::rollback_created_worktrees(&created_worktrees);
                    return Err(err);
                }
                if plan.creates_worktree() {
                    created_worktrees.push(plan);
                }
            }
            let r = &self.runs[i];
            let UnitAction::Spawn(s) = &r.unit.action else {
                unreachable!("validated: roots are spawn steps");
            };
            panes.push(json!({
                "cwd": r.worktree.as_ref().map_or_else(
                    || s.pane.cwd.clone(),
                    |w| Some(w.path.to_string_lossy().into_owned()),
                ),
                "command": r.command,
                "profile": if s.pane.agent.is_some() { "agent" } else { "normal" },
                // A submitting prompt is fed by the engine after its own
                // settle wait - never double-prefilled by the server.
                "prompt": if r.unit.submit { None } else { s.pane.prompt.clone() },
                "focus": s.pane.focus,
                "env": s.pane.env,
                "name": s.name,
                "managed_worktree": r.worktree.as_ref().map(|w| w.managed_json()),
            }));
        }

        let result = match self.client.call(
            "workspace.up",
            json!({ "name": self.name, "layout": self.layout, "panes": panes }),
        ) {
            Ok(result) => result,
            Err(e) => {
                up_cmd::rollback_created_worktrees(&created_worktrees);
                return Err(CliError::runtime(e));
            }
        };
        let result = match super::reject_legacy_error(result) {
            Ok(result) => result,
            Err(err) => {
                up_cmd::rollback_created_worktrees(&created_worktrees);
                return Err(err);
            }
        };
        drop(created_worktrees);
        let ids: Vec<u64> = result
            .get("surface_ids")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(Value::as_u64).collect())
            .unwrap_or_default();
        if ids.len() != roots.len() {
            return Err(CliError::runtime(format!(
                "workspace.up returned {} surface ids for {} panes; is the \
                 instance older than the flow engine?",
                ids.len(),
                roots.len()
            )));
        }
        self.anchor = ids.first().copied();
        let now = Instant::now();
        for (&i, sid) in roots.iter().zip(ids) {
            self.runs[i].surface_id = Some(sid);
            self.runs[i].started = Some(now);
            self.enter_post_action_state(i);
        }
        Ok(())
    }

    /// After a unit's action fired (pane spawned / text ready to feed): a
    /// submitting spawn settles first, then barriers poll, else READY.
    fn enter_post_action_state(&mut self, i: usize) {
        let r = &self.runs[i];
        let needs_settle = match &r.unit.action {
            UnitAction::Spawn(s) => r.unit.submit && s.pane.prompt.is_some(),
            UnitAction::Send { .. } => true,
        };
        let next = if needs_settle {
            State::Settling {
                last_generation: None,
                stable: 0,
                since: Instant::now(),
            }
        } else {
            self.barrier_or_ready(i, None)
        };
        self.transition(i, next);
    }

    fn barrier_or_ready(&self, i: usize, baseline: Option<ReadSnapshot>) -> State {
        match &self.runs[i].unit.ready {
            Some((pattern, timeout)) => State::Polling {
                deadline: Instant::now() + Duration::from_secs(*timeout),
                // Validated at parse - unreachable in practice.
                re: Regex::new(pattern).expect("ready.pattern validated at parse"),
                baseline,
            },
            None => State::Ready,
        }
    }

    /// Advance settling + polling units. `Err` aborts the whole flow
    /// (instance unreachable - US-015 AC5).
    fn progress(&mut self) -> Result<(), String> {
        for i in 0..self.runs.len() {
            if self.on_failure == OnFailure::FailFast && self.any_failed() {
                break;
            }
            match &self.runs[i].state {
                State::Polling {
                    deadline,
                    re,
                    baseline,
                } => {
                    let (deadline, re, baseline) = (*deadline, re.clone(), baseline.clone());
                    let sid = self.runs[i].surface_id.expect("polling has a surface");
                    match self.read_window(sid, READ_WINDOW_LINES)? {
                        Read::Gone => {
                            self.fail(i, "pane closed before the pattern appeared", false);
                        }
                        Read::Snapshot(snapshot) => {
                            let text = match baseline.as_ref() {
                                Some(base) => {
                                    text_after_baseline(base, &snapshot).unwrap_or_default()
                                }
                                None => snapshot.text,
                            };
                            if re.is_match(&text) {
                                if let Some((var, lines)) = self.runs[i].unit.capture.clone() {
                                    let tail = last_lines(&text, lines as usize);
                                    self.vars.insert(var, tail);
                                }
                                self.transition(i, State::Ready);
                            } else if Instant::now() >= deadline {
                                let (pattern, timeout) =
                                    self.runs[i].unit.ready.clone().expect("polling has ready");
                                self.fail(
                                    i,
                                    &format!("timed out after {timeout}s waiting for /{pattern}/"),
                                    true,
                                );
                            }
                        }
                    }
                }
                State::Settling {
                    last_generation,
                    stable,
                    since,
                } => {
                    let (last_generation, mut stable, since) = (*last_generation, *stable, *since);
                    let sid = self.runs[i].surface_id.expect("settling has a surface");
                    let elapsed = since.elapsed();
                    let mut fire = elapsed >= SETTLE_MAX;
                    if !fire {
                        match self.read_window(sid, SETTLE_WINDOW_LINES)? {
                            Read::Gone => {
                                self.fail(i, "pane closed before the text could be fed", false);
                                continue;
                            }
                            Read::Snapshot(snapshot) => {
                                let output_generation = snapshot.output_generation;
                                if last_generation == output_generation {
                                    stable += 1;
                                } else {
                                    stable = 0;
                                }
                                // EP-004 US-014: settle once stable past the
                                // floor; `settle_fire` also carries the bailout
                                // (exercised by the initial check above, and
                                // unit-tested) so the rule lives in one place.
                                fire = settle_fire(elapsed, stable, SETTLE_FLOOR, SETTLE_MAX);
                                if !fire {
                                    self.runs[i].state = State::Settling {
                                        last_generation: output_generation,
                                        stable,
                                        since,
                                    };
                                }
                            }
                        }
                    }
                    if fire {
                        self.fire_feed(i)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Feed the unit's text (send.text or the submitting prompt) into its
    /// surface, then enter the barrier (or READY).
    fn fire_feed(&mut self, i: usize) -> Result<(), String> {
        let r = &self.runs[i];
        let raw = match &r.unit.action {
            UnitAction::Send { text, .. } => text.clone(),
            UnitAction::Spawn(s) => s.pane.prompt.clone().unwrap_or_default(),
        };
        let text = match substitute_vars(&raw, &self.vars) {
            Ok(t) => t,
            Err(var) => {
                self.fail(
                    i,
                    &format!("undefined variable ${{{var}}} (capturing step failed or skipped?)"),
                    false,
                );
                return Ok(());
            }
        };
        let sid = self.runs[i].surface_id.expect("feeding has a surface");
        let baseline = match self.read_window(sid, READ_WINDOW_LINES)? {
            Read::Snapshot(snapshot) => Some(snapshot),
            Read::Gone => {
                self.fail(i, "pane closed before the text could be fed", false);
                return Ok(());
            }
        };
        let submit = self.runs[i].unit.submit;
        let before_submit = if submit {
            super::send_cmd::status_snapshot(self.client, sid)
        } else {
            None
        };
        match self.client.call(
            "surface.send_text",
            json!({ "surface_id": sid, "text": text, "submit": submit }),
        ) {
            Ok(result) => {
                if let Some(msg) = result.get("error").and_then(Value::as_str) {
                    let msg = msg.to_string();
                    self.fail(i, &msg, false);
                    return Ok(());
                }
                if super::send_cmd::should_wait_for_submit_start(&result) {
                    match super::send_cmd::wait_for_submit_start(
                        self.client,
                        sid,
                        before_submit.as_ref(),
                    ) {
                        super::send_cmd::SubmitStart::Confirmed(_) => {}
                        super::send_cmd::SubmitStart::Unconfirmed(reason) => {
                            self.fail(
                                i,
                                &format!(
                                    "submit was written to agent pane {sid}, but no turn start was confirmed within {}ms ({reason})",
                                    super::send_cmd::SUBMIT_START_TIMEOUT.as_millis()
                                ),
                                false,
                            );
                            return Ok(());
                        }
                    }
                }
                let next = self.barrier_or_ready(i, baseline);
                self.transition(i, next);
                Ok(())
            }
            Err(e) if super::send_cmd::is_send_text_disabled_error(&e) => Err(format!(
                "IPC gate is off on the instance; use PANEFLOW_IPC_ORCHESTRATION=1 for \
                 pane spawning and PANEFLOW_IPC_SCRIPTING=1 for prompt submission ({e})"
            )),
            Err(e) => Err(format!("instance unreachable: {e}")),
        }
    }

    /// Skip the dependents of failed/skipped groups, then start every
    /// pending unit whose dependency groups are all READY.
    fn propagate_and_schedule(&mut self) -> Result<(), CliError> {
        // Group status snapshot: (all_ready, any_failed_or_skipped).
        let mut groups: HashMap<&str, (bool, bool)> = HashMap::new();
        for r in &self.runs {
            let e = groups.entry(r.unit.group.as_str()).or_insert((true, false));
            match r.state {
                State::Ready => {}
                State::Failed { .. } | State::Skipped => {
                    e.0 = false;
                    e.1 = true;
                }
                _ => e.0 = false,
            }
        }
        let snapshot: HashMap<String, (bool, bool)> = groups
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();

        for i in 0..self.runs.len() {
            if !matches!(self.runs[i].state, State::Pending) {
                continue;
            }
            let needs = self.runs[i].unit.needs.clone();
            let mut all_ready = true;
            let mut dead_dep: Option<String> = None;
            for need in &needs {
                let (ready, dead) = snapshot.get(need).copied().unwrap_or((false, false));
                if dead {
                    dead_dep = Some(need.clone());
                }
                if !ready {
                    all_ready = false;
                }
            }
            if let Some(dep) = dead_dep {
                self.runs[i].error = Some(format!("dependency '{dep}' failed or was skipped"));
                self.transition(i, State::Skipped);
                continue;
            }
            if all_ready {
                self.start_unit(i)?;
            }
        }
        Ok(())
    }

    /// Start a non-root unit: spawn its pane via the spawn-capable
    /// `surface.split`, or resolve a send target.
    fn start_unit(&mut self, i: usize) -> Result<(), CliError> {
        self.runs[i].started = Some(Instant::now());
        match &self.runs[i].unit.action {
            UnitAction::Spawn(_) => {
                let mut created_worktree = false;
                if let Some(plan) = &self.runs[i].worktree {
                    up_cmd::execute_worktree_plan(plan)?;
                    created_worktree = plan.creates_worktree();
                }
                let r = &self.runs[i];
                let UnitAction::Spawn(s) = &r.unit.action else {
                    unreachable!();
                };
                let direction = if self.split_count.is_multiple_of(2) {
                    "vertical"
                } else {
                    "horizontal"
                };
                self.split_count += 1;
                let params = json!({
                    "direction": direction,
                    "surface_id": self.anchor,
                    "cwd": r.worktree.as_ref().map_or_else(
                        || s.pane.cwd.clone(),
                        |w| Some(w.path.to_string_lossy().into_owned()),
                    ),
                    "command": r.command,
                    "profile": if s.pane.agent.is_some() { "agent" } else { "normal" },
                    "prompt": if r.unit.submit { None } else { s.pane.prompt.clone() },
                    "env": s.pane.env,
                    "name": s.name,
                    "managed_worktree": r.worktree.as_ref().map(|w| w.managed_json()),
                });
                match self.client.call("surface.split", params) {
                    Ok(result) => {
                        if let Some(msg) = result.get("error").and_then(Value::as_str) {
                            let msg = msg.to_string();
                            self.rollback_unit_worktree(i, created_worktree);
                            self.fail(i, &msg, false);
                            return Ok(());
                        }
                        self.runs[i].surface_id = result.get("surface_id").and_then(Value::as_u64);
                        if self.runs[i].surface_id.is_none() {
                            self.rollback_unit_worktree(i, created_worktree);
                            self.fail(i, "surface.split returned no surface_id", false);
                            return Ok(());
                        }
                        self.enter_post_action_state(i);
                    }
                    Err(e) => {
                        self.rollback_unit_worktree(i, created_worktree);
                        return Err(CliError::runtime(e));
                    }
                }
            }
            UnitAction::Send { target, .. } => {
                // Flow-owned aliases take precedence; fall back to the
                // instance-wide selector for arbitrary panes.
                let target = target.clone();
                let sid = match self.resolve_flow_target(&target) {
                    Some(result) => result,
                    None => super::selector::resolve_target(self.client, &target),
                };
                match sid {
                    Ok(sid) => {
                        self.runs[i].surface_id = Some(sid);
                        self.enter_post_action_state(i);
                    }
                    Err(e) => {
                        let msg = e.message.clone();
                        self.fail(i, &format!("cannot resolve send target: {msg}"), false);
                    }
                }
            }
        }
        Ok(())
    }

    fn resolve_flow_target(&self, target: &str) -> Option<Result<u64, CliError>> {
        let matches: Vec<(&str, u64)> = self
            .runs
            .iter()
            .filter_map(|r| {
                let UnitAction::Spawn(s) = &r.unit.action else {
                    return None;
                };
                if s.name == target || r.unit.id == target || r.unit.group == target {
                    return r.surface_id.map(|sid| (r.unit.id.as_str(), sid));
                }
                None
            })
            .collect();
        match matches.as_slice() {
            [] => None,
            [(_, sid)] => Some(Ok(*sid)),
            many => {
                let listed = many
                    .iter()
                    .map(|(id, sid)| format!("{id}({sid})"))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(Err(CliError::target(format!(
                    "ambiguous flow target '{target}'; matches: {listed}"
                ))))
            }
        }
    }

    fn rollback_unit_worktree(&self, i: usize, created_worktree: bool) {
        if !created_worktree {
            return;
        }
        if let Some(plan) = self.runs[i].worktree.as_ref() {
            up_cmd::rollback_created_worktrees(&[plan]);
        }
    }

    fn read_window(&self, sid: u64, lines: u64) -> Result<Read, String> {
        match self.client.call(
            "surface.read",
            // EP-003 US-011: barriers parse raw output and settling reads
            // output_generation from the same response. The untrusted fence
            // would corrupt regex matching, so opt out here.
            json!({ "surface_id": sid, "lines": lines, "fenced": false }),
        ) {
            Ok(result) => {
                if let Some(message) = legacy_error_message(&result) {
                    if is_surface_gone_error(&message) {
                        return Ok(Read::Gone);
                    }
                    return Err(format!("surface.read failed: {message}"));
                }
                Ok(Read::Snapshot(ReadSnapshot {
                    text: result
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    output_generation: result.get("output_generation").and_then(Value::as_u64),
                }))
            }
            Err(e) if is_surface_gone_error(&e) => Ok(Read::Gone),
            Err(e) => Err(format!("instance unreachable: {e}")),
        }
    }

    fn fail(&mut self, i: usize, error: &str, timeout: bool) {
        self.runs[i].error = Some(error.to_string());
        self.transition(i, State::Failed { timeout });
    }

    fn any_failed(&self) -> bool {
        self.runs
            .iter()
            .any(|r| matches!(r.state, State::Failed { .. }))
    }

    fn skip_pending_after_fail_fast(&mut self) {
        for r in &mut self.runs {
            if matches!(r.state, State::Pending) {
                r.state = State::Skipped;
                r.error = Some("fail_fast: an earlier step failed".to_string());
            }
        }
    }

    fn transition(&mut self, i: usize, next: State) {
        if next.is_terminal() {
            self.runs[i].finished = Some(Instant::now());
        }
        let label = next.label();
        let changed = label != self.runs[i].state.label();
        self.runs[i].state = next;
        if changed {
            let line = format!(
                "[{:>7.1}s] {} → {}{}",
                self.started.elapsed().as_secs_f32(),
                self.runs[i].unit.id,
                label,
                self.runs[i]
                    .error
                    .as_deref()
                    .map(|e| format!(" ({e})"))
                    .unwrap_or_default()
            );
            // Live transitions go to stderr under --json so stdout stays a
            // single machine-readable document (US-015).
            if self.json_out {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
    }

    /// Final report + exit code (US-015): 0 all READY, 4 when a barrier
    /// timed out, 1 for any other failure / abort. Always printed - also on
    /// abort paths (partial report).
    fn report(self, aborted: Option<String>) -> Result<i32, CliError> {
        let any_timeout = self
            .runs
            .iter()
            .any(|r| matches!(r.state, State::Failed { timeout: true }));
        let all_ready = self.runs.iter().all(|r| matches!(r.state, State::Ready));
        let status = if aborted.is_some() {
            "aborted"
        } else if all_ready {
            "ready"
        } else {
            "failed"
        };

        let steps: Vec<Value> = self
            .runs
            .iter()
            .map(|r| {
                json!({
                    "id": r.unit.id,
                    "status": r.state.label(),
                    "duration_ms": match (r.started, r.finished) {
                        (Some(s), Some(f)) => Some(f.duration_since(s).as_millis() as u64),
                        _ => None,
                    },
                    "surface_id": r.surface_id,
                    "error": r.error,
                })
            })
            .collect();
        let report = json!({
            "flow": self.name,
            "status": status,
            "aborted": aborted,
            "steps": steps,
        });
        if self.json_out {
            super::print_json(&report)?;
        } else {
            println!("---");
            for r in &self.runs {
                println!(
                    "{:<24} {:<8} {}",
                    r.unit.id,
                    r.state.label(),
                    r.error.as_deref().unwrap_or("")
                );
            }
        }
        if let Some(reason) = &aborted {
            eprintln!("paneflow: flow aborted: {reason} (partial report above)");
            return Ok(EXIT_RUNTIME);
        }
        Ok(if all_ready {
            EXIT_OK
        } else if any_timeout {
            EXIT_TIMEOUT
        } else {
            EXIT_RUNTIME
        })
    }
}

fn legacy_error_message(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    error
        .as_str()
        .map(str::to_string)
        .or_else(|| {
            error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| Some(error.to_string()))
}

fn is_surface_gone_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("not found") || lower.contains("-32602")
}

fn text_after_baseline(baseline: &ReadSnapshot, current: &ReadSnapshot) -> Option<String> {
    if matches!(
        (current.output_generation, baseline.output_generation),
        (Some(current), Some(previous)) if current <= previous
    ) {
        return None;
    }
    Some(new_text_since_baseline(&baseline.text, &current.text))
}

fn new_text_since_baseline(baseline: &str, current: &str) -> String {
    if current == baseline {
        return String::new();
    }
    if let Some(rest) = current.strip_prefix(baseline) {
        return rest.to_string();
    }
    let old_lines: HashSet<&str> = baseline.lines().collect();
    current
        .lines()
        .filter(|line| !old_lines.contains(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Substitute `${var}` tokens from the capture store. `${item}` was resolved
/// at expansion, so every remaining token must be a captured variable -
/// unknown means the capturing step failed or was skipped (US-014). The
/// result is bounded to the `send_text` cap with an explicit marker.
fn substitute_vars(text: &str, vars: &HashMap<String, String>) -> Result<String, String> {
    let tokens = up_cmd::extract_tokens(text)?;
    let mut out = text.to_string();
    for token in tokens {
        let Some(value) = vars.get(token) else {
            return Err(token.to_string());
        };
        out = out.replace(&format!("${{{token}}}"), value);
    }
    if out.len() > MAX_SEND_LEN {
        let mut truncated = out;
        let mut cut = MAX_SEND_LEN - "…[truncated]".len();
        while !truncated.is_char_boundary(cut) {
            cut -= 1;
        }
        truncated.truncate(cut);
        truncated.push_str("…[truncated]");
        return Ok(truncated);
    }
    Ok(out)
}

/// Last `n` lines of a read window (capture payload).
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn settle_fire_bails_at_max_and_fires_when_stable_past_floor() {
        // EP-004 US-014 AC2: output that never settles (stable stays 0) must
        // NOT fire before the max, and MUST bail out at the max regardless.
        assert!(
            !settle_fire(Duration::from_millis(7999), 0, SETTLE_FLOOR, SETTLE_MAX),
            "below max with no stability keeps waiting"
        );
        assert!(
            settle_fire(SETTLE_MAX, 0, SETTLE_FLOOR, SETTLE_MAX),
            "bailout fires at the max even if the output never settled"
        );
        // The settled path: stable across >=2 reads AND past the floor.
        assert!(
            !settle_fire(Duration::from_millis(1799), 9, SETTLE_FLOOR, SETTLE_MAX),
            "stable but below the floor keeps waiting"
        );
        assert!(
            !settle_fire(Duration::from_millis(2000), 1, SETTLE_FLOOR, SETTLE_MAX),
            "past the floor but only one stable read keeps waiting"
        );
        assert!(
            settle_fire(SETTLE_FLOOR, 2, SETTLE_FLOOR, SETTLE_MAX),
            "two stable reads past the floor settles"
        );
    }

    #[test]
    fn substitute_vars_replaces_known_and_reports_unknown() {
        let mut vars = HashMap::new();
        vars.insert("summary".to_string(), "all green".to_string());
        vars.insert("out.api".to_string(), "api ok".to_string());
        assert_eq!(
            substitute_vars("r: ${summary} / ${out.api}", &vars).expect("ok"),
            "r: all green / api ok"
        );
        assert_eq!(
            substitute_vars("x ${missing}", &vars).unwrap_err(),
            "missing"
        );
    }

    #[test]
    fn substitute_vars_truncates_to_the_send_cap_with_marker() {
        let mut vars = HashMap::new();
        vars.insert("big".to_string(), "x".repeat(MAX_SEND_LEN));
        let out = substitute_vars("head ${big}", &vars).expect("ok");
        assert!(out.len() <= MAX_SEND_LEN);
        assert!(out.ends_with("…[truncated]"), "marker present");
        assert!(out.starts_with("head x"), "head preserved");
    }

    #[test]
    fn last_lines_takes_the_tail() {
        assert_eq!(last_lines("a\nb\nc", 2), "b\nc");
        assert_eq!(last_lines("a\nb", 10), "a\nb");
        assert_eq!(last_lines("", 3), "");
    }

    #[test]
    fn ready_baseline_uses_only_new_output() {
        let baseline = ReadSnapshot {
            text: "old READY\n".to_string(),
            output_generation: Some(10),
        };
        let unchanged = ReadSnapshot {
            text: "old READY\nnew READY\n".to_string(),
            output_generation: Some(10),
        };
        assert!(text_after_baseline(&baseline, &unchanged).is_none());

        let advanced = ReadSnapshot {
            text: "old READY\nnew READY\n".to_string(),
            output_generation: Some(11),
        };
        assert_eq!(
            text_after_baseline(&baseline, &advanced).expect("delta"),
            "new READY\n"
        );
    }

    #[test]
    fn flow_target_aliases_include_unit_id_group_and_custom_name() {
        let spawn = Unit {
            id: "impl".to_string(),
            group: "feature".to_string(),
            needs: Vec::new(),
            action: UnitAction::Spawn(Box::new(flow_spec::SpawnUnit {
                pane: crate::cli::workspace_spec::PaneSpec {
                    cwd: None,
                    agent: None,
                    command: Some("true".to_string()),
                    prompt: None,
                    focus: None,
                    env: None,
                    name: Some("custom".to_string()),
                    worktree: None,
                    copy_env: None,
                    setup: None,
                    setup_timeout_secs: None,
                    worktree_teardown: None,
                },
                name: "custom".to_string(),
            })),
            ready: None,
            capture: None,
            submit: false,
        };
        let mut run = UnitRun::new(spawn, Some("true".to_string()), None);
        run.surface_id = Some(42);
        let fake = FakeInstance::new(true);
        let engine = Engine {
            client: &fake,
            on_failure: OnFailure::FailFast,
            name: "flow".to_string(),
            layout: "even_h",
            runs: vec![run],
            vars: HashMap::new(),
            started: Instant::now(),
            json_out: true,
            split_count: 0,
            anchor: Some(42),
        };
        assert_eq!(
            engine
                .resolve_flow_target("impl")
                .expect("found")
                .expect("ok"),
            42
        );
        assert_eq!(
            engine
                .resolve_flow_target("custom")
                .expect("found")
                .expect("ok"),
            42
        );
        assert_eq!(
            engine
                .resolve_flow_target("feature")
                .expect("found")
                .expect("ok"),
            42
        );
    }

    // --- end-to-end engine run over a scripted fake transport -------------

    /// Routed fake instance: workspace.up returns surface ids; surface.read
    /// returns per-surface scripted text that changes over time (a counter);
    /// send_text logs. Enough to drive a 2-step flow through spawn → barrier
    /// → feed → barrier → READY without a live instance.
    struct FakeInstance {
        calls: RefCell<Vec<(String, Value)>>,
        reads: RefCell<HashMap<u64, Vec<String>>>,
        scripting: bool,
    }
    impl FakeInstance {
        fn new(scripting: bool) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                reads: RefCell::new(HashMap::new()),
                scripting,
            }
        }
        fn push_reads(&self, sid: u64, texts: &[&str]) {
            self.reads
                .borrow_mut()
                .insert(sid, texts.iter().map(|s| s.to_string()).collect());
        }
    }
    impl IpcTransport for FakeInstance {
        fn call(&self, method: &str, params: Value) -> Result<Value, String> {
            self.calls
                .borrow_mut()
                .push((method.to_string(), params.clone()));
            match method {
                "system.capabilities" => Ok(json!({
                    "scripting": self.scripting,
                    "orchestration": self.scripting,
                })),
                "workspace.up" => {
                    let n = params["panes"].as_array().map_or(0, Vec::len);
                    let ids: Vec<u64> = (1..=n as u64).collect();
                    Ok(json!({ "index": 1, "panes": n, "surface_ids": ids }))
                }
                "surface.split" => Ok(json!({ "split": true, "surface_id": 99 })),
                "surface.read" => {
                    let sid = params["surface_id"].as_u64().unwrap_or(0);
                    let mut reads = self.reads.borrow_mut();
                    let texts = reads.entry(sid).or_default();
                    let text = if texts.len() > 1 {
                        texts.remove(0)
                    } else {
                        texts.first().cloned().unwrap_or_default()
                    };
                    Ok(json!({ "text": text }))
                }
                "surface.send_text" => Ok(json!({ "sent": true })),
                other => Err(format!("unexpected method {other}")),
            }
        }
    }

    /// A submitting flow against a gate-off instance is refused before any
    /// mutation (US-012) - including under --dry-run.
    #[test]
    fn submitting_flow_refused_when_gate_off() {
        let dir = tempfile::tempdir().expect("tmp");
        let file = dir.path().join("flow.toml");
        std::fs::write(
            &file,
            "[defaults]\ntimeout_secs = 1\n\n[[step]]\nid = \"root\"\npane = { command = \"true\" }\n\n[[step]]\nid = \"go\"\nneeds = [\"root\"]\nsend = { target = \"root\", text = \"x\", submit = true }\n",
        )
        .unwrap();
        let fake = FakeInstance::new(false);
        let err = run(&fake, file.to_str().unwrap(), true, false).expect_err("refused");
        assert!(
            err.message.contains("PANEFLOW_IPC_SCRIPTING"),
            "got: {}",
            err.message
        );
        // Only the capabilities probe ran - no mutation.
        let calls = fake.calls.borrow();
        assert!(calls.iter().all(|(m, _)| m == "system.capabilities"));
    }

    #[test]
    fn flow_run_refused_when_orchestration_gate_off() {
        let dir = tempfile::tempdir().expect("tmp");
        let file = dir.path().join("flow.toml");
        std::fs::write(
            &file,
            "[defaults]\ntimeout_secs = 1\n\n[[step]]\nid = \"root\"\npane = { command = \"true\" }\n",
        )
        .unwrap();
        let fake = FakeInstance::new(false);
        let err = run(&fake, file.to_str().unwrap(), false, false).expect_err("refused");
        assert!(
            err.message.contains("PANEFLOW_IPC_ORCHESTRATION"),
            "got: {}",
            err.message
        );
        let calls = fake.calls.borrow();
        assert!(calls.iter().all(|(m, _)| m == "system.capabilities"));
    }

    /// Dry-run prints the plan and never mutates the instance (US-010).
    #[test]
    fn dry_run_makes_no_mutating_call() {
        let dir = tempfile::tempdir().expect("tmp");
        let file = dir.path().join("flow.toml");
        std::fs::write(
            &file,
            "[defaults]\ntimeout_secs = 1\n\n[[step]]\nid = \"root\"\npane = { command = \"true\" }\nready = { pattern = \"ok\" }\n",
        )
        .unwrap();
        let fake = FakeInstance::new(true);
        let code = run(&fake, file.to_str().unwrap(), true, false).expect("ok");
        assert_eq!(code, EXIT_OK);
        assert!(fake.calls.borrow().is_empty(), "no IPC at all (no submit)");
    }

    #[test]
    fn prepare_runs_substitutes_port_offsets_before_ipc() {
        let plan = flow_spec::load(
            "port_base = 4100\n\n[defaults]\ntimeout_secs = 1\n\n[[step]]\nid = \"api\"\npane = { command = \"true\", env = { PORT = \"${port_offset}\", PLAIN = \"x\" } }\n\n[[step]]\nid = \"ui\"\npane = { command = \"true\", env = { PORT = \"${port_offset}\" } }\n",
        )
        .expect("valid");
        let cfg = PaneFlowConfig::default();
        let runs = prepare_runs(&plan, &cfg, |port| port != 4100).expect("runs");

        let UnitAction::Spawn(api) = &runs[0].unit.action else {
            panic!("spawn");
        };
        let UnitAction::Spawn(ui) = &runs[1].unit.action else {
            panic!("spawn");
        };
        let api_env = api.pane.env.as_ref().expect("api env");
        let ui_env = ui.pane.env.as_ref().expect("ui env");
        assert_eq!(api_env["PORT"], "4110", "busy base stride is skipped");
        assert_eq!(api_env["PLAIN"], "x");
        assert_eq!(ui_env["PORT"], "4120");
    }

    /// Full happy path: root spawns (workspace.up), barrier matches,
    /// capture feeds the second step, feed is sent, flow exits 0 (US-011/
    /// US-012/US-014). The fake's read script: first read misses, second
    /// matches - exercising at least one real poll wait.
    #[test]
    fn two_step_flow_runs_to_ready() {
        let dir = tempfile::tempdir().expect("tmp");
        let file = dir.path().join("flow.toml");
        std::fs::write(
            &file,
            "[defaults]\ntimeout_secs = 30\n\n[[step]]\nid = \"impl\"\npane = { command = \"true\" }\nready = { pattern = \"tests passed\" }\ncapture = { var = \"sum\", lines = 1 }\n\n[[step]]\nid = \"review\"\nneeds = [\"impl\"]\nsend = { target = \"impl\", text = \"check: ${sum}\" }\n",
        )
        .unwrap();
        let fake = FakeInstance::new(true);
        fake.push_reads(1, &["building...", "tests passed", "tests passed"]);
        let code = run(&fake, file.to_str().unwrap(), false, true).expect("ok");
        assert_eq!(code, EXIT_OK);
        let calls = fake.calls.borrow();
        let sent: Vec<&Value> = calls
            .iter()
            .filter(|(m, _)| m == "surface.send_text")
            .map(|(_, p)| p)
            .collect();
        assert_eq!(sent.len(), 1, "one feed");
        assert_eq!(
            sent[0]["text"], "check: tests passed",
            "capture substituted"
        );
        assert_eq!(sent[0]["submit"], false, "default never submits");
        assert_eq!(sent[0]["surface_id"], 1, "fed into impl's pane");
    }

    /// A barrier that never matches times out, fails the step, and fail_fast
    /// skips the dependents - exit 4 (US-011/US-015).
    #[test]
    fn timeout_fails_step_and_exits_4() {
        let dir = tempfile::tempdir().expect("tmp");
        let file = dir.path().join("flow.toml");
        std::fs::write(
            &file,
            "[defaults]\ntimeout_secs = 1\n\n[[step]]\nid = \"impl\"\npane = { command = \"true\" }\nready = { pattern = \"never-matches\" }\n\n[[step]]\nid = \"after\"\nneeds = [\"impl\"]\nsend = { target = \"impl\", text = \"x\" }\n",
        )
        .unwrap();
        let fake = FakeInstance::new(true);
        fake.push_reads(1, &["nope"]);
        let code = run(&fake, file.to_str().unwrap(), false, true).expect("report");
        assert_eq!(code, EXIT_TIMEOUT);
        // The dependent never fed anything.
        assert!(
            fake.calls
                .borrow()
                .iter()
                .all(|(m, _)| m != "surface.send_text"),
            "skipped dependent must not send"
        );
    }
}
