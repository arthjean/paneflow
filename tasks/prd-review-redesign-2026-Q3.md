[PRD]
# PRD: Paneflow Review Redesign (Premium Multi-Agent Decision Cockpit)

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-06-17 | Arthur Jean | Initial draft. Informed by the 7-dimension deep audit of the Review/diff interface (architecture, UX, visual design, internal design-system, agent attribution, write-ops, external design reference). Reintegrates the non-write ideas from the shelved Mergeflow concept (`/home/arthur/dev/mergeflow/tasks/prd-mergeflow.md`). |

## Problem Statement

The Review interface (the Git Diff mode: `src-app/src/diff/` + `src-app/src/app/diff_sidebar/` + `app/agents_diff.rs`) is functional but reads as late-MVP. The audit isolates three root causes, not a styling problem:

1. **Architectural lock-in: two duplicated diff engines.** `app/agents_diff.rs` (2181 LOC, GPUI `list`-based renderer + hand-rolled `parse_unified_diff`) is a parallel re-implementation of `diff/view.rs` (3059 LOC, custom `DiffElement` direct-paint + `imara_diff` pipeline). They share zero code and have already diverged visually (line rows 22px vs 18px, file headers 44px vs 32px). Any visual change must be made twice. This blocks every other improvement.

2. **Flat visual hierarchy.** The diff body color system is already good (`DiffColors`, Codex-sampled green/red `0x40c977`/`0xfa423e`, token-driven word-diff). But the three chrome tiers (column header, file card, sticky header) all resolve to the same `ui.surface = 0x212121` (`view.rs:2255,2039`), there is no type scale (9 font sizes in one toolbar strip), and the file header is a single monospace string `{sigil} {path} +N -N` (`rows.rs:329`) where path and diffstat fuse. The interface looks assembled, not designed.

3. **The best capabilities are hidden or absent.** The signature interaction (left-click a changed line to ask an AI about it, `view.rs:1128`) has zero affordance. Agent attribution and cost (the Mergeflow differentiator) do not exist despite the session parsers already carrying the data. The "act" pathway (`ask_review_about_hunk`, `view.rs:1449`) is buried in a right-click menu. These make the surface feel like a viewer, not a tool.

**Why now:** Paneflow's Agents view was already redesigned to a Codex-app premium bar and ships a complete internal primitive library. The Review interface is the last major surface still at MVP. Unifying it with the Agents design language is the highest-leverage quality move available, and the multi-worktree + session-parsing foundations the Mergeflow concept depended on are already in the codebase.

## Overview

Redesign the Review interface to a premium, Codex-App-grade bar (sober, dense, clear, strong visual hierarchy) and rename the mode "Review" (its verb is correct: see + ask + launch reviews). The work is staged behind a foundational unification epic so no visual change is ever done twice.

Hard product framing decisions (locked):

