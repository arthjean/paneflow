# Persistent-path qualification runbook

This runbook freezes what a qualification of the persistent session path
exercises, on which candidate, with which inputs, and what counts as evidence.
It is the platform-neutral half; the per-OS qualification epics execute it on
native machines and produce the reports. Nothing here depends on a developer
machine: every input is a tracked script, a tracked fixture, or a documented
environment requirement.

## Candidate identity

One candidate is one commit SHA plus one artifact manifest. Produce the
manifest with the tracked script after building the release artifacts:

```bash
cargo build --release --locked -p paneflow-app -p paneflow-host
scripts/candidate-manifest.sh                      # writes bench/results/candidate-<sha12>-<target>.json
```

```powershell
cargo build --release --locked -p paneflow-app -p paneflow-host
scripts/candidate-manifest.ps1
```

The manifest records `candidate_sha`, `dirty`, the target triple, and the
SHA-256 and size of each artifact:

| Key | Artifact | Location |
|---|---|---|
| `desktop` | `paneflow` | `target/release/` (or `target/<target>/release/`) |
| `host` | `paneflow-host` | same directory as the desktop |
| `helper.paneflow-shim`, `helper.paneflow-ai-hook`, `helper.paneflow-mcp` | embedded helpers | `target/embed-build/<target>/release-min/` |
| `engine.manifest`, `engine.<archive>` | libghostty pin and the prebuilt archives | `native/libghostty/manifest.toml`, `native/libghostty/prebuilt/<target>/lib/` |
| `conpty.manifest`, `conpty.dll`, `conpty.openconsole` | bundled ConPTY (Windows targets only) | `native/conpty/manifest.json`, `native/conpty/prebuilt/x86_64-pc-windows-msvc/` |

`preflight` is `fail` when a required artifact is missing or, on Unix, when a
binary is not executable, and the script exits nonzero. Qualification never
starts on a `fail` manifest or on a `dirty: true` manifest. Package
preparation enforces the same placement rules on the existing routes in
`.github/workflows/release.yml`: the Linux package smokes require an executable
`paneflow-host` next to the installed desktop with a matching `--version`, the
`Validate Windows helper packaging set` step requires every embedded helper to
match the WiX `HelperBinaries` entries, and the Windows MSI smoke exercises the
silent install and the update relay. At runtime the desktop's update preflight
refuses to stop a retained host when the replacement or a helper is missing
(`replacement_preflight_and_wrong_home_never_stop_live_sessions` in
`crates/paneflow-host/src/bootstrap.rs` and
`an_update_preflight_defers_a_missing_replacement_and_counts_the_host_sessions`
in `src-app/src/app/quit_dialog.rs`).

All implementation stories of the persistent-session PRD must have completed
review before an OS qualification starts. A candidate produced from a branch
still under review is a rehearsal, not evidence.

## Candidate change during qualification

A change to the candidate after evidence has been collected records the
boundaries it touched and invalidates the evidence that depends on them:

| Changed boundary | Files | Invalidated evidence |
|---|---|---|
| Session lifecycle, ownership, persistence | `crates/paneflow-host/src/{host,runtime,process,persistence,manifest,cold_text,viewport_scan,cancellation_scan}.rs` | Every W01-W08 cell on every OS |
| IPC protocol or framing | `crates/paneflow-host/src/{protocol,server,client,control}.rs`, `crates/paneflow-ipc-client/` | W02, W03, W04, W06, W08 on every OS |
| Terminal engine pin or PTY adapter | `native/libghostty/`, `native/conpty/`, `crates/paneflow-host/src/pty/` | Every cell on the affected OS; W02, W03 on every OS |
| Worker projection | `crates/paneflow-serve/` | W04 worker cells, W07, W08 on every OS |
| Desktop entry points | `src-app/src/terminal/host_link.rs`, `src-app/src/app/{hosted_sessions,quit_dialog,close_policy,event_handlers,bootstrap}.rs` | W07 and the D-cells on every OS |
| Packaging or update relay | `packaging/`, `src-app/src/app/bootstrap.rs`, `src-app/src/app/ipc_handler.rs` | Package preparation and upgrade cells on the affected OS |
| Scripts, fixtures, harness only | `scripts/`, `crates/paneflow-host/src/bin/`, `crates/paneflow-host/tests/` | Nothing, once the new harness passes the seeded-failure test |

Shared lifecycle, IPC, or engine changes therefore require renewed core
evidence (W01-W06) on all four shipping triples, not only on the OS where the
change was noticed. The rerun records the new `candidate_sha`; earlier
artifacts stay archived under their own SHA and are not merged.

## Platform matrix

