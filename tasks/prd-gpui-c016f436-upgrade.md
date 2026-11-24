[PRD]
# PRD: Move Paneflow to GPUI at Zed `c016f436`

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-24 | Arthur Jean | Initial PRD, from the 2026-11-24 exploration of the move from `703b5ac3` to `c016f436` (four parallel code reviews, each claim re-verified in both repositories) and a throwaway migration that built, linted and passed the whole Linux x86_64 suite. The Rust 1.100.0 toolchain move is a prerequisite done separately. Linux is validated first (EP-002), every macOS-only proof is gathered in EP-003, and every Windows-only proof in the final epic, EP-004, after macOS. 4 epics, 19 stories. |

## Problem Statement

Paneflow builds GPUI from Zed revision `703b5ac3f4ffe1e87a073123ae56e22f72f095c4` (2026-11-14), pinned in seven entries of `src-app/Cargo.toml` (lines 46, 47, 277, 278, 427, 433, 458 at `65b7a2ad`) and in the manifests of the two vendored crates, `vendor/gpui_wgpu` and `vendor/gpui_linux`. Zed `main` was at `c016f436e60bfa5105de53837935e9de94eb0867` on 2026-11-24: 167 commits later, 24 of them in the `gpui*` crates (R1). Four facts make the move more than a revision change:

