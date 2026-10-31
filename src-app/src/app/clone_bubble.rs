use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Bounds, BoxShadow, ClickEvent, Context, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, PathBuilder, Pixels, SharedString, Stateful, Styled, Window,
    canvas, deferred, div, point, prelude::*, px, size, svg,
};

use crate::PaneFlowApp;
use crate::app::clone_repo::{CloneFailure, CloneProgress, run_clone};
use crate::settings::components::{select_menu_surface, with_alpha};
use crate::ui_primitives::quick_pick::{PANEL_RADIUS, ROW_RADIUS};
use crate::ui_primitives::squircle::{squircle_border, squircle_fill};

const PILL_WIDTH: f32 = 224.;
const PILL_HEIGHT: f32 = 44.;
const CARD_WIDTH: f32 = 320.;
const CARD_HEIGHT: f32 = 152.;
const CARD_RADIUS: f32 = 18.;
const EDGE_INSET: f32 = 18.;
const STACK_GAP: f32 = 10.;
const BADGE_SIZE: f32 = 28.;
const STOP_SIZE: f32 = 9.;
const SOURCE_WIDTH: f32 = 560.;
const SOURCE_HEIGHT: f32 = 81.;
const SOURCE_TOP: f32 = 96.;
const SOURCE_INSET: f32 = 6.;
const INTRO_MS: u64 = 560;
const RESIZE_MS: u64 = 420;
const LEAVE_MS: u64 = 220;
const DONE_DOT: f32 = 20.;
const CHECK_STROKE: f32 = 2.;
const CHECK_POINTS: [(f32, f32); 3] = [(5.8, 10.4), (8.7, 13.3), (14.3, 7.3)];
const SPLASH_MS: f32 = 720.;
const SPLASH_RING_GROWTH: f32 = 14.;
const SPLASH_PARTICLES: usize = 8;
const SPRING_OMEGA: f32 = 9.;
const SWEEP_STEPS: u32 = 42;
const SWEEP_CYCLE_MS: u64 = 1700;
const SWEEP_DOT: f32 = 3.;
const SWEEP_PITCH_X: f32 = 2.6;
const SWEEP_PITCH_Y: f32 = 4.5;
const SWEEP_OPTICAL_LIFT: f32 = 1.;
const SWEEP_BASE: f32 = 0.06;
const SWEEP_MID: f32 = 0.3;
const SWEEP_HIGH: f32 = 0.92;
const SWEEP_CENTER_FLOOR: f32 = 0.42;
const SWEEP_TRIANGLE: [(u32, u32); 10] = [
    (1, 3),
    (2, 2),
    (2, 4),
    (3, 1),
    (3, 3),
    (3, 5),
    (4, 0),
    (4, 2),
    (4, 4),
    (4, 6),
];
const SWEEP_GROUP: &str = "clone-bubble-stop";
const LEAVE_DROP: f32 = 10.;
const DONE_GREEN: u32 = 0x34c759;
const FAILED_RED: u32 = 0xff453a;
const REMOVE_ATTEMPTS: u32 = 20;
const REMOVE_RETRY_MS: u64 = 100;

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) enum CloneStatus {
    Running(Option<CloneProgress>),
    Canceling,
    Done,
    Failed(String),
}

#[derive(Clone, Copy)]
enum BubbleView {
    Intro { from: Bounds<Pixels> },
    Collapsed { animate: bool },
    Expanded,
    Leaving { expanded: bool },
}

pub(crate) struct CloneJob {
    id: u64,
    name: String,
    url: String,
    destination: PathBuf,
    status: CloneStatus,
    status_since: Instant,
    view: BubbleView,
    view_since: Instant,
    cancel: Arc<AtomicBool>,
}

impl CloneJob {
    fn is_running(&self) -> bool {
        matches!(
            self.status,
            CloneStatus::Running(_) | CloneStatus::Canceling
        )
    }

    fn set_status(&mut self, status: CloneStatus) {
        self.status = status;
        self.status_since = Instant::now();
    }

    fn set_view(&mut self, view: BubbleView) {
        self.view = view;
        self.view_since = Instant::now();
    }
}

pub(crate) fn spring(t: f32) -> f32 {
    let curve = |x: f32| 1. - (1. + SPRING_OMEGA * x) * (-SPRING_OMEGA * x).exp();
    (curve(t.clamp(0., 1.)) / curve(1.)).clamp(0., 1.)
}

pub(crate) fn ease_out_back(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.;
    let x = t.clamp(0., 1.) - 1.;
    1. + c3 * x.powi(3) + c1 * x.powi(2)
}

