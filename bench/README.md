# Performance benchmarks

`bench/` holds the reproducible measurements behind Paneflow's performance
claims. Every number published about the terminal pipeline or the code editor
comes from one of the suites below, run with the scripts described here, and
the raw result of each run is archived next to the baseline it is compared
against.

There are four suites, four baselines, and four result prefixes:

| Suite | Test | Script | Baseline | Result files |
|---|---|---|---|---|
| `paneflow-terminal-bench` | `terminal::perf_bench::terminal_pipeline_benchmark` | `scripts/bench-terminal.sh` / `.ps1` | `bench/baseline.json` | `bench/results/<stamp>-<sha>.json` |
| `paneflow-editor-bench` | `app::diff_dock::code::perf_bench::editor_pipeline_benchmark` | `scripts/bench-editor.sh` / `.ps1` | `bench/editor-baseline.json` | `bench/results/editor-<stamp>-<sha>.json` |
| `paneflow-startup-bench` | `startup_bench::startup_first_frame_benchmark` | `scripts/bench-startup.sh` / `.ps1` | `bench/startup-baseline.json` | `bench/results/startup-<stamp>-<sha>.json` |
| `paneflow-persistent-bench` | `tests/persistent_baseline.rs::persistent_session_baseline` (crate `paneflow-host`) | `scripts/bench-persistent.sh` / `.ps1` | `bench/persistent-baseline.json` | `bench/results/persistent-<stamp>-<sha>.json` |

The first three suites share one harness, `src-app/src/bench_harness.rs`: the metric
type, the timing helpers, the JSON document, the comparison table, and the
single `#[global_allocator]` the test binary installs. That allocator counts
allocated bytes, allocation calls, and live bytes (allocations minus
deallocations), which is how a retained-memory metric can be reported at all.
It exists only in `cfg(test)` builds.

## Persistent session suite

The suite is the ignored integration test `persistent_session_baseline` in
`crates/paneflow-host/tests/persistent_baseline.rs`. It starts the real
detached host from the release build, drives it through the real IPC endpoint
and samples an empty topology, then opens 1, 10 and 50 sessions running
`paneflow-session-fixture idle`, the deterministic fixture executable of the
host crate at 80x24 (no shell syntax, no randomness). Every session attaches
through `session.attach` and follows `session.output` by default. The
`--no-followers` (`-NoFollowers`) option isolates the host-only or host+worker
topology for comparison; its record explicitly reports zero attachments.
With `--with-desktop`
(`-WithDesktop` on Windows), the native app restores the exact host session
identities from an isolated saved layout instead of using headless followers.
It verifies `surface.read` contains the fixture announcement for every pane
before sampling. One pane is visible; the other tabs are in the background.
The runner closes only its own desktop process after each sample, then verifies
the original host sessions remain live. On Windows it adjusts only that owned
window until the host reports a stable 80x24 visible grid. Every record includes
observed per-session dimensions. Each restored pane is then focused once
through the existing IPC so background panes adopt that calibrated size; the
first pane is made visible again before settling and sampling. Other native desktops currently record the
geometry deviation and require manual window calibration for exact W01 sizing.
For each scenario it settles for
four seconds, then samples per-thread CPU time of the host process over a ten
second window and attributes it by thread name: `host.session`,
`host.pty_reader`, `host.pty_writer`, `host.viewport_scan`,
`host.cancellation_scan`, `host.ipc_connection`, `host.ipc_accept`,
`host.launch_owner`, `host.main`, `host.other`. It also records the creation
time per session, real attachment latency and checkpoint size, the
`session.list` round trip and resident memory. A bounded paused-follower probe
records independent inspect and reattach latency while one receiver is paused.

The document records what a review needs to trust a number: commit, an FNV-1a
fingerprint of the uncommitted diff, OS, architecture, CPU model, logical CPU
count, RAM, rustc version, build profile, the terminal engine identity the
host reported, the PTY implementation, the fixture invocation and the scenario
list. The schema version is 3. Fingerprints include tracked and untracked
source contents; controller executable identity is recorded when supplied.
A fingerprint that changes between the start and the end of a run fails the
run: the result is not candidate-qualified.
`--with-worker` (`-WithWorker`) starts the existing `paneflow serve run` entry
point and attributes its CPU separately. `--with-desktop` includes the worker
and attributes native mirror, follower, and runtime threads separately from
the host viewport and cancellation scans. Cursor blinking and telemetry are
disabled in the isolated fixture configuration. Native desktop runs require a
working graphical session and display a benchmark window.

