[PRD]
# PRD: Move Paneflow to GPUI at Zed `703b5ac3` and bring the UI in line with it

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-15 | Arthur Jean | Initial PRD: move the GPUI pin from `37faef81` to `703b5ac3`, migrate every broken API, and re-align paths, layers, frame pacing, animation, text and platform behavior with the new GPUI as Zed specifies and uses it. 6 epics, 20 stories. |
| 1.1 | 2026-11-15 | Arthur Jean | CI moves to one final epic. EP-001 to EP-006 are validated locally, by code, on the Windows host and in WSL Linux, and never wait on CI. The new EP-007 owns every CI result and fixes everything until the whole CI is green before merge. US-002 is rescoped to the local proof. 7 epics, 23 stories. |
| 1.2 | 2026-11-15 | Arthur Jean | No macOS hardware is available before 2027, so macOS is validated by CI only and trusted to upstream code. US-010 is cancelled; the macOS latency target, assumption, risk and open question are rewritten; nothing holds the macOS build. |
| 1.3 | 2026-11-15 | Arthur Jean | Linux gets its own validation epic before CI. Every check that needs a real Linux desktop session, and the Linux visual pass, moves to the new EP-007 on Arthur's Fedora machine, so EP-001 to EP-006 iterate on the Windows host with WSL clippy and tests only. The CI epic becomes EP-008. US-024 to US-026 are added; the Linux latency of US-009 moves to US-024, and the real-session checks of US-016 and US-017 move to US-025. 8 epics, 26 stories. |
| 1.4 | 2026-11-16 | Arthur Jean | US-014 follows UAX #14 for Windows paths: GPUI never breaks between `\` and a letter (LB24), so a path breaks after `C:` or a `-`, and Paneflow inserts no break points of its own. |
| 1.5 | 2026-11-16 | Arthur Jean | The Linux benchmark runs move to EP-007, where they run on Fedora on the dual boot machine instead of under WSL2. US-019 keeps the bump summary; the new US-027 runs the terminal, editor and startup suites on both pins, re-baselines them, runs `scripts/perf-gates.sh` and fills the summary's Linux tables. 8 epics, 27 stories. |

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

The work happens on one branch and lands as one pull request series. EP-001 to EP-006 are validated locally, by code: tests, lints and gates run on the Windows host and in WSL Linux after each story, and no story before EP-008 pushes or waits on CI. EP-007 then runs, on Arthur's Fedora machine, every Linux check that needs a real desktop session, the Linux benchmark runs and re-baseline, and the Linux visual pass. EP-008, the last epic, owns CI: it pushes the branch, reads every workflow run, and fixes every failure until the whole CI is green on the commit that merges. EP-001 moves the pin to `703b5ac3`, mirrors Zed's `[patch.crates-io]` change for `async-task`, and migrates every broken call site so that the workspace builds, lints and tests on Linux, macOS and Windows. The squircle primitives stop translating cached shapes and build each shape at its final position, which is what Zed's own callers do (`zed/crates/ui/src/components/progress/circular_progress.rs:98`, `zed/crates/ui/src/components/divider.rs:105`).

EP-002 to EP-005 then bring behavior in line with the new GPUI, one domain per epic: paths and layers, frame pacing and animation, text, platform integration. Each story names the Zed definition, the Zed test or example that specifies the behavior, and where Zed's own application code uses it, so the implementer ports a pattern instead of inventing one. EP-006 writes the bump summary on the model of `bench/results/gpui-bump-37faef81-2026-10-28.md` and updates `CHANGELOG.md`, `DESIGN.md` and `ARCHITECTURE.md`; EP-007 runs and re-baselines the Linux benchmarks on Fedora and completes the summary.

Key decisions, each with its evidence:
- **One pin move to `703b5ac3`, not two.** A staged move through `78f5d7446c` (the last commit before the renderer rework) would isolate risk but doubles the benchmark re-baseline and the visual pass, and was never compiled. The branch absorbs the risk instead: nothing merges before EP-001 to EP-003 and EP-006 to EP-008 are certified.
- **Local validation per epic, CI once at the end.** A CI round trip costs 30 to 45 minutes, and waiting on it after each epic stalls the work. Each story is proven locally instead (Windows natively, Linux x86_64 by clippy and tests in WSL), EP-007 validates Linux on a real Fedora session in one pass, and EP-008 drives the CI to green in one long pass at the end. The cost is that macOS-only code is reviewed by inspection until EP-008, so its failures surface late; EP-008 is sized for that.
- **No translated shape cache.** `Path` cannot be moved or inspected, `PathBuilder::build` no longer tessellates, and binning is not cacheable from outside GPUI. A benchmark budget replaces the build-count tests; a bounded origin-keyed cache is the fallback only if the budget fails.
- **GPUI's default frame intervals, written explicitly.** Zed sets none of the three options (no match outside `zed/crates/gpui`), so the defaults are the specification: 33.333 ms inactive, 33.333 ms in low power, 16.667 ms under thermal pressure (`platform.rs:2850-2852`). This also keeps the promise already published in `CHANGELOG.md:141`.
- **Tabs expand to columns before shaping, 4 columns wide.** Zed's editor never hands a tab to the shaper: `TabMap` expands it with `tab_size - ((expanded_chars_old - 1) % tab_size)` (`zed/crates/editor/src/display_map/tab_map.rs:459`), default `"tab_size": 4` (`zed/assets/settings/default.json:1716`). Expanding gives identical columns on the three platforms.
- **macOS is validated by CI only.** No Mac is available before 2027, so the macOS build is trusted to the upstream code: EP-008 compiles, lints and tests it on the `macos-15` runners, and no hardware measurement gates it. Upstream issue zed-industries/zed#66954 (open, untriaged) reports about 18 ms more key-to-screen latency on 120 Hz displays after `0373366f43`, and the new display link is unconditional on macOS 14 and later (`zed/crates/gpui_macos/src/window.rs:1160`); it is accepted as a known risk, tracked upstream, and fixed by a later GPUI bump if confirmed. The hardware measurement is redone once Arthur has a Mac.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Paneflow builds on current GPUI | Pin at `703b5ac3`; `run_tests` green on the Linux x86_64, Linux aarch64, macOS aarch64 and Windows x86_64 legs | Each later GPUI bump lands within 14 days of the upstream commit it targets |
| No performance regression ships unexplained | Every metric of the terminal, editor and startup suites within 10 % of its committed baseline, or its delta explained in the bump summary | Allocation baselines within 1 % for 3 consecutive bumps |
| Path cost stays inside the frame | Squircle building for a 12-badge tab strip at or below 0.4 ms per frame (5 % of an 8.33 ms frame) | Same at 24 badges |
| Input latency holds | Keystroke latency on Windows and Linux within 2 ms of the old pin; macOS not measured until a Mac is available | Same after the next bump; macOS at 120 Hz within 4.2 ms once measured |
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
- macOS key-to-present latency does not regress beyond 4.2 ms at 120 Hz. Contradicted by an open upstream report; not validated: no Mac is available before 2027, so it is accepted on trust in the upstream code (US-010 cancelled).
- No `cfg(target_os = "macos")` code in Paneflow uses a broken API. Based on greps, never compiled; validated by US-002 through CI.
- The Linux rounded-quad fallback for translucent squircles existed because paths went through an intermediate MSAA texture. Inferred, no document states the reason; validated by US-008 on 2026-11-15 (correct blending on GNOME and KDE, Wayland and X11, NVIDIA and Mesa AMD), and the fallback is removed.

### Hard Constraints
- Every change works on Linux (Wayland and X11), macOS and Windows; `cfg(windows)` items are linted only by the Windows CI leg, and any item added to a file with a `mod tests` goes before that module (`AGENTS.md`).
- Rust toolchain stays at 1.98.0 (`rust-toolchain.toml`); the migration compiled on it.
- Source code carries no comments; intent goes into names, types and tests (`AGENTS.md`).
- The render thread never blocks; no synchronous I/O is added to paint paths.
- Benchmarks compare only against their own platform's baseline; baselines exist for `linux-x86_64` only (`bench/README.md:76-78`), and `--set-baseline` refuses a dirty tree. No `--profile <run-name>`.
- Visual verification is Arthur's: no story launches or screenshots the installed app. A story delivers the change and the list of surfaces to look at.
- No AI attribution in commits; commits follow `type(scope): US-NNN - description`.

## Quality Gates

Every story in EP-001 to EP-006 is validated locally, by code, after its last change. Nothing in these epics is pushed for validation and no story waits on a CI result: CI belongs to EP-008, and every check that needs a real Linux desktop session belongs to EP-007. These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints on the host target
- `cargo nextest run --workspace --locked --profile ci` - the test suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget, when a test changes
- `cargo deny check advisories licenses sources` - when `Cargo.toml`, `Cargo.lock` or `deny.toml` changes

On the Windows host, the Linux leg is proven locally by running the clippy command, and the tests the story touches, in WSL on a clone of the exact local state, with a private `CARGO_TARGET_DIR` that is deleted afterwards. macOS code cannot be compiled locally: each story reviews its `cfg(target_os = "macos")` items by inspection and says so, and EP-008 compiles them. A criterion that needs a real Linux desktop session is not run under WSLg: it lives in EP-007.

For UI stories, additional gates:
- The `DESIGN.md` UI delivery gate (section 9, `DESIGN.md:1662-1678`): Dark, Light and one other preset; `reduce_motion` on and off; the 800 by 500 minimum window; every control state.
- The story lists the surfaces and states it changed for Arthur's manual visual pass. The implementing agent does not launch the installed app.

## Epics & User Stories

### EP-001: The pin moves and every target builds

Paneflow resolves GPUI at `703b5ac3`, with the same forks Zed builds against, and compiles, lints and tests on the Windows host and in WSL Linux. macOS is reviewed by inspection; EP-008 proves it in CI.

