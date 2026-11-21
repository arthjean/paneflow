[PRD]
# PRD: One window backdrop model on every platform

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-20 | Arthur Jean | Initial PRD, from the 2026-11-20 comparison of Paneflow's backdrop code with Zeron's (zgx), a reading of the pinned GPUI sources, and a live blur test on GNOME 51.1. One `background_style` key replaces seven, one veil function owns every translucent fill, and the veil thickens where the system cannot blur. 6 epics, 16 stories. Reviewed against the PRD rubric the same day; the failures it reported are fixed in this version. |
| 1.1 | 2026-11-21 | Arthur Jean | Every macOS real-hardware check and visual pass moves to a final epic, EP-007, run on an Apple Silicon bare-metal Mac mini rented from Scaleway. US-010 keeps only the fullscreen rule and becomes code-only; A3 and A4 are validated in EP-007. 7 epics, 19 stories. |

## Problem Statement

Paneflow's translucent window (the Blended background style) is built from seven configuration keys and three unrelated platform implementations, and it degrades badly where the system does not blur.

1. **Seven keys describe one choice.** `window_backdrop`, `windows_terminal_material`, `windows_chrome_material`, `windows_material_veil`, `macos_chrome_material`, `linux_terminal_material` and `linux_chrome_material` (`crates/paneflow-config/src/schema/config.rs:25-37`) are read through seven derived accessors (`config.rs:322-425`) across 116 matching lines in 21 files under `src-app/src` (a grep on 2026-11-20 for the seven key names, their accessors and `background_style`). Settings already folds them into one select, Themed, Mica or Blended (`config.rs:825-855`), and shows `Custom` when the keys disagree (`src-app/src/settings/tabs/appearance.rs:863`). The per-key toggles remain next to it (`appearance.rs:189-321`).
2. **Three platforms, three looks.** Windows paints a theme veil over the Acrylic system backdrop and a shade on the terminal card. Linux paints the same veil and shade. macOS shows the AppKit Sidebar material behind the sidebar only, with its own system tint, and keeps the terminal opaque (`DESIGN.md:1993-1996`, `src-app/src/window_chrome/macos_backdrop.rs:111`).
3. **Blur on Linux needs a second, config-only key.** Paneflow asks the compositor for blur only when `window_backdrop` is `blurred` (`src-app/src/app/constants.rs:128-131`, called from `src-app/src/launch.rs:477` and `src-app/src/app/detached_panes/window.rs:127`), which Settings never writes. A user who picks Blended on a compositor that can blur therefore gets a sharp desktop behind the window unless an extension such as Blur my Shell blurs it from outside (`docs/user/settings.md:64`).
4. **No blur means an unreadable veil.** Where the compositor offers no blur protocol (GNOME 50 and earlier, sway, Weston, X11), Blended keeps the same 0.72 or 0.80 veil over a sharp desktop: the text and shapes of the windows behind show through the chrome at 28 % or 20 % strength.

The affected people are every user who selects Blended, and the maintainer, who touches seven keys and three code paths for any change to the look.

**Why now:** Mutter 51 ships `ext-background-effect-v1`. On 2026-11-20, on GNOME Shell 51.1 (Wayland) with Blur my Shell disabled, the session still advertised `ext_background_effect_manager_v1`, and a debug Paneflow started with `window_backdrop: "blurred"` and both Linux materials on showed a native blur, confirmed visually by Arthur. The main Linux desktop can now blur without an extension, so the only thing between a Blended user and a blurred window is Paneflow's own configuration model. The same day, a reading of Zeron's code showed a single-model design (the system supplies a blur, the app lays one tint over it, and the tint adapts to the blur) that removes all four problems.

## Overview

The work replaces the seven keys with one, `background_style`, whose values are `themed` and `blended`, and gives Blended one definition on every platform: **the system blurs what is behind the window, and Paneflow paints one theme veil over it.** A single module decides how opaque that veil is from one input, whether the system blurs.

