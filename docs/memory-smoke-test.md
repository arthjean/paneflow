# Memory smoke-test runbook

Manual validation for the memory-optimization PRD. This is not an RSS
benchmark. It proves the issue #11 workload stays usable while the structural
caps added by the PR are observable in code, logs, tests, or direct inspection.

This runbook implements the historical `US-012` memory-validation contract.

---

## When to run

Run this before opening or merging the memory-optimization PR, after the
dependent stories are in review:

- `US-001` / `US-002`: hidden Review and Agents diff caches are released.
- `US-004` / `US-006`: agent/review terminal profiles and terminal-cache
  retention are bounded.
- `US-010`: GPUI-bound IPC requests are queued with backpressure.

Run the full 30-minute desktop smoke on every OS you can access. If Linux,
macOS, or Windows cannot be exercised locally, write `not verified on <OS>` in
the PR note instead of assuming parity.

---

## Prerequisites

From the repository root:

```bash
cargo build -p paneflow-app --locked
cargo fmt --check
cargo clippy --workspace --locked -- -D warnings
cargo test --workspace --locked
```

Run these commands only on a trusted checkout. Rust builds and tests can execute
build scripts, proc macros, tests, and the app itself; `--locked` keeps the
dependency graph pinned to the reviewed `Cargo.lock`.

For the interactive run, launch PaneFlow from a terminal so logs are visible.

PowerShell:

```powershell
$env:RUST_LOG = "info,paneflow=debug"
cargo run -p paneflow-app --locked
```

Bash:

```bash
RUST_LOG=info,paneflow=debug cargo run -p paneflow-app --locked
```

Keep a separate shell open for CLI inspection commands. If the smoke uses
only the read-only `paneflow ps` bursts below, do not enable
`PANEFLOW_IPC_SCRIPTING`. If a separate manual check really needs
`paneflow send`, use a disposable workspace, run a short-lived app session with
`PANEFLOW_IPC_SCRIPTING=1`, and close it immediately after the check; that mode
allows any same-UID process to inject keystrokes into agent panes.

Do not paste raw debug logs into the PR. Summarize pass/fail results, and
sanitize workspace paths, process arguments, terminal output, tokens, and
customer data before sharing logs anywhere.

---

## Static cap inspection

Run these checks before the live smoke. They make the structural limits visible
without relying on fragile process-memory numbers.

```bash
rg -n "AGENT_SCROLLBACK_LINES|REVIEW_SCROLLBACK_LINES|CACHED_SCROLLBACK_LINES|resolved_scrollback_lines_for_profile" crates/paneflow-config/src/schema.rs
rg -n "AGENTS_TERMINAL_HOT_CACHE_LIMIT|BOTTOM_TERMINAL_HOT_CACHE_LIMIT|AGENTS_TERMINAL_CACHE_IDLE_TTL|oldest_evictable_terminal_id" src-app/src/app
rg -n "IPC_REQUEST_QUEUE_CAPACITY|IPC_DRAIN_MAX_PER_TICK|IPC_DRAIN_MAX_DEQUEUES_PER_TICK|sync_channel|try_send" src-app/src/ipc.rs src-app/src/app/ipc_handler.rs
rg -n "close_agents_diff_panel|hide_column|drop_loaded_data|reset_display_caches|review_terminals" src-app/src/app/agents_diff src-app/src/diff
rg -n "MAX_DISPLAY_ROWS|MAX_FILE_BYTES|truncated" src-app/src/diff
```

Expected inspection results:

- Normal terminal scrollback remains `10_000`; agent, review, and cached
  profiles resolve to `4_000`, `2_000`, and `1_000` lines respectively.
- Agent terminal hot cache is bounded at 8 entries, with active/running
  terminals protected from eviction.
- Bottom terminal hot cache is bounded separately.
- IPC GPUI pending requests use a 256-capacity queue, and each UI tick drains at
  most the documented live request budget.
- Agents diff close clears the cached model and offsets; hidden Review columns
  clear loaded row/display/review-terminal state.
- Diff display rows and raw file bytes stay capped, with truncation state
  visible instead of retaining every row from a huge diff.

Useful targeted regression tests:

```bash
cargo test -p paneflow-config --locked terminal_scrollback_profiles_resolve_defaults_and_caps
cargo test -p paneflow-app --locked hidden_column_cleanup_drops_loaded_data_and_display_caches
cargo test -p paneflow-app --locked oldest_evictable_terminal_id_protects_active_and_running
cargo test -p paneflow-app --locked dispatch_to_gpui_returns_overload_when_request_queue_full
cargo test -p paneflow-app --locked ipc_drain_caps_live_requests_per_tick
cargo test -p paneflow-app --locked ipc_drain_skips_cancelled_without_spending_live_budget
cargo test -p paneflow-app --locked ipc_drain_caps_cancelled_dequeues_per_tick
```

If a test name changes, use the `rg` commands above to find the current targeted
coverage and record the replacement in the PR note.

