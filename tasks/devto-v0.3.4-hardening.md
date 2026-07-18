---
title: "Hardening the subsystem that runs your CLI agents (Paneflow v0.3.4)"
published: false
tags: rust, ai, performance, opensource
cover_image:
---

> Historical v0.3.4 draft. Several code paths cited below were later moved or
> deleted during the Agents mode and workspace refactors; do not use this as a
> current source map without re-checking paths against the present tree.

Paneflow is a terminal multiplexer and AI agents IDE written in pure Rust on top of Zed's GPUI. The pitch is one line: a Rust native host for your CLI agents (Claude Code, Codex, OpenCode) is leaner than the Electron app running the same agents.

That claim only holds if the thing survives an all day session, not a 5 minute demo. v0.3.4 is the release where I went and made it hold. Two audits back to back across four axes (memory, security, performance, robustness) on the agent subsystem, roughly 49 fixes.

This is the engineering writeup: what was broken, the actual code, and the number that proves the fix. Every finding cites a file and a line. If I can't point at the code, it's not a finding.

## The rules I held the whole way

Three constraints, not negotiable:

1. **No new `.unwrap()` / `.expect()`.** The project runs `panic = deny` on the clippy gate, so a careless panic fails the build, not review.
2. **No new abstraction layers.** Hardening that grows the codebase usually just relocates the bug.
3. **Every perf claim is reproducible.** There is a heaptrack runbook in the repo and Criterion baselines for the hot paths. "38x faster" is a command you can run, not a number I picked.

## Performance: the O(n2) in the markdown streaming path

This is the one worth reading.

Agent responses stream in as chunks. Each chunk gets appended to a `markdown::Markdown` widget for live rendering. The append in Zed's widget looks like this (`crates/markdown/src/markdown.rs:588` upstream):

```rust
self.source = SharedString::new(self.source.to_string() + text);
```

Read that carefully. Every append clones the entire accumulated source, concatenates, and reallocates. For a response that arrives in N chunks, you pay 1 + 2 + 3 + ... + N copies. That is O(n2) over the length of the response. On five agents streaming at once, this burned 30 to 50 percent of a core copying text that had not changed.

The fix has two parts.

**Part 1, root cause, in a fork.** I patched the widget to accumulate into a `String` buffer with `push_str` instead of rebuilding a `SharedString` on every call. One additive field, no API change. The Criterion bench inside the fork:

```
markdown_append/pre_fix_concat     ~94.4 us
markdown_append/post_fix_buffered  ~2.44 us
```

About 38x on that path. Nine downstream Zed consumers of the widget compile unchanged. Paneflow builds against `arthjean/zed@paneflow/markdown-append-fix` (pinned by exact sha in `Cargo.lock`, see `src-app/Cargo.toml:47-55`) and reverts to upstream the moment the PR merges.

**Part 2, bound the call rate, in app code.** Even with an O(1) append, calling it at 60 Hz for a long response is wasteful. The streaming tick is now adaptive (`src-app/src/agents/thread_view.rs:44-66`):

```rust
const STREAMING_TICK_FAST: Duration   = Duration::from_millis(16);  // 60 Hz, < 4 KB
const STREAMING_TICK_MEDIUM: Duration = Duration::from_millis(50);  // 20 Hz, past 4 KB
const STREAMING_TICK_SLOW: Duration   = Duration::from_millis(150); // ~7 Hz, past 16 KB
```

Long responses slow their repaint cadence as they grow. The eye does not notice 7 Hz on a 16 KB wall of text; the CPU does.

### Two smaller perf wins

**`highlight_code` is memoized.** Syntax highlighting via syntect ran synchronously inside `render`, 3 to 8 ms per visible code block per frame. It is now cached by language and a hash of the content (`src-app/src/agents/message_render.rs:1056-1121`):

```rust
fn highlight_cache_key(lang: Option<&str>, source: &str) -> u64 {
    let src_hash = djb2(source);
    let lang_hash = lang.map(djb2).unwrap_or(0);
    src_hash ^ lang_hash.rotate_left(13)
}
const HIGHLIGHT_CACHE_CAP: usize = 256;
```

On a warm cache, per frame cost drops from ~150 to 400 ms (3 to 8 ms across 50 blocks) to a HashMap lookup.

