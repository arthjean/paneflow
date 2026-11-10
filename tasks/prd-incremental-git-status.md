[PRD]
# PRD: Incremental git status fed by file-system events

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-07 | Arthur Jean | Initial PRD for Stage C of #134: a long-lived per-worktree status model in `paneflow-git`, fed by a vendored `gix-notify` watcher, that re-reads only what changed and falls back to the full Stage B snapshot on any loss. A freshness contract separates fenced answers (Linux, macOS, and full statuses on Windows) from converging ones. 5 epics, 18 stories. |
| 1.1 | 2026-11-09 | Arthur Jean | Done: every epic certified, delivered by #135 (rebase-merged as `312029844`), `run_tests` 42631047242 green on every leg at `fe30b96a7`, the same tree. Windows fences through the volume change journal (EP-005), so `is_clean` answers incrementally there too. Gaps against this PRD, accepted: a no-change probe misses its 2 ms (2.16 ms on Linux, 7.4 to 8.8 ms on Windows, where opening the repository and resolving HEAD cost about 3.5 ms); a Windows checkout rewriting 100 files costs the monitor more than a full snapshot (143 to 192 ms against 87 ms), since git leaves the files racily clean and the first status pays their first read; a new ReFS Dev Drive has no change journal, so its fenced probes take the full snapshot until an administrator creates one; peak heap was counted by the bench's allocator because heaptrack 1.5.0 aborts on this WSL. |
| 1.2 | 2026-11-10 | Arthur Jean | Withdrawn together with `tasks/prd-in-process-git-status.md`: the monitor, the change feed and the Windows change-journal fence were removed from `main` before any release, and the background probes run the git CLI again. The code stays reachable at the tag `archive/gix-engine`; this document is kept as the record of what was built and measured. |

## Problem Statement

**Reference convention.** Paths refer to Paneflow at `7b248b199` plus Stages A and B of #134 (thread budget, shared listing pool, unix `fstatat` listing, cache-tree staged differ, `index.skipHash`), still uncommitted on `fix/134-shared-directory-enumeration` when this PRD was written; Stage C starts once they land. Measurements are release builds on one 16-thread machine, Windows 11 26300 and WSL2 Linux 6.18, taken on 2026-11-07 with a harness that is local evidence in a session scratchpad, not tracked by git; the published numbers are in the "In-process git status" section of `ARCHITECTURE.md`.

1. **Every probe pays for the whole worktree to report a one-file change.** A rust-lang/rust status (64,264 index entries, 4,827 tracked directories) costs 48 to 50 ms wall and 218 to 230 ms CPU on Linux, and 74 to 75 ms wall and 334 to 340 ms CPU on Windows, whatever changed. An agent that keeps writing keeps the Files sidebar probing at its debounce cap (40 ms debounce, 160 ms cap, `src-app/src/app/files_sidebar/worker.rs:12-13`), which on that repository is up to about 4 full statuses a second, or about 1.3 s of CPU per second on Windows.
2. **Consumers multiply the cost.** The Files sidebar, the diff badge and the diff dock each run their own full snapshot, and one dock build runs two (`src-app/src/app/diff_dock/git.rs:53` and `:92`). Eight probes at once in one process take 373 to 375 ms each on Windows and 219 ms each on Linux, where the git CLI takes 165 ms.
3. **The diff badge misses worktree edits for up to 30 seconds.** Its watcher reacts only to `HEAD` and `index` (`src-app/src/app/workspace_ops/git_watch.rs:73-74`), so a plain edit reaches it through the 30 s poll of shown workspaces (`git_watch.rs:10`), and a hidden workspace waits up to 5 minutes (`git_watch.rs:12`). Making the badge follow worktree events today would add one full status per event burst.
4. **No existing watcher can vouch for completeness.** All five use notify 7 non-recursively, and the app git watcher only logs errors (`git_watch.rs:94-97`). notify 7's Windows backend ignores the completion byte count (`notify-7.0.0/src/windows.rs:312-330`, 16 KiB buffer at `:39`), so a `ReadDirectoryChangesW` overflow is lost without a signal. A model that trusted the absence of events on that stream would show a stale status and never know it.

