[PRD]
# PRD: Move Paneflow to GPUI at Zed `703b5ac3` and bring the UI in line with it

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-15 | Arthur Jean | Initial PRD: move the GPUI pin from `37faef81` to `703b5ac3`, migrate every broken API, and re-align paths, layers, frame pacing, animation, text and platform behavior with the new GPUI as Zed specifies and uses it. 6 epics, 20 stories. |

## Problem Statement

**Reference convention.** Paneflow paths refer to `main` at `9f606d3aa`. Zed paths are written `zed/...` and refer to `C:\dev\zed` at `703b5ac3f4ffe1e87a073123ae56e22f72f095c4` (2026-11-14, equal to `origin/main` that day); "old" means `37faef81f74fc1bb08be9939c4cffc8ede6ced2f` (2026-10-27), the revision pinned in `src-app/Cargo.toml:46,47,249,407`. Upstream commits are cited by short hash and PR number. `tasks/research/gpui-upgrade-703b5ac3.md` and `tasks/research/gpui-upgrade-703b5ac3-probe.patch` are local, untracked evidence from the 2026-11-15 research: a throwaway migration that compiled, passed clippy on Windows and Linux x86_64, passed `cargo deny`, and passed 4542 of 4544 tests on Windows.

1. **Paneflow is 65 GPUI commits behind and does not compile against current GPUI.** Moving the pin alone yields 35 errors in the binary and 54 in the test build. Three upstream changes cause them: a blanket `impl<T: Element> IntoElement for T` (`zed/crates/gpui/src/element.rs:145`, `39467cf4b1`, #64864), a non-generic opaque `Path` (`zed/crates/gpui/src/path_builder.rs:387`, `ffe0fa29e4`, #66809), and `Window::paint_layer` without a bounds argument (`zed/crates/gpui/src/window.rs:4911`, `1d8eda94bd`, #66810).
2. **The squircle shape cache rests on an API that no longer exists.** `src-app/src/ui_primitives/squircle.rs:153-160` moves a cached shape by adding an offset to each vertex. The new `Path` exposes no geometry (`path_builder.rs:387-410`), and the expensive step, binning curves into tiles, now runs on every `paint_path` (`path_builder.rs:407`, `window.rs:5170`). Six tests read path vertices or count builds (`squircle.rs:394,428,451,484`, `pane.rs:2324,2347`).
3. **The renderer was rewritten, so every painted surface can change without a compile error.** Paths moved from 4x MSAA to analytic coverage, shadows to a closed-form approximation, quads to a depth pre-pass, gradients to an ordered dither (`ba490197cf`, `caead827d7`, `ffe0fa29e4`, `1d8eda94bd`, `be07e43ea0`, `8a5cf7e23a`). Upstream states that strokes build 2 to 8 times slower and that adjacent paths can show seams. Paneflow paints 22 `paint_path` sites and builds strokes every frame for curly underlines and box-drawing arcs.
4. **Frame pacing changed silently.** Old GPUI limited only forced renders and next-frame callbacks (old `window.rs:1873-1875`); new GPUI limits every redraw (`window.rs:1977`, `2580a00264`, #66237) and adds two `WindowOptions` fields (`platform.rs:2691,2697`, `7c8dbadcb5`, #66281). Paneflow sets none of them (`src-app/src/launch.rs:417`, `src-app/src/app/detached_panes/window.rs:71`).
5. **Text lays out differently.** Wrapping follows UAX #14 (`f0210f1621`, #64624), tabs gain their native width on Windows where they had none (`0294cfb58e`, #66468), color emoji and combining marks are positioned differently (`4a6121bb0f`, `985bbf42e6`). The code editor and the diff view shape raw line text with no tab handling (`src-app/src/app/diff_dock/code/element.rs:432`, `src-app/src/diff/element.rs:402`).
6. **Paneflow hand-rolls what GPUI now provides.** Two spinners rebuild their view on every frame (`src-app/src/ui_primitives.rs:548-555,1014-1020`) where Zed animates in the retained scene (`zed/crates/ui/src/components/icon.rs:265-266`); submenus estimate their side from root bounds (`src-app/src/ui_primitives/action_menu.rs:442,864`) where Zed attaches to a target (`zed/crates/ui/src/components/context_menu.rs:1915-1917`).

**Why now:** the renderer rework landed on 2026-11-12 and every later GPUI fix builds on it, so the gap only widens. The migration surface is still small (22 mechanical sites and one redesign), and the same upstream range carries fixes Paneflow wants: a Windows crash in the display-change handler (`78f5d7446c`), zero-width tabs on Windows, animations stuck at 30 fps on M5 Macs (`0373366f43`), and Wayland activation (`4d847859f5`).

## Overview

The work happens on one branch and lands as one pull request series. EP-001 moves the pin to `703b5ac3`, mirrors Zed's `[patch.crates-io]` change for `async-task`, and migrates every broken call site so that the workspace builds, lints and tests on Linux, macOS and Windows. The squircle primitives stop translating cached shapes and build each shape at its final position, which is what Zed's own callers do (`zed/crates/ui/src/components/progress/circular_progress.rs:98`, `zed/crates/ui/src/components/divider.rs:105`).

EP-002 to EP-005 then bring behavior in line with the new GPUI, one domain per epic: paths and layers, frame pacing and animation, text, platform integration. Each story names the Zed definition, the Zed test or example that specifies the behavior, and where Zed's own application code uses it, so the implementer ports a pattern instead of inventing one. EP-006 re-baselines the benchmarks, writes the bump summary on the model of `bench/results/gpui-bump-37faef81-2026-10-28.md`, and updates `CHANGELOG.md`, `DESIGN.md` and `ARCHITECTURE.md`.

Key decisions, each with its evidence:
- **One pin move to `703b5ac3`, not two.** A staged move through `78f5d7446c` (the last commit before the renderer rework) would isolate risk but doubles the benchmark re-baseline and the visual pass, and was never compiled. The branch absorbs the risk instead: nothing merges before EP-001 to EP-003 and EP-006 are certified.
- **No translated shape cache.** `Path` cannot be moved or inspected, `PathBuilder::build` no longer tessellates, and binning is not cacheable from outside GPUI. A benchmark budget replaces the build-count tests; a bounded origin-keyed cache is the fallback only if the budget fails.
- **GPUI's default frame intervals, written explicitly.** Zed sets none of the three options (no match outside `zed/crates/gpui`), so the defaults are the specification: 33.333 ms inactive, 33.333 ms in low power, 16.667 ms under thermal pressure (`platform.rs:2850-2852`). This also keeps the promise already published in `CHANGELOG.md:141`.
- **Tabs expand to columns before shaping, 4 columns wide.** Zed's editor never hands a tab to the shaper: `TabMap` expands it with `tab_size - ((expanded_chars_old - 1) % tab_size)` (`zed/crates/editor/src/display_map/tab_map.rs:459`), default `"tab_size": 4` (`zed/assets/settings/default.json:1716`). Expanding gives identical columns on the three platforms.
- **macOS input latency is validated on hardware before release.** Upstream issue zed-industries/zed#66954 (open, untriaged) reports about 18 ms more key-to-screen latency on 120 Hz displays after `0373366f43`, and the new display link is unconditional on macOS 14 and later (`zed/crates/gpui_macos/src/window.rs:1160`).

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Paneflow builds on current GPUI | Pin at `703b5ac3`; `run_tests` green on the Linux x86_64, Linux aarch64, macOS aarch64 and Windows x86_64 legs | Each later GPUI bump lands within 14 days of the upstream commit it targets |
| No performance regression ships unexplained | Every metric of the terminal, editor and startup suites within 10 % of its committed baseline, or its delta explained in the bump summary | Allocation baselines within 1 % for 3 consecutive bumps |
| Path cost stays inside the frame | Squircle building for a 12-badge tab strip at or below 0.4 ms per frame (5 % of an 8.33 ms frame) | Same at 24 badges |
| Input latency holds | Keystroke-to-present latency on macOS at 120 Hz within 4.2 ms of the old pin; Windows and Linux within 2 ms | Same after the next bump |
| Text is identical across platforms where Paneflow owns layout | A tab occupies the same number of columns in the code editor and diff view on Linux, macOS and Windows (4-column stops) | Tab size configurable per user |
| Hand-rolled animation is gone | 0 spinner call sites that re-render a view per frame (from 2) | 0 view re-renders per frame for any rotating glyph |

## Target Users

### Paneflow maintainer
- **Role:** Arthur, the only contributor, who ships Paneflow on Linux, macOS and Windows.
- **Behaviors:** bumps GPUI by moving four `rev` values, then fixes what breaks; records each bump in `bench/results/` and `CHANGELOG.md`.
- **Pain points:** a pin 65 commits behind cannot take upstream fixes; the Windows `cfg` code is invisible to a Linux host and the macOS code to both; a renderer rewrite invalidates tests that encoded the old path API.
- **Current workaround:** staying on the old pin and carrying workarounds (the translated squircle cache, per-frame spinner re-renders, the Linux rounded-quad fallback at `src-app/src/ui_primitives/squircle.rs:236-246`).
- **Success looks like:** one certified branch, every gate green on the four CI legs, a bump summary with measured numbers, and no workaround kept without a stated reason.

### Developer running coding agents in Paneflow
- **Role:** a developer with several panes of agents streaming output, on a laptop or a desktop, on any of the three platforms.
- **Behaviors:** types in one pane while others stream; reads code in the diff dock; keeps secondary windows unfocused on another display.
- **Pain points:** on Windows, tabs in the code editor and diff view have no width; on M5 Macs animations run at 30 fps; a rotating loader costs a full view rebuild per frame.
- **Current workaround:** none for tabs or the M5 frame rate; closing panes with a "Thinking" agent to save CPU.
- **Success looks like:** the same UI as before the bump, with correct tab columns, no added input latency, and lower CPU while agents think.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Zed (the reference application):** rebuilds paths inside paint and never caches them (`zed/crates/ui/src/components/divider.rs:105-121`, `zed/crates/editor/src/element.rs:11233`); opens layers with `window.paint_layer(|window| { ... })` (`zed/crates/editor/src/element.rs:5086`); leaves the three frame intervals at their defaults; rotates icons through `TimedTransformation::rotation` (`zed/crates/ui/src/components/icon.rs:265-266`) behind `with_rotate_animation` (`zed/crates/ui/src/traits/animation_ext.rs:10`).
- **GPUI's own examples:** `zed/crates/gpui/examples/painting.rs:431` repaints clones of prebuilt paths, `zed/crates/gpui/examples/svg_animation.rs` compares retained-scene animation with per-frame re-rendering.
- **Other GPUI applications:** no public commit of longbridge/gpui-component adapting to these changes was found; forks only carry the upstream commits.
- **Market gap:** not applicable; this is a dependency migration.

### Best Practices Applied
- Mirror the dependency's own patch set: Zed moved `async-task` to `https://github.com/smol-rs/async-task` at `41fd3359c75e221bd8b187a8ee361aa309c1714c` (`zed/Cargo.toml:1045-1046`), and Paneflow's rule is to stay in step (`Cargo.toml:93-94`).
- Treat upstream PR text as the specification of limits: #66809 states "Code that caches and reuses `Path`s pays binning cost on every paint", "strokes are 2-8x slower", and that conflation artifacts can appear where MSAA showed none; #66812 states output "differs from `main` by at most 5/255 per channel".
- Port the tests that specify behavior rather than re-deriving it: `test_wrap_line_break_opportunities` (`zed/crates/gpui/src/text_system/line_wrapper.rs:1113`), `power_modes_preserve_frame_limits` and `frame_limits_combine_enabled_intervals` (`zed/crates/gpui/src/window.rs:8994,9046`), `cached_view_follows_inherited_opacity` (`window.rs:10638`), `tabs_preserve_native_advances_and_cursor` (`zed/crates/gpui_windows/src/direct_write.rs:1808`).
- Verified locally against the code: `TimedTransformation` advances the retained scene on the CPU (`zed/crates/gpui/src/scene.rs:115`, `window.rs:3059`), so it works on every renderer including DirectX; color emoji bounds on Windows are the union of the layers (`direct_write.rs:797-808`).

*Full research sources: `tasks/research/gpui-upgrade-703b5ac3.md` (local evidence), the upstream PRs named above, and zed-industries/zed#66954.*

## Assumptions & Constraints

### Assumptions (to validate)
- Building each squircle at its final position on every paint fits the 0.4 ms budget, because fills are reported "about as fast as before or slightly faster" (#66809). Validated by US-003.
- Curly underlines and box-drawing arcs stay inside the terminal suite's baseline despite slower strokes. Validated by US-003.
- macOS key-to-present latency does not regress beyond 4.2 ms at 120 Hz. Contradicted by an open upstream report; validated by US-010 on real hardware.
- No `cfg(target_os = "macos")` code in Paneflow uses a broken API. Based on greps, never compiled; validated by US-002 through CI.
- The Linux rounded-quad fallback for translucent squircles existed because paths went through an intermediate MSAA texture. Inferred, no document states the reason; validated by US-008.

### Hard Constraints
- Every change works on Linux (Wayland and X11), macOS and Windows; `cfg(windows)` items are linted only by the Windows CI leg, and any item added to a file with a `mod tests` goes before that module (`AGENTS.md`).
- Rust toolchain stays at 1.98.0 (`rust-toolchain.toml`); the migration compiled on it.
- Source code carries no comments; intent goes into names, types and tests (`AGENTS.md`).
- The render thread never blocks; no synchronous I/O is added to paint paths.
- Benchmarks compare only against their own platform's baseline; baselines exist for `linux-x86_64` only (`bench/README.md:76-78`), and `--set-baseline` refuses a dirty tree. No `--profile <run-name>`.
- Visual verification is Arthur's: no story launches or screenshots the installed app. A story delivers the change and the list of surfaces to look at.
- No AI attribution in commits; commits follow `type(scope): US-NNN - description`.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints on the host target
- `cargo nextest run --workspace --locked --profile ci` - the test suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget, when a test changes
- `cargo deny check advisories licenses sources` - when `Cargo.toml`, `Cargo.lock` or `deny.toml` changes

On the Windows host, the Linux leg is proven before a push by running the clippy command in WSL on a clone of the exact commit, with a private `CARGO_TARGET_DIR`.

For UI stories, additional gates:
- The `DESIGN.md` UI delivery gate (section 9, `DESIGN.md:1662-1678`): Dark, Light and one other preset; `reduce_motion` on and off; the 800 by 500 minimum window; every control state.
- The story lists the surfaces and states it changed for Arthur's manual visual pass. The implementing agent does not launch the installed app.

## Epics & User Stories

### EP-001: The pin moves and every target builds

Paneflow resolves GPUI at `703b5ac3`, with the same forks Zed builds against, and compiles, lints and tests on Linux, macOS and Windows.

**Definition of Done:** the four `rev` values name `703b5ac3`, `Cargo.lock` is committed, and every required `run_tests` job is green on the pull request commit.

**Zed references:**
- `zed/Cargo.toml:1044-1056`: the `[patch.crates-io]` set GPUI is built against; `async-task` at `:1045-1046`.
- `zed/crates/gpui/src/element.rs:52,145`: `pub trait Element: 'static + Sized` and `impl<T: Element> IntoElement for T {`.
- `zed/crates/gpui/src/path_builder.rs:387-410`: `pub struct Path { curves: Arc<[PathCurve]>, even_odd: bool }`, no public accessor.
- `zed/crates/gpui/src/window.rs:4911,5170`: `pub fn paint_layer<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R` and `pub fn paint_path(&mut self, path: Path, color: impl Into<Background>)`.
- `zed/rust-toolchain.toml:2`: `channel = "1.98.1"`, unchanged across the range.

#### US-001: Move the pin, mirror Zed's patch set and migrate every broken call site
**Description:** As the maintainer, I want the GPUI pin at `703b5ac3` with every compile break fixed in one atomic change so that the workspace builds, lints and tests on the host again.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** None

**Zed references:**
- `zed/Cargo.toml:1045-1046`: `async-task = { git = "https://github.com/smol-rs/async-task", rev = "41fd3359c75e221bd8b187a8ee361aa309c1714c" }`.
- `zed/crates/gpui/src/element.rs:145` (`39467cf4b1`, #64864); reference element after the change: `zed/crates/terminal_view/src/terminal_element.rs:1102` (`impl Element for TerminalElement`, no `IntoElement` impl).
- `zed/crates/gpui/src/path_builder.rs:126,137,252,284,387` (`ffe0fa29e4`, #66809): `stroke`, `fill`, `translate`, `build(self) -> Result<Path, Error>`, `Path`.
- `zed/crates/gpui/src/window.rs:4911` (`1d8eda94bd`, #66810); reference call: `zed/crates/editor/src/element.rs:5086`, `window.paint_layer(|window| {`.
- Building at the final position, as Zed does: `zed/crates/ui/src/components/progress/circular_progress.rs:98`.

**Paneflow sites:** `src-app/Cargo.toml:46,47,249,407`; `Cargo.toml:97`; `deny.toml:144-153`; `IntoElement` impls at `src-app/src/widgets/text_input.rs:816`, `src-app/src/diff/element.rs:1567`, `src-app/src/ui_primitives.rs:142,477`, `src-app/src/app/sidebar/style.rs:104`, `src-app/src/terminal/element/mod.rs:1856`, `src-app/src/app/diff_dock/code/element.rs:1123`; `Path<Pixels>` at `src-app/src/ui_primitives/squircle.rs:87-186,380`, `src-app/src/ui_primitives/ring.rs:198,204`, `src-app/src/diff/element.rs:173`; `paint_layer` at `src-app/src/ui_primitives/shimmer.rs:178`, `src-app/src/terminal/element/mod.rs:1695,1733`, `src-app/src/diff/element.rs:1550`, `src-app/src/app/diff_dock/code/navigation_paint.rs:25`.

**Acceptance Criteria:**
- [ ] Given `src-app/Cargo.toml`, when the four `rev` values are read, then each equals `703b5ac3f4ffe1e87a073123ae56e22f72f095c4`, and the committed `Cargo.lock` resolves under `--locked`
- [ ] Given the root `Cargo.toml`, when the `async-task` patch is read, then its URL and revision equal Zed's at `zed/Cargo.toml:1046`, and the `async-process`, `calloop` and `khronos-egl` revisions still equal Zed's
- [ ] Given `deny.toml`, when `allow-git` is read, then it lists `https://github.com/smol-rs/async-task`, no longer lists `https://github.com/zed-industries/async-task.git`, and the note above it names this bump
- [ ] Given a type that implements `Element`, when the source is searched, then no hand-written `impl IntoElement` exists for it, and `SelectMenu` (`src-app/src/settings/components.rs:1323`), which does not implement `Element`, keeps its impl
- [ ] Given any use of a GPUI path, when the source is searched, then the type is written `Path` with no generic argument and no code reads a field of `Path`
- [ ] Given the five `paint_layer` calls, when they compile, then each passes only a closure, and the terminal still opens its background layer before its text layer
- [ ] Given a squircle request, when it is painted, then the shape is built at its final bounds, `translated` and the vertex-offset cache are gone, and the Surface and Classic styles still paint quads without building a path (`src-app/src/ui_primitives.rs:1394`)
- [ ] Given the tests at `src-app/src/ui_primitives/squircle.rs:394,428,451,484` and `src-app/src/pane.rs:2324,2347`, when the suite runs, then they are replaced by tests on observable behavior (which requests build a path, which paint quads, one build per painted shape) and none reads path geometry
- [ ] Given zero-sized or negative bounds, when a squircle is requested, then no path is built and nothing panics
- [ ] Given `PathBuilder::build` returns an error, when a shape is painted, then that shape is skipped and the frame still completes
- [ ] No lint is silenced with `#[allow]` to make the migration compile, and no comment is added to source code

#### US-002: Prove the Linux, macOS and Windows legs
**Description:** As the maintainer, I want the migrated commit proven on every CI leg so that platform-gated code I cannot compile locally is known to build and pass.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui_windows/build.rs` (shader list changed by `ffe0fa29e4` and `4a6121bb0f`; still compiled with fxc, no new SDK requirement).
- `zed/crates/gpui_apple/src/display_link.rs:60-62`: `AnyClass::get(c"CAMetalDisplayLink").is_some()`, a runtime check, so the minimum macOS version does not rise.
- `zed/crates/gpui_linux/src/linux/wayland/window.rs:21`: `ext_background_effect_surface_v1` comes from the `wayland-protocols` crate, no new system library.

**Paneflow sites:** `.github/workflows/run_tests.yml:181` (Style), `:228` (Tests Linux x86_64), `:298` (Performance gates), `:430` (macOS aarch64 smoke build), `:645` (macOS render smoke), `:853` (Windows x86_64 libghostty check), `:1034` (Windows render smoke), `:1255` (Linux aarch64); `scripts/check-helper-sizes.sh`.

**Acceptance Criteria:**
- [ ] Given the pull request commit, when `run_tests` completes, then Style, Tests (Linux x86_64), Linux aarch64, macOS aarch64 smoke build, macOS render smoke, Windows x86_64 libghostty check, Windows render smoke and cargo-deny are green
- [ ] Given the Windows host, when the Linux clippy command runs in WSL on a clone of the exact commit, then it passes before the push
- [ ] Given the Linux x86_64 build, when `scripts/check-helper-sizes.sh` runs, then the three helper binaries stay under their caps
- [ ] Given a `cfg(target_os = "macos")`, `cfg(unix)` or `cfg(windows)` item fails on its leg, when the failure is read, then it is fixed in this story and the report states, per platform, whether the code was verified by compilation or by inspection
- [ ] Given the Performance gates job fails on the bump, when the failure is read, then the failing budget and its measured value are recorded for US-003 and US-019, and no budget in `crates/paneflow-host/tests/persistent_baseline/gates.rs` is raised in this story

---

### EP-002: Shapes, paths and layers follow the new renderer

Every path, shadow, gradient and layer Paneflow paints is correct and within budget under analytic path coverage, primitive batching and the depth pre-pass.

**Definition of Done:** path building has a measured budget and a benchmark that guards it, no two paths share a coincident edge, and `DESIGN.md` states the rendering facts as they now are.

**Zed references:**
- `zed/crates/gpui/src/path_winding.rs:1-26`: module doc, winding computed per pixel on the GPU with CPU binning into cells.
- `zed/crates/gpui/src/path_builder.rs:24-58,142,149,407`: `StrokeOptions`, `FillOptions`, `with_style`, `dash_array`, `Path::bin`.
- `zed/crates/gpui/src/scene.rs:363-389`: `PrimitiveKind` order (Shadow, Quad, Path, Underline, sprites, Surface) and `max_shared_kind`: "We guarantee that overlapping primitives are drawn in paint order."
- `zed/crates/gpui/src/window.rs:4925,5085-5164`: `paint_drop_shadows`; `paint_quad` splitting rounded or bordered quads into an interior and four strips.
- `zed/crates/gpui_wgpu/src/shaders.wgsl:1042`, `zed/crates/gpui_apple/src/shaders.metal:506`, `zed/crates/gpui_windows/src/shaders.hlsl:920`: "Raph Levien's blurred rounded rectangle approximation".
- Upstream: `ba490197cf` (#66806), `caead827d7` (#66807), `ffe0fa29e4` (#66809), `1d8eda94bd` (#66810), `be07e43ea0` (#66811), `8a5cf7e23a` (#66812), `d3fc053d5b` (#66898).

#### US-003: Validate assumption: per-paint path building fits the frame budget
**Description:** As the maintainer, I want the cost of building squircles and terminal strokes measured on the new renderer so that the cache decision and the stroke mitigations rest on numbers, not on upstream's general statement.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- #66809 PR text: fills "about as fast as before or slightly faster", "strokes are 2-8x slower", "a 1,000-point scribble takes about 1.5ms now vs 0.2ms before".
- `zed/crates/gpui/src/path_builder.rs:306-364`: `stroke_outline`, strokes converted to an outline through `tiny_skia_path` then binned like a fill.
- `zed/crates/gpui/examples/paths_bench.rs:28` and `zed/crates/gpui_platform/benches/primitives.rs:245`: upstream's own path benchmarks.

**Paneflow sites:** `src-app/src/ui_primitives/squircle.rs:162-215`; `src-app/src/pane.rs:539-555` (tab badges); `src-app/src/terminal/element/paint/decorations.rs:101-131` (curly underline, two cubic segments per column); `src-app/src/terminal/element/paint/sprites.rs:346-421,533-584` (arcs, diagonals); `src-app/src/bench_harness.rs`; `scripts/bench-terminal.sh`.

**Acceptance Criteria:**
- [ ] Given a release build on the Linux x86_64 baseline machine class, when the measurement runs, then it reports the time to build and paint the paths of a 12-badge tab strip (48 shapes), a 200 by 50 terminal screen with a curly underline on every row, and a 200 by 50 screen of box-drawing arcs and diagonals
- [ ] Given the three results, when they are written, then they sit in a results file under `bench/results/` with the commit, the machine and the command, and each number is backed by the benchmark harness output
- [ ] Given the squircle result, when it is compared to 0.4 ms per frame, then the verdict (within or over budget) is stated and decides which branch of US-004 applies
- [ ] Given the two terminal results, when they are compared to the committed terminal baseline, then any metric more than 10 % above baseline is named with the mitigation US-004 or US-019 must carry
- [ ] Given two consecutive runs of the same case differ by more than 10 %, when the results are read, then the run is discarded and repeated on an idle machine, and no number from it is recorded
- [ ] No throwaway Cargo profile is created; an isolated build uses `CARGO_TARGET_DIR` outside the repository and is deleted afterwards

#### US-004: Lock the squircle cost behind a benchmark, and mitigate if the budget fails
**Description:** As the maintainer, I want squircle building guarded by a committed benchmark case so that a later change or bump cannot make tab strips slow without a red signal.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Zed references:**
- `zed/crates/gpui/src/path_builder.rs:407`: `pub(crate) fn bin(&self, scale: f32) -> PathTiles`, the per-paint cost a caller cannot cache.
- `zed/crates/gpui/src/path_builder.rs:387`: `#[derive(Clone, Debug)] pub struct Path`, cloning shares the curves through an `Arc`.
- `zed/crates/gpui/examples/painting.rs:431`: prebuilt paths repainted through `window.paint_path(path, color)`, the only caching pattern upstream shows.

**Paneflow sites:** `src-app/src/ui_primitives/squircle.rs:68-151`; `src-app/src/pane.rs:2324-2366`; `src-app/src/app/diff_dock/code/perf_bench.rs` and `src-app/src/terminal/perf_bench.rs` (suite hosts); `bench/baselines/linux-x86_64/`.

**Acceptance Criteria:**
- [ ] Given the benchmark suites, when they run, then a case measures squircle building for a 12-badge tab strip and its baseline is committed under `bench/baselines/linux-x86_64/`
- [ ] Given US-003 found the cost at or below 0.4 ms, when the code is read, then no shape cache exists and shapes are built at their final bounds
- [ ] Given US-003 found the cost above 0.4 ms, when the code is read, then a cache keyed by shape, bounds origin and scale factor holds at most 256 entries, evicts the least recently used entry, and the benchmark case is at or below 0.4 ms
- [ ] Given the window moves to a display with another scale factor, when the next frame paints, then every shape is built for the new scale and none built for the old scale is painted
- [ ] Given 1,000 distinct origins in one scroll, when the frames paint, then the cache, if present, never exceeds 256 entries and never clears wholesale

#### US-005: Ring gauge and check mark paint as single round-capped strokes
**Description:** As a user, I want gauges and check marks drawn as one stroke each so that no seam shows between an arc and its end caps under analytic coverage.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui/src/path_builder.rs:24-44`: `pub struct StrokeOptions { pub line_width: f32, pub line_cap: LineCap, pub line_join: LineJoin, pub miter_limit: f32 }`, default `LineCap::Butt`, `LineJoin::Miter`, `4.0`.
- `zed/crates/gpui/src/path_builder.rs:9,142`: `pub use lyon_path::{FillRule, LineCap, LineJoin};` and `pub fn with_style(self, style: PathStyle) -> Self`.
- `zed/crates/gpui/examples/painting.rs:275-280`: `PathBuilder::stroke(px(1.)).with_style(PathStyle::Stroke(options))`.
- `zed/crates/ui/src/components/progress/circular_progress.rs:98,127,175`: Zed's circular gauge built from strokes.

**Paneflow sites:** `src-app/src/ui_primitives/ring.rs:152-208` (arc plus two cap discs, three paths); `src-app/src/app/clone_bubble.rs:257-266` (check mark stroke).

**Acceptance Criteria:**
- [ ] Given an open gauge, when it paints, then its value arc is one stroke with `LineCap::Round` and the two cap discs are gone, so a gauge paints at most two paths (track and value)
- [ ] Given a full gauge (`sweep.closed`), when it paints, then it paints a closed circle stroke as today
- [ ] Given the check mark in the clone bubble, when it paints, then it is one stroke with round caps and round joins
- [ ] Given a gauge whose radius is at or below half its stroke, when it paints, then nothing is painted and nothing panics
- [ ] Given a gauge value of 0 or a non-finite value, when it paints, then only the track paints

#### US-006: A fill and its border never share a coincident path edge
**Description:** As a user, I want bordered shapes free of hairline seams so that pane cards, dialogs and rows look as they did under MSAA.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- #66809 PR text: "you may see conflation artifacts where MSAA didn't have any", the seam appears where two separately painted paths share an edge.
- `zed/crates/gpui/src/path_builder.rs:297-303`: a stroke is filled `FillRule::NonZero`, a fill uses its own rule (`EvenOdd` by default, `:54-58`).
- `zed/crates/gpui/src/window.rs:5085-5164`: GPUI itself splits a rounded or bordered quad into an interior and strips whose masks touch without overlapping.

**Paneflow sites:** `src-app/src/ui_primitives/squircle.rs:250-300,316-344` (fill, then border); `src-app/src/pane.rs:539-555` (badge layers); `src-app/src/settings/components.rs:444-455` (focus ring and border); `src-app/src/window_chrome/mod.rs:45-91`; `src-app/src/diff/element.rs:680-691,1502` (hatch).

**Acceptance Criteria:**
- [ ] Given a squircle with a fill and a border, when both paint, then the fill extends under the border by at least half the border width, so no edge of the border coincides with an edge of the fill
- [ ] Given the pane badge layers at `src-app/src/pane.rs:539-555`, when they paint, then each upper layer overlaps the layer below by at least 0.5 device pixel along shared contours
- [ ] Given a translucent border color, when it paints over the extended fill, then the visible fill color inside the border is unchanged from the opaque case, with a test on the bounds handed to the builder
- [ ] Given a border width of 0 or bounds smaller than the border width, when the shape paints, then only the fill paints and nothing panics
- [ ] The story lists every bordered surface it changed (pane card, dialogs, tab icon cards, terminal search, tooltip, settings controls) for the visual pass

#### US-007: Shadows, gradients and terminal layer order conform to DESIGN.md
**Description:** As the maintainer, I want every shadow, gradient and layer checked against the design contract so that the renderer change does not drift the UI away from `DESIGN.md`.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui/src/style.rs:349,366`: `pub struct BoxShadow` and `BoxShadow::new(x, y, color)`; Zed's elevation shadows at `zed/crates/ui/src/styles/elevation.rs:42-50`.
- `zed/crates/gpui/src/window.rs:4925`: `paint_drop_shadows`, bounds dilated by three times the blur.
- #66810 PR text: a shadow's bounds "are now the area it paints"; #66812: at most 5/255 per channel.
- `zed/crates/gpui_wgpu/src/shaders.wgsl:426-434`: ordered Bayer dither, new on Linux (`caead827d7`).
- `zed/crates/gpui/src/scene.rs:363-389` and `zed/crates/gpui/src/window.rs:4907-4911`: within one layer primitives draw by kind then in paint order; a nested layer draws above its parent.

**Paneflow sites:** shadows at `src-app/src/window_chrome/csd.rs:105`, `src-app/src/ui_primitives/action_menu.rs:831,835`, `src-app/src/settings/components.rs:571,1035-1039,1158-1164`, `src-app/src/pane.rs:394,396`, `src-app/src/app/clone_bubble.rs:1054`; gradients at `src-app/src/pane.rs:317,482,496,512`, `src-app/src/app/sidebar/resize.rs:204,209`; layers at `src-app/src/terminal/element/mod.rs:1695-1752`; contract at `DESIGN.md:110-115,172,542-543,1370` and `ARCHITECTURE.md:293-307`.

**Acceptance Criteria:**
- [ ] Given each shadow site, when its values are read, then they equal the values `DESIGN.md` states for that surface, and no site compensates in code for the new approximation
- [ ] Given the terminal element, when a frame paints, then backgrounds are painted in the first layer, glyphs and box-drawing paths in the second, and decorations, cursor and Kitty images outside both, with a test asserting the call order of the paint phases
- [ ] Given `ARCHITECTURE.md:293-307`, when it is read after the story, then it states the ordering rule as GPUI now defines it (kind, then paint order, a nested layer above its parent) with no claim the code no longer relies on
- [ ] Given a Kitty image with a negative z-index and one with a non-negative z-index on the same cell, when the frame paints, then the first is painted before the text layer and the second after it
- [ ] The story lists the shadow and gradient surfaces for the visual pass, naming Linux as the platform where gradients gain dithering

#### US-008: Re-evaluate the Linux translucent-window squircle fallback
**Description:** As a Linux user with a translucent backdrop, I want squircles to keep their continuous corners so that the UI matches the other platforms, if the reason for the fallback is gone.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-006

**Zed references:**
- #66809 PR text: paths used to render "into a separate MSAA target, which was resolved into an intermediate texture and then drawn onto the main target"; now "Tiles are drawn in the main render pass alongside every other primitive."
- `zed/crates/gpui_wgpu/src/wgpu_renderer.rs:490-499`: the surface is configured once with the renderer's alpha mode (`e20d19d32f`, #66002).
- `8e474e1598` (#66902): premultiplied alpha passed to wgpu shaders as a uniform.

**Paneflow sites:** `src-app/src/ui_primitives/squircle.rs:236-246,304-314` (`translucent_over_transparent_window`); `src-app/src/window_chrome/linux_backdrop.rs`.

**Acceptance Criteria:**
- [ ] Given a translucent window on Wayland and on X11, when a translucent squircle paints as a path, then the result is recorded (correct blending, or the defect observed) with the compositor and driver named
- [ ] Given the path blends correctly on both, when the code is read, then the rounded-quad fallback and `translucent_over_transparent_window` are removed
- [ ] Given the path does not blend correctly on either, when the code is read, then the fallback stays and the reason is recorded in `DESIGN.md` next to the squircle section
- [ ] Given the check cannot be run on a real Wayland or X11 session, when the story is reported, then it says so and the fallback stays untouched

---

### EP-003: Frame pacing and animation follow the new scheduler

Paneflow states its frame policy, keeps its input latency, and animates rotating glyphs in GPUI's retained scene.

**Definition of Done:** both windows set the three frame intervals explicitly, macOS latency is measured on hardware, and no spinner rebuilds a view per frame.

**Zed references:**
- `zed/crates/gpui/src/platform.rs:2681-2697,2850-2852`: the three `Option<Duration>` fields and their defaults (33,333, 33,333 and 16,667 microseconds).
- `zed/crates/gpui/src/window.rs:1374-1397,1977`: `fn minimum_frame_interval`, the longest enabled interval wins; `let should_throttle = !request_frame_options.require_presentation;`.
- `zed/crates/gpui/src/elements/svg.rs:83,162,341,375,406`: `with_timed_transformation`, the reduced-motion filter, `Easing`, `TimedTransformation`, `rotation`.
- `zed/crates/gpui/src/scene.rs:84-124` and `zed/crates/gpui/src/window.rs:3059`: animated sprites advanced without rebuilding the scene.
- `zed/crates/gpui/src/view.rs:328,540-548,624-640`: the cached-view key gains `opacity`.
- Upstream: `2580a00264` (#66237), `7c8dbadcb5` (#66281), `0373366f43` (#65472), `99e70ee4a0` (#66317), `aff99ecc32` (#66896), `8fa085e397` (#65951), `decd2399b1` (#65952).

#### US-009: Write the frame-interval policy explicitly on both windows
**Description:** As the maintainer, I want the three frame intervals stated in Paneflow's window options so that the frame policy is a decision in our code and a later GPUI default change cannot alter it silently.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui/src/platform.rs:2681-2697`: "Defaults to approximately 30 fps. Set to `None` to disable this limit. The longest applicable interval wins; mandatory platform presentations bypass all limits."
- `zed/crates/gpui/src/platform.rs:2850-2852`: `inactive_frame_interval: Some(Duration::from_micros(33_333))`, `low_power_frame_interval: Some(Duration::from_micros(33_333))`, `thermal_frame_interval: Some(Duration::from_micros(16_667))`.
- `zed/crates/gpui/src/window.rs:8994,9046`: tests `power_modes_preserve_frame_limits` and `frame_limits_combine_enabled_intervals`.
- `zed/crates/gpui/src/window.rs:1978-1979`: the inactive limit is lifted during sustained high-rate input; the low-power and thermal limits are not.
- `zed/crates/gpui_windows/src/platform.rs:890`: Windows low power is read from `GUID_POWER_SAVING_STATUS` only.

**Paneflow sites:** `src-app/src/launch.rs:417-429`; `src-app/src/app/detached_panes/window.rs:71-97`; `bench/results/gpui-bump-37faef81-2026-10-28.md:60-87,271-278` (the earlier 30 fps finding); `CHANGELOG.md:141`.

**Acceptance Criteria:**
- [ ] Given the main window and a detached pane window, when their `WindowOptions` are built, then both set `inactive_frame_interval`, `low_power_frame_interval` and `thermal_frame_interval` from one shared definition, with the values 33.333 ms, 33.333 ms and 16.667 ms
- [ ] Given the shared definition, when a test reads it, then it asserts the three values, so a change of policy is a deliberate edit
- [ ] Given a debug build with `PANEFLOW_LATENCY_PROBE=1` on Windows and on Linux, when keystroke latency is measured on the old pin and on the new pin with the window focused, then the two results are recorded and differ by at most 2 ms
- [ ] Given an unfocused window streaming terminal output, when it redraws, then it presents at most 30 frames per second, and the behavior is recorded in the bump summary as a user-visible change
- [ ] Given the latency difference exceeds 2 ms, when the result is read, then the story is not marked done and the cause is investigated before any interval is changed

#### US-010: Validate assumption: macOS key-to-present latency does not regress
**Description:** As a macOS user on a 120 Hz display, I want keystroke latency unchanged by the bump so that the terminal keeps feeling immediate.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002

**Zed references:**
- `zed/crates/gpui_macos/src/window.rs:1160`: `let metal_frames = MetalFrameSource::is_available().then(Rc::<MetalFrames>::default);`, unconditional on macOS 14 and later.
- `zed/crates/gpui_macos/src/display_link.rs:1-5`: "On macOS 14 and later a window is paced by a `CAMetalDisplayLink` on its layer"; earlier systems keep the `CVDisplayLink` registry.
- `zed/crates/gpui_apple/src/metal_renderer.rs:424-427`: the link "hands out each frame's drawable early enough for the frame to reach the display on time".
- zed-industries/zed#66954 (open, untriaged, 2026-11-14): key-to-screen latency from 27 ms to about 45 ms at 120 Hz, attributed to `0373366f43` (#65472).

**Paneflow sites:** `src-app/src/terminal/view.rs:68` (latency probe); `ARCHITECTURE.md:175-203,317-319` (the probe does not measure the visible pixel); `scripts/bench-startup.sh`.

**Acceptance Criteria:**
- [ ] Given a Mac on macOS 14 or later with a 120 Hz display, when keystroke-to-present latency is measured on the old pin and on the new pin with the same method, then both numbers, the method and the machine are recorded in the bump summary
- [ ] Given the new pin is within 4.2 ms of the old one, when the result is read, then the assumption is recorded as validated
- [ ] Given the new pin is more than 4.2 ms slower, when the result is read, then the macOS release of the bump is held, the finding is recorded with a link to zed-industries/zed#66954, and the options (wait for an upstream fix and re-pin, or carry a vendored patch) are put to Arthur
- [ ] Given no such hardware is available to the implementing agent, when the story is reported, then it is left for manual verification with the exact procedure written down, and it is not marked done
- [ ] Given the measurement, when startup is also traced, then time to first frame on macOS is recorded on both pins

#### US-011: Spinners rotate through TimedTransformation
**Description:** As a user watching an agent think, I want rotating loaders animated in the retained scene so that a spinner no longer rebuilds its view on every frame.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui/src/elements/svg.rs:83`: `pub fn with_timed_transformation(mut self, transformation: TimedTransformation) -> Self`, "phase-locked to the app's shared animation clock".
- `zed/crates/gpui/src/elements/svg.rs:406`: `pub fn rotation(period: Duration, from: Transformation) -> Self`, one clockwise turn per period.
- `zed/crates/gpui/src/elements/svg.rs:162`: `.filter(|timed| timed.is_animated() && !cx.reduce_motion())`.
- `zed/crates/ui/src/components/icon.rs:265-266`: `svg.with_timed_transformation(TimedTransformation::rotation(period, self.transformation))`; `zed/crates/ui/src/traits/animation_ext.rs:6-12`: "The view isn't re-rendered to advance the rotation. All spinners share one phase."
- `zed/crates/gpui/examples/svg_animation.rs:99`; `zed/crates/gpui/src/elements/animation.rs:1012` (`test_reduce_motion_renders_single_static_frame`).

**Paneflow sites:** `src-app/src/ui_primitives.rs:536-555` (`comet_spinner`, `COMET_TURN` 700 ms); `src-app/src/ui_primitives.rs:1012-1020` (empty-state spinner, 1 s); `src-app/src/ui_primitives.rs:92` (`set_reduce_motion`, which also sets GPUI's flag); `DESIGN.md:493-510`.

**Acceptance Criteria:**
- [ ] Given `comet_spinner`, when it renders, then it uses `with_timed_transformation` with a 700 ms rotation and no `with_animation` closure
- [ ] Given the empty-state spinner, when it animates, then it uses `with_timed_transformation` with a 1 s rotation
- [ ] Given a view that shows a spinner and nothing else changing, when 60 frames pass, then the view's render count stays at 1, asserted by a test
- [ ] Given `reduce_motion` is on, when a spinner renders, then it is static, as `DESIGN.md:507` requires
- [ ] Given two spinners on screen, when they animate, then they share one phase, and `DESIGN.md` records that the phase is app-wide instead of per element
- [ ] The grid loader at `src-app/src/app/sidebar/lane.rs:329-344` is unchanged

#### US-012: Cached views repaint on inherited opacity and keep subscriptions without redraw loops
**Description:** As the maintainer, I want the two upstream cached-view fixes confirmed against Paneflow's cached views so that sidebars fade correctly and no new subscription turns into a redraw loop.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui/src/view.rs:46,328`: `AnyView::cached` doc and the `opacity: f32` field of the cache key; `zed/crates/gpui/src/window.rs:10638` (`cached_view_follows_inherited_opacity`).
- `zed/crates/gpui/src/elements/deferred.rs:152,261`: `test_cached_reuse_keeps_entity_subscriptions`, `test_nested_deferred_draws_with_reused_views`.
- `zed/crates/gpui/src/app.rs:1466`: accessed entities tracked as a stack of scopes.
- zed-industries/zed#66690: views drawn from the cache drop their accessibility nodes (open upstream, to check against Paneflow's roles).

**Paneflow sites:** `src-app/src/app/render.rs:658-663` (sidebar under an opacity wrapper); `src-app/src/app/diff_dock/mod.rs:566` and `src-app/src/app/files_sidebar/integration.rs:79`; `src-app/src/pane.rs:1997` (terminal); deferred menus at `src-app/src/app/sidebar/context_menu.rs:127`, `agents_section.rs:1560`, `customize_menu.rs:303`, `space_switcher.rs:483`, `status_groups.rs:300`; counters and tests at `src-app/src/app/sidebar/lane.rs:676`, `src-app/src/app/files_sidebar/tests.rs:59-110`, `src-app/src/terminal/view.rs:1955-2036`.

**Acceptance Criteria:**
- [ ] Given the primary sidebar collapsing, when its wrapper opacity changes, then the cached `SidebarView` repaints with the new opacity on that frame, asserted by a test
- [ ] Given a sidebar menu open and one agent in the Thinking state, when 100 root renders pass with no other input, then the sidebar render count stays in the range `lane.rs:676` already asserts (10 to 12 per second) and terminal views are not re-rendered
- [ ] Given an entity read only inside a deferred menu under a cached view, when it notifies, then the menu updates on the next frame
- [ ] Given an entity that notifies on every frame, when it is read under a cached view, then the test fails with the offending view named, so a redraw loop cannot land unnoticed
- [ ] Given a cached sidebar or terminal view reused from the previous frame, when the accessibility tree is read, then the roles Paneflow declares are still present; if they are not, the finding is recorded with a link to zed-industries/zed#66690 and no workaround is added without Arthur's decision

---

### EP-004: Text follows the new text system

Tabs, wrapping and glyph placement behave the same on the three platforms wherever Paneflow owns the layout, and change only where the new GPUI is correct.

**Definition of Done:** tabs occupy 4-column stops in the code editor and the diff view on every platform, wrapped surfaces have regression tests for UAX #14 break points, and the terminal grid is proven not to drift.

**Zed references:**
- `zed/crates/gpui/src/text_system/line_wrapper.rs:545-546`: `pub(crate) fn break_indices(text: &str)` over `icu_segmenter::LineSegmenter::new_for_non_complex_scripts`.
- `zed/crates/gpui/src/text_system/line_wrapper.rs:852,1113`: `test_emergency_wrap_preserves_graphemes`, `test_wrap_line_break_opportunities`.
- `zed/crates/gpui_windows/src/direct_write.rs:797-808,1808`: color-layer bounds as a union; `tabs_preserve_native_advances_and_cursor`.
- `zed/crates/gpui_wgpu/src/cosmic_text_system.rs:819-820`: glyph position includes `x_offset` and `y_offset`.
- `zed/crates/editor/src/display_map/tab_map.rs:21,441,459,480`: `TabMap`, `expand_tabs`, the tab-stop rule, `collapse_tabs`.
- Upstream: `f0210f1621` (#64624), `0294cfb58e` (#66468), `4a6121bb0f` (#66588), `985bbf42e6` (#66799), `44ec5691b5` (#65728).

#### US-013: Tabs expand to columns in the code editor and the diff view
**Description:** As a developer reading tab-indented code, I want each tab to advance to the next 4-column stop so that indentation looks the same on Linux, macOS and Windows.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/editor/src/display_map/tab_map.rs:441-460`: `fn expand_tabs`, with `let tab_len = tab_size - ((expanded_chars_old - 1) % tab_size);`.
- `zed/crates/editor/src/display_map/tab_map.rs:480`: `collapse_tabs`, the reverse mapping from display columns to buffer columns.
- `zed/assets/settings/default.json:1716`: `"tab_size": 4`.
- `zed/crates/gpui_windows/src/direct_write.rs:1808` and `0294cfb58e` (#66468): after the bump a raw tab takes DirectWrite's native advance on Windows; "This does not introduce a four-space policy or attempt to unify tab stops across platforms."

**Paneflow sites:** `src-app/src/app/diff_dock/code/element.rs:380-432` (`ShapeCache::shape_in`), `:971-996` (caret and selection through `x_for_index`); `src-app/src/app/diff_dock/code/minimap.rs:58`; `src-app/src/app/diff_dock/code/edit.rs:502` (Tab inserts `"\t"`); `src-app/src/diff/element.rs:402,408,591,1641,1746`; `src-app/src/widgets/text_input.rs:914`.

**Acceptance Criteria:**
- [ ] Given a line with tabs in the code editor or the diff view, when it is shaped, then each tab is replaced by the spaces that reach the next multiple of 4 columns, following the rule at `tab_map.rs:459`
- [ ] Given the buffer, when a line is displayed, then the stored text is unchanged: a tab stays one character on disk, in the clipboard and in the diff
- [ ] Given a caret or selection after a tab, when its x position is computed, then it maps from the buffer offset to the expanded column and back, so the caret never rests inside a tab and a click in a tab's span lands on the nearer side
- [ ] Given the same line on Linux, macOS and Windows, when it is shaped, then a tab spans the same number of columns on the three platforms, asserted by a platform-independent test on the expansion
- [ ] Given a line of 10,000 tabs, when it is shaped, then expansion is linear in the line length and the frame completes
- [ ] Given a wide character (a CJK ideograph, an emoji) before a tab, when the line is shaped, then the tab stop is computed from display columns, with a test for a double-width character
- [ ] Given a search match or a word-diff range that spans a tab, when its rectangle is drawn, then it covers the expanded width (`src-app/src/diff/element.rs:1746`)

#### US-014: Wrapping surfaces hold under UAX #14
**Description:** As a user reading tooltips, settings descriptions and markdown, I want line breaks at sensible points so that paths, branch names and URLs wrap without stranded punctuation.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui/src/text_system/line_wrapper.rs:1113`: `test_wrap_line_break_opportunities`, with `"aaa foo/bar"` breaking after the slash, `"aaa (hello)"` and `"aaa required!"` keeping punctuation with the word, `"aaa 你好！"` keeping the closing mark.
- `zed/crates/gpui/src/text_system/line_wrapper.rs:852`: `test_emergency_wrap_preserves_graphemes` (`e\u{301}`, `👍🏽`, `🇯🇵`, `👩\u{200d}💻` never split).
- `zed/crates/gpui/src/platform/test/platform.rs:137`: tests use `NoopTextSystem`, so the wrap logic runs in `#[gpui::test]`.
- #64624 PR text: hyphenated words can break after the hyphen, on purpose.

**Paneflow sites:** tooltip at `src-app/src/ui_primitives.rs:662-685`; settings description at `src-app/src/settings/components.rs:331-335`; markdown at `src-app/src/markdown/view.rs:941,949`; dialog bodies inside `modal_card` (`src-app/src/settings/components.rs:882-902`); rule "rows never wrap" at `DESIGN.md:1673-1674`.

**Acceptance Criteria:**
- [ ] Given a tooltip, a settings description and a markdown paragraph containing `feat/gpui-upgrade`, `C:\dev\paneflow\src-app`, `https://example.com/a/b?x=1` and a hyphenated word, when each wraps at a width that forces a break, then a test asserts the break falls after `/`, `\` or `-` and never before `/` or `?`
- [ ] Given a row that `DESIGN.md:1673-1674` says never wraps, when its text is longer than the row, then it truncates with an ellipsis on one line as before
- [ ] Given a 200-character token with no break opportunity, when it wraps, then it breaks between grapheme clusters and never inside one
- [ ] Given text in Chinese or Japanese with closing punctuation, when it wraps, then the closing mark stays on the line of the character before it
- [ ] The story lists the dialogs and callouts whose bodies wrap, for the visual pass

#### US-015: Glyph placement holds for emoji, combining marks and scale factors
**Description:** As a terminal user, I want the grid to stay aligned whatever a cell contains so that color emoji, combining marks and fractional scale factors never shift a column.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui_windows/src/direct_write.rs:775-808`: emoji layers measured per layer, raster bounds as the union of the layers (`4a6121bb0f`, #66588).
- `zed/crates/gpui_wgpu/src/cosmic_text_system.rs:819-820`: `(glyph.x + glyph.x_offset * f32::from(font_size))`, `(glyph.y - glyph.y_offset * f32::from(font_size))` (`985bbf42e6`, #66799).
- `44ec5691b5` (#65728): on macOS a single-run line is laid out at the exact size; 152.71 px against 152.85 px at 11 px.

**Paneflow sites:** `src-app/src/terminal/element/paint/text.rs:58-98` (`CellAligner::glyph_x`), `:153-155,254-255`; `src-app/src/app/sidebar/agents_section.rs:3017-3035` (width-sensitive title fits); `src-app/src/terminal/element/golden/`.

**Acceptance Criteria:**
- [ ] Given a cell holding a base character with a combining mark, when its glyphs are positioned, then the first glyph of the cluster sits at `cell_width * column` and the mark keeps only its offset inside the cluster, asserted by a test on `CellAligner`
- [ ] Given a cell holding a color emoji wider than its base glyph, when it paints, then its origin is the cell origin and the following cell's origin is unchanged
- [ ] Given the terminal golden corpus, when the suite runs, then no golden file changes
- [ ] Given a title that fits its row by less than 0.5 px, when it is measured on macOS, then it does not flip between fitting and truncating between two frames
- [ ] The story lists, for the visual pass, color emoji on Windows at 100 %, 125 % and 150 % scale and combining marks on Linux

---

### EP-005: Platform integration

Window observers, surface setup, blur and menu placement use what the new GPUI provides on each platform.

**Definition of Done:** Linux window state survives fullscreen toggles and startup holds on Wayland and X11, the blurred backdrop is available where the compositor supports it, and submenus open on the side that has room.

**Zed references:**
- `zed/crates/gpui/src/window.rs:3365`: `pub fn set_client_corner_radii(&mut self, radii: Corners<Pixels>)`, "client-drawn corner radii so platform effects can match the window's shape"; called at `zed/crates/workspace/src/workspace.rs:12703`.
- `zed/crates/gpui_linux/src/linux/wayland/window.rs:113,2249`: `ExtBackgroundEffectSurfaceV1` and the Wayland `set_client_corner_radii`.
- `zed/crates/gpui/src/elements/anchored.rs:72`: `pub fn attach(mut self, target: Bounds<Pixels>, corner: Anchor) -> Self`.
- `zed/crates/gpui/src/app.rs:1610`, `zed/crates/gpui/src/platform.rs:2831`: `active_display`, `WindowBounds::centered(display_id, size, cx)`; used at `zed/crates/zed/src/zed.rs:1730,1739`.
- Upstream: `14b7b0ef37` (#53746), `e20d19d32f` (#66002), `ba06cc5191` (#65671), `ce6c5413f1` (#66496), `64ef8026c7` (#61459), `4d847859f5` (#64058).

#### US-016: Linux window observers and surface setup hold
**Description:** As a Linux user, I want window size, fullscreen and startup to behave as before so that the bump changes nothing I can see on Wayland or X11.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002

**Zed references:**
- `ba06cc5191` (#65671): a fullscreen change now fires the resize callback on Wayland and X11 (`zed/crates/gpui_linux/src/linux/wayland/window.rs`, `zed/crates/gpui_linux/src/linux/x11/window.rs`).
- `zed/crates/gpui_wgpu/src/wgpu_renderer.rs:490-499`: `if !surface_configured { surface.configure(&context.device, &surface_config); }`, the adapter test now uses the real configuration (`e20d19d32f`, #66002).
- `zed/crates/gpui_wgpu/src/wgpu_renderer.rs:1001`: `update_drawable_size`, resize still reconfigures.
- `bfcd5949ff` (#66774): Wayland frame callbacks timed from receipt.

**Paneflow sites:** `src-app/src/app/window.rs:282-292`; `src-app/src/app/detached_panes/window.rs:205-208`; `src-app/src/window_state.rs:91,115`; `scripts/bench-startup.sh`; `src-app/src/layout/render.rs:736,967` (`dirty_to_draw_duration`).

**Acceptance Criteria:**
- [ ] Given a window toggled to fullscreen and back on Linux, when the bounds observers fire, then the saved windowed size is unchanged, asserted by a test on the save path
- [ ] Given the bounds observer fires while fullscreen, when it runs, then it performs no file write and no layout rebuild beyond what a resize already does
- [ ] Given a release build on Linux x86_64, when time to first frame is measured with `scripts/bench-startup.sh` on both pins, then the two numbers are recorded and the new one is within 10 % of the baseline
- [ ] Given an adapter that is rejected by the new surface test, when the app starts, then it falls back to the next adapter or exits with the existing error path, and the log names the rejected adapter
- [ ] The story states which of Wayland, X11, NVIDIA and Mesa were exercised and which were reviewed by inspection only

#### US-017: Blurred backdrop on Linux through ext-background-effect
**Description:** As a Linux user on a compositor with background effects, I want the blurred backdrop setting honored so that Linux matches macOS and Windows where it can.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016

**Zed references:**
- `zed/crates/gpui_linux/src/linux/wayland/window.rs:21,113`: `ext_background_effect_surface_v1`, preferred over the KDE blur protocol, which stays as a fallback (`14b7b0ef37`, #53746).
- `zed/crates/gpui/src/window.rs:3365` and `zed/crates/workspace/src/workspace.rs:12703,12707`: `window.set_client_corner_radii(corner_radii)` under client decorations, `Corners::default()` under server decorations.
- `zed/crates/settings/src/content_into_gpui.rs:52` and `zed/crates/zed/src/main.rs:812`: `Blurred` mapped from settings and applied with `window.set_background_appearance(...)`.

**Paneflow sites:** `src-app/src/app/constants.rs:38-117` (Linux maps the backdrop to opaque or transparent); `src-app/src/window_chrome/linux_backdrop.rs:11-42`; `src-app/src/window_chrome/csd.rs:61-62` (`set_client_inset`).

**Acceptance Criteria:**
- [ ] Given Linux on a compositor that supports the protocol and the backdrop setting on "blurred", when the window opens, then it requests `WindowBackgroundAppearance::Blurred`
- [ ] Given client-side decorations, when the window's corner radius is set, then `set_client_corner_radii` is called with the same radius next to `set_client_inset`, and with zero radii under server decorations
- [ ] Given a compositor without the protocol and without KDE blur, when the setting is "blurred", then the window uses the translucent appearance it uses today and logs the fallback once
- [ ] Given X11, when the setting is "blurred", then behavior is unchanged from today
- [ ] Given macOS or Windows, when the setting is read, then the mapping at `src-app/src/app/constants.rs` is unchanged

#### US-018: Submenus and popovers attach on the side that has room
**Description:** As a user opening a submenu near a window edge, I want it placed from its real size so that it never overflows the window or covers its parent row.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui/src/elements/anchored.rs:72`: `attach(target, corner)`, flips to the other side of `target` when the element does not fit, and the offset flips with it.
- `zed/crates/ui/src/components/context_menu.rs:1915-1917`: `anchored().position_mode(AnchoredPositionMode::Local).attach(Bounds::new(Point::default(), trigger_size), ...)`.
- `zed/crates/ui/src/components/popover_menu.rs:383-385`: `.attach(child_bounds, self.resolved_attach()).offset(offset + offset)`.
- `ce6c5413f1` (#66496).

**Paneflow sites:** `src-app/src/ui_primitives/action_menu.rs:442-458,864-880` (`opens_left`, `attach_submenu`); `src-app/src/terminal/path_picker/render.rs:32,271-279`; `src-app/src/app/diff_dock/code/view/change_markers.rs:416`.

**Acceptance Criteria:**
- [ ] Given a submenu whose parent row is near the right edge, when it opens, then it is placed with `anchored().attach(...)` on the left of the row, decided from the submenu's measured size
- [ ] Given room on both sides, when a submenu opens, then it opens on the right, as today
- [ ] Given room on neither side, when a submenu opens, then it stays inside the window and its parent row remains visible
- [ ] Given the path picker, when it opens above or below its field, then its behavior is unchanged in this story
- [ ] The width estimate `opens_left` and the absolute positioning in `attach_submenu` are removed

---

### EP-006: Evidence and records

The bump is measured, re-baselined and written down where the repository keeps such records.

**Definition of Done:** baselines are committed for the new pin, the bump summary exists, and the changelog and design documents describe the UI as it now behaves.

**Zed references:**
- `zed/crates/gpui_platform/benches/primitives.rs` and the two-revision comparison script added by `26abd1c959` (#66316).
- `zed/crates/gpui/src/profiler/journal.rs:171`: `FrameSkipReason::AnimationFrameRateLimit`, the reason a throttled frame is skipped.
- `zed/crates/gpui/src/app.rs:1741`: `pub fn debug_state(&self) -> serde_json::Value`.

#### US-019: Re-baseline the benchmarks and write the bump summary
**Description:** As the maintainer, I want the terminal, editor and startup suites re-run and re-baselined on the new pin so that the next regression is measured against GPUI as it now is.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-009, US-011, US-013

**Zed references:**
- #66809, #66810, #66811 PR texts: the GPU and CPU cost changes expected from the renderer.
- `zed/crates/gpui/src/window.rs:1999-2019`: a throttled frame is deferred and recorded as skipped, which changes frame-timing traces.
- `zed/crates/gpui_platform/benches/primitives.rs:245`: upstream's primitive benchmark, usable to compare the two GPUI revisions directly.

**Paneflow sites:** `bench/README.md:12-17,33-53,76-89`; `bench/baselines/linux-x86_64/{terminal,editor,startup,persistent,terminal-alloc,editor-alloc}.json`; `bench/results/gpui-bump-37faef81-2026-10-28.md` (the model); `scripts/bench-terminal.sh`, `scripts/bench-editor.sh`, `scripts/bench-startup.sh`, `scripts/perf-gates.sh`.

**Acceptance Criteria:**
- [ ] Given the three suites on Linux x86_64, when they run on the old pin and on the final branch commit, then a results file under `bench/results/` holds the before and after tables in the layout of the 37faef81 summary
- [ ] Given each metric, when it is compared, then any metric more than 10 % above its baseline is listed with its cause and the story that addressed it or accepted it
- [ ] Given the allocation baselines, when they are compared, then each is within 1 % or its change is explained, and the refreshed baselines are committed on top of the code commit, never from a dirty tree
- [ ] Given `scripts/perf-gates.sh`, when it runs on the final commit, then it passes without any budget raised, or each raised budget is listed in the summary with the measured value and Arthur's approval
- [ ] Given the summary, when it is read, then it records the frame-interval behavior, the latency measurements of US-009 and US-010, and the path costs of US-003
- [ ] Given a benchmark run on a loaded machine, when two runs differ by more than 10 %, then the run is repeated and no baseline is set from it

#### US-020: Changelog, design and architecture documents, visual-pass checklist
**Description:** As the maintainer, I want the bump recorded in the changelog and the design documents so that users know what changed and the documents describe the code as it is.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-019

**Zed references:**
- Release notes carried by the upstream commits: `0294cfb58e` ("Fixed zero-width tabs in text on Windows"), `0373366f43` (M5 animations), `78f5d7446c` (Windows display-change crash), `4d847859f5` (Wayland activation), `f0210f1621` (UAX #14 wrapping).
- `zed/crates/gpui/src/path_builder.rs:387-410` and `zed/crates/gpui/src/window.rs:4911`: the API facts `DESIGN.md` and `ARCHITECTURE.md` must state.

**Paneflow sites:** `CHANGELOG.md` (`[Unreleased]`, the format of `:141-148`); `DESIGN.md:493-529,598-600,698-705,1662-1688`; `ARCHITECTURE.md:202-203,271-315`; `bench/README.md:80-89`; `.github/workflows/run_tests.yml:420` (stale revision `0b984b5` in a comment); `deny.toml:144-148`.

**Acceptance Criteria:**
- [ ] Given `CHANGELOG.md`, when `[Unreleased]` is read, then it holds user-facing entries for tab width in the editor and diff view, wrapping, the frame limit on unfocused windows, spinner behavior, and each inherited fix, in the bold-lead format already used
- [ ] Given `DESIGN.md:598-600`, when it is read, then it no longer describes a shape cache that translates paths, and it states how squircles are built and budgeted
- [ ] Given `DESIGN.md:493-510`, when it is read, then spinner timing names the shared app-wide phase and the reduced-motion rule
- [ ] Given `ARCHITECTURE.md`, when the render sections are read, then the frame policy (three intervals, every redraw limited) and the layer ordering rule are stated as the code implements them
- [ ] Given `bench/README.md` and `.github/workflows/run_tests.yml:420`, when they are read, then the bump is recorded and no comment names a GPUI revision that is not the pinned one
- [ ] Given the whole branch, when the checklist is written, then one list groups, per platform, every surface and state the stories flagged for Arthur's visual pass
- [ ] Given a document claims a behavior no story verified, when it is reviewed, then the claim is removed or marked unverified

## Functional Requirements

- FR-01: The system must resolve `gpui` and `gpui_platform` at `703b5ac3f4ffe1e87a073123ae56e22f72f095c4` from the four declarations in `src-app/Cargo.toml`, with a committed lockfile that builds under `--locked`.
- FR-02: The system must patch `async-task`, `async-process`, `calloop` and `khronos-egl` to the same repositories and revisions as `zed/Cargo.toml` at the pinned revision.
- FR-03: The system must NOT implement `IntoElement` by hand for a type that implements `Element`.
- FR-04: The system must build each path at its final position and must NOT read or mutate the geometry of a built `Path`.
- FR-05: When a fill and a border are painted as separate paths, the system must overlap them so that no edge is shared.
- FR-06: The system must set the inactive, low-power and thermal frame intervals explicitly on every window it opens, from one definition.
- FR-07: When a glyph rotates continuously, the system must animate it through `TimedTransformation` and must NOT re-render its view per frame.
- FR-08: When a line shown in the code editor or the diff view contains a tab, the system must display it as spaces up to the next 4-column stop and must NOT alter the stored text.
- FR-09: When `reduce_motion` is on, the system must render every spinner static.
- FR-10: The system must record, for each bump, a results file under `bench/results/` and refreshed baselines for the suites that changed.

## Non-Functional Requirements

- **Performance:** squircle building for a 12-badge tab strip at or below 0.4 ms per frame; every terminal, editor and startup metric within 10 % of its committed baseline or explained; allocation baselines within 1 %.
- **Latency:** keystroke-to-present within 2 ms of the old pin on Windows and Linux, within 4.2 ms on macOS at 120 Hz; an unfocused window presents at most 30 frames per second.
- **Reliability:** 0 panics on zero-sized bounds, on a failed path build, or on a rejected GPU adapter; the full test suite passes on the four CI legs with 0 tests quarantined for this work.
- **Accessibility:** `reduce_motion` stops 100 % of rotating glyphs; the accessibility roles declared on cached views remain present in the tree.
- **Compatibility:** Rust 1.98.0; Linux (Wayland and X11, x86_64 and aarch64), macOS on Apple Silicon, Windows 10 and 11 x64; 0 new system packages and 0 new licenses in `deny.toml`.
- **Memory:** a shape cache, if one exists, holds at most 256 entries.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Empty shape | Zero-sized or negative bounds handed to a squircle, ring or stroke | No path is built, nothing is painted, no panic | None |
| 2 | Path build failure | `PathBuilder::build` returns `Err` | That shape is skipped, the frame completes | None |
| 3 | Scale factor change | Window dragged to a display with another scale | Shapes rebuilt for the new scale on the next frame | None |
| 4 | Boundary line | A code line of 10,000 tabs or a 200-character unbreakable token | Linear-time expansion; wrap between grapheme clusters only | None |
| 5 | Low power | The OS reports power saving | Redraws limited to 30 frames per second | None |
| 6 | Unfocused window | A detached pane window without focus streams output | Redraws limited to 30 frames per second; full rate returns with focus | None |
| 7 | Reduced motion | `reduce_motion` on while an agent thinks | Spinners static, status still updates | None |
| 8 | Unsupported compositor | Linux "blurred" backdrop without the protocol | Falls back to today's translucent appearance, one log line | None |
| 9 | Rejected GPU adapter | An adapter fails the new surface test at startup | Next adapter is tried, or the existing startup error is shown | The existing startup error |
| 10 | Menu near an edge | Submenu opened with room on neither side | Stays inside the window, parent row visible | None |
| 11 | Upstream regression confirmed | macOS latency over threshold in US-010 | macOS release held, decision put to Arthur | None |
| 12 | Platform leg fails | A `cfg`-gated item breaks on a CI leg not compiled locally | Fixed before merge; the report states what was compiled where | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | macOS input latency regresses on 120 Hz displays (zed-industries/zed#66954, open) | Med | High | US-010 measures on hardware before release; the macOS release is held if the threshold is exceeded |
| 2 | Per-paint path building costs more than the frame allows | Med | High | US-003 measures first; US-004 carries a bounded origin-keyed cache as the fallback and a benchmark that guards the budget |
| 3 | Seams between adjacent paths on bordered surfaces | High | Med | US-006 removes coincident edges by construction; US-005 collapses multi-path shapes into one stroke |
| 4 | The renderer rework is three days old upstream and may receive fixes after `703b5ac3` | High | Med | Work stays on a branch; re-pinning to a later commit before merge is one manifest change plus the gates |
| 5 | macOS-only code fails to compile, unseen from Windows and Linux | Med | Med | US-002 requires the macOS CI legs green on the pull request commit |
| 6 | Tab expansion breaks caret, selection or search geometry in the editor | Med | Med | US-013 maps offsets both ways with tests for carets, clicks, wide characters and search ranges |
| 7 | New cached-view subscriptions cause extra full redraws | Low | Med | US-012 asserts render counts with a menu open and an agent thinking |
| 8 | Benchmarks drift from machine noise and a baseline is set from a bad run | Med | Med | Runs differing by more than 10 % are discarded; baselines are set only from a clean tree |
| 9 | The `feat/agents-browser` branch carries a patched GPUI that conflicts with the new renderer | High | Low | Out of scope; recorded in Non-Goals so the conflict is expected when that branch is rebased |

## Non-Goals

What this version explicitly does NOT include:

- **A user setting for frame rate or power behavior.** The policy is a constant in this version; a setting can follow once the latency measurements exist.
- **A configurable tab size.** Tabs are 4 columns wide; a per-user or per-language size is a later change to `paneflow-config`.
- **A redesign of the grid "Thinking" loader** (`src-app/src/app/sidebar/lane.rs:329-344`). It is a grid of dots with stepped opacity, which `TimedTransformation` does not express.
- **Rebasing `feat/agents-browser`** onto the new GPUI. That branch pins an older revision with its own GPUI patch (`import_shared_texture` is not upstream) and is handled with the browser work.
- **Windows and macOS benchmark baselines.** Baselines exist for Linux x86_64 only, and this work keeps that scope.
- **Adopting `App::debug_state`, `App::active_display` or `GraphicalEnvironment::desktop_environment`.** They are opportunities recorded in the research note, not part of aligning the UI.
- **Any change to the terminal engine, the git layer or the IPC.**

## Files NOT to Modify

- `native/libghostty/**`: the terminal engine archives and generated bindings; unrelated to GPUI.
- `vendor/**`: vendored crates keep their upstream text.
- `crates/paneflow-shim/**`, `crates/paneflow-ai-hook/**`, `crates/paneflow-mcp/**`: size-capped helpers that do not depend on GPUI.
- `src-app/src/git_command.rs`: the single git boundary.
- `src-app/src/terminal/pty_session/session_backend.rs` and `src-app/src/terminal/types.rs`: the engine boundary and its neutral types.
- `rust-toolchain.toml`: the toolchain stays at 1.98.0.
- `crates/paneflow-host/tests/persistent_baseline/gates.rs`: perf budgets are not raised to make the bump pass; a change needs Arthur's approval and an entry in the bump summary.
- `C:\dev\zed`: the reference checkout is read-only.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** should squircles stay paths at all on the hot path? Recommended: keep paths and measure (US-003). Alternative if the budget fails twice: corners rasterized once as sprites in GPUI's atlas, which cost nothing per frame but need one sprite per radius and scale. Engineering to confirm after the spike.
- **Data Model:** the code editor needs a mapping between buffer offsets and display columns for tab expansion. A per-line expansion table (as `TabMap` computes per row) versus a prefix-sum cached with the shaped line in `ShapeCache`. Trade-off: the cached table costs memory per visible row and saves a scan on every caret query.
- **API Design:** where should the frame-interval definition live so both windows share it? Recommended: next to the other window constants in `src-app/src/app/constants.rs`. No new IPC or config surface.
- **Dependencies:** the lockfile gains `icu_segmenter`, `icu_segmenter_data`, `icu_locale_fallback` and, through an optional GPUI dependency, `bench_metrics` with `criterion`; it loses `lyon`, `lyon_algorithms`, `lyon_tessellation` and `float_next_after`. `cargo deny` reported licenses and advisories clean in the research probe. Should the optional benchmark chain be excluded from the lockfile review, or reviewed like any other source?
- **Migration:** rollback is one revert of the branch; no user data or configuration format changes. If macOS latency fails US-010, is a vendored `gpui_macos` patch acceptable given that the fork was dropped on 2026-08-26 (`e333c2397`), or does macOS stay on the previous release until upstream fixes it?

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| GPUI commits behind upstream `main` | 65 (pin `37faef81`) | 0 at merge | Month-1 | `git rev-list --count` over the GPUI crates in `C:\dev\zed` |
| Compile errors against current GPUI | 35 binary, 54 test | 0 | Month-1 | `cargo check --workspace --all-targets` |
| Tests passing on the Windows leg | 4544 run on the old pin | 100 % of the suite, 0 quarantined for this work | Month-1 | `cargo nextest run --profile ci` |
| Squircle build cost, 12-badge strip | N/A (translated cache) | at or below 0.4 ms per frame | Month-1 | The benchmark case added by US-004 |
| Spinner call sites that re-render a view per frame | 2 | 0 | Month-1 | Render-count test of US-011 |
| Tab width in the editor on Windows | 0 columns | 4-column stops, same on three platforms | Month-1 | Expansion test of US-013 |
| macOS keystroke-to-present at 120 Hz | Measured on the old pin by US-010 | within 4.2 ms of baseline | Month-1 | Hardware measurement recorded in the bump summary |
| Days from upstream commit to pinned | 19 (2026-10-27 to 2026-11-15) | 14 or fewer for the next bump | Month-6 | Dates in `bench/results/` summaries |

## Open Questions

- **Re-pin before merge?** If upstream fixes zed-industries/zed#66954 or follows up on the path renderer before this branch merges, should the pin move to that commit instead of `703b5ac3`? Arthur, at the start of EP-006; decides whether US-002 and US-019 run once or twice.
- **Low-power frame limit.** The PRD keeps GPUI's 30 fps limit in power-saving mode, which also limits keystroke echo there. If US-009 or US-010 shows a perceptible cost, should `low_power_frame_interval` become `None`? Arthur, after US-009; changes one constant and one changelog entry.
- **Tab width.** Is a fixed 4 columns acceptable until a setting exists, including for Go and Makefiles where 8 is conventional? Arthur, before US-013.
- **macOS hardware.** Who runs US-010, and on which Mac and display? Arthur, before EP-003 is certified; without it the macOS release of the bump is held.
- **Phase of spinners.** All spinners now share one app-wide phase. Is that acceptable against `DESIGN.md:493-510`, or must each loader start at its own zero? Arthur, at US-011.
- **Accessibility of cached views.** If zed-industries/zed#66690 reproduces in Paneflow, is it accepted as an upstream defect or worked around by disabling the cache on affected views? Arthur, at US-012.
[/PRD]