**`compute_activity_state` scans less.** Honest framing here: this is not O(1), and the audit's original note that called it that was wrong. The function used to scan the full `items` Vec (user messages, assistant chunks, tool calls) on every `cx.notify()`. It now walks a parallel `Vec<usize>` of tool call indices only (`src-app/src/agents/thread_view.rs:233-241`). A thread of 500 items with 200 tool calls scans 200 positions instead of 500. Still O(N), just over a smaller N. 2 to 5x on a typical thread. I am keeping the accurate number, not the flattering one.

## Memory: bounded footprint on long sessions

**Diff bodies are freed after review.** A `DiffSnapshot` held every edit's original `old_text` for the full thread lifetime. A refactor session of 500 turns (50 KB files, 20 edits each) retained 10 to 100 MB of file content for edits you had already reviewed and would never look at again. Now the body is dropped on review completion and the renderer shows a placeholder (`src-app/src/agents/edit_tool_block.rs:345-363`):

```rust
if let Some(n) = diff.cleared_diff_lines {
    return col
        .child(/* ... */ format!("[diff body cleared after review, {n} lines]"))
        .into_any_element();
}
```

The heaptrack target for this fix is under 5 MB delta, down from 50 to 200 MB on the reference scenario.

**Per tool call UI state is pruned.** `tool_label_markdown` (an `Entity<Markdown>` per tool call) and `diff_scroll_handles` used to be cleared only on Keep All / Reject All. Read, Search, and Execute calls leaked GPUI registry entries for the whole thread. They are now purged the moment each call hits a terminal state (`src-app/src/agents/thread_view.rs:1083-1085`).

**Caches are bounded.** The session cache was a HashMap global to the process with no cap; switching across 20+ project directories grew it monotonically. Now capped at 10 LRU entries (`src-app/src/agent_sessions.rs:52`). The composer's `pending_prompts` queue is bounded by both count and bytes: 8 prompts, 80 MiB (`src-app/src/agents/composer_ext.rs:53,64`).

## Security: the threat model is "something the agent touched"

An AI IDE reads files written by other tools all day. The relevant threat is not "the user is malicious," it is "the input the agent produced or read is hostile."

**JSONL parsers are capped at 64 KiB per line.** A malicious 500 MB single line JSONL planted in `~/.claude/projects/<slug>/` used to OOM the process on read. Both session readers now cap (`src-app/src/claude_sessions.rs:39`, `src-app/src/codex_sessions.rs:34`):

```rust
const MAX_LINE_BYTES: u64 = 64 * 1024;
```

An oversized line is detected and the session is skipped, not panicked on.

**The Rust 1.85 env race.** Rust 1.85 made `std::env::remove_var` `unsafe` because it races concurrent `getenv` from other threads. Paneflow scrubbed the `CLAUDECODE` env var, and it was doing it from the wrong place. It now runs at the top of `main()` before any thread or runtime exists (`src-app/src/main.rs:1023-1031`, `crates/paneflow-acp/src/spawn.rs:67-72`):

```rust
// SAFETY: called from main() before any thread::spawn or async
// runtime init, no concurrent getenv possible.
unsafe { std::env::remove_var(CLAUDECODE_ENV); }
```

The only race free place to mutate process env is before you have a second thread.

**`surface.send_text` was an undocumented same UID RCE primitive.** The IPC method that injects text into a pane could drive any agent or shell. It is now gated behind an explicit opt in and documented as such (`src-app/src/app/ipc_handler.rs:717-724`):

```rust
if !ipc_scripting_enabled() {
    return JsonRpcError {
        code: -32601,
        message: "surface.send_text disabled; set PANEFLOW_IPC_SCRIPTING=1 to enable".into(),
    }.into_value();
}
```

Even when enabled, payloads are capped at 64 KiB.

**API keys are redacted before they hit the log.** Under `RUST_LOG=trace`, wire lines were logged verbatim. Anything matching `sk-...` or `*_API_KEY=...` is now scrubbed before `trace_wire_line`, with a zero allocation fast path for the common case (`crates/paneflow-acp/src/spawn.rs:117-130`).

**`workspace.create` canonicalizes its cwd.** Relative traversal, NUL bytes, and passing a regular file as the cwd are rejected before use (`src-app/src/app/ipc_handler.rs:1241-1254`).