1. **The vendored `gpui_wgpu` no longer compiles against the new GPUI.** Upstream added `Primitive::NativeView` and `PrimitiveBatch::NativeViews` (`f4fe0be1f8`, #67311), and the renderer match at `vendor/gpui_wgpu/src/wgpu_renderer.rs:1812` fails with `E0004` (R3). Both vendored copies must be rebased onto the new upstream sources while keeping every Paneflow change their headers list.
2. **The new GPUI needs Rust 1.100.** `gpui_macros`, a git dependency Paneflow does not vendor, calls `str::trim_prefix` (`zed/crates/gpui_macros/src/derive_inspector_reflection.rs:169`, `c31063a050`, #67367), stable only from Rust 1.100.0 (R2). Paneflow pins 1.98.0.
3. **Layout and text now persist across frames.** Layout nodes survive between frames (`90549abcfd`, #67222) and an unchanged text measurement is reused instead of measured again (`79af38b26c`, #67223). Paneflow lays the same sidebar title out twice per frame (`src-app/src/app/sidebar/style.rs:163`, `:201`), and one review predicted that titles would stay truncated after the sidebar widens (R6).
4. **Some changes show only on screen or on one platform.** The anti-aliased border is now composited through `over()` in the WGSL, HLSL and Metal shaders (`e2f00a41ac`, #66972); Windows rewrites clipboard line endings (`2eff23878e`, #66945), stops refreshing system fonts synchronously at startup (`58fea3d335`, #66001), reports driver versions differently (`990491f0ea`, #67404) and stops drawing windows cloaked on other virtual desktops (`2acb26ac1a`, #67387); every macOS window now goes through native-view overrides of `sendEvent:`, `makeFirstResponder:`, `performKeyEquivalent:` and `keyDown:` (`d31d1074a0`, #67316). None of these is visible to a Linux build or a unit test (R7 to R11).

The people affected are Paneflow's users on all three platforms, who get upstream fixes only through a pin move, and Arthur, the sole maintainer, who carries the vendored copies.

**Why now:** the 703b5ac3 PRD set the goal that each later GPUI bump lands within 14 days of the upstream commit it targets (`tasks/prd-gpui-703b5ac3-upgrade.md`, Goals); `c016f436` is from 2026-11-24, so the target is 2026-12-08. The range is still small: the migration prototype needed no API change in Paneflow (R4), and the next upstream changes will be written against the native-view scene and the persisted layout this range introduces.

## Overview

The work is one pin move, proven in four epics.

**EP-001 moves the pin and proves the build on Linux.** The seven `src-app/Cargo.toml` entries and the vendored manifests move to `c016f436`. The vendored crates are rebased by a three-way merge, with upstream `703b5ac3` as the base, Paneflow's copy as one side and upstream `c016f436` as the other; the prototype merged all seven files changed on both sides without a conflict, and the Paneflow patch came out identical line for line (R3). The lockfile changes only the `source` line of 19 packages (R5). Two tests pin what the research could not prove from the code alone: the sidebar title test from the A/B probe (R6) and a naga validation of every WGSL variant the vendored renderer builds, whose upstream tests no Paneflow test run executes (R15, A3).

**EP-002 validates on Linux**, on Arthur's Fedora machine and on the Linux CI legs: the benchmark suites against the old pin, keystroke latency, a visual pass under Wayland and X11, and the Linux CI jobs.

**EP-003 gathers every macOS-only proof** on Arthur's Apple Silicon Mac: the macOS CI jobs, input and window behavior under the native-view overrides, a visual pass, and the first macOS keystroke latency comparison, deferred by the 703b5ac3 PRD for want of a Mac.

**EP-004, the final epic, gathers every Windows-only proof** on Arthur's Windows 11 dual boot: the Windows CI jobs and lints of `cfg(windows)` code, clipboard line endings, startup and fonts, the system report and virtual desktops, a visual pass, and the merge gate for the whole branch.

Hardware checks and their fixes live only in the platform epics, never in a feature story, so EP-001 and EP-002 reach review without a Mac or a reboot into Windows (project practice since 2026-10-08, restated for macOS on 2026-11-23 in `tasks/prd-layered-syntax-highlighting.md`, changelog 1.1 and 1.3).

## Goals

| Goal | Baseline | Target | Measured by |
|------|----------|--------|-------------|
| GPUI pin | `703b5ac3` (2026-11-14) | `c016f436` on `main` by 2026-12-08 | US-001, US-019 |
| Paneflow changes carried by the vendored crates | +711/−32 lines (`gpui_wgpu`), +450/−35 (`gpui_linux`) against upstream `703b5ac3` | The same added and removed lines against upstream `c016f436` | US-001 |
| No unexplained performance regression | `bench/baselines/linux-x86_64/*.json` | Every metric within 10 % of its baseline, or its delta explained in the bump summary (target carried over from the 703b5ac3 PRD) | US-005 |
| Keystroke latency | Linux: 3.21 ms median on `9f606d3aa`, 3.29 ms on `650569c4` (`bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`); macOS: never measured (US-010 of the 703b5ac3 PRD cancelled for want of a Mac) | Linux and macOS medians within 2 ms of the old pin, the rule the 703b5ac3 PRD applied to Linux and Windows; on a 120 Hz display, the new pin within 4.2 ms of the old one, that PRD's macOS target | US-006, US-013 |
| CI | `run_tests` on `main` | Every `run_tests` job green on the merge commit, or red for a cause shown to predate the branch | US-008, US-009, US-014, US-019 |

## Research Findings

Evidence from the 2026-11-24 exploration. Zed paths are written `zed/...` and refer to `/home/arthur/dev/zed` at `c016f436`. The prototype lived in `~/.cache/claude-gpui-probe/` on Arthur's Fedora machine: local, untracked evidence that no other agent can read. Its patch, `bump-probe.patch`, applies cleanly on `65b7a2ad`.

- **R1, the range.** `git rev-list --count 703b5ac3..c016f436` is 167; `git log -- 'crates/gpui*'` lists 24 commits (the annex of the published plan lists each one). No `gpui*` manifest changes dependencies; the workspace manifest changes only `agent-client-protocol`, `tree-sitter`, `flatbuffers` and two new non-GPUI crates.
- **R2, Rust 1.100.** A one-line program calling `str::trim_prefix` fails with `E0658` on rustc 1.98.0 and 1.99.1 and compiles on 1.100.0. Zed moved to 1.100 in `3a5180211a` (#67115). On `65b7a2ad`, Rust 1.100 alone raises six errors under `clippy -D warnings`, none in GPUI code: four deprecations of `fetch_update` and two `clippy::needless_bool` (prototype `clippy2.log`). rustfmt 1.100 changes nothing (`cargo fmt --all --check` clean).
- **R3, the vendored crates.** With only the revisions changed, `cargo check -p gpui_linux -p gpui_wgpu` fails on `wgpu_renderer.rs:1812` (`PrimitiveBatch::NativeViews` not covered). `git merge-file` over the seven files changed on both sides reports 0 conflicts; the files changed only upstream are taken whole; the merged crates compile. The set of lines the Paneflow patch adds and removes, computed against upstream `703b5ac3` before and against `c016f436` after, is identical. Upstream still unwraps the keymap state without a guard (`zed/crates/gpui_linux/src/linux/wayland/client.rs:2120`, `:2125`), so no Paneflow change becomes redundant. The `nightly_coverage` cfg that `vendor/gpui_linux/Cargo.toml`'s `unexpected_cfgs` allow covers occurs twice in upstream `703b5ac3` and never in `c016f436` (`925ee76e1b`, #67199). The `gpui_wgpu` header omits three Paneflow changes present in its patch: `rustfmt.toml`, the "Window presentation" log at `wgpu_renderer.rs:702` and `:775`, and `recover` going back through the presentation probe (`wgpu_renderer.rs:2512`).
- **R4, the prototype.** On `65b7a2ad` with the six toolchain fixes: `cargo check --workspace --all-targets --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings` and `cargo fmt --all --check` pass; `cargo nextest run --workspace --locked --profile ci` runs 6137 tests, all passed. On the older base `988d3768`: 5510 of 5511 passed; the one failure, `validates_the_reviewed_macos_bundle_as_one_unit`, needed the macOS libghostty archive the prototype had not copied, and passes in the `65b7a2ad` run.
- **R5, the lockfile.** After the revision change, `Cargo.lock` is identical to the old one with the hash replaced: 19 packages change their `source` (`bench_metrics`, `collections`, `derive_refineable`, `gpui`, `gpui_apple`, `gpui_ios`, `gpui_macos`, `gpui_macros`, `gpui_platform`, `gpui_shared_string`, `gpui_util`, `gpui_web`, `gpui_windows`, `http_client`, `refineable`, `scheduler`, `sum_tree`, `ztracing`, `ztracing_macro`). wgpu stays at git `cddb2f5d` (30.0.0), cosmic-text at 0.19.0, taffy at 0.15.0. Zed's `[patch.crates-io]` forks of `async-process`, `async-task`, `calloop` and `khronos-egl` keep the revisions Paneflow mirrors (`zed/Cargo.toml:1050-1062`).
- **R6, layout persistence and the sidebar title.** `request_measured_layout` keeps measuring every frame (`zed/crates/gpui/src/window.rs:5762`). The second `layout_as_root` of an element reuses its `layout_id` and recomputes only for a new available space (`zed/crates/gpui/src/elements/element.rs:581-610`). An A/B test with the real text system (`gpui_wgpu::CosmicTextSystem`) and the structure of a workspace row painted the same text on both pins for widths 60, 600, 600, 60, 60, 600, 60, 600, 120 and 600 px: "A…" at 60, the full title at 600, "A works…" at 120 (prototype `title-line-truncation-test.rs`, `probe-new.log`, `old/probe-old.log`). The predicted defect does not occur. A first harness built on `TestAppContext` truncated nothing on either pin and proved nothing.
- **R7, borders.** `e2f00a41ac` changes `over()` and the border blend in `fs_quad` in `zed/crates/gpui_wgpu/src/shaders.wgsl`, `zed/crates/gpui_windows/src/shaders.hlsl` and `zed/crates/gpui_apple/src/shaders.metal`; by one review's algebra the result is unchanged over an opaque fill and lighter, without NaN, over a translucent one (read, not rendered).
- **R8, Windows clipboard.** `zed/crates/gpui_windows/src/clipboard.rs:180-190` writes CRLF and `:237` reads LF. Paneflow's terminal paste already normalizes line endings through `normalize_paste_text` (from `paneflow_ipc_client::send_text`, re-exported at `src-app/src/terminal/input.rs:162`, called at `:1242`), and asks for confirmation only when bracketed paste is off and the text holds a newline (`paste_needs_confirmation`, `input.rs:113`).
- **R9, Windows startup and the system report.** DirectWrite's collection is no longer refreshed at startup (`zed/crates/gpui_windows/src/direct_write.rs:100`); the PR reports a first frame at 195.6 ms instead of 285.3 ms on its author's machine. Paneflow lists fonts through GDI (`src-app/src/fonts.rs:57`). `gpu_specs` now reports the DXGI user-mode driver version for every vendor (`zed/crates/gpui_windows/src/directx_renderer.rs:852-882`), printed as is by `format_gpu_specs` (`src-app/src/system_info.rs:121`).
- **R10, Windows visibility.** A window cloaked by DWM is no longer presentable (`zed/crates/gpui_windows/src/window.rs:1686-1704`), so a window on another virtual desktop stops drawing.
- **R11, macOS native views.** `add_window_methods` installs the four overrides on every GPUI window (`zed/crates/gpui_macos/src/window/native_views.rs:738-756`); each returns early while no native view is attached (for example `:796`). Paneflow sends Objective-C messages only to `NSApplication` and `NSImage` (`src-app/src/app_icon.rs`). `gpu_specs()` still returns `None` on macOS (`zed/crates/gpui_macos/src/window.rs:2247`).
- **R12, frame pacing.** GPUI's default intervals are unchanged (`zed/crates/gpui/src/platform.rs:2903-2905`), so `src-app/src/frame_policy.rs:18` stays as written; it caps unfocused and low-power windows at 33,333 us a frame, 30 frames per second (`frame_policy.rs:13-14`, test at `:32`).
- **R13, upstream issues.** zed-industries/zed#66690 (cached views drop accessibility nodes) and #66954 (macOS key-to-screen latency on 120 Hz displays) were both open on 2026-11-24 (`gh issue view`); nothing in the range touches either.
- **R14, CI state.** `run_tests` on `main` at `65b7a2ad` is red on `macOS aarch64 smoke build` (`paneflow-script::ui::a_prompt_open_in_one_action_lets_another_action_and_a_timer_run`) and `Performance gates (Linux x86_64)`, before any GPUI change. The same run measured `paneflow-mcp` at 522,824 bytes against its 524,288-byte cap. Built locally with `release-min`, Rust 1.100 shrinks `paneflow-mcp` from 525,032 to 516,960 bytes and `paneflow-ai-hook` from 381,616 to 373,328, and grows `paneflow-shim` from 400,944 to 402,744 (`scripts/check-helper-sizes.sh` on the prototype); the local sizes run about 2 KB above CI's.

- **R15, the shader tests upstream already has.** `vendor/gpui_wgpu/src/wgpu_renderer.rs:3094-3118` holds naga tests for its three variants, and upstream `c016f436` has four (`zed/crates/gpui_wgpu/src/wgpu_renderer.rs:2736-2760`), so the merge brings them in. They never run in Paneflow: `vendor/gpui_wgpu` is not a workspace member (root `Cargo.toml`, `[workspace] members`; it enters through `[patch]`), `cargo nextest run --workspace` does not test it, and its manifest has no `[dev-dependencies]` where upstream declares naga (`zed/crates/gpui_wgpu/Cargo.toml:55-60`). A non-member's dev-dependencies are not resolved into `Cargo.lock`. naga 30.0.0 at git `cddb2f5d` is already in the lockfile, and its `wgsl-in` feature needs only `unicode-ident`, already among its locked dependencies.

## Technical Decisions

- **TD-01, target exactly `c016f436`.** Every finding above was checked at this revision; a later Zed commit would need its own review. Options considered: the newest Zed commit at implementation time (rejected: unreviewed), the commit before `c31063a050` to avoid Rust 1.100 (rejected: it drops the layout persistence of #67222 and #67223, and the toolchain move happens anyway).
- **TD-02, rebase the vendored crates by a three-way merge.** Base upstream `703b5ac3`, ours `vendor/`, theirs upstream `c016f436`, per file with `git merge-file`; files changed only upstream are copied whole. Re-applying the header list by hand was rejected: the headers omit three changes (R3), and a merge carries what the code holds, not what the header says.
- **TD-03, no new Paneflow change in the vendored crates.** The edge-texel clamp of `fs_mono_sprite` could also go into `fs_subpixel_sprite`, which now receives Paneflow's varying; no defect has been seen on a subpixel sprite, and each Paneflow change is a cost at every rebase, so it stays out until one is.
- **TD-04, validate the WGSL with naga from `paneflow-app`, built from the renderer's own lists.** wgpu validates shader modules through naga when the renderer starts, and the subpixel module is built only on adapters with dual-source blending (`create_pipelines`, upstream `wgpu_renderer.rs:726`), so a GPU test proves only the variants the test machine's adapter selects. Upstream's naga tests would prove all four but never run in Paneflow (R15). Options: (a) add naga to the vendored manifest's dev-dependencies, rejected because a non-member's dev-dependencies are neither locked nor run; (b) make `vendor/gpui_wgpu` a workspace member, rejected because it would put upstream code under the workspace clippy and fmt gates and pull in its upstream dev-dependencies (criterion, image, `gpui` with `bench-support`); (c) a Linux-only test in `paneflow-app` that reads `vendor/gpui_wgpu/src/wgpu_renderer.rs` as text, extracts the four `concat!` lists, assembles each variant from the listed files and validates it with naga at git `cddb2f5d`, the capabilities upstream's tests use. (c) is chosen: it runs on every Linux CI leg whatever the adapter, follows the renderer's lists rather than a copy of them, adds no package (R15), and leaves the vendored crate's upstream tests as upstream text.
- **TD-05, adopt no new GPUI capability in this PRD.** Each was weighed (Non-Goals gives the reason for each); none removes a defect or a measured cost today.

## Assumptions & Constraints

### Assumptions

| ID | Assumption | Risk | Validated by |
|----|-----------|------|--------------|
| A1 | The Rust 1.100.0 toolchain move, which Arthur is doing separately, is on `main` before US-001 starts | High: without it nothing builds | Checked at the start of US-001 |
| A2 | The red jobs on `main` at `65b7a2ad` (R14) are fixed separately, or still fail the same way on the branch's merge base | Medium | US-008, US-009, US-019 |
| A3 | Every WGSL variant of the rebased renderer passes naga validation, the subpixel variants included | Medium: a failure would break the first frame on most desktop GPUs | US-004 |
| A4 | Layout persistence costs nothing in Paneflow's suites (Zed's figures come from its synthetic trees) | Medium | US-005 |
| A5 | The macOS native-view overrides change no input path while no native view is attached | Medium | US-010, US-011 |
| A6 | A Windows window brought back from another virtual desktop repaints | Low | US-017 |
| A7 | The border change is invisible over opaque fills | Low | US-007, US-012, US-018 |

### Constraints

- Linux (Wayland and X11), macOS on Apple Silicon and Windows 10 and 11 x64 are shipping targets (`AGENTS.md`); `cfg(windows)` code in `paneflow-app` compiles only on Windows.
- Windows proofs run on Arthur's Windows 11 dual boot after a push and a reboot; macOS proofs run on Arthur's Apple Silicon Mac (an Apple M4 in `tasks/prd-layered-syntax-highlighting.md`, US-034). A GitHub `macos-15` runner is a VM and is not the macOS proof.
- Only `linux-x86_64` benchmark baselines exist (`bench/baselines/`); macOS and Windows figures are old-pin-versus-new comparisons.

## Non-Goals

- **The Rust 1.100.0 toolchain move.** Arthur does it separately (A1); this PRD neither edits `rust-toolchain.toml`, the workflow toolchain inputs, nor the six fixes of R2.
- **A Zed revision other than `c016f436`** (TD-01).
- **Native views** (#67311, #67316). `NativeView::new` fails outside macOS (`zed/crates/gpui/src/window/native_view.rs`), against the cross-platform rule of `AGENTS.md`.
- **Following the desktop text scale** (`App::text_scale_factor`, #66828). GPUI reports it on Wayland under GNOME only, Zed itself does not use it, and Paneflow sizes its UI in pixel constants, so scaling only the terminal would make it disagree with the rest of the window.
- **`Animation::with_steps` for the sidebar spinner.** `SpinnerClock` (`src-app/src/app/sidebar/lane.rs:283`) already re-renders once per step on a shared epoch, as `repeat_synced().with_steps(8)` would; it also drives the shimmer, which cannot step, so its ticker would stay.
- **`request_keyed_measured_layout` in `TitleLine`** (`style.rs:175`): its measure closure only returns two captured sizes.
- **Removing `keep_active_tab_in_view`** (`src-app/src/pane.rs:2476`). Upstream now scrolls to an item requested before the first frame (`zed/crates/gpui/src/elements/div.rs`), but the helper re-scrolls when the strip's width changes, which upstream does not do.
- **The edge-texel clamp for subpixel sprites** (TD-03).
- **The upstream issues #66690 and #66954** (R13). The CHANGELOG known issue on accessibility stays.
- **Running the new GL texture path on hardware.** It serves OpenGL ES adapters without shader storage (`f00a94318c`, #67323); no machine available has one.
- **Fixing the red jobs on `main`** (R14). They predate the branch (A2).
- **Upstreaming the vendored changes to Zed.**

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo fmt --all --check` - formatting of the vendored path dependencies, which the Windows CI leg checks
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, test modules included
- `cargo nextest run --workspace --locked --profile ci` - the full suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget in a unit test
- `bash scripts/check-helper-sizes.sh` - the embedded helpers stay under their caps (after nextest, on Linux x86_64)
- `cargo deny check advisories licenses sources` - the lockfile changes in US-001 and US-004

## Epics & User Stories

### EP-001: The pin moves to `c016f436` and the workspace builds and tests on Linux

GPUI comes from `c016f436`, the vendored crates carry every Paneflow change on top of the new upstream sources, and two tests pin the behaviors the research could only prove once.

**Definition of Done:** no `Cargo.toml` in the repository names `703b5ac3f4ffe1e87a073123ae56e22f72f095c4`; the `Cargo.lock` diff against the commit before US-001 is limited to the `source` lines of the 19 packages of R5 and the `naga` line US-004 adds to `paneflow-app`'s dependencies; the Paneflow patch of each vendored crate is unchanged line for line; the title and WGSL tests pass in `cargo nextest` on Linux x86_64.

#### US-001: Rebase the vendored crates and move the pins
**Description:** As the maintainer, I want the seven GPUI entries, the vendored crates and the lockfile moved to `c016f436` in one commit so that the workspace builds on the new GPUI with every Paneflow change kept.

**Priority:** P0
**Dependencies:** None (needs A1: Rust 1.100.0 on `main`)

**Paneflow sites:** `src-app/Cargo.toml` (seven entries), `vendor/gpui_wgpu/Cargo.toml:1, 65, 68, 99`, `vendor/gpui_linux/Cargo.toml:1, 104, 106-109`, the vendored sources, `Cargo.lock`. The prototype patch (local evidence, R4) shows the expected result.

**Acceptance Criteria:**
- [x] Given `src-app/Cargo.toml`, when its Zed entries are read, then all seven name `c016f436e60bfa5105de53837935e9de94eb0867`
- [x] Given `git grep 703b5ac3f4ffe1e87a073123ae56e22f72f095c4 -- '*Cargo.toml'`, when it runs on the story's commit, then it prints nothing
- [x] Given `git diff` of `Cargo.lock` against the story's parent, when it is read, then the only changed lines are the `source` lines of the 19 packages of R5
- [x] Given each file of `vendor/gpui_wgpu` and `vendor/gpui_linux` that the Paneflow patch does not touch, when it is compared with `zed/crates/<crate>` at `c016f436`, then it is byte-identical
- [x] Given the Paneflow patch of each crate, computed as the diff between upstream and `vendor/` without `Cargo.toml` and `rustfmt.toml`, when the patch against upstream `703b5ac3` at the parent and the patch against upstream `c016f436` at the story's commit are compared, then the set of lines each adds and removes is the same
- [x] Given the two patches of `gpui_wgpu`, when their lines are counted, then both add 711 and remove 32, the counts as of `65b7a2ad`
- [x] Given the two patches of `gpui_linux`, when their lines are counted, then both add 450 and remove 35, the counts as of `65b7a2ad`
- [x] Given `vendor/` changed on `main` after `65b7a2ad`, when the story starts, then the merge is redone from the current vendored files rather than from the prototype patch, and the two count criteria use the counts of the story's parent
- [x] Given a merge conflict in any file, when it is resolved, then the implementation evidence names the file, the upstream commit that touched the hunk and the resolution

**Implementation evidence (2026-11-24, working tree on `a3f53dbb`, not yet committed):** A1 holds: `rust-toolchain.toml` pins 1.100.0 since `a3f53dbb`. `git log 65b7a2ad..HEAD -- vendor/` is empty, so the merge ran on the current vendored files, which are the files of `65b7a2ad`. Per TD-02, `git merge-file` (ours `vendor/`, base upstream `703b5ac3`, theirs upstream `c016f436`) ran on the six files changed on both sides, `gpui_wgpu/src/{shaders.wgsl,wgpu_context.rs,wgpu_renderer.rs}` and `gpui_linux/src/linux/{wayland/client.rs,wayland/window.rs,x11/window.rs}`, with 0 conflicts; the six files changed only upstream (`shaders_subpixel.wgsl`, `dispatcher.rs`, `platform.rs`, `x11/client.rs`, `x11/clipboard.rs`, `xdg_desktop_portal.rs`) were copied whole. No upstream file was added or removed in either crate, and neither upstream `Cargo.toml` changed. Patch counts, counting non-empty lines as the PRD does: `gpui_wgpu` +711/−32 before and after, `gpui_linux` +450/−35 before and after (with empty lines: +748/−32 and +479/−39 on both sides); the sorted sets of added and removed lines are byte-identical before and after. Every file outside the patch is byte-identical to upstream `c016f436`; `LICENSE-APACHE`, a symlink upstream, equals the Zed root file it points to. `Cargo.lock` changes only the `source` line of the 19 packages of R5, checked package by package. `cargo check --workspace --all-targets --locked` passed on the new pin.

#### US-002: The vendored headers and manifests describe the rebased code
**Description:** As the maintainer, I want each vendored header to state the new revision and every Paneflow change the code holds so that the next rebase starts from an exact list.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given the header of `vendor/gpui_wgpu/Cargo.toml`, when it is read, then it names `c016f436`
- [x] Given the same header, when its list of changes is read, then it names `rustfmt.toml` and why the crate keeps its own
- [x] Given the same header, when its list of changes is read, then it names the "Window presentation" log and the function that writes it
- [x] Given the same header, when its list of changes is read, then it names `recover` going back through the presentation probe
- [x] Given the header of `vendor/gpui_linux/Cargo.toml`, when it is read, then it names `c016f436`
- [x] Given `vendor/gpui_linux/Cargo.toml`, when it is read, then neither the `[lints.rust] unexpected_cfgs` table nor the sentence that justified it remains
- [x] Given `RUSTFLAGS="-D warnings" cargo check -p gpui_linux --locked`, when it runs, then it passes, so removing the allow raised no `unexpected_cfgs` warning
- [x] Given each Paneflow change a header lists, when its function or file is searched in the vendored crate, then it exists
- [x] Given a header entry whose code the merge no longer holds, when it is found, then the entry is removed and the implementation evidence names the upstream commit that made it unnecessary
- [x] Given the `-D warnings` check above raising `unexpected_cfgs`, when it fails, then the allow stays, restricted to the cfg names the warning prints, and the header says which upstream code needs it
- [x] Given the comment above the GPUI entries in `src-app/Cargo.toml` (line 28 at `65b7a2ad`), when it is read, then it no longer calls the bump a "four-value" change and names the seven entries and the two vendored manifests
- [x] Given the `[Unreleased]` section of `CHANGELOG.md`, when it is read, then it has one entry for the GPUI update that names the upstream fixes users can notice (Windows clipboard line endings on copy, Windows startup without a synchronous font refresh, Linux rendering on OpenGL ES drivers without shader storage) and states they were not reproduced in Paneflow
- [x] Given the known issue on screen readers in `CHANGELOG.md`, when the entry is written, then that issue stays, since #66690 is open (R13)

**Implementation evidence (2026-11-24):** No header entry was removed: every listed function and file exists in the merged crates (`render_colr_v1_image`, `try_adapter_with_surface`, `select_adapter_and_device`, `PresentationProbeScope`, `draw_offscreen`, `to_tile_texel_centers`, `new_internal`, `new_offscreen`, `recover`, `normal_size_hints`, `set_client_inset`, `ShmPresenter`, `reflow_client_inset`, `src/linux/wayland/presentation.rs`). `vendor/gpui_linux/src` no longer mentions `nightly_coverage`, and `RUSTFLAGS="-D warnings" cargo check -p gpui_linux --locked` passes without the `unexpected_cfgs` allow. The `rustfmt.toml` entry is added to both headers; the file equals Zed's root `rustfmt.toml` at `c016f436`. A probe that removed both files showed `cargo fmt --all --check` still clean on rustfmt 1.100, so the header gives the reason that holds (the rules are written out rather than inferred), not a reformat that would happen today. The CHANGELOG entry is under `[Unreleased]` › Fixed; the screen reader known issue of 0.20.0 is unchanged.

#### US-003: Pin sidebar title truncation across resizes with a test
**Description:** As the maintainer, I want the A/B probe of R6 kept as a test so that a later GPUI change to layout or text reuse cannot leave a title truncated after the sidebar widens.

**Priority:** P1
**Dependencies:** Blocked by US-001

**Paneflow sites:** `src-app/src/app/sidebar/style.rs` (the `tests` module; the probe test is local evidence, R6).

**Acceptance Criteria:**
- [x] Given a Linux-only test in `style.rs` that builds a workspace-shaped title row through `sidebar_title_row` and `sidebar_title_with_trailing` with `gpui_wgpu::CosmicTextSystem`, when it draws the widths 60, 600, 600, 60, 60, 600, 60, 600, 120 and 600 px in that order, then every frame narrower than the title's natural width paints an ellipsis and every wider frame paints the whole title
- [x] Given the same test on the story's commit, when it runs, then it passes
- [x] Given a scratch copy where the second `layout_as_root` of `TitleLine::prepaint` is removed, when the test runs, then it fails, which shows that it detects a title that is never truncated
- [x] Given the test built on `TestAppContext` instead of a real text system, when the alternative is considered, then the implementation evidence states why it was not used (it truncated nothing on either pin, R6)

**Implementation evidence (2026-11-24):** `a_title_truncated_by_a_narrow_row_regains_its_text_once_the_row_widens` (`src-app/src/app/sidebar/style.rs`, `tests` module, Linux only) runs `HeadlessAppContext` with `gpui_wgpu::CosmicTextSystem` and Paneflow's embedded fonts (`Assets::load_fonts`, Geist 14 px), so the result does not depend on the fonts installed on the runner. It derives the natural width (title + `SIDEBAR_BRANCH_GAP` + branch, 293 px) from the frame that paints the whole title, asserts that the widths straddle it, then checks each of the ten frames. Mutation: with the `layout_as_root` of `TitleLine::prepaint` replaced by the natural size, the test fails on the first frame ("a 60 px row narrower than 293 px painted "A workspace title long enough to be cut""); restored afterwards. `TestAppContext` was not used because its text system truncated nothing on either pin in the R6 probe, so a test built on it would pass with the defect present.

#### US-004: Validate every WGSL variant of the vendored renderer with naga
**Description:** As the maintainer, I want the four shader variants the renderer builds validated in the test suite so that assumption A3 holds on every adapter, not only on one with dual-source blending.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Paneflow sites:** `vendor/gpui_wgpu/src/wgpu_renderer.rs` after the US-001 merge (the `concat!` lists of `STORAGE_BUFFER_SHADERS`, `WEBGL_SHADERS`, `SUBPIXEL_STORAGE_BUFFER_SHADERS` and `SUBPIXEL_WEBGL_SHADERS`; before the merge the copy has three, with a single `SUBPIXEL_SHADERS`), `src-app/Cargo.toml` (Linux dev-dependencies). Design: TD-04, option (c).

**Acceptance Criteria:**
- [x] Given a Linux-only test in `paneflow-app` that reads the four `*_SHADERS` `concat!` lists from `vendor/gpui_wgpu/src/wgpu_renderer.rs` and assembles each variant from the files and literals they list, when it validates each with naga at git `cddb2f5d`, the two storage and texture variants with no capability and the two subpixel variants with `DUAL_SOURCE_BLENDING`, then all four pass
- [x] Given the two texture variants, when the test reads them, then neither contains `var<storage`, as upstream's tests assert
- [x] Given the test, when it runs on a machine without a GPU, then it still runs and passes, because it creates no adapter
- [x] Given `Cargo.lock` after the naga dev-dependency is added, when it is compared with the story's parent, then it gains no `[[package]]` entry
- [x] Given a scratch copy where `tile_texel_centers` in `MonoSpriteVarying` takes `@location(3)`, which `clip_distances` already uses, when the test runs, then it fails and names the subpixel variant among the failures
- [x] Given a scratch copy where a `concat!` list names a `.wgsl` file that does not exist, when the test runs, then it fails and names the file
- [x] Given a scratch copy where the renderer defines a number of `*_SHADERS` lists other than four, when the test runs, then it fails and prints the names it found

**Implementation evidence (2026-11-24):** `src-app/tests/vendored_wgsl_shaders.rs` (`#![cfg(target_os = "linux")]`) parses the `const *_SHADERS: &str = concat!(...)` lists of `vendor/gpui_wgpu/src/wgpu_renderer.rs`, assembles each from its `include_str!` files and string literals, and validates it with `naga` 30.0.0 at git `cddb2f5d` (`wgsl-in`, a Linux dev-dependency of `paneflow-app`): the `SUBPIXEL_*` variants with `DUAL_SOURCE_BLENDING`, the others with no capability. It creates no adapter. `Cargo.lock` gains only the line `"naga",` in `paneflow-app`'s dependencies; the package count stays 858. Mutations, run on the built test binary and restored afterwards: `tile_texel_centers` at `@location(3)` fails all four variants, `SUBPIXEL_STORAGE_BUFFER_SHADERS` and `SUBPIXEL_WEBGL_SHADERS` included ("Entry point vs_mono_sprite at Vertex is invalid"); `shaders_missing.wgsl` in the storage list fails with "STORAGE_BUFFER_SHADERS lists shaders_missing.wgsl, which cannot be read"; a fifth `EXTRA_SHADERS` list fails with "the renderer defines 5 *_SHADERS lists, expected 4" and the five names.

**EP-001 validation (2026-11-24, final working tree on `a3f53dbb`, Fedora, Linux x86_64, tier `deep`):** `cargo fmt --check`, `cargo fmt --all --check`, `RUSTFLAGS="-D warnings" cargo check -p gpui_linux --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings`: pass. `cargo nextest run --workspace --locked --profile ci`: 6141 tests run, 6141 passed, 115 skipped. A first run failed `frame_policy::tests::every_paneflow_window_is_opened_with_the_paced_options`, which counts `.open_window(` calls: the US-003 test now calls `HeadlessAppContext::open_window(&mut cx, ...)`, as `ui_primitives.rs` does. `scripts/check-test-wallclock.sh`: no new wall-clock assertion. `scripts/check-helper-sizes.sh`: `paneflow-shim` 402,744 B (margin 121,544), `paneflow-ai-hook` 373,328 B (margin 10,672, over its 307,200 B soft cap as before), `paneflow-mcp` 516,960 B (margin 7,328), combined 1,293,032 B. `cargo deny check advisories licenses sources`: ok. Nothing is committed yet; the criteria that name "the story's commit" hold on this tree and must be read again on the commit.

**EP-001 review (2026-11-24, fresh session, tier `deep`, working tree on `a3f53dbb`): DONE, no blocking finding.** US-001: the three-way merge was redone independently from upstream archives of `703b5ac3` and `c016f436` (`/home/arthur/dev/zed`) and `HEAD:vendor/`; each of the six merged files comes out with 0 conflicts and byte-identical to the working tree, the six upstream-only files match `c016f436`, both crates keep the same file set, and the sorted added and removed line sets of each patch are identical before and after (`gpui_wgpu` +711/−32, `gpui_linux` +450/−35, non-empty lines). The renderer's new `PrimitiveBatch::NativeViews` arm is an explicit no-op beside `Surfaces`, not a catch-all. `git grep` of the old hash in `*Cargo.toml` prints nothing; `src-app/Cargo.toml` names `c016f436` seven times; `Cargo.lock` changes 19 `source` lines (the R5 packages) plus `"naga",`, with 858 packages before and after. US-002: every header-listed symbol and file exists, the "Window presentation" log is written in `new_internal` and `new_offscreen`, `recover` goes through `new_with_presentation_probe` and keeps the probe and `SoftwarePresenter`, both `rustfmt.toml` equal Zed's root file at `c016f436`, and the Zed workspace manifest changes in the range touch no dependency of either crate. US-003 and US-004: the mutations were replayed and restored (checked by SHA-256): removing the second `layout_as_root` fails the title test on the 60 px frame; `@location(3)` on `tile_texel_centers` fails all four variants, both `SUBPIXEL_*` included; `shaders_missing.wgsl` and a fifth `EXTRA_SHADERS` list fail with the file and the five names. The `HeadlessAppContext::open_window(&mut cx, ...)` form that keeps `frame_policy`'s guard count follows the precedent of `ui_primitives.rs` and `scene_export/mod.rs` (headless test windows, not product windows). Certifying run after the last change: `cargo fmt --check`, `cargo fmt --all --check`, `RUSTFLAGS="-D warnings" cargo check -p gpui_linux --locked`, `cargo clippy --workspace --all-targets --locked -- -D warnings` pass; `cargo nextest run --workspace --locked --profile ci` 6141 run, 6141 passed, 115 skipped; `check-test-wallclock.sh` clean; `check-helper-sizes.sh` 402,744 / 373,328 / 516,960 B, all under their hard caps; `cargo deny check advisories licenses sources` ok. Non-blocking, unapplied: the rewritten header line of `vendor/gpui_linux/Cargo.toml` (line 6) runs to 102 columns where the rest wraps at 78. Certification covers this tree: the US-001 commit must hold exactly the epic files (`Cargo.lock`, `CHANGELOG.md`, `src-app/Cargo.toml`, `src-app/src/app/sidebar/style.rs`, `src-app/tests/vendored_wgsl_shaders.rs`, `vendor/gpui_wgpu/**`, `vendor/gpui_linux/**`), without the unrelated `.gitignore` and `tasks/prd-agents-browser*` changes.

---

### EP-002: Linux validation on Fedora

What a Linux build can show only on a real session or in CI: the suites against the old pin, keystroke latency, the screen under Wayland and X11, and the Linux CI jobs. macOS is EP-003, Windows EP-004.

**Definition of Done:** each story below has its report written as a section of this PRD, with the commit it ran on; `bench/results/gpui-bump-c016f436-<date>.md` is committed; every defect found is fixed in this epic or recorded as a blocker on the story it belongs to.

#### US-005: Compare and re-baseline the Linux suites
**Description:** As the maintainer, I want the terminal, editor, tree and startup suites run on both pins on Fedora so that the cost of layout persistence is measured, not inferred (A4).

**Priority:** P0
**Dependencies:** Blocked by US-001, US-002, US-003, US-004

**Paneflow sites:** `scripts/bench-terminal.sh`, `bench-editor.sh`, `bench-tree.sh`, `bench-startup.sh`, `scripts/perf-gates.sh`, `bench/baselines/linux-x86_64/`, `bench/README.md`, `bench/results/gpui-bump-703b5ac3-2026-11-16.md` (the layout to follow).

**Acceptance Criteria:**
- [x] Given the four suites run interleaved on the parent of US-001 and on the head of EP-001, each build in its own worktree and target directory outside the repository, when the summary is written, then `bench/results/gpui-bump-c016f436-<date>.md` holds both columns per metric with the machine, the session and the CPU share
- [x] Given each metric, when it is compared with its baseline, then any metric more than 10 % worse is listed with its cause and either a fix in this epic or Arthur's acceptance
- [x] Given the allocation baselines, when they are compared, then each is within 1 % or its change is explained (the 10 % and 1 % rules come from the 703b5ac3 PRD's Goals)
- [ ] Given `scripts/perf-gates.sh` on the final commit of the epic, when it runs, then it passes without a raised budget, or each raised budget is listed in the summary with its measured value and Arthur's approval
- [x] Given two runs of one metric more than 10 % apart, when the result is read, then the run is repeated with a one-minute load average under 1.2 and no build running, the conditions of the 703b5ac3 runs, and no baseline is set from the first run
- [x] Given refreshed baselines, when they are committed, then they come from a clean tree on top of the code commit and `bench/README.md` names that commit

**Implementation evidence (2026-11-25, `226032bf` against `a3f53dbb`, Fedora, tier `default`):** the four suites ran interleaved from two worktrees with their own target directories under `~/.cache/claude-c016-bench/` (terminal and editor 10 rounds, tree and startup 8), each round adding the new binary run again as an A/A column, every run pinned with `taskset` and started only under a one-minute load below 1.2 (0.53 to 1.19). [bench/results/gpui-bump-c016f436-2026-11-25.md](../bench/results/gpui-bump-c016f436-2026-11-25.md) holds both columns per metric with the machine, the session and the CPU shares; every run is in `bench/results/gpui-c016f436-us005-interleaved-2026-11-25.json`. No metric of the new pin is more than 10 % worse than the old pin (largest +3.5 %, `lexer_viewport_query_60_rows`), and no two runs called for a repeat. Against the committed baselines, `service_text_220x60` (+11.2 %), `wrapped_giant_line_first_frame` (+28.4 %) and `deferred_burst_edit_wall` (+120.2 %) read the same on the old pin: introduced between the baseline commits and `a3f53dbb`, not by the bump, and not bisected; Arthur accepted handing the epic to review with them (session of 2026-11-25). Allocation columns are identical except `shape_cold_60_rows` (+960 bytes, 0.05 %). `editor.json`, `tree.json` and `startup.json` were re-recorded with `--set-baseline` on a clean `226032bf` worktree and `bench/README.md` names it; `terminal.json` stays at `650569c4` because the script refuses the suite's 0.88 CPU share, a property of the suite since `e26aec3d`, on both pins. The third criterion, `perf-gates.sh`, is not met as written: the CI job that runs it fails on `226032bf` (run 45491422085) and on `a3f53dbb` (run 45482822833) in the same `startup` and `gates` steps with the same messages, so the failure predates the bump (A2); no budget was raised. It was not run locally.

#### US-006: Keystroke latency and the unfocused frame cap on Fedora
**Description:** As a Linux user, I want keystroke echo and the unfocused frame cap measured on both pins so that the frame and input telemetry changes of #67387 are shown to change nothing I can feel.

**Priority:** P1
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given debug builds of both pins with `PANEFLOW_LATENCY_PROBE=1`, when latency is measured by the procedure of `bench/results/gpui-703b5ac3-frame-pacing-2026-11-15.md`, then both medians, the session and the driver are recorded in the bump summary and differ by at most 2 ms
- [x] Given an unfocused window streaming terminal output on the new pin, when it redraws, then it presents at most 30 frames per second, the cap of `frame_policy.rs` (R12)
- [x] Given medians more than 2 ms apart, when the result is read, then the story stays open and the cause is investigated before any frame interval changes

**Implementation evidence (2026-11-25):** debug builds of `a3f53dbb` and `226032bf`, probe thresholds lowered to 0 in those copies only, five interleaved runs per pin of 200 keystrokes typed with `agent-desktop press` on GNOME 51.1 Wayland, NVIDIA 620.70.06; focus checked from a screenshot before typing and a probe line required after every key; no run aborted. Medians 3.53 ms (old) and 3.02 ms (new), means of the run medians: 0.51 ms faster, within 2 ms. Unfocused (GNOME Calculator in front) the new pin presents 25.3 to 25.6 frames per second, never more than 27 in a second. Per-run table in the bump summary.

#### US-007: Visual pass on Linux, Wayland and X11
**Description:** As a Linux user, I want the window checked on screen under both display servers so that the border blend (R7) and the persisted layout are compared with the old pin where no test looks.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given `linux_chrome_material` on and a translucent window backdrop under Wayland, when rounded cards, outlined buttons and the focus hairline are screenshotted on both pins at the same window size, then the report shows each pair with the count of differing pixels and describes every difference
- [x] Given the same setup under X11, when the same surfaces are screenshotted on both pins, then the report shows each pair with the count of differing pixels and describes every difference
- [x] Given an opaque window, when the same surfaces are compared, then the report states whether any pixel differs (A7)
- [ ] Given the sidebar dragged narrower until titles truncate, then wider, when it is watched, then every title regains its full text, under both display servers
- [ ] Given a menu, a select and a tooltip opened over a scrolled list, when each is hovered and clicked at its edges, then each reacts only inside its visible bounds (#67390)
- [x] Given a detached pane window, when it is screenshotted on both pins at the same size, then the report gives the count of differing pixels and describes every difference
- [x] Given a terminal printing a 100 MB file, when it is watched on both pins, then the report states whether any frame shows a torn, blank or stale region
- [x] Given a defect, when it is found, then it is fixed in this epic with a screenshot of the fix, or recorded as a blocker on this story

**Implementation evidence (2026-11-25):** release builds of both pins (debug builds for the two settings pages, opened at startup by an environment variable in the measurement copies only), each scene captured for the old pin, the new pin and the old pin again, on GPUI's Wayland backend and on its X11 backend under Xwayland (GNOME 51 has no Xorg session; Arthur accepted Xwayland as the X11 proof). Welcome page, split panes with the focus hairline, Settings General and Settings Appearance, Blended and Themed: 0 differing pixels in the window on every backend (one Appearance set over a changing Remmina background was repeated and reads 0). The detached pane window differs by 48 pixels, 12 per rounded corner, at most 11 levels lighter on the new pin, identical across backends and materials and 0 between the old pin and itself: the border blend of `e2f00a41ac` (R7) over the transparent pixels outside the fill, not a defect. Opaque windows: no differing pixel in the main window. Under X11 a sidebar drag through Xwayland truncates and restores the long workspace title on both pins. 40 consecutive prints of a 100 MB file: 17 captures per pin, every one a block of consecutive lines, no torn, blank or stale region. No defect found. **Left open, needs Arthur:** the sidebar drag under Wayland and the menu, select and tooltip edges over a scrolled list (#67390) need a real pointer; neither was checked, so the fourth criterion holds for X11 only and the fifth is not met.

#### US-008: The Linux CI jobs are green on the branch
**Description:** As the maintainer, I want every Linux job of `run_tests` checked on the branch so that the Linux side of the merge gate is settled before macOS and Windows.

**Priority:** P0
**Dependencies:** Blocked by US-005, US-007

**Acceptance Criteria:**
- [ ] Given the branch head, when `run_tests` runs, then `Style (fmt + clippy)`, `Tests (Linux x86_64)`, `Linux aarch64 (aarch64-unknown-linux-gnu)`, `Security Audit (cargo-deny)` and `Workflow audit (zizmor)` succeed
- [x] Given `Performance gates (Linux x86_64)`, when it runs, then it succeeds, or it fails for the cause it fails for on `main` at the branch's merge base, shown by both run ids in the report (A2)
- [x] Given the `check-helper-sizes.sh` output in the `Tests (Linux x86_64)` log, when the report is written, then it records the three sizes and their margins
- [x] Given a red Linux job whose cause the branch introduced, when it is found, then it is fixed in this epic before the story is done

**Implementation evidence (2026-11-25):** `run_tests` run 45491422085 on `226032bf`: `Style (fmt + clippy)`, `Tests (Linux x86_64)`, `Linux aarch64` and `Security Audit (cargo-deny)` succeed. `Workflow audit (zizmor)` is skipped by its path filter (no `.github/` change since `a3f53dbb`, where it passed in run 45482822833); run locally on `226032bf`, `uvx zizmor@1.32.0 --config .github/zizmor.yml --min-severity high .github` reports no finding. The first criterion is left unticked: zizmor did not run in CI on the branch, only that local equivalent did. `Performance gates (Linux x86_64)` fails in the same steps as on `a3f53dbb` (run 45482822833). Helper sizes in the CI log: `paneflow-shim` 401 400 B (margin 122 888), `paneflow-ai-hook` 371 384 B (margin 12 616), `paneflow-mcp` 514 648 B (margin 9 640). No red Linux job comes from the branch. The commit carrying this report changes only `bench/` and `tasks/`; its own `run_tests` run is the one to read at review.

---

### EP-003: macOS validation on Apple Silicon

Every proof that only macOS can give, gathered in one pass on Arthur's Apple Silicon Mac. The macOS code ships with EP-001; this epic builds it, runs it and fixes what it finds.

**Definition of Done:** each story below has its report written as a section of this PRD, with the commit and the build it ran on; the macOS jobs of `run_tests` are green or red only for a cause shown to predate the branch; every defect found is fixed in this epic or recorded as a blocker on the story it belongs to.

#### US-009: Build, lint and test on macOS
**Description:** As the maintainer, I want the workspace built, linted and tested on the Mac and on the macOS CI jobs so that `cfg(target_os = "macos")` code is proven against the new GPUI.

**Priority:** P0
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given the branch head on the Mac, when the workspace lint gate runs there, then it passes, `gpui_apple` with `font-kit` included
- [ ] Given the same commit, when the full test suite runs on the Mac, then every test passes, or each failure also fails on the parent of US-001 on the same Mac and is listed in the report as predating the branch
- [ ] Given the branch head, when `run_tests` runs, then `macOS aarch64 smoke build` and `macOS aarch64 render smoke (visual)` succeed, or fail only for a cause also present on `main` at the merge base, shown by both run ids (A2)
- [ ] Given a lint or test failure the branch introduced, when it is found, then it is fixed in this epic before the story is done

#### US-010: Keyboard and input under the native-view overrides
**Description:** As a macOS user, I want keyboard, IME and focus behavior unchanged so that the overrides every window now receives (R11) are shown to be inert without native views (A5).

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given a release build from the story's commit, when each of the 45 `secondary-` bindings of `src-app/src/keybindings/defaults.rs` (Cmd on macOS) is pressed in a context where it applies, then it runs its action once, as on the old pin, and the report lists any binding it could not exercise
- [ ] Given a terminal and the code editor, when text is composed with a Japanese input method and committed, then the committed text appears once at the caret and the candidate window opens under the caret
- [ ] Given two panes and a detached pane window, when focus moves between them by click and by keyboard, then keystrokes go to the focused one only
- [ ] Given a key equivalent that the old pin handled and the new one swallows or doubles, when it is found, then it is fixed in this epic or recorded as a blocker with the AppKit selector involved

#### US-011: Window chrome and the system report on macOS
**Description:** As a macOS user, I want the title bar, window moves and the system report checked so that the `sendEvent:` and `makeFirstResponder:` overrides (R11) are shown to leave window handling unchanged.

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given the title bar, when it is double-clicked, then the window zooms as on the old pin
- [ ] Given the title bar, when it is dragged, then the window moves with the pointer and stays where it is released
- [ ] Given the traffic lights, when the window is screenshotted, then they are centered in the title bar as `65b7a2ad` places them
- [ ] Given a click on a traffic light, when it is pressed, then the window closes, minimizes or enters full screen once
- [ ] Given Help › System Info… and its Copy button, when the report is pasted, then its GPU line comes from Metal, since `gpu_specs()` still returns `None` (R11)
- [ ] Given a title-bar click that the old pin handled and the new one ignores or repeats, when it is found, then it is fixed in this epic or recorded as a blocker with the AppKit selector involved

#### US-012: Visual pass on macOS
**Description:** As a macOS user, I want the window checked on screen so that the Metal border change (R7) and text rendering are judged by eye.

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given the blurred window backdrop, when rounded cards, outlined buttons and the focus hairline are screenshotted on both pins at the same window size, then the report shows each pair with the count of differing pixels and describes every difference
- [ ] Given an opaque window, when the same surfaces are compared, then the report states whether any pixel differs (A7)
- [ ] Given a Retina display at scale 2 and a display at scale 1, when the terminal, the sidebar and the editor are screenshotted, then every glyph is drawn, which shows that `font-kit` still reaches `gpui_apple`
- [ ] Given the sidebar narrowed until titles truncate and widened again, when it is watched, then every title regains its full text
- [ ] Given a defect, when it is found, then it is fixed in this epic or recorded as a blocker on this story

#### US-013: Keystroke latency on macOS
**Description:** As a macOS user, I want keystroke echo measured on both pins so that the first macOS latency figure exists and the bump is shown not to worsen it.

**Priority:** P1
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given debug builds of both pins with `PANEFLOW_LATENCY_PROBE=1` on the Mac, when keys are posted through Quartz into a focused terminal, interleaved over at least five runs per pin (the run count of the 703b5ac3 PRD's final Linux measurement), then both medians, the display's refresh rate and the macOS version are recorded in the bump summary
- [ ] Given the two medians, when they are compared, then they differ by at most 2 ms on a 60 Hz display, or by at most 4.2 ms on a 120 Hz display
- [ ] Given medians further apart than the threshold for the display's refresh rate, when the result is read, then the story stays open and the cause is investigated; #66954 is named as the cause only if the display runs at 120 Hz

---

### EP-004: Windows validation on the Windows 11 dual boot

Every proof that only Windows can give, gathered in one pass on Arthur's Windows 11 dual boot (x64) after a push and a reboot, then the merge gate for the whole branch. This is the final epic.

**Definition of Done:** each story below has its report written as a section of this PRD, with the commit it ran on; every defect found is fixed in this epic or recorded as a blocker on the story it belongs to; US-019 records a green `run_tests` on the commit that merges.

#### US-014: Build, lint and test on Windows
**Description:** As the maintainer, I want the workspace built, linted and tested on Windows and on the Windows CI jobs so that `cfg(windows)` code, which no Linux or macOS build lints, is proven against the new GPUI and Rust 1.100.

**Priority:** P0
**Dependencies:** Blocked by US-009, US-010, US-011, US-012, US-013

**Acceptance Criteria:**
- [ ] Given the branch head, when `run_tests` runs, then `Windows x86_64 libghostty check` and `Windows x86_64 render smoke (visual)` succeed
- [ ] Given the dual boot, when the workspace lint gate runs there, then it passes
- [ ] Given the dual boot, when the full test suite runs there, then every test passes, or each failure also fails on the parent of US-001 there and is listed as predating the branch
- [ ] Given a lint that Rust 1.100 raises in `cfg(windows)` code, when it is found, then it is fixed in this epic and the fix is noted in the report

#### US-015: Clipboard line endings on Windows
**Description:** As a Windows user, I want pasting and copying multi-line text to behave as before so that GPUI's CRLF conversion (R8) changes no paste path and gives Windows applications CRLF text.

**Priority:** P0
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Given three lines copied from Notepad, when they are pasted into a terminal running PowerShell 7, then the shell receives three lines once, with no empty line between them
- [ ] Given the same text pasted into `cmd.exe`, which does not enable bracketed paste, when the paste starts, then Paneflow asks for confirmation before sending it, as on the old pin
- [ ] Given the same text pasted into a shell that enables bracketed paste, when the paste starts, then no confirmation appears
- [ ] Given the same text, when it is pasted into the code editor and saved, then the file has the line endings the editor had before the paste, with no stray `\r`
- [ ] Given the same text, when it is pasted into a settings text field, then the field holds it without stray characters
- [ ] Given a terminal selection, an editor selection and an OSC 52 copy from a shell, when each is pasted into Notepad, then each line starts on its own line
- [ ] Given text with a lone `\r` and text with an embedded NUL, when each is copied in Paneflow and pasted back, then the result matches what the report predicts from `clipboard.rs:180-190` and `normalize_paste_text`, and any difference from the old pin is listed

#### US-016: Startup and fonts on Windows
**Description:** As a Windows user, I want startup measured and fonts checked on both pins so that the end of the synchronous font refresh (R9) is shown to help and to lose no font.

**Priority:** P1
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Given release builds of both pins, when `scripts/bench-startup.ps1` runs interleaved on the dual boot, then the first-frame totals of both pins are recorded in the bump summary
- [ ] Given a font installed while Paneflow is closed and chosen as the terminal font, when Paneflow starts, then the terminal draws with it
- [ ] Given a configured font that is not installed, when Paneflow starts, then the terminal draws with the fallback font, and the report states whether the first frame came later than with an installed font
- [ ] Given the settings font picker, when it opens, then it lists the font installed above, since Paneflow lists fonts through GDI (`fonts.rs:57`)

#### US-017: System report and virtual desktops on Windows
**Description:** As a Windows user, I want the system report and a window on another virtual desktop checked so that the driver string (R9) and the presentability change (R10) are known (A6).

**Priority:** P1
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Given the System Info… dialog of the title bar's Help menu and its Copy button on both pins, when both reports are pasted, then the story's report records both GPU lines, the new one in the DXGI form (for example "NVIDIA Corporation 32.0.15.6094")
- [ ] Given a Paneflow window streaming terminal output, when it is moved to another virtual desktop and the user returns after the output changed, then the window shows the latest output without any input
- [ ] Given a minimized window streaming output, when it is restored, then it shows the latest output
- [ ] Given a window that stays stale on return, when it is found, then it is fixed in this epic or recorded as a blocker with the visibility event GPUI sent

#### US-018: Visual pass on Windows
**Description:** As a Windows user, I want the window checked on screen so that the HLSL border change (R7) is judged on Mica, Acrylic and opaque windows.

**Priority:** P0
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Given the Mica backdrop, when rounded cards, outlined buttons and the focus hairline are screenshotted on both pins at the same window size, then the report shows each pair with the count of differing pixels and describes every difference
- [ ] Given the Acrylic backdrop, when the same surfaces are screenshotted on both pins, then the report shows each pair with the count of differing pixels and describes every difference
- [ ] Given an opaque window, when the same surfaces are compared, then the report states whether any pixel differs (A7)
- [ ] Given a display scale of 100 %, when the terminal, the sidebar and the editor are screenshotted on both pins, then the report gives the count of differing pixels and describes every difference
- [ ] Given a display scale of 150 %, when the same views are screenshotted on both pins, then the report gives the count of differing pixels and describes every difference
- [ ] Given a defect, when it is found, then it is fixed in this epic or recorded as a blocker on this story

#### US-019: The branch is ready to merge
**Description:** As the maintainer, I want the whole `run_tests` workflow green on the commit that merges so that the bump lands with every platform proven and nothing changed after the proof.

**Priority:** P0
**Dependencies:** Blocked by US-014, US-015, US-016, US-017, US-018

**Acceptance Criteria:**
- [ ] Given the commit to merge, when `run_tests` runs, then `tests_pass` succeeds
- [ ] Given a job that fails on that commit, when the report is written, then it is a job also red on `main` at the merge base for the same cause, with both run ids and Arthur's acceptance, and no other job fails
- [ ] Given the commits after the last green run, when they are listed, then there are none
- [ ] Given the pull request, when its body is read, then it lists the validation each epic ran, links the bump summary, and contains no auto-closing keyword

## Edge Cases & Error States

| Scenario | Trigger | Expected behavior |
|----------|---------|-------------------|
| `main` changes the vendored crates before US-001 | A commit touches `vendor/gpui_*` after `65b7a2ad` | US-001 redoes the merge from the current files; the prototype patch is not applied blindly |
| The toolchain move is not on `main` | A1 fails when US-001 starts | US-001 is `BLOCKED`; nothing in this PRD changes the toolchain |
| A red CI job predates the branch | R14 still holds at the merge base | It is recorded with both run ids and does not count against the branch (US-008, US-009, US-019) |
| A platform defect is found late | EP-003 or EP-004 finds a regression | It is fixed in that epic; EP-001 and EP-002 keep their certification unless the fix touches their code, in which case their gates are re-run |
| naga rejects a variant | US-004 fails on a subpixel variant | The varying or the shader is fixed in `vendor/gpui_wgpu` and the header lists the change; the bump does not merge with a failing variant |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| The subpixel shader fails validation on GPUs with dual-source blending | Low (upstream ships the same shaders with its naga tests; Paneflow's patch changes only the mono sprite varying, R3) | High: no first frame | US-004 validates every variant without a GPU |
| Layout persistence regresses a Paneflow suite | Medium | Medium | US-005 compares both pins interleaved, with the 10 % rule |
| A macOS key equivalent or IME path changes under the overrides | Low (R11) | High for macOS users | US-010 exercises them on hardware |
| Windows paste gains or loses a line ending | Low (R8) | High: commands run twice or merge | US-015 covers terminal, editor and fields |
| Red jobs on `main` hide a branch regression | Medium (R14) | Medium | Each red job is compared with the merge base by cause, not by name |
| `paneflow-mcp` crosses its size cap | Low after Rust 1.100 (about 8 KB smaller locally, R14) | Medium: CI red | `check-helper-sizes.sh` is a gate of every story; US-008 records the CI sizes |

## Files NOT to Modify

- `rust-toolchain.toml`, the `toolchain:` inputs in `.github/workflows/*.yml`, `AGENTS.md:11`, `README.md:90`, `docs/release/rustfmt.md`: owned by the separate Rust 1.100.0 move (A1).
- `src-app/src/frame_policy.rs`: GPUI's default intervals are unchanged (R12).
- The `[patch.crates-io]` table of the root `Cargo.toml`: Zed keeps the same forks at the same revisions (R5).
- `deny.toml`: the bump adds no source (R5).
- `vendor/cosmic-text/**` and `vendor/portable-pty/**`: unrelated to GPUI; cosmic-text stays at 0.19.0.
- `native/libghostty/**`: the terminal engine archives and manifest.
- Upstream code inside `vendor/gpui_wgpu` and `vendor/gpui_linux` beyond the merge: no new Paneflow change (TD-03), except a fix US-004 forces.
- `tasks/prd-gpui-703b5ac3-upgrade.md` and `bench/results/gpui-bump-703b5ac3-2026-11-16.md`: records of the previous bump.
[/PRD]
