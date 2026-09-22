[PRD]
# PRD: Automatic Contrast Correction

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-20 | Arthur Jean | Initial draft, decisions fixed after the Ghostty source review (`C:\dev\ghostty`, HEAD `27e8b3fa8`) and the Superlogical ACC announcement |
| 1.1 | 2026-09-20 | Arthur Jean | Added EP-006: UI role contrast invariants (test-only) so GUI legibility on light presets is locked by tests rather than corrected at runtime |
| 1.5 | 2026-09-21 | Arthur Jean | EP-004 certified. `layout_220x60_acc60` lands at +5.7% against `layout_220x60` (214.9 us versus 203.4 us) with an identical 1195 allocs/iter, so the correction path allocates nothing per cell. Two review fixes: the ACC delta line moved from the second scenario to just before the comparison table so US-012 AC-3 reads next to the numbers it qualifies, and the two contrast-cache counter tests now hold `theme_generation()` stable across their measurement window, since five tests in the same binary move that global in parallel |
| 2.0 | 2026-09-21 | Arthur Jean | EP-006 certified and the PRD closed. Arthur ran the US-017 AC-5 visual pass on the sidebar, the Changes dock and Settings across the nine changed variants and reported no regression, which was the last criterion left on the epic. All six epics and all seventeen stories are DONE |
| 1.9 | 2026-09-21 | Arthur Jean | The corpus harness now measures the theme the app actually paints. `corpus_theme` went through `theme_by_name` alone while `resolve_theme_name` always applies `apply_surface_overrides`; it now goes through `app_theme_by_name`, a `#[cfg(test)]` helper in `src-app/src/theme/mod.rs` that chains the two. Paneflow Dark is the only preset affected, because it is the only dark one with `ui: None`: its background moves from `#282c34`, which is never rendered, to `#181818`. Three goldens move and every moved line is a Paneflow Dark line, verified by blessing each one before and after the realignment. In `contrast_corpus_share.txt`, `lsd-la.ansi` goes from lc60 84 to 132 and from xterm_lc45 196 to 250, and `lazygit.ansi` from lc45 137 to 176 of 176, which clears the anomaly recorded in 1.8 where Cursor Dark, on an identical ANSI palette, already reached 176; no `acc_*` column drops. In `contrast_corpus_pull.txt` the preset drops from four pulls to two: `rgb(59,130,246)` moves from ratio 0.58 pulled to 0.64 unpulled and `rgb(239,68,68)` from 0.56 to 0.62, because a darker background needs a smaller lightness lift and therefore drains less chroma. That is a convergence, not a loss: Vercel Dark, Claude Dark, Cursor Dark and Tailwind Dark all pull exactly `rgb(99,102,241)` and `rgb(139,92,246)`, and Paneflow Dark now does the same instead of standing out with four. In `contrast_corpus_harmony.txt` only the OKLab distances move; `hue_drift` stays 0.00 and every chroma is still carried through unchanged, so the EP-003 hue invariant holds. One fixture had to follow: `a_drained_truecolor_reaches_the_theme_through_the_layout` used `Spec(255,0,255)`, whose drain on `#282c34` was an artifact of the phantom background, and now uses `Spec(139,92,246)`, a source the pull golden records as pulled on the real one. `contrast_corpus_light_indexed_gap` did not drift. The two twin helpers take the same shortcut with no measurable consequence and stay as they are: `preset_theme` in `src-app/src/theme/palette.rs` only asserts relative invariants, checked green under the realigned theme, and the `src-app/src/terminal/element/color.rs` tests are properties also evaluated against an arbitrary background |
| 1.8 | 2026-09-21 | Arthur Jean | EP-006 certified after one review fix and two corrections to the 1.7 record. The ANSI exemption is no longer keyed on the slot name: a slot is exempt only when it measures below Lc 1 against `ansi_background`, which is `black` on the five dark presets and `bright_white` on Tailwind Light alone. Paneflow Light, Vercel Light, Claude Light and Cursor Light set `bright_white` to a dark ink (`#383a42` on Paneflow Light, Lc 96.2), so those four now assert all sixteen slots rather than fifteen, and `only_a_palette_background_endpoint_is_exempt_from_the_ansi_rows` pins the exempt set per variant so a future preset cannot widen it silently. First correction: the lazygit named-ANSI count reaches 176 of 176 on Vercel Dark, Claude Dark, Cursor Dark and Tailwind Dark, not on all five dark presets. Paneflow Dark stays at 137 because `corpus_theme` skips `apply_surface_overrides` and measures against `#282c34`, a background the app never paints, while the role table measures the `#181818` it does; that harness gap belongs to US-003, not EP-006. Tailwind Light goes from 69 to 172 of 176. Second correction: US-017 moved 38 slots across nine variants, not 30, and the pre-US-017 worklist was 36 failing rows, 20 ANSI and 16 UI |
| 1.7 | 2026-09-21 | Arthur Jean | EP-006 implemented. Two measured corrections to the US-016 table. First, the ANSI row asserts fifteen of the sixteen slots: the slot a terminal palette defines as its background endpoint (`black` on a dark theme, `bright_white` on a light one) sits at Lc 0 by construction on every preset, including Vercel Dark where both are `#000000`, and TUIs use it as a foreground only over colored backgrounds, so requiring Lc 45 of it would redefine the slot rather than fix a preset. Second, `vc_word_added` and `vc_word_deleted` are alpha washes painted under text, not text roles, so the row measures `text` over the word wash composited on the line wash, which is the stack the diff element actually paints and the only reading under which APCA is the right metric; all ten variants pass it. US-017 then moved 30 slots across nine variants, six of them collateral moves that keep a normal and bright ANSI twin apart after the normal slot was lifted. The lazygit fixture is the proof: its named-ANSI cells at Lc 45 go from 137 to 176 of 176 on all five dark presets and from 69 to 172 on Tailwind Light, recorded in `contrast_corpus_share.txt` |
| 1.6 | 2026-09-21 | Arthur Jean | EP-005 certified. The stepper row, the palette scope and the docs mirror all land on one ladder: `MINIMUM_CONTRAST_STEPS` in `src-app/src/settings/tabs/terminal.rs` is the single source for the six values, and the palette scope reads it through `minimum_contrast_step`/`minimum_contrast_setting` so Auto is marked on the key's absence rather than on 60. `settings_stepper_row` was split into `stepper_frame` plus `stepper_button` instead of being called with a numeric value, because Auto and Off are not numbers; the row keeps the same frame. Hot reload is proven end to end: `terminal_key_repaints_open_terminals` now lists `minimum_contrast`, and both the settings write and the external `paneflow.json` edit reach `TerminalView::set_minimum_contrast` through `propagate_config`. The `docs/user/` mirror was regenerated from the site, so it also carries site changes unrelated to ACC (Launch Pad, `paneflow hooks setup`, the dropped `paneflow sessions` page); both are paneflow-web follow-ups, not ACC defects, since the mirror documents the shipped 0.16.0 |
| 1.4 | 2026-09-21 | Arthur Jean | EP-003 certified. `agent-cli.ansi` added to the corpus during the review: it is the only fixture whose colors fire the harmony pull, and it fires on dark presets only. Arthur closed the US-009 visual pass and the pull ships on by default, with no `harmonize` setting. Known arete recorded below: `#ef4444` straddles the 0.6 chroma threshold across dark presets |
| 1.3 | 2026-09-21 | Arthur Jean | US-009 AC-3 rewritten after the EP-003 implementation measured the pull: EP-001 already resolves 230, 187 and 229 from the theme-derived palette, so those cells reach Lc 82 to 97 by a lightness move alone, lose no chroma, and no pull fires; the criterion becomes a recorded golden of OKLab distances plus a hue invariant instead of a 0.05 distance target, and FR-06 names gamut clipping as a chroma reduction |
| 1.2 | 2026-09-21 | Arthur Jean | Decisions after the EP-001 review, the Ghostty `generate256Color` reading and the Superlogical web research: ACC runs on every theme at one default (Lc 60); only program-chosen colors (`Spec`, `Indexed(16..=255)`) are corrected, the theme's own sixteen slots are the attractor; harmonization pulls to the nearest theme color in OKLab distance, not to the nearest hue; the generated palette stays on every preset and US-003 is rewritten around the cube anchors instead of a dark no-diff promise; US-016 gains a row on the sixteen ANSI slots |