**Why now:** Stages A and B took the per-probe design as far as it goes. Windows is at the CLI's CPU, and the Linux remainder is inside gix (`std::path::Components` in gix-fs about 10 %, `entry_by_path` about 4.6 %). The next order of magnitude requires not re-reading what did not change. On 2026-11-03 gitoxide's maintainer opened [GitoxideLabs/gitoxide#3153](https://github.com/GitoxideLabs/gitoxide/pull/3153) (draft) with `gix-notify`, a bounded event crate built for long-running applications. Its only gitoxide dependencies, `gix-error` 0.5 and `gix-parallel` 0.50, already sit in Paneflow's `Cargo.lock` through gix 0.90, so Paneflow can build on the upstream direction now instead of writing a third watcher of its own.

## Overview

`crates/paneflow-git` gains a `Monitor`: one per worktree, long-lived, owned by a background thread, and holding no gix `Repository`, only derived data. That data is the baseline snapshot, each tracked entry's worktree verdict, the untracked set, the directory listings, and the identities of every input that is not a worktree file (index stat and checksum, resolved HEAD, ignore and attribute sources, config files). A vendored `gix-notify` watcher feeds it. A probe drains the events. If the watcher reports any loss, or an input changed in a way the model cannot carry, the probe takes the full Stage B snapshot and re-baselines. Otherwise it re-evaluates only the dirty paths and directories through gix-status limited to them, opening the repository fresh as every probe does today. It returns the same `Snapshot` type, so the porcelain mappers and the UI do not change.

Every probe declares a freshness. `Fenced` must reflect every write that completed before the probe started. On Linux and macOS the monitor gets that from a `gix-notify` delivery fence through a marker directory under Paneflow's own cache, never under `.git` or the worktree. Windows has no fence, so a fenced probe takes the full snapshot there; `is_clean` before a worktree removal is fenced. `Converging` probes (sidebar, badge, dock) answer incrementally without a fence, and every event delivered after them schedules another probe, so what the user sees converges within the measured delivery latency. A P2 epic tests whether the NTFS USN journal can serve as a Windows fence without elevation.

Key decisions, each with its evidence:
- **`gix-notify`, vendored at the PR head (`1077036b4`), not notify 7 and not a watcher of our own.** notify 7 loses Windows overflows silently (problem 4). `gix-notify` reports `Loss::{WatchSetChanged, Rescan, Overflow, BackendError, BackendStopped}` with a generation and a sequence, bounds its queue at 4,096 events or 4 MiB, fences on Linux and macOS, and checks root identity on Windows (`gix-notify/src/lib.rs` at that commit). Its author did not test Windows locally, so a spike measures loss reporting on all three platforms before anything trusts it.
- **No git fsmonitor.** The engine neutralizes `core.fsmonitor` and never writes the index (`ARCHITECTURE.md:189-191`, `:280-281`). gitoxide's fsmonitor client writes the FSMN extension back through `Outcome::write_changes` and needs a gix newer than the pinned `=0.90.0`.
- **Re-evaluate through gix-status, not a second status implementation.** Parity with `snapshot()` then comes from the same code. `into_index_worktree_iter` already takes pathspec patterns (`gix-0.90.0/src/status/index_worktree.rs:542-544`), and a spike measures whether that costs what the dirty paths cost.
- **Carry the model across index rewrites.** Agents run `git status`, `git add` and `git commit` constantly. Re-baselining on every index write would turn most probes back into full statuses, and an entry whose id, mode, flags and stage did not change keeps its worktree verdict.
- **No repository-size threshold.** One code path everywhere, so the differential test covers what every user runs.
- **Verify periodically and test differentially.** The `gix-notify` docs recommend periodic verification and state that inotify misses memory-mapped writes. Zed shipped stale-status regressions repeatedly: `.git` events missed, a rescan feedback loop from `.git/lfs` events, slow background results dropped. So a seeded random test drives real event sequences, `.git` changes included, on all three CI legs.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| A one-file change costs what one file costs | Incremental probe p50 at or below 5 ms on rust-lang/rust on Linux (from 48 to 50 ms) | Same on macOS, and at or below 10 ms on Windows (from 74 to 75 ms) |
| Statuses stay exactly right | 0 differences in the differential test on the Linux, macOS and Windows legs | 0 drift reports from periodic verification over a month of daily use |
| The badge follows the worktree | A tracked-file edit reaches the badge of a shown workspace within 1 s (from up to 30 s) | Same with fenced probes on Windows if the USN journal spike holds |
| An idle worktree costs nothing | 0 full statuses per minute on an idle visible worktree, apart from 1 verification every 5 minutes (from 2 a minute through the 30 s poll) | Same with 30 workspaces open |

## Target Users

### Developer running coding agents on a large repository
- **Role:** a developer on Linux, macOS or Windows who lets one or more agents edit a repository of tens of thousands of files, with the Files sidebar and the diff badge visible.
- **Behaviors:** agents write files, run `git status`, `git add` and `git commit` many times a minute; the developer reviews in the diff dock.
- **Pain points:** each burst of agent writes triggers a full status (48 to 75 ms, 218 to 340 ms CPU on rust-lang/rust), several consumers repeat it, and fans spin on a laptop while nothing visible changes.
- **Current workaround:** closing the Files sidebar, which removes the dominant source of probes and the view they feed.
- **Success looks like:** the sidebar and the badge update within a debounce window of every edit, at a cost proportional to the edit.

### Paneflow user with many workspaces
- **Role:** a user with 5 to 30 workspaces open, agents running in several of them.
- **Behaviors:** glances at the badges to see which workspace has changes, switches to review.
- **Pain points:** a badge can be 30 s behind a shown workspace and 5 minutes behind a hidden one, so it cannot be trusted as a live signal.
- **Current workaround:** opening the workspace and waiting, or running `git status` in a pane.
- **Success looks like:** the badge of a shown workspace moves within a second of an edit.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- **Git fsmonitor** ([githooks](https://git-scm.com/docs/githooks), [git-fsmonitor--daemon](https://git-scm.com/docs/git-fsmonitor--daemon)): the hook protocol v2 returns an opaque token and every possibly changed path, and `/` or a non-zero exit means "everything changed". The builtin daemon synchronizes with cookie files under `.git`, refuses network mounts unless `fsmonitor.allowRemote` is set, reports submodule changes against the parent, and on Linux is limited by inotify's per-user watch count. Paneflow cannot use it: it writes under `.git`, it is a program the repository config names, and it needs the user to enable it.
- **gitoxide [#3153](https://github.com/GitoxideLabs/gitoxide/pull/3153)** (draft, 133 files, opened 2026-11-03): `gix-notify` (owned FSEvents, inotify and `ReadDirectoryChangesW` backends, a bounded queue, loss with generation and sequence, fences on macOS and Linux only), `gix-fsmonitor` (hook v2 and git's daemon client, FSMN writeback), `gix-fsmonitor-daemon`, and cached listings in `gix-dir`. Paneflow takes `gix-notify` alone, which has no git knowledge and no repository dependency.
- **Zed**: stale status is a recurring regression class ([#2670](https://git.secluded.site/zed/commit/f83514cde48f3f7d710bf66a0597187eddf112ad) missed `.git` changes, [#25927](https://git.secluded.site/zed/commit/95446195aff0b9b00dcb5e8ce0af4a3869e26da9) a rescan feedback loop from `.git/lfs` events, [#24722](https://git.secluded.site/zed/commit/744579ede9fdcee44e6b44e8c4d6267829dd588b) slow results that never reached the UI, issue [#27167](https://github.com/zed-industries/zed/issues/27167) a stale panel after moves and `git restore`).
- **notify 7** (Paneflow's watcher today): emits `Flag::Rescan` on inotify `IN_Q_OVERFLOW` (`notify-7.0.0/src/inotify.rs:210-211`) and on FSEvents `MustScanSubDirs` (`fsevent.rs:114-115`), maps inotify `ENOSPC` to `ErrorKind::MaxFilesWatch` (`inotify.rs:442-444`), and drops a Windows overflow silently.
- **Market gap:** an in-process status that is incremental, exact on Linux and macOS, never writes to the repository, and needs no daemon the user has to enable.

### Best Practices Applied
- Install watches before the baseline scan and keep the events received while scanning (`gix-notify` crate docs).
- Treat events as paths to reconcile, never as operations to replay: they may be duplicated, coalesced, or name paths that no longer exist (`gix-notify` crate docs).
- Any loss means a full scan, as git's hook protocol treats `/` and a failed hook.
- Verify state periodically, since some writes produce no event (`gix-notify` crate docs: memory-mapped writes, unobserved hard links, overlay layers).
- Refuse incremental status on network file systems, as git's daemon does.

*Full research sources available in project documentation.*

## Assumptions & Constraints

### Assumptions (to validate)
- `gix-notify` at `1077036b4` reports a loss for every overflow on all three platforms, including a Windows 64 KiB buffer overflow, based on its documented `Loss` contract; its author did not test Windows locally (US-001).
- A Linux fence works with the marker directory under `~/.paneflow` on a different file system from the worktree, as long as the same inotify instance covers both, based on the `synchronize_at` contract ("covered by this same watcher's single native stream") (US-001).
- `ReadDirectoryChangesW` delivery latency on Windows NTFS stays at or below 50 ms at p99 under a parallel build, which bounds how far a converging status lags; no source measured it (US-001).
- A gix status limited by pathspec to 10 dirty paths costs at most 3 ms on rust-lang/rust on Linux, based on gix accepting patterns in `into_index_worktree_iter`, with no measurement yet (US-005).
- A monitor for rust-lang/rust fits in 32 MiB of peak heap above the full snapshot, based on the 19.5 MiB peak of a full status (`ARCHITECTURE.md`, 2026-11-05) (US-016).
- An unprivileged process can read the NTFS USN journal well enough to fence, which is unverified (US-017).
- Linux watch budget: rust-lang/rust needs about 4,828 watches, against a default `max_user_watches` of 524,288 on WSL 6.18 and 8,192 on kernels before 5.11.

### Hard Constraints
- The engine never writes the index, never creates anything under `.git` or in the worktree, and never runs `core.fsmonitor`, a hook, or any program the repository config declares (`ARCHITECTURE.md:189-191`, `:280-281`).
- No gix `Repository`, `ThreadSafeRepository` or object-database handle lives between probes (`ARCHITECTURE.md:178-181`, `snapshot.rs:31-35`): a cached repository once failed to find objects after `git repack -a -d`.
- The GPUI main thread never blocks: monitors run on the files-tree thread, `smol::unblock`, or their own threads.
- Linux (Wayland and X11), macOS (Intel and Apple Silicon) and Windows 10/11 all work; a platform without a guarantee degrades to full statuses, never to a stale one.
- The CLI fallback stays, and an overflowing dock listing never shows a lower bound (`ARCHITECTURE.md:342-359`).
- Per-user state goes through `crates/paneflow-home` (`cache_dir()`), never `dirs::cache_dir`.
- No comments in Rust, shell or PowerShell sources; vendored crates keep their upstream text.
- Unit tests assert outcomes under generous deadlines; latency budgets live in the perf gates.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, mandatory before every commit and push
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints every target, test modules included
- `cargo nextest run --workspace --locked --profile ci` - the CI test runner, one process per test, 300 s timeout
- `bash scripts/check-test-wallclock.sh` - when a test changes: no new wall-clock budget in test code
- `cargo deny check advisories licenses sources` - when dependencies change (US-002, and any story that adds a crate)

For stories with `#[cfg(windows)]` code, additional gate:
- `cargo nextest run --workspace --locked --profile ci` run natively on Windows before push, since only the Windows CI leg compiles that code

No visual gate: no story changes what the UI draws.

## Epics & User Stories

### EP-001: A change feed that reports every loss on Linux, macOS and Windows

A vendored `gix-notify` watcher covers one worktree and its git directories, refuses file systems it cannot vouch for, and lives only while probes need it.

**Definition of Done:** injected overflow, root replacement and backend stop each surface as a loss on the Linux, macOS and Windows CI legs; a fence certifies a preceding write on Linux and macOS; a feed releases its watches and handles when it drops; an ineligible file system never gets a feed.

#### US-001: Validate assumption: gix-notify reports every loss and fences on Linux and macOS
**Description:** As the maintainer, I want `gix-notify`'s loss and fence guarantees measured on all three platforms before any status trusts the absence of an event, so that a stale status can only come from a bug a test can catch.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `gix-notify` at `1077036b4` built against Paneflow's lockfile (`gix-error` 0.5.2, `gix-parallel` 0.50.1), when a throwaway harness creates 20,000 files in one watched directory without draining until the last one is written, then every one of 10 runs per platform reports `Loss::Overflow` or `Loss::Rescan`, never a silent gap, on Linux, macOS and Windows
- [x] Given a watched worktree root, when it is renamed away and replaced by another directory of the same name, then a loss is reported before any event from the new directory is published
- [x] Given a marker directory on a different file system from the worktree (tmpfs against ext4 on Linux), when a write completes and `synchronize_at` returns a fence, then that write's event is drained at or before the fence sequence in 1,000 of 1,000 trials on Linux and on macOS
- [x] Given Windows 11 NTFS under a parallel `cargo build`, when 1,000 single-file writes are timed from the write's return to the event's publication, then p50 and p99 delivery latency are recorded on #134, and US-011 uses that p99 as the converging bound
- [x] Given any of these checks fails on a platform, then the failing case is recorded, a fix in the vendored copy is drafted with a red-then-green test before EP-002 relies on it, and if no fix holds, that platform stays on full statuses (`Ineligible`)
- [x] Given the macOS runs, then they go through a throwaway `probe/*` branch with its own workflow, only after Arthur approves it

#### US-002: Vendor gix-notify as a pinned crate copy
**Description:** As the maintainer, I want `gix-notify` vendored under `vendor/gix-notify` at a pinned upstream commit, so that the change feed builds on gix 0.90 today and the copy drops once a release Paneflow can use exists.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given `vendor/gix-notify` copied from GitoxideLabs/gitoxide@`1077036b4`, when the workspace resolves, then `paneflow-git` links it with the `gix-error` and `gix-parallel` versions already in `Cargo.lock`, and no second copy of either appears
- [x] Given the upstream manifest's `notify` 8 dependency for targets other than Linux, macOS and Windows, then the copy drops it, and the header of `vendor/gix-notify/Cargo.toml` states that change, the upstream commit, and the condition to drop the copy
- [x] Given `crates/paneflow-git/tests/lockfile.rs`, when `gix-notify` reaches `paneflow-shim`, `paneflow-ai-hook` or `paneflow-mcp`, or the vendored version drifts from the pin, then the test fails
- [x] Given a Paneflow change to the vendored sources (from US-001), then it carries a test, and the upstream text stays as it is
- [x] Given the new transitive crates (`inotify` 0.11 on Linux, `dispatch2` on macOS), then no advisory, license or source exception is added for them, or the story stops and names the blocking advisory

#### US-003: A per-worktree change feed with an eligibility check
**Description:** As the maintainer, I want a `ChangeFeed` in `paneflow-git` that covers one worktree and its git directories and refuses file systems whose events it cannot vouch for, so that the model only runs where events are complete.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [x] Given a worktree on Linux, when the feed starts, then it registers one non-recursive watch per directory that holds a tracked entry or untracked non-ignored content, plus the git dir and, for a linked worktree, the common dir, and no watch inside an ignored directory that holds no tracked entry
- [x] Given a worktree on macOS or Windows, when the feed starts, then it registers one recursive watch on the worktree root, plus the git dir when it lies outside the root, and drops events under ignored directories that hold no tracked entry without queuing work
- [x] Given a worktree on NFS, SMB, a FUSE mount, WSL's `/mnt/<drive>` seen from Linux, or a `\\wsl$` or `\\wsl.localhost` path seen from Windows, when a feed is requested, then it returns `Ineligible` with that reason, registers nothing, and probes stay on full statuses
- [x] Given the fence marker directory, then it is created through `paneflow_home` with private permissions under the Paneflow cache directory, never under `.git` or the worktree, and on Linux or macOS a failure to create it makes the feed `Ineligible`
- [x] Given a registration failure (inotify `ENOSPC` past `max_user_watches`, access denied), when the feed starts, then no partial watch set stays installed and the feed reports `Ineligible` with the cause in one log line, not one per probe
- [x] Given Linux, when a new feed would bring Paneflow's watches above 50 % of `max_user_watches`, then the feed is refused as `Ineligible`, so Paneflow never starves editors and other watchers on the same account

#### US-004: Feed lifecycle bounds memory, watches and handles
**Description:** As a user with many worktrees, I want a feed to exist only while probes need it and to release everything when it goes, so that Paneflow holds no watches, handles or queued events for worktrees nobody is looking at.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Given a feed that served no probe for 5 minutes, when the eviction tick runs, then the feed drops, its watches and handles are released, and the next probe starts a new baseline
- [x] Given Windows and a watched worktree, when the user deletes or renames the worktree directory, then the operation succeeds while the feed is alive, and the feed reports a loss
- [x] Given an event storm beyond the queue bounds (4,096 events or 4 MiB), when the probe drains, then it sees `Loss::Overflow`, and the feed's queued memory stays within those bounds plus the native buffers
- [x] Given the app exits, then no feed thread outlives the process, and the worktree can be deleted right after exit
- [x] Given any feed operation, then none runs on the GPUI main thread; a probe that waits on a feed blocks only its own worker thread

---

### EP-002: An incremental snapshot identical to the full one

`Monitor::snapshot` answers from the dirty set when the feed vouches for completeness and from the full Stage B snapshot otherwise, with the same result either way.

**Definition of Done:** after every step of the differential test on the Linux, macOS and Windows legs, the monitor's snapshot equals `snapshot()` field by field; every loss and every uncarried input change re-baselines; a one-file edit re-checks one entry.

#### US-005: Validate assumption: a status limited to the dirty paths costs what the dirty paths cost
**Description:** As the maintainer, I want the cost of a pathspec-limited gix status measured on rust-lang/rust, so that the incremental pass reuses gix-status, or knows exactly which entry point the vendored copy needs.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given rust-lang/rust and `into_index_worktree_iter` with 1, 10, 100 and 1,000 literal paths, when each runs 40 times in release, then p50 wall time and CPU on Linux and Windows are recorded on #134 next to the full status (48 to 50 ms Linux, 74 to 75 ms Windows)
- [x] Given 10 dirty paths, when the limited status costs more than 3 ms p50 on Linux or 5 ms on Windows, then the story specifies an entry-subset entry point for the vendored gix-status (the index entries to check and the directories to walk) and measures a prototype of it
- [x] Given a limited status, then the directory-read counter used by `crates/paneflow-git/tests/directory_listing.rs` shows that only the directories of the listed paths were read
- [x] Given paths containing pathspec magic (`*`, `?`, `[`, a leading `:` or `!`), then the limited status treats them as literal paths, and the escaping rule is recorded

#### US-006: Baseline and input identities
**Description:** As the maintainer, I want the model to start from a full snapshot taken after the watches are installed, and to fingerprint every input that is not a worktree file, so that it never carries a verdict across a change no event reports.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003, US-005

**Acceptance Criteria:**
- [x] Given a new feed, when the first probe runs, then watches are registered first and the full snapshot runs second, and an event received during the scan is applied after it, so a write in the middle of the scan shows in the next probe
- [x] Given the identities recorded at baseline (index file stat and trailing checksum; resolved HEAD and compared base; every `.gitignore` the walk read, `.git/info/exclude` and `core.excludesFile`; every `.gitattributes` and `info/attributes`; the repository, worktree, global and system config files; `.gitmodules`), when any of them changes without an event, then the next probe detects it by stat and re-baselines, except the index and HEAD, which US-009 carries
- [x] Given the model between probes, then it holds no gix `Repository`, `ThreadSafeRepository` or object-database handle, and each probe opens the repository fresh as `snapshot()` does
- [x] Given an interrupted probe (`Options::interrupt`), when it returns `Error::Interrupted`, then the model is unchanged and the next probe re-evaluates the same dirty set
- [x] Given a probe whose `Options::base` differs from the previous probe's, then only the staged half is recomputed through `staged.rs`, and the worktree half is shared

#### US-007: Incremental tracked pass
**Description:** As a developer whose agent edits a few files, I want a probe to re-check only the tracked paths that events named, so that the Files sidebar costs milliseconds on a 64,000-entry repository.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [x] Given a baseline and an edit to one tracked file, when the next probe runs, then only that entry is stat'ed (and hashed when its stat differs), every other entry keeps its verdict, and the result equals `snapshot()`
- [x] Given a tracked file deleted, replaced by a directory, replaced by a symlink, or made executable, when the next probe runs, then its change kind equals the full snapshot's
- [x] Given a directory created, removed or renamed above tracked entries, when the next probe runs, then every tracked entry under both rename endpoints is re-checked
- [x] Given a case-only rename on a case-insensitive file system (APFS default, NTFS), then both spellings are re-checked and the result equals the full snapshot's
- [x] Given an event for a path the index does not hold, then the tracked pass skips it and hands it to the untracked pass
- [x] Given a tracked file that vanishes between its event and its stat, then the probe reports it deleted as the full snapshot would, never an error
- [x] Given a dirty path inside a submodule, then that submodule is re-probed with the same `Submodules` option as the full snapshot uses

#### US-008: Incremental untracked pass
**Description:** As a developer, I want untracked files to appear and disappear from a walk of the touched directories only, so that untracked listings stay exact without walking the tree.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [x] Given a new untracked file in a tracked directory, when the next probe runs, then only that directory is re-listed, and the file appears in the order the full snapshot gives it
- [x] Given a new untracked directory tree (`a/b/c/f`), when the next probe runs, then its subtree is walked and each file is listed one by one, as the full snapshot lists them
- [x] Given an untracked file deleted or an untracked directory removed, then its entries disappear
- [x] Given more untracked entries than `entry_cap`, then `truncated` is set exactly as the full snapshot sets it, and no lower bound is ever reported as a total
- [x] Given a change to any ignore source, then the probe re-baselines instead of re-walking
- [x] Given an event inside an ignored directory that holds no tracked entry, then no directory is re-listed

#### US-009: Carry the model across index and HEAD rewrites
**Description:** As a developer whose agent runs `git status`, `git add` and `git commit` all the time, I want an index rewrite to re-check only the entries it changed, so that agent git commands do not turn every probe back into a full status.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007, US-008

**Acceptance Criteria:**
- [x] Given `git status` rewriting the index with refreshed stat data and unchanged ids, when the next probe runs, then the index is read once, no worktree file is hashed, and the result equals `snapshot()`
- [x] Given `git add` of 3 files, when the next probe runs, then exactly the entries whose id, mode, flags or stage changed are re-evaluated, and the staged half is recomputed through `staged.rs`
- [x] Given `git commit` (HEAD moves and the index is rewritten), then the staged half is recomputed against the new HEAD tree, worktree verdicts carry over, and the result equals `snapshot()`
- [x] Given `git checkout` of another branch, when more than 10 % of the index entries changed or the feed reported a loss, then the probe re-baselines instead of splicing
- [x] Given a change of index format (split index, sparse index) or of skip-worktree flags, then the probe re-baselines
- [x] Given a conflicted merge (stages 1 to 3), then the conflict kinds equal the full snapshot's

#### US-010: Differential test against the full snapshot on every leg
**Description:** As the maintainer, I want a seeded random test that applies real operations between probes and compares the monitor's snapshot with a fresh full one after each step, so that a stale-status bug fails CI on the platform that has it.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-007, US-008

**Acceptance Criteria:**
- [x] Given 24 seeded scenarios of 12 operations drawn from: edit, same-size rewrite, delete, create file, create nested directory, rename file, rename directory, case-only rename, chmod, symlink swap, `.gitignore` edit, `git add`, `git rm --cached`, `git commit`, `git checkout` of another branch, submodule edit, and write inside an ignored directory, when each step is followed by a fenced probe (Linux, macOS), then the monitor's snapshot equals `snapshot()` field by field
- [x] Given Windows, when each step is followed by converging probes, then the monitor's snapshot equals `snapshot()` within a deadline of 10 s, and the test fails if it does not
- [x] Given an injected loss (a forced overflow, a replaced root), then the next probe re-baselines and still equals `snapshot()`
- [x] Given a same-size rewrite within the second of the index's mtime (a racy entry), then the monitor's verdict equals the full snapshot's
- [x] Given a mismatch, then the failure names the seed, the step, the operation and the differing paths
- [x] Given the PR's CI run, then its JUnit reports show the test ran on the Linux, macOS and Windows legs

---

### EP-003: Consumers on one model per worktree, with a freshness contract

The Files sidebar, the diff badge, the diff dock and `is_clean` probe through one monitor per worktree, and each declares the freshness it needs.

**Definition of Done:** all four consumers share one monitor per worktree; `is_clean` answers only from a fenced or a full status; a tracked-file edit reaches the badge of a shown workspace within 1 s; the race harness still reports 0 failed agent writes on Windows.

#### US-011: Freshness contract: Fenced or Converging
**Description:** As the maintainer, I want every probe to declare whether it needs every write completed before it started or only a status that converges, so that a destructive decision never rests on an unfenced incremental answer.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [x] Given `Freshness::Fenced` on Linux or macOS, when the probe runs, then it fences through the marker directory before draining and answers incrementally, and a fence timeout of 500 ms or `CoverageLost` makes it take the full snapshot
- [x] Given `Freshness::Fenced` on Windows, then the probe takes the full snapshot (until EP-005)
- [x] Given `Freshness::Converging`, when the probe runs, then it drains without fencing and answers incrementally, and every event delivered after it wakes the consumer through the feed's waker
- [x] Given a converging consumer and a write whose event arrives after its probe, then the consumer shows the change within the p99 delivery latency measured in US-001 plus its own debounce
- [x] Given `PANEFLOW_GIT_MONITOR=off`, then every probe takes the full snapshot and no feed starts

#### US-012: One monitor per worktree, and the Files sidebar on it
**Description:** As a developer, I want the Files sidebar to read its status from the worktree's monitor, so that an agent's one-file edit costs milliseconds instead of a full scan.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-004, US-010, US-011

**Acceptance Criteria:**
- [x] Given a registry in `src-app` keyed by the canonical worktree root, when the sidebar, the badge and the dock probe the same worktree, then they share one monitor and one feed
- [x] Given the files worker's dirty mark (`worker.rs:238`) and its 40 to 160 ms debounce, when an agent edits a tracked file, then the sidebar probes through the monitor with `Converging` and returns the same `GitStatuses` as before
- [x] Given an ineligible feed or a monitor error, then the sidebar falls back to `snapshot()` and then to the CLI exactly as today, and the engine counters record the reason under a new `Fallback` variant
- [x] Given the GPUI main thread, then no monitor call runs on it
- [x] Given the race harness (`src-app/src/git_race.rs`), then the sidebar scenarios still report 0 failed truncating writes on the Windows leg

#### US-013: Diff badge, diff dock and is_clean on the monitor
**Description:** As a user with several workspaces, I want the diff badge to follow worktree edits and the dock to build from one status, so that the badge is never 30 s behind a shown workspace and the dock never computes the same status twice.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] Given a shown workspace and an edit to a tracked file, when the feed's waker fires, then the badge re-probes through the monitor within its existing debounce (300 ms, capped at 2 s) instead of waiting for the 30 s poll
- [x] Given a dock build, then `compute_head_diff` and the dock's stats read one monitor snapshot instead of two
- [x] Given `is_clean` before a worktree removal, then it probes with `Fenced` through the monitor, and any error still refuses the removal
- [x] Given a hidden workspace whose monitor was evicted, then its badge keeps the 5-minute hidden poll and starts no feed
- [x] Given the public shapes `GitStatuses`, `GitDiffStats`, `WorktreeDiff`, `FileChange`, `GitSummary` and `TrackedSummary`, then none of them changes

---

### EP-004: Self-verification, degraded modes and evidence of the gain

The monitor detects and repairs drift on its own, counts every fallback, turns a failing watcher into lost speed rather than a stale status, and its gain is measured and guarded.

**Definition of Done:** a write that produced no event is repaired by the next verification; every fallback reason is counted; a backend failure recovers with backoff; a probe's own reads never schedule another probe; `ARCHITECTURE.md` has a dated "Incremental status" bullet with measured wall time, CPU, memory and watch counts on Linux and Windows, and the perf gates fail on a regression of the one-edit probe.

#### US-014: Periodic verification and drift reports
**Description:** As the maintainer, I want the model compared with a full snapshot at a bounded rate, so that a change the OS never reported is repaired within minutes and reported.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] Given a visible worktree with a live monitor, when 5 minutes or 500 incremental probes have passed since the last full snapshot, then the next probe takes a full snapshot and compares it with the model
- [x] Given a difference, then the probe returns the full snapshot, re-baselines, logs one `warn` line with the number of differing paths and their change kinds (never file contents), and increments a drift counter
- [x] Given a tracked file rewritten through a memory-mapped write on Linux, which inotify does not report, then the next verification repairs the status
- [x] Given a hidden worktree, then no verification runs

#### US-015: Degraded modes and recovery
**Description:** As a user, I want the status to stay exact when the watcher fails, so that a lost event stream costs speed and never correctness.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] Given `Loss::BackendError` or `Loss::BackendStopped`, then the probe answers from the full snapshot and the feed retries registration with exponential backoff from 1 s to 5 minutes
- [x] Given more than 60 s of wall-clock time between two drains, as after a system sleep, then the next probe re-baselines
- [x] Given each fallback reason (each loss kind, each ineligibility, an input identity change, the index carry limit, a verification mismatch), then the engine counters read by `crates/paneflow-host/src/work_counters.rs` record it
- [x] Given an idle worktree probed 100 times in a row, then the probes' own reads produce no event that schedules another probe
- [x] Given repeated overflows from a busy directory that is not ignored, then full snapshots for that worktree run at most once per consumer debounce window

#### US-016: Benchmarks, memory and a perf gate
**Description:** As the maintainer, I want the incremental status measured against the full snapshot and the CLI, and its one-edit latency guarded on the calibrated runner, so that the gain is evidence and stays.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-012

**Acceptance Criteria:**
- [x] Given rust-lang/rust, t3code and the Paneflow checkout, when the bench runs a no-change probe, one edit, 100 edits, `git add`, `git commit`, a branch checkout and 8 concurrent consumers, then p50 and p95 wall time and CPU on Linux and Windows are recorded against the full snapshot and the CLI
- [x] Given a monitor on rust-lang/rust, then its peak heap measured by heaptrack on Linux is recorded, with a target of at most 32 MiB above the full snapshot's peak
- [x] Given the perf gates (`scripts/perf-gates.sh`, budgets in `crates/paneflow-host/tests/persistent_baseline/gates.rs`), then a one-edit incremental probe budget is added for the calibrated Linux runner, written by a test through an output variable as `PANEFLOW_GATE_HOST_STATUS_OUT` is
- [x] Given a regression past that budget, then the gate fails and prints the measured and budgeted values
- [x] Given Linux, then the inotify watch count of a rust-lang/rust monitor is recorded (expected about 4,828)

---

### EP-005: A fenced incremental status on Windows

Windows gets the fenced incremental answers Linux and macOS have, through the NTFS USN journal, if a non-elevated process can read it.

**Definition of Done:** either `Fenced` probes answer incrementally on Windows NTFS and ReFS with the differential test green in fenced mode, or the spike shows the journal needs elevation and US-018 is cancelled with the evidence on #134.

#### US-017: Validate assumption: the USN journal fences without elevation
**Description:** As the maintainer, I want to know whether a non-elevated Paneflow can read the NTFS USN journal of a worktree's volume, so that Windows can get a fence without writing under `.git` or the worktree.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given a non-elevated process on Windows 11 NTFS and on a ReFS Dev Drive, when it queries the worktree volume's journal (`FSCTL_QUERY_USN_JOURNAL`, and `FSCTL_READ_UNPRIVILEGED_USN_JOURNAL` where available), then the outcome, the access the volume handle needs, and the behavior with the journal disabled are recorded
- [x] Given a readable journal, when a write returns and the journal's `NextUsn` is read afterwards, then the write's record lies below that USN in 1,000 of 1,000 trials, and the cost of query and read is recorded
- [x] Given that the read requires elevation or that default installs have no journal on the system volume, then the story records it on #134 and US-018 becomes `CANCELLED`

#### US-018: Fenced incremental status on Windows through the USN journal
**Description:** As a Windows user, I want `is_clean` and every fenced probe to answer incrementally, so that Windows stops paying a full status for exact answers.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [x] Given `Freshness::Fenced` on Windows with a readable journal, when the probe runs, then it reads journal records up to the current `NextUsn`, maps them to worktree paths, and answers incrementally
- [x] Given a journal wrap (the model's USN below `LowestValidUsn`), a changed journal id, or a volume without a journal, then the probe re-baselines from the full snapshot
- [x] Given the differential test (US-010) on the Windows leg, then it runs with fenced probes and passes
- [x] Given a journal read error, then the probe takes the full snapshot and the engine counters record the reason

---

## Functional Requirements

- FR-01: `Monitor::snapshot` must return, for the same `Options`, a `Snapshot` equal to `snapshot()` for the worktree state at the probe's start when `Fenced`, and one that converges to it when `Converging`.
- FR-02: When the feed reports any loss, or an input identity changed that the model does not carry, the monitor must answer from the full snapshot and re-baseline.
- FR-03: The monitor must NOT write the index, create anything under `.git` or in the worktree, run `core.fsmonitor`, a hook or any program the repository config declares, or keep a gix `Repository` between probes.
- FR-04: The feed must refuse network, FUSE, 9p and drvfs file systems and WSL UNC paths, and the probe must then use full snapshots.
- FR-05: The fence marker directory must live under the Paneflow cache directory, with private permissions.
- FR-06: On a monitor error, a probe must fall back to `snapshot()` and then to the CLI as today.
- FR-07: No monitor or feed operation may run on the GPUI main thread.
- FR-08: `is_clean` must use `Fenced` and must never answer clean on an error or an ineligible feed.
- FR-09: A feed must drop after 5 minutes without a probe and release its watches and handles.
- FR-10: The monitor must compare itself with a full snapshot at least every 5 minutes or 500 incremental probes while its worktree is visible.
- FR-11: `PANEFLOW_GIT_MONITOR=off` must disable every monitor.

## Non-Functional Requirements

- **Performance:** a one-edit incremental probe on rust-lang/rust at or below 5 ms p50 on Linux and macOS and 10 ms on Windows; a no-change probe at or below 2 ms p50; at most 10 ms CPU per incremental probe; a re-baseline at most as costly as the Stage B snapshot (50 ms Linux, 75 ms Windows); fence timeout 500 ms.
- **Security:** the feed follows no symlink out of the worktree; the marker directory is private to the user (0700 on unix); logs carry path counts and change kinds, never file contents; the monitor spawns no process.
- **Accessibility:** not applicable: 0 UI elements are added or changed.
- **Scalability:** per feed at most 4,096 queued events and 4 MiB of queued paths, plus 64 KiB of native buffer per Windows registration; at most 32 MiB of peak heap per rust-lang/rust monitor; Paneflow's inotify watches stay at or below 50 % of `max_user_watches`; 30 open workspaces cost at most one verification each per 5 minutes of visibility.
- **Reliability:** 0 differences in the differential test on 3 CI legs; 0 drift reports over a month of daily use; feed recovery with backoff from 1 s to 5 minutes; a converging status lags at most the measured p99 delivery latency plus the consumer's debounce.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | First probe of a worktree | No feed yet | Watches first, full snapshot second, events during the scan applied after | None |
| 2 | Event flood | `git checkout` of a distant branch, `npm install` in an unignored directory | Queue overflow is a loss; full snapshot; re-baseline | None |
| 3 | Watch limit reached | inotify `ENOSPC`, or the 50 % budget | Feed `Ineligible`, full snapshots, one log line | None |
| 4 | Network or virtual file system | NFS, SMB, FUSE, `/mnt/c` from WSL, `\\wsl$` from Windows | No feed; full snapshots | None |
| 5 | Worktree deleted or moved | User or agent removes the directory | Loss; the next probe fails as `snapshot()` fails today; feed dropped | Existing "repository not found" handling |
| 6 | Ignore rules change | `.gitignore`, `info/exclude` or `core.excludesFile` edited | Identity change; re-baseline | None |
| 7 | Index rewritten by a git command | `git status`, `git add`, `git commit` in a pane | Carry the unchanged entries; re-baseline past 10 % changed | None |
| 8 | Case-only rename | `mv Foo foo` on APFS or NTFS | Both spellings re-checked | None |
| 9 | Write the OS never reports | Memory-mapped write on Linux, overlay lower layer | Repaired by the next periodic verification; drift counted | None |
| 10 | System sleep | Laptop lid closed for an hour | Over 60 s between drains: re-baseline | None |
| 11 | Probe interrupted | Workspace switched mid-probe | `Error::Interrupted`; model unchanged | None |
| 12 | Converging probe races a write | The write's event lands after the drain | The next event schedules a probe; the view converges within the bound | None |
| 13 | Worktree removal decision | `is_clean` before removing a worktree | Fenced on Linux and macOS, full on Windows; an error refuses the removal | Existing removal refusal |
| 14 | Monitor disabled | `PANEFLOW_GIT_MONITOR=off` | Full snapshots everywhere | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | A stale status ships (the Zed regression class) | High | High | Differential test on 3 legs (US-010), periodic verification (US-014), `is_clean` fenced or full (US-011), kill switch (FR-11) |
| 2 | `gix-notify` misses a Windows overflow (untested upstream on Windows) | Med | High | US-001 before any trust; fix in the vendored copy with a test; otherwise Windows stays on full statuses |
| 3 | `gix-notify`'s draft API changes or the PR never merges | Med | Med | Pinned vendored copy behind a narrow `ChangeFeed` seam; the header states when to drop it |
| 4 | A pathspec-limited status costs O(index), not O(dirty) | Med | Med | US-005 measures first; an entry-subset entry point in the vendored gix-status otherwise |
| 5 | inotify watch exhaustion on old kernels or with many worktrees | Med | Low | 50 % budget, `Ineligible` with full statuses, eviction after 5 minutes |
| 6 | Memory grows with 30 live monitors | Low | Med | Eviction, measured heap budget (US-016) |
| 7 | A probe's own reads feed back into events | Low | Med | Read-only probes; an explicit test (US-015) |
| 8 | Windows converging lag is visible to users | Low | Low | Measured p99 in US-001; USN fence in EP-005 |

## Non-Goals

What this version explicitly does NOT include:

- Git's fsmonitor in any form (hook, builtin daemon, gitoxide's client), and FSMN or UNTR index extensions: they write to the repository or run programs the repository names.
- Persisting the model across app restarts: a cold start is a full status, which Stage B already brought to 48 to 75 ms on rust-lang/rust.
- Incremental status on network, FUSE, 9p or drvfs file systems: their events are incomplete or absent.
- Moving the Files sidebar's tree-listing watcher, the code view, markdown or config watchers onto `gix-notify`: revisit after EP-003 shows the feed's behavior in production.
- A fanotify backend: unprivileged fanotify still needs one mark per directory, which brings nothing over inotify for this use.
- Writing upstream pull requests to gitoxide inside these stories: the text of an upstream PR is Arthur's to write.

## Files NOT to Modify

- `src-app/src/git_command.rs` UserAction path and the probe argument builder: user git actions keep their own path.
- `crates/paneflow-process/**`: process spawning is out of scope.
- `crates/paneflow-shim/**`, `crates/paneflow-ai-hook/**`, `crates/paneflow-mcp/**`: size-capped helpers that must never link a watcher or gix.
- The GPUI `rev` values and the existing `[patch.crates-io]` entries: add the `gix-notify` entry, change none.
- `vendor/portable-pty/**`, `native/libghostty/**`: unrelated engines.
- The public shapes of `GitStatuses`, `GitDiffStats`, `WorktreeDiff`, `FileChange`, `GitSummary` and `TrackedSummary`: the UI must not change.
- `crates/paneflow-config/src/watcher.rs`, `src-app/src/app/diff_dock/code/view/disk_sync.rs`, `src-app/src/markdown/view.rs`: their own watchers are out of scope (Non-Goals).

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** should the monitor live in `paneflow-git` with its registry in `src-app` (recommended: the engine stays testable without GPUI), or in `paneflow-host`, so it survives a window restart at the cost of an IPC path? Engineering to confirm.
- **Data Model:** should per-entry verdicts be a vector aligned with the index entries, re-aligned on each index carry (recommended: one contiguous allocation), or a map keyed by path (splicing without re-alignment, more memory)? Trade-off: alignment work on every index rewrite against about 64 bytes per entry more.
- **Fence placement:** one marker directory per feed under `paneflow_home::cache_dir()`, or one shared by all feeds? US-001 settles whether a marker on another file system fences inotify reliably.
- **Dependencies:** `gix-notify` (vendored, pulls `inotify` 0.11 on Linux and `dispatch2` on macOS) next to notify 7, which the other watchers keep. Should the Files worker move to `gix-notify` later to drop notify 7's silent Windows overflow there too?
- **Migration:** none; `PANEFLOW_GIT_MONITOR=off` is the rollback. Should it be honored in release builds (recommended: yes, like `PANEFLOW_GIT_ENGINE=cli`)?

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| One-edit probe p50, rust-lang/rust, Linux | 48 to 50 ms (full snapshot) | 5 ms or less | Month-1 | Bench of US-016, release build |
| One-edit probe p50, rust-lang/rust, Windows | 74 to 75 ms | 10 ms or less | Month-6 | Bench of US-016 |
| CPU per one-edit probe, rust-lang/rust | 218 to 340 ms | 10 ms or less | Month-1 | Bench of US-016 |
| Differential test differences | N/A (new) | 0 on 3 legs | Month-1 | CI JUnit reports |
| Drift reports | N/A (new) | 0 | Month-6 | Drift counter and `warn` lines |
| Badge lag after a tracked edit, shown workspace | Up to 30 s | 1 s or less | Month-1 | Manual check and an outcome test with a deadline |
| Full statuses per minute, idle visible worktree | 2 (30 s poll) | 0.2 (1 verification per 5 minutes) | Month-1 | Engine counters |
| Peak heap of a rust-lang/rust monitor above the full snapshot | N/A (new) | 32 MiB or less | Month-1 | heaptrack on Linux |

## Open Questions

- Is vendoring a draft upstream crate acceptable, rather than waiting for a `gix-notify` release? Arthur, before US-002; US-002 to US-018 depend on it.
- Any fix found in US-001 is worth sending upstream to #3153; the PR text is Arthur's to write. Arthur, after US-001.
- macOS validation needs a throwaway `probe/*` branch with a `macos-15` workflow. Arthur to approve when US-001 starts.
- Are 5 minutes or 500 probes the right verification cadence, given about 220 to 340 ms of CPU per verification on rust-lang/rust? Arthur, before US-014.
- Should a hidden workspace running an agent keep its monitor, so its badge follows too? Decide after the US-016 memory numbers.
- Stage C should start on its own branch once Stages A and B of #134 are committed and merged. Arthur to confirm the order.
[/PRD]