**Kill on parent death, cross platform.** If Paneflow dies, the agent CLIs it spawned should not survive as orphans. Linux uses `PR_SET_PDEATHSIG` in the shim's `pre_exec` (`crates/paneflow-shim/src/main.rs:273-319`), Windows uses a `JobObject` that kills on close (`src-app/src/agents/parent_guard.rs:48-55`), macOS is a documented no op pending a `kqueue` hook. The shim's self exclusion check uses the Unix `(dev, ino)` inode identity instead of comparing path strings, which a symlink can defeat.

**The breaking change.** `claude_code_bypass_permissions` now defaults to `false` (`crates/paneflow-config/src/loader.rs:696`). On a fresh install the agent asks before each Claude Code tool call. The old default of `true` was convenient and a latent vulnerability: per Anthropic's own docs, bypass mode offers no protection against prompt injection. If you scripted around the old behavior, set it back to `true` explicitly. You opt in to the loaded gun now.

## Robustness: no panics under resource exhaustion

**The IPC thread no longer takes the app down.** It used to `.expect()` on spawn. Under `RLIMIT_NPROC` exhaustion or `EAGAIN` on a fork bombed host, that panicked the GPUI main thread and killed every live agent. Now it degrades (`src-app/src/ipc.rs:219-340`):

```rust
if let Err(e) = spawn_result {
    status.disable();
    tracing::error!("IPC disabled: paneflow-ipc thread spawn failed: {e}. \
                     Check `ulimit -u` / container thread limits.");
    // tx dropped -> consumer sees Disconnected and tolerates it as "no IPC work this tick"
}
```

The app keeps running without IPC. One feature degrades instead of the whole process dying.

**`wait_for_exit` has a 30 s deadline.** A SIGKILL race where the OS reaped the zombie before `poll_child_exit` could observe it used to park a `spawn_blocking` thread forever. With tokio's blocking pool capped at 128, repeated races leaked the pool over a session (`src-app/src/agents/agent_terminal.rs:294-301`):

```rust
const WAIT_FOR_EXIT_DEADLINE: Duration = Duration::from_secs(30);
```

**The runtime event channel is bounded.** It was `futures::channel::mpsc::unbounded` and could accumulate hundreds of `RuntimeEvent::Chunk(String)` on a burst (Claude Code doing 200 rapid edits) before the GPUI consumer drained it, spiking 5 to 20 MB. Now it is `tokio::sync::mpsc` at capacity 256 with backpressure (`src-app/src/agents/runtime.rs:63-85`). While I was there, the runtime command channel moved off a `spawn_blocking` + `Mutex` + join per command (about 100 us each) to a native async receiver.

**Silent error sinks now leave a breadcrumb.** Six `Err(Closed)` branches in the title summarizer used to swallow a dropped update when a window closed mid task, so a generated title or file insertion would just vanish. They now log (`src-app/src/agents/title_summarizer.rs:179-263`).

**Mutex poison is recovered, not propagated.** If a thread panicked while holding the session cache lock, the next lock would panic on the poison in turn. The cache now recovers with `into_inner()` and warns (`src-app/src/agent_sessions.rs:113-130`). The same path treats a backwards clock step (NTP correction, DST) as a conservative cache miss rather than trusting a negative duration.

## Quality gates

So this does not regress next month:

- **`cargo deny` in CI.** A daily security audit cron opens an issue when the lockfile gains a new advisory overnight, complementing the blocking PR gate. Two known transitive advisories are whitelisted with written rationale.
- **Criterion baselines** for `blob_compress`, `markdown_append`, and `highlight_code`, the three hot paths above, so a regression shows up as a number.
- **A heaptrack runbook** (`tasks/heaptrack-runbook.md`) with the reproducible procedure behind every RAM claim in this post, including the streaming scenario with 5 agents.

## The bar

"Leaner than the wrapper" is a marketing line until it survives a real workday. The work in v0.3.4 is what turns it into a property you can measure: bounded memory on long sessions, no main thread panic under resource exhaustion, a locked down IPC surface, and a streaming path that does not melt a core.

Paneflow is free and open source: [github.com/arthjean/paneflow](https://github.com/arthjean/paneflow).