Anything the runner cannot measure is written as `pending` with the reason,
never as a zero. The default run has headless followers; worker and native
desktop numbers require their respective options. Per-thread attribution on
macOS is not implemented. Linux `comm` truncates names to 15 bytes; ambiguous
prefixes are reported as merged, never attributed to a guessed worker. Thread
CPU deltas aggregate duplicate names before subtracting the baseline. These
short samples establish a baseline, not the 300-second, three-repetition
performance acceptance gate. Native timer wake reasons, allocation ownership,
and complete W01-W08 qualification remain unmeasured unless a separate native
trace supplies that evidence.

The 2026-09-21 persistent result is a host-only historical sample. It does not
exercise attachment, a paused follower, a delayed spawn, or the desktop and
worker process set. Its diff fingerprint excludes untracked sources, and its
creation cost at 10/50 sessions divides the incremental batch by the cumulative
session count. New runs include untracked file contents in the fingerprint and
divide by the number actually created. The historical JSON is retained as
recorded and cannot certify the complete persistent path or native waiter
behavior on Linux and macOS.

The selected `persistent-baseline.json` is the Windows native desktop run from
2026-09-22. Its companion runs isolate the [host](results/persistent-20260922T080613Z-d442894acbc8-host-only.json),
[host and worker](results/persistent-20260922T080711Z-d442894acbc8-host-worker.json),
and [host, worker, and desktop](results/persistent-20260922T080810Z-d442894acbc8-native-desktop.json).
All three measured the same source fingerprint `ee47b728e251ffc7`, with
0/1/10/50 sessions at an observed 80x24 and a passing paused-follower probe.
The native run restored all 50 existing sessions through the desktop. These
records establish a measured reference; they do not claim a performance
improvement or satisfy the longer performance acceptance window.
The [runtime identity receipt](results/persistent-20260922T081128Z-d442894acbc8-conpty-identity.json)
records Windows build 26200.9457, the engine revision, and the loaded ConPTY
1.24.260710001 module path, file version, and SHA-256 matching the pinned payload.
It was collected after all samples from a fresh isolated host using the same
release executable.

```bash
scripts/bench-persistent.sh                 # writes bench/results/persistent-<stamp>-<sha>.json and compares
scripts/bench-persistent.sh --set-baseline  # also copies the result to bench/persistent-baseline.json
scripts/bench-persistent.sh --with-worker  # existing worker plus headless attachments
scripts/bench-persistent.sh --with-desktop # native desktop restoration and per-process CPU
scripts/bench-persistent.sh --no-followers # W01 host-only topology
scripts/bench-persistent.sh --with-worker --no-followers # W01 host+worker topology
scripts/bench-persistent.sh --quick        # smoke protocol: 5 s streams, 200 echo samples, 2 worker cycles
scripts/bench-persistent.sh --worker-replacement <paneflow-exe> # W04 build replacement with that worker binary
scripts/bench-persistent.sh --prior <result.json>  # rerun that keeps the earlier failures in the record
scripts/bench-persistent.sh --seed-failure # proves a failed decision exits nonzero and retains the artifact
```

### Workloads W02 to W08 and threshold decisions

Schema 3 runs W02, W03, W04 (worker replacement), and W05 after the W01
samples, in the same isolated home, and records a decision list under
`thresholds`. W02 fills one session with `history 10000` (deterministic
ANSI and Unicode lines), attaches ten times sequentially, ten times
concurrently, then cycles 100 attach/detach rounds, and checks that the
checkpoint bytes are identical every time and that the host reports zero
staged checkpoint bytes afterwards (`NFR-09.attach_p95`,
`NFR-09.concurrent_total`, `NFR-06.checkpoint_release`,
`W02.content_equivalence`). W03 measures one 32 MiB flood through a follower,
an echo probe at idle and while ten `stream` fixtures emit 1 MiB/s each, and
per-stream fairness (`NFR-08.idle_p95`, `NFR-08.idle_p99`, `NFR-08.loaded_p95`,
`NFR-08.throughput_ratio` against the matched baseline when one exists). W04
kills and restarts the existing worker, or replaces its binary with
`--worker-replacement`, while fixtures run, and checks that every child
identity and generation survives (`NFR-11.worker_cycles`). W05 runs ten
batches of fifty `flood 65536` sessions, waits for their exits, and checks
that the host releases every runtime within five seconds, then compares
resident memory, threads, and handles or file descriptors with the warmed
baseline after quiescence (`NFR-04.runtime_release`, `NFR-04.reclaim_max_ms`,
`NFR-05.memory_after_churn`, `NFR-05.memory_slope`, `NFR-05.threads`,
`NFR-05.handles`). A batch that misses the reclaim budget records the
runtimes still held and their inspected state. Fixture processes the run
owned are listed with their kernel start time; the final `host.shutdown`
must be acknowledged and the host process must exit within ten seconds
(`NFR-12.host_shutdown`), and any fixture survivor after that fails
`NFR-12.fixture_orphans`. A run that fails a threshold panics after writing
its artifact, and a host that refused shutdown because of unresolved
ownership stays alive on its private home by design: the artifact names it.

