# Performance benchmarks

`bench/` holds the reproducible measurements behind Paneflow's performance
claims. Every number published about the terminal pipeline or the code editor
comes from one of the suites below, run with the scripts described here, and
the raw result of each run is archived next to the baseline it is compared
against.

There are two suites, two baselines, and two result prefixes:

| Suite | Test | Script | Baseline | Result files |
|---|---|---|---|---|
| `paneflow-terminal-bench` | `terminal::perf_bench::terminal_pipeline_benchmark` | `scripts/bench-terminal.sh` / `.ps1` | `bench/baseline.json` | `bench/results/<stamp>-<sha>.json` |
| `paneflow-editor-bench` | `app::diff_dock::code::perf_bench::editor_pipeline_benchmark` | `scripts/bench-editor.sh` / `.ps1` | `bench/editor-baseline.json` | `bench/results/editor-<stamp>-<sha>.json` |

Both suites share one harness, `src-app/src/bench_harness.rs`: the metric
type, the timing helpers, the JSON document, the comparison table, and the
single `#[global_allocator]` the test binary installs. That allocator counts
allocated bytes, allocation calls, and live bytes (allocations minus
deallocations), which is how a retained-memory metric can be reported at all.
It exists only in `cfg(test)` builds.

## Terminal suite

The benchmark is the ignored test `terminal_pipeline_benchmark` in
`src-app/src/terminal/perf_bench.rs`. It exercises the terminal pipeline
without a GPU or a window: the libghostty parser and snapshot, the conversion
into the renderer's neutral `Content`, the window-free layout pass, the
per-frame lookups the render thread performs, and the runtime loop's idle
behavior. Timings are wall-clock medians; allocations are counted by the
shared allocator.

| Metric | Unit | What it captures |
|---|---|---|
| `idle_wakeups_display_per_s` | wakeups/s | Runtime loop iterations of a display-only session with nothing to do. Direct CPU cost of an idle pane. |
| `idle_wakeups_shell_per_s` | wakeups/s | Same for a live shell sitting at its prompt. Skipped when the host cannot spawn a shell. |
| `publish_scroll_220x60` | ns | One scrolled line of styled output, then snapshot plus conversion to `Content`, on a 220x60 grid where every row is dirty. |
| `publish_echo_220x60` | ns | One keystroke echo on the bottom row, then snapshot plus conversion. Only one row changed. |
| `publish_scroll_120x40` | ns | The scroll case on a 120x40 grid, the size of a typical split pane. |
| `layout_220x60` | ns | The layout pass over a full 220x60 snapshot: run batching, background rectangles, contrast checks. |
| `line_text_at_220x60` | ns | Text of one hovered row extracted from the published snapshot, the input of link detection. |
| `base_font_resolve` | ns | The base font resolution the renderer performs for every pane on every frame. |
| `active_theme_read` | ns | The theme read the layout pass performs for every pane on every frame. |
| `gate_trickle_publishes` | frames per 1000 chunks | Frames the publish gate lets through when grid changes arrive every 2 ms with the queue drained. Bounds redraw frequency on trickle output such as ConPTY. |
| `pipeline_corpus_mib_s` | MiB/s | Parse plus snapshot plus conversion throughput over the deterministic corpus, one publish per stream. |

The corpus is `deterministic_streams()` in
`src-app/src/terminal/bench_corpus.rs`, seeded with `CORPUS_SEED`, so every
run parses byte-identical input.

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
| `theme_switch` | ns | A theme change on 300 KB of Rust, which today requeries the whole document on the render thread. |
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