Four decisions shape it, each argued in [Technical Decisions](#technical-decisions):

1. **One key, resolved at read time.** `background_style` is the only key the app reads. A config that has no `background_style` is resolved from the legacy keys by a fixed rule, so nobody's window changes at upgrade. Settings writes the new key and removes the legacy ones.
2. **One veil module.** `BlurSupport` (`Full` or `None`) and the veil functions live in one platform-neutral module. Every shell and terminal fill that depends on the backdrop goes through it. The `material_veiled` flag and the per-OS branches disappear.
3. **Driven degradation.** With `BlurSupport::None` the shell veil keeps 25 % of its see-through (the `LEAST` value of Zeron's `glass_tint.luau`) and the terminal card paints the opaque theme background, so the desktop reads as color and not as shapes.
4. **Each platform uses the mechanism already proven there.** Linux keeps the vendored GPUI Wayland protocols and asks for blur whenever Blended is on. macOS switches to GPUI's `WindowBackgroundAppearance::Blurred`, an untinted `NSVisualEffectView`. Windows keeps the Acrylic system backdrop that Blended uses today, not GPUI's accent blur.

The construction order is: configuration and Settings, the veil module, Linux, macOS, Windows code and cleanup, then one Windows hardware epic and one macOS hardware epic.

## Goals

| Goal | Baseline (2026-11-20) | Target | How measured |
|------|----------------------|--------|--------------|
| Config keys that select the window backdrop | 7 | 1 (`background_style`) | Guard test: no legacy key name is read in `src-app/src` |
| Background styles offered in Settings | 3 on Windows, 2 elsewhere, plus `Custom` and per-OS toggle rows | 2 on every platform, no `Custom`, no toggle rows | Story criteria of US-003 |
| Places that decide a backdrop opacity | `constants.rs`, `pane.rs` and the `material_veiled` flag, with per-OS branches | 1 module | `material_veiled` has 0 occurrences in `src-app/src`, and the D6 constants are defined only in the veil module (US-005) |
| Blended on a Wayland compositor that offers blur, with no extra key | sharp desktop | blurred desktop | US-007, real GNOME 51 session |
| Shell veil opacity where the system cannot blur | 0.72 dark, 0.80 light | 0.93 dark, 0.95 light | Unit test on the veil function |

## Research Findings

Web research was not run: every design question was answered by source code (Paneflow, the pinned GPUI checkout, Zeron and zgx) or by a live test. Zeron paths are in a local clone at `/home/arthur/dev/zeron`, which is local evidence and not tracked here. zgx facts come from `github.com/zeronsh/zgx` at revision `b8f298b6b9d1a363adf9e21655334a6e260dc4c7`, read on 2026-11-20.

| # | Finding | Source |
|---|---------|--------|
| R1 | Zeron asks the system for an untinted blur behind the window on every platform and lays the theme's own tint over it (`glass.tint`). | `apps/zgx/src/main.rs:1269-1280` (Zeron) |
| R2 | Zeron thickens the tint where the desktop blurs less: full see-through from 40 points of blur, 25 % of it with no blur, linear between; the new opacity is `1 - (1 - opacity) * keep`. | `apps/zgx/ui/ds/glass_tint.luau` (Zeron) |
| R3 | zgx reports whether the Wayland compositor can blur by listing the registry globals `org_kde_kwin_blur_manager` and `ext_background_effect_manager_v1`. GNOME before 51, sway and Weston offer neither. | `crates/zgx-desktop/src/wayland_blur.rs` (zgx) |
| R4 | The vendored GPUI Wayland backend already binds both protocols and sets a blur region on the surface. | `vendor/gpui_linux/src/linux/wayland/window.rs:2562-2584`, `vendor/gpui_linux/src/linux/wayland/client.rs:259-260` |
| R5 | Paneflow already probes those two globals off the main thread and has a tested decision table for Wayland, X11 and client-side decorations. | `src-app/src/window_chrome/linux_backdrop.rs:74-152`, tests at `:181-352` |
| R6 | GNOME Shell 51.1 advertises `ext_background_effect_manager_v1` with Blur my Shell disabled, and Paneflow's blur request is honored there. | Live test on Arthur's machine, 2026-11-20 (`wayland-info`, debug build on an isolated home, visual confirmation by Arthur) |
| R7 | On macOS, GPUI's `WindowBackgroundAppearance::Blurred` installs a `BlurredView`, an `NSVisualEffectView` subclass whose `updateLayer` hides the desktop-tint layer and removes the saturation filter. | Pinned GPUI `703b5ac3`, `crates/gpui_macos/src/window.rs:275-283`, `:1958-1978`, `:4059-4095` |
| R8 | On Windows, GPUI's `Blurred` sets the accent policy state 4 (acrylic blur behind) through `SetWindowCompositionAttribute`; `MicaBackdrop` sets `DWMWA_SYSTEMBACKDROP_TYPE`. | Pinned GPUI `703b5ac3`, `crates/gpui_windows/src/window.rs:918-940`, `:1630-1664` |
| R9 | Paneflow rejected the accent blur on Windows because it lags while the window is dragged, and Blended uses the Acrylic system backdrop (`DWMSBT_TRANSIENTWINDOW`) instead; `blurred` set in the config resolves to `auto` there. | `DESIGN.md:1993`, `src-app/src/app/constants.rs:51-57`, `src-app/src/window_chrome/backdrop.rs:41-46` |
| R10 | Windows turns a system backdrop solid by itself when Transparency effects are off, so an app has nothing to change for it; only the accent blur needs the app to follow the setting. An Acrylic system backdrop also goes solid while its window is inactive. | zgx `crates/zgx-desktop/src/window_chrome.rs:384-396` and the `Glass` notes quoted in Zeron `apps/zgx/src/main.rs:1269-1277` |
| R11 | The system backdrop needs Windows 11 build 22621; below it Paneflow opens an opaque window. | `src-app/src/app/constants.rs:88-97`, `:141-167` |
| R12 | Current veil values: shell 0.72 on dark themes and 0.80 on light ones, terminal card shade 0.35. The macOS material is dropped in fullscreen. | `src-app/src/app/constants.rs:21-22`, `src-app/src/pane.rs:266`, `src-app/src/window_chrome/mod.rs:20-22`, commit `bec5125a` |
| R13 | `docs/user/` is an auto-generated mirror of the site repository and must not be edited by hand. | `docs/user/README.md:3-6` |
| R14 | The runtime ignores unknown top-level config keys; `schemas/paneflow.schema.json` is an editor aid. Tests keep it in step with the `PaneFlowConfig` fields (`crates/paneflow-config/src/schema.rs:188-199`) and require `docs/user/configuration/schema.md` to mention every schema key (`:345-353`). | `schemas/paneflow.schema.json:5`, `crates/paneflow-config/src/schema.rs` |

## Technical Decisions

### D1. One key, `background_style`, with read-time resolution of the legacy keys

- **Options:** (a) rewrite `paneflow.json` at startup, (b) keep the seven keys and hide them, (c) add one key and resolve the old ones when it is absent.
- **Choice:** (c). The file is edited by hand and may be shared between machines, and the repository never rewrites user config at startup. Settings already owns the write path (`appearance.rs:915-923`).
- **Rule**, evaluated in order, legacy keys considered only for the running platform:
  1. `background_style` present and valid: use it. Present and invalid: `themed`, with one warning in the log.
  2. `window_backdrop` is `opaque` or `off`: `themed`. On macOS, `transparent` also gives `themed` (it disables the material today, `config.rs:345-352`).
  3. Any legacy material key of the running platform is `true` (`windows_chrome_material`, `windows_terminal_material`, `macos_chrome_material`, `linux_chrome_material`, `linux_terminal_material`): `blended`.
  4. Otherwise `themed`.
- **Consequence:** a config that turned on only one of two materials (shown as `Custom` today) becomes `blended`. A Windows config on the Mica style becomes `blended`.

### D2. Mica is removed as a style

- **Options:** keep `mica` as a third, Windows-only value, or drop it.
- **Choice:** drop it. Mica tints with the wallpaper and blurs nothing (R10, zgx changelog `windows-plain-glass-blur.md`), so it is a different material from "a blur under our veil" and would keep a Windows-only branch in the veil module. Arthur agreed to the single model on 2026-11-20. This is listed under [Open Questions](#open-questions) because it removes a shipped option.

### D3. Windows keeps the Acrylic system backdrop

- **Options:** (a) GPUI `Blurred`, the accent blur Zeron uses, (b) the Acrylic system backdrop Blended uses today.
- **Choice:** (b). Paneflow already measured (a) as lagging on drag (R9). (b) is a documented DWM attribute, works today, and Windows handles Transparency effects for it (R10), so no registry watcher is needed. Its known limit, a solid backdrop while the window is inactive (R10), is accepted: the veil then blends the theme color at 0.72 or 0.80 over the system's solid fallback color, and no desktop content shows through.
- **Below build 22621** the window stays opaque (R11) and the effective style is Themed (D5).

### D4. macOS uses GPUI's `Blurred`

- **Options:** (a) keep the custom Sidebar `NSVisualEffectView`, (b) port zgx's objc2 glass view, (c) GPUI `Blurred`.
- **Choice:** (c). It already provides the untinted blur the model needs (R7), so `macos_backdrop.rs` and its `cocoa` and `objc` calls are deleted rather than migrated to objc2. `app_icon.rs` is the only other user of those crates and is out of scope.
- **Consequence:** macOS gains a translucent terminal card and loses the system Sidebar tint. In fullscreen the effective style is Themed (D5), as the material is dropped today (R12).

### D5. `BlurSupport` has two values

- **Options:** a continuous blur amount in points as Zeron has, or `Full` and `None`.
- **Choice:** `Full` and `None`. Only Hyprland's socket can report an amount (Zeron `apps/zgx/src/desktop_blur.rs`), and that measurement is a non-goal. The veil function takes a `keep` factor internally so a later partial value needs no new call site.
- **Effective style.** Blended needs a window surface with alpha. Where the window has none, the effective style is Themed and `BlurSupport` is not consulted. One platform-neutral function, `effective_style(style, surface_has_alpha)`, returns it. `surface_has_alpha` is false on Windows below build 22621 and on a fullscreen macOS window, and true everywhere else.
- **Sources of `BlurSupport`**, consulted only when the effective style is Blended:

| Platform | `Full` when | `None` when |
|----------|-------------|-------------|
| Linux Wayland | the probe found either blur global | no global, the probe failed, or the probe has not returned yet |
| Linux X11 and other surfaces | never | always |
| macOS, not fullscreen | always | never |
| Windows, build 22621 or later | always | never |

`None` therefore arises only on Linux, and through the debug override of D7 on any platform.

### D6. Veil values

| Surface | Effective style Themed | Blended, `Full` | Blended, `None` |
|---------|--------|-----------------|-----------------|
| Shell (sidebar, title bar, gutters) | opaque theme color | theme color at 0.72 (dark) or 0.80 (light) | theme color at 0.93 (dark) or 0.95 (light) |
| Terminal card | opaque theme background | black (dark) or white (light) at 0.35 over the shell veil | opaque theme background |

0.72, 0.80 and 0.35 are today's values (R12). 0.93 and 0.95 are `1 - (1 - base) * 0.25` with Zeron's `LEAST` (R2), rounded to two decimals. Whether 0.25 suits Paneflow's themes is assumption A1.

### D7. `window_backdrop` and `PANEFLOW_WINDOW_BACKDROP` are retired

`window_backdrop` is read only by the legacy rule of D1. `PANEFLOW_WINDOW_BACKDROP` is removed. A debug-only `PANEFLOW_DEV_BLUR_SUPPORT` (`full` or `none`) replaces it for testing, in line with the existing `PANEFLOW_DEV_*` variables (`AGENTS.md`, "Running and testing").

## Assumptions & Constraints

| # | Assumption | Risk | Validation |
|---|------------|------|------------|
| A1 | Keeping 25 % of the see-through reads as color, not shapes, on Paneflow's dark and light themes. | Medium: a wrong value is a one-constant change. | US-006. Dark theme observed 2026-11-20 by Arthur on GNOME 51.1 Wayland (debug build, `veil` tagged home, `PANEFLOW_DEV_BLUR_SUPPORT=none`): the shell read as a faint tint with no shapes, the terminal card as opaque. Light theme observed the same day with the same override: the sidebar and the panes read as near-opaque color, with no shapes. That run kept the compositor blur on behind the window; the light veil (0.95) lets through less than the dark one (0.93), which showed no shapes over a sharp desktop. The `Full` column (real blur) was approved the same day for the sidebar and the panes. **Confirmed**, 0.25 kept |
| A2 | Treating an offered Wayland blur as `Full` is right even on a compositor configured for a weak blur (Hyprland with a small radius). | Low: the result is today's veil. | None, accepted; see Non-Goals |
| A3 | GPUI's `BlurredView` follows the window's rounded corners and leaves the shadow intact under Paneflow's client-side chrome on macOS. | High: not verifiable on Linux. | US-017, on an Apple Silicon bare-metal Mac (EP-007) |
| A4 | With Reduce transparency on, macOS renders the `NSVisualEffectView` solid, so no desktop content shows through the veil, with no code of ours. | Medium | US-018, on an Apple Silicon bare-metal Mac (EP-007) |
| A5 | With Transparency effects off, or the window inactive, the Acrylic system backdrop is solid, so no desktop content shows through the veil and the window is not black. | Medium | US-014 |
| A6 | The Wayland probe, one registry round trip, usually answers before the first frame is presented; when it does not, the first frames use the thick veil and the veil thins once. | Low | US-007 logs the probe's duration. Measured 2026-11-20 on GNOME Shell 51.1 Wayland (debug build, `ep003-blur` tagged home): 2.5 ms and 4.1 ms at startup, 0.36 ms on a hot reload from Themed to Blended |
| A7 | KDE's `org_kde_kwin_blur_manager` path in the vendored backend blurs as the `ext-background-effect-v1` path does. Only GNOME 51 was tested live (R6). | Low: the path is upstream GPUI code, unchanged here, and a failure leaves today's sharp veil. | None on Arthur's machine; accepted, Open Question 5 |

Constraints: no blocking work on the GPUI main thread (`AGENTS.md`, "The render thread never blocks"); no source comments; `cfg(windows)` code is linted only by the Windows CI leg and any item added to a file with a `mod tests` goes before it; Windows behavior is verified on real hardware after a push, in EP-006, and macOS behavior on an Apple Silicon bare-metal Mac, in EP-007.

## Non-Goals

- **Blur inside the window** (frosted menus, popovers, toasts). GPUI has no backdrop-blur primitive; menus stay opaque under Blended, as `DESIGN.md:908-910` already requires.
- **Measuring how much the compositor blurs** (Hyprland's IPC socket). `BlurSupport` stays two-valued.
- **GPUI's accent blur on Windows**, and any blur below Windows 11 build 22621.
- **Following `EnableTransparency` in the registry.** The system backdrop does it (R10).
- **Liquid Glass (`NSGlassEffectView`) and an objc2 migration.** D4 deletes the code that would have needed them; `app_icon.rs` keeps `cocoa` and `objc`.
- **Changing the default.** A config with none of the keys stays Themed.
- **A user-editable tint color or opacity.** The veil comes from the theme and the constants of D6.
- **Rewriting `docs/user/`.** The public docs change in the site repository after this ships. The one exception is the `background_style` row that a repository test requires in `docs/user/configuration/schema.md` (US-002).
- **Changing `window_decorations`**, the corner masks, or the panel geometry.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI style gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints on the host target
- `cargo nextest run --workspace --locked --profile ci` - the test suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget in tests

## Epics & User Stories

### EP-001: One `background_style` key

The configuration crate and Settings speak one key. The app still compiles against the existing derived accessors, which now read the resolved style.

**Definition of Done:** `PaneFlowConfig::resolved_background_style()` returns `Themed` or `Blended` for every combination of the eight keys per D1, Settings writes only `background_style`, and the existing accessors return values derived from it.

#### US-001: Resolve the background style from the new key and the legacy keys
**Description:** As a user upgrading Paneflow, I want my window to keep its look so that the new key costs me nothing.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given a config with `"background_style": "blended"`, when it is loaded, then `resolved_background_style()` returns `Blended` whatever the legacy keys hold
- [x] Given a config with no `background_style` and a legacy material key of the running platform set to `true`, when it is loaded, then the result is `Blended`
- [x] Given a config with no `background_style` and `"window_backdrop": "opaque"` or `"off"`, when it is loaded, then the result is `Themed` even if a legacy material key is `true`
- [x] Given a config with none of the eight keys, when it is loaded, then the result is `Themed`
- [x] Given legacy material keys set to `true` for another platform only, when the config is loaded, then the result is `Themed`
- [x] Given `"background_style": "mica"` or any other unknown string, when the config is loaded, then the result is `Themed` and the load does not fail
- [x] Given `"background_style": 3`, when the config is loaded, then the result is `Themed` and the rest of the file still loads
- [x] `BackgroundStyle` has exactly the variants `Themed` and `Blended` and `BackgroundStyle::ALL` is the same on every platform

#### US-002: Write the new key and drop the legacy ones
**Description:** As a user, I want Settings to leave one line in my config so that the file says what I chose.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given a config holding legacy material keys, when a background style is saved through the config writer, then the file contains `background_style` with the chosen value
- [x] Given the same save, then the file no longer contains any of the seven legacy keys
- [x] Given a config with unrelated keys and custom formatting, when a background style is saved, then every unrelated key keeps its value
- [x] Given the config file cannot be written, when a background style is saved, then the writer reports failure and the in-memory config is unchanged
- [x] `schemas/paneflow.schema.json` declares `background_style` with the enum `themed`, `blended`
- [x] `schemas/paneflow.schema.json` marks each of the seven legacy keys `deprecated`
- [x] `docs/user/configuration/schema.md` gains one table row for `background_style`, the only edit to `docs/user/` in this PRD, because `public_configuration_schema_doc_mentions_schema_keys` (`crates/paneflow-config/src/schema.rs:345-353`) requires every schema key there
- [x] The rows of the seven legacy keys stay in `docs/user/configuration/schema.md`, since the keys stay in the schema

#### US-003: One background style control in Settings
**Description:** As a user, I want one Background style choice so that I do not have to understand per-platform switches.

**Priority:** P0
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [x] Given Settings > Appearance on any platform, when the Background style select opens, then it lists exactly Themed and Blended
- [x] Given any config, when the select renders, then it never shows `Custom`
- [x] Given the user picks Blended, when the choice is saved, then open windows repaint with the new style without a restart (observed 2026-11-20 by Arthur on GNOME 51.1 Wayland, debug build on the `backdrop` tagged home: the main and a detached window turned translucent at once, and opaque again on Themed)
- [x] The hot-reload match in `src-app/src/app/settings.rs` (`:243-248` today) propagates the config to every workspace when the saved key is `background_style`
- [x] The Windows chrome material, macOS sidebar material and Linux chrome material rows are gone from Settings > Appearance
- [x] The Windows and Linux terminal material rows are gone from Settings > Terminal
- [x] Settings search returns the Background style row for the queries "material", "transparency" and "blur", and returns no removed row
- [x] Given the save fails, when the user picks a style, then a toast reports the failure and the select keeps showing the previous style

---

### EP-002: One veil module

Every fill that depends on the backdrop is computed in one place from the style and `BlurSupport`.

**Definition of Done:** the veil module is the only place that holds the opacities of D6, `material_veiled` has no occurrence in `src-app/src`, and unit tests cover each cell of the D6 table.

#### US-004: `BlurSupport` and the veil functions
**Description:** As the maintainer, I want the veil rules in one tested module so that a change to the look is a change in one file.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] A module under `src-app/src/window_chrome/` defines `BlurSupport` with the variants `Full` and `None`
- [x] Given Blended, `Full` and a dark theme color, when the shell veil is computed, then its alpha is 0.72; with a light theme color, 0.80
- [x] Given Blended and `None`, when the shell veil is computed, then its alpha is 0.93 on a dark theme color and 0.95 on a light one
- [x] Given Blended and `Full`, when the terminal card fill is computed, then it is black at 0.35 on a dark theme and white at 0.35 on a light one
- [x] Given Blended and `None`, when the terminal card fill is computed, then it is the theme background with alpha 1
- [x] Given Themed, when either fill is computed, then its alpha is 1 whatever `BlurSupport` is
- [x] Given Blended and `surface_has_alpha` false, when `effective_style` is called, then it returns Themed
- [x] Given Blended and `surface_has_alpha` true, when `effective_style` is called, then it returns Blended
- [x] Given a `keep` factor outside 0 to 1, when the thickening function is called, then it clamps the factor and returns an alpha between the base alpha and 1

#### US-005: Route every backdrop-dependent fill through the veil module
**Description:** As the maintainer, I want no call site to decide an opacity by platform so that the three platforms cannot drift apart.

**Priority:** P0
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [x] `cockpit_shell_background`, `cockpit_chrome_background`, `cockpit_backdrop_background`, `sidebar_tab_fill` and `window_border_color` in `src-app/src/app/constants.rs` take their result from the veil module and contain no `cfg!(target_os` test
- [x] `pane_card_background` in `src-app/src/pane.rs` takes its result from the veil module and has no `#[cfg(target_os` branch
- [x] `material_veiled` has no occurrence in `src-app/src` or `crates/paneflow-config/src`
- [x] The opacity constants 0.72, 0.80 and 0.35 are defined only in the veil module
- [x] Given Themed, when the main window and a detached pane window render, then every shell and terminal fill is the same color as before this story, checked by the existing `material_tests` adapted to the new functions
- [x] Given a window whose `BlurSupport` changes while it is open, when the next frame renders, then the shell veil uses the new value
- [x] Given a config whose `background_style` is invalid, when a window renders, then every shell and terminal fill is the Themed one

#### US-006: A debug override for `BlurSupport`, and the visual check of the thick veil
**Description:** As the maintainer, I want to force `BlurSupport` in a debug build so that I can see the degraded look on a machine that blurs.

**Priority:** P1
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Given a debug build started with `PANEFLOW_DEV_BLUR_SUPPORT=none`, when a Blended window renders, then it uses the `None` column of D6
- [x] Given a debug build started with `PANEFLOW_DEV_BLUR_SUPPORT=full`, when a Blended window renders on a surface that cannot blur, then it uses the `Full` column of D6
- [x] Given a release build, when `PANEFLOW_DEV_BLUR_SUPPORT` is set, then it has no effect
- [x] Given an unknown value of the variable, when a debug build starts, then one warning is logged and the detected `BlurSupport` is used
- [x] `AGENTS.md`, "Running and testing", lists `PANEFLOW_DEV_BLUR_SUPPORT`
- [x] Assumption A1 is recorded as confirmed or adjusted in this PRD after Arthur views a dark and a light theme with the override set to `none` on a debug build with its own tagged home

---

### EP-003: Linux blurs whenever Blended is on

Blended asks the compositor for blur by itself, and the probe's answer drives the veil.

**Definition of Done:** on a Wayland session that offers a blur protocol, Blended shows a blurred desktop with no key other than `background_style`; on a session that offers none, it shows the thick veil; both are covered by the decision-table tests.

#### US-007: Request blur from the style, and feed the probe into `BlurSupport`
**Description:** As a GNOME 51 or KDE user, I want Blended to blur the desktop behind Paneflow so that I do not need an extension or a hidden key.

**Priority:** P0
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Given Blended on a Wayland session whose registry lists `ext_background_effect_manager_v1` or `org_kde_kwin_blur_manager`, when the window opens, then its background appearance becomes `Blurred` and `BlurSupport` is `Full`
- [x] Given the same session and a config that holds only `"background_style": "blended"`, when the window opens on GNOME 51 with Blur my Shell disabled, then the desktop behind the window is blurred, checked on a debug build with its own tagged home (observed 2026-11-21 on GNOME Shell 51.1 Wayland, `ep003-blur` tagged home, Blur my Shell disabled: the wallpaper's stars and streaks read as smooth color bands behind the window, against sharp stars behind the same 0.72 veil on an XWayland reference run with `PANEFLOW_DEV_BLUR_SUPPORT=full`; screenshots kept as local evidence)
- [x] Given Themed, when the window opens on any session, then no blur is requested and the appearance is the unblurred one the decision table returns today
- [x] Given Blended while the probe has not answered, when a frame renders, then `BlurSupport` is `None`
- [x] Given the probe answers `Offered`, when the next frame renders, then `BlurSupport` is `Full` without a restart
- [x] The probe runs off the GPUI main thread, as `request_compositor_blur` does today
- [x] The probe's duration is logged once at debug level (assumption A6)
- [x] Given a detached pane window, when it opens under Blended, then it follows the same rules as the main window
- [x] `window_backdrop_requests_blur` and every read of `window_backdrop` are removed from `src-app/src/window_chrome/linux_backdrop.rs` and its callers

#### US-008: Degrade on sessions that cannot blur
**Description:** As a user on X11, sway or an older GNOME, I want Blended to switch to the thick veil so that the windows behind show through at no more than 7 % strength.

**Priority:** P0
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given Blended on a Wayland session with neither blur global, when the window renders, then `BlurSupport` is `None` and the appearance is `Transparent`
- [x] Given Blended on X11, when the window renders, then `BlurSupport` is `None` whatever the probe state
- [x] Given the probe cannot connect to the compositor, when Blended renders, then `BlurSupport` is `None` and one warning is logged
- [x] Given Blended falls back to `None`, when the session runs, then the fallback is logged once and the log line no longer names `window_backdrop`
- [x] The decision-table tests in `linux_backdrop.rs` cover every combination of style, surface kind and probe state

---

### EP-004: macOS uses the untinted system blur

macOS Blended becomes the same veil over a blur as the other platforms. The `cfg(target_os = "macos")` code of this epic cannot run on the Linux development host: it is reviewed by inspection and compiled by the macOS CI legs. Every check on a real Mac, visual passes included, belongs to EP-007.

**Definition of Done:** `src-app/src/window_chrome/macos_backdrop.rs` no longer exists, the macOS CI legs (`macOS aarch64 smoke build` and `macOS aarch64 render smoke (visual)`) pass, and the fullscreen rule is covered by a platform-neutral unit test. Met 2026-11-21 on `5033a596`: both macOS legs green, as were every other `run_tests` job; the Windows leg had first failed on `db685ad1` because the fullscreen test read the runner's Windows build, fixed in that commit.

#### US-009: Replace the Sidebar material with GPUI's `Blurred`
**Description:** As a macOS user, I want Blended to blur the desktop behind the whole window so that Paneflow looks the same as on the other platforms.

**Priority:** P0
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [x] Given Blended on macOS and a window that is not fullscreen, when the window renders, then its background appearance is `Blurred` and `BlurSupport` is `Full` (reviewed by inspection on Linux, `cfg(target_os = "macos")` code compiled only by the macOS CI legs; neutral logic tested on every host in `window_chrome::system_blur::tests`)
- [x] Given Themed on macOS, when the window renders, then its background appearance is the one used today for a config with no material key (`Transparent`, as `window_backdrop` absent resolved to before)
- [x] Given the user switches between Themed and Blended, when the config hot-reloads, then the appearance changes without a restart
- [x] `src-app/src/window_chrome/macos_backdrop.rs` is deleted and no caller of `apply_subtle_sidebar_material`, `sync_subtle_sidebar_material` or `SidebarMaterial` remains
- [x] Given a detached pane window on macOS, when it opens under Blended, then it uses the same appearance and veil as the main window
- [x] `cocoa` and `objc` remain in `src-app/Cargo.toml` only because `src-app/src/app_icon.rs` uses them (technical correction, 2026-11-21: `app_icon.rs` uses only `objc`, so `cocoa` and the macOS `raw-window-handle` entry left with `macos_backdrop.rs`; `objc` remains for `app_icon.rs` alone)
- [x] Given Blended on macOS and a hot-reload to an invalid `background_style` value, when the window repaints, then its appearance is the Themed one and no blur view remains

#### US-010: The fullscreen rule on macOS
**Description:** As a macOS user, I want Blended to drop its blur in fullscreen so that the window never shows a dead, flat tint on its own Space.

**Priority:** P1
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Given Blended and a fullscreen window, when it renders, then the effective style is Themed and the appearance is not `Blurred`
- [x] Given the window leaves fullscreen, when the next frame renders, then the appearance is `Blurred` again
- [x] A unit test covers the fullscreen rule on every host through a platform-neutral function (`a_fullscreen_window_drops_the_blur_and_gets_it_back_on_leaving`, on a GPUI test window, and `a_platform_that_drops_alpha_in_fullscreen_keeps_it_windowed`)

---

### EP-005: Windows code path, cleanup and documentation

The Windows code follows the single model, the legacy accessors disappear, and the tracked documents describe what ships. Windows code in this epic is reviewed by inspection and linted by the Windows CI leg.

**Definition of Done:** the guard test of US-012 passes, every criterion of US-013 holds, and the `Windows x86_64 libghostty check` job passes.

#### US-011: Blended on Windows is the Acrylic system backdrop under the veil
**Description:** As a Windows user, I want Blended to keep the blur it has today so that the new model changes nothing I liked.

**Priority:** P0
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given Blended on build 22621 or later, when the window opens, then the system backdrop type is `DWMSBT_TRANSIENTWINDOW` and `BlurSupport` is `Full`
- [ ] Given a build below 22621, when the window opens, then its appearance is `Opaque` and the effective style is Themed
- [ ] Given the user switches between Themed and Blended, when the config hot-reloads, then the window repaints without a restart
- [ ] Given the theme switches between light and dark, when the window renders, then `DWMWA_USE_IMMERSIVE_DARK_MODE` follows it as today
- [ ] `WindowBackdropPreference`, `window_backdrop_preference`, `window_backdrop_uses_mica` and every read of `PANEFLOW_WINDOW_BACKDROP` are removed
- [ ] No code path sets `WindowBackgroundAppearance::Blurred` on Windows
- [ ] Every item added to a file that has a `mod tests` is declared before that module
- [ ] This story's `cfg(windows)` changes are stated as reviewed by inspection, pending EP-006

#### US-012: Remove the legacy accessors and guard against their return
**Description:** As the maintainer, I want the old vocabulary gone so that nobody adds an eighth key.

**Priority:** P0
**Dependencies:** Blocked by US-003, US-006, US-008, US-009, US-011

**Acceptance Criteria:**
- [ ] `windows_terminal_material_enabled`, `linux_terminal_material_enabled`, `terminal_material_enabled`, `native_material_requested`, `macos_chrome_material_enabled`, `cockpit_chrome_material_enabled`, `background_style_settings` and `BACKGROUND_STYLE_KEYS` are removed from `crates/paneflow-config`
- [ ] A guard test fails if any of the seven legacy key names appears in a `.rs` file under `src-app/src`
- [ ] The seven legacy fields stay deserializable in `PaneFlowConfig` and are read only by the D1 resolution
- [ ] `src-app/src/scene_export/agents_window.config.json` uses `background_style` and none of the legacy keys
- [ ] The hot-reload match in `src-app/src/app/settings.rs` names `background_style` and none of the six legacy material keys
- [ ] Given a config that still holds legacy keys, when the app starts, then it loads without a warning and resolves per D1

#### US-013: Tracked documentation
**Description:** As a contributor, I want the design document and the changelog to describe the single model so that the next change starts from the truth.

**Priority:** P1
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] `DESIGN.md` section 7, "Platform Materials", describes `background_style`, `BlurSupport` and the D6 table, and names none of the seven legacy keys as a current setting
- [ ] Every other `DESIGN.md` passage that names Mica as a background style is updated
- [ ] `CHANGELOG.md`, under `[Unreleased]`, states that Blended now blurs on Wayland compositors that offer it
- [ ] The same entry states that the Mica style and the per-platform material switches are replaced by `background_style`
- [ ] The same entry states that macOS Blended now covers the whole window
- [ ] The same entry states that `PANEFLOW_WINDOW_BACKDROP` is removed
- [ ] The changelog entry tells a user who relied on the Mica style or on `PANEFLOW_WINDOW_BACKDROP` what replaces it
- [ ] Given a search of `DESIGN.md` for the seven legacy key names, when a hit remains, then its sentence describes the key as legacy and read only for upgrade
- [ ] `docs/user/` differs from before this PRD only by the `background_style` row of US-002

---

### EP-006: Windows validation on real hardware

Every Windows check that needs a real machine, and the fixes they bring, on Arthur's Windows 11 dual boot.

**Definition of Done:** each criterion below is recorded as observed on Windows 11 with its build number, and every defect found is fixed and observed fixed, or tracked in a GitHub issue referenced here.

#### US-014: Blended on Windows 11
**Description:** As a Windows user, I want Blended verified on a real machine so that the release does not ship a regression nobody saw.

**Priority:** P0
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [ ] Given Blended on Windows 11, when the window sits over other windows, then what is behind it is blurred under the veil in a dark and in a light theme
- [ ] Given a screen recording of the window dragged across the display and resized from a corner under Blended, and the same recording under Themed, when the two are compared, then the Blended window follows the pointer as the Themed one does; both recordings are kept as local evidence
- [ ] Given the window loses focus, when the backdrop goes solid, then no desktop content shows through the shell and the window is not black (assumption A5)
- [ ] Given Transparency effects is turned off in Windows Settings while Paneflow runs, when the window repaints, then no desktop content shows through the shell and the window is not black (assumption A5)
- [ ] Given a detached pane window, when it opens under Blended, then it matches the main window
- [ ] Given the style is switched in Settings, when the choice is saved, then the main and detached windows repaint without a restart

#### US-015: Upgrade paths and the fallback on Windows
**Description:** As a Windows user upgrading, I want my existing config to resolve to a working window so that the update is invisible.

**Priority:** P0
**Dependencies:** Blocked by US-006, US-012

**Acceptance Criteria:**
- [ ] Given a config saved by the previous release on the Mica style, when the new build starts, then the window opens Blended and the file is not rewritten until a style is chosen
- [ ] Given a config saved on the previous Blended style, when the new build starts, then a screenshot of the window matches one taken on the previous release with the same config and theme; both are kept as local evidence
- [ ] Given a config saved on Themed, when the new build starts, then the window is opaque
- [ ] Given a debug build started with `PANEFLOW_DEV_BLUR_SUPPORT=none`, when Blended renders on Windows 11, then the shell uses the thick veil and the terminal card is opaque
- [ ] Given `"background_style": "mica"` typed by hand, when the app starts, then the window opens Themed and the app does not crash

#### US-016: Fix what the hardware pass finds
**Description:** As the maintainer, I want every Windows defect from US-014 and US-015 closed or tracked so that the epic ends with a known state.

**Priority:** P0
**Dependencies:** Blocked by US-014, US-015

**Acceptance Criteria:**
- [ ] Each failed criterion of US-014 and US-015 has a fix commit observed working on Windows 11, or a GitHub issue referenced in this PRD
- [ ] The `Windows x86_64 libghostty check` job is green on the commit that closes the epic
- [ ] Given no criterion failed, when the epic closes, then this story records that fact and changes no code
- [ ] Given a defect whose fix does not fit this story's session, when it is found, then it is tracked in a GitHub issue referenced here instead of fixed in this story
- [ ] Assumption A5 is recorded in this PRD as confirmed or as the defect that replaced it

---

### EP-007: macOS validation on real hardware

Every macOS check that needs a real Mac, visual passes included, and the fixes they bring, in one session on an Apple Silicon bare-metal Mac mini rented from Scaleway. A GitHub-hosted macOS runner is a virtual machine and does not satisfy these criteria.

**Definition of Done:** each criterion below is recorded as observed on a real Apple Silicon Mac with its macOS version and chip, and every defect found is fixed and observed fixed, or tracked in a GitHub issue referenced here.

#### US-017: Blended on macOS
**Description:** As a macOS user, I want Blended verified on a real Mac so that the release does not ship a broken corner, shadow or blur nobody saw.

**Priority:** P0
**Dependencies:** Blocked by US-010, US-012

**Acceptance Criteria:**
- [ ] Given Blended in a dark and in a light theme, when the window sits over other windows, then what is behind the whole window, sidebar, title bar and terminal card included, is blurred under the veil
- [ ] Assumption A3 is recorded in this PRD as observed, with the macOS version: the blur follows the window's rounded corners and the window shadow is intact
- [ ] Given Themed, when the window renders, then it looks as it did on the previous release with no material key, checked against a screenshot of that release kept as local evidence
- [ ] Given a detached pane window, when it opens under Blended, then it matches the main window
- [ ] Given the style is switched in Settings, when the choice is saved, then the main and detached windows repaint without a restart
- [ ] Given Blended, when the window enters fullscreen, then no blur shows and the fills are the Themed ones; when it leaves fullscreen, then the blur returns
- [ ] Given Blended and a hand edit of `background_style` to an invalid value, when the config hot-reloads, then the window turns Themed and no blur remains
- [ ] Given the window loses focus, when it renders inactive, then it is neither black nor broken, and what it shows is recorded here

#### US-018: Accessibility, upgrade paths and the fallback on macOS
**Description:** As a macOS user upgrading, I want my config and my accessibility settings to give a correct window so that the update is invisible.

**Priority:** P0
**Dependencies:** Blocked by US-006, US-010, US-012

**Acceptance Criteria:**
- [ ] Assumption A4 is recorded in this PRD as observed with Reduce transparency on: no desktop content shows through the window under Blended
- [ ] Given Reduce transparency is turned on while Paneflow runs, when the window repaints, then no desktop content shows through and the window is not black
- [ ] Given a config saved by the previous release with `"macos_chrome_material": true`, when the new build starts, then the window opens Blended and the file is not rewritten until a style is chosen
- [ ] Given a config with none of the eight keys, when the new build starts, then the window opens Themed
- [ ] Given a debug build started with `PANEFLOW_DEV_BLUR_SUPPORT=none`, when Blended renders, then the shell uses the thick veil and the terminal card is opaque

#### US-019: Fix what the macOS hardware pass finds
**Description:** As the maintainer, I want every macOS defect from US-017 and US-018 closed or tracked so that the epic ends with a known state.

**Priority:** P0
**Dependencies:** Blocked by US-017, US-018

**Acceptance Criteria:**
- [ ] Each failed criterion of US-017 and US-018 has a fix commit observed working on the Mac, or a GitHub issue referenced in this PRD
- [ ] The `macOS aarch64 smoke build` and `macOS aarch64 render smoke (visual)` jobs are green on the commit that closes the epic
- [ ] Given no criterion failed, when the epic closes, then this story records that fact and changes no code
- [ ] Given a defect whose fix does not fit the rented session, when it is found, then it is tracked in a GitHub issue referenced here instead of fixed in this story
- [ ] Assumptions A3 and A4 are recorded in this PRD as confirmed or as the defect that replaced them

## Edge Cases & Error States

| Scenario | Expected behavior |
|----------|-------------------|
| `background_style` and legacy keys disagree | `background_style` wins (D1, rule 1) |
| Config shared between two operating systems with different legacy material keys | Each machine resolves from its own platform's keys until a style is saved; after that both follow `background_style` |
| KDE with the blur effect turned off | The global is absent, so `BlurSupport` is `None` and the veil thickens |
| The compositor gains or loses the blur global while Paneflow runs | Not followed: the probe runs once per process, as today (the `OnceLock` at `linux_backdrop.rs:15`, checked at `:156`) |
| Server-side decorations on Linux | The decision table keeps its current rule for explicit alpha on X11 (`linux_backdrop.rs:35-46`) |
| macOS fullscreen | Effective style Themed, no blur view |
| Windows below build 22621 | Opaque window, effective style Themed |
| Menus, popovers and the usage bubble under Blended | Opaque, unchanged (`DESIGN.md:900-910`) |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| macOS Blended cannot be checked on hardware before release | Medium | A broken corner, shadow or blur ships | EP-007 checks A3, A4 and every visual criterion on an Apple Silicon bare-metal Mac before release; US-012 depends only on US-009, so the code epics proceed without a Mac |
| Existing Mica users dislike the forced move to Blended | Medium | Complaints after upgrade | Changelog entry in US-013 says what changed; D2 is reversible by adding a third value |
| The thick veil looks too opaque or too thin | Medium | Cosmetic | One constant, checked visually in US-006, inside EP-002 and before the cleanup of US-012 |
| A `cfg(windows)` lint fails on the Windows leg only | Medium | One red CI run | Items before `mod tests`, inspection, and the Windows leg as the gate of US-011 and US-016 |
| A fill is missed in US-005 and keeps a platform branch | Low | One surface drifts | The `material_veiled` and legacy-key guard tests |

## Open Questions

1. **Dropping Mica (D2).** It removes a shipped Windows option. Keeping it means a third `background_style` value on Windows only and one extra column in D6.
2. **macOS hardware.** Resolved 2026-11-21: Arthur runs EP-007 on an Apple Silicon bare-metal Mac mini rented from Scaleway (Apple's license sets a 24-hour minimum lease, so the whole epic fits one session). A GitHub `macos-15` runner is a virtual machine with a paravirtualized GPU and does not count as a real Mac for these criteria.
3. **The default.** The PRD keeps Themed as the default. Making Blended the default would change every window that has no key.
4. **Public docs.** `docs/user/configuration/schema.md`, `settings.md` and `troubleshooting.md` describe the seven keys and `PANEFLOW_WINDOW_BACKDROP`; they are regenerated from the site repository (R13), which needs its own change after this ships, including the `background_style` row added here by US-002.
5. **KDE.** Assumption A7 has no validation on Arthur's machine. A KDE user's report, or a KDE session in a virtual machine, would close it.

## Files NOT to Modify

- `docs/user/**`: auto-generated mirror of the site repository (R13). Single exception: the `background_style` row in `docs/user/configuration/schema.md` required by US-002.
- `vendor/gpui_linux/**`, `vendor/gpui_wgpu/**`, `vendor/cosmic-text/**`: the Wayland blur protocols are already bound (R4); a change here must be re-applied on every GPUI bump.
- `src-app/src/app_icon.rs`: the remaining `cocoa` and `objc` user, out of scope (D4).
- `src-app/src/window_chrome/csd.rs`, `title_bar.rs`, `title_band.rs`: decorations and caption geometry are not part of the backdrop model.
- `native/libghostty/**`: generated bindings and pinned archives.
- `tasks/prd-*.md` and `tasks/prd-*-status.json` of other PRDs.
[/PRD]
