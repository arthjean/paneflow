use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use gpui::{App, AsyncWindowContext, Context, Keystroke, WeakEntity, Window};
use paneflow_api::methods::browser::TabOwner;
use paneflow_browser_proto::{ProfileName, TabId};
use serde::Serialize;

use crate::PaneFlowApp;
use crate::app::diff_dock::DiffDockTab;

const SELF_TYPE_ENV: &str = "PANEFLOW_LATENCY_SELF_TYPE";
const ARM_ENV: &str = "PANEFLOW_LATENCY_ARM";
const URL_ENV: &str = "PANEFLOW_LATENCY_URL";
const OUT_ENV: &str = "PANEFLOW_LATENCY_OUT";

const SETTLE: Duration = Duration::from_secs(4);
const LOAD_DEADLINE: Duration = Duration::from_secs(30);
const PLAYBACK_WARMUP: Duration = Duration::from_secs(5);
const KEY_INTERVAL: Duration = Duration::from_millis(45);
const WARMUP_KEY_INTERVAL: Duration = Duration::from_millis(300);
const ROUND_PAUSE: Duration = Duration::from_millis(500);
const ROUNDS: usize = 3;
const LETTERS_PER_ROUND: usize = 104;
const WARMUP_KEYS: usize = 5;
const LETTERS: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q", "r", "s",
    "t", "u", "v", "w", "x", "y", "z",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Arm {
    None,
    Control,
    Cpu,
}

impl Arm {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "control" => Some(Self::Control),
            "cpu" => Some(Self::Cpu),
            _ => None,
        }
    }
}

struct Plan {
    arm: Arm,
    url: Option<String>,
    out: PathBuf,
}

#[derive(Serialize)]
struct Report {
    arm: Arm,
    samples_ms: Vec<f64>,
    keystrokes: usize,
    handled: usize,
    focused: bool,
    root_renders: u64,
    recording_seconds: f64,
    tab_title: Option<String>,
    frames_copied: Option<u64>,
    window_width: f32,
    window_height: f32,
    scale_factor: f32,
    error: Option<String>,
}

static RECORDING: AtomicBool = AtomicBool::new(false);
static SAMPLES: Mutex<Vec<f64>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);

fn plan() -> Option<&'static Plan> {
    static PLAN: OnceLock<Option<Plan>> = OnceLock::new();
    PLAN.get_or_init(|| {
        if std::env::var(SELF_TYPE_ENV).as_deref() != Ok("1") {
            return None;
        }
        let arm = std::env::var(ARM_ENV)
            .ok()
            .and_then(|value| Arm::parse(&value))
            .unwrap_or(Arm::None);
        let out = std::env::var_os(OUT_ENV).map(PathBuf::from)?;
        let url = std::env::var(URL_ENV).ok().filter(|url| !url.is_empty());
        Some(Plan { arm, url, out })
    })
    .as_ref()
}

pub(crate) fn active() -> bool {
    plan().is_some()
}

pub(crate) fn record(total_ms: f64) {
    if RECORDING.load(Ordering::Acquire) {
        SAMPLES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(total_ms);
    }
}

pub(crate) fn start(window: &mut Window, cx: &mut Context<PaneFlowApp>) {
    let Some(plan) = plan() else {
        return;
    };
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    cx.spawn_in(window, async move |app, cx| {
        let outcome = run(plan, app.clone(), cx).await;
        let report = cx
            .update(|window, cx| report(plan, outcome, window, cx))
            .unwrap_or_else(|_| Report::failed(plan.arm, "the window closed".into()));
        write(&plan.out, &report);
    })
    .detach();
}

struct Outcome {
    tab: Option<TabId>,
    keystrokes: usize,
    handled: usize,
    focused: bool,
    root_renders: u64,
    recording_seconds: f64,
    error: Option<String>,
}