fn window_fraction(start: f32, end: f32, t: f32) -> f32 {
    ((t - start) / (end - start)).clamp(0., 1.)
}

fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from + (to - from) * t
}

fn motion_progress(since: Instant, millis: u64) -> f32 {
    if crate::ui_primitives::reduce_motion() {
        return 1.;
    }
    (since.elapsed().as_secs_f32() / Duration::from_millis(millis).as_secs_f32()).clamp(0., 1.)
}

fn accent() -> Hsla {
    crate::theme::selection_color()
}

pub(crate) fn row_sweep_opacity(row: u32, col: u32, step: u32) -> f32 {
    let progress = (step % SWEEP_STEPS) as f32 / SWEEP_STEPS as f32;
    let ping_pong = 0.5 - 0.5 * (progress * std::f32::consts::TAU).cos();
    let scan_row = 1. + ping_pong * 3.;
    let distance = (row as f32 - scan_row).abs();
    let beam = (1. - distance / 2.2).max(0.);
    let mut opacity = SWEEP_BASE + beam * beam * (SWEEP_HIGH - SWEEP_BASE);
    if distance > 1.3 {
        opacity = opacity.max(SWEEP_MID - ((distance - 1.3) * 0.12).min(0.18));
    }
    if (row, col) == (3, 3) {
        opacity = opacity.max(SWEEP_CENTER_FLOOR);
    }
    opacity
}

fn sweep_step(since: Instant) -> u32 {
    if crate::ui_primitives::reduce_motion() {
        return 0;
    }
    let step_ms = SWEEP_CYCLE_MS as f32 / SWEEP_STEPS as f32;
    (since.elapsed().as_millis() as f32 / step_ms) as u32 % SWEEP_STEPS
}

fn row_sweep(step: u32, color: Hsla) -> gpui::Div {
    let width = 6. * SWEEP_PITCH_X + SWEEP_DOT;
    let height = 3. * SWEEP_PITCH_Y + SWEEP_DOT;
    let mut triangle = div()
        .absolute()
        .left(px((BADGE_SIZE - width) / 2.))
        .top(px((BADGE_SIZE - height) / 2. - SWEEP_OPTICAL_LIFT))
        .w(px(width))
        .h(px(height));
    for (row, col) in SWEEP_TRIANGLE {
        triangle = triangle.child(
            div()
                .absolute()
                .left(px(col as f32 * SWEEP_PITCH_X))
                .top(px((row - 1) as f32 * SWEEP_PITCH_Y))
                .size(px(SWEEP_DOT))
                .rounded_full()
                .bg(color)
                .opacity(row_sweep_opacity(row, col, step)),
        );
    }
    triangle
}

fn ease_out_cubic(t: f32) -> f32 {
    1. - (1. - t.clamp(0., 1.)).powi(3)
}

fn centered_in_badge(side: f32) -> gpui::Div {
    let inset = (BADGE_SIZE - side) / 2.;
    div()
        .absolute()
        .left(px(inset))
        .top(px(inset))
        .size(px(side))
        .rounded_full()
}

fn status_badge(fill: Hsla, glyph: impl IntoElement) -> gpui::Div {
    div().relative().size(px(BADGE_SIZE)).flex_none().child(
        centered_in_badge(DONE_DOT)
            .bg(fill)
            .flex()
            .items_center()
            .justify_center()
            .child(glyph),
    )
}