---

## Live 6-8 agent smoke

Target duration: 30 minutes.

### Setup

1. Open a real project workspace in PaneFlow.
2. Start 6-8 agent or shell surfaces. A valid mix is:
   - 4-6 hooked agent sessions producing periodic output.
   - 2 plain shell panes running harmless loop output.
3. Make at least one workload produce steady output for the whole run. Use a
   cross-platform command when possible:

   PowerShell:

   ```powershell
   1..1800 | ForEach-Object { "memory-smoke $_ $(Get-Date -Format o)"; Start-Sleep -Seconds 1 }
   ```

   Bash:

   ```bash
   for i in $(seq 1 1800); do echo "memory-smoke $i $(date -Is)"; sleep 1; done
   ```

4. Keep one agent actively running while navigating away from its surface.

### Actions during the 30 minutes

- Switch between Agents, normal terminal panes, Review, and the Agents diff
  dock every few minutes.
- Open Agents diff on a repository with a non-empty diff, then close it. Within
  the next tick, the global Agents diff model should be released; a late async
  result must not recreate it.
- Open Review and load a diff. If review terminals are running, first close or
  let them exit; hiding the column is expected to be blocked while any review
  terminal is still running. After the review terminals are closed/exited, hide
  the Review column and verify hidden column data plus exited review-terminal
  references are released by the existing hide path.
- Open more than 8 agent threads if available. The cache may evict only
  inactive/exited terminals; active or running agents must stay alive.
- Close and reopen the bottom Agents terminal panel. Retention should follow the
  documented bottom-terminal cap, with running terminals protected.
- While the agents are producing output, send a small burst of IPC reads or
  status requests from another shell. The app should remain responsive, and
  overload should be reported as a clear retryable error if the queue fills.
  Read-only `ps` bursts are safe for this:

  PowerShell:

  ```powershell
  1..128 | ForEach-Object { paneflow ps --json > $null }
  ```

  Bash:

  ```bash
  for i in $(seq 1 128); do paneflow ps --json >/dev/null; done
  ```

### Pass criteria

- PaneFlow remains responsive: typing, focus movement, panel toggles, and tab
  selection keep working during the run.
- No active agent, PTY, or user process is silently terminated by memory policy.
- Old scrollback may be trimmed or capped according to the profile, but new
  output stays visible and the pane remains usable.
- Agents diff and hidden Review data are released after close/hide, either
  immediately, within 30 seconds, or on the next documented tick. Review column
  hide may be blocked while review terminals are still running; close or let
  those terminals exit before expecting review-terminal references to be
  released.
- IPC overload, if triggered, fails fast with the existing overload error rather
  than blocking indefinitely or growing an unbounded queue.
- Any OS not exercised is explicitly listed as not verified.

### Failure triage

- Bucket A: a prior memory story regressed. Reopen that story and fix before
  shipping the PR.
- Bucket B: the backend lacks a safe trim for an active live terminal, but the
  active process is protected. Document the fallback in the PR note.
- Bucket C: PaneFlow kills an active agent, loses current output beyond the
  intended scrollback window, deadlocks, or crashes. Block the PR.

---

## PR note template

Paste this into the PR description and fill the checkboxes honestly.

```markdown
## Memory optimization validation

Refs #11.

### User impact

This PR makes the multi-agent workspace more predictable under the issue #11
shape: 6-8 long-running agents, repeated Review / Agents diff navigation, and
bursty IPC usage. It does this through structural caps and cache release paths,
not by killing active agents or claiming a fragile RSS benchmark.

### Structural limits added

- Agent/review/cached terminal scrollback profiles: 4,000 / 2,000 / 1,000 lines.
- Agents terminal hot cache: 8 entries, with active/running terminals protected.
- Bottom terminal cache: bounded separately, with running terminals protected.
- Sessions/sidebar/attribution and closed-pane undo state: capped by the PRD
  stories where applicable.
- GPUI IPC pending queue: 256 requests, with per-tick drain budget.
- Hidden Review and closed Agents diff: loaded rows, offsets, attribution, and
  exited review-terminal references are released on hide/close; hiding a Review
  column is blocked while review terminals are still running.
- Large diff display rows and raw file reads are capped, with truncation made
  explicit in the diff model.

### Local checks

- [ ] `cargo fmt --check`
- [ ] `cargo clippy --workspace --locked -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] Targeted cap tests listed in `docs/memory-smoke-test.md`
- [ ] 30-minute 6-8 agent smoke on Linux: PASS / FAIL / not verified on Linux
- [ ] 30-minute 6-8 agent smoke on macOS: PASS / FAIL / not verified on macOS
- [ ] 30-minute 6-8 agent smoke on Windows: PASS / FAIL / not verified on Windows

### Smoke result

- Responsiveness:
- Agents diff close/reopen:
- Review hide/reopen:
- Active agent protection:
- IPC burst behavior:
- OS verification gaps:
- Log handling: raw logs not shared / sanitized if shared
```