| Cell | OS | Architecture | Package route | Required |
|---|---|---|---|---|
| WIN10 | Windows 10 x64 | x86_64 | MSI | no: assumed equivalent to WIN11 and recorded as untested in the report |
| WIN11 | Windows 11 x64 | x86_64 | MSI | yes |
| LIN-X64 | Fedora x86_64 natively (GNOME Wayland, and X11 through XWayland); Ubuntu, Debian, Arch, and openSUSE as host-only containers | x86_64 | tarball built like `release.yml` (ubuntu-22.04) | yes; per-distribution Wayland and X11 smokes and a native Xorg session are recorded `unavailable` |
| LIN-ARM64 | Linux aarch64 | aarch64 | none exercised | yes, through the native `Linux aarch64` job of `run_tests.yml` (clippy, workspace tests, release build) at the candidate SHA; render smoke, performance, and endurance are recorded `unavailable` |
| MAC-ARM | macOS Apple Silicon | aarch64 | DMG/app bundle | yes |
| MAC-X64 | macOS Intel | x86_64 | app bundle | when hardware is available; recorded as `unavailable` otherwise, never as passed |

Every required cell has an explicit result at the candidate SHA. A skipped
required cell is a release blocker (NFR-14).

## Environment requirements

- The tracked toolchain from `rust-toolchain.toml`; a release build of
  `paneflow-app` and `paneflow-host` (`cargo build --release --locked`).
- The libghostty archives fetched by `scripts/fetch-libghostty.sh` or `.ps1`.
- An isolated `PANEFLOW_HOME`: the harness creates its own temporary home and
  cleans only the fixtures it owns. Never run it against the operator's real
  home.
- A graphical session for the native desktop cells (`--with-desktop`,
  `-WithDesktop`); Wayland and X11 are separate cells on Linux.
- No competing load on the qualification machine for performance cells. Shared
  CI runners run the functional cases and archive timings but never decide
  performance thresholds.