W04 fault cases, W06 injected failures, and the W07 desktop entry points that
live in the unit and integration suites are recorded as `automated_tests`
with their test names; the W07 interactive cells are recorded as `pending`
with the runbook cell that supplies them, and the baseline record leaves W08
`pending` because the endurance run is a separate ignored test in the same
target, `persistent_session_endurance` (`--endurance <minutes>` /
`-Endurance <minutes>`, output `bench/results/persistent-endurance-<stamp>-<sha>.json`):
it retains ten fixtures for the whole run, bursts ten flood sessions every few
minutes, spreads the worker and desktop cycles after an untouched idle interval,
checks that the first input on the idle control connection echoes exactly once,
samples host memory, threads, handles, and ownership counters at a fixed
interval, and rewrites its document at every sample. The full
protocol (60 s streams, 1,000 echo samples, 60 s quiescence, ten worker
cycles) is the default; `--quick` (`-Quick`) shortens every window for CI
and rehearsals and labels the record `smoke`. Threads and handles are
sampled on Windows and Linux; on macOS they are `pending`.

Any failed decision makes the test exit nonzero after the artifact is
written. `--prior` (`-Prior`) carries the failed decisions of an earlier
artifact into `prior_failures` so a rerun cannot erase the first failure, and
`--seed-failure` (`-SeedFailure`) injects one failing decision to prove that
path; the non-ignored test
`a_seeded_failure_fails_the_run_and_retains_its_artifact` proves it on every
CI target. The workload inputs, thresholds, and the platform evidence they
feed are frozen in
[docs/release/persistent-qualification.md](../docs/release/persistent-qualification.md).

## Terminal suite

The benchmark is the ignored test `terminal_pipeline_benchmark` in
`src-app/src/terminal/perf_bench.rs`. It exercises the terminal pipeline
without a GPU or a window: the libghostty parser and snapshot, the conversion
into the renderer's neutral `Content`, the window-free layout pass, the
per-frame lookups the render thread performs, and the runtime loop's idle
behavior. Timings include wall-clock p50, p95 and p99; allocations are counted by the
shared allocator.

| Metric | Unit | What it captures |
|---|---|---|
| `idle_wakeups_display_per_s` | wakeups/s | Runtime loop iterations of a display-only session with nothing to do. Direct CPU cost of an idle pane. |
| `idle_wakeups_shell_per_s` | wakeups/s | Same for a live shell sitting at its prompt. Skipped when the host cannot spawn a shell. |
| `publish_scroll_220x60` | ns | One scrolled line of styled output, then snapshot plus conversion to `Content`, on a 220x60 grid where every row is dirty. |
| `publish_echo_220x60` | ns | One keystroke echo on the bottom row, then snapshot plus conversion. Only one row changed. |
| `publish_scroll_120x40` | ns | The scroll case on a 120x40 grid, the size of a typical split pane. |
| `layout_220x60` | ns | The layout pass over a full 220x60 snapshot: run batching, background rectangles, contrast checks. |
| `layout_220x60_acc60` | ns | The same layout pass with the automatic contrast correction on at Lc 60. Compared against `layout_220x60` at the end of the run: the suite prints a `PANEFLOW_BENCH_WARNING` when the correction costs more than 10%. |
| `layout_echo_{uncached,cached}_220x60` | ns | Paired native echo, publication and layout workloads with and without retained row layouts. |
| `layout_scroll_{uncached,cached}_220x60` | ns | The same pair with full-viewport scrolling; checks the cost when every row changes. |
| `service_spaces_220x60`, `service_spaces_8192`, `service_text_220x60` | ns | Service-output parsing and extraction for blank redraws, a long blank line and ordinary styled output. |
| `line_text_at_220x60` | ns | Text of one hovered row extracted from the published snapshot, the input of link detection. |
| `base_font_resolve` | ns | The base font resolution the renderer performs for every pane on every frame. |
| `active_theme_read` | ns | The theme read the layout pass performs for every pane on every frame. |
| `gate_trickle_publishes` | frames per 1000 chunks | Frames the publish gate lets through when grid changes arrive every 2 ms with the queue drained. Bounds redraw frequency on trickle output such as ConPTY. |
| `pipeline_corpus_mib_s` | MiB/s | Parse plus snapshot plus conversion throughput over the deterministic corpus, one publish per stream. |

The corpus is `deterministic_streams()` in
`src-app/src/terminal/bench_corpus.rs`, seeded with `CORPUS_SEED`, so every
run parses byte-identical input. These headless metrics exclude platform text
shaping, GPU submission and presentation. The cached/uncached layout pairs
compare paths in the same executable; they do not replace a before/after
release-app frame trace. `pipeline_corpus_mib_s` still excludes service
detection, which has separate scenarios above.