pub(crate) fn check_stroke_points(draw: f32) -> Vec<(f32, f32)> {
    let segment = |from: (f32, f32), to: (f32, f32)| (to.0 - from.0).hypot(to.1 - from.1);
    let first = segment(CHECK_POINTS[0], CHECK_POINTS[1]);
    let total = first + segment(CHECK_POINTS[1], CHECK_POINTS[2]);
    let drawn = total * draw.clamp(0., 1.);
    let along = |from: (f32, f32), to: (f32, f32), t: f32| {
        (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
    };
    if drawn <= first {
        vec![
            CHECK_POINTS[0],
            along(CHECK_POINTS[0], CHECK_POINTS[1], drawn / first),
        ]
    } else {
        vec![
            CHECK_POINTS[0],
            CHECK_POINTS[1],
            along(
                CHECK_POINTS[1],
                CHECK_POINTS[2],
                (drawn - first) / (total - first),
            ),
        ]
    }
}

fn drawn_check(draw: f32) -> AnyElement {
    let points = check_stroke_points(draw);
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            if draw <= 0. {
                return;
            }
            let at = |(x, y): (f32, f32)| point(bounds.origin.x + px(x), bounds.origin.y + px(y));
            let mut builder = PathBuilder::stroke(px(CHECK_STROKE));
            for (idx, vertex) in points.iter().enumerate() {
                if idx == 0 {
                    builder.move_to(at(*vertex));
                } else {
                    builder.line_to(at(*vertex));
                }
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, gpui::white());
            }
            let cap = CHECK_STROKE / 2.;
            for vertex in &points {
                let center = at(*vertex);
                window.paint_quad(
                    gpui::fill(
                        Bounds::new(
                            point(center.x - px(cap), center.y - px(cap)),
                            size(px(CHECK_STROKE), px(CHECK_STROKE)),
                        ),
                        gpui::white(),
                    )
                    .corner_radii(px(cap)),
                );
            }
        },
    )
    .absolute()
    .left(px((BADGE_SIZE - DONE_DOT) / 2.))
    .top(px((BADGE_SIZE - DONE_DOT) / 2.))
    .size(px(DONE_DOT))
    .into_any_element()
}

fn done_badge(elapsed_ms: f32) -> gpui::Div {
    let green: Hsla = gpui::rgb(DONE_GREEN).into();
    let pop = ease_out_back(window_fraction(0., 380., elapsed_ms));
    let draw = ease_out_cubic(window_fraction(170., 470., elapsed_ms));
    let ring = window_fraction(40., 620., elapsed_ms);
    let burst = window_fraction(90., 640., elapsed_ms);

    let mut badge = div().relative().size(px(BADGE_SIZE)).flex_none();
    if ring < 1. {
        let side = DONE_DOT + SPLASH_RING_GROWTH * ease_out_cubic(ring);
        badge = badge.child(
            centered_in_badge(side)
                .border(px(1.5))
                .border_color(green.opacity(0.55 * (1. - ring))),
        );
    }
    if burst > 0. && burst < 1. {
        let travel = ease_out_cubic(burst);
        let fade = (1. - burst) * (burst * 6.).min(1.);
        let dot = lerp(2.6, 1., burst);
        for idx in 0..SPLASH_PARTICLES {
            let angle = (idx as f32 + 0.5) / SPLASH_PARTICLES as f32 * std::f32::consts::TAU;
            let reach = if idx % 2 == 0 { 17. } else { 14.5 };
            let radius = lerp(DONE_DOT / 2. + 1., reach, travel);
            let center = BADGE_SIZE / 2.;
            badge = badge.child(
                div()
                    .absolute()
                    .left(px(center + radius * angle.cos() - dot / 2.))
                    .top(px(center + radius * angle.sin() - dot / 2.))
                    .size(px(dot))
                    .rounded_full()
                    .bg(green)
                    .opacity(fade),
            );
        }
    }
    badge
        .child(centered_in_badge(DONE_DOT * lerp(0.3, 1., pop)).bg(green))
        .child(drawn_check(draw))
}