- Windows: the bundled ConPTY from `native/conpty/manifest.json` must be the
  one the host loads; the run records the loaded module path, version, and
  SHA-256, taken from the `OpenConsole.exe` command line of a live session and
  the files under `%USERPROFILE%\.paneflow\cache\conpty\<version>\` (`host.status`
  does not report it).
- Linux: `/proc` readable for the host, worker, and desktop PIDs (thread and fd
  sampling). The socket directory (`XDG_RUNTIME_DIR`, else `TMPDIR`, else
  `/tmp`) must leave the endpoint under the 108-byte `sun_path` limit; a longer
  path is refused with an explicit error.
- macOS: thread and handle sampling is not implemented by the harness; those
  fields are `pending` and the report supplies `sample`/`vmmap` output.

## Workloads and inputs

The harness is `crates/paneflow-host/tests/persistent_baseline.rs` with its
workloads in `crates/paneflow-host/tests/persistent_baseline/workloads.rs`,
driven by `scripts/bench-persistent.sh` / `.ps1`. Every fixture is a mode of
`paneflow-session-fixture` (`crates/paneflow-host/src/bin/paneflow-session-fixture.rs`).

| Workload | Fixture and inputs | Automated by | Protocol |
|---|---|---|---|
| W01 baseline | `idle` at 80x24; 0/1/10/50 sessions; host-only, host+worker, host+worker+desktop | `persistent_session_baseline` W01 section | 4 s settle, 10 s sample; this window decides NFR-01 |
| W02 history | `history 10000` (deterministic ANSI and Unicode lines), 10 sequential attachments, 10 concurrent, 100 attach/detach cycles | `workloads::workload_history` | same in both protocols |
| W03 throughput | `flood 33554432` for the single-stream number; 10 x `stream 1048576 <seconds>`; `echo` fixture probed at idle and under load | `workloads::workload_throughput` | 5 s streams and 200 echo samples (quick); 60 s and 1,000 samples (full) |
| W04 faults | paused follower; worker kill/restart and build replacement with sessions live; generation change; output eviction; control disconnect | `paused_follower_probe`, `workloads::workload_worker_replacement`, plus the automated tests listed under `workloads.W04.automated_tests` | 2 worker cycles (quick); 10 (full); 100 in the endurance run |
| W05 churn | 10 batches of 50 `flood 65536` create/end/detach, RSS, thread and handle deltas, NFR-04 reclaim probe, retention with injected time | `workloads::workload_churn`, `records_past_their_retention_release_their_cold_text_under_an_injected_clock` | 5 s quiescence (quick); 60 s (full) |
| W06 injected failures | disk full/denied, stalled storage, corrupt manifest and cold text, delayed spawn, wait failure, runtime panic, old-generation scans, shutdown RPC failure | the unit and integration tests listed under `workloads.W06.automated_tests` | `cargo test --workspace --locked` |
| W07 native desktop | resize, paste, search, natural exit with both exit codes and with/without prior input, ended-row opening, stale activity at close, fallback discovery | desktop entry-point tests listed under `workloads.W07.automated_tests`, plus the D-cells below | interactive |
| W08 endurance | 9 `idle` fixtures plus one `echo` fixture retained for the whole run; a burst of 10 `flood 65536` sessions every 5 min; 100 desktop detach/reopen cycles and 100 worker crash/restart cycles spread evenly after the idle interval; one control connection untouched for the first 30 min, then the first input on it must echo exactly once | `persistent_session_endurance` (`scripts/bench-persistent.sh --endurance <minutes>` / `.ps1 -Endurance <minutes>`, `--idle-minutes` / `-IdleMinutes` for the idle interval) | 480 min (120 min on LIN-ARM64, with `PANEFLOW_BENCH_ENDURANCE_REQUIRED_MINUTES=120`); shorter runs are recorded as `rehearsal` and never as acceptance evidence |

Invocation:

```bash
scripts/bench-persistent.sh --with-worker                # full protocol, host + worker
scripts/bench-persistent.sh --with-desktop               # adds the native desktop restoration
scripts/bench-persistent.sh --quick                      # smoke protocol for CI and rehearsals
scripts/bench-persistent.sh --worker-replacement <exe>   # W04 build replacement with the given worker binary
scripts/bench-persistent.sh --prior <result.json>        # rerun after a failure; the prior failures stay in the record
scripts/bench-persistent.sh --seed-failure               # proves nonzero exit and artifact retention
scripts/bench-persistent.sh --with-desktop --endurance 480 --idle-minutes 30   # W08; writes persistent-endurance-<stamp>-<sha>.json
scripts/bench-persistent.sh --prebuilt <package>/candidate --with-worker       # runs a packaged harness and binaries without Cargo
```

`--prebuilt` takes a directory holding `candidate.json` from
`scripts/candidate-manifest.sh`, `bin/persistent_baseline`,
`bin/paneflow-session-fixture`, and the desktop and host either in `bin/` or
in `PaneFlow.app/Contents/MacOS/`. The candidate identity then comes from that
manifest instead of `git`, and the harness reads `PANEFLOW_BENCH_HOST` and
`PANEFLOW_BENCH_FIXTURE` instead of the paths Cargo compiled in.

The PowerShell script takes `-WithWorker`, `-WithDesktop`, `-Quick`,
`-WorkerReplacement`, `-Prior`, `-SeedFailure`, `-Endurance <minutes>`, and
`-IdleMinutes <minutes>`. The endurance run reads
`PANEFLOW_BENCH_WORKER_CYCLES`, `PANEFLOW_BENCH_DESKTOP_CYCLES`,
`PANEFLOW_BENCH_BURST_MINUTES`, `PANEFLOW_BENCH_SAMPLE_SECONDS`, and
`PANEFLOW_BENCH_ENDURANCE_REQUIRED_MINUTES` for rehearsals and for the LIN-ARM64
budget; any value below the W08 row marks the artifact `rehearsal`. The document
is rewritten at every sample, so a deadlock or a watchdog panic leaves the last
sample on disk.

### Linux host lifecycle checks

`scripts/qualify-linux-host.sh` drives the packaged `paneflow-host` (and the
desktop's `paneflow host` verbs when the desktop loads) through the Unix
ownership cases on any distribution, in a private home and socket directory,
and writes one tab-separated ledger row per check: L02 socket mode, L03
process groups, L04 owner lock, L05 occupied endpoint, L06 a descendant that
holds the PTY after its root exits, L07 SIGSTOP/SIGCONT during output, L08
missing runtime libraries, L09 a read-only sessions directory, L10 host
SIGKILL then restart (lost state, no launch), L11 a corrupt manifest, L12 an
explicit restart, L13 detached `paneflow host start` and `host stop`, L14 no
surviving fixture. It exits nonzero when a check fails. L09 needs a non-root
user.

```bash
scripts/qualify-linux-host.sh --app <extracted paneflow.app> \
  --fixture target/release/paneflow-session-fixture \
  --out <evidence dir>/ledger.tsv --route "<distribution and install route>"
