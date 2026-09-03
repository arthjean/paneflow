use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use gpui::{Font, FontStyle, FontWeight, px};
use paneflow_terminal_ghostty as ghostty;

use crate::bench_harness::{
    Direction, Metric, allocation_counters, measure, process_cpu_time, publish,
    refuse_debug_profile,
};

use super::bench_corpus::{CORPUS_SEED, deterministic_streams};
use super::element::{CellDimensions, LayoutInputs, base_font, layout_from_snapshot};
use super::ghostty_session::{
    CellMirror, RUNTIME_LOOP_ATTENTIVE_REASONS, RUNTIME_LOOP_GATE_WAITS, RUNTIME_LOOP_IDLE_WAITS,
    RUNTIME_LOOP_ITERATIONS, RUNTIME_LOOP_MESSAGES, RUNTIME_LOOP_QUIET_WAITS,
    simulate_gate_trickle,
};
use super::pty_session::TerminalState;
use super::types::{Content, Point};

const SCROLLBACK_LINES: usize = 10_000;

fn terminal(cols: usize, rows: usize) -> ghostty::DisplayTerminal {
    let size = ghostty::WindowSize::new(cols, rows, 8, 16).expect("valid benchmark grid");
    ghostty::DisplayTerminal::new(
        size,
        SCROLLBACK_LINES,
        ghostty::TerminalAppearance::default(),
    )
    .expect("libghostty must initialize")
}

fn fill(terminal: &mut ghostty::DisplayTerminal, rows: usize) {
    let streams = deterministic_streams();
    let mut lines = 0usize;
    while lines < rows * 3 {
        for stream in &streams {
            terminal.feed(stream).expect("corpus must parse");
            terminal.feed(b"\x1b[0m\r\n").expect("newline must parse");
            lines += 1;
        }
    }
}

fn scroll_chunk(index: usize) -> Vec<u8> {
    format!(
        "\x1b[38;5;{}m{index:>7}\x1b[0m  streaming output line with words, numbers 0123456789 and a path src/lib.rs:42\r\n",
        index % 256
    )
    .into_bytes()
}

fn echo_chunk(index: usize, rows: usize) -> Vec<u8> {
    format!("\x1b[{rows};1Hprompt> {index:06}").into_bytes()
}

#[derive(Default)]
struct Publisher {
    mirror: CellMirror,
    front: Option<Content>,
}

impl Publisher {
    fn publish(&mut self, terminal: &mut ghostty::DisplayTerminal) -> &Content {
        let snapshot = terminal.snapshot().expect("snapshot must succeed");
        let content = self.mirror.publish(snapshot);
        if let Some(previous) = self.front.replace(content) {
            self.mirror.recycle(previous);
        }
        self.front.as_ref().expect("a frame was just published")
    }
}