`PANEFLOW_BENCH_SKIP_IDLE=1` skips the two idle probes, which spend several
seconds waiting for a shell to settle; the timed scenarios run first either
way, so the probes never disturb them.

## Editor suite

The benchmark is the ignored test `editor_pipeline_benchmark` in
`src-app/src/app/diff_dock/code/perf_bench.rs`. It exercises the right-hand
code editor without a GPU or a window: the rope document, the tree-sitter
parse and highlight query, the run resolution the diff view shares, the UTF-16
conversions the input handler makes, the external-reload path, and the
platform shaper.

| Metric | Unit | What it captures |
|---|---|---|
| `open_300kb_highlighted` | ns | A 300 KB Rust file opened: rope build, longest-line measure, and an explicit query of the first 60-row viewport. Since US-030 the initial parse is deferred, so this is the work between the read and the first visible text. |
| `open_to_first_tree_300kb` | ns | The same file from the read to `apply_parsed`: the deferred initial parse plus the viewport query it makes possible. Everything but the apply runs off the render thread, so this is latency to color, not render-thread cost. |
| `open_2mb_to_text` | ns | A 2 MB Rust file from the read to visible text, with the initial parse still in flight. US-030 budgets this under 50 ms. |
| `open_3_7mb` | ns | A 3.7 MB Rust file opened past the 3 MB highlight cap, covering the rope build and the source-string longest-line scan. |
| `open_markdown_injected` | ns | A 64 KB Markdown file opened, the only corpus that runs a second grammar pass through the inline injection. |
| `keystroke_to_runs` | ns | Render-thread work of one inserted character at a pseudo-random row of 300 KB of Rust. Its `p95` column is the `keystroke_to_runs_p95` target of the PRD. Deferred parses run outside the timer; the `apply_parsed` requery they trigger is inside it. |
| `viewport_query_60_rows` | ns | The highlight query for one 60-row viewport, the work a viewport-bounded requery would do per frame. |
| `fill_60_stale_rows` | ns | One budgeted `fill_stale_rows` over a never-queried 60-row viewport, walking disjoint viewports of the 300 KB corpus. Since US-019 a contiguous stale span is one ranged query, so this metric must stay close to `viewport_query_60_rows`. |
| `keystroke_3_7mb_plain` | ns | Render-thread work of one inserted character in the 3.7 MB file, past the highlight cap: the rope splice alone, with no interpolation and no per-row table. |
| `plain_highlighter_retained_bytes` | bytes | Live allocated bytes the highlighter of that 3.7 MB file holds. A file past the cap keeps no per-row runs and no per-row states. |
| `unclosed_comment_close_ui` | ns | Render-thread work of closing an unterminated block comment at the top of the file, which re-tokenizes the whole document. |
| `resolve_runs_3750` | ns | `resolve_runs` over 3 750 captures taken from a 10 000-character minified JSON line, the shape the diff view shares. |
| `byte_to_utf16_eof` | ns | One byte offset converted to a UTF-16 offset at the end of a 3.7 MB document, two to four times per keystroke through `EntityInputHandler`. |
| `to_disk_string_3_7mb` | ns | The whole 3.7 MB document rendered to the string a save writes. |
| `theme_switch` | ns | A theme change on 300 KB of Rust, rebuilding capture color tables without reparsing or requerying the trees. |
| `shape_cold_60_rows` | ns | Sixty never-seen ASCII rows of 100 characters shaped with the editor monospace font, the cold-cache cost of one scrolled viewport. |
| `shape_warm_60_rows` | ns | The same sixty rows shaped again, the warm-cache cost the line-layout cache serves on a second frame. |
| `prepaint_60_rows_warm` | ns | The same sixty rows re-shaped the way `CodeElement::prepaint` does it since US-025: keyed by content hash through `shape_line_by_hash`, with one reused `Vec<TextRun>`. Its `allocs_per_iter` is the per-viewport allocation count US-025 caps at one per row. |
| `reload_200_retained_bytes` | bytes | Live allocated bytes a colored 2 MB tab still holds after 200 external reloads: document, per-row runs, and undo history. Tree-sitter trees allocate through the C allocator, so they are outside this number and the tree memory probe below counts them instead. |
| `pagedown_stale_rows` | rows | Median rows of a 60-row viewport still uncolored after one 2 ms fill, over 20 pseudo-random jumps on a freshly opened 300 KB Rust file. |
| `pagedown_stale_rows_max` | rows | The worst of those 20 jumps: rows the first frame after a PageDown leaves in plain text. |
| `pagedown_frames_to_fresh` | frames | Successive 2 ms fills the worst of those 20 jumps needs before no visible row is stale. |
| `textdiff_300kb_50_blocks` | ns | `paneflow_textdiff::compare_lines_inner` with word highlighting over 300 KB of synthetic Rust against a copy with 50 rewritten 10-line blocks. Its `p95` is the 25 ms target of EP-012. |
| `textdiff_5k_lines_one_word_each` | ns | The same call over 5 000 edited lines of eight words, each with one word changed and separated from the next by an identical line, so the line pass yields 5 000 one-line blocks and the word pass runs once per edited line. Target: 150 ms. |
| `textdiff_5k_lines_all_different` | ns | The same call over 5 000 dense lines of sixteen words split into four all-different blocks by identical separator lines. Each block exceeds the 20 000-chunk fine comparison threshold: the first three trip the bad-lines guard and the fourth is skipped without a word pass. Target: 50 ms. |