```

Each distribution runs it twice: in the bare image, where the desktop cannot
load and L08 proves the missing-library path leaves the host untouched, and
again after installing the runtime libraries the `.deb` and `.rpm` declare in
`src-app/Cargo.toml`.

## Thresholds

The harness decides these thresholds itself and records each decision with
its measured value, its rule, and its verdict. A `pending` decision has a
reason and never counts as a pass.

| Decision | Workload | Rule |
|---|---|---|
| `NFR-04.runtime_release`, `NFR-04.reclaim_max_ms` | W05 | 0 live runtimes owned by completed sessions, reclaimed within 5 s of exit |
| `NFR-05.memory_after_churn`, `NFR-05.threads`, `NFR-05.handles`, `NFR-05.memory_slope` | W05 | RSS <= warmed baseline + max(16 MiB, 10%); threads and handles <= baseline + 2; <= 1 MiB per batch over the last 5 batches |
| `NFR-06.checkpoint_release` | W02 | 0 staged checkpoint bytes and 0 active captures after 120 attachments |
| `NFR-08.idle_p95`, `NFR-08.idle_p99`, `NFR-08.loaded_p95` | W03 | 30 ms, 100 ms, 50 ms |
| `NFR-08.throughput_ratio` | W03 | >= 90% of the matched baseline when one exists |
| `NFR-09.attach_p95`, `NFR-09.concurrent_total` | W02 | 1,000 ms over 10 repetitions; 10 concurrent attachments within 5 s |
| `NFR-11.worker_cycles` | W04 | 0 child identity or generation changes across worker cycles |
| `NFR-12.host_shutdown` | all | the host acknowledges the final shutdown and its process exits within 10 s |
| `NFR-12.fixture_orphans` | all | 0 fixture processes provably alive after the run |
| `W02.content_equivalence` | W02 | identical checkpoint bytes across all attachments |
| `NFR-05.memory_after_endurance`, `NFR-05.threads_after_endurance`, `NFR-05.handles_after_endurance` | W08 | same rules as the W05 decisions, between the first post-warmup sample and the last |
| `NFR-04.burst_release` | W08 | every burst's runtimes released within 5 s of exit while the retained sessions stay live |
| `NFR-11.worker_cycles`, `NFR-11.desktop_cycles`, `NFR-11.idle_first_input` | W08 | 0 identity or generation changes across the 100 worker and 100 desktop cycles; exactly one echo of the first input after the idle interval |
| `NFR-12.retained_identities`, `NFR-12.ownership_counters` | W08 | every retained session keeps its generation and process at every sample; live runtimes, pending launches, and unresolved descendants never exceed the retained set plus one burst, and the final sample owns exactly the retained set |

W03 also records `fairness_min_over_max` across the ten paced streams as a
reported value without a threshold.

NFR-01, NFR-02, NFR-03, NFR-07, NFR-10, NFR-13, and NFR-15 are decided by the
unit and integration tests in the coverage ledger, by the W01 short window on
the qualification machine, or by the native trace the report attaches. The
harness records them as `automated_tests` or `pending`, never as a number it
did not measure.

## Evidence format

Each run writes `bench/results/persistent-<stamp>-<sha>.json` with
`schema_version` 3:

- `candidate`: commit, diff fingerprint, toolchain, machine, OS build, engine
  identity, PTY implementation, and controller executable identity.
- `workloads.W01` through `workloads.W08`: measured values, or
  `automated_tests` lists naming the tests that decide the workload, or
  `pending` with the reason.
- `thresholds`: the decision list above.
- `prior_failures`: the failed decisions of the run named by `--prior`, kept
  verbatim so a rerun cannot erase the first failure.
- `fixtures`: every fixture identity (PID and kernel start time) the run
  owned, and the survivors at the end.
- `allocator`: `none added`; native allocations are covered by OS profiling.
  On Linux glibc the host caps malloc arenas at 2 unless `MALLOC_ARENA_MAX`
  is set, and trims the heap when a session runtime retires, so the W05 and
  W08 resident-memory decisions measure retained memory, not cached arenas.
- `comparison`: the human-readable table printed at the end of the run.

The run exits nonzero when any decision fails; the artifact is retained. The
tracked test `a_seeded_failure_fails_the_run_and_retains_its_artifact` proves
this on every CI target.

The per-OS report adds: OS edition and build, architecture, package identity
and installed paths, the candidate manifest, screenshots or recordings for the
interactive cells, and one row per cell of the matrix below with `pass`,
`fail`, `unavailable`, or `pending` and a reason. Reports live under
`docs/release/qualification/`, one file per OS and date, for example
[windows-20260922.md](qualification/windows-20260922.md) and
[linux-20260923.md](qualification/linux-20260923.md).

## Platform flush contract

Metadata, critical, and final revisions leave the PTY path through the
persistence writer thread (`crates/paneflow-host/src/persistence.rs`). The
guarantees differ by failure mode and the report states which one each cell
exercised:

- Process crash of the host: every revision is written to a unique temporary
  file and renamed over the manifest, so a reader sees either the previous or
  the new complete revision on every OS (`write_atomically_with` in
  `crates/paneflow-host/src/manifest.rs`).
- Power loss or OS crash: critical and final revisions call `sync_all`
  (`fsync` on Unix, `FlushFileBuffers` on Windows) on the temporary file
  before the rename, and on Unix the parent directory is synced after the
  rename. Metadata revisions are not synced; they are reconstructed from the
  next observation.
- Denied rename on Windows (antivirus or indexer holding the target): the
  rename is retried five times over 100 ms before the revision is reported as
  a storage failure and retried at the next interval.

## Native UI exercise cells

These are executed by hand on each OS cell with the release desktop and
recorded with a screenshot or recording. They are the interactive half of W07
and the Desktop lifecycle and recovery actions.

| Cell | Action | Expected |
|---|---|---|
| D-01 | Close the desktop with Keep running, kill the host process, reopen the saved layout | Every pane shows the lost state with Retry; no shell launched; fixture PIDs unchanged or provably absent |
| D-02 | Kill the worker while 10 fixtures run, let it restart, then type in a focused pane | Child identities and generations unchanged; the next input echoes once |
| D-03 | Replace the worker binary with the candidate build while sessions run | Same as D-02 through the build-replacement route |
| D-04 | Close the only workspace with Keep running | One fallback row under Other sessions; clicking it attaches without creating a shell |
| D-05 | Delete the recorded cwd of a kept-running session, then open its row | Attachment through the home directory; the row survives and shows the prerequisite when no root is valid |
| D-06 | Let a fixture exit with code 0 and with code 3, with and without prior input, in a pane, a detached window, and the diff dock | Passive final view on every surface; no auto-close; explicit close removes it |
| D-07 | Click an ended row | Bounded final text opens read-only; evicted text is reported as unavailable |
| D-08 | Close a pane whose agent activity is stale | Confirmation asks; nothing is stopped implicitly |
| D-09 | Stop everything and quit while one fixture ignores termination (`blocked-stdin` plus a held descendant) | Desktop stays open with the unresolved identity, Retry, Keep running and quit, Cancel |
| D-10 | Stop everything and quit with the host data directory made read-only | Confirmed exits and unconfirmed durability are reported separately; Quit with unsaved final state exits only the desktop |
| D-11 | Force-terminate the desktop process while 10 fixtures run, then reopen the saved layout | Fixture PIDs and generations unchanged; every pane reattaches without a create or restart RPC |

## Coverage ledger

Automated cases are named by their test function. Platform evidence names the
matrix cell or the D-cell. Missing coverage is a release blocker, not an
inferred pass.

### Audit findings

| Finding | Automated cases | Platform evidence |
|---|---|---|
| A01 | `replacement_preflight_and_wrong_home_never_stop_live_sessions`, `a_client_from_another_release_attaches_when_protocol_and_engine_agree`, `an_update_preflight_defers_a_missing_replacement_and_counts_the_host_sessions` | upgrade cells, D-03 |
| A02 | `a_pane_restored_into_an_ended_session_resumes_without_an_attachment`, W02 `NFR-06.checkpoint_release` | D-01 |
| A03 | `a_natural_exit_keeps_a_cold_record_releases_the_runtime_and_removal_drops_the_text`, `records_past_their_retention_release_their_cold_text_under_an_injected_clock`, W05 `NFR-04.runtime_release` | none required |
| A04 | `a_follower_resumes_after_the_checkpoint_survives_idle_keepalives_and_sees_the_exit`, W01 wait-reason attribution | W01 short window |
| A05 | the three `a_control_connection_idle_for_*` tests, `a_connection_lost_before_the_input_ack_reports_unknown_delivery_without_a_resend` | W08 30 min idle control |
| A06 | `metadata_revisions_coalesce_to_the_latest_and_a_critical_barrier_completes`, `an_older_revision_queued_behind_a_newer_one_never_regresses_the_file`, `a_metadata_write_starts_within_the_flush_bound`, `metadata_admission_respects_the_byte_budget_and_reservations` | none required |
| A07 | `a_stop_of_generation_one_never_writes_exited_into_generation_two`, `a_stop_racing_a_natural_exit_settles_on_a_single_confirmed_exit` | none required |
| A08 | `a_restart_whose_persist_fails_restores_the_prior_record_without_a_stranded_start`, `concurrent_restarts_of_one_generation_commit_at_most_one_new_generation` | D-10 |
| A09 | `a_startup_deadline_then_cancellation_retains_the_late_child`, `a_cancelled_launch_terminates_the_late_child_instead_of_publishing_it`, `h02_a_requester_that_disconnects_mid_create_leaves_the_host_owning_the_child` | none required |
| A10 | `a_delayed_seed_or_marker_write_never_recreates_a_removed_session_directory`, `a_revision_queued_before_removal_cannot_resurrect_the_record`, `a_duplicate_hook_retries_failed_seed_persistence_without_another_notification` | none required |
| A11 | `a_child_wait_failure_stays_unverified_without_a_fabricated_exit`, `an_unverifiable_pid_stays_non_resumable_and_is_never_signaled` | process-safety cells |
| A12 | `every_retained_end_state_restores_without_creating_a_process`, `restoration_and_stale_restart_do_not_launch_through_host_ipc`, `a_requested_session_id_is_created_once_and_never_silently_replaced` | D-01, D-07 |
| A13 | `a_stop_all_outcome_separates_unresolved_ownership_from_durability_failures`, `shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds`, `a_forced_shutdown_with_unresolved_ownership_keeps_the_host_serving` | D-09, D-10 |
| A14 | `an_unknown_agent_state_asks_instead_of_passing_for_an_idle_shell`, `an_unreachable_host_never_asks_and_never_stops_blindly` | D-08 |
| A15 | `every_unattached_session_is_listed_exactly_once_across_workspaces_and_the_fallback_group`, `a_fallback_workspace_prefers_the_recorded_cwd_and_falls_back_to_home` | D-04, D-05 |
| A16 | `natural_exit_retains_the_final_view_on_every_branch`, `an_exit_without_output_ends_the_follower_and_leaves_an_empty_completed_text` | D-06 |
| A17 | `a_scan_waiting_to_persist_cannot_overwrite_a_restarted_generation`, `a_marker_captured_under_generation_one_is_dropped_once_generation_two_runs`, `a_scan_waiting_to_persist_cannot_recreate_a_removed_record` | none required |
| A18 | `h01_root_exit_and_pty_hangup_do_not_prove_descendant_exit`, `descendants_remain_recoverable_after_the_parent_exits`, `a_held_open_descendant_bounds_the_final_drain_and_marks_the_output_incomplete`, `a_descendant_started_just_before_the_root_exits_stays_owned_until_the_stop` (Linux) | D-09, Linux L06 |

### Functional requirements

| FR | Automated cases | Platform evidence |
|---|---|---|
| FR-01 | `a_requested_session_id_is_created_once_and_never_silently_replaced`, `concurrent_restarts_of_one_generation_commit_at_most_one_new_generation`, `session_remove_refuses_a_live_session_and_deletes_an_ended_manifest` | none required |
| FR-02 | `h02_a_requester_that_disconnects_mid_create_leaves_the_host_owning_the_child`, `replacement_preflight_and_wrong_home_never_stop_live_sessions`, `repeated_disconnects_deliver_each_input_once_and_release_every_connection_thread` | D-01, D-02, D-11 |
| FR-03 | `a_session_runs_a_shell_answers_input_and_checkpoints_atomically`, `a_checkpoint_survives_a_partial_escape_sequence`, `output_before_the_tail_is_reported_as_evicted_not_fabricated`, W02 `W02.content_equivalence` | none required |
| FR-04 | `a_stop_of_generation_one_never_writes_exited_into_generation_two`, `a_marker_captured_under_generation_one_is_dropped_once_generation_two_runs` | none required |
| FR-05 | `a_child_wait_failure_stays_unverified_without_a_fabricated_exit`, `a_new_host_instance_marks_inherited_running_records_lost_and_never_signals_them`, `a_record_owned_by_a_previous_host_reconnects_as_host_replaced` | D-01 |
| FR-06 | `a_forced_shutdown_with_unresolved_ownership_keeps_the_host_serving`, `a_stop_all_outcome_separates_unresolved_ownership_from_durability_failures` | D-09 |
| FR-07 | the three `a_control_connection_idle_for_*` tests, `a_connection_lost_before_the_input_ack_reports_unknown_delivery_without_a_resend` | W08 idle control |
| FR-08 | `a_metadata_write_starts_within_the_flush_bound`, `a_stalled_exclusive_job_times_out_the_waiter_without_losing_the_queue`, `parallel_hook_commits_publish_in_revision_order_before_returning_acknowledgements` | none required |
| FR-09 | `a_natural_exit_keeps_a_cold_record_releases_the_runtime_and_removal_drops_the_text`, `final_text_keeps_its_tail_on_a_char_boundary`, `eviction_drops_the_oldest_final_output_first_and_keeps_identities` | D-06, D-07 |
| FR-10 | `the_allocation_never_exceeds_the_physical_budget_while_filling`, `a_child_that_stops_reading_its_input_never_starves_the_control_path`, `checkpoint_staging_admits_two_captures_and_the_third_waits_for_a_release`, `streaming_followers_leave_reserved_slots_for_control_requests`, `an_output_flood_reaches_every_follower_and_a_paused_follower_never_stalls_control`, `metadata_admission_respects_the_byte_budget_and_reservations` | W03, W05 |
| FR-11 | `a_session_finished_yesterday_is_forgotten_and_a_fresh_one_is_kept`, `a_session_the_machine_rebooted_under_is_kept_for_a_month`, `a_live_session_is_never_forgotten_however_old_it_is` | none required |
| FR-12 | `cargo test --workspace --locked` on all four triples in `run_tests.yml` | every matrix cell |
| FR-13 | `a_new_host_instance_marks_inherited_running_records_lost_and_never_signals_them`, `an_explicit_restart_starts_a_new_generation_as_an_ordinary_shell` | D-01 |
| FR-14 | `host_status_reports_queue_capacities_and_reserved_control_slots`, `a_stop_all_outcome_separates_unresolved_ownership_from_durability_failures` | none required |
| FR-15 | `every_retained_end_state_restores_without_creating_a_process`, `restoration_and_stale_restart_do_not_launch_through_host_ipc`, W04 `NFR-11.worker_cycles` | D-01, D-04, D-07 |
| FR-16 | `every_unattached_session_is_listed_exactly_once_across_workspaces_and_the_fallback_group`, `a_session_whose_workspace_is_gone_is_adopted_by_the_deepest_open_root_over_its_cwd` | D-04, D-05 |
| FR-17 | `an_unknown_agent_state_asks_instead_of_passing_for_an_idle_shell`, `an_update_restart_never_pretends_sessions_can_be_kept`, `shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds` | D-08, D-09, D-10 |

### Non-functional requirements

| NFR | Automated cases or harness decision | Platform evidence |
|---|---|---|
| NFR-01 | W01 CPU attribution | W01 short window at 50 sessions, per OS |
| NFR-02 | W01 wait-reason attribution, `a_follower_resumes_after_the_checkpoint_survives_idle_keepalives_and_sees_the_exit` | native trace per OS |
| NFR-03 | W01 memory per session | per-OS native allocation evidence |
| NFR-04 | `NFR-04.runtime_release`, `NFR-04.reclaim_max_ms`, `a_natural_exit_keeps_a_cold_record_releases_the_runtime_and_removal_drops_the_text` | none required |
| NFR-05 | `NFR-05.*` | full protocol per OS |
| NFR-06 | `NFR-06.checkpoint_release`, `a_staged_checkpoint_releases_its_admission_when_dropped`, `an_oversized_control_frame_is_rejected_without_buffering_it` | none required |
| NFR-07 | `a_metadata_write_starts_within_the_flush_bound`, `a_stalled_exclusive_job_times_out_the_waiter_without_losing_the_queue`, `a_failed_final_revision_is_retained_and_retried_after_storage_recovers`, `metadata_admission_respects_the_byte_budget_and_reservations` | D-10 |
| NFR-08 | `NFR-08.*` | full protocol per OS |
| NFR-09 | `NFR-09.*` | full protocol per OS |
| NFR-10 | `shutdown_deadline_is_shared_by_stalled_stops_and_keeps_inspection_responsive`, `a_startup_deadline_then_cancellation_retains_the_late_child`, `a_held_open_descendant_bounds_the_final_drain_and_marks_the_output_incomplete` | D-09 |
| NFR-11 | `NFR-11.worker_cycles`, `concurrent_restarts_of_one_generation_commit_at_most_one_new_generation`, the three idle-control tests | W08 100 desktop and 100 worker cycles |
| NFR-12 | `NFR-12.host_shutdown`, `NFR-12.fixture_orphans` | W08 endurance per OS |
| NFR-13 | `an_oversized_control_frame_is_rejected_without_buffering_it`, `the_named_pipe_acl_has_no_world_or_authenticated_user_grant`, `the_unix_socket_is_owner_read_write_only`, `a_served_endpoint_is_never_taken_over_while_a_stale_one_is_reclaimed`, `an_unverifiable_pid_stays_non_resumable_and_is_never_signaled` | process-safety cells, Linux L02, L05 |
| NFR-14 | this ledger | one row per required cell in each report |
| NFR-15 | `every_retained_end_state_restores_without_creating_a_process`, `a_stop_all_outcome_separates_unresolved_ownership_from_durability_failures`, `every_unattached_session_is_listed_exactly_once_across_workspaces_and_the_fallback_group` | D-01, D-04, D-09 |

### Unhappy paths

| Path | Automated cases | Platform evidence |
|---|---|---|
| 1 Empty home | `the_host_assigns_durable_ids_persists_manifests_and_stops_owned_processes` | first-launch cell |
| 2 Launch still pending | `admission_stops_at_eight_unresolved_launches`, `a_startup_deadline_then_cancellation_retains_the_late_child` | none required |
| 3 Host compatibility mismatch | `an_incompatible_or_missing_handshake_is_refused_before_any_effect`, `a_client_from_another_release_attaches_when_protocol_and_engine_agree` | upgrade cells |
| 4 Replacement unavailable | `replacement_preflight_and_wrong_home_never_stop_live_sessions`, `an_update_preflight_defers_a_missing_replacement_and_counts_the_host_sessions` | upgrade cells |
| 5 Restart persistence failure | `a_restart_whose_persist_fails_restores_the_prior_record_without_a_stranded_start` | D-10 |
| 6 Ambiguous input delivery | `a_connection_lost_before_the_input_ack_reports_unknown_delivery_without_a_resend` | none required |
| 7 Idle control | the three `a_control_connection_idle_for_*` tests | W08 |
| 8 Slow output client | `an_output_flood_reaches_every_follower_and_a_paused_follower_never_stalls_control`, `paused_follower_probe` | none required |
| 9 Output eviction | `output_before_the_tail_is_reported_as_evicted_not_fabricated` | none required |
| 10 Stop cannot be confirmed | `a_child_wait_failure_stays_unverified_without_a_fabricated_exit`, `a_forced_shutdown_with_unresolved_ownership_keeps_the_host_serving` | D-09 |
| 11 Child exits before EOF | `natural_exit_drains_the_final_output_before_the_stream_ends`, `a_held_open_descendant_bounds_the_final_drain_and_marks_the_output_incomplete` | none required |
| 12 Old callback/write | `a_scan_waiting_to_persist_cannot_overwrite_a_restarted_generation`, `an_older_revision_queued_behind_a_newer_one_never_regresses_the_file` | none required |
| 13 Host dies | `a_new_host_instance_marks_inherited_running_records_lost_and_never_signals_them`, `every_retained_end_state_restores_without_creating_a_process` | D-01 |
| 14 Cold text unavailable | `a_lost_data_directory_reports_unavailable_text_without_blocking_retirement`, `a_missing_data_directory_refuses_the_write_instead_of_recreating_it` | D-07 |
| 15 Over limit | `an_oversized_control_frame_is_rejected_without_buffering_it`, `oversized_terminal_dimensions_are_refused_before_reaching_the_engine`, `admission_stops_at_eight_unresolved_launches`, `accepted_hook_frames_fit_the_reload_limit_and_oversized_events_never_commit` | none required |
| 16 Concurrent create/restart/remove | `concurrent_restarts_of_one_generation_commit_at_most_one_new_generation`, `a_requested_session_id_is_created_once_and_never_silently_replaced` | none required |
| 17 Permission revoked | `a_failed_final_revision_is_retained_and_retried_after_storage_recovers`, `a_lost_data_directory_reports_unavailable_text_without_blocking_retirement` | D-10 |
| 18 Unknown agent during close | `an_unknown_agent_state_asks_instead_of_passing_for_an_idle_shell` | D-08 |
| 19 Rental unavailable | none; recorded as `unavailable` in the macOS report | MAC cells |
| 20 Runtime panic | `a_panicked_runtime_keeps_ownership_until_a_stop_confirms_the_exit`, `a_runtime_panic_keeps_the_child_owned_until_an_explicit_stop_confirms_its_exit` | none required |
| 21 Restore absent/ended session | `every_retained_end_state_restores_without_creating_a_process`, `a_pane_restored_into_an_ended_session_resumes_without_an_attachment` | D-07 |
| 22 Stop-all is incomplete | `a_stop_all_outcome_separates_unresolved_ownership_from_durability_failures`, `shutdown_keeps_unsaved_final_state_owned_until_retry_succeeds` | D-09, D-10 |
| 23 Kept-running workspace unavailable | `a_session_whose_workspace_is_gone_is_adopted_by_the_deepest_open_root_over_its_cwd`, `a_fallback_workspace_prefers_the_recorded_cwd_and_falls_back_to_home` | D-04, D-05 |
| 24 Fallback attachment cannot open | `a_fallback_workspace_prefers_the_recorded_cwd_and_falls_back_to_home` | D-05 |
| 25 Natural exit | `natural_exit_retains_the_final_view_on_every_branch`, `a_natural_exit_keeps_a_cold_record_releases_the_runtime_and_removal_drops_the_text` | D-06 |
| 26 Worker restarts | `workload_worker_replacement` (`NFR-11.worker_cycles`), `a_host_restart_marks_a_busy_record_stale_instead_of_idle`, `revisioned_host_snapshots_recover_a_lost_notification_and_reject_queued_older_events` | D-02, D-03 |
| 27 Detached list fails | `an_unreadable_record_is_skipped_and_the_rest_of_the_listing_survives` | none required |
| 28 Occupied endpoint | `a_served_endpoint_is_never_taken_over_while_a_stale_one_is_reclaimed` | Linux L05 |

## Agent Runtime System integration

The worker (`crates/paneflow-serve`) is the Agent Runtime System's consumer of
host state. Its tracker is not edited by this qualification. The contract both
sides already share is the one qualified here: every host snapshot, agent
event, viewport scan, and cancellation marker carries the session generation
and a revision; the worker refuses older revisions and rebuilds its projection
from the accepted state (`revisioned_host_snapshots_recover_a_lost_notification_and_reject_queued_older_events`,
`an_event_from_a_generation_the_session_has_left_is_refused`). Desktop, host,
worker, and the packaged update routes report identity and uncertainty through
the same `SessionLifecycle` and reconnection states, so the D-02 and D-03
cells and the W04 worker decision are mandatory candidate evidence, not a
separate PRD's certification.