- **100% free, OSS, no monetization in this interface.** No paid plan, no license gate, no feature paywall. Everything here ships in the free GPL core.
- **No native git write operations.** Paneflow Review never runs `git add`/`apply`/`commit`/`checkout`/`merge`/`worktree remove`. The "act" layer is agent-mediated: the user directs a real CLI agent (human-in-loop, pre-filled prompt, no auto-submit) or drops to a shell in the worktree. This keeps Paneflow an agent cockpit, not a generalist git client (Risk #3 of the Mergeflow PRD, rated High probability).
- **Reintegrate from Mergeflow only the non-write ideas:** agent attribution + estimated cost, multi-worktree polish, the safe two-step destructive-action pattern (applied to agent-mediated discard, not native discard).

What changes for the user: the Review mode gains a clear three-tier visual hierarchy, full keyboard operability, a visible and named click-to-ask affordance, per-worktree agent attribution with estimated cost inline, and a first-class "direct the agent at this hunk" action. The diff engine itself (already premium: virtualized, syntax-highlighted, file-anchored cross-column scroll sync) is preserved and unified, not rebuilt.

## Goals

This is internal product-quality work, not a revenue product. Metrics are dogfooding and engineering-quality oriented.

| Goal | Target |
|------|--------|
| Duplicated diff-render/parse LOC eliminated | ≥ 800 LOC removed (single render + parse path) |
| Visual consistency with Agents view | Review and Agents share one primitive library; no duplicate tooltip/filter/empty-state structs remain |
| Keyboard operability of the review loop | review → next/prev hunk → ask → launch is 100% keyboard-completable |
| Attribution coverage | Claude Code + Codex worktrees show agent + model + estimated cost inline; OpenCode shows agent + recency (graceful degradation) |
| Performance (no regression) | 200-file diff scroll frame time < 16ms P95 on the reference machine (RTX 4070 Ti SUPER / 7800X3D); cold open to first paint unchanged |
| Discoverability | click-to-ask and direct-agent affordances are visible without hover-hunting; empty/loading/failed states are designed, not raw strings |

## Target Users

### Agent orchestrator (primary, = Arthur)
Runs 2-6 coding agents in parallel across git worktrees daily. Ends a session with several divergent diffs to survey, question, and decide on. Wants one window, all worktrees side by side, with cost visibility and a keyboard-driven review loop, then directs agents to fix/land work without leaving the cockpit.

### Paneflow user (secondary)
OSS users reviewing their own or agent-produced changes. Want a fast, legible, native diff surface that feels like the rest of the app and surfaces its best capabilities without a manual.

## Research Findings (audit summary)

Full per-dimension reports: 7-agent audit, 2026-06-17. Key evidence cited inline below.

- **Two render stacks, zero sharing.** `diff/element.rs:120` `DiffElement` (virtualized direct-paint) is unreachable from `agents_diff.rs`, which uses GPUI `list` (`agents_diff.rs:1353`). Two parsers: `diff/git.rs` imara_diff vs `agents_diff.rs:1878` `parse_unified_diff`. `split_hunk_rows` (`agents_diff.rs:1005`) duplicates `build_split_rows` (`diff/rows.rs`).
- **Tokens are shared; structure is not.** Both views bind to `UiColors` (8 slots) + `DiffColors` (`theme/model.rs:291-395`). But `DiffHeaderTooltip` (`view.rs:172`) is a byte-for-byte copy of `HoverActionTooltip` (`agents_sidebar/mod.rs:1405`); the diff sidebar filter, section header, empty states, and icon buttons are all re-coded inline instead of calling the Agents view primitives.
- **The body color is good; the hierarchy is flat.** All chrome at `ui.surface`; context rows get no wash (`element.rs:347`); selected column = 1px bottom accent border the code itself comments as "~invisible" (`view.rs:2301`); file header is one undifferentiated mono string (`rows.rs:329`); lone hardcoded `rgba(0x0ea5e9bf)` (`view/render.rs:136`); word-diff alpha 0.40 too strong on dark (`view.rs:2062`).
- **Keyboard is nearly absent.** Exactly one DiffView-context binding (`Ctrl+Shift+C` copy-hunk, `view/render.rs:224`). `goto_hunk` (`view.rs:1076`) already exists; only action registration + bindings are missing.
- **Signature interaction invisible.** `handle_body_click` → `ask_review_about_line` (`view.rs:1128`) launches/prefills an AI question on click with no tooltip, no hint, no empty-state education. Disabled silently in Split view (`view.rs:1332`).
- **Review prefill is timing-based.** `REVIEW_PREFILL_DELAY_MS = 1800` (`view.rs:106`) with a 9px muted clipboard-fallback hint shown only after the race is lost (`view.rs:1808`).
- **Attribution foundation exists.** `SessionMeta` (`agent_sessions.rs:465`) carries agent + cwd + git_branch + timestamp; `DiffWorktree` (`view/model.rs:8`) carries path (= cwd) + branch. Matching needs zero new parsing. Token usage + model name + pricing table do NOT exist yet (`claude_sessions.rs:43` envelope has no usage; grep finds no pricing table).
- **The interface is read-only by construction.** `diff/git.rs` runs only read commands (`view confirmed at git.rs:150-651`). The agent-mediated act bridge (`review_terminal.rs`, `ask_review_about_hunk`) already exists and is the correct, human-in-loop "act" path.
- **External reference (Codex/Linear/Zed/Geist):** three-surface depth via luminance not borders; diff semantics via background tint (8-12%) + 2-3px left-edge bar, never bright text; tabular numerics everywhere; metadata as border-only pills; empty states as structural silence (no illustration/animation); no per-row dividers; no animation on diff content.

## Assumptions & Constraints

- The Agents view's primitive library (`settings/components.rs`, `agents_sidebar/mod.rs`, `widgets/`) is the canonical premium bar; the Review redesign adopts it rather than inventing.
- Claude Code / Codex session JSONL formats remain parseable; attribution is enrichment and degrades silently on parse failure or absent data.
- The GPUI fork pin (`ArthurDEV44/zed@paneflow/markdown-append-fix`) stays as-is; this work needs no fork changes.
- Cross-platform mandatory: Linux (Wayland + X11), macOS (Intel + Apple Silicon), Windows. No POSIX-only paths or commands. All git access via the existing subprocess wrappers with timeouts, never via a shell.
- No new heavyweight dependencies. Pricing table is embedded at build time (no network: 100% local is a hard Paneflow constraint).
- Performance floor is non-negotiable: the unified renderer must match or beat current scroll performance.

## Quality Gates

Every user story must pass:
- `cargo fmt --check` (run before every commit and tag-push; the release pipeline fails all four matrix legs on a single diff)
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`

UI stories add: launch `cargo run` against a fixture repo with ≥ 2 worktrees and visually verify the story's acceptance criteria on Linux; no panics, no frozen frames, no scroll-frame regressions. macOS/Windows branches are inspection-verified where a Linux host cannot run them.

## Epics & User Stories

### EP-001: Unification & Design-System Foundation

The enabler. Collapse the two diff engines into one and lift the Agents view primitives into a shared library so every later visual change is made once. Invisible to the user; unblocks everything.

**Definition of Done:** the agents diff panel and the Review mode render through one `DiffElement` + one git pipeline; the shared primitive set exists in `settings/components.rs`; duplicated structs are deleted; all gates green; no visual or perf regression.

#### US-001: Unify the diff render path
**Priority:** P0 · **Size:** L (5 pts) · **Dependencies:** None
**Acceptance Criteria:**
- [x] `app/agents_diff.rs` renders via `diff/element.rs` `DiffElement` (with a thin viewport wrapper) instead of its own GPUI `list` renderer; the bespoke `render_flat_*` functions (`agents_diff.rs:1139-1763`) are removed
- [x] Row height, file-header height, gutter width, and color washes are sourced from one set of constants shared by both surfaces (reconcile the 22px/18px and 44px/32px divergence to a single value)
- [x] The agents diff panel keeps its current behavior (collapse/expand, split/unified, untracked-file display) through the unified path
- [x] Scroll performance on a 200-file diff is unchanged or improved (frame time < 16ms P95 on the reference machine) — perf/GUI verification pending (architecture = proven DiffElement fast path)
- [x] Net deletion of ≥ 400 LOC of duplicated render code

#### US-002: Unify the git diff pipeline
**Priority:** P0 · **Size:** M (3 pts) · **Dependencies:** Blocked by US-001
**Acceptance Criteria:**
- [x] `diff/git.rs` exposes a HEAD-relative diff variant (e.g. `compute_head_diff`) so the agents panel consumes the structured `imara_diff` pipeline instead of `parse_unified_diff`
- [x] `agents_diff.rs:1878` `parse_unified_diff` and `agents_diff.rs:1005` `split_hunk_rows` are deleted; both surfaces produce the canonical `FileDiff`/`DiffHunk`/`DisplayRow` types
- [x] Binary files, files > 512 KiB, and lockfiles are stubbed identically on both surfaces
- [x] The agents pipeline gains the parser tests that `diff/git.rs` already has (no untested git plumbing remains)

#### US-003: Extract the shared primitive library
**Priority:** P0 · **Size:** M (3 pts) · **Dependencies:** None (parallel with US-001)
**Acceptance Criteria:**
- [x] `settings/components.rs` (or a new `ui_primitives.rs`) gains `pub(crate)`: `PaneflowTooltip`, `filter_pill`, `section_eyebrow`, `toolbar_pill`, `icon_button_sm` (20×20) / `icon_button_md` (24×24), `panel_empty_state(icon, message, animate)`
- [x] `DiffHeaderTooltip` (`view.rs:172`) and `HoverActionTooltip` (`agents_sidebar/mod.rs:1405`) are both replaced by `PaneflowTooltip`
- [x] The diff sidebar filter (`diff_sidebar/mod.rs:237`) and Agents filter both call `filter_pill`; the diff "Changes" header (`diff_sidebar/mod.rs:163`) calls `section_eyebrow`
- [x] Type-scale constants are declared (`LABEL_XS=10`, `LABEL_SM=11`, `BODY=12`, `BODY_EMPHASIS=13`, `TITLE=14`) and used by new code
- [x] No new duplicate primitive is introduced anywhere in the diff module

#### US-004: Decompose the god-files (code-motion only)
**Priority:** P1 · **Size:** M (3 pts) · **Dependencies:** Blocked by US-001, US-002
**Acceptance Criteria:**
- [x] `view.rs` is split along its seams: `view/loader.rs` (load lifecycle + generation guards), `view/scroller.rs` (sync + file-at-offset), `view/interaction.rs` (click/hover/menu), `view/review.rs` (review terminal lifecycle); `render_column`/`render_toolbar` move into `view/render.rs`
- [x] `agents_diff.rs` is split into `agents_diff/{git,model,render,mod}.rs`
- [x] Behavior-preserving: zero functional change, all tests green, golden diff is rename/move only

---

### EP-002: Premium Visual Hierarchy

Make the Review surface read as a structured document navigator, not terminal output. Pure visual work on top of the unified renderer.

**Definition of Done:** column header, file card, and body each occupy a distinct luminance tier; the file header is decomposed into typed segments; one type scale; selected/sticky/context states are clearly readable; no raw-hex leaks.

#### US-005: Three-tier surface scale
**Priority:** P0 · **Size:** S (2 pts) · **Dependencies:** Blocked by EP-001
**Acceptance Criteria:**
- [x] Column header at a darker chrome tier (e.g. `ui.overlay`), file card at `ui.surface`, body at `ui.base`, sticky header at a blended elevated variant; no two adjacent tiers share a value (`view.rs:2034-2039,2255`) — column header is `ui.overlay` on dark / `ui.subtle` on light (overlay==base==white on light), sticky = `surface.blend(text 0.06)`
- [x] Surface luminance steps replace borders between same-level elements; 1px borders only at cross-hierarchy boundaries
- [x] Verified on both bundled dark and light themes — GUI visual pass pending (headless host)

#### US-006: File-header decomposition
**Priority:** P0 · **Size:** M (3 pts) · **Dependencies:** Blocked by EP-001
**Acceptance Criteria:**
- [x] The single `{sigil} {path} +N -N` string (`rows.rs:329`, `element.rs:230`) becomes four typed segments: status sigil (status color), basename (semibold, primary), directory prefix (muted), diffstat (right-aligned, green/red tint) — `HeaderParts` built off-thread, painted by `element::paint_file_header`
- [x] Diffstat uses the semantic tint colors at controlled saturation, monospace, right-aligned
- [x] Long paths truncate on the directory prefix, never on the basename

#### US-007: Type scale, gutter, context wash, word-diff tuning
**Priority:** P1 · **Size:** M (3 pts) · **Dependencies:** Blocked by US-003 (scale constants)
**Acceptance Criteria:**
- [x] All Review UI uses the type-scale constants; no 9px or 10px text remains in non-tooltip product UI — every `text_size(px(N))` across `diff/` + `diff_sidebar/` migrated to `ui_primitives::{LABEL_XS/SM,BODY,BODY_EMPHASIS}` (9px bumped to LABEL_XS)
- [x] A persistent gutter background quad (~4% muted) renders the line-number rail as a structural column (`element.rs:347`) — `RowPalette.gutter_bg` (muted 0.045), per content row
- [x] Context rows get a 2-3% wash so the body reads as a document surface, not the window background — `RowPalette.context_bg` (muted 0.025)
- [x] Word-diff background alpha reduced from 0.40 to ~0.28 on dark (`view.rs:2062`); gutter width is fixed for the diff session (no jitter) — gutter width already derived from precomputed `max_line_no` (no per-frame jitter)

#### US-008: Readable interactive states + token-guard
**Priority:** P1 · **Size:** S (2 pts) · **Dependencies:** Blocked by US-005
**Acceptance Criteria:**
- [x] Selected column uses a 3px left accent bar + tinted header (`ui.accent.opacity(0.08)`), replacing the ~invisible 1px bottom border (`view.rs:2255-2257,2301`)
- [x] Sticky file header is visually elevated above the inline file card (distinct tint or shadow, not just a hairline) — `sticky_header_bg = surface.blend(text 0.06)`
- [x] `rgba(0x0ea5e9bf)` drag-target border (`view/render.rs:136`) replaced with `ui.accent.opacity(0.75)`
- [x] `ColumnState::Failed` renders a `Callout` (Error) instead of an inline string (`view.rs:2180`); loading states use the animated `loader-circle.svg` pattern from `agents_sidebar`

---

### EP-003: Interaction & Discoverability

Make the surface keyboard-complete and surface its best capabilities. This is where "viewer" becomes "tool".

**Definition of Done:** the review loop is keyboard-driven; click-to-ask is visible and named; the scope selector is legible; empty/loading states onboard the user; the review prefill never fails silently.

#### US-009: Keyboard-first review loop
**Priority:** P0 · **Size:** M (3 pts) · **Dependencies:** Blocked by EP-001
**Acceptance Criteria:**
- [x] DiffView-context bindings added: `[`/`]` prev/next hunk (wired to `goto_hunk`), `u` unified/split, `s` sync, `Esc` dismiss popover + refocus body — actions `Diff{Next,Prev}Hunk`/`DiffToggleView`/`DiffToggleSync`/`DiffDismiss`, all context `DiffView && !Terminal && !TextInput` so neither an embedded review/shell terminal nor the base-branch filter input loses a keystroke (the `!` clauses are what make bare keys safe). Open Question resolved: `[`/`]` over `J`/`K`. Body+header click now focus the DiffView so the loop is live without tabbing in.
- [x] The hunk counter pivots on `cur_y + HUNK_JUMP_MARGIN` (the hunk parked at the viewport top by `goto_hunk`), not the cumulative count of every hunk scrolled past — counter now reads exactly the hunk the nav last jumped to
- [x] All toolbar actions have a keyboard path; the 5 actions are registered in `keybindings/registry.rs` so they appear in the shortcuts display

#### US-010: Make click-to-ask visible and named
**Priority:** P0 · **Size:** M (3 pts) · **Dependencies:** Blocked by EP-001
**Acceptance Criteria:**
- [x] Changed lines show a hover tooltip "Click to ask an agent about this line" (attached only while the cursor is over a changed line, via the existing hover-row tracking)
- [x] First entry to Review shows a one-line onboarding bar naming the capability (`render_ask_hint`); self-hides once any review CLI runs (capability then self-evident) or on `×` dismiss
- [x] Split-view parity: `actionable_row_at` now also detects changed Split rows; the tooltip there reads "Switch to Unified view to ask an agent about this line" (no misleading pointer/highlight in Split)
- [x] The Review button is now a labeled `toolbar_pill` (sparkles icon + "Review"), per-column header and solo toolbar both, replacing the bare 18px eye

#### US-011: Reliable review prefill
**Priority:** P1 · **Size:** S (2 pts) · **Dependencies:** Blocked by EP-001
**Acceptance Criteria:**
- [x] On Review launch the review-terminal header shows a prominent accent pill "Prompt ready · {key} to paste" immediately (rendered the instant the terminal mounts, before the prefill timer), replacing the prior subtle muted hint
- [x] `review_prefill_delay_ms` config field (default 2000 ms, clamped `[250, 10000]`) replaces the hardcoded `REVIEW_PREFILL_DELAY_MS`; both review launch paths read `config.resolved_review_prefill_delay_ms()`; a stepper in Settings → AI Agent → Review edits it; the clipboard write stays the synchronous safety net
- [x] Behavior verified with a deliberately slow CLI start (cold-start simulation) — manual/GUI verification pending (headless host)

#### US-012: Legible scope selector + onboarding states
**Priority:** P1 · **Size:** M (3 pts) · **Dependencies:** Blocked by US-003
**Acceptance Criteria:**
- [x] The scope/project/branches trio keeps the `›`-separated hierarchy; the project + branches triggers are demoted to muted `ui.muted` secondary labels under the primary scope chip
- [x] Worktree-scope branch badge shows "{chosen}/{total} branches" (or "All {total} branches") without opening the picker — the total is eagerly fetched off-thread on Worktree-scope entry (`rebuild_diff_view`), degrading to the chosen count when not yet known
- [x] No-repo / loading / no-changes states use `panel_empty_state` (no-repo: icon + title + positioning hint; loading: animated loader + the active branch being diffed; no-changes: a "Clean" state); the sidebar filter renders only when a file list actually exists (`has_files` gate). Edge-case-#1 open-project affordance = the breadcrumb project picker

#### US-013: Per-file actions in the sidebar
**Priority:** P2 · **Size:** S (2 pts) · **Dependencies:** Blocked by EP-001
**Acceptance Criteria:**
- [x] Sidebar file rows reveal a hover cluster (right-anchored, `group_hover`): a collapse toggle (`DiffView::toggle_file_collapse`, mirrors the body header click) and a copy-file-diff action (`DiffView::copy_file_diff` → `copy_scope` file scope). Both route via the scope's host (single `DiffView` or `MultiRepoDiffView::active_*`)
- [x] Sidebar vertical rhythm normalized — directory rows bumped 24→28px to match the file-row height (they interleave in tree mode); the 10px+indent / 12px padding rail is shared

---

### EP-004: Agent Attribution & Cost

The differentiator: connect each worktree's diff to the agent session that produced it, with model and estimated cost inline. Always enrichment, never blocks review, always free.

**Definition of Done:** Claude Code and Codex worktrees show agent + model + estimated cost in the column header; absence of session data changes nothing; cost is always labeled estimated and never fabricated.

#### US-014: Session-to-column matching (zero new parsing)
**Priority:** P0 · **Size:** S (2 pts) · **Dependencies:** Blocked by EP-001
**Acceptance Criteria:**
- [x] A pure `match_sessions_to_column(sessions, col_path, col_branch)` ranks by exact-cwd > branch > timestamp via `cwd_matches`; folded into the off-thread column-load task (`loader.rs`), so attribution never touches the main thread. (Dropped the `time_window` param: a pure fn has no clock, and the readers + recency sort already surface the most recent — the ranking is the deliverable.)
- [x] Result cached on the `Column.attribution` field, populated only when the off-thread diff load lands (re-fetched only on re-diff); per-frame render reads the Vec O(1)
- [x] Codex uses cwd-only matching (empty branch never wins the branch tier); the `#[allow(dead_code)]` on `SessionMeta::git_branch` is removed (now consumed by `match_sessions_to_column`)

#### US-015: Agent badge in the column header
**Priority:** P0 · **Size:** S (2 pts) · **Dependencies:** Blocked by US-014
**Acceptance Criteria:**
- [x] `render_attribution_badge` (new `view/attribution.rs` seam) renders between the file-count chip and the base chip: agent glyph (brand-tinted svg) + model short name + "~$X.XX (est.)", as a border-only pill
- [x] No matching session = `None` → zero-width slot; the no-attribution header is pixel-identical to today
- [x] Multiple sessions are listed in a hover `AttributionTooltip` (most relevant first: branch then recency), with a per-session cost line

#### US-016: Token usage + model parsing
**Priority:** P1 · **Size:** L (5 pts) · **Dependencies:** Blocked by US-014
**Acceptance Criteria:**
- [x] `SessionMeta` extended additively with `model: Option<String>` and `usage: Option<AssistantUsage { input, output, cache_read, cache_creation }>`; a parametrized scan (`read_session_meta_inner(path, scan_usage)`) walks past the title break aggregating `message.usage` across assistant turns + capturing `message.model`, bounded by `MODEL_USAGE_SCAN_LIMIT` (20k), exposed via `read_sessions_with_usage_for_cwd` (bypasses the title mtime cache; the attribution result is Column-cached instead)
- [x] Codex rollout usage parsed (`turn_context.payload.model` + the last cumulative `token_count`, normalized to the shared tiers: input = uncached, cache_read = cached subset); OpenCode degrades to agent + recency (no model/usage in its CLI contract). Codex `token_count` shape is best-effort — a schema miss yields `usage = None`, never an error
- [x] Corrupt/unknown-schema lines fall through `serde_json::from_str` and are skipped; absent session dirs return an empty Vec (the readers already guard this)

#### US-017: Cost estimation + display
**Priority:** P1 · **Size:** S (2 pts) · **Dependencies:** Blocked by US-016
**Acceptance Criteria:**
- [x] New `src-app/src/pricing.rs` embeds a versioned `PRICING_TABLE` (Claude opus/sonnet/haiku + Codex gpt-5/gpt fallback) with a documented update procedure (`PRICING_TABLE_VERSION` + ordered specific→general substring match); `estimate_cost` per session, aggregated per worktree (`column_cost`) and across worktrees (`attribution_total`)
- [x] Every cost is labeled "(est.)" and the tooltip footer carries `prices v<VERSION>`; the badge shows "~$X.XX (est.)" with a tooltip breakdown (per-session lines + aggregate token tiers + version)
- [x] Unknown model = tokens shown in the tooltip, cost omitted (`estimate_cost` → `None`, tooltip reads "unpriced model"); never a fabricated number
- [x] The toolbar shows an aggregated "Total ~$X.XX (est.) · N worktrees" (`render_toolbar`), hidden entirely when nothing is priced

---

### EP-005: Human-in-Loop Act Layer

Make the agent-mediated "act" pathway a first-class, visible part of the hierarchy. No native git writes, ever.

**Definition of Done:** a clear per-hunk "direct the agent" action exists outside the right-click menu; a fix/stage prompt variant exists; agent-mediated discard uses the two-step armed pattern; the worktree shell escape hatch is discoverable.

#### US-018: First-class "Direct agent at this hunk"
**Priority:** P0 · **Size:** M (3 pts) · **Dependencies:** Blocked by EP-001, US-010
**Acceptance Criteria:**
- [x] A floating per-hunk action cluster (`render_hunk_actions`, `interaction.rs`) is revealed while hovering a changed line over a resolvable hunk — deferred+anchored above the cursor, NOT in the right-click menu — and a `DiffActOnHunk` keybinding (`a`) directs the agent at the hunk under the cursor / viewport. Both route through `act_on_hunk` → `send_to_review` (the same send path as `ask_review_about_hunk`)
- [x] The cluster's primary button is "Direct agent" (act intent, distinct from the right-click "Ask the CLI about this hunk" review framing); the prompt directs the agent and never claims Paneflow itself stages
- [x] Fully human-in-loop: pre-filled prompt, no auto-submit (reuses `send_to_review`, which appends without Enter and focuses)

#### US-019: Fix-and-stage prompt variant
**Priority:** P1 · **Size:** S (2 pts) · **Dependencies:** Blocked by US-018
**Acceptance Criteria:**
- [x] `build_cli_hunk_prompt(HunkAction::FixStage, …)` alongside `build_cli_review_prompt` (`review_terminal.rs`) directs the CLI to apply the good parts and stage selectively (`git add -p` / `git apply`) and explain what it kept/dropped (unified the three act prompts under one builder + a `HunkAction` enum rather than a standalone `build_cli_fix_prompt`)
- [x] Selectable per-hunk via the cluster's "Fix & stage" button; the agent runs the git action in the witnessed terminal; Paneflow runs no git write itself

#### US-020: Safe agent-mediated discard + shell escape hatch
**Priority:** P2 · **Size:** S (2 pts) · **Dependencies:** Blocked by US-018
**Acceptance Criteria:**
- [x] The cluster's "Discard" button uses the two-step armed pattern: first click arms (`hunk_discard_armed`, red "Confirm discard" pill), second click executes `act_on_hunk(HunkAction::Discard, …)`; moving to a different line or firing any act disarms
- [x] The per-column terminal button (and the solo-scope toolbar terminal button) gains the explicit tooltip "Open a shell here to run git commands in this worktree" (the manual-git escape hatch)

## Functional Requirements

- FR-01: One render path and one git-diff pipeline serve both the Review mode and the agents diff panel.
- FR-02: The Review interface never executes a git write command; the only "act" paths are agent-mediated (CLI) and a manual shell in the worktree.
- FR-03: The interface presents a three-tier luminance hierarchy (body / file card / chrome) and a single declared type scale.
- FR-04: The core review loop (next/prev hunk, ask, launch review, toggle view/sync) is fully keyboard-operable.
- FR-05: Click-to-ask and direct-agent affordances are visibly discoverable (tooltip + entry-state education), not hidden behind hover or right-click.
- FR-06: Worktree columns are attributed to local agent sessions when data exists, showing agent, model, and estimated cost, and degrade silently to nothing when it does not.
- FR-07: All cost figures are labeled estimated and carry a pricing-table version; unknown models show tokens without a cost.
- FR-08: All git access is via subprocess wrappers with timeouts, never a shell; attribution parsing is read-only over session files.
- FR-09: Empty, loading, and failed states are designed components, not raw strings.

## Non-Functional Requirements

- **Performance:** unified renderer matches or beats current scroll (200-file diff < 16ms P95 frame time on the reference machine); attribution runs off-thread and never blocks first diff paint; per-frame render O(1) in attribution.
- **Cross-platform:** Linux (Wayland + X11), macOS (Intel + Apple Silicon), Windows; no POSIX-only assumptions; verified per platform or inspection-noted.
- **Accessibility:** full keyboard nav of the review loop; reduced-motion setting honored (loading animation respects it); diff tint + left-edge bar provides a redundant non-color signal; text contrast ≥ WCAG AA on both themes.
- **Reliability:** zero `panic!`/`unwrap` in new production paths (house lints); attribution failures never surface as errors.
- **Maintainability:** no duplicate UI primitive across Review and Agents; god-files decomposed along stated seams.

## Edge Cases & Error States

| # | Scenario | Expected behavior |
|---|----------|-------------------|
| 1 | No git repo in active workspace | Designed empty state with a one-line positioning hint + open-project affordance |
| 2 | Diff computing on a large repo | Animated loading state showing the branch being diffed; siblings unaffected |
| 3 | Git subprocess fails | `Callout` (Error) per column with the stderr excerpt + retry, never a raw string |
| 4 | Worktree with zero changes | "Clean" empty state, not an error |
| 5 | No matching agent session | Zero-width attribution slot; review unaffected |
| 6 | Unknown model in pricing table | Tokens shown, cost omitted with "unpriced model" tooltip |
| 7 | Review prefill race lost (slow CLI) | Clipboard fallback already surfaced prominently before the race |
| 8 | Click-to-ask in Split view | Works, or a clear tooltip explains the Unified-only limitation |
| 9 | OpenCode worktree | Agent badge + recency only (no tokens/cost), by graceful design |
| 10 | Corrupt session JSONL line | Skipped with debug log; parsing continues |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Unifying the two renderers regresses the agents panel behavior or perf | Med | High | US-001 is the de-risking story, scheduled first; golden behavior tests + perf check before/after; the agents panel is the smaller surface, so it adopts `DiffElement` rather than the reverse |
| 2 | Scope creep toward a generalist git client | High | Med | Hard Non-Goals; no native git writes; "act" is agent-mediated only; verbs named explicitly in UI |
| 3 | Session format drift breaks attribution | Med | Med | Enrichment-only with silent skip; fixtures per format; bounded scan caps |
| 4 | Pricing table goes stale | Med | Low | Versioned table, documented update procedure, always labeled "est.", unknown model shows tokens only |
| 5 | God-file decomposition introduces behavior changes | Med | Med | US-004 is code-motion only, behavior-preserving, golden diff = move/rename, full test pass |

## Non-Goals

- **No monetization of any kind in this interface.** No paid plan, license gate, trial, or feature paywall. Everything ships in the free GPL core.
- **No native git write operations.** No staging checkboxes, commit form, native discard, merge, or worktree-remove run by Paneflow. The act layer is agent-mediated or a manual shell.
- **Not a generalist git client.** No interactive rebase, blame, commit-graph/log browser, submodule UI, or 3-way merge editor.
- **No new diff renderer.** The existing `DiffElement` is preserved and unified, not replaced.
- **No animation on diff content.** Hunk expand, line insert, and scroll are instant; animation is limited to loading states (reduced-motion honored).
- **No separate metadata side panel.** Attribution and cost live inline in the column header, never in a permanent right column.

## Files to Modify (primary)

- `src-app/src/diff/` (view.rs and seams, element.rs, rows.rs, git.rs, scope_header.rs, review_terminal.rs)
- `src-app/src/app/agents_diff.rs` (unify onto shared render + pipeline, then decompose)
- `src-app/src/app/diff_sidebar/` (filter, section header, per-file actions, empty states)
- `src-app/src/settings/components.rs` (shared primitive library) and `widgets/` (Callout reuse)
- `src-app/src/{claude_sessions,codex_sessions,opencode_sessions,agent_sessions}.rs` (additive usage/model fields)
- `src-app/src/theme/model.rs` (surface-tier tokens, type-scale constants)
- New: `src-app/src/pricing.rs`
- `src-app/src/keybindings/` (DiffView-context bindings)

## Files NOT to Modify

- The GPUI fork pin in `src-app/Cargo.toml` / `Cargo.lock` (no fork change needed).
- `~/.claude/**`, `~/.codex/**`, `~/.config/opencode/**` (session data parsed strictly read-only).

## Technical Considerations

- **Renderer unification (US-001):** confirm whether the agents panel needs any `DiffElement` capability it lacks today (it gains word-diff, syntax highlight, sticky headers for free). Decide the viewport-wrapper boundary so the agents panel can size independently inside its column.
- **HEAD-relative diff (US-002):** confirm `compute_head_diff` semantics (HEAD~1 vs working tree vs index) match what the agents panel shows today before deleting `parse_unified_diff`.
- **Attribution composition (US-014):** fold the session match into the existing off-thread diff task; return `ColumnDiff { diff, sessions }` rather than a second async round-trip.
- **Token scan cost (US-016):** the early-exit in the Claude parser is a deliberate perf choice; the usage scan must stay bounded (`MODEL_USAGE_SCAN_LIMIT`) and run only on attribution, not on the title path.
- **Pricing updates (US-017):** embed at build time v1; a signed remote manifest is out of scope unless model churn proves painful.
- **Type scale (US-003):** the Agents view already uses an implicit 5-step scale; name those exact values, do not invent new ones.

## Open Questions

- **Mode name:** RESOLVED (EP-003) — keep "Diff" for muscle memory. The rename was an Open Question, not an AC of any US-009..013 story, and a global "Diff"→"Review" rename (AppMode::Diff, tab titles, keybinding labels, scope header) is broad reversible churn; deferred to a dedicated decision. The EP text already calls the surface "Review" conceptually.
- **Keyboard scheme:** RESOLVED (US-009) — `[`/`]` for prev/next hunk (over `J`/`K`). Clash with terminal-context bindings is avoided structurally by the `DiffView && !Terminal` context predicate, not by key choice.
- **Attribution scope coverage:** show attribution in all three scopes (Project / Multi-project / Worktree) or Worktree-only at first? Worktree is where it matters most; Project/Multi-project can follow.
[/PRD]
