[PRD]
# PRD: In-process git status for the background probes

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-11-05 | Arthur Jean | Initial PRD from the #130 spike and diagnosis: a read-only gix engine with a vendored short-read fix, a parity oracle against the git CLI, then the Files sidebar (P0), the diff badge (P1), the diff dock and the removal snapshot (P2) move in process behind a CLI fallback. 5 epics, 15 stories. |

## Problem Statement

**Reference convention.** Paths refer to Paneflow at commit `236bf399f` (`origin/main` on 2026-11-05). The race harness used for the diagnosis (`probe-race`) is local evidence in a session scratchpad, not tracked by git, so another agent cannot read it. The published evidence is the spike comment and the diagnosis comment on [#130](https://github.com/arthjean/paneflow/issues/130#issuecomment-6657657624). All measurements below are from Windows 11 26300 with Git 2.53.0.windows.1 unless stated otherwise.

1. **The Files sidebar makes agent edits fail on Windows.** `files_git::read` (`src-app/src/app/files_git.rs:132`) runs `git status`, which re-hashes every tracked file whose size matches its index entry but whose stat changed. Above 32 KiB, Git for Windows hashes through a memory-mapped view (`index_core`, `SMALL_FILE_SIZE`), and while that view is open a truncating write fails with `ERROR_USER_MAPPED_FILE` (1224). Under a writer that rewrites a 480 KiB file at constant size, 136 to 325 writes failed per 40 probes, red in 5 runs out of 5. With `GIT_OPTIONAL_LOCKS=0` the refreshed stat is never written back, so the same file is mapped again on every probe. The sidebar refreshes on every burst of file events (40 ms debounce, `src-app/src/app/files_sidebar/worker.rs:12-13`), so the agent's own writes trigger the probe that blocks its next write. Codex 0.166 `apply_patch` and opencode 1.18 fail with an error; Bun 1.4 `fs.writeFile` reports success and keeps the old tail of the file ([oven-sh/bun#47945](https://github.com/oven-sh/bun/issues/47945)). `worktree::is_clean` (`src-app/src/workspace/worktree.rs:1089`) has the same exposure.
2. **The diff badge and the diff dock map files for about a second after any index write.** Git for Windows compares mtimes at one-second granularity, so a same-size rewrite in the second of an index write still matches the stat, the entry is racily clean, and `git diff-index --raw` (`src-app/src/workspace/git.rs:230`) and `git diff --name-status -M` (`src-app/src/diff/git.rs:369`) hash it through a mapped view: 611 to 721 failed writes over 5 rounds of 600 ms. Waiting 1.1 s after the index write makes both clean, which confirms the window.
3. **Every probe is a process.** A Files sidebar refresh spawns 2 git processes, a diff-stat probe 3 (`crates/paneflow-host/tests/persistent_baseline/gates.rs:255-261`), a diff dock refresh 2 plus one `git show` per file, and 30 workspaces cost 24 git processes a minute (`gates.rs:263-268`).
4. **The one-flag mitigation does not work.** `-c core.bigFileThreshold` streams instead of mapping only when no conversion applies, so under `core.autocrlf=true`, the Git for Windows default, writes still fail. Without a conversion, about 60 % of status probes die with `fatal: failed to read 16384 bytes`. Git also sometimes dies with `fatal: mmap failed: Invalid argument` when it maps a file at the instant it is truncated to zero.
5. **gix, the in-process candidate, is not ready as published.** gix 0.90 never fails an agent write, but `gix-status` 0.37 aborts the whole status when a file shrinks while it is hashed. With `core.autocrlf=false` this fails 68 to 72 % of probes on Windows 11 and 86 of 300 on Linux. The spike saw no failure on Windows only because the system `core.autocrlf=true` routes hashing through a fully read buffer. The fix (arthjean/gitoxide@ac15bc058, upstream PR [GitoxideLabs/gitoxide#3161](https://github.com/GitoxideLabs/gitoxide/pull/3161)) brings it to 0 of 600 on Windows, but it has no maintainer review yet.

**Why now:** #129 removed the dominant source (`diff --shortstat`) on 2026-11-05 and left these residuals documented on #124. A Bun 1.4 tool turns one collision into silent data loss, and the agents most exposed (Codex, opencode) are the ones Paneflow users run side by side. The spike answered the mapping, racy, cost and weight questions, the diagnosis pinned the remaining failure scenarios with a deterministic loop, and the one upstream blocker has a verified fix that Paneflow can vendor today.

## Overview

Paneflow gains a read-only in-process git engine, `crates/paneflow-git`, built on gix 0.90 with a vendored `gix-status` that reports a file shrinking under the hash as modified instead of failing. The engine opens the repository per probe and drops it, never writes the index, never runs a program the repository's own config declares, and reads the worktree with plain reads, which a truncating writer never trips over. It returns a status snapshot: per path, the HEAD-to-index change, the index-to-worktree change, the conflict kind and the untracked flag. Each probe maps that snapshot to the types the UI already consumes (`GitStatuses`, `GitDiffStats`, `WorktreeDiff`, `is_clean`), so no rendering changes.

The git CLI stays as the fallback and the oracle. An engine selector in `src-app` picks gix unless the repository uses reftable or SHA-256, gix failed twice in a row for that repository, or `PANEFLOW_GIT_ENGINE=cli` is set. A differential test harness runs the CLI probe and the gix probe on the same fixture matrix (line endings, attributes, conflicts, renames, submodules, linked worktrees, sparse checkout, split index, untracked cache) on the Linux, macOS and Windows legs and fails on any difference. A race harness brought into the test suite replays both mapping scenarios against each probe; each switch story turns its probe's race test from red to green on the Windows leg.

Key decisions, each with its evidence:
- **Vendor `gix-status` now** rather than wait for a release: without the fix, 68 to 72 % of probes fail on a repository with `core.autocrlf=false`, which is the Linux and macOS default. The copy follows the `vendor/portable-pty` convention and is dropped at the first release that contains the fix.
- **Parity with today's filter neutralization**, not "no filters at all": `neutralize_repository_filters` (`src-app/src/git_command.rs:146`) blanks only drivers declared in the repository's local and worktree config, so a globally installed git-lfs still cleans LFS files. gix runs drivers from trusted sections by default (`gix-0.90.0/src/filter.rs:225-245`), so a validation spike settles the mechanism before any probe switches.
- **The Files sidebar first**, because it maps on every probe; the badge and the dock only map inside the racy window. `is_clean` comes last because a false "clean" answer would let a worktree removal discard work, so it falls back to the CLI on any gix error and never answers clean on an error.
- **A separate crate**, because no crate under `crates/` touches git today, gix must stay out of the size-capped helper binaries, and the parity and race tests of the engine then run without building GPUI.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| Agent writes never fail because of a Paneflow probe | 0 failed truncating writes in the race harness for the Files sidebar on the Windows leg | 0 for every background probe (sidebar, badge, dock) in both scenarios |
| Probes stop spawning git | 0 git processes per Files sidebar refresh on the gix path | 0 per diff-stat probe and per dock listing; 30 workspaces at 1 git process a minute or fewer |
| Statuses stay exactly right | 0 differences between the CLI and gix probes on the oracle matrix, on every CI leg | Fallback to the CLI on less than 1 % of probes, measured by the engine counters |
| No regression in cost | gix probe p50 at or below the CLI p50 on the Paneflow checkout, Windows and Linux | Same on a repository of about 50,000 files, or a measured size threshold routes larger repositories to the CLI |

## Target Users

### Windows user running Codex, opencode or a Bun-based agent
- **Role:** a developer on Windows 10 or 11 who runs one or more coding agents in Paneflow panes, with the Files sidebar or the diff badge visible.
- **Behaviors:** lets the agent edit files for minutes at a time, reviews the changes in the diff dock, keeps several workspaces open.
- **Pain points:** Codex `apply_patch` reports "Failed to write file" and opencode reports `EUNKNOWN` at random; a Bun 1.4 tool can leave a file with the old tail after reporting success, which surfaces later as a corrupt file or a broken build.
- **Current workaround:** none that keeps the feature. Closing the Files sidebar removes the dominant source; nothing removes the racy window of the badge.
- **Success looks like:** agent edits never fail or corrupt because Paneflow was looking at the repository.

### Every Paneflow user with many workspaces
- **Role:** a user with 5 to 30 workspaces open, on any of the three platforms.
- **Behaviors:** switches workspaces, keeps agents running in hidden ones.
- **Pain points:** each file-event burst and each 30 s poll spawns git processes (24 a minute with 30 workspaces), each paying process creation, config parsing and index loading.
- **Current workaround:** none.
- **Success looks like:** probes cost a function call, not a process.

### Paneflow maintainer
- **Role:** Arthur, sole maintainer, who ships the four targets.
- **Behaviors:** reviews epics, cuts releases, answers bug reports.
- **Pain points:** a status divergence would show wrong colors in the Files tree or a wrong count on the badge, and an `is_clean` divergence could discard work.
- **Current workaround:** the git CLI as the only engine.
- **Success looks like:** every divergence is caught by the oracle before release, and a user can switch back to the CLI without a new build.

## Research Findings

Key findings that informed this PRD:

### Competitive Context
- Zed: runs `git status --porcelain=v1 --untracked-files=all --no-renames -z` as a subprocess, with `--no-optional-locks`, `core.fsmonitor=false`, an empty hooks path and `diff.external=` ([repository.rs](https://raw.githubusercontent.com/zed-industries/zed/main/crates/git/src/repository.rs)). Paneflow's probe profile already matches that hardening; on Windows it inherits the same mapping behavior.
- starship: moved from libgit2 toward parsing git and gitoxide ([starship#921](https://github.com/starship/starship/issues/921)); whether its `git_status` module uses gix today is unconfirmed.
- VS Code, lazygit, GitHub Desktop (dugite): believed to run the CLI; unconfirmed, so this drives no decision.
- **Market gap:** no confirmed tool computes background status in process to keep files edited by other processes writable on Windows. By mechanism, any tool that runs `git status` in the background exposes Codex and opencode writes to 1224; that is inferred, not measured on those tools.

### Best Practices Applied
- Never write the index from a background probe (`GIT_OPTIONAL_LOCKS=0` today; with gix, never call `Outcome::write_changes`, `gix-0.90.0/src/status/iter/types.rs:88-112`).
- Never run repository-configured programs from a background probe (Zed's fsmonitor note; Paneflow's `neutralize_repository_filters`).
- Bound the threads per status: gix uses one thread per logical core when `thread_limit` is `None` (`gix-0.90.0/src/status/index_worktree.rs:38-41`), plus up to two producer threads (`gix-0.90.0/src/status/iter/mod.rs:72,116`).
- Skip what the probe does not need: submodule status opens each submodule fully (`gix-0.90.0/src/status/index_worktree.rs:230-238`).
- Pin gix exactly: 14 gix releases shipped in 2026, about one breaking minor a month ([crates.io](https://crates.io/api/v1/crates/gix/versions)).
- Keep a fresh repository per probe: a cached gix `Repository` returned "object could not be found" once after `git repack -a -d` in the spike; fresh opens never did.

*Full research sources: the #130 issue comments, and the local helper reports of this PRD run.*

## Assumptions & Constraints

### Assumptions (to validate)
- A1: gix can be made to skip drivers declared in the repository's own config while keeping system and global ones, through `config_overrides` or a driver-free pipeline passed to `gix_status::index_as_worktree` (`gix-status-0.37.0/src/index_as_worktree/types.rs:41`). Validated by US-002.
- A2: gix status matches the CLI on linked worktrees, submodules, sparse checkout, split index and the untracked cache. The spike compared path sets only, on base fixtures. Validated by US-006 to US-008.
- A3: a fresh open per probe with a thread limit of 1 or 2 costs no more wall time and CPU than the CLI process. The spike measured gix with one thread per core. Validated by US-004.
- A4: upstream accepts #3161 or an equivalent fix in a release within six months. The vendored copy covers the gap.
- A5: gix honors `GIT_CONFIG_NOSYSTEM` (read in `gix-0.90.0/src/config/cache/init.rs`) and `GIT_CONFIG_GLOBAL`, so the existing fixture isolation (`src-app/src/git_fixture.rs:27-30`) applies to both engines. Validated by US-006.
- A6: gix re-hashes racily clean entries with plain reads. The diagnosis measured 0 failed writes for gix in the racy scenario; gix compares mtimes at second granularity by default (`gitoxide.core.useNsec` false, `gix-0.90.0/src/config/cache/access.rs:287`).

### Hard Constraints
- Linux (x86_64, aarch64), macOS (Apple Silicon) and Windows 10/11 x64 all ship the change; every story names how each platform is verified.
- Nothing runs on the GPUI main thread: probes stay on the Files sidebar worker thread and in `smol::unblock`.
- Write operations (commit, checkout, worktree add and remove, stash) stay on the git CLI.
- The engine never writes under `.git` and never runs a program declared in the repository's own config.
- gix stays out of `paneflow-shim`, `paneflow-ai-hook` and `paneflow-mcp`, which have hard size caps.
- `cargo deny` licenses, advisories and sources stay clean; every cargo invocation stays `--locked`.
- No comments in source; items go before any `mod tests`.
- Wall-clock budgets live in the perf gates, never in unit tests.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, before every commit and push
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, tests included
- `cargo nextest run --workspace --locked --profile ci` - the CI test runner
- `cargo deny check advisories licenses sources` - for stories that change dependencies or vendored crates
- `bash scripts/check-test-wallclock.sh` - for stories that add or change tests
- `bash scripts/check-workflows.sh` - for stories that change `.github/**`

Platform gates:
- `cfg(windows)` code is linted only by the `Windows x86_64 libghostty check` job: read item order before pushing.
- Stories with a Windows race test also run `cargo test --workspace --no-fail-fast` natively on Arthur's Windows host and record the red-before and green-after counts in the PR.
- Stories that change a perf gate budget run `scripts/perf-gates.sh` on the calibrated Linux runner class.
- No UI changes: Arthur checks the Files sidebar, the badge and the dock himself on his Windows host before a release.

## Epics & User Stories

### EP-001: A read-only in-process git engine

Paneflow can compute a repository's status in process, without mapping worktree files, without running repository-configured programs, and with a measured cost no higher than the CLI's.

**Definition of Done:** `crates/paneflow-git` returns a status snapshot on the three platforms, with the vendored short-read fix, a validated no-program guarantee, measured cost and thread limit, and an engine selector in `src-app` that falls back to the CLI. No probe uses it yet.

#### US-001: Vendor gix-status 0.37.0 with the short-read fix
**Description:** As the maintainer, I want `gix-status` 0.37.0 vendored with the fix from arthjean/gitoxide@ac15bc058 so that a file shrinking while it is hashed reports a modification instead of aborting the status.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `vendor/gix-status/`, when it is compared with the `gix-status` 0.37.0 archive from crates.io, then the only differences are the `HashEq` and `FastEq` short-read handling of commits 6eb1980 and ac15bc058, the left-out examples and dev-dependencies, and the header.
- [x] The vendored `Cargo.toml` header names the upstream version and commit, the exact change, links #130 and GitoxideLabs/gitoxide#3161, and says to drop the copy at the first `gix-status` release that contains the fix, following `vendor/portable-pty/Cargo.toml:1-8`.
- [x] The root `[patch.crates-io]` routes `gix-status` to the vendored path with a one-line comment that points at the header, after the existing entries, which stay unchanged.
- [x] Given gix is bumped to a version that no longer depends on `gix-status` 0.37.0, then a check fails because the patch entry is unused, instead of the build silently using the published crate.

#### US-002: Validate assumption: a gix status runs no program the repository configures
**Description:** As the maintainer, I want proof that an in-process status runs no filter driver, fsmonitor command, hook or textconv declared in the repository's own config, while keeping system and global drivers like the CLI probe does, so that opening an untrusted checkout cannot execute code.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given a fixture whose `.git/config` declares `filter.marker.clean`, `filter.marker.smudge`, `filter.marker.process`, `core.fsmonitor` and a `diff.marker.textconv`, each pointing at a program that creates a marker file, and whose `.gitattributes` assigns `filter=marker diff=marker` to every file, when a gix status runs on a stat-dirty tracked file, then the marker file never appears, on the Linux, macOS and Windows legs.
- [x] Given the same drivers reached through an `include.path` or `includeIf` in the repository config, when a gix status runs, then the marker file never appears.
- [x] Given `filter.marker.required=true` in the repository config, when a gix status runs, then it succeeds and hashes the raw file, as the CLI probe does after `neutralize_repository_filters`.
- [x] Given a driver declared in the global config (`GIT_CONFIG_GLOBAL` pointing at a fixture file), when both the CLI probe and a gix status run on the same stat-dirty file, then the driver runs in both or in neither, and both report the same status.
- [ ] The chosen mechanism (`config_overrides`, or `gix_status::index_as_worktree` with a driver-free pipeline) and the rejected one are recorded with their test result in the PR and in `ARCHITECTURE.md`.
- [x] Given neither mechanism passes the first criterion, then the story is BLOCKED and the epic stops before US-003.

#### US-003: The paneflow-git crate returns a status snapshot
**Description:** As a probe author, I want a function that returns the status of a worktree as a neutral snapshot so that each probe can map it to its own types without touching gix.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001, US-002

**Acceptance Criteria:**
- [x] `crates/paneflow-git` is a workspace member whose only git dependency is `gix = "=0.90.0"` with `default-features = false` and the features `status`, `parallel` and `sha1`; `src-app` is its only dependent, and `cargo tree` shows no gix crate under `paneflow-shim`, `paneflow-ai-hook` or `paneflow-mcp`.
- [x] Given a worktree path, when the snapshot is requested, then it opens a `ThreadSafeRepository` for that call and drops it before returning; no gix repository outlives a call.
- [x] The snapshot lists entries sorted by path bytes, each with an optional HEAD-to-index change (added, modified, deleted, type change), an optional index-to-worktree change (modified, deleted, type change, intent to add, submodule), an optional conflict kind (the seven kinds of `gix-status-0.37.0/src/index_as_worktree/types.rs:205-219`) and an untracked flag, plus the HEAD tree id or an unborn marker.
- [x] Renames are disabled on both halves (`tree_index_track_renames(Disabled)`, `index_worktree_rewrites(None)`), and untracked files are listed one by one, sorted before any cap.
- [x] The options carry a thread limit, an untracked mode (files or none), a submodule mode, an entry cap and an interrupt flag; the defaults are the values US-004 measures.
- [x] Given a snapshot of a repository, the index file's bytes and modification time are unchanged afterwards, and nothing under `.git` is created or removed.
- [ ] Given a writer that rewrites a 480 KiB tracked file at constant size while 200 snapshots run, then no snapshot fails and no write fails, on the Linux and Windows legs.
- [x] Given a path outside any repository, a bare repository, or an interrupt raised mid-walk, then the snapshot returns a distinct error for each instead of panicking.

#### US-004: Validate assumption: an in-process probe costs no more than the CLI
**Description:** As the maintainer, I want the wall time, CPU time and peak memory of a fresh-open gix snapshot measured against the CLI probe it replaces so that the thread limit and any size threshold rest on numbers, not on the spike's all-cores run.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] A release-build measurement entry point (an example or a bench suite of the existing harness) reports p50 and p95 wall time, CPU time and peak memory over 200 probes for the gix snapshot and for `git --no-optional-locks status --porcelain=v1 -z -uall --no-renames`, on the Paneflow checkout and on a public repository of about 50,000 files.
- [ ] Runs exist on Windows (Arthur's host) and Linux x86_64, each recorded with the machine, the load conditions and the commit in the PR.
- [x] The thread limit default is set from these numbers, at 1 or 2, with the reason in `ARCHITECTURE.md`.
- [x] Peak memory on Linux is measured with heaptrack and compared with the peak RSS of the git process on the same repository.
- [x] Given gix exceeds the CLI p50 or CPU time on the larger repository, then US-005 adds an index-entry threshold above which the CLI runs, set from the measured crossover.

#### US-005: The engine selector falls back to the git CLI
**Description:** As a user, I want Paneflow to use the git CLI whenever the in-process engine cannot answer so that I always see statuses, and to force the CLI with one environment variable if gix misbehaves.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003, US-004

**Acceptance Criteria:**
- [x] Given a repository whose `HEAD` holds the reftable marker (`src-app/src/workspace/git.rs:10`) or whose `extensions.objectFormat` is not `sha1`, when a probe runs, then the CLI answers and no gix open happens for that repository.
- [x] Given a gix probe that fails, when it fails, then the same call returns the CLI result, so the caller never sees the gix error.
- [x] Given two consecutive gix failures for one git directory, then every later probe of that directory uses the CLI for the rest of the process lifetime, and one warning names the directory and the last error.
- [x] Given `PANEFLOW_GIT_ENGINE=cli`, every probe uses the CLI; given `PANEFLOW_GIT_ENGINE=gix` or unset, gix is used where allowed; given any other value, gix is used and one warning lists the accepted values.
- [x] `system.counters` exposes probes per engine and per probe, and fallbacks per reason (reftable, object format, error, forced, size threshold).
- [x] `ARCHITECTURE.md` documents the engine, its thread, the fallback rules and the environment variable.

---

### EP-002: A parity oracle against the git CLI

Every difference between the CLI probes and the gix snapshot is caught by a test on every CI leg before a probe switches.

**Definition of Done:** the differential harness passes on the Linux, macOS and Windows legs for the base matrix, linked worktrees, submodules, sparse checkout, split index, the untracked cache and the ownership checks, and the porcelain mapping it tests is the one the probes use.

#### US-006: A differential harness compares both engines on the base fixture matrix
**Description:** As the maintainer, I want each fixture run through the CLI probe and the gix snapshot with their results compared so that a divergence fails a test instead of showing a wrong status.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] One mapping function turns a snapshot into porcelain v1 records (XY code and path); the probes of EP-003 to EP-005 use that same function.
- [ ] The base matrix covers `core.autocrlf` false, true and input; `.gitattributes` `eol=crlf`, `text`, `-text` and `binary`; a staged addition, modification and deletion; staged then modified; a deletion from the worktree; a staged rename under `--no-renames`; nested untracked files; an ignored file; an unborn `HEAD`; intent to add; a type change; and each of the seven conflict kinds.
- [ ] Given every fixture, the multiset of (XY, path) records is equal between the CLI and gix, on the Linux, macOS and Windows legs.
- [ ] Given a divergence, the failure names the fixture, the path, the CLI code and the gix code.
- [ ] The fixture config isolation (`git_fixture::isolate`) is shown to apply to gix: a fixture with a global `core.autocrlf` set outside the isolated files gives the same result in both engines.
- [ ] Given a platform that cannot create a fixture (a symlink type change without the Windows privilege), the test reports that fixture as skipped by name instead of passing silently.

#### US-007: Linked worktrees and submodules match the CLI
**Description:** As a user of Paneflow worktrees and submodules, I want the in-process status to read the right index and report submodules like git does so that worktree tabs and superprojects show correct colors.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given a linked worktree created by `git worktree add` and one created by Paneflow's own worktree flow, when both engines run from the linked path, then they report the linked worktree's index and changes, not the main worktree's.
- [ ] Given submodules that are clean, checked out at a new commit, holding modified content, holding untracked content, and not initialized, then both engines give equal records with the files probe's arguments and with the `is_clean` arguments (`--ignore-submodules=none`).
- [ ] Given `submodule.<name>.ignore` set to `dirty` and to `all`, then both engines give equal records.
- [ ] Given a submodule whose git directory is missing, then the superproject status does not fail and both engines report the submodule the same way.
- [ ] Given 10 submodules, a snapshot opens each submodule repository at most once.

#### US-008: Sparse checkout, split index, untracked cache and ownership match the CLI
**Description:** As a user of large or unusual repositories, I want the in-process status to handle every index layout git writes so that switching engines never hides a change.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006

**Acceptance Criteria:**
- [ ] Given cone sparse checkout without and with `index.sparse=true`, and non-cone sparse checkout, then both engines give equal records, including files outside the sparse set.
- [ ] Given a split index (`git update-index --split-index`), an untracked cache (`git update-index --untracked-cache`), an fsmonitor extension written while `core.fsmonitor` was on, `index.skipHash=true`, and index version 4, then both engines give equal records.
- [ ] Given an index with an unknown mandatory extension, then gix fails, the selector answers with the CLI, and the counters record the fallback.
- [ ] Given a repository the CLI refuses for ownership (`safe.directory`), then the gix probe also returns no status, and a test of the trust decision covers the reduced-trust case on every platform.

---

### EP-003: The Files sidebar stops mapping agent files

The dominant residual of #124 is gone: the Files sidebar computes its statuses in process, with zero git processes and zero failed agent writes.

**Definition of Done:** `files_git::read` runs on the gix snapshot behind the selector, its race test is green on the Windows leg after being recorded red on main, and its statuses equal the CLI's on the oracle matrix.

#### US-009: A reusable race harness pins both mapping scenarios
**Description:** As the maintainer, I want the diagnosis loop in the test suite so that each probe switch proves its probe no longer blocks a truncating write, and a regression turns a test red.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] A test helper races a probe against a writer that rewrites a 480 KiB tracked file at constant size and returns the write count, the failures by OS error and the probe errors.
- [ ] The helper has two scenarios: a stat-dirty file (backdated, index refreshed, then rewritten), and a racy window (an index write aligned to the start of a wall-clock second, then same-size rewrites inside that second, over 5 rounds).
- [ ] The existing `a_truncating_agent_write_never_fails_while_the_diff_stats_probe_runs` runs on the helper in the stat-dirty scenario and stays green, and the untracked listing gets the same test.
- [ ] Given a run where the writer never overlaps a probe (fewer writes than probes), the helper fails the test as inconclusive instead of passing.
- [ ] The PR records the helper's red counts on Arthur's Windows host against main for `files_git::read` and `is_clean` (stat-dirty) and for `GitDiffStats` and the dock listing (racy); those red tests land with their switch stories.
- [ ] The helper makes no wall-clock assertion.

#### US-010: files_git::read runs on the gix snapshot
**Description:** As a Windows user running Codex or opencode, I want the Files sidebar to compute its statuses in process so that its refreshes never make my agent's edits fail.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005, US-007, US-008, US-009

**Acceptance Criteria:**
- [ ] Given the selector picks gix, `files_git::read` builds `GitStatuses` from the snapshot through the US-006 mapping, scoped to the sidebar root as `-- .` is today, and spawns 0 git processes per refresh.
- [ ] Given every oracle fixture, `GitStatuses` built from gix equals `GitStatuses` built from the CLI output.
- [ ] Given the stat-dirty race test against `files_git::read`, then it fails on main on the Windows leg (recorded in US-009) and records 0 failed writes after this story.
- [ ] Given a status larger than today's 4 MiB stdout cap equivalent, then `read` returns `None` and the sidebar keeps its previous statuses, as today.
- [ ] Given gix fails once on a refresh, then that refresh returns the CLI statuses and the counters record the fallback.
- [ ] The probe still runs on the Files sidebar worker thread (`src-app/src/app/files_sidebar/worker.rs:178`), never on the GPUI main thread.

---

### EP-004: The diff badge stops spawning git

The diff badge leaves the racy window and the process cost behind: its listing, its blob reads and its untracked scan run in process.

**Definition of Done:** `GitDiffStats` spawns 0 git processes on the gix path, its racy race test is green on the Windows leg, its existing shortstat parity tests pass on both engines, and the perf gates count probes without relying on git spawns.

#### US-011: GitDiffStats lists changes and reads blobs in process
**Description:** As a user watching the diff badge, I want the badge's listing, base blobs and untracked scan computed in process so that the badge never maps a file after an index write and never spawns git.

**Priority:** P1
**Size:** L (5 pts)
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given the selector picks gix, the HEAD-to-worktree change list comes from the snapshot (both halves composed per path, renames off) instead of `diff-index --raw`, base blobs come from the object database in the same open instead of `cat-file --batch`, and untracked files come from the snapshot instead of `ls-files --others`.
- [ ] Given the probe runs, it spawns 0 git processes on the gix path, against 3 today.
- [ ] `tracked_counts_match_git_shortstat_for_every_kind_of_change`, `an_autocrlf_checkout_counts_line_endings_like_git` and the other `workspace::git` tests pass for both engines.
- [ ] Given a path staged then restored to the `HEAD` content in the worktree, then it is not counted, as today.
- [ ] Given an unborn `HEAD`, the list compares against the empty tree; given a missing base blob (a partial clone), the file is handled like today's missing blob in `a_blob_batch_reports_missing_large_and_cut_off_objects`.
- [ ] Given the racy-window race test against `GitDiffStats::from_cwd`, then it fails on main on the Windows leg (recorded in US-009) and records 0 failed writes after this story.
- [ ] The caps stay as today: 200 tracked and 1,000 untracked paths, 512 KiB per file, 8 MiB of blobs, a capped listing shown as a lower bound.

#### US-012: The perf gates count probes, not git spawns
**Description:** As the maintainer, I want the perf gates to identify a diff-stat probe by a probe counter so that moving the probe in process does not make the gates report that no probe ran.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] `crates/paneflow-host/tests/persistent_baseline/gate_runs.rs:437` counts diff-stat probes from the engine probe counter instead of `git_spawns.by_subcommand.diff-index`.
- [ ] `desktop.diff_stat.git_spawns_per_probe` becomes 0 on the gix path, and the 30-workspace budget is re-derived from measured gix runs with the measurement in its margin text.
- [ ] Given a gate run where the selector fell back to the CLI, the gate fails and names the fallback reason.
- [ ] `the_second_diff_stat_probe_of_an_unchanged_repository_spawns_three_git_processes` becomes a test of 0 spawns on the gix path and keeps a CLI variant under `PANEFLOW_GIT_ENGINE=cli`.
- [ ] `bench/README.md` describes the new counters and budgets.

---

### EP-005: The diff dock and the removal snapshot

The last two probes move in process: the dock lists HEAD-to-worktree changes with renames and reads base texts without spawning git, and the worktree removal snapshot uses gix without ever risking a false "clean".

**Definition of Done:** the dock's listing and base texts run in process with rename parity, its racy race test is green on the Windows leg, and `is_clean` matches the CLI on every oracle fixture and fails safe.

#### US-013: The diff dock lists changes with renames in process
**Description:** As a user reviewing an agent's work in the diff dock, I want the dock's file list, renames included, computed in process so that refreshing the dock never blocks the agent's next write.

**Priority:** P2
**Size:** L (5 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given the selector picks gix, `compute_diff_against` (`src-app/src/diff/git.rs:366`) builds the base-to-worktree change list in process, with rename pairing equivalent to `git diff -M` (50 % similarity, base blobs from the object database, worktree texts read in Rust).
- [ ] Given a pure rename, a rename with an edit above the threshold, an edit below it, and a type change, then the `FileChange` list and `old_path` values equal the CLI's.
- [ ] Given more rename candidates than `diff.renameLimit`, then renames are skipped as git skips them.
- [ ] Given a base other than `HEAD`, the list compares that commit's tree with the worktree, as `git diff <base>` does.
- [ ] Given the racy-window race test against `compute_head_diff`, then it fails on main on the Windows leg (recorded in US-009) and records 0 failed writes after this story.

#### US-014: The diff dock reads base texts from the object database
**Description:** As a user opening the diff dock, I want base texts read in process so that a dock refresh no longer spawns one `git show` per file.

**Priority:** P2
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013

**Acceptance Criteria:**
- [ ] Given the selector picks gix, the base text of each file comes from the object database instead of `git show base:path` and the `ls-tree` fallback.
- [ ] Given every dock fixture, including CRLF blobs, binary blobs and lockfiles, the texts equal what `git show` returns.
- [ ] Given a refresh of 200 files, the dock spawns 0 git processes on the gix path.
- [ ] Given a missing blob, the file shows the same error as today's `git show` failure path.

#### US-015: is_clean uses gix and fails safe
**Description:** As a user removing a worktree, I want the clean check computed in process and never wrong in the dangerous direction so that a removal can never discard uncommitted work.

**Priority:** P2
**Size:** S (2 pts)
**Dependencies:** Blocked by US-007, US-008, US-010

**Acceptance Criteria:**
- [ ] Given the selector picks gix, `is_clean` answers from a snapshot with untracked files listed and submodule content checked, matching `--ignore-submodules=none`.
- [ ] Given every oracle fixture, `is_clean` gives the same answer for both engines, including dirty and untracked submodule content.
- [ ] Given a gix error or an interrupt, the CLI answers; given the CLI also fails, `is_clean` returns an error and never `Ok(true)`.
- [ ] Given the stat-dirty race test against `is_clean`, then it fails on main on the Windows leg (recorded in US-009) and records 0 failed writes after this story.
- [ ] Given a linked worktree whose admin directory was pruned, `is_clean` returns an error, as today.

---

## Functional Requirements

- FR-01: On the gix path, a background probe must read worktree files with plain reads and must never memory-map them.
- FR-02: The engine must never write the index or create, modify or remove anything under `.git`.
- FR-03: The engine must never run a filter driver, fsmonitor command, hook, textconv or external diff declared in the repository's own config, including through `include.path` and `includeIf`.
- FR-04: Filter drivers declared in the system or global config must behave as they do for the CLI probe.
- FR-05: Each probe must open its repository and drop it before returning; no gix repository may be kept between probes.
- FR-06: For every oracle fixture, each probe must return the same result on gix as on the CLI.
- FR-07: The selector must use the CLI for reftable and non-SHA-1 repositories, after two consecutive gix failures for a git directory, above a measured size threshold if US-004 sets one, and when `PANEFLOW_GIT_ENGINE=cli`.
- FR-08: A failed gix probe must be answered by the CLI in the same call.
- FR-09: A file that changes or shrinks while a probe reads it must yield a status for that file, never a failed probe.
- FR-10: Probes must run off the GPUI main thread.
- FR-11: `system.counters` must expose probes per engine and per probe, and fallbacks per reason.
- FR-12: Write operations must stay on the git CLI.
- FR-13: `is_clean` must never answer clean when either engine failed.

## Non-Functional Requirements

- **Performance:** a gix Files sidebar probe p50 at or below the CLI p50 on the Paneflow checkout (spike reference: 22.9 ms CLI on Windows 11, 6.6 ms on WSL Linux), with CPU time per probe at or below the CLI's; at most 2 hashing threads per probe plus gix's 2 producer threads.
- **Reliability:** 0 failed truncating writes in at least 1,000 writes per race scenario per probe; 0 probe errors in 600 probes under the truncating writer; fallbacks on less than 1 % of probes over one week of the maintainer's use, read from the counters.
- **Memory:** peak memory of a gix probe at or below the peak RSS of the git process it replaces on the same repository, measured with heaptrack on Linux.
- **Size:** the stripped release `paneflow` binary grows by at most 4 MB on Linux x86_64 and Windows x64; the three helper binaries stay byte-identical in dependencies and within their caps; a clean release build takes at most 10 % longer on the Linux x86_64 CI runner.
- **Security:** 0 marker-program executions in the US-002 fixtures on the three platforms; `cargo deny` advisories clean, with `gix-sec` at or above 0.13.3 (CVE-2025-24890) and gix at or above 0.82.0 (CVE-2026-82253).
- **Accessibility:** no UI change; the sidebar, badge and dock render the same data with the same components.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | File shrinks mid-hash | An agent truncates a file while a probe hashes it | The file is reported modified; the probe succeeds | None |
| 2 | Same-size rewrite after an index write | A user `git add` then an agent rewrite in the same second | gix re-hashes with plain reads; the write succeeds | None |
| 3 | Unborn HEAD | A freshly initialized repository | Changes are compared with the empty tree | None |
| 4 | reftable or SHA-256 repository | `extensions.refStorage=reftable` or `objectFormat=sha256` | The CLI answers; no gix open | None |
| 5 | gix error | Corrupt or unsupported index, unexpected layout | The CLI answers the same call; after two in a row the repository stays on the CLI | One warning in the log naming the directory |
| 6 | Untrusted repository | The repository is owned by another user and not in `safe.directory` | No status, as with the CLI today | None |
| 7 | Huge status | More entries than the 4 MiB cap equivalent | The sidebar keeps its previous statuses; the badge shows a lower bound | None |
| 8 | Index rewritten during a probe | A user git command replaces `.git/index` atomically | The probe reads the old or the new index, both valid | None |
| 9 | Worktree removed during a probe | A linked worktree is pruned while probed | The probe returns an error; the sidebar keeps its previous statuses; `is_clean` returns an error | None |
| 10 | Workspace closed mid-probe | The interrupt flag is raised | The snapshot stops and returns an interrupted error, not a partial status | None |
| 11 | Repository filter declared | `filter.lfs.*` or a custom driver in `.git/config` | The driver is not run; the raw file is hashed, as with the CLI probe | None |
| 12 | Global LFS driver | git-lfs installed in the global config | The driver behaves as with the CLI probe | None |
| 13 | Many workspaces | 30 workspaces polled | Thread use stays bounded by the thread limit per probe | None |
| 14 | Forced CLI | `PANEFLOW_GIT_ENGINE=cli` | Every probe uses the CLI | None |
| 15 | Non-UTF-8 or long paths | Linux byte paths, Windows paths over 260 characters | Paths round-trip as the CLI parser handles them today | None |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | A gix status diverges from git on a layout the oracle does not cover | Med | High | Oracle matrix on three platforms (EP-002), CLI fallback on errors, `PANEFLOW_GIT_ENGINE=cli` as a no-build rollback |
| 2 | `is_clean` answers clean on a dirty worktree | Low | High | Last story, fail-safe rules (FR-13), submodule content checked, oracle on every fixture |
| 3 | A repository-configured program runs in process | Low | High | US-002 blocks the epic until the marker test passes on three platforms |
| 4 | Upstream rejects or reshapes #3161 | Med | Med | Vendored copy with a drop condition; the behavior is pinned by the race and short-read tests, not by the upstream code |
| 5 | gix ships a breaking minor every month | High | Low | Exact pin; a bump is its own change with the oracle as its gate |
| 6 | Thread pools or fresh opens cost more CPU than the CLI on large repositories | Med | Med | US-004 measures before any switch and can add a size threshold |
| 7 | The perf gates report no probe once spawns disappear | High | Med | US-012 moves probe detection to a counter in the same epic |
| 8 | Binary size and build time grow | High | Low | 4 MB and 10 % budgets measured in US-003 and US-004; gix kept out of the helpers |
| 9 | Windows-only behavior cannot be verified on Linux | High | Med | Race tests run on the Windows leg; red-before and green-after counts recorded from Arthur's host |

## Non-Goals

What this version explicitly does NOT include:

- Write operations (commit, checkout, stash, worktree add and remove) stay on the git CLI.
- reftable and SHA-256 repositories run on the CLI until gix supports them; revisit when `gix-reftable` and the `sha256` feature are complete.
- Using the untracked cache or fsmonitor to speed up status: v1 only reads those extensions correctly.
- Showing statuses for repositories the CLI refuses for ownership.
- Moving other git probes (branch, log, `rev-parse` and `HEAD` reads) in process.
- Changing what the Files sidebar, badge or dock display.
- Reporting the mapping behavior to Git for Windows.

## Files NOT to Modify

- `src-app/src/git_command.rs` `GitProfile::UserAction` path and the probe argument builder: the CLI fallback keeps using them unchanged.
- `crates/paneflow-process/**`: process supervision is unchanged.
- `crates/paneflow-shim/**`, `crates/paneflow-ai-hook/**`, `crates/paneflow-mcp/**`: size-capped helpers that must not get gix.
- The GPUI `rev` values in `src-app/Cargo.toml` and the existing `[patch.crates-io]` entries of the root `Cargo.toml`.
- `vendor/portable-pty/**`.
- `native/libghostty/**`.
- The public shape of `GitSummary`, `TrackedSummary`, `GitStatuses`, `GitDiffStats`, `WorktreeDiff` and `FileChange`: migrate behind them.

## Technical Considerations

Frame as questions for engineering input, not mandates:

- **Architecture:** a separate `crates/paneflow-git` crate versus a module in `src-app`; recommended: the crate, for test speed without GPUI and a hard boundary against the helpers. Engineering to confirm that the oracle, which needs the CLI parsers in `src-app`, can live in `src-app` tests while the engine's own tests live in the crate.
- **Filter neutralization:** `config_overrides` at open versus a driver-free pipeline through `gix_status::index_as_worktree`; recommended: whichever passes US-002 with the least code outside gix's public API. Trade-off: the lower-level call loses `repo.status()`'s dirwalk and tree-index wiring.
- **Dock renames:** gix status offers rename tracking per half only, so `git diff -M <base>` has to be assembled from the composed change list plus a `gix-diff` rewrite tracker. Engineering to confirm the tracker's similarity and limit match git's defaults, or keep the dock listing on the CLI if not.
- **Threads:** a fixed thread limit per probe versus a shared pool for all probes; recommended: a fixed limit from US-004, since probes already run on their own threads.
- **Dependencies:** gix `=0.90.0` with features `status`, `parallel`, `sha1`, and the vendored `gix-status` 0.37.0. Alternative: wait for an upstream release, rejected while #3161 has no review.
- **Migration:** no data migration; rollback is `PANEFLOW_GIT_ENGINE=cli` without a new build, or reverting the selector default.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Failed truncating writes, Files sidebar, stat-dirty scenario | 136 to 325 per 40 probes | 0 per 1,000 writes | Month-1 | Race test on the Windows leg and on Arthur's host |
| Failed truncating writes, badge and dock, racy scenario | 611 to 721 per 5 rounds (diff-index) | 0 per 1,000 writes | Month-6 | Race tests on the Windows leg |
| Git processes per Files sidebar refresh | 2 | 0 | Month-1 | Spawn counters in the probe tests |
| Git processes per diff-stat probe | 3 | 0 | Month-6 | `desktop.diff_stat.git_spawns_per_probe` perf gate |
| Git processes per minute, 30 workspaces | 24 | 1 or fewer | Month-6 | `desktop.tui_30_workspaces.git_spawns_per_min` perf gate |
| Probe errors under a truncating writer, `core.autocrlf=false` | 68 to 72 % with gix 0.90; 0 to 3 % with the CLI | 0 in 600 probes | Month-1 | Race harness on the engine crate |
| CLI-gix differences on the oracle matrix | N/A (new) | 0 on every leg | Month-1 | EP-002 tests |
| Share of probes that fall back to the CLI | N/A (new) | Under 1 % | Month-6 | `system.counters` over one week of use |

## Open Questions

- Should a repository the CLI refuses for ownership get statuses through gix's reduced trust? Arthur, before US-008; v1 keeps the CLI behavior.
- Should `PANEFLOW_GIT_ENGINE` be documented in the public docs, which are synced from the paneflow-web repository? Arthur, before EP-003 ships in a release.
- Who tracks GitoxideLabs/gitoxide#3161 so the vendored copy is dropped at the first release containing the fix? Arthur; the vendored header states the drop condition.
- Which public repository of about 50,000 files serves as the large fixture in US-004? The implementer, in US-004; recommended: `rust-lang/rust`, which checks out on all three platforms.
- Is the mapping behavior worth reporting to Git for Windows, since every tool running background `git status` exposes agents the same way? Arthur, anytime; nothing in this PRD depends on it.
[/PRD]