## Problem Statement

1. **Light themes are unreadable with common TUIs.** `lsd`, `btop`, `lazygit` and most agent CLIs are designed for dark terminals and emit 256-color or truecolor foregrounds that vanish on a light background (yellow owner columns, dim grey timestamps, teal branch names). Users of Paneflow Light, Claude Light, Vercel Light and Tailwind Light switch to a dark theme to read output.
2. **The existing correction never reaches the colors that matter.** `src-app/src/terminal/element/mod.rs:1017` skips `Color::Spec` (truecolor) and `Color::Indexed(16..=255)` (the 256-color cube). Only the 16 ANSI slots are corrected, and the theme already makes those legible. The APCA pipeline in `src-app/src/terminal/element/color.rs` is effectively idle.
3. **The correction is off by default and hidden.** `terminal.minimum_contrast` defaults to `0.0` (`crates/paneflow-config/src/schema/terminal.rs:175`) and has no row in Settings > Terminal (no match in `src-app/src/settings/tabs/terminal.rs`). The only threshold constant, `MIN_APCA_CONTRAST = 45`, is the APCA tier for large headlines, not body text.
4. **A perceptual palette is computed and thrown away.** `src-app/src/terminal/ghostty_session.rs:3785` asks libghostty for the CIELAB-interpolated 256-color palette (`generate256Color`, `C:\dev\ghostty\src\terminal\color.zig:232`), with `harmonious = false`. The renderer ignores it: `indexed_color` at `src-app/src/terminal/element/color.rs:282` recomputes the xterm cube by formula. The generated palette only answers OSC 4 queries.
5. **Lightness moves in HSL, not in a perceptual space.** `adjust_lightness_for_apca` (`src-app/src/terminal/element/color.rs:187`) bisects HSL lightness. HSL is not perceptually uniform: darkened yellow turns brown, darkened cyan shifts toward blue, and the corrected text no longer looks like the theme.
6. **The cache is keyed on floats after conversion.** `contrast_cache_get_or_insert` (`src-app/src/terminal/element/color.rs:129`) hashes nine `f32` words per cell into 128 direct-mapped slots. Every cell pays the HSLA conversion before the lookup, and the cache is not cleared on theme change.

**Why now:** Superlogical, built in part by the Ghostty author, shipped "Automatic Contrast Correction" on 2026-09-19 and made light-mode legibility a visible differentiator. Paneflow already has the APCA math, the libghostty palette generator, and the theme generation signal; what is missing is wiring, coverage, a perceptual color space, and a default that turns it on. The v0.16 theme work (five presets with light variants) makes the light themes a first-class surface.

## Overview

Automatic Contrast Correction (ACC) makes every text cell legible against its background on every theme, without touching the applications' colors more than necessary and without a measurable render cost. The pipeline runs on the CPU during row layout, once per text run, behind a per-theme cache.