The corpus is `src-app/src/app/diff_dock/code/bench_corpus.rs`, seeded with
`EDITOR_CORPUS_SEED`. It is generated, never read from the repository's own
sources, so a run is byte-identical everywhere: synthetic Rust sized to 295 KB,
2 MB (both under the 2 MB highlight cap, the larger by 48 bytes), and 3.7 MB
(about 110 000 lines, past it); a
single-line minified JSON document of exactly 10 000 characters; and Markdown
carrying both inline and fenced code so the injection pass has work to do. The
`textdiff_*` metrics add three seeded pairs from the same file: 300 KB of Rust
with 50 rewritten 10-line blocks, 5 000 eight-word lines with one word changed
per line and an identical separator line between them, and 5 000 sixteen-word
lines in four all-different blocks.

### The PageDown stale-row probe

`pagedown_stale_rows`, `pagedown_stale_rows_max` and `pagedown_frames_to_fresh`
turn the 2 ms highlight budget into a number. Each of the 20 jumps moves a
60-row viewport to a pseudo-random row of the 300 KB Rust corpus, calls
`CodeHighlighter::fill_stale_rows` with `HIGHLIGHT_FRAME_BUDGET`, records the
rows the call left stale, then keeps calling until none is. Rows left stale are
rows the user reads in plain text; frames-to-fresh is how many frames the
editor needs before the viewport is fully colored. Since US-018 a starved fill
schedules those frames itself through `Window::request_animation_frame`, and
since US-019 a 60-row viewport is a single ranged query, so the probe reports 0
stale rows in 1 frame even at a zero budget. A file past the highlight cap
reports the same and spends no budget at all, and so does a file whose initial
parse has not landed yet: since US-030 a treeless highlighter fills nothing and
asks for no follow-up frame.

### The shaping probe and the US-013 threshold

`prepaint_60_rows_warm` measures the same sixty rows through the path the
editor actually takes since US-025. `shape_warm_60_rows` passes a
`SharedString` per row, so `layout_line` allocates one more copy of the text on
every hit; the prepaint probe passes a content hash instead and materializes
nothing when the layout is already cached. The two are not a like-for-like
timing pair, because the prepaint probe also walks the rope and hashes every
line before it reaches the cache, so compare them on allocations rather than
on nanoseconds. That count is the number US-025 caps at sixty for sixty rows.

`shape_cold_60_rows` and `shape_warm_60_rows` decide whether the ASCII grid of
US-013 is worth building. **The threshold is 1.0 ms cold per 60 rows on the
reference machine.** Below it, `shape_line` is not what makes scrolling
expensive and US-013 stays unbuilt; at or above it, the grid path is worth
its complexity. Like every other timing metric, both are stored in nanoseconds
and rendered by the table in milliseconds once they pass 1 ms, so the
threshold reads as `1.00 ms` in the table and `1000000.0` in the document.

The probe deliberately does not use GPUI's `TestAppContext`. That context
installs `NoopTextSystem`, a stub that returns synthetic metrics for every
font, so a measurement taken through it would describe the stub and not the
platform shaper the editor actually pays for. The probe instead resolves the
real platform text system through `gpui_platform::current_platform(true)` and
shapes through a `WindowTextSystem` built on it. When that platform cannot be
created, or when it shapes a zero-width line because no real font is
available, both metrics are reported as unavailable through
`PANEFLOW_BENCH_SKIP` lines, remain in the JSON with `available: false` and a
null value, and the suite carries on. `PANEFLOW_BENCH_SKIP_SHAPE=1` skips the
probe outright with the same unavailable result.

`reload_200_retained_bytes` allocates and retains several hundred megabytes by
design, which is the defect it measures. It runs last, after the timed
scenarios, so it never inflates them.

### The tree memory probe and the highlight caps