fn capsule_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    destructive: bool,
    ui: crate::theme::UiColors,
) -> Stateful<gpui::Div> {
    let (fill, hover, text) = if primary {
        (
            accent(),
            accent().opacity(0.88),
            crate::theme::on_selection_color(),
        )
    } else {
        (
            with_alpha(ui.text, 0.08),
            with_alpha(ui.text, 0.14),
            if destructive {
                gpui::rgb(FAILED_RED).into()
            } else {
                ui.text
            },
        )
    };
    div()
        .id(id)
        .h(px(26.))
        .px(px(12.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(fill)
        .hover(move |style| style.bg(hover))
        .cursor_pointer()
        .text_size(px(12.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(text)
        .role(gpui::accesskit::Role::Button)
        .aria_label(label)
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(label)
}

impl PaneFlowApp {
    pub(crate) fn clone_job_is_running(&self) -> bool {
        self.clone_job.as_ref().is_some_and(CloneJob::is_running)
    }

    pub(crate) fn clone_bubble_clearance(&self) -> f32 {
        match self.clone_job.as_ref().map(|job| job.view) {
            None | Some(BubbleView::Leaving { .. }) => 0.,
            Some(BubbleView::Expanded) => CARD_HEIGHT + STACK_GAP,
            Some(_) => PILL_HEIGHT + STACK_GAP,
        }
    }

    pub(crate) fn begin_clone_job(
        &mut self,
        url: String,
        name: String,
        destination: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = if self.clone_repo.is_some() {
            let viewport = window.viewport_size();
            let left = (f32::from(viewport.width) - SOURCE_WIDTH) / 2.;
            BubbleView::Intro {
                from: Bounds::new(
                    point(px(left), px(SOURCE_TOP)),
                    size(px(SOURCE_WIDTH), px(SOURCE_HEIGHT)),
                ),
            }
        } else {
            BubbleView::Collapsed { animate: false }
        };
        self.clone_repo = None;
        let id = NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        self.clone_job = Some(CloneJob {
            id,
            name,
            url,
            destination,
            status: CloneStatus::Running(None),
            status_since: now,
            view,
            view_since: now,
            cancel: Arc::new(AtomicBool::new(false)),
        });
        self.spawn_clone_task(cx);
        cx.notify();
    }

    fn spawn_clone_task(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.clone_job.as_ref() else {
            return;
        };
        let id = job.id;
        let url = job.url.clone();
        let destination = job.destination.clone();
        let cancel = job.cancel.clone();
        let space_env =
            crate::space_env::subprocess_env(&self.cached_config, self.active_space.as_ref());

        if !crate::ui_primitives::reduce_motion() {
            cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                let step = Duration::from_millis(SWEEP_CYCLE_MS) / SWEEP_STEPS;
                loop {
                    smol::Timer::after(step).await;
                    let ticking = this
                        .update(cx, |app, cx| {
                            let running = app
                                .clone_job
                                .as_ref()
                                .is_some_and(|job| job.id == id && job.is_running());
                            if running {
                                cx.notify();
                            }
                            running
                        })
                        .unwrap_or(false);
                    if !ticking {
                        break;
                    }
                }
            })
            .detach();
        }

        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            let (progress_tx, progress_rx) = smol::channel::unbounded::<CloneProgress>();
            let task_destination = destination.clone();
            let task_cancel = cancel.clone();
            let clone_task = smol::unblock(move || {
                run_clone(
                    &url,
                    &task_destination,
                    &space_env,
                    &task_cancel,
                    move |progress| {
                        let _ = progress_tx.try_send(progress);
                    },
                )
            });
            while let Ok(progress) = progress_rx.recv().await {
                let _ = this.update(cx, |app, cx| {
                    if let Some(job) = app.clone_job.as_mut().filter(|job| job.id == id)
                        && let CloneStatus::Running(current) = &mut job.status
                    {
                        *current = Some(progress);
                        cx.notify();
                    }
                });
            }
            let result = clone_task.await;
            if result == Err(CloneFailure::Canceled) {
                smol::unblock(move || remove_partial_clone(&destination)).await;
            }
            let _ = this.update(cx, |app, cx| app.finish_clone_job(id, result, cx));
        })
        .detach();
    }

    fn finish_clone_job(
        &mut self,
        id: u64,
        result: Result<(), CloneFailure>,
        cx: &mut Context<Self>,
    ) {
        let Some(job) = self.clone_job.as_mut().filter(|job| job.id == id) else {
            return;
        };
        match result {
            Ok(()) => job.set_status(CloneStatus::Done),
            Err(CloneFailure::Failed(message)) => job.set_status(CloneStatus::Failed(message)),
            Err(CloneFailure::Canceled) => {
                job.set_status(CloneStatus::Failed("Clone canceled".to_string()));
                self.dismiss_clone_job(cx);
                return;
            }
        }
        cx.notify();
    }

    fn cancel_clone_job(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.clone_job.as_mut() else {
            return;
        };
        if !matches!(job.status, CloneStatus::Running(_)) {
            return;
        }
        job.cancel.store(true, Ordering::Relaxed);
        job.set_status(CloneStatus::Canceling);
        cx.notify();
    }

    fn set_clone_bubble_expanded(&mut self, expanded: bool, cx: &mut Context<Self>) {
        let Some(job) = self.clone_job.as_mut() else {
            return;
        };
        let is_expanded = matches!(job.view, BubbleView::Expanded);
        if matches!(job.view, BubbleView::Leaving { .. }) || is_expanded == expanded {
            return;
        }
        job.set_view(if expanded {
            BubbleView::Expanded
        } else {
            BubbleView::Collapsed { animate: true }
        });
        cx.notify();
    }

    fn dismiss_clone_job(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.clone_job.as_mut() else {
            return;
        };
        if matches!(job.status, CloneStatus::Running(_)) {
            return;
        }
        let expanded = matches!(job.view, BubbleView::Expanded);
        job.set_view(BubbleView::Leaving { expanded });
        let id = job.id;
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            smol::Timer::after(Duration::from_millis(LEAVE_MS)).await;
            let _ = this.update(cx, |app, cx| {
                if app.clone_job.as_ref().is_some_and(|job| job.id == id) {
                    app.clone_job = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn retry_clone_job(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.clone_job.as_mut() else {
            return;
        };
        if !matches!(job.status, CloneStatus::Failed(_)) {
            return;
        }
        job.cancel = Arc::new(AtomicBool::new(false));
        job.set_status(CloneStatus::Running(None));
        self.spawn_clone_task(cx);
        cx.notify();
    }

    fn open_cloned_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(destination) = self
            .clone_job
            .as_ref()
            .filter(|job| matches!(job.status, CloneStatus::Done))
            .map(|job| job.destination.clone())
        else {
            return;
        };
        self.open_workspace_folders(std::slice::from_ref(&destination), cx);
        if let Some(idx) = self
            .workspaces
            .iter()
            .position(|ws| ws.cwd == destination.display().to_string())
        {
            self.select_workspace(idx, window, cx);
        }
        self.dismiss_clone_job(cx);
    }

    fn reveal_cloned_destination(&mut self, cx: &mut Context<Self>) {
        let Some(destination) = self.clone_job.as_ref().map(|job| job.destination.clone()) else {
            return;
        };
        let task = cx.background_executor().spawn(async move {
            crate::app::workspace_ops::reveal_in_file_manager(&destination).await
        });
        cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            if let Err(message) = task.await {
                let _ = this.update(cx, |app, cx| app.show_toast(message, cx));
            }
        })
        .detach();
    }

    fn render_bubble_badge(
        &self,
        job: &CloneJob,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match &job.status {
            CloneStatus::Running(_) => div()
                .id("clone-bubble-stop")
                .group(SWEEP_GROUP)
                .relative()
                .size(px(BADGE_SIZE))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(|style| style.bg(with_alpha(ui.text, 0.08)))
                .role(gpui::accesskit::Role::Button)
                .aria_label("Cancel clone")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    this.cancel_clone_job(cx);
                }))
                .child(
                    row_sweep(sweep_step(job.status_since), ui.text)
                        .group_hover(SWEEP_GROUP, |style| style.opacity(0.)),
                )
                .child(
                    div()
                        .size(px(STOP_SIZE))
                        .rounded(px(2.))
                        .bg(accent())
                        .opacity(0.)
                        .group_hover(SWEEP_GROUP, |style| style.opacity(1.)),
                )
                .into_any_element(),
            CloneStatus::Canceling => div()
                .relative()
                .size(px(BADGE_SIZE))
                .flex_none()
                .opacity(0.5)
                .child(row_sweep(sweep_step(job.status_since), ui.text))
                .into_any_element(),
            CloneStatus::Done => {
                let elapsed_ms = if crate::ui_primitives::reduce_motion() {
                    SPLASH_MS
                } else {
                    job.status_since.elapsed().as_secs_f32() * 1000.
                };
                if elapsed_ms < SPLASH_MS {
                    window.request_animation_frame();
                }
                done_badge(elapsed_ms).into_any_element()
            }
            CloneStatus::Failed(_) => status_badge(
                gpui::rgb(FAILED_RED).into(),
                div()
                    .text_size(px(12.))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(gpui::white())
                    .child("!"),
            )
            .into_any_element(),
        }
    }

    fn render_bubble_pill(
        &self,
        job: &CloneJob,
        opacity: f32,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let subtitle = match &job.status {
            CloneStatus::Running(None) => "Connecting…".to_string(),
            CloneStatus::Running(Some(progress)) => format!(
                "{}% · {}",
                (progress.fraction * 100.).round() as u32,
                progress.phase
            ),
            CloneStatus::Canceling => "Canceling…".to_string(),
            CloneStatus::Done => "Clone complete".to_string(),
            CloneStatus::Failed(_) => "Clone failed".to_string(),
        };
        div()
            .id("clone-bubble-pill")
            .absolute()
            .right_0()
            .bottom_0()
            .w(px(PILL_WIDTH))
            .h(px(PILL_HEIGHT))
            .pl(px(8.))
            .pr(px(16.))
            .flex()
            .items_center()
            .gap(px(10.))
            .opacity(opacity)
            .cursor_pointer()
            .role(gpui::accesskit::Role::Button)
            .aria_label("Show clone details")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                this.set_clone_bubble_expanded(true, cx);
            }))
            .child(self.render_bubble_badge(job, ui, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(ui.text)
                            .child(SharedString::from(job.name.clone())),
                    )
                    .child(
                        div()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .text_color(ui.muted)
                            .child(subtitle),
                    ),
            )
            .into_any_element()
    }

    fn render_bubble_card(
        &self,
        job: &CloneJob,
        opacity: f32,
        ui: crate::theme::UiColors,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(self.render_bubble_badge(job, ui, window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(14.))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(ui.text)
                            .child(SharedString::from(job.name.clone())),
                    )
                    .child(
                        div()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.5))
                            .text_color(ui.muted)
                            .child(SharedString::from(job.destination.display().to_string())),
                    ),
            )
            .child(
                div()
                    .id("clone-bubble-collapse")
                    .size(px(24.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|style| style.bg(with_alpha(ui.text, 0.08)))
                    .role(gpui::accesskit::Role::Button)
                    .aria_label("Minimize")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.set_clone_bubble_expanded(false, cx);
                    }))
                    .child(
                        svg()
                            .size(px(12.))
                            .path("icons/chevron-down.svg")
                            .text_color(ui.muted),
                    ),
            );

        let detail_text = |text: String, color: Hsla| {
            div()
                .text_size(px(12.))
                .line_height(px(16.))
                .text_color(color)
                .line_clamp(2)
                .child(text)
        };
        let body = match &job.status {
            CloneStatus::Running(progress) => {
                let fraction = progress.as_ref().map_or(0., |progress| progress.fraction);
                let status = match progress {
                    Some(CloneProgress {
                        phase,
                        detail: Some(detail),
                        ..
                    }) => format!("{phase} · {detail}"),
                    Some(CloneProgress { phase, .. }) => (*phase).to_string(),
                    None => "Connecting to the remote…".to_string(),
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(7.))
                    .child(
                        div()
                            .w_full()
                            .h(px(4.))
                            .rounded_full()
                            .bg(with_alpha(ui.text, 0.10))
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(gpui::relative(fraction.clamp(0., 1.)))
                                    .rounded_full()
                                    .bg(accent()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(8.))
                            .text_size(px(11.5))
                            .text_color(ui.muted)
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_x_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .child(status),
                            )
                            .when_some(progress.as_ref(), |row, progress| {
                                row.child(
                                    div()
                                        .flex_none()
                                        .child(format!("{}%", (progress.fraction * 100.).round())),
                                )
                            }),
                    )
                    .into_any_element()
            }
            CloneStatus::Canceling => detail_text(
                "Stopping the clone and removing the partial copy…".to_string(),
                ui.muted,
            )
            .into_any_element(),
            CloneStatus::Done => detail_text(
                "The repository is ready to open as a workspace.".to_string(),
                ui.muted,
            )
            .into_any_element(),
            CloneStatus::Failed(message) => {
                detail_text(message.clone(), with_alpha(ui.text, 0.85)).into_any_element()
            }
        };

        let mut actions = div().flex().justify_end().gap(px(8.));
        match &job.status {
            CloneStatus::Running(_) => {
                actions = actions.child(
                    capsule_button("clone-bubble-cancel", "Cancel", false, true, ui).on_click(
                        cx.listener(|this, _: &ClickEvent, _, cx| this.cancel_clone_job(cx)),
                    ),
                );
            }
            CloneStatus::Canceling => {
                actions = actions.child(
                    capsule_button("clone-bubble-canceling", "Canceling…", false, false, ui)
                        .opacity(0.5)
                        .cursor_default(),
                );
            }
            CloneStatus::Done => {
                actions = actions
                    .child(
                        capsule_button("clone-bubble-close", "Close", false, false, ui).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.dismiss_clone_job(cx)),
                        ),
                    )
                    .child(
                        capsule_button("clone-bubble-reveal", "Show in Folder", false, false, ui)
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.reveal_cloned_destination(cx)
                            })),
                    )
                    .child(
                        capsule_button("clone-bubble-open", "Open", true, false, ui).on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.open_cloned_workspace(window, cx)
                            }),
                        ),
                    );
            }
            CloneStatus::Failed(_) => {
                actions = actions
                    .child(
                        capsule_button("clone-bubble-close", "Close", false, false, ui).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.dismiss_clone_job(cx)),
                        ),
                    )
                    .child(
                        capsule_button("clone-bubble-retry", "Retry", true, false, ui).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.retry_clone_job(cx)),
                        ),
                    );
            }
        }

        div()
            .absolute()
            .right_0()
            .bottom_0()
            .w(px(CARD_WIDTH))
            .h(px(CARD_HEIGHT))
            .p(px(14.))
            .flex()
            .flex_col()
            .justify_between()
            .opacity(opacity)
            .child(header)
            .child(body)
            .child(actions)
            .into_any_element()
    }

    fn render_bubble_ghost(
        &self,
        job: &CloneJob,
        opacity: f32,
        ui: crate::theme::UiColors,
    ) -> AnyElement {
        let field_width = SOURCE_WIDTH - 2. * SOURCE_INSET;
        div()
            .absolute()
            .top(px(SOURCE_INSET))
            .left(px(SOURCE_INSET))
            .w(px(field_width))
            .flex()
            .flex_col()
            .gap(px(6.))
            .opacity(opacity)
            .child(
                div()
                    .relative()
                    .mx(px(2.))
                    .h(px(34.))
                    .px(px(11.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(squircle_fill(px(9.), with_alpha(ui.text, 0.07)))
                    .child(
                        svg()
                            .relative()
                            .size(px(14.))
                            .flex_none()
                            .path("icons/tool_search.svg")
                            .text_color(ui.muted),
                    )
                    .child(
                        div()
                            .relative()
                            .whitespace_nowrap()
                            .text_size(px(14.))
                            .text_color(ui.text)
                            .child(SharedString::from(job.url.clone())),
                    ),
            )
            .child(
                div()
                    .relative()
                    .h(px(29.))
                    .px(px(11.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(squircle_fill(ROW_RADIUS, accent()))
                    .child(
                        svg()
                            .relative()
                            .size(px(14.))
                            .flex_none()
                            .path("icons/brand-github.svg")
                            .text_color(crate::theme::on_selection_color()),
                    )
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .whitespace_nowrap()
                            .text_size(px(14.))
                            .text_color(crate::theme::on_selection_color())
                            .child(SharedString::from(format!("Clone {}", job.url))),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn render_clone_bubble(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(job) = self.clone_job.as_ref() else {
            return div().into_any_element();
        };
        let ui = crate::theme::ui_colors();
        let surface = select_menu_surface(ui);
        let border = with_alpha(ui.border, 0.6);
        let shadow = |strength: f32| {
            vec![
                BoxShadow::new(px(0.), px(10.), gpui::hsla(0., 0., 0., 0.28 * strength))
                    .blur_radius(px(28.)),
            ]
        };

        let shell = |id: &'static str, radius: f32| {
            div()
                .id(id)
                .absolute()
                .overflow_hidden()
                .occlude()
                .rounded(px(radius))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(squircle_fill(px(radius), surface))
                .child(squircle_border(px(radius), px(1.), border))
        };

        let element = match job.view {
            BubbleView::Intro { from } if motion_progress(job.view_since, INTRO_MS) < 1. => {
                window.request_animation_frame();
                let progress = motion_progress(job.view_since, INTRO_MS);
                let eased = spring(progress);
                let viewport = window.viewport_size();
                let to_left = f32::from(viewport.width) - EDGE_INSET - PILL_WIDTH;
                let to_top = f32::from(viewport.height) - EDGE_INSET - PILL_HEIGHT;
                let left = lerp(f32::from(from.origin.x), to_left, eased);
                let top = lerp(f32::from(from.origin.y), to_top, eased);
                let width = lerp(f32::from(from.size.width), PILL_WIDTH, eased);
                let height = lerp(f32::from(from.size.height), PILL_HEIGHT, eased);
                let radius = lerp(f32::from(PANEL_RADIUS), PILL_HEIGHT / 2., eased);
                shell("clone-bubble-intro", radius)
                    .left(px(left))
                    .top(px(top))
                    .w(px(width))
                    .h(px(height))
                    .shadow(shadow(eased))
                    .child(self.render_bubble_ghost(
                        job,
                        1. - window_fraction(0., 0.3, progress),
                        ui,
                    ))
                    .child(self.render_bubble_pill(
                        job,
                        window_fraction(0.45, 0.85, progress),
                        ui,
                        window,
                        cx,
                    ))
                    .into_any_element()
            }
            view => {
                let resize = motion_progress(job.view_since, RESIZE_MS);
                let (expansion, leave) = match view {
                    BubbleView::Intro { .. } | BubbleView::Collapsed { animate: false } => (0., 0.),
                    BubbleView::Collapsed { animate: true } => (1. - spring(resize), 0.),
                    BubbleView::Expanded => (spring(resize), 0.),
                    BubbleView::Leaving { expanded } => (
                        if expanded { 1. } else { 0. },
                        motion_progress(job.view_since, LEAVE_MS),
                    ),
                };
                let animating = match view {
                    BubbleView::Collapsed { animate: true } | BubbleView::Expanded => resize < 1.,
                    BubbleView::Leaving { .. } => leave < 1.,
                    _ => false,
                };
                if animating {
                    window.request_animation_frame();
                }
                let width = lerp(PILL_WIDTH, CARD_WIDTH, expansion);
                let height = lerp(PILL_HEIGHT, CARD_HEIGHT, expansion);
                let radius = lerp(PILL_HEIGHT / 2., CARD_RADIUS, expansion);
                let expanded = matches!(view, BubbleView::Expanded);
                let mut frame = shell("clone-bubble", radius)
                    .right(px(EDGE_INSET))
                    .bottom(px(EDGE_INSET - LEAVE_DROP * leave))
                    .w(px(width))
                    .h(px(height))
                    .opacity(1. - leave)
                    .shadow(shadow(1. - leave));
                if expansion < 0.5 {
                    frame = frame.child(self.render_bubble_pill(
                        job,
                        1. - window_fraction(0., 0.45, expansion),
                        ui,
                        window,
                        cx,
                    ));
                }
                if expansion > 0.3 {
                    frame = frame.child(self.render_bubble_card(
                        job,
                        window_fraction(0.35, 1., expansion),
                        ui,
                        window,
                        cx,
                    ));
                }
                frame
                    .when(expanded, |frame| {
                        frame.on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.set_clone_bubble_expanded(false, cx);
                        }))
                    })
                    .into_any_element()
            }
        };

        deferred(element).with_priority(3).into_any_element()
    }
}

fn remove_partial_clone(destination: &std::path::Path) {
    for _ in 0..REMOVE_ATTEMPTS {
        if !destination.exists() {
            return;
        }
        match std::fs::remove_dir_all(destination) {
            Ok(()) => return,
            Err(error) => {
                log::debug!(
                    "retrying removal of the canceled clone at {}: {error}",
                    destination.display()
                );
                std::thread::sleep(Duration::from_millis(REMOVE_RETRY_MS));
            }
        }
    }
    log::warn!(
        "could not remove the canceled clone at {}",
        destination.display()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spring_starts_at_rest_settles_exactly_and_never_overshoots() {
        assert_eq!(spring(0.), 0.);
        assert!((spring(1.) - 1.).abs() < f32::EPSILON);
        let mut previous = 0.;
        for step in 1..=100 {
            let value = spring(step as f32 / 100.);
            assert!(value >= previous);
            assert!(value <= 1.);
            previous = value;
        }
    }

    #[test]
    fn the_row_sweep_lights_the_apex_first_then_the_base_at_half_cycle() {
        assert!((row_sweep_opacity(1, 3, 0) - SWEEP_HIGH).abs() < 1e-6);
        assert!(row_sweep_opacity(4, 0, 0) < SWEEP_MID);
        let half = SWEEP_STEPS / 2;
        assert!((row_sweep_opacity(4, 0, half) - SWEEP_HIGH).abs() < 1e-6);
        assert!(row_sweep_opacity(1, 3, half) < SWEEP_MID);
        assert_eq!(
            row_sweep_opacity(1, 3, SWEEP_STEPS),
            row_sweep_opacity(1, 3, 0)
        );
        for step in 0..SWEEP_STEPS {
            assert!(row_sweep_opacity(3, 3, step) >= SWEEP_CENTER_FLOOR);
        }
    }

    #[test]
    fn the_sweep_lattice_is_equilateral() {
        let side = 2. * SWEEP_PITCH_X;
        let slant = SWEEP_PITCH_X.hypot(SWEEP_PITCH_Y);
        assert!((side - slant).abs() / side < 0.01);
    }

    #[test]
    fn the_check_draws_from_its_short_stroke_into_the_long_one() {
        assert_eq!(check_stroke_points(0.).len(), 2);
        assert_eq!(check_stroke_points(1.), CHECK_POINTS.to_vec());
        let partial = check_stroke_points(0.2);
        assert_eq!(partial.len(), 2);
        assert!(partial[1].0 < CHECK_POINTS[1].0);
    }

    #[test]
    fn the_done_badge_pop_overshoots_then_lands_on_full_size() {
        assert!(ease_out_back(0.).abs() < 1e-6);
        assert!((ease_out_back(1.) - 1.).abs() < 1e-6);
        assert!((0..100).any(|step| ease_out_back(step as f32 / 100.) > 1.));
    }
}