Four levers, in dependency order. First, the renderer resolves indexed colors 16 to 255 from the libghostty CIELAB palette already computed per theme, with `harmonious = true` on light themes so the cube runs from background to foreground; this alone fixes most 256-color TUIs. Second, the APCA correction extends to truecolor and indexed colors, keeping the exclusions that protect drawings: decorative codepoints (aligned with Ghostty's `isGraphicsElement`) and cells whose foreground equals their background. Third, lightness moves in OKLCH with hue-preserving gamut clipping, and when chroma has to drop, the color is replaced by the nearest theme color in OKLab distance (foreground, dim foreground or an ANSI slot) so corrected pastels land on the theme's text color, as the Superlogical captures show, rather than on a darkened stranger. Fourth, the cache keys on packed sRGB pairs before any conversion, holds 4096 entries, and is cleared when the theme generation changes.

The GUI (sidebar, Changes dock, Files, Settings, command palette, markdown, editor) is deliberately outside the runtime pipeline: its colors come from `ui_colors()` and `SyntaxPalette`, which Paneflow designs. A legibility defect there is a preset bug, so EP-006 adds a role-by-surface APCA invariant table over the ten preset variants and fixes the presets that fail, with no runtime code.

Key decisions: APCA stays the metric (Ghostty and Kitty use WCAG 2 ratios; APCA models polarity and thin monospace text better); the default becomes automatic on every theme at Lc 60, overridable by an explicit `terminal.minimum_contrast`; the correction only touches colors the program chose (truecolor and indexed 16 to 255), the theme's own sixteen ANSI slots, foreground and background are the reference it pulls toward and never a target; no new libghostty C API is needed, the palette generator is already bound; no GPU path, GPUI has no per-glyph shader hook, so the correction stays on the CPU with a cache that makes it a single lookup per run.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Legibility on light themes: share of visible text cells at or above APCA Lc 60 in the fixture corpus (`lsd`, `btop`, `lazygit` captures) | 100% with ACC on, measured by `contrast_corpus` test | 100% across all five light presets |
| Render cost: `layout_220x60` bench delta with ACC on versus off | at most +10% | at most +5% after the per-run hoist |
| Zero drawing artifacts: decorative and `fg == bg` cells untouched in the corpus | 0 corrected decorative cells in the corpus test | 0 user reports of broken block art |
| Discoverability: the setting is reachable from Settings > Terminal and the command palette | shipped | 0 support questions about how to turn it off |
| GUI legibility: every UI role passes its APCA tier on every preset variant | 100% of the role table on all 10 variants in `cargo test` | 0 open issues tagged `light-theme` about GUI text |

## Target Users

### Light-theme developer
- **Role:** Runs Paneflow on Paneflow Light, Claude Light or Vercel Light, often on a laptop outdoors or in a bright office.
- **Behaviors:** Uses `lsd`, `eza`, `btop`, `lazygit`, `git diff --color`, agent CLIs with colored status lines.
- **Pain points:** Yellow and grey text disappears; reads with the eyes squinted or switches to a dark theme for one command.
- **Current workaround:** Dark theme, per-tool theme flags (`lsd --theme`, `btop` theme with background off), or `LS_COLORS` rewrites.
- **Success looks like:** Every tool is readable on the light theme without configuration, and the corrected colors still look like the theme.

### Dark-theme developer
- **Role:** The majority of Paneflow users, on the dark presets.
- **Behaviors:** Same tools, occasionally light-designed output (`bat` themes, some agent banners) that is too dark.
- **Pain points:** Rare, but a correction must never alter the theme's own colors.
- **Current workaround:** None needed.
- **Success looks like:** The theme's sixteen ANSI colors, foreground and background never change; foreign 256-color and truecolor text that falls below the threshold is harmonized toward the theme exactly as on light themes.

### Theme author and maintainer
- **Role:** Paneflow maintainer adding or tuning a preset in `src-app/src/theme/builtin.rs`.
- **Behaviors:** Runs the theme invariant tests in `src-app/src/theme/model.rs`.
- **Pain points:** No way to know whether a preset makes third-party 256-color output legible.
- **Current workaround:** Manual screenshots.
- **Success looks like:** A corpus test fails when a preset lets a common TUI fall below Lc 60 with ACC on.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- Ghostty `minimum-contrast`: WCAG 2 ratio in the cell vertex shader, foreground snapped to black or white when below threshold (`src/renderer/shaders/glsl/common.glsl:97`, Metal twin `src/renderer/shaders/shaders.metal:111`), default off, graphics glyphs excluded via `noMinContrast` (`src/renderer/cell.zig:294`). Users complain the snap destroys subtle colors (ghostty-org/ghostty#1524). Paneflow keeps its finer APCA bisection and reuses only the exclusion list.
- Ghostty `palette-generate` + `palette-harmonious` (`src/terminal/color.zig:232`, `src/config/Config.zig:828`, Jake Stewart, February 2026): 216-color cube and grey ramp interpolated in CIELAB from the theme's eight base colors, background and foreground; `harmonious` keeps the cube oriented background to foreground on light themes and has no effect on dark themes (`invert = is_light_theme and !harmonious`, `src/terminal/color.zig:259`). The cube anchors are `bg, red, green, yellow, blue, magenta, cyan, fg` (the normal ANSI slots, not the bright ones), so on a dark theme the cube's contrast ceiling is the theme foreground and its saturated corners are the theme's own ANSI colors: on Paneflow Dark, `lsd`'s index 40 falls from Lc 64.6 (xterm `#00d700`) to Lc 38.7. Ghostty ships both options off for legacy compatibility, not for contrast, and has no runtime OKLab work: a `repo:ghostty-org/ghostty oklab contrast` search returns 0 issues or PRs (checked 2026-09-21); the only OKLab-adjacent thread is Discussion #10815 proposing OKLCH for this generator. Exposed through `ghostty_color_palette_generate`, already bound in `crates/paneflow-terminal-ghostty/src/color.rs:100`.
- Kitty `text_fg_override_threshold`: `%` mode snaps to black/white, `ratio` mode shifts lightness in HSLuv without changing hue or saturation. Known artifacts on block-drawing glyphs when fg is meant to match bg (kovidgoyal/kitty#6559).
- Windows Terminal `adjustIndistinguishableColors`: OKLab, L-only shift away from the background L with direction flip when out of gamut, 256-entry sRGB-to-linear LUT (`src/types/ColorFix.cpp`). Closest prior art to the OKLCH approach.
- Superlogical "Automatic Contrast Correction" (posts of 2026-09-17 by Alasdair Monk, quoted by Mitchell Hashimoto: "detect programs when they request colors too out of whack from your theme, apply some Oklab magic and hue rotation to match your theme. Harmonious themes, everywhere, always."): runtime evaluation, cached, "WCAG-approved" output, built into the macOS client. Web research on 2026-09-21 found no other primary source: superlogical.com is a single pre-beta announcement page, there is no blog, changelog, docs or repository, and the authors' reply threads are behind X authentication. The two posts and the four screenshots are the whole public record, so every pipeline parameter below is a Paneflow decision. What the ACC on/off screenshots establish:

  | Observation | Evidence in the captures | Design consequence |
  |---|---|---|
  | Correction is per cell against the cell's real background | lazygit: in the selected row (blue background) `rounded-selection` and the path go from pale blue to white; the rest of the panel is untouched | evaluate `(fg, bg)` after INVERSE and explicit backgrounds |
  | Both polarities | same row: text lightened toward white on a dark cell inside a light theme, darkened everywhere else | polarity-aware metric and move; APCA fits |
  | Legible colors are untouched | `lsd` names (index 27) and `d`/`w` permission letters are identical on and off; icons unchanged | threshold-triggered, no global remap |
  | Decorative glyphs are excluded | btop frames, block gauges and dot clouds keep their pale colors while `cpu`, `menu`, `Total:`, `9%` turn dark | `isGraphicsElement`-style exclusion list |
  | 256-color and truecolor are both covered | `lsd` indices 229/230/187 and btop truecolor greys are both corrected | no source-based skip |
  | Collapsed chroma lands on the theme text color | owner (index 230, pale yellow) becomes the theme's dark grey, group (187) grey, size (229) grey-olive; dates (40) stay green, only darker | nearest theme color by OKLab distance, since a hue rotation alone never turns yellow into grey |
  | Backgrounds never move | no background differs between on and off | foreground-only correction |
  | No palette regeneration | the saturated index 27 blue is the same on and off; a CIELAB cube rebuilt from the theme blue would differ | Superlogical does not use Ghostty's `palette-generate`; EP-001 is an additional lever Paneflow keeps on purpose |
- **Market gap:** No terminal with public source combines a polarity-aware metric (APCA), a perceptual lightness move, theme harmonization by OKLab distance, a theme-derived 256-color palette and an automatic default on every theme that never alters the theme's own colors. Superlogical is the closest and is closed and pre-beta.

### Best Practices Applied
- APCA Lc tiers from the Myndex documentation: Lc 90 preferred body text, Lc 75 minimum body text, Lc 60 minimum for fluent text, Lc 45 large text only. Terminal monospace at 12 to 14 px is fluent text, so the default is Lc 60.
- OKLab and OKLCH per Ottosson (bottosson.github.io/posts/oklab) for lightness moves and hue-preserving gamut clipping; the matrices are 30 lines, no dependency.
- Exclusion of drawing glyphs and of `fg == bg` cells, learned from Ghostty and Kitty issue trackers.
- Correction hoisted to the text run and cached on packed sRGB, the same level at which GPUI receives the color.
- Correction scoped to program-chosen colors. The theme's sixteen ANSI slots, foreground and background define the theme; they are the attractor of the harmonization and are never corrected, so a preset whose own colors are weak is a preset bug caught by EP-006, not a runtime job.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- The libghostty snapshot reports palette indices, not resolved RGB, for 256-color cells (`ghostty_session.rs:4449` maps `Palette(index)` to `Color::Indexed`), so the renderer is the right place to resolve them from the theme palette. Validated by reading; US-002 asserts it with a test.
- The CIELAB palette with `harmonious = true` does not break legacy TUIs that assume index 16 is black and 231 is white. Ghostty ships it off by default for that reason. Paneflow only applies `harmonious` on light themes, where the xterm cube is already broken for those tools. Risk accepted, revisited in US-003.
- The generated palette compresses the dark cube toward the theme's seeds (see the Ghostty finding above; measured on `lsd`: Paneflow Dark cells at Lc 45 drop from 196 with the xterm cube to 155, Vercel Dark from 250 to 170). Accepted as the price of theme coherence: ACC now runs on dark themes too and lifts foreign indexed text back to the threshold, while the theme's own colors are exempt. The corpus share golden of US-003 records the numbers so the trade-off is reviewed on every palette or preset change.
- A 4096-entry direct-mapped cache keyed on `u64(fg_rgb, bg_rgb)` covers a 220x60 frame with common TUIs without thrashing. Validated by the corpus hit-rate assertion in US-011.
- ~~Replacing a collapsed-chroma color by the nearest theme color in OKLab distance reproduces the Superlogical captures.~~ Measured during EP-003 and invalidated for the indexed corpus: EP-001 resolves 16 to 255 from the theme-derived palette, so those inputs already carry low chroma and reach the threshold by a lightness move alone, with a chroma ratio of 1.00 on every `lsd` cell of every preset. The pull still fires where the gamut genuinely drains the chroma, which on the current corpus means foreign truecolor (`Spec(255,0,255)` on Paneflow Dark drops from chroma 0.32 to 0.095 and is pulled). The Superlogical collapse is therefore an artifact of correcting the raw xterm cube, a lever Paneflow replaced rather than reproduced. Only a visual pass can settle the final look (US-009 ships behind the same setting and is judged by Arthur).

### Hard Constraints
- The render thread never blocks: no file I/O, no allocation per cell in the layout hot path (AGENTS.md boundary).
- No new libghostty C API and no bump of the libghostty pin: the palette generator and `ghostty_color_*` helpers already exist in `native/libghostty/manifest.toml` pin `0c2a290d`.
- No new crate dependency for color math: OKLab is hand-rolled from Ottosson's published matrices.
- The selection foreground invariant stays at Lc 45 (`src-app/src/theme/model.rs:720`); ACC thresholds are a separate constant.
- Cross-platform: pure Rust, no `cfg` branches.
- No comments in source code; intent goes into names, types and tests.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, mandatory before every commit and push
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lint, including test modules
- `cargo test --workspace --locked` - unit and integration suites
- `scripts/bench-terminal.ps1` (or `.sh`) - for EP-002, EP-003 and EP-004 stories, compare `layout_220x60` against `bench/baseline.json`; a story that raises it by more than 10% does not ship

For UI stories, additional gates:
- Arthur performs the visual pass himself on a debug build with the `lsd`, `btop` and `lazygit` fixtures on Paneflow Light and Paneflow Dark; the story reports what was verified and what was only reviewed by inspection.

## Epics & User Stories

### EP-001: Theme-derived 256-color palette in the renderer

Make the CIELAB palette that libghostty already generates per theme the source of truth for indexed colors 16 to 255, oriented background to foreground on light themes. This is the Ghostty lever and the largest visible win for zero new math.

**Definition of Done:** `Color::Indexed(16..=255)` renders from the generated palette on every preset, the palette is rebuilt exactly once per theme generation, the eight cube anchors equal the theme seeds on every preset (index 16 background, 231 foreground, 196 red, 46 green, 226 yellow, 21 blue, 201 magenta, 51 cyan), and a corpus test proves the grey ramp runs from background to foreground on light presets.

#### US-001: Build a 256-entry render palette per theme
**Description:** As a theme author, I want a `ThemePalette` of 256 `Hsla` values derived once per `TerminalTheme` through `ghostty::generate_palette` so that indexed colors follow the theme instead of the xterm formula.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given a `TerminalTheme`, when `ThemePalette::from_theme` runs, then entries 0 to 15 equal the theme's sixteen ANSI slots and entries 16 to 255 equal `generate_palette(base, mask, ansi_background, foreground, harmonious)` converted to `Hsla`
- [ ] Given a light theme (`is_light_theme` in `src-app/src/theme/model.rs:409`), when the palette is built, then `harmonious` is `true` and entry 16 equals the theme background while entry 231 equals the theme foreground
- [ ] Given a dark theme, when the palette is built, then `harmonious` is `false` and entry 16 is the darkest cube corner while entry 231 is the lightest
- [ ] Given the existing `current_ghostty_palette` in `src-app/src/terminal/ghostty_session.rs:3759`, when the story lands, then it and `ThemePalette` share one construction function so the OSC 4 answers and the pixels can never diverge
- [ ] Given a theme whose foreground equals its background (degenerate fixture), when the palette is built, then the call does not panic and the grey ramp is a constant color

#### US-002: Resolve indexed colors from the render palette
**Description:** As a light-theme developer, I want `indexed_color` to read the `ThemePalette` so that `lsd` and other 256-color tools render with theme-derived colors.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `Color::Indexed(i)` with `i >= 16`, when the row layout converts it, then the result is `ThemePalette[i]` and the xterm cube formula at `src-app/src/terminal/element/color.rs:282` is deleted
- [ ] Given `Color::Indexed(i)` with `i < 16`, when converted, then the result is unchanged from today (named ANSI slot)
- [ ] Given a libghostty cell reporting `Palette(200)`, when it flows through `color_from_ghostty` and the layout, then the pixel color equals `ThemePalette[200]` (integration test in `src-app/tests/` or the element test module)
- [ ] Given a theme change, when the next frame lays out, then the palette used carries the new theme generation and no stale entry is served
- [ ] Given a `Color::Indexed` value on a cell flagged `INVERSE`, when converted, then the swap of foreground and background still happens after palette resolution, not before

#### US-003: Corpus guard for the cube anchors and the light-theme grey ramp
**Description:** As a maintainer, I want a fixture test over `lsd`, `btop` and `lazygit` snapshots plus an anchor invariant on every preset so that a preset or a palette change that regresses indexed legibility, or detaches the cube from the theme seeds, fails CI.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] Given cell snapshots for `lsd -la`, `btop` and `lazygit` stored under `src-app/tests/fixtures/contrast/` with their provenance in a README (authored from the tools' documented palettes is acceptable until a live capture replaces them), when rendered on each light preset with the render palette and `minimum_contrast = 0`, then every non-decorative text cell sourced from `Color::Indexed(232..=255)` has APCA |Lc| at least 45 against its resolved background, and every cell sourced from `Color::Indexed(16..=231)` below Lc 45 is listed in a committed golden that holds at most one distinct index; truecolor and named cells are EP-002's scope and are only recorded, never asserted, here
- [ ] Given every preset variant, light and dark, when the palette is built, then the eight cube anchors equal the theme seeds within one 8-bit step per channel (16 = `ansi_background`, 231 = `foreground`, 196 = `red`, 46 = `green`, 226 = `yellow`, 21 = `blue`, 201 = `magenta`, 51 = `cyan`) and the grey ramp 232 to 255 is monotone in lightness from background to foreground; on dark presets index 16 stays darker than 231
- [ ] Given the three fixtures on every preset variant, when laid out with `minimum_contrast = 0`, then a share golden records per fixture and variant the text cell count, the indexed cell count, and the counts at Lc 45 and Lc 60 both with the render palette and with the xterm formula it replaced, so a palette or preset change shows as a reviewed diff rather than a silent shift
- [ ] Given a fixture that cannot be loaded, when the test runs, then it fails with the fixture path in the message rather than passing vacuously

---

### EP-002: Correction coverage and skip rules

Extend the APCA correction to every color source that TUIs actually use, keep drawings intact, and make the default automatic.

**Definition of Done:** Truecolor and indexed 16 to 255 foregrounds are corrected, named and indexed 0 to 15 foregrounds, decorative cells and `fg == bg` cells are never touched, and a fresh install gets Lc 60 on every theme without a config change.

#### US-004: Correct truecolor and indexed foregrounds
**Description:** As a light-theme developer, I want truecolor and 256-color text corrected so that `btop` and `lazygit` become readable.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] Given a cell with `Color::Spec` or `Color::Indexed(16..=255)` foreground and `minimum_contrast > 0`, when laid out, then `ensure_minimum_contrast` runs on it (the `skip_contrast` predicate at `src-app/src/terminal/element/mod.rs:1017` is inverted, not removed)
- [ ] Given a cell with `Color::Named` or `Color::Indexed(0..=15)` foreground, when laid out with any threshold, then it is never corrected (counter assertion): the theme's own colors are the attractor of the correction, and a weak preset slot is EP-006's problem
- [ ] Given a cell whose resolved foreground equals its resolved background (`fg == bg` after `INVERSE` handling, alpha ignored), when laid out, then the foreground is left untouched
- [ ] Given a cell with a decorative codepoint, when laid out, then the foreground is left untouched
- [ ] Given a cell with the `DIM` flag, when corrected, then the alpha halving still applies after the correction so dim text stays dim
- [ ] Given `minimum_contrast == 0`, when laid out, then no correction function is called (counter assertion in the test)

#### US-005: Align decorative codepoints with Ghostty
**Description:** As a user, I want block art, Powerline and box drawing left alone so that ACC never breaks a TUI frame.

**Priority:** P0
**Size:** XS (1 pt)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given the codepoint ranges in Ghostty `isGraphicsElement` (`C:\dev\ghostty\src\renderer\cell.zig`), when compared with `is_decorative_character` (`src-app/src/terminal/element/mod.rs:52`), then every range Ghostty excludes is excluded by Paneflow, documented in a table-driven test listing each range with its Unicode block name
- [ ] Given U+2591 to U+2593 (shade blocks) and U+2800 to U+28FF (Braille, used by `btop` graphs), when laid out, then they are excluded
- [ ] Given a plain letter, digit or CJK glyph, when tested, then it is not decorative

#### US-006: Automatic default threshold
**Description:** As a light-theme developer, I want ACC on at Lc 60 without touching my config, and as a dark-theme developer I want the same harmonization of foreign colors without my theme's own colors changing.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given `terminal.minimum_contrast` unset, when resolved on any theme, light or dark, then the threshold is 60: one default, every polarity, as Superlogical's "everywhere, always"
- [ ] Given `terminal.minimum_contrast` set to an explicit finite value, when resolved, then that value clamped to `[0, 90]` wins on every theme
- [ ] Given a theme change at runtime, when the next frame lays out, then corrected colors follow the new theme's backgrounds and pull targets without a restart, and the `lsd` fixture on Paneflow Dark shows index 40 lifted to at least Lc 60 while every named ANSI cell is byte-identical to the uncorrected layout
- [ ] Given `terminal.minimum_contrast` set to `NaN`, `-5` or `"abc"` in `paneflow.json`, when loaded, then the automatic default applies and a warning is logged once
- [ ] Given the selection foreground invariant test at `src-app/src/theme/model.rs:720`, when the story lands, then it still asserts Lc 45 through a constant distinct from the ACC default

---

### EP-003: Perceptual correction

Move lightness in OKLCH with hue-preserving gamut clipping, and harmonize corrected hues with the theme.

**Definition of Done:** Corrected colors keep their OKLCH hue within 2 degrees when chroma allows, drop chroma before anything else, and when chroma collapses the color lands on the nearest theme color in OKLab distance, re-bisected in lightness so the threshold still holds.

#### US-007: OKLab and OKLCH conversions with gamut clipping
**Description:** As an engineer, I want a dependency-free `oklab` module so that later stories can move lightness perceptually.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given sRGB white, black and the six primaries, when converted to OKLab and back, then each channel round-trips within 1/255
- [ ] Given Ottosson's published reference values for OKLab (white L = 1.0, and the sample colors in the OKLab article), when converted, then results match within 0.001
- [ ] Given an OKLCH color outside the sRGB gamut, when clipped with hue preserved, then the returned sRGB color is in gamut, its hue is within 1 degree of the input, and chroma is the maximum representable for that L and hue
- [ ] Given a color with zero chroma, when its hue is requested, then a stable hue of 0 is returned and no NaN escapes
- [ ] Given the module, when clippy runs, then no `unwrap` or `expect` is present outside tests

#### US-008: Bisect lightness in OKLCH
**Description:** As a light-theme developer, I want corrected text to keep its hue and look like the same color, only darker or lighter.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-007

**Acceptance Criteria:**
- [ ] Given a foreground below the threshold, when corrected, then the search moves OKLCH L toward the polarity that increases |Lc| (darker on light backgrounds, lighter on dark), with chroma clipped to gamut at each step, and stops within 20 iterations at the L closest to the original that meets the threshold
- [ ] Given the corrected color, when its hue is compared with the input, then the difference is at most 2 degrees whenever the threshold was met without a chroma reduction
- [ ] Given a foreground that cannot meet the threshold at any L for its hue and chroma, when corrected, then chroma is reduced in steps of 20% and the search repeats, and only if chroma zero still fails does the fallback pick black or white by larger |Lc|
- [ ] Given `adjust_lightness_for_apca` and the HSL desaturation ladder at `src-app/src/terminal/element/color.rs:157`, when the story lands, then they are deleted
- [ ] Given the theme invariant tests in `src-app/src/theme/model.rs`, when run, then they pass unchanged

#### US-009: Harmonize corrected hues with the theme palette
**Description:** As a light-theme developer, I want a corrected pastel to land on the theme's own text color rather than on a foreign brown, as in the Superlogical captures where `lsd`'s owner column becomes the theme grey, so that corrected text looks native.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given a correction whose realized chroma fell below 60% of the source chroma, whether through the US-008 ladder or through hue-preserving gamut clipping, or whose source carried chroma at or above 0.02 and ended below it, when the final color is computed, then the result is the theme color nearest in OKLab distance (Euclidean over L, a, b) among `foreground`, `dim_foreground` and the sixteen ANSI slots, then re-bisected in L so the threshold still holds against the cell background
- [ ] Given a correction that met the threshold with a chroma reduction of 40% or less, when computed, then no pull happens and the hue stays within 2 degrees; given a source that was already achromatic (chroma below 0.02 before correction), when corrected, then it is never tinted by the pull
- [ ] Given the `lsd` fixture on every preset variant, when rendered with the default threshold, then a committed golden records per variant and per source index the OKLab distance from the corrected color to the nearest of `foreground` and `dim_foreground`, the source and realized chroma, and the hue drift, so a palette, preset or pull-rule change shows as a reviewed diff rather than a silent shift; and the owner (index 230), group (187) and size (229) columns keep their hue within 2 degrees while the date column (index 40) keeps its green hue within 5 degrees. These indices are not pulled and must not be: EP-001 resolves them from the theme-derived palette, they reach Lc 82 to 97 by a lightness move alone and lose no chroma, so forcing them onto `foreground` would collapse three distinct columns into one grey and undo the per-index differentiation EP-001 bought
- [ ] Given the story, when Arthur performs the visual pass, then the report states whether the result reads closer to the Superlogical screenshots than US-008 alone; if not, the pull stays in the code behind a `harmonize` boolean defaulting to `false` and the PRD is amended

---

### EP-004: Performance

Keep ACC at a single cache lookup per text run and prove it with the bench.

**Definition of Done:** `layout_220x60` with ACC on is within 10% of the baseline with ACC off, the cache hit rate on the fixture corpus is above 99%, and the cache is cleared on theme change.

#### US-010: Packed sRGB contrast cache
**Description:** As an engineer, I want the contrast cache keyed on packed sRGB pairs before any conversion so that a hit costs one hash and one compare.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given a foreground and background as 8-bit sRGB, when looked up, then the key is `u64` = `(fg_rgb << 32) | bg_rgb` plus the threshold as a separate `u16` field in the entry, and no `Hsla` conversion happens before the lookup
- [ ] Given a hit, when returned, then the stored `Hsla` is returned without recomputation (counter assertion)
- [ ] Given a theme generation change (`crate::theme::theme_generation()`), when the cache is next touched, then it is cleared before any lookup
- [ ] Given 4096 slots, when the fixture corpus (US-003) is laid out twice, then the second pass hit rate is at least 99%
- [ ] Given the `the_contrast_cache_never_changes_the_answer` test at `src-app/src/terminal/element/color.rs:311`, when adapted, then it still asserts cache and direct computation agree over 3x the slot count
- [ ] Given the cache lives in a `thread_local`, when the layout runs on a non-render thread in tests, then it works without a global lock

#### US-011: Hoist correction to the text run
**Description:** As an engineer, I want the correction applied once per `BatchedTextRun` instead of once per cell so that a 220-column line of one color costs one lookup.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given consecutive cells with the same raw foreground, raw background and flags, when batched, then `ensure_minimum_contrast` is called once for the run (counter assertion on a 220-column single-color line: exactly 1 call)
- [ ] Given a run that is broken by a selection or search highlight, when batched, then the highlighted segment keeps its own foreground rule (selection foreground, search foreground) and the correction is not applied over it
- [ ] Given a run containing a decorative codepoint in the middle, when batched, then the decorative cell forms its own uncorrected run and the neighbors are corrected
- [ ] Given a `DIM` cell inside a non-dim run, when batched, then the run splits so alpha handling is per cell

#### US-012: Bench guard for ACC
**Description:** As a maintainer, I want `scripts/bench-terminal` to measure ACC on and off so that a regression is caught before release.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given `src-app/src/terminal/perf_bench.rs`, when the suite runs, then a new metric `layout_220x60_acc60` lays out the truecolor corpus with `minimum_contrast = 60` and reports alongside `layout_220x60`
- [ ] Given `bench/baseline.json`, when regenerated on the same machine, then the new metric is present with `direction = lower_is_better`
- [ ] Given a run where `layout_220x60_acc60` exceeds `layout_220x60` by more than 10%, when the comparison prints, then the delta is flagged in the output so the story report cannot miss it
- [ ] Given a corpus with zero correctable cells, when the metric runs, then it completes and reports rather than dividing by zero

---

### EP-005: Settings, palette and documentation

Expose the threshold where users look for it and keep the public docs in sync.

**Definition of Done:** The threshold is editable from Settings > Terminal and the command palette, hot-reloads, and the user documentation describes the automatic default.

#### US-013: Minimum contrast row in Settings > Terminal
**Description:** As a user, I want a "Minimum contrast" control next to font settings so that I can set Auto, Off or an explicit Lc value.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given Settings > Terminal, when rendered, then a row titled "Minimum contrast" uses `settings_stepper_row` (`src-app/src/settings/tabs/terminal.rs:766`) with values Auto, Off, 45, 60, 75, 90 and the description "Raises text contrast against its background. Auto uses 60 on every theme and never changes the theme's own colors."
- [ ] Given Auto, when persisted, then `terminal.minimum_contrast` is removed from `paneflow.json` (`Value::Null` through `persist_setting(true, "minimum_contrast", ...)`)
- [ ] Given Off, when persisted, then the value `0` is written and the next frame shows uncorrected colors
- [ ] Given a value change, when persisted, then every open terminal repaints within one frame via `propagate_config` (the `nested` key list at `src-app/src/app/settings.rs` includes `minimum_contrast`)
- [ ] Given a config file already holding `72.5`, when the row renders, then it shows the nearest stepper value and does not rewrite the file until the user changes it
- [ ] Given the search index in `src-app/src/settings/search.rs`, when "contrast" is typed, then the row is found

#### US-014: Command palette scope for minimum contrast
**Description:** As a keyboard user, I want a "Minimum contrast" scope in the command palette so that I can preview thresholds live like themes.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given the palette, when "contrast" is typed, then a "Minimum contrast" scope command appears with the current value as its trailing label
- [ ] Given the scope, when a value is highlighted with the arrow keys, then it applies in place (the `Apply::Setting` path) and the terminals repaint without closing the palette
- [ ] Given the scope with Auto selected, when the list renders, then the current mark sits on Auto, not on 60
- [ ] Given the `every_scope_is_reachable_from_a_command` test, when the story lands, then it lists the new scope

#### US-015: User documentation for ACC
**Description:** As a user, I want the docs to explain what changes on light themes and how to turn it off.

**Priority:** P2
**Size:** XS (1 pt)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given the paneflow-web docs source (the origin of the `docs/user/` mirror), when updated, then the Terminal settings page documents `terminal.minimum_contrast` with the Auto, Off and Lc semantics, the Lc 60 default on every theme, and the rule that the theme's own sixteen colors are never corrected
- [ ] Given `docs/user/settings.md`, when resynced from the site, then the row for `terminal.minimum_contrast` matches the site text (no hand edit in the mirror)
- [ ] Given a reader who wants the old behavior, when they follow the docs, then setting `"minimum_contrast": 0` is described as the way to disable ACC on every theme

---

### EP-006: UI role contrast invariants

Lock GUI legibility on every preset variant with a role-by-surface APCA table, then fix the presets that fail. Test-only for the invariant; preset color edits only where the table fails.

**Definition of Done:** A table-driven test asserts an APCA tier for every `UiColors` text role on every surface it is drawn on, and for each of the sixteen terminal ANSI slots on `ansi_background`, across the ten preset variants (five presets, light and dark), and all ten pass.

#### US-016: Role-by-surface APCA invariant table
**Description:** As a theme author, I want a test that enumerates every UI text role against every surface it is drawn on with a required APCA tier so that a preset that makes sidebar, dock or settings text illegible fails `cargo test`.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `UiColors` (`src-app/src/theme/model.rs:448`), when the table is written in `src-app/src/theme/model.rs` tests, then it lists at least these pairs with tiers: `text` on `base`, `surface`, `overlay` at Lc 60; `muted` on `base`, `surface`, `overlay` at Lc 45; `accent` on `base` and `surface` at Lc 45; `vc_added`, `vc_modified`, `vc_deleted`, `vc_conflict` on `surface` at Lc 45; `agent_error`, `agent_claude`, `agent_codex` on `surface` at Lc 45; `group_1` to `group_8` on `surface` at Lc 30; `vc_word_added` text on `vc_added_background` and `vc_word_deleted` on `vc_deleted_background` at Lc 45; `text` on `tool_card_header_bg` at Lc 60; `on_selection_color()` on `selection_color()` at Lc 60; each of the sixteen terminal ANSI slots (`black` to `bright_white`) on `ansi_background` at Lc 45, because ACC never corrects the theme's own slots (US-004) and Paneflow Dark's `red` (Lc 34), `blue` (38) and `magenta` (40) currently fail it
- [ ] Given each of the ten variants (`PRESETS` light and dark names through `ui_colors_with(&theme)`), when the test runs, then every table row is evaluated with `apca_contrast` and a failure names the preset, the role, the surface, the measured Lc and the required tier in one message
- [ ] Given a semi-transparent background role (alpha below 1), when the pair is evaluated, then the background is first composited over the surface it sits on (`vc_added_background` over `surface`) rather than compared raw
- [ ] Given a table row added later with an unknown role name, when compiled, then it is a compile error, not a runtime skip (the table references struct fields, not strings)
- [ ] Given a preset that fails, when the test runs before US-017, then the failing rows are listed in the story report so US-017 has an exact worklist

#### US-017: Fix the presets that fail the table
**Description:** As a light-theme developer, I want every preset variant to pass the role table so that sidebar, Changes dock, Files, Settings and palette text is legible on light presets.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] Given the failing rows from US-016, when the preset colors in `src-app/src/theme/builtin.rs` are adjusted, then all ten variants pass the table with no row removed and no tier lowered
- [ ] Given a color change, when the existing invariant tests in `src-app/src/theme/model.rs` run (`bundled_themes_satisfy_selection_contrast_invariant`, `bundled_themes_keep_core_syntax_roles_distinguishable`, `vc_diff_slots_distinct_with_subtle_zed_alpha_backgrounds`, `light_ui_keeps_the_work_area_pure_white`, `dark_ui_uses_cockpit_surface_palette`), then they still pass
- [ ] Given a row that cannot pass without changing the surface color itself, when fixed, then the surface change is limited to that preset and the story report lists the before and after hex values per changed slot
- [ ] Given no preset fails after US-016, when this story starts, then it is closed as no-op with the test output attached rather than changing colors for their own sake
- [ ] Given the changed presets, when Arthur performs the visual pass on the sidebar, Changes dock and Settings on each changed variant, then the report states what was verified

---

## Functional Requirements

- FR-01: The system must resolve `Color::Indexed(16..=255)` from a palette generated per theme by `ghostty::generate_palette`, with `harmonious = true` on light themes and `false` on dark themes.
- FR-02: The system must apply APCA contrast correction to `Spec` and `Indexed(16..=255)` foregrounds when the resolved threshold is greater than 0, and must never correct `Named` or `Indexed(0..=15)` foregrounds: they are the theme's own colors and the attractor of the harmonization.
- FR-03: The system must not correct cells whose codepoint is decorative or whose resolved foreground equals its resolved background.
- FR-04: When `terminal.minimum_contrast` is unset, the system must resolve the threshold to 60 on every theme; an explicit finite value clamped to `[0, 90]` overrides it and `0` disables the correction.
- FR-05: The system must move lightness in OKLCH with hue preserved within 2 degrees whenever the threshold can be met without reducing chroma.
- FR-06: When the realized chroma falls below 60% of the source chroma, whether through the reduction ladder or through hue-preserving gamut clipping, or when a source that carried chroma collapses below 0.02, the system must replace the color by the nearest theme color in OKLab distance among foreground, dim foreground and the sixteen ANSI slots, then re-bisect lightness (behind the `harmonize` flag if the visual pass rejects it). An already achromatic source is never pulled, so neutral truecolor text keeps its neutrality.
- FR-07: The system must cache corrections keyed on packed 8-bit sRGB pairs and the threshold, and must clear the cache when the theme generation changes.
- FR-08: The system must apply the correction once per batched text run, not once per cell.
- FR-09: The system must expose the threshold in Settings > Terminal and as a command palette scope, with hot reload.
- FR-10: The system must NOT change the theme's own sixteen ANSI colors, foreground or background on any theme; foreign 256-color and truecolor text on dark themes is harmonized under the same rules as on light themes.
- FR-11: The system must NOT add a libghostty C API, bump the libghostty pin, or add a crate dependency for color math.
- FR-12: Every `UiColors` text role must meet its APCA tier on every surface it is drawn on, and every terminal ANSI slot must reach Lc 45 on `ansi_background`, for all ten preset variants, enforced by a test and never by runtime correction.

## Non-Functional Requirements

- **Performance:** `layout_220x60_acc60` at most 10% above `layout_220x60` in `scripts/bench-terminal`; cache hit rate at least 99% on the fixture corpus second pass; zero heap allocations per cell in the correction path (allocation counter in the bench harness reports 0 `allocs_per_iter` delta).
- **Correctness:** Every corrected cell reaches APCA |Lc| at or above the resolved threshold, or the black/white fallback, verified over the fixture corpus; hue drift at most 2 degrees when chroma is untouched; OKLab round trip within 1/255 per channel.
- **Startup:** Palette generation per theme at most 1 ms on the bench machine (256 `generate_palette` entries plus conversion), measured once in `perf_bench` under `active_theme_read`.
- **Reliability:** No panic on degenerate themes (foreground equals background), NaN or negative config values, or empty fixtures; a failed fixture load fails the test with its path.
- **Compatibility:** Identical output on Linux, macOS and Windows (pure Rust, no `cfg`); no change to `paneflow.json` schema beyond the existing `terminal.minimum_contrast` key.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Foreground equals background | TUI draws shapes with space glyphs or same-color text | Cell left untouched | — |
| 2 | Decorative glyph in a colored run | Powerline arrows, box drawing, Braille graphs | Glyph forms its own uncorrected run | — |
| 3 | Color unreachable at any lightness | Saturated hue on a mid-grey background | Chroma ladder, then black/white fallback by larger \|Lc\| | — |
| 4 | Degenerate theme | Preset with foreground equal to background | Palette builds with a constant grey ramp, no panic | — |
| 5 | Invalid config value | `minimum_contrast` is `NaN`, negative, string | Automatic default applies, one warning logged | Log: "terminal.minimum_contrast is not a number; using Auto" |
| 6 | Theme change mid-frame | Palette preview arrows through themes | Cache cleared by generation, next frame uses new palette and threshold | — |
| 7 | Dim text | `DIM` flag on a corrected cell | Correction on opaque color, then alpha halved | — |
| 8 | Selection or search over corrected text | Selection spans a corrected run | Selection foreground wins, correction not applied over it | — |
| 9 | OSC 4 palette override by an app | App redefines index 200 | Not honored by the renderer (existing limitation, see Non-Goals) | — |
| 10 | Cache collision burst | Corpus with more than 4096 distinct pairs | Direct-mapped eviction, answers still exact (never stale across generation) | — |
| 11 | Explicit threshold above 90 | Config sets 120 | Clamped to 90 | — |
| 12 | Dark theme, unset config | Fresh install on Paneflow Dark | Named ANSI text byte-identical to v0.16; indexed and truecolor text below Lc 60 lifted toward the theme, counts recorded in the corpus share golden | - |
| 13 | Theme's own slot below the threshold | Paneflow Dark `red` at Lc 34 as `\e[31m` text | Left untouched at runtime; the US-016 ANSI row flags the preset for US-017 | - |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | `harmonious` palette breaks a legacy TUI that assumes index 16 is black on light themes | Med | Med | Apply only on light themes where the xterm cube is already illegible; corpus test; the row in Settings lets users set Off |
| 2 | OKLab-distance harmonization looks worse than plain OKLCH darkening | Low | Low | Measured in EP-003: the pull never fires on the indexed corpus and only on foreign truecolor whose gamut drains the chroma, so the blast radius is small. US-009 still ships behind `harmonize`, judged by Arthur's visual pass against the Superlogical captures, default flipped by evidence |
| 3 | Per-run hoist breaks selection or search highlight boundaries | Low | High | Run split rules in US-011 with counter and boundary tests; golden comparison against the pre-story layout on the fixture corpus |
| 4 | Bench regression from cache misses on truecolor-heavy output (`btop` gradients) | Med | Med | 4096 slots, packed key, hit-rate assertion; escalate to 16384 slots if the corpus shows under 99% |
| 5 | The generated palette and OSC 4 answers diverge after a refactor | Low | Med | US-001 forces one construction function shared by `current_ghostty_palette` and the renderer |
| 6 | Dark-theme users see their theme's own colors altered | Low | High | FR-10: named and indexed 0 to 15 are exempt from correction, golden on the named cells of the corpus with unset config |
| 7 | The generated dark cube inherits weak seeds (Paneflow Dark `red` Lc 34, `blue` 38, `magenta` 40) and dims 256-color TUIs | High | Med | ACC on dark lifts foreign indexed text to Lc 60; the US-016 ANSI row forces the seeds to Lc 45 in US-017; the share golden makes every drift a reviewed diff |

## Non-Goals

- No GPU or shader path: GPUI offers no per-glyph shader hook, and the cached CPU path meets the budget.
- No honoring of OSC 4 dynamic palette overrides in the renderer: cells report indices and the theme palette resolves them, as today. Revisit when libghostty exposes resolved RGB per cell in the snapshot.
- No correction of background colors, cursor colors, images or emoji (Ghostty excludes emoji too).
- No runtime contrast correction on GUI surfaces (sidebar, Changes dock, Files, Settings, palette, markdown, editor): their colors are Paneflow's own; EP-006 locks them with tests and fixes presets instead.
- No per-application or per-pane threshold; one setting per config.
- No change to the selection foreground invariant (Lc 45); preset colors change only through US-017 for a failing US-016 row.
- No WCAG 2 ratio mode for parity with Ghostty or Kitty configs, and no claim of "WCAG-approved" output as Superlogical words it: APCA is the metric.
- No opt-out by polarity: there is no dark-only or light-only default, `0` is the only off switch.

## Files NOT to Modify

- `native/libghostty/**` — pinned engine archives, bindings and manifest; no C API change is needed.
- `crates/paneflow-terminal-ghostty/src/color.rs` — the `generate_palette` binding already exists; consume it, do not extend it.
- `src-app/src/theme/builtin.rs` preset colors — ACC corrects at render time; presets stay the design source of truth. Only US-017 may edit them, and only the slots named by a failing US-016 row.
- `src-app/src/terminal/ghostty_session.rs:4445` `color_from_ghostty` — the index mapping is correct; resolution belongs to the renderer.
- `bench/baseline.json` except through `scripts/bench-terminal` regeneration in US-012.

## Technical Considerations

- **Architecture:** Where does `ThemePalette` live? Recommended: `src-app/src/theme/palette.rs`, built inside `install_theme` in `src-app/src/theme/watcher.rs` so it shares the theme generation and the render thread never calls libghostty. Engineering to confirm the `TerminalTheme` `Copy` semantics tolerate a 256-entry array (6 KiB) or whether the palette should sit in a separate `Arc` next to the cache.
- **Data Model:** Cache entry layout: `key: u64`, `threshold: u16`, `value: Hsla` (16 bytes) = 32 bytes per slot, 128 KiB for 4096 slots per thread. Alternative: store packed sRGB output (4 bytes) and convert on hit. Trade-off: 12 bytes per slot versus one `Hsla::from(Rgba)` per hit.
- **API Design:** Should `ensure_minimum_contrast` take `Rgb` inputs and return `Hsla`, or stay `Hsla` to `Hsla` with an internal packing step? Recommended: `Rgb` in, since `Color::Spec` already carries `Rgb` and named or indexed colors can be stored packed in `ThemePalette`.
- **Dependencies:** None. OKLab matrices from Ottosson (M1, M2 and inverses) hand-rolled with reference-value tests. Alternative: the `palette` crate, rejected for binary size and the single-use surface.
- **Migration:** `terminal.minimum_contrast` keeps its key and clamp. Users with an explicit `0` keep ACC off; users with unset config get the automatic default. Rollback: set `0`. No file migration.
- **Threshold semantics on the boundary:** Is a theme "light" by `background.l > 0.5` (`model.rs:409`) or by preset membership (`theme_name_is_light`)? Recommended: `is_light_theme` on the resolved `TerminalTheme`, so custom themes behave by their actual colors.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Share of non-decorative text cells at APCA \|Lc\| ≥ 60 on Paneflow Light, `lsd` fixture | To be measured in US-003 before US-004 (expected well under 100%: yellow owner and size columns fail) | 100% | Month-1 | `contrast_corpus` test output |
| `layout_220x60_acc60` versus `layout_220x60` | N/A (new metric) | ≤ +10%, then ≤ +5% after US-011 | Month-1 / Month-6 | `scripts/bench-terminal` comparison |
| Cache hit rate, fixture corpus second pass | N/A (new) | ≥ 99% | Month-1 | US-010 assertion |
| Pixel diff on named ANSI text, every preset, unset config | 0 | 0 | Every release | FR-10 golden test |
| Indexed `lsd` cells at APCA \|Lc\| ≥ 60 on Paneflow Dark after correction | 84 of 272 (share golden, palette only) | 272 | Month-1 | `contrast_corpus_share` golden after US-006 |
| Support questions about light-theme legibility | Not tracked | 0 open issues tagged `light-theme` at Month-6 | Month-6 | GitHub issues |
| UI role table rows passing across the ten preset variants | To be measured by US-016 before US-017 | 100% | Month-1 | `cargo test -p paneflow-app -- theme::model` |

## Open Questions

- ~~Arthur, after the US-009 visual pass: does hue harmonization ship on by default, or stay behind `harmonize = false`?~~ Closed by Arthur on 2026-09-21: harmonization ships on by default and no `harmonize` setting is introduced, so FR-06 needs no flag in EP-005. The `agent-cli.ansi` fixture added during the EP-003 review is what made the case observable: the pull fires on `#6366f1` and `#8b5cf6` across all five dark presets (chroma ratios 0.53 to 0.59) and on no light preset at all, recorded per variant in `contrast_corpus_pull.txt`. One arete stays open for EP-005 to watch rather than decide now: `#ef4444` lands at ratio 0.56 on Paneflow Dark and 0.60 to 0.64 on the four other dark presets, so the same error red is pulled on one preset and not the others. If that discontinuity ever reads wrong, the fix is a progressive transition around the threshold, not a flag.
- Arthur, after the US-006 visual pass on Paneflow Dark: does one Lc 60 default read heavy-handed on dark presets (`lsd`'s greens and oranges all lifted)? If so the fallback is Lc 45 on dark, decided by that evidence, not upfront; the single-default rule stays the target.
- Engineering, during US-001: does `TerminalTheme` remain `Copy` with the palette embedded, or does the palette move to an `Arc` next to the contrast cache? Affects the `install_theme` signature in `watcher.rs`.
[/PRD]