fn bench_font() -> Font {
    Font {
        family: "test-mono".into(),
        features: gpui::FontFeatures::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

fn layout(content: &Content, cols: usize, rows: usize, theme: &crate::theme::TerminalTheme) {
    let state = layout_from_snapshot(LayoutInputs {
        cells: content.cells.clone(),
        cursor: None,
        selection_range: None,
        copy_mode_cursor: None,
        search_highlights: &[],
        display_offset: content.display_offset,
        history_size: content.history_size,
        desired_cols: cols,
        desired_rows: rows,
        first_visible_row: 0,
        last_visible_row: rows as i32,
        dims: CellDimensions {
            cell_width: px(8.0),
            line_height: px(16.0),
        },
        base_font: bench_font(),
        theme,
        exited: None,
        exit_signal: None,
        integrated_glyphs_enabled: true,
        color_emoji_enabled: true,
        minimum_contrast: 0.0,
    });
    std::hint::black_box(state);
}

fn idle_wakeups_per_second(settle: Duration, window: Duration) -> f64 {
    std::thread::sleep(settle);
    let before = RUNTIME_LOOP_ITERATIONS.load(Ordering::Relaxed);
    let started = Instant::now();
    std::thread::sleep(window);
    let after = RUNTIME_LOOP_ITERATIONS.load(Ordering::Relaxed);
    (after - before) as f64 / started.elapsed().as_secs_f64()
}

fn publish_scenarios(metrics: &mut Vec<Metric>) -> Content {
    let mut sample = None;
    for (cols, rows) in [(220usize, 60usize), (120, 40)] {
        let mut term = terminal(cols, rows);
        fill(&mut term, rows);
        let mut publisher = Publisher::default();
        let mut index = 0usize;
        let name = if cols == 220 {
            "publish_scroll_220x60"
        } else {
            "publish_scroll_120x40"
        };
        metrics.push(measure(
            name,
            "one scrolled line fed, then snapshot plus conversion to the neutral Content (every row dirty)",
            20,
            300,
            || {
                index += 1;
                term.feed(&scroll_chunk(index)).expect("chunk must parse");
                std::hint::black_box(publisher.publish(&mut term));
            },
        ));
        if cols == 220 {
            let mut index = 0usize;
            metrics.push(measure(
                "publish_echo_220x60",
                "one keystroke echo on the bottom row, then snapshot plus conversion (one row dirty)",
                20,
                300,
                || {
                    index += 1;
                    term.feed(&echo_chunk(index, rows)).expect("chunk must parse");
                    std::hint::black_box(publisher.publish(&mut term));
                },
            ));
            sample = publisher.front.take();
        }
    }
    sample.expect("the 220x60 scenario publishes at least once")
}

fn layout_scenario(metrics: &mut Vec<Metric>, content: &Content) {
    let theme = crate::theme::paneflow_dark();
    metrics.push(measure(
        "layout_220x60",
        "window-free layout pass over a full 220x60 snapshot: batched runs, background rects, contrast",
        10,
        200,
        || layout(content, 220, 60, &theme),
    ));
}

fn line_text_scenario(metrics: &mut Vec<Metric>) {
    let (cols, rows) = (220usize, 60usize);
    let state = TerminalState::new_display_only(rows, cols);
    let mut text = String::new();
    for index in 0..rows * 2 {
        text.push_str(&format!(
            "{index:>5} a line with a link https://example.com/{index} and a path src/main.rs:{index}\n"
        ));
    }
    state.write_output(text.as_bytes());
    let backend = state.session_backend();
    let point = Point::new((rows / 2) as i32, 3);
    metrics.push(measure(
        "line_text_at_220x60",
        "text of one hovered row extracted from the published snapshot (link detection input)",
        20,
        500,
        || {
            std::hint::black_box(backend.line_text_at(point));
        },
    ));
}

fn render_thread_lookups(metrics: &mut Vec<Metric>) {
    metrics.push(measure(
        "base_font_resolve",
        "the base Font the renderer resolves for every pane on every frame",
        100,
        20_000,
        || {
            std::hint::black_box(base_font());
        },
    ));
    metrics.push(measure(
        "active_theme_read",
        "the theme read the layout pass makes for every pane on every frame",
        100,
        20_000,
        || {
            std::hint::black_box(crate::theme::active_theme());
        },
    ));
}

fn gate_scenario(metrics: &mut Vec<Metric>) {
    let publishes = simulate_gate_trickle(Duration::from_millis(2), 1_000);
    metrics.push(Metric::count(
        "gate_trickle_publishes",
        "frames per 1000 chunks",
        publishes as f64,
        "grid changes arriving every 2 ms with the queue drained: frames the publish gate lets through",
    ));
}

fn idle_scenarios(metrics: &mut Vec<Metric>) {
    {
        let display = TerminalState::new_display_only(24, 80);
        let rate = idle_wakeups_per_second(Duration::from_millis(200), Duration::from_secs(1));
        metrics.push(Metric::count(
            "idle_wakeups_display_per_s",
            "wakeups/s",
            rate,
            "runtime loop iterations per second of a display-only session with nothing to do",
        ));
        drop(display);
        std::thread::sleep(Duration::from_millis(200));
    }
    let cwd = std::env::current_dir().ok();
    match TerminalState::new(cwd, 1, 1, Some((80, 24)), None, None) {
        Ok(shell) => {
            let silent_for = Duration::from_millis(1_500);
            let give_up = Instant::now() + Duration::from_secs(15);
            let mut last_change = Instant::now();
            let mut last_bytes = shell.processed_output_bytes_for_test();
            while last_change.elapsed() < silent_for && Instant::now() < give_up {
                std::thread::sleep(Duration::from_millis(100));
                let bytes = shell.processed_output_bytes_for_test();
                if bytes != last_bytes {
                    last_bytes = bytes;
                    last_change = Instant::now();
                }
            }
            let bytes_before = shell.processed_output_bytes_for_test();
            let quiet_before = RUNTIME_LOOP_QUIET_WAITS.load(Ordering::Relaxed);
            let idle_before = RUNTIME_LOOP_IDLE_WAITS.load(Ordering::Relaxed);
            let gate_before = RUNTIME_LOOP_GATE_WAITS.load(Ordering::Relaxed);
            let messages_before = RUNTIME_LOOP_MESSAGES.load(Ordering::Relaxed);
            RUNTIME_LOOP_ATTENTIVE_REASONS.store(0, Ordering::Relaxed);
            let rate = idle_wakeups_per_second(Duration::ZERO, Duration::from_secs(1));
            let bytes = shell.processed_output_bytes_for_test() - bytes_before;
            let quiet = RUNTIME_LOOP_QUIET_WAITS.load(Ordering::Relaxed) - quiet_before;
            let idle = RUNTIME_LOOP_IDLE_WAITS.load(Ordering::Relaxed) - idle_before;
            let gate = RUNTIME_LOOP_GATE_WAITS.load(Ordering::Relaxed) - gate_before;
            let messages = RUNTIME_LOOP_MESSAGES.load(Ordering::Relaxed) - messages_before;
            let reasons = RUNTIME_LOOP_ATTENTIVE_REASONS.swap(0, Ordering::Relaxed);
            println!(
                "PANEFLOW_BENCH_NOTE idle shell: {bytes} PTY bytes parsed, {quiet} quiet, {idle} idle, {gate} gate waits, {messages} messages, attentive reasons {reasons:#b} during the window"
            );
            metrics.push(Metric::count(
                "idle_wakeups_shell_per_s",
                "wakeups/s",
                rate,
                "runtime loop iterations per second of a live shell session sitting at its prompt",
            ));
            drop(shell);
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(error) => {
            println!("PANEFLOW_BENCH_SKIP idle_wakeups_shell_per_s: {error}");
        }
    }
}

fn pipeline_scenario(metrics: &mut Vec<Metric>) {
    const ROUNDS: usize = 10;
    let (cols, rows) = (220usize, 60usize);
    let mut term = terminal(cols, rows);
    fill(&mut term, rows);
    let streams = deterministic_streams();
    let bytes = streams.iter().map(Vec::len).sum::<usize>() * ROUNDS;
    let mut publisher = Publisher::default();
    let mut publishes = 0usize;
    let (bytes_before, calls_before) = allocation_counters();
    let started = Instant::now();
    for _ in 0..ROUNDS {
        for stream in &streams {
            term.feed(stream).expect("corpus must parse");
            std::hint::black_box(publisher.publish(&mut term));
            publishes += 1;
        }
    }
    let wall = started.elapsed();
    let (bytes_after, calls_after) = allocation_counters();
    let throughput = bytes as f64 / wall.as_secs_f64() / (1024.0 * 1024.0);
    metrics.push(Metric {
        name: "pipeline_corpus_mib_s",
        unit: "MiB/s",
        direction: Direction::HigherIsBetter,
        value: throughput,
        p95: None,
        mean: Some(wall.as_nanos() as f64 / publishes as f64),
        alloc_bytes_per_iter: Some((bytes_after - bytes_before) as f64 / publishes as f64),
        allocs_per_iter: Some((calls_after - calls_before) as f64 / publishes as f64),
        iters: publishes,
        note: "corpus streams fed one per batch with a publish after each: parse plus snapshot plus conversion throughput",
        available: true,
    });
}

#[test]
#[ignore = "terminal performance benchmark: run through scripts/bench-terminal"]
fn terminal_pipeline_benchmark() {
    refuse_debug_profile();

    let mut metrics = Vec::new();
    let timed_started = Instant::now();
    let cpu_before = process_cpu_time();
    let sample = publish_scenarios(&mut metrics);
    layout_scenario(&mut metrics, &sample);
    line_text_scenario(&mut metrics);
    render_thread_lookups(&mut metrics);
    gate_scenario(&mut metrics);
    pipeline_scenario(&mut metrics);
    let cpu_share = (process_cpu_time() - cpu_before).as_secs_f64()
        / timed_started.elapsed().as_secs_f64().max(f64::EPSILON);
    println!("PANEFLOW_BENCH_NOTE cpu share over the timed scenarios: {cpu_share:.2}");
    if cpu_share < 0.9 {
        println!(
            "PANEFLOW_BENCH_WARNING the process only got {:.0}% of a core while timing: another workload was competing, treat the timings as inflated",
            cpu_share * 100.0
        );
    }
    if std::env::var_os("PANEFLOW_BENCH_SKIP_IDLE").is_none() {
        idle_scenarios(&mut metrics);
    }

    publish("paneflow-terminal-bench", CORPUS_SEED, &metrics, cpu_share);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_gate_simulation_never_publishes_more_than_once_per_chunk() {
        assert!(simulate_gate_trickle(Duration::from_millis(2), 100) <= 100);
    }
}