async fn run(
    plan: &'static Plan,
    app: WeakEntity<PaneFlowApp>,
    cx: &mut AsyncWindowContext,
) -> Outcome {
    let mut outcome = Outcome {
        tab: None,
        keystrokes: 0,
        handled: 0,
        focused: false,
        root_renders: 0,
        recording_seconds: 0.,
        error: None,
    };
    pause(cx, SETTLE).await;
    let created = app.update_in(cx, |app, window, cx| {
        if app.active_workspace().is_some() {
            return false;
        }
        let Ok(cwd) = std::env::current_dir() else {
            return false;
        };
        app.open_workspace_folders(&[cwd], cx);
        let Some(ws_idx) = app.shown_workspace_idx() else {
            return false;
        };
        app.open_tab_with_surface(
            ws_idx,
            "Shell".to_string(),
            paneflow_config::schema::TerminalSurfaceProfile::Normal,
            crate::app::workspace_ops::SurfaceLaunch {
                command: None,
                env: None,
            },
            window,
            cx,
        )
    });
    if created.unwrap_or(false) {
        pause(cx, SETTLE).await;
    }
    let opened = app.update(cx, |app, cx| open_arm(plan, app, cx));
    match opened {
        Ok(Ok(tab)) => outcome.tab = tab,
        Ok(Err(error)) => {
            outcome.error = Some(error);
            return outcome;
        }
        Err(_) => {
            outcome.error = Some("the app closed".into());
            return outcome;
        }
    }
    if plan.arm == Arm::Cpu {
        outcome.tab = wait_for_dock_tab(&app, cx).await;
        if outcome.tab.is_none() {
            outcome.error = Some("the dock browser tab never got a host tab".into());
            return outcome;
        }
    }
    if let Some(tab) = outcome.tab
        && !wait_loaded(tab, cx).await
    {
        outcome.error = Some("the video page never finished loading".into());
        return outcome;
    }
    if outcome.tab.is_some() {
        pause(cx, PLAYBACK_WARMUP).await;
    }
    let _ = app.update_in(cx, |app, window, cx| app.focus_active_pane(window, cx));
    pause(cx, Duration::from_secs(1)).await;
    outcome.focused = cx
        .update(|window, cx| window.focused(cx).is_some())
        .unwrap_or(false);
    for _ in 0..WARMUP_KEYS {
        press(cx, "z");
        pause(cx, WARMUP_KEY_INTERVAL).await;
        press(cx, "backspace");
        pause(cx, WARMUP_KEY_INTERVAL).await;
    }
    let renders_before = crate::work_counters::ROOT_RENDERS.get();
    let recording_started = std::time::Instant::now();
    RECORDING.store(true, Ordering::Release);
    for _ in 0..ROUNDS {
        for letter in LETTERS.iter().cycle().take(LETTERS_PER_ROUND) {
            if press(cx, letter) {
                outcome.handled += 1;
            }
            outcome.keystrokes += 1;
            pause(cx, KEY_INTERVAL).await;
        }
        press(cx, "ctrl-u");
        outcome.keystrokes += 1;
        pause(cx, ROUND_PAUSE).await;
    }
    RECORDING.store(false, Ordering::Release);
    outcome.root_renders = crate::work_counters::ROOT_RENDERS.get() - renders_before;
    outcome.recording_seconds = recording_started.elapsed().as_secs_f64();
    outcome
}

fn open_arm(
    plan: &Plan,
    app: &mut PaneFlowApp,
    cx: &mut Context<PaneFlowApp>,
) -> Result<Option<TabId>, String> {
    let workspace = app.active_workspace().ok_or("no active workspace")?;
    let workspace_id = workspace.id;
    let cwd = workspace.cwd.clone();
    match plan.arm {
        Arm::None => Ok(None),
        Arm::Cpu => {
            let url = plan.url.clone().ok_or("the cpu arm needs a URL")?;
            app.open_diff_browser_url(url, cx);
            Ok(None)
        }
        Arm::Control => {
            let url = plan.url.as_deref().ok_or("the control arm needs a URL")?;
            let profile = crate::workspace::durable_workspace_id(workspace_id)
                .and_then(|durable| ProfileName::new(durable.to_string()).ok())
                .ok_or("the workspace has no browser profile")?;
            let control = crate::browser_control::control()?;
            let tab = control
                .open_surface(profile, TabOwner::Unclaimed, url)
                .map_err(|error| error.to_string())?;
            app.open_diff_dock_panel(cwd, cx);
            Ok(Some(tab))
        }
    }
}