`MAX_HIGHLIGHT_BYTES` and `MAX_MARKDOWN_HIGHLIGHT_BYTES`
(`src-app/src/diff/highlighter.rs`) are set by measurement, not by guess. The
rule US-031 fixes them by: **a file at its cap must hold less than 128 MiB of
tree-sitter tree.** Both caps are read through `highlight_cap(ext)`, so the
editor's `CodeHighlighter` and the diff view's `highlight_lines` sit behind the
same rule.

The measurement is a second ignored test, `tree_memory_probe`, in the same
file as the editor suite. It routes tree-sitter's own C allocator to a counting
allocator through `tree_sitter::set_allocator`, so the bytes it reports are the
tree and nothing else. That counter is deliberately kept out of the timed
suite: installing it would change every parse timing, and freeing a block
allocated before it was installed would corrupt the heap. Run it alone.

```bash
cargo test --release --locked -p paneflow-app --bin paneflow \
  app::diff_dock::code::perf_bench::tree_memory_probe \
  -- --ignored --exact --nocapture --test-threads=1
```

Measured on Windows 11 x86_64, release profile, tree-sitter 0.26.13, on the
generated corpus:

| Grammar | Source | Tree | Bytes of tree per source byte | Cap the 128 MiB rule allows |
|---|---|---|---|---|
| Rust | 295 KB | 8.70 MB | 29.5 | 4.55 MB |
| Rust | 2 MB | 58.7 MB | 29.4 | 4.57 MB |
| Rust | 3.7 MB | 108.4 MB | 29.3 | 4.58 MB |
| Minified JSON | 295 KB | 16.1 MB | 54.6 | 2.46 MB |
| Minified JSON | 2 MB | 101.6 MB | 50.8 | 2.64 MB |
| Markdown (two passes) | 64 KB | 8.25 MB | 129.1 | 1.04 MB |
| Markdown (two passes) | 2 MB | 254.9 MB | 127.5 | 1.05 MB |

The ratio is a property of the grammar, not of the file size, so a cap set on
Rust alone does not hold. Minified JSON costs 1.7 times what Rust costs per
source byte, because a one-line document of short key-value pairs is nearly all
nodes and no text. Markdown costs about 4.3 times, because the inline injection
parses the whole document a second time; that second pass is why it keeps a cap
of its own.

`MAX_HIGHLIGHT_BYTES` is therefore 2 MB, set by the densest single-pass grammar
in the corpus rather than by Rust: JSON allows 2.64 MB, rounded down to the
megabyte. Rust alone would have allowed 4 MB, and a 3 MB cap put JSON at
143.7 MiB, past the budget. At 2 MB the measured grammars hold 56.0 MiB (Rust)
and 96.9 MiB (JSON), which also leaves headroom for the thirteen grammars the
corpus does not generate: anything up to 67 bytes per source byte stays inside
the budget. `MAX_MARKDOWN_HIGHLIGHT_BYTES` is 1 MB (121.8 MiB at the cap, 95%
of the budget: the tightest of the three margins). The probe asserts all three,
so raising a cap without re-measuring fails the test.

The probe also checks the other half of US-031: a deferred parse holds a second
tree while it is in flight, and `apply_parsed` drops the superseded one. That
second tree costs far less than the first, because an incremental parse shares
the subtrees the edit did not touch: on the 295 KB Rust corpus a one-line edit
adds 203 KB to the 8.70 MB the first tree holds, and the counter returns to
8.70 MB once `apply_parsed` has run.

## Scroll frame scenario

The editor suite runs without a window, so it cannot say what one wheel notch
costs when terminals share the frame. That number comes from a separate
ignored test, `layout::render::tests::editor_scroll_frame_by_pane_count`:

```bash
cargo test -p paneflow-app --release -- --ignored layout::render
```

It opens the 300 KB Rust corpus in a `CodeView` docked to the right of the pane
grid, fills every terminal pane with `deterministic_streams()` and lets them go
idle, places the caret at the top and scrolls away from it, then dispatches 120
`ScrollWheelEvent` notches of `Lines(3)` spaced 8 ms apart. It repeats that for
0, 2 and 6 terminal panes and prints one JSON line carrying
`scroll_frame_p50_us_panes_N` and `scroll_frame_p95_us_panes_N` for each N,
computed from GPUI's `dirty_to_draw_duration` over at least 100 frames per
configuration. `render_content_lock_samples_panes_N` counts the terminal grid
snapshots taken across those frames. Before EP-010 it read one snapshot per pane
per frame, the witness that a scroll which only moved the editor still repainted
every terminal. EP-010 hosts each `TerminalView` in a `ViewElement::cached`, so
an idle pane now takes none and the measurement asserts zero. A configuration
that cannot build its panes is reported with
`scroll_frame_available_panes_N: false` and the others still run.