**Definition of Done:** the four `rev` values name `703b5ac3`, `Cargo.lock` is committed, and the quality gates pass on the Windows host and in WSL Linux x86_64 on the final local state.

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
- [x] Given `src-app/Cargo.toml`, when the four `rev` values are read, then each equals `703b5ac3f4ffe1e87a073123ae56e22f72f095c4`, and the committed `Cargo.lock` resolves under `--locked`
- [x] Given the root `Cargo.toml`, when the `async-task` patch is read, then its URL and revision equal Zed's at `zed/Cargo.toml:1046`, and the `async-process`, `calloop` and `khronos-egl` revisions still equal Zed's
- [x] Given `deny.toml`, when `allow-git` is read, then it lists `https://github.com/smol-rs/async-task`, no longer lists `https://github.com/zed-industries/async-task.git`, and the note above it names this bump
- [x] Given a type that implements `Element`, when the source is searched, then no hand-written `impl IntoElement` exists for it, and `SelectMenu` (`src-app/src/settings/components.rs:1323`), which does not implement `Element`, keeps its impl
- [x] Given any use of a GPUI path, when the source is searched, then the type is written `Path` with no generic argument and no code reads a field of `Path`
- [x] Given the five `paint_layer` calls, when they compile, then each passes only a closure, and the terminal still opens its background layer before its text layer
- [x] Given a squircle request, when it is painted, then the shape is built at its final bounds, `translated` and the vertex-offset cache are gone, and the Surface and Classic styles still paint quads without building a path (`src-app/src/ui_primitives.rs:1394`)
- [x] Given the tests at `src-app/src/ui_primitives/squircle.rs:394,428,451,484` and `src-app/src/pane.rs:2324,2347`, when the suite runs, then they are replaced by tests on observable behavior (which requests build a path, which paint quads, one build per painted shape) and none reads path geometry
- [x] Given zero-sized or negative bounds, when a squircle is requested, then no path is built and nothing panics
- [x] Given `PathBuilder::build` returns an error, when a shape is painted, then that shape is skipped and the frame still completes (proven by refusing non-finite or empty geometry before `PathBuilder` runs: in debug builds lyon's `debug_assert` panics on a non-finite point before GPUI can return its `Err`, so that `Err` is not reachable from a test)
- [x] No lint is silenced with `#[allow]` to make the migration compile, and no comment is added to source code

#### US-002: Prove the Linux and Windows legs locally
**Description:** As the maintainer, I want the migrated state proven on the two platforms I can compile so that later epics build on a known-good base, and only macOS waits for EP-008.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Zed references:**
- `zed/crates/gpui_windows/build.rs` (shader list changed by `ffe0fa29e4` and `4a6121bb0f`; still compiled with fxc, no new SDK requirement).
- `zed/crates/gpui_apple/src/display_link.rs:60-62`: `AnyClass::get(c"CAMetalDisplayLink").is_some()`, a runtime check, so the minimum macOS version does not rise.
- `zed/crates/gpui_linux/src/linux/wayland/window.rs:21`: `ext_background_effect_surface_v1` comes from the `wayland-protocols` crate, no new system library.

**Paneflow sites:** `scripts/check-helper-sizes.sh`; `scripts/perf-gates.sh`; `bench/baselines/linux-x86_64/{terminal,editor}-alloc.json`.

**Acceptance Criteria:**
- [x] Given the Windows host, when the quality gates run on the final local state, then fmt, clippy, the full nextest suite, the wall-clock check and `cargo deny` pass
- [x] Given a WSL clone of the exact local state, when the Linux clippy command runs, then it passes, and the tests changed by US-001 pass on Linux
- [x] Given the Linux x86_64 build, when `scripts/check-helper-sizes.sh` runs, then the three helper binaries stay under their hard caps
- [x] Given the `cfg(target_os = "macos")`, `cfg(unix)` and `cfg(windows)` items, when the story is reported, then it states per platform whether the code was verified by compilation or by inspection
- [x] Given an allocation counter moves on the bump, when it is read, then the allocation baselines are refreshed from a clean clone of the committed code, each changed counter is explained, and no budget in `crates/paneflow-host/tests/persistent_baseline/gates.rs` is raised

**Allocation evidence (review, 2026-11-15):** both baselines were refreshed at `988bd78f8b9c` with `git_dirty` false, and `gates.rs` is unchanged. Every changed counter went down, and all of them come from one upstream change, `ddb90e5e89` (#66682): `WindowTextSystem::layout_line` and `shape_line` no longer copy text longer than 23 bytes into a new `SharedString` on a line-layout cache hit. The editor's 60-row shaping saves one 120-byte copy per row: `shape_warm_60_rows` goes from 60 allocations and 7200 B per iteration to 0, and `shape_cold_60_rows` drops by the same 60 allocations and 7200 B. The terminal's text batch saves one 40-byte copy per frame: every `layout_*_220x60` counter drops by 1 allocation and 40 B, and `layout_echo_shifted_uncached_220x60`, which shapes two such batches, by 2 and 80 B. No other counter moved.

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
- [x] Given a release build on the Linux x86_64 baseline machine class, when the measurement runs, then it reports the time to build and paint the paths of a 12-badge tab strip (48 shapes), a 200 by 50 terminal screen with a curly underline on every row, and a 200 by 50 screen of box-drawing arcs and diagonals
- [x] Given the three results, when they are written, then they sit in a results file under `bench/results/` with the commit, the machine and the command, and each number is backed by the benchmark harness output
- [x] Given the squircle result, when it is compared to 0.4 ms per frame, then the verdict (within or over budget) is stated and decides which branch of US-004 applies
- [x] Given the two terminal results, when they are compared to the committed terminal baseline, then any metric more than 10 % above baseline is named with the mitigation US-004 or US-019 must carry
- [x] Given two consecutive runs of the same case differ by more than 10 %, when the results are read, then the run is discarded and repeated on an idle machine, and no number from it is recorded
- [x] No throwaway Cargo profile is created; an isolated build uses `CARGO_TARGET_DIR` outside the repository and is deleted afterwards

**Review evidence (2026-11-15):** results in `bench/results/gpui-703b5ac3-paths-2026-11-15.md`, measured under WSL2 on the CPU of the `linux-x86_64` baseline machine, not its Fedora session. The 10 % spread rule is applied to the medians, the statistic the harness compares; the badge p95 moved from 188.5 to 267.0 us between the two runs, a WSL tail that leaves the verdict unchanged. The two stroke metrics above 10 % (`paths_curly_underline_200x50`, `paths_box_drawing_200x50`) are carried by US-019 under its second criterion: it lists them with their cause and either the story that ships the sprite-atlas mitigation or Arthur's acceptance of the cost. That choice is Arthur's.

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
- [x] Given the benchmark suites, when they run, then a case measures squircle building for a 12-badge tab strip and its baseline is committed under `bench/baselines/linux-x86_64/`
- [x] Given US-003 found the cost at or below 0.4 ms, when the code is read, then no shape cache exists and shapes are built at their final bounds
- [x] Given US-003 found the cost above 0.4 ms, when the code is read, then a cache keyed by shape, bounds origin and scale factor holds at most 256 entries, evicts the least recently used entry, and the benchmark case is at or below 0.4 ms
- [x] Given the window moves to a display with another scale factor, when the next frame paints, then every shape is built for the new scale and none built for the old scale is painted
- [x] Given 1,000 distinct origins in one scroll, when the frames paint, then the cache, if present, never exceeds 256 entries and never clears wholesale

**Review evidence (2026-11-15):** US-003 found 0.16 ms, so no cache exists. The case is `paths_tab_strip_12_badges` in `terminal/perf_bench.rs`; its allocation baseline is in `bench/baselines/linux-x86_64/terminal-alloc.json` and gated in CI by `scripts/perf-gates.sh`. Its timing row joins `terminal.json` when US-019 re-baselines on Fedora.

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
- [x] Given an open gauge, when it paints, then its value arc is one stroke with `LineCap::Round` and the two cap discs are gone, so a gauge paints at most two paths (track and value)
- [x] Given a full gauge (`sweep.closed`), when it paints, then it paints a closed circle stroke as today
- [x] Given the check mark in the clone bubble, when it paints, then it is one stroke with round caps and round joins
- [x] Given a gauge whose radius is at or below half its stroke, when it paints, then nothing is painted and nothing panics
- [x] Given a gauge value of 0 or a non-finite value, when it paints, then only the track paints

**Visual pass (Arthur):** the usage rings (sidebar capsule, Settings Usage, concentric rings) at values between 0 and 1, full, and 0; the clone bubble check mark through its draw animation.

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
- [x] Given a squircle with a fill and an opaque border, when both paint, then the fill extends under the border by half the border width, so no edge of the border coincides with an edge of the fill; given a translucent border, the fill reaches the outer edge, so the border composites over the fill across its whole width (amended 2026-11-15 by Arthur's decision, see the review fix below)
- [x] Given the pane badge layers at `src-app/src/pane.rs:539-555`, when they paint, then each upper layer overlaps the layer below by at least 0.5 device pixel along shared contours
- [x] Given a translucent border color, when it paints over the extended fill, then the visible fill color inside the border is unchanged from the opaque case, with a test on the bounds handed to the builder
- [x] Given a border width of 0 or bounds smaller than the border width, when the shape paints, then only the fill paints and nothing panics
- [x] The story lists every bordered surface it changed (pane card, dialogs, tab icon cards, terminal search, tooltip, settings controls) for the visual pass

**Visual pass (Arthur):** pane cards (squircle and morph frames) and the 2 px drop veil; dialogs (`modal_card`); menus and select popups (`menu_surface_filled`); the quick pick shell and the path picker; the terminal search overlay; tooltips; the settings input surface and its focus ring; the clone bubble pill; the tab badges in the pane strip and in Settings Appearance, whose highlight is now a single band clear of the rim. Each in Dark, Light and one other preset, at scale 1 and 2.

**Review fix (2026-11-15):** the badge highlight band traced its two contours in the same direction, so along its shared sides and bottom corners the per-pixel winding summed to twice the edge coverage and the even-odd rule painted a white fringe at up to 0.22 alpha. `trace_band` in `ui_primitives/squircle.rs` now traces the inner contour mirrored, so the shared edges cancel; `a_band_traces_its_inner_contour_against_its_outer_one_so_shared_edges_cancel` asserts the opposite orientations.

**Visual pass (2026-11-15, Windows 11, scale 1, debug build on an isolated home):** Paneflow Dark, Paneflow Light and Claude Dark in Rounded, Paneflow Dark and Light in Classic, reduce motion on, the 800 by 500 window; pane cards, tab badges, usage ring and bubble, tooltip, Files menu, command palette, terminal search, settings select, raised button, input field with its focus ring, toast, clone bubble check, detached pane window, drop veil. An A/B against the same build without this story's geometry measured the badge fringe the review fix removed (bottom corners at 0x58 green against 0x27 to 0x2c now), menus identical, 1 px card borders within one level. One visible change remains: the translucent 2 px drop veil border reads in two tones (0x52 inside, 0x43 outside, uniform 0x52 before), because its outer half now lies over the background. Accepting it or treating translucent borders differently is Arthur's decision. Scale 2 was not checked: both displays run at scale 1.

**Review fix, translucent borders (2026-11-15, Arthur's decision):** a fill stopped at mid-border leaves the outer half of a translucent border over the background, so the border reads in two tones along its whole length; a fill reaching the outer edge only leaves a faint halo on the anti-aliased pixels of curves, at most a quarter of the fill to background difference. `fill_depth_under_border` in `ui_primitives/squircle.rs` now keeps the fill under an opaque border to half its width and lets it reach the outer edge under a translucent one. `squircle_fill_beneath_border` takes the border color, `CardShape::background` takes the card border, and a dock card without a border (in the title band) keeps a whole fill. Tests: `a_fill_beneath_a_translucent_border_reaches_the_outer_edge`, `a_bordered_card_hides_its_fill_edge_under_an_opaque_border_only`, `a_card_fill_stops_under_an_opaque_border_and_stays_whole_otherwise`.

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
- [x] Given each shadow site, when its values are read, then they equal the values `DESIGN.md` states for that surface, and no site compensates in code for the new approximation
- [x] Given the terminal element, when a frame paints, then backgrounds are painted in the first layer, glyphs and box-drawing paths in the second, and decorations, cursor and Kitty images outside both, with a test asserting the call order of the paint phases
- [x] Given `ARCHITECTURE.md:293-307`, when it is read after the story, then it states the ordering rule as GPUI now defines it (kind, then paint order, a nested layer above its parent) with no claim the code no longer relies on
- [x] Given a Kitty image with a negative z-index and one with a non-negative z-index on the same cell, when the frame paints, then the first is painted before the text layer and the second after it
- [x] The story lists the shadow and gradient surfaces for the visual pass, naming Linux as the platform where gradients gain dithering

**Review fix (2026-11-15):** the style table of `DESIGN.md` 4.9 gave menus and select popups the toast shadow, which contradicted the new shadow inventory and the code (`panel_shadow`); it now sends them to the inventory. The paint order test asserts the cursor last instead of only when present. Visual pass (Arthur): the shadow and gradient surfaces of the `DESIGN.md` shadow inventory, gradients on Linux first.

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
- [x] Given a translucent window on Wayland and on X11, when a translucent squircle paints as a path, then the result is recorded (correct blending, or the defect observed) with the compositor and driver named
- [x] Given the path blends correctly on both, when the code is read, then the rounded-quad fallback and `translucent_over_transparent_window` are removed
- [x] Given the path does not blend correctly on either, when the code is read, then the fallback stays and the reason is recorded in `DESIGN.md` next to the squircle section (not applicable: the path blends correctly on every session checked)
- [x] Given the check cannot be run on a real Wayland or X11 session, when the story is reported, then it says so and the fallback stays untouched (not applicable: the check was run on real sessions)

**Review evidence (2026-11-15):** this Windows host has no real Wayland or X11 session with a translucent backdrop (WSLg is not one), so the check was Arthur's manual pass on Fedora. He ran it on GNOME (Mutter) and KDE Plasma (KWin), each on Wayland and on X11, with the proprietary NVIDIA driver and with Mesa AMD (RADV), and reported correct blending on every combination. GPUI 703b5ac3 draws paths in the main render pass, so the intermediate MSAA texture that motivated the fallback is gone. `translucent_over_transparent_window` and its three rounded-quad branches in `ui_primitives/squircle.rs` (`squircle_corners_fill`, `squircle_fill_blend`, `paint_fill_beneath_border`) are removed; `linux_backdrop::translucent_window_active` stays because `app/constants.rs` still reads it for the shell material.

---

### EP-003: Frame pacing and animation follow the new scheduler

Paneflow states its frame policy, keeps its input latency, and animates rotating glyphs in GPUI's retained scene.

**Definition of Done:** both windows set the three frame intervals explicitly, Windows and Linux latency is measured, and no spinner rebuilds a view per frame. macOS latency is not measured (US-010 cancelled in PRD 1.2).

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
- [x] Given the main window and a detached pane window, when their `WindowOptions` are built, then both set `inactive_frame_interval`, `low_power_frame_interval` and `thermal_frame_interval` from one shared definition, with the values 33.333 ms, 33.333 ms and 16.667 ms
- [x] Given the shared definition, when a test reads it, then it asserts the three values, so a change of policy is a deliberate edit
- [x] Given a debug build with `PANEFLOW_LATENCY_PROBE=1` on Windows, when keystroke latency is measured on the old pin and on the new pin with the window focused, then the two results are recorded and differ by at most 2 ms (Linux moved to US-024 in PRD 1.3)
- [x] Given an unfocused window streaming terminal output, when it redraws, then it presents at most 30 frames per second, and the behavior is recorded in the bump summary as a user-visible change
- [x] Given the latency difference exceeds 2 ms, when the result is read, then the story is not marked done and the cause is investigated before any interval is changed (not applicable: the Windows difference is 0.58 ms at the median)

**Implementation evidence (2026-11-15):** `src-app/src/frame_policy.rs` defines `FRAME_POLICY` (33.333, 33.333 and 16.667 ms) and `paced_window_options`, which `src-app/src/launch.rs` (main window) and `src-app/src/app/detached_panes/window.rs` (detached pane window) use as the base of their `WindowOptions`. Three tests pin it: `the_policy_caps_unfocused_and_low_power_windows_at_thirty_fps_and_thermal_pressure_at_sixty`, `paced_window_options_carry_the_policy_and_leave_every_other_option_at_its_default`, and `every_paneflow_window_is_opened_with_the_paced_options`, which fails when an `open_window` call skips the policy. Windows was measured on real hardware (Ryzen 7 7800X3D, 240 Hz, debug builds of both pins, 200 keystrokes per run, `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`): focused `key-handler→paint-cpu` median 4.99 ms on `37faef81` against 5.57 ms on `703b5ac3` (p95 7.10 against 7.67 ms); an unfocused streaming window presents 68 to 70 frames per second on the old pin and 28.3 (never more than 30 in a second) on the new one, recorded in that file and in `CHANGELOG.md` as a user-visible change. The Linux measurement moved to US-024 (PRD 1.3); its procedure is in the same file.

**Review evidence (2026-11-15):** both `cx.open_window` calls (`launch.rs:416`, `detached_panes/window.rs:70`) take `..crate::frame_policy::paced_window_options()`, and they are the only two in `src-app/src`. The review widened `every_paneflow_window_is_opened_with_the_paced_options` from those two files to every `.rs` file under `src-app/src`, so the guard `ARCHITECTURE.md` describes now holds for a window opened anywhere in the app. The "recorded in the bump summary" part of the fourth criterion lands with the summary itself: US-019's first criterion requires it, and the numbers are in `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md` and `CHANGELOG.md` now. The `CHANGELOG.md` entry now states what the probe measures (keystroke to end of paint, not to the screen).

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
- [x] Given no such hardware is available to the implementing agent, when the story is reported, then it is left for manual verification with the exact procedure written down, and it is not marked done
- [ ] Given the measurement, when startup is also traced, then time to first frame on macOS is recorded on both pins

**Status: CANCELLED (PRD 1.2, 2026-11-15).** No Mac is available before 2027. macOS is validated by CI only (EP-008) and the latency risk of zed-industries/zed#66954 is accepted on trust in the upstream code. The procedure for a later hardware run stays in `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`.

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
- [x] Given `comet_spinner`, when it renders, then it uses `with_timed_transformation` with a 700 ms rotation and no `with_animation` closure
- [x] Given the empty-state spinner, when it animates, then it uses `with_timed_transformation` with a 1 s rotation
- [x] Given a view that shows a spinner and nothing else changing, when 60 frames pass, then the view's render count stays at 1, asserted by a test
- [x] Given `reduce_motion` is on, when a spinner renders, then it is static, as `DESIGN.md:507` requires
- [x] Given two spinners on screen, when they animate, then they share one phase, and `DESIGN.md` records that the phase is app-wide instead of per element
- [x] The grid loader at `src-app/src/app/sidebar/lane.rs:329-344` is unchanged

**Implementation evidence (2026-11-15):** `ui_primitives::spin` applies `TimedTransformation::rotation(turn, Transformation::default())` through `with_timed_transformation`, `comet_spinner` with `COMET_TURN` (700 ms) and `panel_empty_state` with `EMPTY_STATE_TURN` (1 s); no `with_animation` closure remains, and `comet_spinner` no longer takes an element id, so its five callers in `settings/tabs/agents.rs`, `settings/tabs/spaces.rs` and `window_chrome/title_bar.rs` drop it. `spinners_turn_for_sixty_frames_without_rendering_their_view_again` renders two comet spinners and an animated empty state and asserts one render over 60 simulated frames; against the previous `with_animation` body it fails with 61 renders. `spinners_turn_once_per_period_on_the_shared_clock_and_rest_under_reduce_motion` asserts the periods, no rotation under `reduce_motion`, and identical rotations for two comet spinners, which GPUI phase-locks to the app's shared animation clock; `DESIGN.md` section 4.8 records the app-wide phase. `lane.rs` only gains a test after its existing ones.

**Review evidence (2026-11-15):** every caller of the two spinners (`settings/tabs/agents.rs` three times, `settings/tabs/spaces.rs`, `window_chrome/title_bar.rs`, and `panel_empty_state`) goes through `ui_primitives::spin`; no rotating `with_animation` remains in `src-app/src`. The phase and the per-frame motion come from GPUI `703b5ac3`, checked in its source: `Svg::paint` turns a `TimedTransformation` into a `SpriteAnimation` unless `cx.reduce_motion()` (`zed/crates/gpui/src/elements/svg.rs:160-163`), and `Window::advance_scene_animations` poses every animated sprite from the one `cx.synced_animation_epoch` (`zed/crates/gpui/src/window.rs:3057-3060`), so the equality the phase test asserts is the whole contract. `App::set_reduce_motion` refreshes every window when the flag changes (`zed/crates/gpui/src/app.rs:1418-1423`), so a spinner stops at once when reduced motion is turned on.

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
- [x] Given the primary sidebar collapsing, when its wrapper opacity changes, then the cached `SidebarView` repaints with the new opacity on that frame, asserted by a test
- [x] Given a sidebar menu open and one agent in the Thinking state, when 100 root renders pass with no other input, then the sidebar render count stays in the range `lane.rs:676` already asserts (10 to 12 per second) and terminal views are not re-rendered
- [x] Given an entity read only inside a deferred menu under a cached view, when it notifies, then the menu updates on the next frame
- [x] Given an entity that notifies on every frame, when it is read under a cached view, then the test fails with the offending view named, so a redraw loop cannot land unnoticed
- [x] Given a cached sidebar or terminal view reused from the previous frame, when the accessibility tree is read, then the roles Paneflow declares are still present; if they are not, the finding is recorded with a link to zed-industries/zed#66690 and no workaround is added without Arthur's decision (the roles are not present on a reuse frame: finding recorded in `ARCHITECTURE.md` with the link, no workaround)

**Implementation evidence (2026-11-15):** `SidebarView::faded_column` (`app/sidebar/view.rs`) is the column `app/render.rs` draws the primary sidebar in; `a_collapsing_sidebar_repaints_its_cached_view_at_the_new_opacity_on_that_frame` asserts that an unchanged opacity reuses the cache and each new opacity repaints the view in that frame with no follow-up frame. `an_open_sidebar_menu_and_a_thinking_agent_keep_terminals_idle_over_a_hundred_root_renders` (`lane.rs`) keeps a deferred `menu_reveal` menu open beside a thinking loader and asserts 10 to 12 sidebar renders over 100 root renders, two cached terminal views never re-rendered, and no redraw loop. `an_entity_read_only_inside_a_deferred_menu_reaches_it_after_the_sidebar_is_reused` asserts that after two cache-reuse frames the deferred menu shows a new label on the next frame and that an entity read only while painting it still wakes the window. `redraw_guard::assert_no_redraw_loop` (test-only) draws 60 frames and panics with the name of any watched view that rebuilt on all of them; `an_entity_notifying_every_frame_under_a_cached_view_names_the_looping_view` expects that panic naming `LoopingReader`. Accessibility: GPUI's test platform has no accessibility adapter, so the tree cannot be read in a test; by inspection of GPUI `703b5ac3`, `reuse_prepaint` and `reuse_paint` replay no accessibility node, so a cached view's roles are absent on reuse frames, which matches the earlier Windows observation (60 references while the sidebar re-renders, 12 once static). Recorded in `ARCHITECTURE.md` with zed-industries/zed#66690; no workaround without Arthur's decision.

**Review evidence (2026-11-15):** `app/render.rs` draws the primary sidebar through `SidebarView::faded_column`, the same function the opacity, deferred-menu and redraw-loop tests enter through, so the tests exercise the production column rather than a copy. The 100-root-render test builds its sidebar from the production `comet_trail_loader`, `SpinnerClock` and `menu_reveal`, not from `SidebarView` itself, which needs a full `PaneFlowApp`. `redraw_guard` is test-only by design (the fourth criterion is about a test), and `lane.rs` uses it on real sidebar and terminal stand-ins. Leaving a pending accessibility workaround to Arthur is the outcome the fifth criterion prescribes. Gates on the final state: `cargo fmt --check`, Windows clippy and WSL Linux clippy (`--workspace --all-targets --locked -D warnings`) pass; `cargo nextest run --workspace --locked --profile ci` on Windows passes 4591 of 4593, the two others being `paneflow-host` tests (`an_output_flood_reaches_every_follower_and_a_paused_follower_never_stalls_control`, `ownership_probes::the_fixture_modes_are_driven_through_the_real_host_ipc`) that fail under load and pass on retry and in isolation; `paneflow-host` does not depend on `src-app` and runs only its own binaries, so EP-003 leaves those test binaries unchanged. The 94 targeted EP-003 tests pass in WSL Linux; `scripts/check-test-wallclock.sh` passes. macOS: EP-003 adds no `cfg(target_os = "macos")` item.

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
- [x] Given a line with tabs in the code editor or the diff view, when it is shaped, then each tab is replaced by the spaces that reach the next multiple of 4 columns, following the rule at `tab_map.rs:459`
- [x] Given the buffer, when a line is displayed, then the stored text is unchanged: a tab stays one character on disk, in the clipboard and in the diff
- [x] Given a caret or selection after a tab, when its x position is computed, then it maps from the buffer offset to the expanded column and back, so the caret never rests inside a tab and a click in a tab's span lands on the nearer side
- [x] Given the same line on Linux, macOS and Windows, when it is shaped, then a tab spans the same number of columns on the three platforms, asserted by a platform-independent test on the expansion
- [x] Given a line of 10,000 tabs, when it is shaped, then expansion is linear in the line length and the frame completes
- [x] Given a wide character (a CJK ideograph, an emoji) before a tab, when the line is shaped, then the tab stop is computed from display columns, with a test for a double-width character
- [x] Given a search match or a word-diff range that spans a tab, when its rectangle is drawn, then it covers the expanded width (`src-app/src/diff/element.rs:1746`)

**Implementation evidence (2026-11-16):** `src-app/src/tab_stops.rs` owns the rule: `tab_len(column) = TAB_SIZE - column % TAB_SIZE` with `TAB_SIZE = 4`, columns counted with `unicode-width` (a CJK ideograph or an emoji counts 2, a combining mark 0). `TabStops::expand` makes one pass over the line and records each tab's buffer offset, display offset and width; `to_display` and `to_buffer` map offsets by binary search, and a display column inside a tab's span lands on the nearer side. `TabExpandedLine::shape` shapes the expanded text with the runs stretched over the tabs. Entry points: the code editor's `ShapeCache::shape_in` (`app/diff_dock/code/element.rs`) caches it by the buffer text, and the hit map keeps it, so the caret, the selection, `CodeHitMap::offset_at` (clicks and IME `character_index_for_point`) all map through it; the minimap (`code/minimap.rs`) and the diff view's `DiffElement::shape` (`diff/element.rs`, word-diff rectangles through `push_word_rects`) shape through it too. The stored text never changes: the document, the clipboard (`slice_string`) and `to_disk_string` keep the tab. Horizontal extents follow the expansion: `CodeDocument` now tracks `longest_line_columns` (tab-expanded columns, maintained per edit), `cursor::display_column` drives caret reveal (`view/motion.rs`) and IME bounds (`view/input_handler.rs`), and the diff's `FileSpan::max_chars` uses `tab_stops::display_columns`. The giant-line read-only threshold still counts characters (`load::longest_line_chars`), so a tab-indented file does not become read-only sooner. Tests: `tab_stops::tests` (the 4-column rule, parity with Zed's `TabMap` expansion on ASCII, double-width characters, combining marks, the nearer side, runs, a word range over a tab, 10,000 tabs in one pass), `view::render::tests::a_tab_indented_row_is_as_wide_as_its_four_column_expansion` and `a_painted_tab_keeps_one_buffer_character_and_maps_clicks_and_carets` on a painted `CodeView`, `document::tests::longest_line_counts_expanded_tab_columns`, `diff::element::tests::a_word_diff_range_over_a_tab_covers_its_expanded_width`. The expansion is plain Rust and never asks the platform text system, so a tab spans the same columns on the three platforms. Out of scope and unchanged: vertical motion keeps a character goal column, the status column counts characters, and the single-line `TextInput` is not expanded. Visual pass: a tab-indented file in the code editor (caret after a tab, a selection across a tab, a click in each half of a tab, IME candidate placement after a tab, horizontal scroll to the end of the longest tab-indented line), its minimap, and a diff of tab-indented code in unified and split modes with a word-diff range over a tab.

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
- [x] Given a tooltip, a settings description and a markdown paragraph containing `feat/gpui-upgrade`, `C:\dev\paneflow\src-app`, `https://example.com/a/b?x=1` and a hyphenated word, when each wraps at a width that forces a break, then a test asserts the break falls after `/` or `-` and never before `/`, `?` or `-`; a Windows path breaks only at its UAX #14 opportunities (after `C:` or a `-`), because LB24 (`PR × AL`) forbids a break between `\` and a letter, and Paneflow does not insert break points of its own (decision of 2026-11-16)
- [x] Given a row that `DESIGN.md:1673-1674` says never wraps, when its text is longer than the row, then it truncates with an ellipsis on one line as before
- [x] Given a 200-character token with no break opportunity, when it wraps, then it breaks between grapheme clusters and never inside one
- [x] Given text in Chinese or Japanese with closing punctuation, when it wraps, then the closing mark stays on the line of the character before it
- [x] The story lists the dialogs and callouts whose bodies wrap, for the visual pass

**Implementation evidence (2026-11-16):** Paneflow owns none of these wraps: GPUI's `LineWrapper` breaks them, so the tests drive the real containers through `src-app/src/text_wrap_probe.rs`, which draws a `StyledText` in a window with a fixed width and reads `TextLayout::wrapped_text`. The containers are the shipping ones: `tooltip_shell()`, `settings::components::setting_description` (now the body of `setting_text`), the markdown `paragraph_row` with `build_styled_text` at `BODY_TEXT_SIZE`, and `ui_primitives::single_line_label` (now the label of every `action_menu` row). `a_tooltip_breaks_paths_branches_and_urls_after_their_separators`, `a_settings_description_breaks_...` and `markdown::view::tests::a_markdown_paragraph_breaks_...` check breaks after `/` and `-`, a Windows path that breaks after `C:` or `src-` (LB24 forbids `\` then a letter, decision 1.4), and no line that opens with `/`, `?` or `-`. `a_row_label_truncates_on_one_line_instead_of_wrapping` checks one line that ends in `…`. `a_token_without_break_opportunity_breaks_between_grapheme_clusters` wraps a 200-character token of `e\u{301}`, `👍🏽`, `🇯🇵` and `👩\u{200d}💻` at four widths and checks each break falls on a grapheme boundary. `closing_cjk_punctuation_stays_with_the_character_before_it` covers `！`, `。` and `」`. Dialog bodies that wrap, for the visual pass: the worktree removal confirmation (`app/worktree_remove.rs`), About (`app/about_dialog.rs`), unsaved changes (`app/unsaved_dialog.rs`), quit (`app/quit_dialog.rs`), system information (`app/system_info_dialog.rs`) and the sidebar selection dialog (`app/sidebar/selection.rs`), all through `modal_card`; also settings descriptions, tooltips and markdown paragraphs.

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
- [x] Given a cell holding a base character with a combining mark, when its glyphs are positioned, then the first glyph of the cluster sits at `cell_width * column` and the mark keeps only its offset inside the cluster, asserted by a test on `CellAligner`
- [x] Given a cell holding a color emoji wider than its base glyph, when it paints, then its origin is the cell origin and the following cell's origin is unchanged
- [x] Given the terminal golden corpus, when the suite runs, then no golden file changes
- [x] Given a title that fits its row by less than 0.5 px, when it is measured on macOS, then it does not flip between fitting and truncating between two frames
- [x] The story lists, for the visual pass, color emoji on Windows at 100 %, 125 % and 150 % scale and combining marks on Linux

**Implementation evidence (2026-11-16):** `CellAligner::glyph_x` (`terminal/element/paint/text.rs`) already puts the first glyph of each cell at `cell_width * column` and keeps every other glyph of the cluster at its shaped offset from that glyph, so the `x_offset` that cosmic-text now adds (#66799) survives inside the cluster and never moves a column. New tests: `a_combining_mark_after_a_drifted_base_keeps_only_its_cluster_offset` (a shaper that has drifted by 0.75 px over four cells still lands the base on column 3 and the mark 2.5 px after it) and `a_color_emoji_wider_than_its_cells_starts_at_its_cell_and_leaves_the_next_one_alone` (an emoji with a 20 px advance over two 8 px cells, and an emoji with a skin-tone modifier). Raster bounds, the union of the Windows layers in #66588, decide only the sprite's extent, not its origin, which `paint_placed_glyphs` takes from the aligner. The golden corpus (`golden_frame_corpus` and the other terminal golden tests) passes with no file under `terminal/element/golden/` changed. The sidebar title is truncated by GPUI's `text_ellipsis`, not by a Paneflow measurement: `agents_section::tests::a_title_that_fits_by_less_than_half_a_pixel_never_flips_between_frames` bisects the section width to the edge of fitting, then draws three frames at 0.4 px of slack (whole every time) and three just below (truncated to the same width every time). GPUI's test platform measures with `NoopTextSystem` on every OS, so that test alone never reaches CoreText; `a_title_measured_by_the_platform_text_system_never_flips_at_its_fitting_edge` runs the same check on `TestAppContext::build_with_text_system` with `gpui_platform::current_platform(true).text_system()`, which is DirectWrite on Windows, cosmic-text on Linux and CoreText on macOS, so the `macos-15` legs of EP-008 run it on the #65728 path; no Mac is available to run it earlier. Visual pass: color emoji in a terminal on Windows at 100 %, 125 % and 150 % scale (`👍🏽`, `🇯🇵`, `👩‍💻` next to ASCII), combining marks on Linux (`e\u{301}`, `n\u{303}`, Devanagari matras), and a sidebar title near its truncation width.

**Review evidence (2026-11-16):** Reachability, read from the execution roots: `CodeElement` prepaint shapes every code row, and every gutter number, through `ShapeCache::shape_in` into a `TabExpandedLine`, and the selection quads, the caret quad and the `CodeHitMap` it stores all map buffer offsets through it; `CodeHitMap::offset_at` serves the pointer and `character_index_for_point`; `bounds_for_range` and the caret reveal use `cursor::display_column`; `MinimapPaint` and `DiffElement::shape` shape through `TabExpandedLine::shape`, and `push_word_rects` maps word-diff ranges through it. No path still shapes a code or diff row with a raw tab (the remaining `shape_line` calls are line numbers, notes and labels), the diff view has no hit test to map, and every new symbol has a production referent except the test-only `text_wrap_probe`. The wrap tests enter through the shipping `tooltip_shell`, `setting_description`, `paragraph_row` and `single_line_label`, and the glyph tests through the real `CellAligner`. Two fixes from the review: the US-015 title check only ever ran on `NoopTextSystem`, so a second entry point now runs it on the platform text system (see US-015), and `cursor::display_column` sliced the line with ropey's `byte_slice`, which panics on an offset inside a character where the `goal_column` it replaces on the render path rounded; it now rounds through `byte_to_char`, covered by `display_column_counts_tab_stops_and_tolerates_an_offset_inside_a_character`. Gates on the final state: `cargo fmt --check`, Windows clippy and WSL Linux clippy (`--workspace --all-targets --locked -D warnings`, private target dir, deleted) pass; the 287 targeted EP-004 tests, golden corpus included, pass on Windows and in WSL Linux, the platform-text-system title test with them; `cargo deny check advisories licenses sources` and `scripts/check-test-wallclock.sh` pass. `cargo nextest run --workspace --locked --profile ci` on Windows passes 4618 of 4621 with the machine at 90 % CPU from another session's builds: `pty_runtime::tests::windows_shell_matrix_preserves_unicode_environment_and_cwd` (WSL leg timed out) and `paneflow-host` `the_fixture_modes_are_driven_through_the_real_host_ipc` pass when rerun alone, and `paneflow-ai-hook` `missing_session_is_an_immediate_no_op_without_files_or_network` misses its 50 ms spawn budget (best 69 ms) under that load; `paneflow-ai-hook` does not depend on `src-app` and EP-004 changes no file of it, so its binary is the one EP-003 certified. macOS: EP-004 adds no `cfg(target_os = "macos")` item; the CoreText run of the title test belongs to EP-008.

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
- [x] Given a window toggled to fullscreen and back on Linux, when the bounds observers fire, then the saved windowed size is unchanged, asserted by a test on the save path
- [x] Given the bounds observer fires while fullscreen, when it runs, then it performs no file write and no layout rebuild beyond what a resize already does
- [x] Given an adapter that is rejected by the new surface test, when the app starts, then it falls back to the next adapter or exits with the existing error path, and the log names the rejected adapter
- [x] The checks that need a real Linux desktop session (time to first frame on both pins, the sessions and drivers exercised) are run by US-025

**Implementation evidence (2026-11-16):** Both bounds observers now share one fullscreen guard, `window_state::windowed_bounds` (`src-app/src/window_state.rs:114`): the main window's observer records through `record_windowed_size` (`:121`, called from `src-app/src/app/window.rs:283`) and the detached window's observer schedules its session save only through it (`src-app/src/app/detached_panes/window.rs:212`), so a bounds callback while fullscreen or maximized records nothing and writes nothing; neither observer does more than the `cx.notify()` a resize already does. `save` now resolves the path and calls `save_to` (`:162`), which the test drives. `a_fullscreen_round_trip_saves_the_windowed_size` opens a GPUI test window whose bounds observer calls the shipping `record_windowed_size`, resizes it to 1000 by 700, toggles fullscreen (the test platform fires the resize callback, as `ba06cc5191` now makes Wayland and X11 do), resizes to 1920 by 1080, toggles back, which first reports the fullscreen size with the flag cleared (the X11 `_NET_WM_STATE`-before-`ConfigureNotify` order), then 1000 by 700, asserts no state file exists before the save, and reads 1000 by 700 back from `save_to`. Adapter fallback: GPUI's `select_adapter_and_device` (`zed/crates/gpui_wgpu/src/wgpu_context.rs:523`) tries each adapter with the renderer's real surface configuration and logs `Adapter <name> (<backend>) failed: ..., trying next...` at info level; when none passes, the error reaches `cx.open_window` through `WgpuRenderer::new(...)?` and Paneflow's existing path logs it and exits (`src-app/src/launch.rs:468`). Paneflow's default filter was `warn`, which hid that line, so the default filter is now the `DEFAULT_LOG_FILTER` constant (`src-app/src/launch.rs:152`) with `gpui_wgpu::wgpu_context=info`, and `the_default_log_names_a_gpu_adapter_the_surface_test_rejects` asserts through env_logger's own parser that this module logs at info while the rest of the app, `wgpu_renderer` and `wgpu_hal` stay where they were. Cost: about six adapter lines on stderr per Linux launch. Residual for US-025: if a window manager sends `ConfigureNotify` for the fullscreen size before it sets `_NET_WM_STATE`, the screen size is recorded until the window leaves fullscreen; this ordering predates the bump and only a real X11 session can show it. Gates on the final state: `cargo fmt --check`, Windows clippy and WSL Linux clippy (`--workspace --all-targets --locked -D warnings`, on a clone of HEAD with the epic's diff applied, clone deleted) pass; `cargo nextest run --workspace --locked --profile ci` on Windows passes 4629 of 4629; the 38 targeted EP-005 tests pass in WSL Linux, the Linux-only `linux_backdrop` tests among them; `scripts/check-test-wallclock.sh` passes; no dependency changed, so `cargo deny` was not rerun. macOS: no `cfg(target_os = "macos")` item changes.
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
- [x] Given Linux on a compositor that supports the protocol and the backdrop setting on "blurred", when the window opens, then it requests `WindowBackgroundAppearance::Blurred`
- [x] Given client-side decorations, when the window's corner radius is set, then `set_client_corner_radii` is called with the same radius next to `set_client_inset`, and with zero radii under server decorations
- [x] Given a compositor without the protocol and without KDE blur, when the setting is "blurred", then the window uses the translucent appearance it uses today and logs the fallback once
- [x] Given X11, when the setting is "blurred", then behavior is unchanged from today
- [x] Given macOS or Windows, when the setting is read, then the mapping at `src-app/src/app/constants.rs` is unchanged
- [x] Given the criteria above, when they are proven here, then they are proven by tests on the mapping and the window calls; the result on real compositors is checked by US-025

**Implementation evidence (2026-11-16):** GPUI does not tell the app whether the compositor can blur, so `linux_backdrop` asks it: when `window_backdrop` resolves to `blurred` (`constants::window_backdrop_requests_blur`, `src-app/src/app/constants.rs:129`, which reads `PANEFLOW_WINDOW_BACKDROP` first), both window opens (`src-app/src/launch.rs:455`, `src-app/src/app/detached_panes/window.rs:127`) pass the request to `apply_subtle_chrome_material`, which on a Wayland surface starts `probe_compositor_blur` (`src-app/src/window_chrome/linux_backdrop.rs:121`) on GPUI's background executor: a separate `Connection::connect_to_env()` and one registry roundtrip, never on the render thread and never on GPUI's own display queue. It looks for `ext_background_effect_manager_v1` (the name comes from `wayland-protocols`, already a dependency) or `org_kde_kwin_blur_manager`, caches the answer for the process and refreshes the window. `surface_appearance` (`:47`) then returns `Blurred` on Wayland when the compositor offers either protocol, and otherwise the appearance `refresh_blur_region` already chose (transparent when a material is on, opaque or X11-alpha transparent when not); `blur_falls_back` (`:60`) and `first_blur_fallback` (`:68`) log one warning per process that names the missing protocols, on Wayland without either protocol and on X11. Every render already calls `refresh_blur_region` (`src-app/src/app/render.rs:298`, `src-app/src/app/detached_panes/window.rs:303`), so the probe result reaches both windows without new wiring. Corner radii: `csd::client_frame` (`src-app/src/window_chrome/csd.rs:69`) returns the inset and the per-corner `WINDOW_CORNER_RADIUS` the shell already rounds with (square where tiled), or zero inset and `Corners::default()` under server decorations, and `client_side_window_shell` calls `set_client_corner_radii` right after `set_client_inset` (`:88`), as `zed/crates/workspace/src/workspace.rs:12700-12708` does; only GPUI's Wayland window reads it, for the blur region. Tests: `a_blurred_setting_requests_blur_where_the_compositor_offers_it`, `a_wayland_compositor_without_blur_keeps_the_unblurred_appearance_and_logs_it`, `the_blur_fallback_is_logged_once`, `x11_keeps_its_appearance_whatever_the_blur_setting`, `without_the_blurred_setting_no_surface_requests_blur`, `either_blur_protocol_counts_as_offered` (Linux only), `client_decorations_report_the_drawn_corner_radius_next_to_the_inset`, `server_decorations_report_no_inset_and_square_corners`. `window_background_appearance_for_preference` is untouched, so the macOS and Windows mapping is unchanged. `DESIGN.md` (Linux backdrop row) no longer says Paneflow never requests blur. Decision left open for Arthur: `blurred` requests the blur whether or not a Linux material is on; with both materials off the shell paints opaque over it, so nothing shows. Not done here: `docs/user/configuration/schema.md` mirrors paneflow-web and still documents only the Windows meaning of `blurred`; the site edit belongs to EP-006. Visual pass (Arthur, US-025): KDE Plasma 6 and a compositor with ext-background-effect-v1 with `window_backdrop: blurred` and `linux_chrome_material` on, floating, tiled and maximized (rounded and square blur corners); GNOME without either protocol (one warning, translucent window as today); X11 (unchanged). Gates on the final state: `cargo fmt --check`, Windows clippy and WSL Linux clippy (`--workspace --all-targets --locked -D warnings`, on a clone of HEAD with the epic's diff applied, clone deleted) pass; `cargo nextest run --workspace --locked --profile ci` on Windows passes 4629 of 4629; the 38 targeted EP-005 tests pass in WSL Linux, the Linux-only `linux_backdrop` tests among them; `scripts/check-test-wallclock.sh` passes; no dependency changed, so `cargo deny` was not rerun.
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
- [x] Given a submenu whose parent row is near the right edge, when it opens, then it is placed with `anchored().attach(...)` on the left of the row, decided from the submenu's measured size
- [x] Given room on both sides, when a submenu opens, then it opens on the right, as today
- [x] Given room on neither side, when a submenu opens, then it stays inside the window and its parent row remains visible
- [x] Given the path picker, when it opens above or below its field, then its behavior is unchanged in this story
- [x] The width estimate `opens_left` and the absolute positioning in `attach_submenu` are removed

**Implementation evidence (2026-11-16):** `attach_submenu` (`src-app/src/ui_primitives/action_menu.rs:862`) now ports Zed's submenu container (`zed/crates/ui/src/components/context_menu.rs:1910-1923`): a zero-size box pinned to the parent row's top-left holds `deferred(anchored().position_mode(Local).attach(row, Anchor::TopRight).anchor(Anchor::TopLeft).offset(...))`, where the row is `submenu_row_bounds` (`:854`, the panel width minus its padding and border, `ROW_HEIGHT` tall) and the offset keeps today's geometry: the submenu panel starts `SUBMENU_GAP` past the parent panel and its first row lines up with the parent row. GPUI flips the side, and the offset with it, from the submenu's laid-out size, and snaps it inside the window minus the client inset. `opens_left`, the pre-measured `child_width`, the absolute `left`/`top` and the `root_bounds` probe that only fed the estimate are removed; `render` calls it at `:454`. The path picker and the change-marker popover are not touched. Tests through the real `ActionMenu::element` in a GPUI window: `a_submenu_with_room_on_both_sides_opens_on_the_right` (800 by 600, centered menu, rows aligned), `a_submenu_near_the_right_edge_opens_on_the_left`, `a_submenu_near_the_bottom_edge_stays_inside_the_window` (the submenu moves up to fit), `a_submenu_with_room_on_neither_side_stays_inside_and_leaves_its_row_visible` (400 by 300, the test first asserts neither side fits, then that the panel is inside the window and covers less than half of the parent row); the existing click, keyboard and dismiss tests pass with a second submenu item. Below the 800 px minimum window the parent row can only be partly covered: at 800 px with two 320 px menus the worst overlap is about 90 px. Visual pass (Arthur): a tab or workspace context menu with a submenu opened near the right edge, the bottom edge, and in an 800 by 500 window, on each platform; on Linux with client-side decorations, near the edge where the transparent inset lies. Gates on the final state: `cargo fmt --check`, Windows clippy and WSL Linux clippy (`--workspace --all-targets --locked -D warnings`, on a clone of HEAD with the epic's diff applied, clone deleted) pass; `cargo nextest run --workspace --locked --profile ci` on Windows passes 4629 of 4629; the 38 targeted EP-005 tests pass in WSL Linux, the Linux-only `linux_backdrop` tests among them; `scripts/check-test-wallclock.sh` passes; no dependency changed, so `cargo deny` was not rerun. macOS: no `cfg(target_os = "macos")` item changes.
**Review evidence (2026-11-16):** Reachability, read from the execution roots: both `cx.open_window` calls (`launch.rs`, `detached_panes/window.rs`) pass `window_backdrop_requests_blur` to `apply_subtle_chrome_material`, the only writer of `BLUR_REQUESTED`; every main and detached render calls `refresh_blur_region`, the only caller of `window_appearance`, so the probe result and the fallback log reach both windows; `client_side_window_shell` is the shell of both windows, so `set_client_corner_radii` runs beside `set_client_inset` on every frame (GPUI 703b5ac3 returns early when the radii are unchanged, `zed/crates/gpui_linux/src/linux/wayland/window.rs:2249-2256`); both bounds observers go through `windowed_bounds`; `DEFAULT_LOG_FILTER` is the filter `run` installs; `attach_submenu` is reached from `ActionMenu::element`, and `opens_left` and `root_bounds` have no referent left. Checked in GPUI's source: `Blurred` on Wayland binds ext-background-effect first and falls back to KDE blur (`:2509-2546`), and its blur region is the window area rounded by `client_corner_radii`, so Paneflow's probe asks for exactly the two globals GPUI binds (`zed/crates/gpui_linux/src/linux/wayland/client.rs:311-312`); `anchored` snaps inside the window minus `client_inset` (`zed/crates/gpui/src/elements/anchored.rs:190,259`); the rejected adapter is logged by `gpui_wgpu::wgpu_context` at info (`wgpu_context.rs:594-595`). Two test fixes from the review: `a_fullscreen_round_trip_saves_the_windowed_size` ended on a resize to the windowed size, so its save assertion held even without the fullscreen guard; it now also saves while fullscreen and reads 1000 by 700 back, and removing the guard from `windowed_bounds` turns it red (checked, then restored). The four submenu placement tests measured the submenu panel from its first row only, so `inside` never saw the second row; `open_submenu` now returns the union of the first and last submenu rows. Not applied (NON_BLOCKING): `BLUR_REQUESTED` is process-wide and only ever set, as `TRANSLUCENT_WINDOW` is, so a detached window opened after `window_backdrop` changed at runtime follows the value read by the first window that asked for blur; `window_backdrop` is read at window open on every platform, so this is a restart-to-apply edge, not a regression. Decision still open for Arthur (from US-017): `blurred` requests the blur even when both Linux materials are off. Gates on the final state: `cargo fmt --check` and Windows clippy (`--workspace --all-targets --locked -D warnings`) pass; WSL Linux fmt, clippy and 37 targeted tests (the 9 Linux-only `linux_backdrop` tests among them) pass on a clone of HEAD with the epic's diff applied, clone deleted; `cargo nextest run --workspace --locked --profile ci` on Windows ran 4629 tests, 4623 passed, and the 6 others (`pty_runtime` `windows_shell_matrix_preserves_unicode_environment_and_cwd`, four `paneflow-host` tests and `paneflow-textdiff` `refreshing_a_hundred_line_block_in_a_large_document_stays_under_five_milliseconds`) pass when rerun serially; EP-005 changes no file of `paneflow-host`, `paneflow-textdiff` or the PTY runtime. `scripts/check-test-wallclock.sh` passes; no dependency changed, so `cargo deny` was not rerun. macOS: no `cfg(target_os = "macos")` item changes. Manual checks on real compositors and window managers stay with US-025 as the criteria prescribe; the submenu visual pass is Arthur's.

---

### EP-006: Evidence and records

The bump is measured, re-baselined and written down where the repository keeps such records.

**Definition of Done:** the bump summary exists with every figure measured so far and a Linux section that US-027 fills, and the changelog and design documents describe the UI as it now behaves. The Linux re-baseline belongs to US-027 (PRD 1.5).

**Zed references:**
- `zed/crates/gpui_platform/benches/primitives.rs` and the two-revision comparison script added by `26abd1c959` (#66316).
- `zed/crates/gpui/src/profiler/journal.rs:171`: `FrameSkipReason::AnimationFrameRateLimit`, the reason a throttled frame is skipped.
- `zed/crates/gpui/src/app.rs:1741`: `pub fn debug_state(&self) -> serde_json::Value`.

#### US-019: Write the bump summary
**Description:** As the maintainer, I want one summary of what the bump measured and changed so that the next regression is read against GPUI as it now is. The benchmark runs and the re-baseline moved to US-027 (PRD 1.5).

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-009, US-011, US-013

**Zed references:**
- #66809, #66810, #66811 PR texts: the GPU and CPU cost changes expected from the renderer.
- `zed/crates/gpui/src/window.rs:1999-2019`: a throttled frame is deferred and recorded as skipped, which changes frame-timing traces.
- `zed/crates/gpui_platform/benches/primitives.rs:245`: upstream's primitive benchmark, usable to compare the two GPUI revisions directly.

**Paneflow sites:** `bench/README.md:12-17,33-53,76-89`; `bench/baselines/linux-x86_64/{terminal,editor,startup,persistent,terminal-alloc,editor-alloc}.json`; `bench/results/gpui-bump-37faef81-2026-10-28.md` (the model); `scripts/bench-terminal.sh`, `scripts/bench-editor.sh`, `scripts/bench-startup.sh`, `scripts/perf-gates.sh`.

**Acceptance Criteria:**
- [x] Given a results file under `bench/results/`, when it is read, then it follows the layout of the 37faef81 summary and holds a Linux section with the before and after tables of the three suites, left for US-027 to fill
- [x] Given the summary, when it is read, then it records the frame-interval behavior, the latency measurements of US-009, that macOS latency is not measured (US-010 cancelled), and the path costs of US-003
- [x] Given the two stroke metrics US-003 found more than 10 % above the previous renderer, when the summary is read, then each is listed with its cause and the decision it waits for (the sprite-atlas mitigation or Arthur's acceptance)
- [x] Moved to US-027 (PRD 1.5): the suite runs on both pins, the 10 % comparison, the allocation check and re-baseline, `scripts/perf-gates.sh`, and the rule on runs from a loaded machine

**Implementation evidence (2026-11-16):** `bench/results/gpui-bump-703b5ac3-2026-11-16.md` follows the layout of the 37faef81 summary. Its first section, "Linux suites: to be filled by US-027", holds the before and after table of the three suites with every cell `not run`, the Fedora procedure, and the comparisons US-027 owes (the 1.10x p95 ratio `layout_echo_shifted_uncached_220x60`, the two 10 ns medians that moved under WSL2, startup against the baseline). It records the frame-interval behavior (every redraw limited since #66237, the three intervals and the Windows unfocused rate, 68 to 70 against 28.3 fps), the Windows latency of US-009 (median 4.99 against 5.57 ms, within the 2 ms budget), that macOS latency is not measured (US-010 cancelled, zed-industries/zed#66954 accepted on trust), the path costs of US-003 with the squircle verdict, and the allocation columns against `9f606d3aa`, all decreases, read from the committed baselines. The two stroke metrics above 10 % (`paths_curly_underline_200x50` +17 %, `paths_box_drawing_200x50` +11 %) are listed with their cause (each stroke outlined through `tiny_skia_path` and binned on every paint, #66809) and the open decision: accept the cost or ship the sprite-atlas mitigation. Arthur moved the Linux benchmark runs to EP-007 on 2026-11-16 (PRD 1.5, US-027); a WSL2 A/B run started before that decision was stopped before any suite ran, its clones deleted, and nothing from it is recorded.

**Review evidence (2026-11-16):** every figure of the summary was traced to its source. The allocation table matches `bench/baselines/linux-x86_64/{terminal,editor}-alloc.json` against the same files at `9f606d3aa`, row by row, and no other allocation column moved apart from the three new `paths_*` rows; the path table, the squircle verdict (0.16 ms median, 0.27 ms worst p95), the +17 % and +11 % run-mean deltas and the 1.22x, 1.111 and 0.905 ratios match `bench/results/gpui-703b5ac3-paths-2026-11-15.md`; the latency (4.99 against 5.57 ms median, 7.10 against 7.67 ms p95) and the unfocused rate (68 to 70 against 28.3 fps) match `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`; `terminal`, `editor` and `startup` baselines do point at `92a5c29d`. The frame-limit claim was read in GPUI's source: at `703b5ac3`, `has_demand_besides_animation` counts a dirty invalidator, so a notify or an echo is deferred by `frame_pacer.should_defer` like a forced render (`zed/crates/gpui/src/window.rs:1966-1990`). The three upstream fixes the summary names match their commit messages (`78f5d7446c`, `4d847859f5`, `0373366f43`). The Linux section is empty by design: the fourth criterion moved to US-027. No defect found.

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
- [x] Given `CHANGELOG.md`, when `[Unreleased]` is read, then it holds user-facing entries for tab width in the editor and diff view, wrapping, the frame limit on unfocused windows, spinner behavior, and each inherited fix, in the bold-lead format already used
- [x] Given `DESIGN.md:598-600`, when it is read, then it no longer describes a shape cache that translates paths, and it states how squircles are built and budgeted
- [x] Given `DESIGN.md:493-510`, when it is read, then spinner timing names the shared app-wide phase and the reduced-motion rule
- [x] Given `ARCHITECTURE.md`, when the render sections are read, then the frame policy (three intervals, every redraw limited) and the layer ordering rule are stated as the code implements them
- [x] Given `bench/README.md` and `.github/workflows/run_tests.yml:420`, when they are read, then the bump is recorded and no comment names a GPUI revision that is not the pinned one
- [x] Given the whole branch, when the checklist is written, then one list groups, per platform, every surface and state the stories flagged for Arthur's visual pass
- [x] Given a document claims a behavior no story verified, when it is reviewed, then the claim is removed or marked unverified

**Visual-pass checklist (US-020, 2026-11-16).** Every surface and state a story of this PRD flagged for Arthur's visual pass, grouped by platform. Each item runs under the `DESIGN.md` delivery gate (section 9): Paneflow Dark, Paneflow Light and one other preset, `reduce_motion` on and off, the 800 by 500 minimum window, and every control state the surface has. Mark each item pass or defect with the session or display it ran on.

*Every platform (Linux in US-026; Windows on this host; macOS has no hardware before 2027 and is covered by CI only):*
- [ ] US-005: the usage rings (sidebar capsule, Settings Usage, the concentric rings) at 0, at a value between 0 and 1, and full; the clone bubble check mark through its draw animation
- [ ] US-006: pane cards (squircle and morph frames) and the 2 px drop veil; dialogs (`modal_card`); menus and select popups; the quick pick shell and the path picker; the terminal search overlay; tooltips; the settings input surface and its focus ring; the clone bubble pill; the tab badges in the pane strip and in Settings Appearance (one highlight band clear of the rim, no white fringe at the bottom corners); a translucent border reads in one tone; at scale 1 and 2
- [ ] US-007: every surface of the `DESIGN.md` shadow inventory (client-side window, menus and select popups, toasts in both interface styles, dialog cards, drag ghosts, fleet search, custom buttons dialog, clone bubble, detached window tab capsule, raised button, select trigger); the gradients (tab badge fills, the graphite tile, the sidebar resize edge glow)
- [ ] US-011: the comet spinner (title bar update pill, Settings Agents and Spaces) and the empty-state spinner turn smoothly, two spinners on screen point the same way, and both hold still with `reduce_motion` on
- [ ] US-013: a tab-indented file in the code editor: the caret after a tab, a selection across a tab, a click in each half of a tab, IME candidate placement after a tab, horizontal scroll to the end of the longest tab-indented line; its minimap; a diff of tab-indented code in unified and split modes with a word-diff range over a tab
- [ ] US-014: the wrapping bodies of the worktree removal confirmation, About, unsaved changes, quit, System Info and the sidebar selection dialog; settings descriptions, tooltips and markdown paragraphs holding a branch name, a URL, a POSIX path and a Windows path; a sidebar or menu row that truncates on one line
- [ ] US-015: a sidebar title near its truncation width does not flip between whole and truncated
- [ ] US-018: a tab or workspace context menu with a submenu opened near the right edge, near the bottom edge, and in an 800 by 500 window

*Windows (this host):*
- [x] US-006 at scale 1: Paneflow Dark, Paneflow Light and Claude Dark in Rounded, Paneflow Dark and Light in Classic, reduce motion on, 800 by 500 (2026-11-15; translucent border decision applied afterward)
- [ ] US-006 at scale 2 (not checked: both displays of this host run at scale 1)
- [ ] US-015: color emoji in a terminal at 100 %, 125 % and 150 % scale (`👍🏽`, `🇯🇵`, `👩‍💻` next to ASCII)
- [ ] US-013: tabs had no width on Windows before the bump, so the code editor and diff items above are the largest visible change here

*Linux (US-025 and US-026, on the sessions of EP-007):*
- [x] US-008: translucent squircles over a translucent window keep their continuous corners and blend correctly (GNOME and KDE Plasma, Wayland and X11, NVIDIA and Mesa AMD, Arthur, 2026-11-15)
- [x] US-007: gradients gain an ordered dither on Linux only; check the badge fills and the resize edge glow for banding or visible noise
- [x] US-015: combining marks in a terminal (`e\u{301}`, `n\u{303}`, Devanagari matras) stay in their cell
- [x] US-016 (US-025): a window toggled to fullscreen and back keeps its windowed size, also after a restart
- [x] US-017 (US-025): `window_backdrop: blurred` with `linux_chrome_material` on, on KDE Plasma 6 and on a compositor with ext-background-effect-v1, floating, tiled and maximized (rounded and square blur corners); GNOME without either protocol (one warning, the translucent window as before); X11 (unchanged)
- [x] US-018: with client-side decorations, a submenu opened near the window edge where the transparent inset lies stays inside the visible window

*macOS (no hardware before 2027):*
- Not run. Every item of the first group is unverified on macOS until a Mac is available; EP-008 compiles and tests the macOS build in CI, including `a_title_measured_by_the_platform_text_system_never_flips_at_its_fitting_edge` on CoreText.

**Implementation evidence (2026-11-16):** `CHANGELOG.md` `[Unreleased]` gains, in the bold-lead format: under Added, the Linux blurred backdrop; under Changed, tab stops in the code editor and the Changes tab, UAX #14 wrapping, spinners turned by the renderer, submenus placed from their measured size, continuous corners on translucent Linux windows, and the rejected GPU adapter in the Linux log, next to the unfocused-window frame limit EP-003 already added; under Fixed, the three inherited GPUI fixes in one entry that says they were not reproduced in Paneflow. `DESIGN.md` 4.4 now states that a squircle is built at its final bounds in paint on every frame with nothing kept or moved, the fill depth under opaque and translucent borders, the 0.5 device pixel layer clearance and the 0.4 ms budget with the measured 0.16 ms; 4.9 no longer names `cached_squircle_path` or a shape cache. 4.8 already named the app-wide spinner phase and the reduced-motion rule (US-011) and is unchanged. Known Gaps no longer says the Linux shell never requests blur, and lists the two spinners among the animations `reduce_motion` stops. `ARCHITECTURE.md` already stated the three intervals and the layer rule (US-009, US-007) and now adds that every redraw is limited since #66237, checked at `zed/crates/gpui/src/window.rs:1977`. `bench/README.md` records the bump, links the summary and names US-027 for the timing re-baseline. `.github/workflows/run_tests.yml` no longer names `0b984b5`; no workflow comment names a GPUI revision any more, and `deny.toml` keeps its dated bump history. Unverified claims: the inherited fixes are marked as not reproduced, the Linux blur entry describes only the request and the corner radii the tests prove (the on-screen result is US-025), the squircle figure names WSL2. The checklist above groups every surface a story flagged by platform. Left open: `docs/user/configuration/schema.md` mirrors paneflow-web and still documents only the Windows meaning of `blurred`; the fix belongs in paneflow-web (six locales) and was not made here. Checks: `scripts/check-workflows.sh` and `zizmor` (high) pass on `.github`; the status file parses; `git diff --check` is clean; no Rust, test or dependency changed, so fmt, clippy, nextest and `cargo deny` have nothing new to run.

**Review evidence (2026-11-16):** each `CHANGELOG.md` entry was checked against the code that ships it: the 4-column stop and the 2-column wide character (`unicode_width` in the code editor's tab module, `TAB_SIZE = 4`); a hyphen that used to be a word character in GPUI's old `is_word_char` (`37faef81`, `line_wrapper.rs:547-548`), so breaking after it is new; the blur shown only with a material on (`refresh_blur_region` receives `native_material_requested`, chrome or terminal material); the fallback warning once per process on X11 and Wayland without blur; the rejected adapter logged at info by `gpui_wgpu::wgpu_context` under `DEFAULT_LOG_FILTER`; spinners that already held still under reduced motion at `9f606d3aa` (`comet_spinner` and `empty_state_spins` both read `reduce_motion`), so "as before" holds and the old `DESIGN.md` Known Gaps line was stale; submenu placement by `anchored` with the four placement tests. Every symbol `DESIGN.md` and `ARCHITECTURE.md` name exists (`fill_depth_under_border`, `LAYER_CLEARANCE_DEVICE_PX` = 0.5, `squircle_band_path`, `squircle_stroke_path`, `paced_window_options`, `every_paneflow_window_is_opened_with_the_paced_options`) and `cached_squircle_path` is gone from the code and the documents. 4.9 still says fills and borders paint as quads: that paragraph describes the Classic style, where `squircle_corners_fill` and `squircle_corners_border` do paint quads, so it does not contradict 4.4. The only commit hash left in a `.github` comment, `c3e2331`, is a Paneflow commit, not a GPUI revision. One fix from the review: the checklist listed the US-015 sidebar title truncation under Windows only, while US-015 flags it on every platform (its CoreText run is the macOS item); it now sits in the first group. Left open, outside this repository: `docs/user/configuration/schema.md` mirrors paneflow-web and still documents only the Windows meaning of `blurred`; US-017 assigned it to EP-006, but no EP-006 criterion covers it, and the edit belongs in paneflow-web in six locales, which needs Arthur's go-ahead. Gates on the final state: `scripts/check-workflows.sh` and `zizmor` (high) on `.github`, the status file parses, `git diff --check` is clean; no Rust, test or dependency changed, so fmt, clippy, nextest and `cargo deny` have nothing new to run.

---

### EP-007: Linux is validated on a real desktop session

Every Linux check that needs a real desktop session runs here, on Arthur's Fedora 44 machine (the hardware of the `linux-x86_64` baseline), once EP-001 to EP-006 are implemented on the Windows host and before any CI run. So do the Linux benchmark runs and the re-baseline (US-027, PRD 1.5): the baselines come from Fedora, never from WSL2. The earlier epics prove Linux by clippy and the touched tests in WSL only, which keeps the inner loop on one machine.

**Definition of Done:** every Linux runtime criterion of the PRD is recorded with its session, compositor and driver named, the Linux visual pass is done, and every defect it found is fixed on the branch and checked again.

**Sessions:** GNOME (Mutter) and KDE Plasma (KWin), each on Wayland and on X11, with the proprietary NVIDIA driver and with Mesa where the machine offers both, as US-008 did on 2026-11-15. A combination not exercised is named as such.

#### US-027: Re-baseline the Linux benchmarks on Fedora
**Description:** As the maintainer, I want the terminal, editor and startup suites re-run and re-baselined on the new pin on Fedora so that the next regression is measured against GPUI as it now is, on the machine the baselines come from.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-019

**Zed references:**
- #66809, #66810, #66811 PR texts: the GPU and CPU cost changes expected from the renderer.
- `zed/crates/gpui/src/window.rs:1999-2019`: a throttled frame is deferred and recorded as skipped, which changes frame-timing traces.
- `zed/crates/gpui_platform/benches/primitives.rs:245`: upstream's primitive benchmark, usable to compare the two GPUI revisions directly.

**Paneflow sites:** `bench/README.md`; `bench/baselines/linux-x86_64/{terminal,editor,startup,terminal-alloc,editor-alloc}.json`; `bench/results/gpui-bump-703b5ac3-2026-11-16.md` (the summary US-019 wrote); `scripts/bench-terminal.sh`, `scripts/bench-editor.sh`, `scripts/bench-startup.sh`, `scripts/perf-gates.sh`.

**Acceptance Criteria:**
- [x] Given the three suites on Fedora, when they run on the old pin (`9f606d3aa`) and on the final branch commit, then the Linux section of the bump summary holds the before and after tables in the layout of the 37faef81 summary, with the machine, the session and the CPU share
- [x] Given each metric, when it is compared, then any metric more than 10 % above its baseline is listed with its cause and the story that addressed it or accepted it, the two stroke metrics of US-003 included
- [x] Given the allocation baselines, when they are compared, then each is within 1 % or its change is explained, and the refreshed timing and allocation baselines are committed on top of the code commit, never from a dirty tree
- [x] Given `scripts/perf-gates.sh`, when it runs on the final commit, then it passes without any budget raised, or each raised budget is listed in the summary with the measured value and Arthur's approval
- [x] Given a benchmark run on a loaded machine, when two runs differ by more than 10 %, then the run is repeated and no baseline is set from it
- [x] Given `bench/README.md`, when it is read, then it names the commit the `linux-x86_64` timing baselines point at

**Implementation evidence (2026-11-16):** Fedora 45 (the PRD says 44; same hardware), GNOME 51.1 Wayland, Ryzen 7 7800X3D, RTX 4070 Ti SUPER. Each pin ran in its own worktree with its own target directory, two runs per suite, interleaved. A first set of runs taken while other builds loaded the machine (load average near 10) gave editor pairs 50 % apart and was discarded; the kept runs have a load average under 3 and a CPU share of 0.97 or more. `bench/results/gpui-bump-703b5ac3-2026-11-16.md`, section "Linux suites (US-027)", holds the tables, links every result file, and lists every metric more than 10 % above its baseline or the old pin. The runs found a regression from US-013: opening a file was up to 1.9x slower (`open_2mb_to_text` 1.04 to 1.89 ms) and `reload_10_hunks_100k_lines_ui` 30 % slower, because the longest-line measure folded every byte and every edited row looked up Unicode widths. Replacing the measure with a constant in a copy of each pin made both pins open in the same time within 2 %, which located the whole gap. `d78d92927` (`src-app/src/app/diff_dock/code/document.rs` `measure_text_lines`, `every_byte_is_one_column_or_a_newline`; `src-app/src/tab_stops.rs` `advance_str`) restores the old byte-length split behind one vectorized check and folds ASCII without the width table; the tests `every_longest_line_measure_matches_a_per_character_fold_across_rope_chunks`, `an_ascii_document_measures_like_the_per_character_fold`, `only_printable_ascii_and_newlines_take_the_byte_length_shortcut` and `advancing_by_a_whole_string_matches_advancing_one_character_at_a_time` hold the new measure to the per-character fold. Every editor timing is now within 4 % of the old pin. Above 10 % of a baseline: `prepaint_60_rows_warm` (+12 % on `92a5c29d`, +8 % on the old pin; US-013 clones a shaped line with its tab stops on a warm cache), accepted by Arthur; the strokes, +14 % (undercurl) and +17 % (box drawing) against the previous renderer on Fedora through the US-003 cases ported onto the old pin, accepted by Arthur; `line_text_at_220x60` and `base_font_resolve`, bimodal 10 ns medians on both pins. Allocations: `scripts/perf-gates.sh --refresh-alloc-baselines` on `d78d92927` moved one column, `keystroke_3_7mb_plain` 4.0 to 4.05 (ropey 1.6.1 allocates the node stack of a chunk iterator over a row that straddles two leaves, `ropey-1.6.1/src/iter.rs:1406`; about one row in 20). Startup: within 2 % of the baseline on both pins. The five baselines were set from clean runs of `d78d92927` and committed in `79411788`, on top of the code commit; `bench/README.md` names `d78d92927`. `scripts/perf-gates.sh` on `d78d92927` failed 3 of 116 budgets: the allocation column above (passes against the refreshed baseline) and `worker.idle.cpu_ms` (90 ms, limit 30) and `desktop.idle.root_renders` (4, limit 1), which also fail on `9f606d3aa` (80 ms, 3 renders) on this machine. No budget was raised; Arthur decided that the `perf_gates` job of EP-008, on the calibrated runner, gives the verdict, so the fourth criterion stays open until that job is green. Later commits (`1100a278`) change only corner radii in the window shell, which none of the three suites measures.

**Reopened (2026-11-17), no regression accepted:** the brief of the night run withdrew the acceptances above, so each was traced to its cause and fixed. Noise first: the old-pin path port and the branch ran interleaved, ten runs of each, each process pinned to CPU 2 (`taskset -c 2`), with a spread of 0.7 % to 1.9 % on the branch and up to 14 % on the old pin, whose undercurl has a heavy tail. Strokes: timing `PathBuilder::build` and `Window::paint_path` separately in a measurement copy (tagged instrumentation, removed) showed, per frame, 8.2 ms in GPUI's tiny-skia stroker and 2.4 ms binning for the undercurl screen, and 7.8 ms building and 23 ms binning 10,000 small paths for the box-drawing screen (`zed/crates/gpui/src/path_builder.rs:306-364` `stroke_outline`, `crates/gpui/src/path_winding.rs:101` `bin`, called per path by `PaintedPath::new` in `scene.rs:1111`). `0f1304dc` paints each arc, diagonal, cross, Powerline shape and undercurl cell from a mask rasterized once into the sprite atlas through `Window::paint_svg` (`src-app/src/terminal/element/paint/masks.rs` `paint_mask`, keyed by `MaskKey`; geometry in `paint/sprites.rs` `arc_outline`, `diagonal_outline`, `powerline_outline` and `paint/decorations.rs` `undercurl_outline`), as Ghostty draws its sprite font. A first undercurl mask as tall as the cell overlapped the row below and doubled the cost through the scene's overlap ordering (17.2 ms); the mask now covers only the band of its wave (`undercurl_band`). Measured: `paths_box_drawing_200x50` 22.3 ms on the old pin, 4.4 ms on the branch (-80 %); `paths_curly_underline_200x50` 9.25 ms on the old pin, 4.5 ms on the branch (one run, re-measured on the final tree below). Tests: `every_terminal_mask_rasterizes_through_the_gpui_svg_renderer` (fails with a broken SVG header), `a_mask_is_built_once_per_shape_and_cell_size`, and three coverage tests rendering the masks with resvg as GPUI does. `prepaint_60_rows_warm`: 22.5 us on the old pin, 24.9 us on the branch (+10.6 %, spread under 2 %). The warm path only looks the row up in the shape cache and clones what it finds, a 3000-byte `TabExpandedLine` since US-013 (2984 bytes for the bare `ShapedLine` before; both keep 32 decoration runs inline). `7504a269` keeps each cached line behind an `Rc`, so a warm hit shares it and a visible row copies its `ShapedLine` once, into its glyph: 16.2 us against 22.6 us on the old pin (-28 %), test `a_warm_shape_hit_shares_the_cached_line_instead_of_copying_it`. Idle gates: the harness started the worker and the desktop with the developer's environment, so on this machine they found `~/.claude` (2.2 GB of history) and `~/.codex` as usage accounts; a worker started alone used 28,710 ms of CPU over 30 s with the real home and 10 ms with an empty one. `c9be4c60` points `CLAUDE_CONFIG_DIR` and `CODEX_HOME` of both processes at empty directories in the fixture home (`fixture_agent_accounts` in `crates/paneflow-host/tests/persistent_baseline/processes.rs`, test `a_fixture_process_never_finds_the_developer_agent_accounts`, red without the isolation). No budget was raised.

**Remaining (2026-11-17), closed on 2026-11-18 (evidence below), for a dedicated session (`/implement-epic tasks/prd-gpui-703b5ac3-upgrade.md EP-007` resumes only this story):** Arthur closed the night run with the functional stories in review and moved the rest of the performance work here. State at `249d3776`:
- **Editor open regression from `afcd1219`** (chrono replaced by jiff to clear RUSTSEC-2026-0543 under `deny.toml`'s workspace scope). Ten interleaved rounds against the old pin: `open_300kb_highlighted` 154 to 213 us, `open_2mb_to_text` 1.06 to 1.45 ms, `open_3_7mb` 1.90 to 2.62 ms, `open_markdown_injected` 34 to 47 us (+37 %); self noise at most 0.7 %, a same-source build from another path at most 0.3 %. `git bisect d78d92927..afcd1219` with the editor suite (3 runs per step): every commit up to `f501e320` reads 160 to 168 us on `open_300kb_highlighted`, `afcd1219` 213 us. The commit touches no editor code; the cause is not found. Next: re-confirm `f501e320` against `afcd1219` interleaved on a quiet machine, diff `cargo tree -e features -p paneflow-app` between them (afcd1219 enables jiff `tz-system`, `tzdb-zoneinfo`, `tzdb-bundle-platform`, `serde`; feature unification may change a crate on the open path), profile the open path with `perf record -g`, and build afcd1219 with only src-app's or only paneflow-serve's dependency reverted to isolate it. Fix at the cause without bringing chrono back as a direct dependency.
- **`publish_echo_220x60` and `pipeline_corpus_mib_s`** after `f7c68b02`: the publish scroll cases are back within noise of their parent, but two builds of the same source differ by up to 9 % (echo) and 5 % (pipeline) from code placement alone, and both builds of the new code read slower on echo (9.5 and 9.0 us against 8.1 and 8.8 us); instruction counts per round are +0.6 % (echo) and +2.5 % (pipeline). Settle them against the old pin with instruction counts as well as time.
- **Every timing taken from 2026-11-16 to 2026-11-17 13:25 carried background load**: `scripts/bench-startup.sh` left the worker and host of each fixture home running, 121 workers (19 % CPU, 2.4 GB) and 81 hosts by then. `249d3776` stops them when the suite ends; the leftovers were stopped. Re-take the startup comparison on both pins (the old pin's harness still leaks: stop its leftovers after each run).
- **US-024's latency pair** on the final commit (see US-024's status).
- **Baselines and gates**: once nothing is worse than the old pin beyond the measured noise, refresh the allocation baselines (`scripts/perf-gates.sh --refresh-alloc-baselines`, four of them fall to 0 allocations after `0f1304dc` and `7504a269`), regenerate the `linux-x86_64` timing baselines from a clean tree, run `scripts/perf-gates.sh`, name the commit in `bench/README.md`, and update `bench/results/gpui-bump-703b5ac3-2026-11-16.md` "Linux suites (US-027)" with the final tables (the strokes and prepaint rows are fixed by `0f1304dc` and `7504a269`).
- **Unstable test, not diagnosed**: `paneflow-host::pty_descriptor_inheritance a_process_spawned_while_ptys_open_inherits_neither_end` failed twice in one `cargo nextest run --workspace --locked --profile ci` on `249d3776` (TRY 1 after 0.04 s, TRY 2 after 0.19 s) while the perf agent's bench binaries and builds ran on the same machine; the failure output was lost (the JUnit report was overwritten by later runs). It passed 3 of 3 alone and the whole suite passed 4880 of 4880 on the quiet machine right after. A failure after 0.04 s precedes the 200-spawn loop, which points at `paneflow_host::pty::open` failing under concurrent load; unproven. Next: reproduce by running the test while another process holds PTYs or spawns heavily, capture the panic, and fix the cause (test or code).
- **CI guard for the editor**: no CI job measures the editor's timings or instruction counts (`perf-ab` runs the terminal suite only, and `Performance gates` checks the editor's allocations, not its work), so the `afcd1219` regression would merge with every workflow green. Add the editor suite to `scripts/perf-ab.sh` (and `perf-ab.ps1`) with the same blocking instruction-count verdict as the terminal suite, prove it goes red on `afcd1219` against `f501e320`, and green once the regression is fixed.
- **Tooling trap**: a wait loop written `until ! pgrep -f X` matches its own shell's command line and never ends; wait on a PID or an output file, or use `pgrep -f '[X]...'`.

**Implementation evidence (2026-11-18), the remaining items closed:** every measurement below ran on the quiet Fedora machine (one-minute load under 1.2 before each set, no build, no leaked bench worker or host), interleaved with the order reversed every other round, each process pinned to CPU 2; noise is the final binary run again and a second build of the same commit from another path, both in the same rounds; instruction counts come from Callgrind, one count per case. The final code commit is `650569c4`; per-run values are committed in `bench/results/gpui-703b5ac3-us027-interleaved-2026-11-18.json` and the tables in `bench/results/gpui-bump-703b5ac3-2026-11-16.md`, "Linux suites (US-027)".
- **`afcd1219` open regression, fixed (`b42cd513`).** Confirmed quiet: `open_300kb_highlighted` 162 to 212 us against `f501e320`, same binary 0.1 %. Cause: placement, not work. Callgrind counts 995 653 997 against 995 652 036 instructions in `CodeDocument::new`; `perf annotate` puts 82 to 88 % of its cycles on the same `je` of the byte-by-byte newline split of `measure_text_lines`, identical machine code that starts a 64-byte line on `f501e320` and straddles one on `afcd1219`. The feature diff of `cargo tree -e features` (first lead) changes only jiff's features and drops chrono's `clock`. `longest_byte_line` (`src-app/src/app/diff_dock/code/document.rs:349`) finds the newlines with memchr; test `the_longest_byte_line_matches_the_longest_piece_between_newlines` (`document.rs:702`) beside the three equivalence tests. Entry point `CodeDocument::new`, measured through `editor_pipeline_benchmark`. Final, ten rounds: `open_300kb_highlighted` 126.9 us against 153.2 us on `9f606d3aa` (-17 %), 163.6 on `f501e320`, 211.7 on `afcd1219`; `open_2mb_to_text` -13 %, `open_3_7mb` -16 %, `open_markdown_injected` -9 % against the old pin; same binary at most 0.3 %. Instructions: -29 to -36 % against the old pin.
- **Two US-013 editor costs, fixed (`8050f6cd`).** `keystroke_3_7mb_plain` ran 392 more instructions a keystroke than the old pin (each edit folded its row's bytes through the tab-stop width) and `reload_10_hunks_100k_lines_ui` read 3 to 5 % slower. `CodeDocument` records whether it holds only one-column ASCII and newlines (`one_column_text`, cleared by an insert at `document.rs:229`) and `line_columns` (`document.rs:320`) then takes the row's byte length; test `an_edit_that_brings_a_wide_character_or_a_tab_into_a_plain_document_measures_its_columns` (`document.rs:685`), red without the flag update. Final: -10 % and -11 % in time, -4.6 % in instructions for the keystroke. Two attempts without gain were dropped (a contiguous-slice fast path, a vectorizable fold). `textdiff_300kb_50_blocks` read +4 % on `afcd1219` and on `b42cd513` with the same instructions to 0.001 % and no code or dependency change since the old pin: placement, -0.5 % on the final commit. `to_disk_string_3_7mb` read +1.1 % with the same instructions; two later sets of the same binaries read the old pin 0.4 % and 1.9 % slower, so the gap changes sign between sets.
- **`publish_echo_220x60` and `pipeline_corpus_mib_s`, settled and fixed (`650569c4`).** Counted per case in copies of each side: +0.6 % and +2.5 % instructions against the old pin, from `cell_from_ghostty` going from 30.1 to 34.6 instructions per cell with `2fc672a1` (5 886 340 cells). The conversion was an outlined call; `#[inline(always)]` (`src-app/src/terminal/ghostty_session/convert.rs:148`) puts it in its two loops: -2.6 % and -4.6 % instructions against the old pin, echo -3.0 % in time and the pipeline 2.14 against 1.83 MiB/s. A lookup-table rewrite of the cell flags read worse (+6.2 %) and was dropped. Every terminal metric is at or below the old pin; the highest, `layout_220x60_acc60` +0.4 %, has 0.7 % fewer instructions and the old pin's path port reads +1.4 % on it.
- **Startup, re-taken without the leaked workers, then with its host.** A fresh-eyes re-judgment of the raw runs found the first set slower on two trace steps; taking that up found a harness defect: `scripts/bench-startup.sh` (and `.ps1`) built `paneflow-app` alone, so a fresh target launched no host and the Fedora machine's baseline of 2026-11-17 ran a host built on 2026-11-16. `e939a24c` builds the host with the app and the benchmark refuses an app without one (`src-app/src/startup_bench.rs`, `startup_first_frame_benchmark`, through `paneflow_host::bootstrap::resolve_host_executable`; red on a host-less app). Eight rounds with hosts: first frame 303.7, 296.6 and 301.2 ms against 303.0, 298.1 and 301.1 ms on the old pin, every total within the noise (same binary at most 1.1 %) or faster. `step_first_frame` is the wait for the compositor's first `xdg_surface.configure` while the app is idle (`WAYLAND_DEBUG=client`, 66 launches per pin: 26.1 ms after the initial commit on the old pin, 26.4 ms on the new). It changes sign by scenario and set except `stale_socket_step_first_frame`, slower in every set (+2.3 to +5.6 %, about 100 us); the bump alone (`988bd78f`) carries it. Zed `e20d19d32f` (#66002, one surface configure at creation, `vendor/gpui_wgpu/src/wgpu_renderer.rs:509`) reorders that first frame; undone in a measurement copy it brings `welcome` back (-0.1 %) but not `stale_socket` (+3.3 %) and makes every total 0.6 to 1.1 % slower than the old pin. **Open**: the `stale_socket` step is not explained. `restore3_step_app_state_built` switches between about 220 and 300 us with the binary file, not the code (the same commit from two paths reads 305 and 231 us in one set; the old pin 1 run in 44). `startup.json` re-recorded on a clean `e939a24c` (`ba24eac7`).
- **US-024's latency pair, re-measured on `650569c4`.** Five interleaved runs per pin, debug builds with the probe thresholds lowered: 3.21 ms on `9f606d3aa` and 3.29 ms on `650569c4` (p95 3.75 ms on both), recorded in `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`.
- **CI guard for the editor (`9c9f47c9`, `accbb4f1`, `53841f40`).** `scripts/perf-ab.sh` and `perf-ab.ps1` run the editor suite in the timed A/B and count 19 editor cases one by one under Callgrind (`editor_instruction_benchmark`, `src-app/src/app/diff_dock/code/perf_bench.rs:1328`, through `instruction_case` and the exported `paneflow_bench_counted_instructions`, `src-app/src/bench_harness.rs:295` and `:303`; `count_editor_instructions_side`, `scripts/perf-ab.sh:241`), judged with the terminal's blocking +2 % limit; two runs of the same binary agree on 18 cases exactly and on the 19th within 0.02 %. `PERF_AB_PROFILE` (`perf-ab.sh:16`) builds both sides in `release`, because the `gates` builds of both `f501e320` and `afcd1219` placed the loop the slow way (209.6 against 208.9 us). The comparator judged each run wrongly both ways and was fixed first: a metric whose own A/A drifts is no longer judged (`judged_regression`, `ab.rs:443`), and a regression verdict needs the same metric twice (`same_metric_regressed_twice`, `ab.rs:457`); tests `a_regression_is_confirmed_only_by_the_same_calibrated_metric_twice` and `a_drifting_metric_is_not_judged_and_leaves_a_calibrated_regression_standing`. Red: `PERF_AB_PROFILE=release scripts/perf-ab.sh f501e320 afcd1219` times `open_*` 32 to 36 % slower in both executions (A/A at most 0.8 %), `regression`. Green: `PERF_AB_PROFILE=release scripts/perf-ab.sh f501e320 650569c4`, `pass` on the first execution, `open_*` 14 to 21 % faster. The CI configuration (`gates`, `PERF_AB_TIMED_EDITOR=0`, `.github/workflows/perf-ab.yml:41`) with `53841f40` as head against `650569c4`: exit 1, instruction verdict `regression` (`editor::open_*` +47 to +62 %, `editor::keystroke_3_7mb_plain` +6.7 %). The CI job keeps the editor untimed to stay within its 60 minutes (46 used on 2026-11-17); `perf-ab.ps1` was reviewed by inspection only (no PowerShell on this machine).
- **Unstable test, fixed at its cause (`501be79b`).** `a_process_spawned_while_ptys_open_inherits_neither_end` read `/proc/<pid>/fd` of a child in the middle of `exec sleep 600`, and the kernel answers `EACCES` during an exec (a probe saw it in 5 of 500 windows, never without the exec). The child now stays in its first image (`read -r _` on a piped stdin, `crates/paneflow-host/tests/pty_descriptor_inheritance.rs:24`). Before: 14 failures in 300 and 9 in 200 runs under load; after: 0 in 400; a mutation that drops `O_CLOEXEC` still fails it.
- **Baselines and gates.** `scripts/perf-gates.sh --refresh-alloc-baselines` on `650569c4`, then `--set-baseline` of the three suites on the clean tree (CPU share 1.00 for terminal and editor), committed in `5a109f4c` on top of the code, the startup one again on `e939a24c` (`ba24eac7`); `bench/README.md` names `650569c4`. Allocation columns that moved: the two `paths_*` cases to 0 (`0f1304dc`), `keystroke_3_7mb_plain` 4.05 to 4.0 (`8050f6cd`), `pipeline_corpus_mib_s` 3.83 to 3.54 (bisected to `38726003`, grapheme clustering on). `scripts/perf-gates.sh` on `650569c4`: 116 budgets of 116 pass, none raised, `worker.idle.cpu_ms` 0 ms and `desktop.idle.root_renders` 0. The summary, `7c452a28`.

#### US-024: Keystroke latency and unfocused frame rate hold on Fedora
**Description:** As a Linux user, I want the frame policy of US-009 measured on a real desktop so that the bump keeps keystroke echo as fast as before and caps unfocused windows as it does on Windows.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-009, US-019

**Paneflow sites:** `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md` (the Windows figures and the Linux procedure); `src-app/src/frame_policy.rs`; `src-app/src/terminal/element/mod.rs` (the probe thresholds, lowered in the measurement copies only).

**Acceptance Criteria:**
- [x] Given a debug build of each pin on Fedora with `PANEFLOW_LATENCY_PROBE=1`, when keystroke latency is measured with the window focused by the procedure of the results file, then both results, the session and the driver are recorded and the medians differ by at most 2 ms
- [x] Given an unfocused window streaming terminal output on the new pin, when it redraws, then it presents at most 30 frames per second, recorded next to the Windows figures
- [x] Given the latency difference exceeds 2 ms, when the result is read, then the story is not marked done and the cause is investigated before any interval is changed
- [x] Given the bump summary of US-019, when it is read, then it carries the Linux figures

**Implementation evidence (2026-11-16):** `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`, section "Linux (US-024, 2026-11-16)": Fedora 45, GNOME 51.1 on Wayland, NVIDIA 620.70.06 (open kernel modules), GPUI on the RTX 4070 Ti SUPER through Vulkan; debug builds of `9f606d3aa` and `2f8a3d376` with the probe thresholds lowered in those copies only. At Arthur's request the keys were typed with `agent-desktop press`, which goes through Mutter's RemoteDesktop interface; since a GPUI window exposes no accessibility tree, the driver checked after every key that the test window logged it and would have aborted otherwise (no run did, every one of 2255 keys reached it). Five old-pin runs read a 3.05 ms median (p95 3.63 ms), six new-pin runs 3.12 ms (p95 3.70 ms): 0.07 ms apart, so the third criterion does not apply. Unfocused, with GNOME Calculator holding the focus (its AT-SPI `is_focused` read true before and after each window): the old pin presents 90.8 to 92.3 frames per second, the new pin 25.4 and 25.7, never more than 27 in a whole second. Runs where the calculator did not take the focus count for latency only and are named so. The bump summary carries the Linux figures under "Frame intervals" and "Keystroke latency". Not exercised: KDE Plasma, X11 and Mesa.

**Re-measured on the final commit (2026-11-18, US-027):** five interleaved runs per pin of the same procedure, debug builds of `9f606d3aa` and `650569c4` with the probe thresholds lowered in those copies only: 3.21 ms against 3.29 ms (means of the run medians, p95 3.75 ms on both), 0.08 ms apart; every key reached the test window (201 per run). The driver unset the `PANEFLOW_*` variables of the pane it ran from and the pane ran the login shell; both are noted in `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`, "Focused keystroke latency on the final commit (2026-11-18)". The unfocused rate was not re-measured: after `2f8a3d376` neither `src-app/src/frame_policy.rs` nor the GPUI pin changes, and the vendored `gpui_linux` commits (`243e8308` and the US-025 and US-026 fixes) touch no activation or frame-callback code.

**Status (2026-11-17): IN_REVIEW.** The criteria above hold from the 2026-11-16 measurement. The night run's renderer and terminal commits (`0f1304dc`, `7504a269`, `f7c68b02`, `dd56cfc8`, `aaac2784`, among others) came after it, so the latency pair is re-taken on the final commit as part of US-027's remaining work, by the same procedure; if the medians then differ by more than 2 ms, this story reopens.

#### US-025: Linux window, startup and backdrop behave on real sessions
**Description:** As a Linux user, I want window size, startup and the blurred backdrop checked on the desktops I use so that what US-016 and US-017 prove in code holds on screen.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-016, US-017

**Paneflow sites:** `scripts/bench-startup.sh`; `bench/baselines/linux-x86_64/startup.json`; `src-app/src/window_state.rs`; `src-app/src/app/constants.rs`; `src-app/src/window_chrome/linux_backdrop.rs`.

**Acceptance Criteria:**
- [x] Given a release build on Linux x86_64, when time to first frame is measured with `scripts/bench-startup.sh` on both pins, then the two numbers are recorded and the new one is within 10 % of the baseline
- [x] Given a window toggled to fullscreen and back, when it returns to windowed on each session exercised, then it keeps its size and the restored size after a restart is the windowed one
- [x] Given the backdrop setting on "blurred", when the window opens on a compositor with `ext-background-effect`, on one with KDE blur only if available, on GNOME without either, and on X11, then the result on each matches US-017's criteria, with the compositor and driver named
- [x] The story states which of Wayland, X11, NVIDIA and Mesa were exercised and which were reviewed by inspection only

**Progress (2026-11-16), story BLOCKED on sessions only Arthur can open:** startup is in the bump summary (US-027): first-frame medians of 307 to 315 ms on `d78d92927` and 306 to 322 ms on `9f606d3aa`, against 312 to 315 ms in the baseline. Backdrop, debug build, isolated home, `{"window_backdrop": "blurred", "linux_chrome_material": true}`, NVIDIA driver: on GNOME 51.1 Wayland, which advertises `ext_background_effect_manager_v1` (`wayland-info`), the log carries no fallback warning and Arthur saw the sidebar blurred with rounded corners; as an XWayland client of the same session, the log says once that the session offers neither protocol and the window stays translucent. GNOME 51 offers `ext-background-effect`, so "GNOME without either" needs an older GNOME and is not available on this machine. Not run: KDE Plasma (KDE blur), a real X11 session, Mesa, and the fullscreen round trip, which needs a desktop shortcut or a window manager menu that Paneflow does not offer. The same check found square right corners on the floating Classic window, on the old pin too; Arthur chose to fix it on the branch, `1100a278` (`flush_corner_radii` in `src-app/src/layout/density.rs`, `window_corners_under_the_panel` in `src-app/src/window_chrome/title_band.rs`, `rounded_window_corners` in `src-app/src/window_chrome/csd.rs`, read from `PaneFlowApp::render`), and Arthur checked the corners round on the terminal and in Settings on GNOME Wayland with the material on. The corner change also reaches Windows, whose shell draws the same client-side corners; it was reviewed there by inspection only.

**Implementation evidence (2026-11-17, night run):** every session ran nested inside Arthur's GNOME 51.1 Wayland session, through the harness `/home/arthur/dev/.pf-ep007/vp/` (local evidence, untracked): a private `PANEFLOW_HOME`, `XDG_*`, D-Bus and runtime dir per session, driven and captured inside the nested server (captures under `~/Images/Captures d’écran/ep007/`, local). Sessions exercised: sway, labwc and cage (wlroots) on NVIDIA, RADV and lavapipe; kwin_wayland 6.8.3 nested, and headless (`--virtual`) for scale 2, on NVIDIA, RADV and lavapipe; mutter 51.1 and gnome-shell 51.1 headless on NVIDIA, lavapipe and RADV; rootful Xwayland with openbox and picom (server decorations, and client decorations once `_GTK_FRAME_EXTENTS` is advertised) on NVIDIA; Xephyr with openbox on lavapipe. Not exercised: a real KDE Plasma or GNOME X11 session (none installed), a GNOME without `ext-background-effect` (GNOME 51 offers it), and RADV on X11 (it fails with "Invalid surface"; out of scope).
- Fullscreen round trip: pass on sway, labwc, GNOME Shell and X11 (server and client decorations), including after a restart. KWin lost the 2 x 10 px client inset on a round trip (the compositor switches to server decorations in fullscreen and Paneflow declared a zero inset), fixed in `26643af9` (`declared_client_inset` in `src-app/src/window_chrome/csd.rs`, `restorable_size` in `src-app/src/window_state.rs`, tests `a_fullscreen_window_keeps_declaring_the_inset_it_returns_to`, `the_restorable_size_adds_the_client_frame_whatever_inset_the_window_reports`, `a_tiled_window_records_no_restorable_size`): KWin 1180x780 frame, fullscreen and back, `window-state.json` 1200x800 (1180x780 before). A window tiled at quit no longer records its tiled size (KWin quick tile, 1290x1400, keeps 1200x800); under server decorations GPUI exposes no tiling state, so a tiling window manager re-tiles the window anyway.
- Backdrop "blurred" with `linux_chrome_material` on and off: GNOME Shell 51.1 and KWin 6.8.3 both offer `ext_background_effect_manager_v1` and no `org_kde_kwin_blur_manager` (KWin 6.8.3 no longer ships KDE blur at all), so "KDE blur only" is not available; sway and labwc offer neither and Paneflow logs the fallback once and stays translucent; X11 logs the fallback once and stays translucent. Floating corners round, maximized and tiled corners square on both blur compositors. `1f20785b` stops requesting blur behind an opaque window (`an_opaque_window_asks_for_no_blur_behind_it`). Residual on KWin only: KWin offsets the region by the window inset although the protocol says surface-local coordinates (`ext-background-effect-v1.xml:109`), leaving a 10 px unblurred band top and left and a blurred square past the bottom-right corner; GPUI sends the right region and GNOME Shell draws it right. Upstream KWin bug, listed for Arthur.
- RADV forced while GNOME composites on NVIDIA: the window now opens. Mutter rejects the cross-GPU buffers Mesa creates with `create_immed` and closes the whole connection (`meta-wayland-dma-buf.c`, `buffer_params_create_common`). `20e9d3ca` rejects an adapter whose surface configuration loses the device (Mesa's software WSI path made GPUI pick RADV and panic at the first frame); `d3011b18` probes the compositor on a throwaway Wayland connection before using a Vulkan GPU that is not the compositor's; `80cfbbe3` renders such a GPU offscreen and presents through `wl_shm`; `f239caac` logs the probe's expected rejection at the info level; `d0ece8ad` makes the failure message name a forced GPU. Proven on screen on nested Mutter and GNOME Shell (typing, resize, Settings, scale 2), and on Arthur's GNOME session 4 launches out of 4 (log "copied into shared-memory buffers"); sway and KWin with RADV keep the GPU swapchain. Cost: 2.9 to 3.6 ms per frame at 1440x900 on the `wl_shm` path against about 1.0 ms on a swapchain; the probe adds about 13 ms to window creation (debug, 6 interleaved runs, 266.0 against 278.7 ms) only when the GPU is not the compositor's, and nothing on the common path (458.1 against 455.2 ms).
- X11 minimum size: `432c8815` writes WM_NORMAL_HINTS in device pixels plus the client inset (820x520 with client decorations at scale 1, 1640x1040 at scale 2, 800x500 with server decorations).
- Wayland and X11, NVIDIA and Mesa (RADV, lavapipe) were exercised on screen as listed; macOS and Windows were reviewed by inspection only for the shared changes (`window_state.rs`, `csd.rs`), which reduce to the old behavior there (server decorations, zero inset).

#### US-026: The Linux visual pass is clean
**Description:** As the maintainer, I want every surface the stories changed looked at on Linux so that the renderer rework ships without a visible defect on the desktops Paneflow supports.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-020, US-024, US-025

**Paneflow sites:** the per-platform checklist written by US-020; `DESIGN.md` section 9 (the UI delivery gate).

**Acceptance Criteria:**
- [x] Given the checklist of US-020, when Arthur runs its Linux part on the sessions of this epic, then each surface and state is marked pass or defect, with the session and driver named
- [x] Given the `DESIGN.md` UI delivery gate, when the pass runs, then Dark, Light and one other preset, `reduce_motion` on and off, the 800 by 500 minimum window and every control state are covered
- [x] Given a defect, when it is found, then it is fixed on the branch with the local gates green and the surface is checked again before the story is done
- [x] Given the pass is done, when EP-008 starts, then no Linux defect found in this epic is open

**Implementation evidence (2026-11-17, night run):** the pass ran through the harness and sessions named under US-025, in five sub-agent passes (one per backend group, then a coverage round for the items no backend pass could trigger), each returning only defects and capture paths; 897 captures under `~/Images/Captures d’écran/ep007/` (local, untracked), named `<session>-<driver>-<preset>-<style>-<surface>`. The consolidated defect list with every capture path is `/home/arthur/dev/.pf-ep007/vp/defects.md` (local evidence, untracked).
- Coverage of the `DESIGN.md` gate: Paneflow Dark, Paneflow Light and Claude Dark (228 captures), Rounded and Classic, `reduce_motion` on and off (spinners, clone bubble, dialogs; under `reduce_motion` the spinners are strictly static, identical frame hashes), scale 1 everywhere and scale 2 on KWin headless and sway (31 captures), 1440x900, 1200x800, 1000x650 and the 800x500 minimum, and the control states hover, pressed, focused, open, disabled, keyboard-highlighted. Tools built for what the stock tools could not drive: a private `HOME` with usage fixtures (`VP_APP_HOME`), a fake file-chooser portal (`VP_PORTAL=1`), a `zwp_input_method_v2` client that maps the candidate popup at the reported cursor rectangle (`vp-ime`), a wlr virtual pointer for drags (`vp-wltool pointer-script`), a KWin script driver, and an ibus XIM recipe for X11.
- Checklist items (US-020, Linux and every-platform groups): US-005 rings at 0, 45 and 100 % and the clone check mark, pass after V2, V3, V4; US-006 cards, dialogs, menus, select popups, quick pick, path picker, search overlay, tooltips, focus ring, tab badges, pass after K1-K8, K11, C4, T5, V5; US-007 shadows and gradients incl. the drag ghost and detached window capsule on wlroots and X11, pass, no banding seen with the ordered dither; US-011 spinners smooth and in one phase, pass after V1; US-013 tabs in the code editor and diff, IME after a tab on Wayland and X11, pass after K13, V9 (V10 not a defect: the removed range is one space, 1 cell, the added range two tabs, 6 cells, both from the same x); US-014 wrapping bodies, pass after K6, K7, V5 (V7 not a defect: the "/" a path starts with may end a line under UAX #14, and the decision of 2026-11-16 adds no break points of Paneflow's own); US-015 combining marks and emoji in a terminal, pass after T4, T7 and the Devanagari mid-run fix, sidebar title monotonic under a pixel-by-pixel drag, pass after K14; US-016 and US-017 under US-025; US-018 submenus near the window edge with the CSD inset, pass.
- Defects found and fixed, each with a test failing without it where a seam exists, and each re-checked on screen on the combination that showed it: diff scrollbar `dfc942a2`, `b4b791fa`; terminal T1 `2fc672a1` (and `f7c68b02` for its publish cost), T2 `42991848`, T3 `83272b9c`, T4 `e2f9a4a0`, `03485d78`, T5 `b5c18b32`, T6 `dbfa2777`, T7 `38726003`, IME in terminals `aaac2784`; chrome C1 `95d177cb`, C2 `a34f1169`, C3 `1f20785b`, C4 `b213c533`, C5 `014214a3`, C6 `432c8815`, C7 `3c292944`, Classic corners `1100a278`, window name `5b09df7b`; controls K1 `eb8fe9e5`, K2 `5583c3b4`, K3 `095e9356`, K4 `dd043fe9`, K5 `7ac4420c`, `a29ba345`, K6 `2430e6d5`, K7 `bf155772`, K8 `8efc0951`, K9 `8a0a4fee`, K11 `bd1dac99`, K13 `f71a4f33`, K14 `dd0ee512`; coverage V1 `dd56cfc8`, V2 `6898211c`, V3 `f8958f61`, V4 `d358cce5`, V5 `5bdc81fb`, V8 `99f8e90e`, V9 `165c11eb`; window and GPUI M1, M2 `26643af9`, M3 `243e8308`, `cc540048`, M5 `20e9d3ca`, `d3011b18`, `80cfbbe3`, `f239caac`, M6 `ba47d8cf`, `4acbb6b0`, M7 `2d6b436a`. Not defects, with proof in the defect list: K10 (the 59 % arc's start cap is a radius-3.5 semicircle, checked on an 8x zoom), K12 (`DESIGN.md` section 5.4 specifies Zed's 15 px tracks with square thumbs for the code editor), M4 (Mutter drops the first click after a pointer appears, GPUI is not involved), V6 (the modal card covers the row; the terminal re-wraps to 33 columns under it).
- Open: C3's residue is a KWin 6.8.3 bug (the region is offset by the window inset against `ext-background-effect-v1.xml:109`); GPUI sends the right region and GNOME Shell draws it right. Listed for Arthur as an upstream KWin report.
- Not exercised, with the reason: a real KDE Plasma or GNOME X11 session and a real X11 window manager that advertises `_GTK_FRAME_EXTENTS` (none installed; `kwin_x11` is absent and Mutter 51 has no X11 session; the property was spoofed on openbox, which exercises Paneflow's client-decoration path but not a real manager's handling), fcitx5 (not installed; ibus covered X11 and a protocol client covered Wayland), drags on KWin and Mutter (the drop veil and ghost were covered on KWin earlier and on wlroots and X11 in the coverage round), macOS and Windows (out of this epic).

**Status (2026-11-16, superseded by the evidence above): BLOCKED, Arthur's pass.** Not started: the pass needs Arthur on each session of this epic. One Linux defect is already found and fixed (`1100a278`, the square right corners of a floating Classic window, under US-025); the pass rechecks it on every session, with the material off as well.


**Review evidence, EP-007 (2026-11-18):** certified `DONE` as one unit on `6a31c8c9`. Reachability, read from the execution roots: `.github/workflows/perf-ab.yml` runs `scripts/perf-ab.sh` with `PERF_AB_INSTRUCTIONS_BLOCKING: "true"` and `PERF_AB_TIMED_EDITOR: "0"`, and the script counts `editor_instruction_benchmark` on both sides (`count_editor_instructions_side`, `perf-ab.sh:241`, called at `:323-324`) and judges through the corrected comparator (`judged_regression`, `same_metric_regressed_twice`); `longest_byte_line` is the newline split of `measure_text_lines`, which `CodeDocument::new` calls for every opened file; `line_columns` takes the byte length while `one_column_text` holds and `insert` clears it; `cell_from_ghostty` is inlined into the two publish loops that call it; the startup benchmark's host guard is reached from both of its callers, `scripts/bench-startup.sh` (and `.ps1`), which now build the host, and `scripts/perf-gates.sh:130`, whose `target/gates` holds the host it builds at `:98`. US-024: the frame-policy evidence holds, as its re-measure note states. US-025 and US-026: the criteria are proven by the night run's recorded sessions (897 captures and the defect list, local evidence), every defect fixed on the branch with its commit; the KWin region offset is an upstream KWin bug, not a Paneflow defect. A fresh-eyes re-judgment of the raw runs agreed on the `open_*` metrics, the `perf-ab` proofs and the latency pair, and its two disagreements were taken up: `to_disk_string_3_7mb` (sign changes between sets) and the startup steps, which found the host-less startup harness (`e939a24c`). Left open for Arthur, below every criterion's 10 % line: `stale_socket_step_first_frame` +2.3 to +5.6 % (about 100 us of compositor wait, not explained after three approaches) and `restore3_step_app_state_built`'s two modes. Gates on `6a31c8c9`: `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -D warnings`, `cargo nextest run --workspace --locked --profile ci` (4885 of 4885), `scripts/check-helper-sizes.sh`, `cargo deny check advisories licenses sources`, `scripts/check-workflows.sh`, `zizmor`, `scripts/test-ci-scripts.sh` (198 cases), `scripts/check-test-wallclock.sh` and `scripts/perf-gates.sh` (116 budgets of 116) pass. Windows: the `cfg(windows)`-free changes of this epic (`perf-ab.ps1`, `bench-startup.ps1`) were reviewed by inspection only.
---

### EP-008: The whole CI is green before merge

The branch is pushed once the local epics are done, and every failure the CI reports is fixed on the branch until every workflow run is green on the commit that merges. This is the only epic that waits on CI, and it is expected to be long.

**Definition of Done:** on the head commit of the pull request, every required `run_tests` job and every other workflow the branch triggers is green, with no budget raised, no test quarantined for this work, and the records of EP-006 still true for the code that merges.

**Working rule:** push, wait for the whole run, read every failed job, fix all failures from that run in one pass with the local gates green, then push again. A fix is proven locally first wherever the platform allows; a failure is never retried blindly. A failing test is rerun once only to tell a flake from a regression, as `AGENTS.md` requires.

**Zed references:**
- `zed/crates/gpui_windows/build.rs`, `zed/crates/gpui_apple/src/display_link.rs:60-62`, `zed/crates/gpui_linux/src/linux/wayland/window.rs:21`: the platform facts the three legs must confirm.

#### US-021: Every required run_tests job is green on the branch head
**Description:** As the maintainer, I want every `run_tests` job green on the commit that merges so that the code gated by `cfg` on each platform, and macOS above all, is proven.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by every story of EP-001 to EP-007

**Paneflow sites:** `.github/workflows/run_tests.yml:181` (Style), `:228` (Tests Linux x86_64), `:298` (Performance gates), `:430` (macOS aarch64 smoke build), `:645` (macOS render smoke), `:853` (Windows x86_64 libghostty check), `:1034` (Windows render smoke), `:1255` (Linux aarch64); `.config/nextest.toml`.

**Acceptance Criteria:**
- [x] Given the pull request head commit, when `run_tests` completes, then Style, Tests (Linux x86_64), Performance gates, Linux aarch64, macOS aarch64 smoke build, macOS render smoke, Windows x86_64 libghostty check, Windows render smoke, cargo-deny and `tests_pass` are green
- [x] Given a `cfg(target_os = "macos")`, `cfg(unix)` or `cfg(windows)` item fails on its leg, when the failure is read, then it is fixed in this story, and the report states per platform whether each fix was verified by compilation or by inspection
- [x] Given a test fails in CI, when it is rerun once, then a pass is reported as a flake with its tracking issue and a second failure is fixed; no test is added to the quarantine in `.config/nextest.toml` for this work
- [x] Given the Performance gates job fails, when the failure is read, then a counter below its baseline is refreshed from a clean clone of the committed code, and a budget exceeded is fixed in code or put to Arthur with the measured value; no budget in `crates/paneflow-host/tests/persistent_baseline/gates.rs` is raised without his approval
- [x] Given a CI fix changes code that EP-006 or EP-007 measured or documented, when the fix lands, then the affected suites are re-run and the bump summary, `CHANGELOG.md`, `DESIGN.md` or `ARCHITECTURE.md` are updated to match

**Implementation evidence (2026-11-18):** the branch was already pushed, with pull request #160 open. Run `44257712767` on `510cda7c` (the EP-007 head) failed two jobs and passed the rest, Performance gates included (116 budgets, 0 failed). Style: `scripts/check-ci-filter-coverage.sh` reported the shaders `vendor/gpui_wgpu` includes, and the font fixtures of its upstream test module, as uncovered by the `rust` path filter; `vendor/**` was missing from the filter, so a change confined to a vendored crate skipped every Rust job. `04bc4491` adds `vendor/**` and reports a citation that names no file in the repository as a notice, since no build that succeeds reads it (the three `assets/fonts/...` paths of `gpui_wgpu`'s `cosmic_text_system.rs` tests, a module that never compiles here). macOS aarch64 smoke build: `a_title_measured_by_the_platform_text_system_never_flips_at_its_fitting_edge` panicked on both attempts at `gpui_macos/src/platform.rs:220` ("Mac platform not created on main thread"), also on `d654b269`, so a regression, not a flake: `MacPlatform::new` asserts the main thread and libtest runs each test on a spawned one. `0252294d` builds `gpui_apple::AppleTextSystem` with `font-kit` on macOS, the text system `MacPlatform::new` owns (`zed/crates/gpui_macos/src/platform.rs:224`), through a macOS-only dev-dependency that adds no package to `Cargo.lock`; the other platforms keep `gpui_platform::current_platform(true).text_system()`. Verified per platform: the macOS fix by compilation and the test passing on the `macos-15` leg, the Linux path by compilation and the test locally on Fedora, Windows by its CI leg; the filter fix by running the script locally. Run `44263039113` on `0252294d`: every job green, Style, Tests (Linux x86_64) 4910 passed, Performance gates 116 budgets with 0 failed and no baseline refreshed, Linux aarch64 4910 passed, macOS aarch64 smoke build 4733 passed (the CoreText test among them), macOS render smoke, Windows x86_64 libghostty check 4770 passed, Windows render smoke, cargo-deny, the Workflow audit and `tests_pass`. No test retried or reported FLAKY on any leg, and `.config/nextest.toml` and `gates.rs` are unchanged against `main`. The two fixes touch a test and the CI filter, nothing EP-006 or EP-007 measured or documented, so no suite was re-run for them. Local gates on the final code: `cargo fmt --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, the two `agents_section` title tests, `scripts/test-ci-scripts.sh`, `scripts/check-test-wallclock.sh`.

#### US-022: The other workflows the branch triggers are green
**Description:** As the maintainer, I want every workflow that runs on the branch to pass so that nothing outside `run_tests` blocks the merge or a later release.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by every story of EP-001 to EP-007

**Paneflow sites:** the `libghostty Linux`, `libghostty macOS` and `libghostty Windows` workflows; `perf-ab`; the Workflow audit job (`zizmor`); `scripts/check-workflows.sh`.

**Acceptance Criteria:**
- [x] Given the pull request head commit, when every workflow triggered on the branch completes, then each one is green or skipped by its own path filter
- [x] Given a change under `.github/**` in this branch, when it is checked, then `bash scripts/check-workflows.sh` and `uvx zizmor@1.32.0 --config .github/zizmor.yml --min-severity high .github` pass locally before the push
- [x] Given `perf-ab` reports a regression, when it is read, then its counter and measured value are recorded in the bump summary with the cause, or the regression is fixed

**Implementation evidence (2026-11-18):** on `0252294d`, `libghostty Linux`, `libghostty macOS`, `libghostty Windows`, `perf-ab` and `run_tests` (Workflow audit included) are green. Before the push of `04bc4491`, the only `.github` change of this epic, `bash scripts/check-workflows.sh` and `uvx zizmor@1.32.0 --config .github/zizmor.yml --min-severity high .github` passed locally (no findings). `perf-ab` passed with a warning: a confirmed timed regression of the merge commit `93e00209` against `main` on four terminal service metrics (+17 to +30 % at p50), instruction counts `pass`. 30 interleaved rounds on Fedora in both profiles show the shipped `release` profile 9 to 14 % faster on the service metrics and the CI's `gates` profile 9 to 11 % slower, A/A within 0.3 %: a code placement effect of `650569c4` under 16 codegen units. The counters, the measured values and the cause are in the bump summary, section "The perf-ab verdict on the pull request (US-022)", with the raw rounds in `bench/results/gpui-703b5ac3-perf-ab-profiles-2026-11-18.json`.

#### US-023: The branch is ready to merge
**Description:** As the maintainer, I want the pull request to state exactly what was proven where so that I can merge without re-checking the evidence.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-021, US-022

**Acceptance Criteria:**
- [x] Given the head commit, when `cargo fmt --check` runs on it, then it passes, and it is the commit on which US-021 and US-022 are green
- [x] Given the pull request body, when it is read, then it lists the validation actually run per platform (local Windows, WSL Linux, each CI leg), the user-visible behavior, the visual-pass checklist of US-020, and links the bump summary
- [x] Given the branch, when its history is read, then commits are atomic per story in the `type(scope): US-NNN - description` form, with no AI attribution and no closing keyword
- [x] Given macOS latency is not measured (US-010 cancelled), when merge readiness is stated, then the pull request names it as a known limitation with a link to zed-industries/zed#66954; it does not block the merge

**Implementation evidence (2026-11-18):** the body of #160 was rewritten from the EP-001 text: the validation actually run on the Windows host, in WSL Linux, on the Fedora sessions of EP-007 and on each CI leg; the user-visible behavior with `CHANGELOG.md` `[Unreleased]`; the US-020 checklist by platform, with what Windows still leaves open; a link to the bump summary and its perf-ab section; macOS latency as a known limitation with a link to zed-industries/zed#66954, not blocking. History: the 119 commits before the two that record this evidence, every one in the `type(scope): US-NNN - description` or `EP-NNN` form except the two commits that wrote the PRD; no merge commit, no `Co-authored-by` or generated-by line, no closing keyword. The head commit records this evidence and touches only `bench/results/` and `tasks/`, which the `rust` filter skips, so `run_tests` is dispatched on it by hand (`workflow_dispatch` sets every filter output) and the pull request body names that run; the first criterion is ticked by the review once that run is read, since a commit cannot record its own CI result.

**Review evidence, EP-008 (2026-11-18):** certified `DONE` as one unit on `6e9935e9`, the head of #160. Both runs on it were read job by job: the pull request run `44275077720` (on the merge commit `88a51615`) and the dispatched run `44275097861` (on the branch head) are green on Style, Tests (Linux x86_64) 4910 and 4885 passed, Performance gates (116 budgets, 0 failed), Linux aarch64 4910 and 4885, macOS aarch64 smoke build 4733 and 4708 (`a_title_measured_by_the_platform_text_system_never_flips_at_its_fitting_edge` among them, on CoreText), macOS render smoke, Windows x86_64 libghostty check 4770 and 4746, Windows render smoke, cargo-deny, the Workflow audit and `tests_pass`; the 25-test gap between them is `main`'s, which only the merge commit carries. `libghostty Linux`, `libghostty macOS`, `libghostty Windows` and `perf-ab` are green on `6e9935e9`. No FLAKY or retried test in either run; `.config/nextest.toml`, `gates.rs` and `scripts/wallclock-allowlist.txt` are unchanged against `main`. Reachability: `vendor/**` sits in the `rust` filter of `run_tests.yml` that `changes` reads, and `scripts/check-ci-filter-coverage.sh` runs in Style (`run_tests.yml:210`); `platform_text_system` is called by the CoreText title test, and the `gpui_apple` dev-dependency is resolved in `Cargo.lock` with no new package. The perf-ab table of the bump summary was recomputed from `bench/results/gpui-703b5ac3-perf-ab-profiles-2026-11-18.json` (median per-round ratio over 30 rounds) and matches every cell. History: 121 commits by one author, no merge, no attribution or closing keyword, every commit in the story or epic form except the two that wrote the PRD. Fixed by the review: the body of #160 linked the bump summary by a relative path, which GitHub resolves under `/pull/` to the new pull request page, so both links now name the file at `6e9935e9` (US-023, second criterion); `scripts/test-ci-scripts.sh` had no case for `check-ci-filter-coverage.sh`, so four cases now cover a covered citation, an absent one (a notice, counted in the summary) and a present one outside the filter, and they fail when the absent-file branch is removed; the notice said "no build that succeeds reads it", which is false for `src-app/src/assets.rs`'s `target/embed/bin`, a build output absent from a CI checkout, so it now says the filter has no tracked input to watch; `AGENTS.md` said GPUI is pinned across four `rev` values while `src-app/Cargo.toml` holds seven, the `gpui_apple` one added here, so it now says every `rev` value, dev-dependencies included. Gates on the final state: `cargo fmt --check`, `scripts/test-ci-scripts.sh` (202 cases locally, without `pwsh`), `scripts/check-ci-filter-coverage.sh`, `cargo deny check advisories licenses sources` and `scripts/check-test-wallclock.sh` pass; the review changed no Rust, so clippy and nextest stand on the CI runs of `6e9935e9`. The commit that records this review touches `scripts/`, `AGENTS.md` and `tasks/`: its pull request run re-runs every job and must be green before the merge. macOS and Windows: proven by their CI legs only; macOS latency remains the known limitation (zed-industries/zed#66954).

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
- **Latency:** keystroke latency within 2 ms of the old pin on Windows and Linux; an unfocused window presents at most 30 frames per second. macOS is not measured until a Mac is available.
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
| 11 | Upstream regression confirmed | zed-industries/zed#66954 confirmed or fixed upstream, or a macOS user reports added latency | A later GPUI bump takes the fix | None |
| 12 | Platform leg fails | A `cfg`-gated item breaks on a CI leg not compiled locally | Fixed in EP-008 before merge; the report states what was compiled where | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | macOS input latency regresses on 120 Hz displays (zed-industries/zed#66954, open) | Med | High | Accepted: no Mac before 2027. CI compiles and tests macOS (EP-008), the upstream issue is tracked, a later GPUI bump takes its fix, and the hardware measurement is redone once a Mac is available |
| 2 | Per-paint path building costs more than the frame allows | Med | High | US-003 measures first; US-004 carries a bounded origin-keyed cache as the fallback and a benchmark that guards the budget |
| 3 | Seams between adjacent paths on bordered surfaces | High | Med | US-006 removes coincident edges by construction; US-005 collapses multi-path shapes into one stroke |
| 4 | The renderer rework is three days old upstream and may receive fixes after `703b5ac3` | High | Med | Work stays on a branch; re-pinning to a later commit before merge is one manifest change plus the gates |
| 5 | macOS-only code fails to compile, unseen from Windows and Linux | Med | Med | Stories review macOS items by inspection; EP-008 requires the macOS CI legs green before merge and is sized for late failures |
| 10 | CI failures pile up because no epic before EP-008 runs CI | Med | Med | Every story passes the local gates on Windows and WSL Linux; EP-008 batches each CI run's failures into one fix pass |
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
- **Migration:** rollback is one revert of the branch; no user data or configuration format changes. A macOS latency regression, if confirmed later, is fixed by a later GPUI bump; no vendored `gpui_macos` patch is planned (the fork was dropped on 2026-08-26, `e333c2397`).

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| GPUI commits behind upstream `main` | 65 (pin `37faef81`) | 0 at merge | Month-1 | `git rev-list --count` over the GPUI crates in `C:\dev\zed` |
| Compile errors against current GPUI | 35 binary, 54 test | 0 | Month-1 | `cargo check --workspace --all-targets` |
| Tests passing on the Windows leg | 4544 run on the old pin | 100 % of the suite, 0 quarantined for this work | Month-1 | `cargo nextest run --profile ci` |
| Squircle build cost, 12-badge strip | N/A (translated cache) | at or below 0.4 ms per frame | Month-1 | The benchmark case added by US-004 |
| Spinner call sites that re-render a view per frame | 2 | 0 | Month-1 | Render-count test of US-011 |
| Tab width in the editor on Windows | 0 columns | 4-column stops, same on three platforms | Month-1 | Expansion test of US-013 |
| macOS keystroke-to-present at 120 Hz | Not measured: no Mac before 2027 | within 4.2 ms of the old pin | Once a Mac is available | Hardware measurement recorded in a later bump summary |
| Days from upstream commit to pinned | 19 (2026-10-27 to 2026-11-15) | 14 or fewer for the next bump | Month-6 | Dates in `bench/results/` summaries |

## Open Questions

- **Re-pin before merge?** If upstream fixes zed-industries/zed#66954 or follows up on the path renderer before this branch merges, should the pin move to that commit instead of `703b5ac3`? Arthur, at the start of EP-006; decides whether US-002, US-019 and EP-008 run once or twice.
- **Low-power frame limit.** The PRD keeps GPUI's 30 fps limit in power-saving mode, which also limits keystroke echo there. If US-009 shows a perceptible cost, should `low_power_frame_interval` become `None`? Arthur, after US-009; changes one constant and one changelog entry.
- **Tab width.** Is a fixed 4 columns acceptable until a setting exists, including for Go and Makefiles where 8 is conventional? Arthur, before US-013.
- **macOS hardware.** Resolved 2026-11-15 (PRD 1.2): no Mac is available before 2027. US-010 is cancelled, macOS is validated by CI only, and the measurement is redone once Arthur has a Mac.
- **Phase of spinners.** All spinners now share one app-wide phase. Is that acceptable against `DESIGN.md:493-510`, or must each loader start at its own zero? Arthur, at US-011.
- **Accessibility of cached views.** If zed-industries/zed#66690 reproduces in Paneflow, is it accepted as an upstream defect or worked around by disabling the cache on affected views? Arthur, at US-012.
[/PRD]