async fn wait_for_dock_tab(
    app: &WeakEntity<PaneFlowApp>,
    cx: &mut AsyncWindowContext,
) -> Option<TabId> {
    let deadline = std::time::Instant::now() + LOAD_DEADLINE;
    while std::time::Instant::now() < deadline {
        let tab = app
            .update(cx, |app, cx| {
                app.diff_dock.diff_tabs.iter().find_map(|tab| match tab {
                    DiffDockTab::Browser(view) => view.read(cx).tab,
                    _ => None,
                })
            })
            .ok()
            .flatten();
        if tab.is_some() {
            return tab;
        }
        pause(cx, Duration::from_millis(100)).await;
    }
    None
}

async fn wait_loaded(tab: TabId, cx: &mut AsyncWindowContext) -> bool {
    let deadline = std::time::Instant::now() + LOAD_DEADLINE;
    while std::time::Instant::now() < deadline {
        let loaded = crate::browser_control::control()
            .ok()
            .and_then(|control| control.view(tab))
            .is_some_and(|view| !view.loading && view.title.is_some());
        if loaded {
            return true;
        }
        pause(cx, Duration::from_millis(100)).await;
    }
    false
}

fn press(cx: &mut AsyncWindowContext, key: &'static str) -> bool {
    let Ok(keystroke) = Keystroke::parse(key) else {
        return false;
    };
    cx.update(|window, cx| window.dispatch_keystroke(keystroke, cx))
        .unwrap_or(false)
}

async fn pause(cx: &mut AsyncWindowContext, duration: Duration) {
    cx.background_executor().timer(duration).await;
}

fn report(plan: &Plan, outcome: Outcome, window: &mut Window, _cx: &mut App) -> Report {
    let samples_ms = std::mem::take(&mut *SAMPLES.lock().unwrap_or_else(PoisonError::into_inner));
    let runtime = outcome
        .tab
        .and_then(|tab| crate::browser_control::runtime().ok().map(|rt| (tab, rt)));
    let tab_title = runtime
        .and_then(|(tab, runtime)| runtime.control.view(tab))
        .and_then(|view| view.title);
    let frames_copied = runtime.map(|(tab, runtime)| runtime.client.frame_stats(tab).copied);
    if let Some((_, runtime)) = runtime {
        drop(runtime.client.shutdown());
    }
    let bounds = window.bounds();
    Report {
        arm: plan.arm,
        samples_ms,
        keystrokes: outcome.keystrokes,
        handled: outcome.handled,
        focused: outcome.focused,
        root_renders: outcome.root_renders,
        recording_seconds: outcome.recording_seconds,
        tab_title,
        frames_copied,
        window_width: bounds.size.width.as_f32(),
        window_height: bounds.size.height.as_f32(),
        scale_factor: window.scale_factor(),
        error: outcome.error,
    }
}

impl Report {
    fn failed(arm: Arm, error: String) -> Self {
        Self {
            arm,
            samples_ms: Vec::new(),
            keystrokes: 0,
            handled: 0,
            focused: false,
            root_renders: 0,
            recording_seconds: 0.,
            tab_title: None,
            frames_copied: None,
            window_width: 0.,
            window_height: 0.,
            scale_factor: 0.,
            error: Some(error),
        }
    }
}

fn write(out: &std::path::Path, report: &Report) {
    let partial = out.with_extension("partial");
    let written = serde_json::to_vec_pretty(report)
        .map_err(std::io::Error::other)
        .and_then(|bytes| std::fs::write(&partial, bytes))
        .and_then(|()| std::fs::rename(&partial, out));
    if let Err(error) = written {
        log::error!("latency harness: cannot write {}: {error}", out.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_arm_name_parses_and_an_unknown_one_does_not() {
        assert_eq!(Arm::parse("none"), Some(Arm::None));
        assert_eq!(Arm::parse("control"), Some(Arm::Control));
        assert_eq!(Arm::parse("cpu"), Some(Arm::Cpu));
        assert_eq!(Arm::parse("gpu"), None);
    }

    #[test]
    fn a_sample_is_kept_only_while_recording() {
        SAMPLES
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
        record(1.0);
        RECORDING.store(true, Ordering::Release);
        record(2.5);
        RECORDING.store(false, Ordering::Release);
        record(3.0);
        assert_eq!(
            *SAMPLES.lock().unwrap_or_else(PoisonError::into_inner),
            vec![2.5]
        );
    }
}