**The measurement is relative, not absolute.** `TestAppContext` installs
`NoopTextSystem`, so no platform shaping is included: the numbers compare
configurations against each other and never bound the real cost of a frame.
The absolute cost is read from a release profile of the running application.

`terminal_share_p50_panes_6` and `terminal_share_p95_panes_6` are the fraction
of a six-pane scroll frame that disappears at zero panes. **US-029 hosts the
terminal panes behind `ViewElement::cached` only if that share reaches 0.30.**
Below it, caching the panes is not worth its complexity and the story is
canceled with the measured value recorded in the PRD changelog. The EP-006 run
measured 0.68 in p50 and 0.63 in p95, well past the threshold, so US-029
shipped.

`scroll_frame_p95_ratio_panes_N` is that configuration's p95 divided by the
zero-pane p95. It is a tracked measurement with no threshold attached. EP-010
dropped the 1.5 target it used to carry: a control run of the same tree with and
without `.cached` on the `TerminalView` moved the six-pane p95 from 1432 to
1426 us while the grid snapshots went from 720 to 0, so this harness cannot see
what the cache saves. Its `NoopTextSystem` excludes the shaping the cache skips
and keeps the scene replay and the `Pane` chrome, which carry the rest. The p95
ratio read 2.7 before EP-010 and reads 3.1 to 3.7 after it, the editor frame
having grown cheaper while the per-pane cost held.

## Running

```bash
scripts/bench-terminal.sh                 # Linux, macOS
scripts/bench-terminal.ps1                # Windows
scripts/bench-editor.sh                   # Linux, macOS
scripts/bench-editor.ps1                  # Windows
```

`scripts/bench-editor.sh --help` (and `scripts/bench-editor.ps1 -Help`)
prints the options and the environment variables both suites honor.

Each script builds the `paneflow` test binary under the release profile,
records the short commit SHA, whether the worktree is dirty, and a UTC stamp,
then writes its result under `bench/results/`. The run always prints a
Markdown table between the `PANEFLOW_BENCH_TABLE_BEGIN` and
`PANEFLOW_BENCH_TABLE_END` markers: a comparison table when the suite's
baseline exists, and the same table without its comparison columns when it
does not. That table is the artifact to share.

`--set-baseline` (or `-SetBaseline` on Windows) copies the fresh result over
the suite's baseline. The committed terminal baseline is the state of the
pipeline before the September 2026 performance work.

`scripts/bench-editor` refuses `--set-baseline` when the run reports a
`cpu_share` below 0.90: a contended run inflates every timing it would freeze,
and every later comparison against it would read as a false improvement. Close
the competing workload and run again.

**A change that moves a metric updates the baseline in the same pull request.**
A baseline older than the code it is compared against turns every table into
fiction: the editor baseline recorded before EP-002 to EP-005 reports 69 ms of
work per keystroke against a HEAD that measures 2.5 ms.

Both suites refuse to run under the debug profile, which would measure the
compiler rather than the code, and exit non-zero with an explicit message. Set
`PANEFLOW_BENCH_ALLOW_DEBUG=1` to override while developing a suite itself.

Neither suite runs in CI. They are local artifacts compared against a local
baseline, which is what makes the comparison meaningful.

## Fairness rules

A comparison is only meaningful between runs on the same machine, at the same
grid sizes, with the same corpus seed, and both built under the release
profile. The result document records OS, architecture, CPU model, profile,
seed, and commit so that a mismatched comparison is visible. Close heavy
applications before a run; the medians are robust to a stray interruption,
the p95 values are not.

Two runs of the same commit differ by a few percent on the microsecond
metrics. Treat a change below 5% as noise unless the allocation columns, which
are deterministic, moved with it.

The run measures its own CPU share over the timed scenarios (process CPU time
divided by wall time, recorded as `cpu_share` in the result). The scenarios
are single-threaded and never sleep, so an uncontended run reports close to
1.0. A run that prints `PANEFLOW_BENCH_WARNING` got less than 90% of a core:
something else was competing, its timings are inflated, and it should not be
published as a comparison.

## Reading the table

`Change` is the relative move of the headline value and, in parentheses, the
speedup: baseline over now for costs, now over baseline for throughput. A
timing that halved reads `-50.0% (2.00x)`. `Alloc/iter` columns show bytes
allocated per iteration and are exact.

## Result schema

```json
{
  "schema": 1,
  "suite": "paneflow-terminal-bench",
  "generated_unix": 0,
  "stamp": "20260901T120000Z",
  "git_sha": "4066faf6abcd",
  "git_dirty": "false",
  "os": "windows",
  "arch": "x86_64",
  "cpu": "...",
  "profile": "release",
  "corpus_seed": "0x...",
  "metrics": [
    {
      "metric": "publish_scroll_220x60",
      "unit": "ns",
      "direction": "lower_is_better",
      "value": 0.0,
      "p95": 0.0,
      "mean": 0.0,
      "alloc_bytes_per_iter": 0.0,
      "allocs_per_iter": 0.0,
      "iters": 300,
      "note": "..."
    }
  ]
}
```

The editor suite writes the same document with `"suite":
"paneflow-editor-bench"` and its own `corpus_seed`.

## Syntax query parity measurement

The Windows x86_64 release run
[editor-20260904T231427Z-d03590c2816f.json](results/editor-20260904T231427Z-d03590c2816f.json)
measures the Zed query integration on the working tree based on `d03590c2816f`.
CPU share was 0.965. `editor-baseline.json` remains unchanged. These are measured
pipeline times, not end-to-end GPU frame times.

| Metric | Measured | Required |
| --- | ---: | ---: |
| `keystroke_to_runs` p95 | 0.767 ms | < 1.5 ms |
| `viewport_query_60_rows` | 0.306 ms | < 0.6 ms |
| `fill_60_stale_rows` | 0.290 ms | < 0.6 ms |
| `resolve_runs_3750` | 18.4 us | < 500 us |
| `theme_switch` | 3.1 us | < 5 us |
| `open_markdown_injected` | 37.9 us | < 100 us |
| `pagedown_stale_rows_max` | 0 | 0 |
| `pagedown_frames_to_fresh` | 1 | 1 |
| `reload_200_retained_bytes` | 5,409,886 bytes | < 64 MiB |
| `plain_highlighter_retained_bytes` | 0 | 0 |
| `open_to_first_tree_300kb` | 45.365 ms | within 10% of the preceding implementation |

The historic baseline predates `open_to_first_tree_300kb`. Its comparison uses
[the last recorded EP-011 run](results/editor-20260904T210646Z-f76af1ef703c.json):
43.445 ms before, 45.365 ms after (+4.4%). Other comparisons use the baseline
selected by `scripts/bench-editor.ps1`.

## Startup suite

The benchmark is the ignored test `startup_first_frame_benchmark` in
`src-app/src/startup_bench.rs`. Unlike the other two suites it launches the
real release binary, because the cost it measures is the GPU window, the
platform text system, and the state the app builds before its first frame,
none of which exist in a window-free test. The app cooperates through the
startup trace in `src-app/src/startup_trace.rs`: when `PANEFLOW_STARTUP_TRACE`
names a file, the app records a mark at each stage of `main` and of
`PaneFlowApp::new`, writes the timeline as JSON once its first frame has been
presented, and quits. The probe ships in release builds so the shipping profile
is what gets measured.

Two scenarios run back to back, each against its own seeded `PANEFLOW_HOME`
under the system temp directory:

| Scenario | Prefix | Home contents |
|---|---|---|
| Welcome | `welcome_` | An empty session, so the first frame is the welcome screen. |
| Restore | `restore3_` | A session of three workspaces with one terminal pane each, all in a scratch directory. The daily case: a restored layout whose panes spawn shells. |

The seed writes `session.json`, `paneflow.json` (`{}`), `window-state.json`
(a fixed 1400x900 window) and `telemetry_id` before the first launch. Every
one of those files must exist: `migrate_legacy_home` copies the developer's
own files from the legacy `dirs::config_dir()` locations into any home that
lacks them, which would silently turn the fixture into the developer's real
session. One untimed warm-up launch per scenario absorbs whatever else the app
creates on first run; the timed launches that follow
(`PANEFLOW_BENCH_STARTUP_RUNS`, default 10) therefore measure a second launch.
The suite refuses a debug binary unless `PANEFLOW_BENCH_ALLOW_DEBUG` is set.
`PANEFLOW_BENCH_EXE` overrides the binary path, which otherwise resolves to
`paneflow` next to the test binary's profile directory.

| Metric | Unit | What it captures |
|---|---|---|
| `<scenario>_first_frame_total` | ns | From the first line of `main` to the first presented frame of the app window. The headline number of each scenario. |
| `<scenario>_step_<mark>` | ns | The time between one trace mark and the previous one, one metric per mark in launch order. `step_gpui_app_ready` is the platform and text system initialization inside GPUI, `step_fonts_loaded` the registration of the embedded fonts, `step_window_created` the GPU window, `step_ipc_server_started` the singleton guard and IPC thread, `step_workspaces_restored` the session restore, `step_first_render_built` the element tree construction, and `step_window_open_returned` the first layout and paint of that tree. |

The mark names are the metric names, so adding a mark adds a metric and the
comparison table reports it as new. The `cpu_share` field is `0.0` for this
suite: the timed work happens in a child process, so the harness cannot
attribute a core share to it.
