# Performance benchmarks

`bench/` holds the reproducible measurements behind Paneflow's performance
claims. Every number published about the terminal pipeline or the code editor
comes from one of the suites below, run with the scripts described here, and
the raw result of each run is archived next to the baseline it is compared
against.

There are four suites, one baseline per suite and per platform, and four
result prefixes:

| Suite | Test | Script | Baseline | Result files |
|---|---|---|---|---|
| `paneflow-terminal-bench` | `terminal::perf_bench::terminal_pipeline_benchmark` | `scripts/bench-terminal.sh` / `.ps1` | `bench/baselines/<os>-<arch>/terminal.json` | `bench/results/<stamp>-<sha>.json` |
| `paneflow-editor-bench` | `app::diff_dock::code::perf_bench::editor_pipeline_benchmark` | `scripts/bench-editor.sh` / `.ps1` | `bench/baselines/<os>-<arch>/editor.json` | `bench/results/editor-<stamp>-<sha>.json` |
| `paneflow-startup-bench` | `startup_bench::startup_first_frame_benchmark` | `scripts/bench-startup.sh` / `.ps1` | `bench/baselines/<os>-<arch>/startup.json` | `bench/results/startup-<stamp>-<sha>.json` |
| `paneflow-persistent-bench` | `tests/persistent_baseline.rs::persistent_session_baseline` (crate `paneflow-host`) | `scripts/bench-persistent.sh` / `.ps1` | `bench/baselines/<os>-<arch>/persistent.json` | `bench/results/persistent-<stamp>-<sha>.json` |

### Baselines per platform

`<os>-<arch>` is Rust's `std::env::consts::OS` and `ARCH` of the build that
runs the suite: `linux-x86_64`, `windows-x86_64`, `macos-aarch64`. Every
result records it as `platform`. A suite compares only against the baseline
of its own platform, resolved under `PANEFLOW_BENCH_BASELINE_DIR` (the
scripts set it to `bench/baselines`; without it the suite falls back to the
repository's `bench/baselines`). With no baseline for the platform it prints
`No baseline for macos-aarch64.` (`no baseline for macos-aarch64; thresholds
only` in the persistent suite) and drops the comparison columns: it never
falls back to another platform. A baseline file whose recorded `os` and
`arch` differ from the run, or whose schema is not the one the run writes, is
refused with a message naming the file, never compared.

`--set-baseline` (`-SetBaseline`) refuses a dirty tracked worktree before
running, with the command that shows why (`git status --porcelain
--untracked-files=no`): a baseline must name a commit that exists. The
persistent suite also refuses untracked files, because its source
fingerprint counts them and would record the baseline as dirty. Commit the
change first, then record the baseline and commit it on top.

The non-ignored tests
`bench_harness::tests::every_committed_baseline_is_clean_current_and_on_its_platform`
(crate `paneflow-app`) and
`every_committed_persistent_baseline_is_clean_current_and_on_its_platform`
(crate `paneflow-host`, `tests/persistent_baseline.rs`) read every file under
`bench/baselines/` and fail, naming the file, on a schema other than the
current one (`schema` 2 for the shared harness, `schema_version` 4 for the
persistent suite), on a dirty source (`git_dirty` other than `"false"`,
`diff.dirty` other than `false`), on a recorded platform other than the
directory it sits in, or on an unknown file name.

The allocation baselines of the performance gates,
`bench/baselines/linux-x86_64/{terminal,editor}-alloc.json`, follow the same
rules: the gate runs only on Linux x86_64.

Every baseline committed before 2026-10-06 was retired: the four
`bench/*baseline.json` files were Windows runs, three from dirty trees, the
startup one recorded before its optimization, the persistent one at schema 2,
and their CPU field read `windows-x86_64` instead of a model. They remain in
git history (`git show 0a74eb30:bench/baseline.json`) and their raw results
stay in `bench/results/`.

Current state, 2026-10-06. `linux-x86_64` holds `terminal`, `editor` and
`startup`, recorded on `9ecb73a6` on Arthur's Fedora 44 machine (Ryzen 7
7800X3D, CPU share 0.99 and 1.00; the startup suite's stale socket step is
0.2 ms), the two allocation baselines recorded on `acf739ca`, whose
allocation columns match the previous ones exactly, and `persistent`, recorded
on `762ac880` ([result](results/persistent-20261006T123356Z-762ac8804c0a.json)).
The first persistent run, on `acf739ca`
([result](results/persistent-20261006T103639Z-acf739ca51a9.json)), failed
`NFR-04.runtime_release` (one W05 batch never reported zero live runtimes) and
`NFR-12.host_shutdown` (the host did not exit after an acknowledged shutdown),
and its `--prior` rerun on `9ecb73a6`
([result](results/persistent-20261006T104552Z-9ecb73a66619.json)) kept that
failure, so the script refused it. `762ac880` fixed the cause, a recycled
descendant pid the Unix process tree owner retained forever, and the run on it
passed both thresholds. No `windows-x86_64` or `macos-aarch64` baseline exists
yet; the Windows ones are recorded on the dual boot machine with the `.ps1`
scripts.

The first three suites share one harness, `src-app/src/bench_harness.rs`: the metric
type, the timing helpers, the JSON document, the comparison table, and the
single `#[global_allocator]` the test binary installs. That allocator counts
allocated bytes, allocation calls, and live bytes (allocations minus
deallocations), which is how a retained-memory metric can be reported at all.
It exists only in `cfg(test)` builds.

## Performance gates

The `perf_gates` job of `.github/workflows/run_tests.yml` turns the work
counters and the allocation columns into budgets that block a pull request.
It runs on every `pull_request` and every `push` to `main`, is part of
`tests_pass`, and reproduces locally with one command:

```bash
scripts/perf-gates.sh                           # needs Xvfb, xdotool and Mesa lavapipe; XVFB=<path> overrides the Xvfb binary
scripts/perf-gates.sh --refresh-alloc-baselines # rewrites bench/baselines/linux-x86_64/{terminal,editor}-alloc.json; refused from a dirty tree
```

The script builds the `gates` profile (release with 16 codegen units, which
leaves every gated counter and allocation column unchanged and shortens a
cold build by about a quarter), runs the terminal and editor suites,
the hook burst test and the startup suite, starts its own Xvfb with the
lavapipe ICD forced, then runs the ignored test `perf_gates` in
`crates/paneflow-host/tests/persistent_baseline.rs`. That test drives the real
host, worker and desktop through four scenarios: 8 idle sessions and 8
`stream 16384 60` sessions for the host and the worker (30 s windows), then
the desktop idle with 4 panes and the cursor blink on, idle again once
`xdotool` gives its window the X input focus (Xvfb runs no window manager, so
the window is otherwise never active, and a terminal in an inactive window
does not blink), with one agent thinking in that focused window, and on a
workspace whose git repository does not change (35 s window, one 30 s git
poll). It
reads the other suites' results from `target/perf-gates/` and judges every
budget.

Every budget is a constant in one module,
`crates/paneflow-host/tests/persistent_baseline/gates.rs`: its name, its
value, its unit, the scenario that measures it and the margin that justifies
it. The allocation columns (`alloc_bytes_per_iter`, `allocs_per_iter`) of
every terminal and editor metric are judged against the committed Linux
baselines within 1 % either way: a metric that drops by more than 1 % fails
too, with "below the baseline, refresh it in this PR", so no improvement
stays out of the baseline. `gate_trickle_publishes` is judged on its exact
value. The suites' timing columns stay informative. Two runs of the same
commit on Arthur's Fedora machine gave identical allocation columns. The first
CI run of the job (run 37231988624) matched every terminal column and every
editor column except the three shaping metrics (`shape_cold_60_rows`,
`shape_warm_60_rows`, `prepaint_60_rows_warm`), off by up to +20 800 %. The
editor suite asked for the embedded editor font without registering it, so
GPUI shaped with whatever fallback the machine had installed: Fedora (with
JetBrainsMono Nerd Font installed), Ubuntu 26.04 under WSL and the runner gave
three different values. The suite now registers the embedded fonts, as the app
does at startup, and the same commit under WSL, which has no JetBrains font,
then reproduced all 44 editor allocation columns of the Fedora baseline
exactly. No per-metric tolerance is needed.

A failure prints one line per failed budget,
`counter | measured | budget | excess | scenario`, then the local command
that reproduces it, and the job writes the full table to
`$GITHUB_STEP_SUMMARY`. The JSON report `target/perf-gates/perf-gates.json`
is written before any assertion and uploaded with the suite results and logs
as the `perf-gates` artifact (14 days), whatever the outcome. A measurement
that is missing (a dead fixture, a `pending` counter, a window across a
restart, a suite that wrote no result, a desktop or Vulkan adapter that did
not start) fails its budget with the reason: it is never read as within
budget. A metric the suite itself reports `available: false` (no real shaper
on the machine) is listed as not measured, neither accepted nor a
regression. The non-ignored tests `gates::` and `gate_runs::` of the same
binary run in the job before the measurement: they feed the verifier a
synthetic overrun and check the job's workflow (no `pull_request_target` in
any workflow, a `perf-gates-` cache prefix that `release.yml` never uses,
`tests_pass` depending on the job).

The first local run of the gates, on Arthur's Fedora machine under Xvfb with
lavapipe, failed one budget: with 8 printing sessions the host broadcast 2.97
`session` frames per second and per session, against the PRD bound of 2. On
a 500 ms scan tick where the screen changed, the viewport scan announced the
session for its new terminal signals, then `commit_scan` persisted the screen
stamp and `persist` announced it again. The scan now announces a session once
per tick: it skips the signals announcement when its own commit already
announced. The rerun measured 1.97, and every other budget passed both runs
(local evidence in `tasks/perf-gates-ep003/`, not tracked).

The first real hardware run (2026-10-06) found two costs the gates could not
see. The desktop ran a diff-stat probe about every 680 ms in a repository that
did not change: `notify` 7 reports `IN_OPEN` on Linux, the git watcher took
any event on `HEAD` or `index` as a change, and each probe opened both
files, so it triggered the next one. The gate measured 15 probes in its 35 s
window but only judged the 3 processes per probe. The watcher now ignores
access events, and `desktop.diff_stat.probes` bounds the probe count. The
second cost was the cursor blink: a focused window drew 1.85 root renders per
second at rest and 13.0 while an agent thought, because the 530 ms blink and
the 90 ms spinner drew separate frames. The blink now toggles every 540 ms on
the spinner's own grid (`ui_primitives::animation_clock`), so its toggles
land in spinner frames, and the gate measures the focused window
(`desktop.focused_idle.root_renders_per_s`, then the thinking state).
Measuring it found a third cost: a terminal counted as focused whenever its
pane held the window's focus, even in an inactive window, so every inactive
window kept blinking. With the blink on, a window that never took the X focus
drew 56 root renders in the 30 s idle window. A terminal now counts as
focused only while its window is active; under Xvfb the inactive window drew
0, the focused one 1.87 per second, and the thinking state 11.10 per second
(local Xvfb, 2026-10-06).

Mutation proof for `worker.idle.snapshot_broadcasts`: with the unchanged
snapshot check of `Worker::broadcast_snapshot_if_changed` disabled, the gate
measured 15 snapshots in the 30 s idle window against a budget of 0 and
failed with
`worker.idle.snapshot_broadcasts | 15 snapshots per window | = 0 snapshots per window | +15 | host_worker_idle: ...`
(Ubuntu 26.04 under WSL, release build of `26430c1b`).

The job was soaked on GitHub runners before it became blocking, by rerunning it
on `main` and on five dispatch branches. Earlier soaks found three parasitic
failures, each fixed at its source rather than by widening a budget: a real
`fsync` stall on the runner disk pushed `hooks.burst.p95` to 820 ms (the
hook burst now keeps its home on tmpfs, so only the injected durability
latency counts), `worker.idle.cpu_ms` sat at its bound because the idle
worker polled for shutdown (it now blocks on a condition variable), and a
stalled apt mirror plus a cold release build took one run to 28 minutes (apt
retries, and the `gates` profile). On `2bb833e0` the job then passed 32
consecutive runs out of 32, with no rerun of a failure: 12 to 16 minutes with
a warm cache, 19 to 25 minutes cold, inside the 30 minute bound.

## Real-time A/B

The gates above count work and never time it. Real time, CPU included, is
compared by an A/B that builds the two commits on the same machine in the same
run, so a shared runner's speed cancels out:

```bash
scripts/perf-ab.sh <base-commit> <head-commit>   # or scripts/perf-ab.ps1 <base> <head>
```

The script checks out each commit in a detached worktree under a scratch
directory outside the repository, copies the untracked native archives
(`native/*/prebuilt`, verified again by each worktree's
`scripts/fetch-libghostty.sh`), and builds both in the `gates` profile into one
shared `CARGO_TARGET_DIR`, so the dependencies compile once. A
`CARGO_TARGET_DIR` already set by the caller is used and kept, which is how
the CI job caches dependencies; otherwise the build tree lives in the scratch
directory. The worktrees and the scratch directory are removed at the end,
whatever the outcome. The script refuses to run when the working tree has
uncommitted changes and either commit resolves to its `HEAD`: it measures
commits, never a dirty tree.

Each attempt runs 10 rounds ordered base, head, head, base. A slot runs the
artifact's own terminal suite (`terminal_pipeline_benchmark`, idle scenarios
skipped), which writes every per-iteration sample to
`PANEFLOW_BENCH_SAMPLES_OUT`, then the active scenario of US-002 against the
artifact's own `paneflow-host`, host only: 8 `stream 16384 60` sessions plus one
`flood 8388608`, 4 s of settling, a 15 s window cut into 5 slices of 3 s, then
200 echo round trips (`ab::perf_ab_active_samples`). The measuring harness and
the fixture come from the head commit, so both hosts face the same workload
and the same measurement code. The metrics are:

| Metric | Unit | Samples per artifact and attempt |
|---|---|---|
| `terminal.<metric>`, every timed terminal metric | ns per iteration, as the mean of 10 consecutive iterations | 30 per slot, 600 |
| `active.cpu.<role>`, host CPU per named thread role, and `active.cpu.total` | CPU ms per wall second | 5 per slot, 100 |
| `active.echo_round_trip` | ms | 200 per slot, 4 000 |

The terminal suite exports each iteration in measurement order, and the
comparator averages 10 consecutive iterations into one sample, as Criterion
does: on a first A/A run on a loaded desktop, the p50 of single iterations moved
by at most 0.6 % between the two base passes but their p95 by up to 57 %, one
preempted iteration being enough to set a pooled tail. The comparator
(`ab::perf_ab_compare`) pools each cohort and applies the rule
of pf (`pf/scripts/pgso/qualify.py:42,365-392`), nearest-rank percentiles: a
metric regresses when the head's p50 exceeds the base's by more than 10 %, or
its p95 does where that p95 is judged. The A/A cohort is the first base pass of
every round against its last base pass, measured interleaved with the head, so
it sees the same neighbors and the same thermal drift; a separate A/A pass
would not fit the 60 minute budget. The run is rejected as uncalibrated when
any metric's A/A p50 moves by more than 5 %. A metric's p95 is judged only when
its own A/A p95 moves by at most 5 %; otherwise the row reads `p95 not judged`
and only its p50 decides, because one preempted batch on a shared runner sets a
pooled tail (see the evidence below). Every cohort needs at least 50 samples, and a run at
least 10 rounds; below that, or when a suite compares no metric, the verdict
is `insufficient`, never a pass. A value under the measurement resolution of
its unit is compared at that resolution, so an idle thread that wakes once is
not a +300 % regression and two idle threads never divide by zero: 100 ns for
the terminal iterations (timer overhead), 1 CPU ms per second (0.1 % of a core)
for the thread roles, 0.01 ms for the echo. A metric measured by one side only
(added or removed by the change) is listed as not compared.

A first attempt that ends in a regression or uncalibrated is measured once
more with the same builds before any verdict, and only two calibrated
regressions in a row make a regression:

| Attempts | Verdict | Exit | `promotion.effect` |
|---|---|---|---|
| pass | `pass` | 0 | `calibrated` |
| regression, regression | `regression` | 1 | `calibrated` |
| regression, pass | `unconfirmed_regression` | 0 | `resets` |
| uncalibrated, pass | `pass` | 0 | `uncalibrated` |
| uncalibrated, regression | `unconfirmed_regression` | 0 | `uncalibrated` |
| uncalibrated, uncalibrated, or regression, uncalibrated | `uncalibrated` | 5 | `uncalibrated` |
| a missing measurement | `insufficient` | 2 | `excluded` |

Other exits: 2 for a usage error, a dirty tree or an unexpected failure, 3 when
the base is unavailable and 4 when the head is (it does not build, its
terminal suite writes no samples, or its host fails a slot). The first error
lines of the build log follow the message. Both commits must carry the sample
export and the `gates` profile, that is this change or later: an older base,
such as a release before it, is reported unavailable rather than measured
differently.

The output lands in `target/perf-ab/` (`PANEFLOW_PERF_AB_DIR` overrides it):
`result.json` with both distributions of every metric (all samples), the
deltas, the A/A drift, the verdict and the identity of both commits and the
machine; `summary.md` drawn from it; `verdict`; the build logs; and every
slot's samples and log under `attempt-<n>/`.

### Instruction counts

After both builds and before the timed rounds, `scripts/perf-ab.sh` counts the
instructions of the pure CPU paths with Gungraun (Callgrind `Ir`): each
commit's `instructions` benchmarks in `paneflow-agent-config` (20 screen rules
on a 200x60 screen) and `paneflow-terminal-ghostty` (parse and conversion of a
1 MiB corpus through the statically linked libghostty). The base saves a
Gungraun baseline (`--save-baseline=base`) in the scratch directory, and the
head is compared against it (`--baseline=base`) with a soft limit of +2 %.
Instruction counts are deterministic, so one execution is a verdict: a
benchmark above the limit writes `regression` to `instructions-verdict` and
turns a timed `pass` or `unconfirmed_regression` into exit 1. The workflow
reads that file whatever the timed outcome, so an uncalibrated run or an
unavailable slot never hides an instruction regression. The layout pass is not counted: it lives
in the `paneflow-app` binary crate, which has no library target a benchmark
could call, and the PRD keeps it out of scope until that crate gains one.

The counts need Valgrind and a `gungraun-runner` of the same version as the
`gungraun` library in `Cargo.lock` (`cargo install gungraun-runner --version
0.20.0 --locked`). Without them, or when the base predates the benchmarks, the
counts are reported `not_measured` with the reason, never as zero, and the
timed verdict stands alone. `scripts/perf-ab.ps1` always reports them
`not_measured`: Valgrind runs on Linux only. The results land in
`instructions.json` (base and head `Ir`, delta and verdict per benchmark) and
in an "Instruction counts" section appended to `summary.md`. The `bench`
profile keeps its symbols (`strip = false`), because Callgrind finds the
benchmark function by name and a stripped binary counts zero instructions.

The US-017 spike validated the counts before they joined the A/B. The
`workflow_dispatch` workflow `.github/workflows/gungraun-spike.yml` installs
Valgrind and the matching `gungraun-runner`, and `scripts/gungraun-spike.sh`
runs both benchmarks twice on the same commit; it validates when every count is
non-zero and the two runs agree within 0.1 %, and otherwise concludes "not
validated" with the reasons and the tail of the error log, for instance when
Valgrind cannot execute libghostty's code. Run 37369490241 on `ubuntu-24.04`,
2026-10-05, Valgrind 3.22.0, gungraun 0.20.0, cold cache: validated in 7 min
44 s for the whole job (limit 15 min). Its second attempt, a separate job with
a warm cache, took 1 min 9 s and reported the same counts.

| Benchmark | Run 1 (Ir) | Run 2 (Ir) | Drift |
|---|---|---|---|
| `screen_rules::evaluate_rules.twenty_rules_200x60` | 2 517 984 | 2 518 031 | 0.0019 % |
| `terminal::parse_and_convert.mebibyte_220x60` | 119 529 927 | 119 529 927 | 0 % |

The same runs on Fedora 44 with Valgrind 3.27.1 gave 2 440 096 and 119 009 924:
a count depends on the machine (glibc, CPU dispatch), which is why the A/B only
ever compares a base and a head counted in the same job. A seeded change that
sums the screen bytes ten times per evaluation came out at +11.7 % on the rules
benchmark and +0.000 % on the untouched parse benchmark (local evidence, not
tracked).

### Shadow mode and promotion

`.github/workflows/perf-ab.yml` runs the A/B on `ubuntu-24.04` with a 60 minute
bound: on pull requests that touch `src-app/src/terminal/**`,
`src-app/src/app/**`, `crates/paneflow-host/**`, `crates/paneflow-serve/**` or
`Cargo.lock` (base: the first parent of the merge commit), every night on
`main` against the latest published release, and on `workflow_dispatch` with
optional base and head. It triggers on `pull_request`, never
`pull_request_target`, reads with `contents: read` only, and caches under the
`perf-ab-` prefix, which `release.yml` never uses. The summary is appended to
`$GITHUB_STEP_SUMMARY` and `target/perf-ab/` is uploaded as the `perf-ab`
artifact for 90 days, longer than the three-week promotion window, so every
run the promotion counts can still be downloaded when it is decided. Runs
uploaded before this change kept 14 days: the runs of 2026-10-05 and
2026-10-06 cited below expire on 2026-10-19 and 2026-10-20.

Two workflow variables decide what fails the job, and only an execution
failure (exit 2 or 4) turns it red otherwise:

- `PERF_AB_INSTRUCTIONS_BLOCKING: "true"`: an instruction regression fails the
  job. Promotion criterion: 10 consecutive CI runs in which the counts were
  measured and none flagged a regression on a pair whose benchmarked code did
  not change. It was met on 2026-10-06 by the runs below, and one commit set
  the variable to `"true"`.
- `PERF_AB_BLOCKING: "false"`: the real-time verdict stays in shadow mode, green
  whatever it says. An unavailable base or a twice uncalibrated run adds a
  warning. It stays consultative: see the decision below.

A `workflow_dispatch` run has its own concurrency group, so manual runs with
different commits proceed in parallel instead of replacing each other; pull
requests still cancel their superseded runs. The nightly run reports the base
unavailable until a release carries the sample export.

Real-time promotion criterion: 30 consecutive runs over at least 3 weeks, the
A/A p50s calibrated on at least 90 % of them, and no regression verdict that a
second execution did not confirm. Each run states its effect in `result.json`
(`promotion.effect`) and at the top of its summary. A `calibrated` run counts.
An `uncalibrated` run (one of its executions was rerun because an A/A p50
drifted) counts among the 30 but against the 90 % only, never as a failure. An
`unconfirmed_regression` (`resets`) restarts the count. An `excluded` run (a
missing measurement) and a run whose base was unavailable are not runs of the
gate and are skipped. The promotion is one pull request that sets
`PERF_AB_BLOCKING` to `"true"` and cites the 30 runs; from then on a confirmed
regression fails the job.

`scripts/perf-ab-promotion.sh` makes the count. It lists the completed runs of
`perf-ab.yml` with `gh`, downloads each `perf-ab` artifact once into
`target/perf-ab-promotion/runs/<run>/` (`PANEFLOW_AB_PROMOTION_DIR` overrides
the directory), and runs the ignored test `ab::perf_ab_promotion`, which writes
`promotion.json` and `promotion.md`. The test orders the counted runs by the
stamp of their `result.json`, judges the latest 30, and decides:

| Decision | When |
|---|---|
| `pending` | fewer than 30 counted runs, or 30 clean ones spanning less than 21 days |
| `promote` | the latest 30 hold no reset, at least 27 calibrated, over at least 21 days |
| `keep_consultative` | 30 counted runs or more, and the latest 30 hold a reset or fewer than 27 calibrated |
| `incomplete` | an artifact expired before it was downloaded and its run may fall within the latest 30, so no decision is possible |

A run without `result.json` (its base or head was unavailable, or it stopped
before the A/B), with `promotion.effect` `excluded`, or measured under an
older `schema_version` is listed as skipped with its reason, and so is a run
whose artifact expired after it finished before the latest 30 counted runs,
since it cannot change the decision. The rates it
reports, over all counted runs and over the latest 30, are the ones a
`keep_consultative` decision records here.

Decision of 2026-10-06: the real-time verdict stays consultative and
`PERF_AB_BLOCKING` stays `"false"`. The criterion can no longer be met by the
first 30 counted runs, so waiting for the thirtieth would not change the
outcome. The count of that day (`scripts/perf-ab-promotion.sh`, which still
reports `pending` because it decides only at 30 runs) holds 11 counted runs:
7 calibrated (64 %), 2 uncalibrated, and 2 unconfirmed regressions (18 %),
both parasitic p95 regressions on identical terminal code (see the runs
below). Those 2 resets already rule out 30 runs without one, and 4 runs that
are not calibrated leave at most 26 calibrated of 30 where 27 are needed. The
nightly run 37448250584 was skipped (base `v0.17.5` unavailable), as were the
two schema 1 runs of 2026-10-05.

The question reopens only with a comparator that keeps the p95 noise from
producing parasitic regressions. Such a change raises `AB_SCHEMA_VERSION`, so
the count starts over on a fresh window and runs of the current rule are
skipped.

### Instruction promotion runs

Ten `workflow_dispatch` runs on 2026-10-06, all started at 06:29 UTC on
`ubuntu-24.04` with the comparator of `0f4b5995` as head. None of the bases
changes the benchmarked code, so any instruction regression would have been a
false positive. None occurred: the largest drift is 0.0037 % on the rules
benchmark, about 540 times under the +2 % limit, and the parse benchmark
never moved by a single instruction (119 529 927 `Ir` in every run).

| Run | Base | Real-time verdict | `promotion.effect` | Rules `Ir` delta | Parse `Ir` delta | Duration |
|---|---|---|---|---|---|---|
| 37423974914 | `3e582a2d` | `uncalibrated` | `uncalibrated` | 0 % | 0 % | 49 min |
| 37423980823 | `161f19ea` | `unconfirmed_regression` | `resets` | 0 % | 0 % | 45 min |
| 37423986771 | `53ada3a6` | `pass` | `calibrated` | 0 % | 0 % | 33 min |
| 37423992357 | `0f4b5995` | `pass` | `calibrated` | -0.0019 % | 0 % | 33 min |
| 37423998405 | `0f4b5995` | `pass` | `calibrated` | -0.0019 % | 0 % | 33 min |
| 37424003492 | `0f4b5995` | `pass` | `uncalibrated` | 0 % | 0 % | 50 min |
| 37424009047 | `0f4b5995` | `unconfirmed_regression` | `resets` | 0 % | 0 % | 50 min |
| 37424014354 | `0f4b5995` | `pass` | `calibrated` | -0.0037 % | 0 % | 33 min |
| 37424020097 | `0f4b5995` | `pass` | `calibrated` | 0 % | 0 % | 33 min |
| 37424026079 | `0f4b5995` | `pass` | `calibrated` | +0.0019 % | 0 % | 33 min |

A seeded failure then proved the blocking path. Run 37430358545, dispatched
after the promotion with base `0f4b5995` and a throwaway head that summed the
screen bytes ten times per evaluation, failed the job with exit 1: the rules
benchmark went from 2 517 984 to 2 803 309 `Ir` (+11.3 %), the parse benchmark
stayed at 119 529 927, and the real-time verdict was a calibrated `pass`, so
the instruction verdict alone turned the job red. The seed branch was deleted
afterwards.

The same runs are the first evidence for US-020. Six of ten were calibrated
passes. Two first attempts flagged a parasitic p95 regression on identical
terminal code, `terminal.publish_echo_220x60` p95 +14.1 % and
`terminal.layout_220x60` p95 +18.6 %, both with an A/A p95 inside 5 %; the
second execution did not confirm either, so neither became a verdict, but each
resets the real-time count. One run was uncalibrated on its first attempt only
(`active.echo_round_trip` A/A p50 +6.6 %), and the run against `3e582a2d` was
uncalibrated twice by terminal p50s drifting up to 10.9 %. A run with a rerun
took 45 to 50 minutes, inside the 60 minute bound.

### Calibration evidence

Three full runs of `scripts/perf-ab.sh` on 2026-10-05, Ubuntu 26.04 under WSL2
on Arthur's Ryzen 7 7800X3D with the Windows desktop in use (browser, several
agent sessions, about 23 % load), from a throwaway clone whose commits held this
change (local evidence, not tracked):

- A/A, base and head the same commit, single-iteration terminal samples: the
  first attempt was uncalibrated; the terminal p50s moved by at most 0.6 %
  between the two base passes but their p95s by up to 57 %. This is what led to
  the batches of 10 iterations, and it exposed that the export used to sort the
  samples, which batching cannot use.
- A/A with the batches, 2 157 s end to end including both builds and both
  attempts: uncalibrated twice (exit 5, effect `uncalibrated`). Every p50, every
  host CPU role and the echo stayed within 2.5 % between base passes (one
  exception, `active.cpu.host.viewport_scan` p50 +5.5 % in one attempt); the
  terminal p95s still drifted, up to 111 % for `service_spaces_8192`. The second
  attempt produced a parasitic regression, `terminal.layout_220x60` p95 +11.4 %
  on identical code, which the A/A rejection kept from becoming a verdict.
- Seeded regression, head burning 150 ns of session-thread CPU per output byte:
  the first attempt flagged `active.cpu.host.session` p50 +147 % and p95
  +102 %, and `active.cpu.total` p50 +92 %, with an A/A drift of 2.4 % on those
  metrics, while the echo stayed flat (+0.8 %). The attempt as a whole was still
  uncalibrated by terminal p95s, as above.

The first CI run, 37369016686 on `ubuntu-24.04` (2026-10-05, A/A on
`1d017391`, 2 713 s for the A/B step, 47 min for the job), settled the
question. Both attempts came out uncalibrated under the former rule, which
also bounded every A/A p95: every p50 stayed within 2.0 %, while p95s moved by
up to 55.1 % (`terminal.layout_220x60_acc60`), 18.6 %
(`active.cpu.merged_truncated_names`) and 11.5 % (`active.cpu.host.session`).
With that rule no run would reach the 90 % calibration of the promotion
criterion, so the A/A now bounds the p50s, and each p95 is judged only where
its own A/A holds.

## Screen rule corpus

`bench/screen-corpus-baseline.json` is not a timing baseline: it records how
the built-in screen rules classify every capture under
`runtimes/<slug>/fixtures/screens/`, with the accuracy and the states still
missing per runtime. `cargo test -p paneflow-agent-config` fails when the
classification drifts from it. After an intended rule or corpus change,
regenerate it with `PANEFLOW_SCREEN_CORPUS_BLESS=1 cargo test -p
paneflow-agent-config the_screen_corpus` and review the diff. The evaluation
cost is the ignored test
`twenty_rules_on_a_200_by_60_viewport_evaluate_within_a_millisecond_p95`, run
with `cargo test -p paneflow-agent-config --release -- --ignored`.

## Persistent session suite

The suite is the ignored integration test `persistent_session_baseline` in
`crates/paneflow-host/tests/persistent_baseline.rs`. It starts the real
detached host from the release build, drives it through the real IPC endpoint
and samples an empty topology, then opens 1, 10 and 50 sessions running
`paneflow-session-fixture idle`, the deterministic fixture executable of the
host crate at 80x24 (no shell syntax, no randomness). Every session attaches
through `session.attach` and follows `session.output` by default. The
`--no-followers` (`-NoFollowers`) option isolates the host-only or host+worker
topology for comparison; its record explicitly reports zero attachments.
With `--with-desktop`
(`-WithDesktop` on Windows), the native app restores the exact host session
identities from an isolated saved layout instead of using headless followers.
It verifies `surface.read` contains the fixture announcement for every pane
before sampling. One pane is visible; the other tabs are in the background.
The runner closes only its own desktop process after each sample, then verifies
the original host sessions remain live. On Windows it adjusts only that owned
window until the host reports a stable 80x24 visible grid. Every record includes
observed per-session dimensions. Each restored pane is then focused once
through the existing IPC so background panes adopt that calibrated size; the
first pane is made visible again before settling and sampling. Other native desktops currently record the
geometry deviation and require manual window calibration for exact W01 sizing.
For each scenario it settles for
four seconds, then samples per-thread CPU time of the host process over a ten
second window and attributes it by thread name: `host.session`,
`host.pty_reader`, `host.pty_writer`, `host.viewport_scan`,
`host.cancellation_scan`, `host.ipc_connection`, `host.ipc_accept`,
`host.launch_owner`, `host.main`, `host.other`. It also records the creation
time per session, real attachment latency and checkpoint size, the
`session.list` round trip and resident memory. A bounded paused-follower probe
records independent inspect and reattach latency while one receiver is paused.

The document records what a review needs to trust a number: commit, an FNV-1a
fingerprint of the uncommitted diff, OS, architecture, CPU model, logical CPU
count, RAM, rustc version, build profile, the terminal engine identity the
host reported, the PTY implementation, the fixture invocation and the scenario
list. The schema version is 4. Fingerprints include tracked and untracked
source contents; controller executable identity is recorded when supplied.
A fingerprint that changes between the start and the end of a run fails the
run: the result is not candidate-qualified.
`--with-worker` (`-WithWorker`) starts the existing `paneflow serve run` entry
point and attributes its CPU separately. `--with-desktop` includes the worker
and attributes native mirror, follower, and runtime threads separately from
the host viewport and cancellation scans. Cursor blinking and telemetry are
disabled in the isolated fixture configuration. Native desktop runs require a
working graphical session and display a benchmark window.

Anything the runner cannot measure is written as `pending` with the reason,
never as a zero. The default run has headless followers; worker and native
desktop numbers require their respective options. macOS attributes CPU per
thread through `proc_pidinfo` with full thread names. Linux reads each thread's
on-CPU time in nanoseconds from `/proc/<pid>/task/<tid>/schedstat` rather than
in 10 ms clock ticks from `stat`, and reports `pending` on a kernel built
without `CONFIG_SCHED_INFO`. Linux `comm` truncates names to 15 bytes; ambiguous
prefixes are reported as merged, never attributed to a guessed worker. Thread
CPU deltas aggregate duplicate names before subtracting the baseline. These
short samples establish a baseline, not the 300-second, three-repetition
performance acceptance gate. Native timer wake reasons, allocation ownership,
and complete W01-W08 qualification remain unmeasured unless a separate native
trace supplies that evidence.

The 2026-09-21 persistent result is a host-only historical sample. It does not
exercise attachment, a paused follower, a delayed spawn, or the desktop and
worker process set. Its diff fingerprint excludes untracked sources, and its
creation cost at 10/50 sessions divides the incremental batch by the cumulative
session count. New runs include untracked file contents in the fingerprint and
divide by the number actually created. The historical JSON is retained as
recorded and cannot certify the complete persistent path or native waiter
behavior on Linux and macOS.

Until 2026-10-06 the persistent baseline was the Windows native desktop run
from 2026-09-22, at schema 2 with a dirty source; it was retired with the
other pre-platform baselines (see "Baselines per platform"). Its companion runs isolate the [host](results/persistent-20260922T080613Z-d442894acbc8-host-only.json),
[host and worker](results/persistent-20260922T080711Z-d442894acbc8-host-worker.json),
and [host, worker, and desktop](results/persistent-20260922T080810Z-d442894acbc8-native-desktop.json).
All three measured the same source fingerprint `ee47b728e251ffc7`, with
0/1/10/50 sessions at an observed 80x24 and a passing paused-follower probe.
The native run restored all 50 existing sessions through the desktop. These
records establish a measured reference; they do not claim a performance
improvement or satisfy the longer performance acceptance window.
The [runtime identity receipt](results/persistent-20260922T081128Z-d442894acbc8-conpty-identity.json)
records Windows build 26200.9457, the engine revision, and the loaded ConPTY
1.24.260710001 module path, file version, and SHA-256 matching the pinned payload.
It was collected after all samples from a fresh isolated host using the same
release executable.

```bash
scripts/bench-persistent.sh                 # writes bench/results/persistent-<stamp>-<sha>.json and compares
scripts/bench-persistent.sh --set-baseline  # also copies the result to bench/baselines/<os>-<arch>/persistent.json
scripts/bench-persistent.sh --with-worker  # existing worker plus headless attachments
scripts/bench-persistent.sh --with-desktop # native desktop restoration and per-process CPU
scripts/bench-persistent.sh --no-followers # W01 host-only topology
scripts/bench-persistent.sh --active       # 1, 4 and 8 streaming sessions plus one flood, with the worker and work counters
scripts/bench-persistent.sh --with-worker --no-followers # W01 host+worker topology
scripts/bench-persistent.sh --quick        # smoke protocol: 5 s streams, 200 echo samples, 2 worker cycles
scripts/bench-persistent.sh --worker-replacement <paneflow-exe> # W04 build replacement with that worker binary
scripts/bench-persistent.sh --prior <result.json>  # rerun that keeps the earlier failures in the record
scripts/bench-persistent.sh --seed-failure # proves a failed decision exits nonzero and retains the artifact
scripts/bench-persistent.sh --prebuilt <dir> # runs a packaged harness and binaries without Cargo (see docs/release/persistent-qualification.md)
```

### Active agents scenario

`scripts/bench-persistent.sh --active` (`-Active` on Windows) runs the ignored
test `persistent_session_active` against the same release host, plus the
existing worker. For each of 1, 4 and 8 sessions it opens that many fresh
`paneflow-session-fixture stream 16384 60` sessions and one
`paneflow-session-fixture flood 8388608` session, waits 4 s, then samples a
30 s window. Each scenario records, for the host, the worker and, with
`--with-desktop`, the desktop: CPU per named thread, resident memory, and the
delta of every work counter below over the window (`work_counters`). The
desktop readiness check only waits for every surface to be listed, because a
streaming pane scrolls the fixture announcement away.

A stream session that is no longer live, or whose generation changed, at the
end of the window fails the scenario with the session id, and the scenario
publishes no sample computed over fewer sessions than planned. The flood
session is expected to finish; its state at the end of the window is recorded.
A scenario that opens no session fails with "no active session was opened". A
counter the process does not report is written as `{"pending": reason}`; the
comparison prints it as pending with its reason and never as an improvement. A
counter window that crosses a process restart is written as
`{"invalid": "<process> restarted during the measurement"}`.

An idle `paneflow-session-fixture echo` session stays open for the whole run.
After each counter window, while the streams still print, the harness sends it
200 numbered lines 10 ms apart through `session.input` and times each echo as a
follower receives it (`echo_ms`: median, p95, p99, raw samples). The round trip
crosses the host session thread twice, once for the PTY write and once for the
publication, so it is the latency a process listing on that thread would
inflate. The comparison prints the p95 per scenario next to the baseline. The
probe is idle during the counter window and does not move the counters.

The document is `bench/results/persistent-active-<stamp>-<sha>.json`, schema 4.
It compares against `bench/baselines/<os>-<arch>/persistent-active.json` when
that file exists, and `--set-baseline` writes it. A baseline of another schema is refused
with "baseline schema N differs from candidate schema 4; comparison refused,
record a new baseline"; the regular suite prints the same refusal.

The "before EP-002" reference is
[persistent-active-20261004T105029Z-eda8d59c1725.json](results/persistent-active-20261004T105029Z-eda8d59c1725.json):
`main` at `eda8d59c`, a clean tracked tree, no untracked file, release build,
measured under WSL2 on Arthur's machine (Ryzen 7 7800X3D, 16 logical CPUs)
before any EP-002 story. Over each 30 s window:

| Active sessions | `process_listings` | `foreground_observations` | `agent_bus_session_broadcasts` | worker `snapshot_broadcasts` | host CPU | host RSS |
|---|---|---|---|---|---|---|
| 1 | 54 | 60 | 90 | 14 | 0.47 % | 13 MiB |
| 4 | 216 | 236 | 356 | 15 | 1.73 % | 23 MiB |
| 8 | 432 | 472 | 712 | 15 | 3.47 % | 29 MiB |

Listings grow with each streaming session at about 1.8 per second (14.4 per
second at 8 sessions), and the worker broadcasts a full snapshot on every 2 s
sweep whatever the session count. The script took 103 s with an up-to-date build, 324 s including an
incremental release rebuild, and 388 s from a cold release build.

### EP-002 before and after

Both runs come from Arthur's Fedora machine (Ryzen 7 7800X3D, release build)
on 2026-10-04. "Before" is
[persistent-active-20261004T133907Z-09f86573d350.json](results/persistent-active-20261004T133907Z-09f86573d350.json),
`main` at `09f86573` with a clean tree. "After" is the same command on the
uncommitted EP-002 tree, so its document is local evidence
(`tasks/perf-gates-ep002/`, not tracked) and must be rerun on the commit
before it can become a baseline. Over each 30 s window at 8 active sessions:

| Counter or measure | Before | After |
|---|---|---|
| `process_listings` (whole host) | 432 (14.4 per second) | 57 (1.9 per second) |
| `foreground_observations` | 472 | 0 |
| worker `snapshot_broadcasts` | 15 | 0 |
| worker `projection_broadcasts` | 0 | 0 |
| host CPU | 10.3 % | 3.0 % |
| host session threads CPU time | 2.98 s | 0.53 s |

The desktop side was measured with the `desktop_headless_spike` harness under
a local Xvfb with Mesa lavapipe (`VK_DRIVER_FILES` forced to `lvp_icd`), mean
over 5 windows of 30 s, with the main thread CPU sampled from
`/proc/<pid>/task/<pid>/stat` (local scripts in `tasks/perf-gates-ep002/`):

| State | Counter or measure | Before | After |
|---|---|---|---|
| idle, 4 panes | `root_renders` | 15.8 | 0.6 |
| idle, 4 panes | `host_agent_snapshots_applied`, `session_list_calls` | 15 | 0.2 |
| one agent thinking | `root_renders` | 625 (20.8 per second, the Xvfb frame rate) | 333 (11.1 per second) |
| one agent thinking | desktop main thread CPU | 2.9 % | 1.9 % |
| 4 `stream` sessions | `root_renders` | 1 013 | 1 050 |

The streaming state is driven by terminal output, which EP-002 does not
change. The remaining idle frames and the single snapshot are the follow
header and the first frames after the window opens.

The review of EP-002 added two measurements, run on the same machine on
2026-10-04 in two alternating rounds (before, after, before, after). "Before"
is `09f86573` built in a separate worktree with only the two instruments
copied in; "after" is the EP-002 tree after the review fixes. Both documents
are local evidence (`tasks/perf-gates-ep002/review/`, not tracked).

The echo round trip of the active scenario (US-006), in ms over 200 samples per
scenario, the two rounds side by side:

| Active sessions | p95 before | p95 after | p99 before | p99 after |
|---|---|---|---|---|
| 1 | 0.112, 0.099 | 0.092, 0.097 | 0.421, 0.562 | 0.099, 0.109 |
| 4 | 0.104, 0.096 | 0.106, 0.093 | 0.154, 0.482 | 0.131, 0.126 |
| 8 | 0.096, 0.115 | 0.094, 0.096 | 0.570, 0.337 | 0.135, 0.123 |

The p95 does not increase, and the tail shrinks: before EP-002 the session
thread listed every process itself, which shows up as p99 spikes of 0.3 to
0.6 ms.

The tab badge cost (US-005), `desktop_tab_badges_cpu` with 12 tab badges and one
agent thinking, median over 5 windows of 30 s:

| Measure | Before | After |
|---|---|---|
| desktop main thread CPU | 4.65 %, 4.67 % | 2.30 %, 2.20 % |
| `root_renders` per window | 625 | 333 |
| main thread CPU time per frame | 2.23 ms | 2.03 ms |

Most of the drop comes from US-004 drawing fewer frames. The per-frame cost,
which the spinner change does not touch, falls by about 9 %. It mostly reflects the
squircle paths no longer rebuilt on every frame, but US-004 also dropped the
spinner's animation wrapper, so 9 % is an upper bound for US-005 alone.

### Workloads W02 to W08 and threshold decisions

Schema 3 runs W02, W03, W04 (worker replacement), and W05 after the W01
samples, in the same isolated home, and records a decision list under
`thresholds`. W02 fills one session with `history 10000` (deterministic
ANSI and Unicode lines), attaches ten times sequentially, ten times
concurrently, then cycles 100 attach/detach rounds, and checks that the
checkpoint bytes are identical every time and that the host reports zero
staged checkpoint bytes afterwards (`NFR-09.attach_p95`,
`NFR-09.concurrent_total`, `NFR-06.checkpoint_release`,
`W02.content_equivalence`). W03 measures one 32 MiB flood through a follower,
an echo probe at idle and while ten `stream` fixtures emit 1 MiB/s each, and
per-stream fairness (`NFR-08.idle_p95`, `NFR-08.idle_p99`, `NFR-08.loaded_p95`,
`NFR-08.throughput_ratio` against the matched baseline when one exists). W04
kills and restarts the existing worker, or replaces its binary with
`--worker-replacement`, while fixtures run, and checks that every child
identity and generation survives (`NFR-11.worker_cycles`). W05 runs ten
batches of fifty `flood 65536` sessions, waits for their exits, and checks
that the host releases every runtime within five seconds, then compares
resident memory, threads, and handles or file descriptors with the warmed
baseline after quiescence (`NFR-04.runtime_release`, `NFR-04.reclaim_max_ms`,
`NFR-05.memory_after_churn`, `NFR-05.memory_slope`, `NFR-05.threads`,
`NFR-05.handles`). A batch that misses the reclaim budget records the
runtimes still held and their inspected state. Fixture processes the run
owned are listed with their kernel start time; the final `host.shutdown`
must be acknowledged and the host process must exit within ten seconds
(`NFR-12.host_shutdown`), and any fixture survivor after that fails
`NFR-12.fixture_orphans`. A run that fails a threshold panics after writing
its artifact, and a host that refused shutdown because of unresolved
ownership stays alive on its private home by design: the artifact names it.

W04 fault cases, W06 injected failures, and the W07 desktop entry points that
live in the unit and integration suites are recorded as `automated_tests`
with their test names; the W07 interactive cells are recorded as `pending`
with the runbook cell that supplies them, and the baseline record leaves W08
`pending` because the endurance run is a separate ignored test in the same
target, `persistent_session_endurance` (`--endurance <minutes>` /
`-Endurance <minutes>`, output `bench/results/persistent-endurance-<stamp>-<sha>.json`):
it retains ten fixtures for the whole run, bursts ten flood sessions every few
minutes, spreads the worker and desktop cycles after an untouched idle interval,
checks that the first input on the idle control connection echoes exactly once,
samples host memory, threads, handles, and ownership counters at a fixed
interval, and rewrites its document at every sample. The full
protocol (60 s streams, 1,000 echo samples, 60 s quiescence, ten worker
cycles) is the default; `--quick` (`-Quick`) shortens every window for CI
and rehearsals and labels the record `smoke`. Threads and handles are
sampled on Windows and Linux; on macOS they are `pending`.

Any failed decision makes the test exit nonzero after the artifact is
written. `--prior` (`-Prior`) carries the failed decisions of an earlier
artifact into `prior_failures` so a rerun cannot erase the first failure, and
`--seed-failure` (`-SeedFailure`) injects one failing decision to prove that
path; the non-ignored test
`a_seeded_failure_fails_the_run_and_retains_its_artifact` proves it on every
CI target. The workload inputs, thresholds, and the platform evidence they
feed are frozen in
[docs/release/persistent-qualification.md](../docs/release/persistent-qualification.md).

## Work counters (Compteurs de travail)

Each process exposes monotonic `u64` counters, relaxed atomics with no
allocation and no I/O, through a status method that answers without waiting
for the GPUI thread. Every `counters` object also carries `process_identity`
(`pid` and the OS start instant `started_at`), so a reader can tell a restart
from a counter going backwards. The shared vocabulary and the reader live in
`crates/paneflow-host/src/work_counters.rs`: `HOST_COUNTERS`,
`WORKER_COUNTERS`, `DESKTOP_COUNTERS`, `sample` (a missing `counters` object or
counter reads as `pending` with its reason, never `0`) and `window` (a changed
`process_identity` invalidates the sample instead of producing a delta).
Nested counters are addressed with dots, for example
`git_spawns.by_subcommand.status`.

| Process and method | Counter | Unit | Counting point |
|---|---|---|---|
| host, `host.status` | `process_listings` | full system process listings | `process::unix_process_entries` (Linux `/proc`, macOS `proc_listpids`) and `process::windows_process_entries_named` (Toolhelp). Since EP-002 the periodic listing runs once per 500 ms for the whole host on the `paneflow-host-process-listing` thread (`process_listing.rs`) and every session that printed reads that shared snapshot; only exit and stop paths still list on the session thread |
| host, `host.status` | `foreground_observations` | foreground job walks | `runtime_observer::observe_foreground_runtime`, once the leader is provably live. The viewport scan reuses its last walk while the foreground group, its leader's start instant and its leader's name are unchanged, and only when the leader itself was identified or still has no child process (`ForegroundCache`) |
| host, `host.status` | `agent_bus_session_broadcasts` | agent bus frames of type `session` | `AgentBus::broadcast` |
| host, `host.status` | `agent_bus_session_removed_broadcasts` | frames of type `session_removed` | `AgentBus::broadcast` |
| host, `host.status` | `agent_bus_cancellation_broadcasts` | frames of type `cancellation` | `AgentBus::broadcast` |
| host, `host.status` | `agent_bus_event_broadcasts` | hook event frames | `AgentBus::broadcast` |
| host, `host.status` | `agent_bus_snapshot_broadcasts` | full agent snapshots served (`agent.snapshot` replies and follow headers) | `SessionHost::agent_snapshot` |
| worker, `worker.status` | `snapshot_broadcasts` | full snapshots broadcast to controllers, only when their content (without `updated_at_ms`) changed since the last broadcast | `AgentBus::broadcast` of type `snapshot`, through `Worker::broadcast_snapshot_if_changed`; a follower still receives the full snapshot as its stream header |
| worker, `worker.status` | `projection_broadcasts` | per-session projections broadcast | `AgentBus::broadcast` of type `event` |
| worker, `worker.status` | `sweeps` | 2 s state sweeps | `worker::sweep` |
| desktop, `system.counters` | `root_renders` | renders of the root view | `PaneFlowApp::render` |
| desktop, `system.counters` | `host_agent_snapshots_applied` | host agent snapshots applied; a snapshot identical to the last one applied is skipped unless it is the header of a new follow | `PaneFlowApp::apply_host_agent_snapshot` |
| desktop, `system.counters` | `session_list_calls` | `session.list` calls to the host | `host_link::list_sessions` |
| desktop, `system.counters` | `git_spawns.total`, `.probe`, `.user_action`, `.by_subcommand.<name>` | git processes, by profile and subcommand (16 named, the rest under `other`), filter queries included | `git_command::record_spawn`, called before each spawn in `git_command::run`, `run_keeping_stdout_head`, the filter query, and the git clone |
| desktop, `system.counters` | `process_spawns` | processes spawned through `paneflow-process` | `paneflow_process::run_supervised` and `spawn_detached` |

`system.counters` sits next to `system.identify` in the IPC server: same
connection checks, served on the connection thread, listed in
`system.capabilities`. The helper binaries (`paneflow-shim`,
`paneflow-ai-hook`, `paneflow-mcp`) do not depend on these crates. The cost
bound is the ignored test
`work_counters::tests::an_increment_costs_less_than_fifty_nanoseconds_in_release`:

```bash
cargo test --release --locked -p paneflow-host --lib work_counters -- --ignored
```

### Terminal memory per session

`host.status` also reports, under `resources.sessions[]`, the memory held by
each live session's libghostty terminal. The figures come from
`DisplayTerminal::memory_usage()` (`GHOSTTY_TERMINAL_DATA_MEMORY_USAGE`),
summed over the primary and alternate screens. They are read on demand when
`host.status` is called, never after a `feed`, because the read walks every
page. Each session thread gets 500 ms to answer.

| Field | Unit | Meaning |
|---|---|---|
| `memory.resident_bytes` | bytes | physical memory used by the terminal's pages; a compressed page counts only its compressed size. Use this figure for budgets |
| `memory.virtual_bytes` | bytes | address space reserved for the pages, compressed and spare pages included; always at least `resident_bytes` |
| `memory.compressed_bytes` | bytes | compressed history data, already included in `resident_bytes` |
| `memory.image_bytes` | bytes | Kitty graphics image data, not included in `resident_bytes` |
| `memory.compression_supported` | boolean | whether compressing scrollback can free memory on this platform; when false the compressed figures stay 0 |
| `memory_unavailable` | text | why `memory` is `null`: the terminal is retired, unverified, or did not answer in time. A missing figure is never reported as 0 |

## Desktop sans écran (headless desktop)

The US-003 spike asks whether the real release desktop runs and measures under
a virtual display with Mesa lavapipe on a GitHub `ubuntu-24.04` runner. The
`workflow_dispatch` workflow `.github/workflows/perf-desktop-spike.yml` runs
`scripts/perf-desktop-spike.sh` once under Xvfb and once under sway headless.
The script runs the ignored test `desktop_headless_spike`, which starts the
release host with an isolated `PANEFLOW_HOME`, launches the desktop with
`PANEFLOW_SOCKET_PATH` and `PANEFLOW_ALLOW_SOCKET_OVERRIDE=1`, cursor blinking
and telemetry off, and reads `system.counters` over IPC. It measures 5 windows
of 30 s for three states: the desktop idle with 4 panes, one simulated agent
thinking (an `ai.prompt_submit` hook event that starts the sidebar spinner),
and 4 `stream` sessions. For each counter it reports the mean and the
coefficient of variation across the 5 windows. The first frame is bounded by
the time until every restored pane shows the fixture announcement, with a 90 s
deadline.

The spike is validated when the first frame arrives within 90 s, idle
`root_renders` stays within ±1 frame of its mean in every 30 s window, and the
job finishes within 15 min. A desktop or Vulkan adapter that fails to start is
reported with the desktop log tail and the elapsed time and concludes "not
validated" instead of measuring an absent render. Counters whose coefficient of
variation exceeds 10 % will not be gated in absolute terms.

Result on 2026-10-04: **validated** on both servers, with Mesa lavapipe
(`llvmpipe`, Vulkan 1.4) as the only adapter. The deciding run is
[37198169856](https://github.com/arthjean/paneflow/actions/runs/37198169856)
at `83fe98b4`, with the release build restored from the `perf-spike-` cache:

| Server | Job | Installation | Cached build | Measurement | First frame | Idle `root_renders` per 30 s | Thinking | 4 streams |
|---|---|---|---|---|---|---|---|---|
| Xvfb | 13 min 53 s | 18 s | 5 min 13 s | 7 min 41 s | 1.4 s | 16 to 17 | 625 to 626 | 875 to 903 |
| sway headless | 14 min 59 s | 17 s | 6 min 21 s | 7 min 42 s | 2.5 s | 29 to 30 | 727 to 747 | 620 to 625 |

The server kept for US-013 is **Xvfb**: the shorter job, a first frame under
2 s, and no compositor or runtime directory to manage; sway headless sits at
the 15 min limit. No counter exceeded a 10 % coefficient of variation in this
run or in the previous one,
[37196774824](https://github.com/arthjean/paneflow/actions/runs/37196774824),
whose cold build made each job last 21 to 23 min. The idle desktop still
renders the root view once or twice per 2 s worker sweep (15 snapshots applied
and 15 `session.list` calls per window), and one thinking agent drives about
21 root renders per second under Xvfb: these are the regressions US-004 and
US-008 remove. A spike that is not validated would have proposed counting
inside `TestAppContext` or a third-party runner and blocked US-013.

### Tab badge cost

The ignored test `desktop_tab_badges_cpu` measures what the pane tab badges
cost the desktop main thread. It opens 12 idle fixture sessions in two side by
side panes of 6 tabs, because a pane holds at most 8 tabs (`MAX_PANE_TABS`),
sends an `ai.prompt_submit` hook event so one agent thinks and the sidebar
spinner runs, waits 4 s, then measures 5 windows of 30 s: the CPU time of the
desktop main thread, read from `/proc/<pid>/task/<pid>/stat` (Linux only; the
other platforms report it pending and the test fails), and the desktop work
counters. It needs a display and the release binaries:

```bash
cargo build --release --locked -p paneflow-app -p paneflow-host
harness=$(cargo test --release --locked -p paneflow-host --test persistent_baseline --no-run --message-format=json \
  | grep -o '"executable":"[^"]*persistent_baseline-[^"]*"' | tail -n 1 | sed 's/^"executable":"//; s/"$//')
cd crates/paneflow-host
PANEFLOW_BENCH_HOST=$PWD/../../target/release/paneflow-host \
PANEFLOW_BENCH_FIXTURE=$PWD/../../target/release/paneflow-session-fixture \
PANEFLOW_BENCH_CONTROLLER=$PWD/../../target/release/paneflow \
PANEFLOW_BENCH_OUT=/tmp/tab-badges.json \
"$harness" desktop_tab_badges_cpu --ignored --exact --nocapture --test-threads=1
```

Under Xvfb, set `DISPLAY` to the virtual display and `VK_DRIVER_FILES` to the
lavapipe ICD, as the headless spike does.

## Real hardware protocol

CI runners have no reliable GPU, so frame time, GPU load and the real cost of
a release on a desktop are measured by hand on Arthur's machines, before a
minor version that changes rendering (`docs/release/runbook.md`, Step 2).
A run compares the build against the previous minor version in the states the
change can move, usually `idle-4-panes` and `agent-thinking`: about five
minutes. The protocol measures one running Paneflow, whatever its
version, through `scripts/perf-hardware.sh` (Linux, macOS) or
`scripts/perf-hardware.ps1` (Windows), which run the ignored test
`hardware_protocol` of `crates/paneflow-host/tests/persistent_baseline.rs`
against the instance's own processes.

### Machines and sessions

| Platform | Session | GPU source | Frame time source |
|---|---|---|---|
| Fedora | Wayland (default session) | `nvidia-smi` and the amdgpu `gpu_busy_percent` file in sysfs (what `radeontop` reads), both sampled once per second | a MangoHud log |
| Fedora | X11 | same | a MangoHud log |
| Windows 11 | dual boot | the `GPU Engine` performance counter of the desktop process's 3D engines (`typeperf`, the counter Task Manager shows) | a PresentMon log |
| macOS | not measured | no source wired | none |

The result records the display backend the desktop actually uses, read from
its environment on Linux: `wayland` when it has `WAYLAND_DISPLAY`, `x11` when
it only has `DISPLAY`. For X11, log into an Xorg session where the desktop
still offers one; on GNOME without Xorg, launch Paneflow with
`env -u WAYLAND_DISPLAY`, which drives GPUI's X11 backend through XWayland, and
say so in the summary. The sources are the ones Arthur's hardware has; an
Intel GPU (`intel_gpu_top`) or a Mac (`powermetrics`) gets a source when such
a machine joins the protocol, and until then its GPU load is `not_measured`.

`nvidia-smi` and the sysfs file report the whole device, not one process, so
close every other GPU client (browser, video, other terminals' animations)
before a run.

### Preparation

1. Quit the daily Paneflow and every other instance; stop their host and worker
   (`paneflow host stop`, `paneflow serve stop`). The script refuses to run
   with more than one desktop, host or worker.
2. Plug the laptop in, set the screen to its usual resolution and refresh rate,
   and note both in the summary.
3. Install the build under test into a scratch directory: a published release
   from `gh release download v<version> -p "paneflow-<version>-x86_64.tar.gz"`
   (`.msi` on Windows), or `cargo build --release` of the commit for `main`.
4. In a terminal outside Paneflow, export an isolated home for that build
   and launch it from the same terminal, so the script finds its endpoints:

   ```bash
   export PANEFLOW_HOME="$HOME/.paneflow-hw-v0.17.5"
   ./paneflow &          # MangoHud: MANGOHUD=1 MANGOHUD_CONFIG=autostart_log=1,log_duration=60,output_folder=/tmp/hw ./paneflow &
   ```

   Every version measured here (v0.15.1 onward) honors `PANEFLOW_HOME`, so the
   daily state is never migrated or downgraded.
5. Maximize the window and wait until the first frame and the session restore
   settle.

### The four states

Each state is held for the whole 60 s window, with nobody touching the
machine. Run the script about 10 s after the state is reached.

| State | Setup |
|---|---|
| `idle-4-panes` | One workspace split into 2 x 2 panes, each an idle shell in a git repository, nothing printing, no agent. |
| `agent-thinking` | `idle-4-panes`, with one pane running Claude Code or Codex on a prompt that keeps it thinking for more than 60 s, its spinner visible in the sidebar. |
| `stream-4` | Four panes each running `paneflow-session-fixture stream 16384 600` from a release build of `main` (`target/release/`), the same stream the active scenario uses. |
| `panes-8` | One workspace split into 8 visible panes (2 rows of 4), idle shells. |

```bash
scripts/perf-hardware.sh --state idle-4-panes --label v0.17.5
scripts/perf-hardware.sh --state agent-thinking --label v0.17.5 --frame-log /tmp/hw/paneflow_*.csv
scripts/perf-hardware.ps1 -State stream-4 -Label v0.17.5 -FrameLog C:\hw\presentmon.csv
```

On Windows, capture the frame log in parallel with PresentMon 2.x:
`PresentMon --process_id <desktop pid> --output_file C:\hw\presentmon.csv
--timed 60 --terminate_after_timed`; the frame time is its
`MsBetweenPresents` column ([PresentMon console
README](https://github.com/GameTechDev/PresentMon/blob/main/README-ConsoleApplication.md)).
MangoHud writes a `frametime` column in milliseconds. The script accepts
either.

### What a run records

`bench/results/hardware-<os>-<arch>-<session>-<state>-<label>-<stamp>.json`,
over the window:

| Field | Content |
|---|---|
| `processes.<desktop,host,worker>` | CPU percent of one core per process and per thread role, resident memory, threads and handles, and the delta of every work counter. |
| `root_renders` | Root renders of the desktop over the window, from `system.counters`. |
| `gpu.<source>` | p50, p95 and max load in percent, one entry per source. |
| `frame_time_ms` | p50 and p95 frame time from the log. |
| `session`, `version`, `label`, `machine` | Display backend, `paneflow --version` of the measured binary, the label, and the machine identity. |

A measurement that cannot be taken is written
`{"not_measured": "<reason>"}`, never 0: a missing tool ("nvidia-smi is
unavailable: No such file or directory"), a version without counters
("system.counters failed: ... Method not found", every release before the
EP-001 counters), a process the version does not have ("no paneflow host
process was running"), or no frame log. The non-ignored test
`hardware_sources_parse_their_tools_and_never_turn_a_missing_reading_into_zero`
pins every parser and that rule. The Linux sources were exercised on a real
machine when the protocol was written (Fedora 44, RTX 4070 Ti SUPER and Radeon
610M, against v0.17.5); the Windows `typeperf` and PresentMon parsing is
covered by those tests on recorded output only, until its first run.

### Summary and verdicts

Each execution commits its JSON results and one
`bench/results/hardware-summary-<date>.md` with a table per state (CPU per
process, resident memory, root renders, GPU p50 and p95, frame time p50 and
p95) and one verdict per candidate regression of
`tasks/prd-performance-gates.md`, each confirmed, refuted, or not decidable
with the reason:

| Candidate | Introduced | Decided by |
|---|---|---|
| Tab badges rebuilt every frame | v0.17.1 | `agent-thinking`: desktop CPU and GPU, v0.17.0 against v0.17.1 |
| Full process listing per session | v0.17.0 | `stream-4`: host CPU, v0.15.1 against v0.17.0, then v0.17.5 against `main` after EP-002 |
| Viewport scan every 500 ms | v0.17.0 | `stream-4`: host CPU and its `host.viewport_scan` role |
| Worker snapshot every 2 s | v0.17.0 | `idle-4-panes`: worker and desktop CPU, desktop GPU |
| Extra git config process per probe | v0.17.5 | `idle-4-panes` in a git repository: desktop CPU, v0.17.1 against v0.17.5 |
| Startup sleep on a stale socket | May 2026 | not this protocol: `scripts/bench-startup.sh`, scenario `stale_socket_` |
| Per-session broadcasts | `610e6fc6` (unreleased) | `stream-4`: host and worker CPU, `main` before and after EP-002 |

The first execution compares v0.17.5 and `main` after EP-002 under Fedora
Wayland, in `idle-4-panes` and `agent-thinking`, which decide the tab badge
and worker snapshot candidates; the host candidates are already decided by the
counters of EP-002. It ran on 2026-10-06, see
[hardware-summary-2026-10-06.md](results/hardware-summary-2026-10-06.md): the
tab badge regression is confirmed in v0.17.5 and fixed on `main` (desktop CPU
25.0 % to 2.8 % of a core while an agent thinks, GPU p50 39 % to 12 %), and the
run found two new causes the gates miss. Seed the isolated home before the
first launch (`session.json` with no workspace, `paneflow.json` as `{}`,
`window-state.json`, `telemetry_id`), as the startup suite does: a home that
lacks them inherits the developer's legacy session through
`migrate_legacy_home`.

```bash
scripts/perf-hardware.sh --state idle-4-panes --label v0.17.5
scripts/perf-hardware.sh --state agent-thinking --label v0.17.5
scripts/perf-hardware.sh --state idle-4-panes --label main-<sha>
scripts/perf-hardware.sh --state agent-thinking --label main-<sha>
```

## Terminal suite

The benchmark is the ignored test `terminal_pipeline_benchmark` in
`src-app/src/terminal/perf_bench.rs`. It exercises the terminal pipeline
without a GPU or a window: the libghostty parser and snapshot, the conversion
into the renderer's neutral `Content`, the window-free layout pass, the
per-frame lookups the render thread performs, and the runtime loop's idle
behavior. Timings include wall-clock p50, p95 and p99; allocations are counted by the
shared allocator.

| Metric | Unit | What it captures |
|---|---|---|
| `idle_wakeups_display_per_s` | wakeups/s | Runtime loop iterations of a display-only session with nothing to do. Direct CPU cost of an idle pane. |
| `idle_wakeups_shell_per_s` | wakeups/s | Same for a live shell sitting at its prompt. Skipped when the host cannot spawn a shell. |
| `publish_scroll_220x60` | ns | One scrolled line of styled output, then snapshot plus conversion to `Content`, on a 220x60 grid where every row is dirty. |
| `publish_echo_220x60` | ns | One keystroke echo on the bottom row, then snapshot plus conversion. Only one row changed. |
| `publish_scroll_120x40` | ns | The scroll case on a 120x40 grid, the size of a typical split pane. |
| `layout_220x60` | ns | The layout pass over a full 220x60 snapshot: run batching, background rectangles, contrast checks. |
| `layout_220x60_acc60` | ns | The same layout pass with the automatic contrast correction on at Lc 60. Compared against `layout_220x60` at the end of the run: the suite prints a `PANEFLOW_BENCH_WARNING` when the correction costs more than 10%. |
| `layout_echo_{uncached,cached}_220x60` | ns | Paired native echo, publication and layout workloads with and without retained row layouts. |
| `layout_scroll_{uncached,cached}_220x60` | ns | The same pair with full-viewport scrolling; checks the cost when every row changes. |
| `service_spaces_220x60`, `service_spaces_8192`, `service_text_220x60` | ns | Service-output parsing and extraction for blank redraws, a long blank line and ordinary styled output. |
| `line_text_at_220x60` | ns | Text of one hovered row extracted from the published snapshot, the input of link detection. |
| `base_font_resolve` | ns | The base font resolution the renderer performs for every pane on every frame. |
| `active_theme_read` | ns | The theme read the layout pass performs for every pane on every frame. |
| `gate_trickle_publishes` | frames per 1000 chunks | Frames the publish gate lets through when grid changes arrive every 2 ms with the queue drained. Bounds redraw frequency on trickle output such as ConPTY. |
| `pipeline_corpus_mib_s` | MiB/s | Parse plus snapshot plus conversion throughput over the deterministic corpus, one publish per stream. |

The corpus is `deterministic_streams()` in
`src-app/src/terminal/bench_corpus.rs`, seeded with `CORPUS_SEED`, so every
run parses byte-identical input. These headless metrics exclude platform text
shaping, GPU submission and presentation. The cached/uncached layout pairs
compare paths in the same executable; they do not replace a before/after
release-app frame trace. `pipeline_corpus_mib_s` still excludes service
detection, which has separate scenarios above.

`PANEFLOW_BENCH_SKIP_IDLE=1` skips the two idle probes, which spend several
seconds waiting for a shell to settle; the timed scenarios run first either
way, so the probes never disturb them.

## Editor suite

The benchmark is the ignored test `editor_pipeline_benchmark` in
`src-app/src/app/diff_dock/code/perf_bench.rs`. It exercises the right-hand
code editor without a GPU or a window: the rope document, the tree-sitter
parse and highlight query, the run resolution the diff view shares, the UTF-16
conversions the input handler makes, the external-reload path, and the
platform shaper.

| Metric | Unit | What it captures |
|---|---|---|
| `open_300kb_highlighted` | ns | A 300 KB Rust file opened: rope build, longest-line measure, and an explicit query of the first 60-row viewport. Since US-030 the initial parse is deferred, so this is the work between the read and the first visible text. |
| `open_to_first_tree_300kb` | ns | The same file from the read to `apply_parsed`: the deferred initial parse plus the viewport query it makes possible. Everything but the apply runs off the render thread, so this is latency to color, not render-thread cost. |
| `open_2mb_to_text` | ns | A 2 MB Rust file from the read to visible text, with the initial parse still in flight. US-030 budgets this under 50 ms. |
| `open_3_7mb` | ns | A 3.7 MB Rust file opened past the 3 MB highlight cap, covering the rope build and the source-string longest-line scan. |
| `open_markdown_injected` | ns | A 64 KB Markdown file opened, the only corpus that runs a second grammar pass through the inline injection. |
| `keystroke_to_runs` | ns | Render-thread work of one inserted character at a pseudo-random row of 300 KB of Rust. Its `p95` column is the `keystroke_to_runs_p95` target of the PRD. Deferred parses run outside the timer; the `apply_parsed` requery they trigger is inside it. |
| `viewport_query_60_rows` | ns | The highlight query for one 60-row viewport, the work a viewport-bounded requery would do per frame. |
| `fill_60_stale_rows` | ns | One budgeted `fill_stale_rows` over a never-queried 60-row viewport, walking disjoint viewports of the 300 KB corpus. Since US-019 a contiguous stale span is one ranged query, so this metric must stay close to `viewport_query_60_rows`. |
| `keystroke_3_7mb_plain` | ns | Render-thread work of one inserted character in the 3.7 MB file, past the highlight cap: the rope splice alone, with no interpolation and no per-row table. |
| `plain_highlighter_retained_bytes` | bytes | Live allocated bytes the highlighter of that 3.7 MB file holds. A file past the cap keeps no per-row runs and no per-row states. |
| `unclosed_comment_close_ui` | ns | Render-thread work of closing an unterminated block comment at the top of the file, which re-tokenizes the whole document. |
| `resolve_runs_3750` | ns | `resolve_runs` over 3 750 captures taken from a 10 000-character minified JSON line, the shape the diff view shares. |
| `byte_to_utf16_eof` | ns | One byte offset converted to a UTF-16 offset at the end of a 3.7 MB document, two to four times per keystroke through `EntityInputHandler`. |
| `to_disk_string_3_7mb` | ns | The whole 3.7 MB document rendered to the string a save writes. |
| `theme_switch` | ns | A theme change on 300 KB of Rust, rebuilding capture color tables without reparsing or requerying the trees. |
| `shape_cold_60_rows` | ns | Sixty never-seen ASCII rows of 100 characters shaped with the editor monospace font, the cold-cache cost of one scrolled viewport. |
| `shape_warm_60_rows` | ns | The same sixty rows shaped again, the warm-cache cost the line-layout cache serves on a second frame. |
| `prepaint_60_rows_warm` | ns | The same sixty rows re-shaped the way `CodeElement::prepaint` does it since US-025: keyed by content hash through `shape_line_by_hash`, with one reused `Vec<TextRun>`. Its `allocs_per_iter` is the per-viewport allocation count US-025 caps at one per row. |
| `reload_200_retained_bytes` | bytes | Live allocated bytes a colored 2 MB tab still holds after 200 external reloads: document, per-row runs, and undo history. Tree-sitter trees allocate through the C allocator, so they are outside this number and the tree memory probe below counts them instead. |
| `pagedown_stale_rows` | rows | Median rows of a 60-row viewport still uncolored after one 2 ms fill, over 20 pseudo-random jumps on a freshly opened 300 KB Rust file. |
| `pagedown_stale_rows_max` | rows | The worst of those 20 jumps: rows the first frame after a PageDown leaves in plain text. |
| `pagedown_frames_to_fresh` | frames | Successive 2 ms fills the worst of those 20 jumps needs before no visible row is stale. |
| `textdiff_300kb_50_blocks` | ns | `paneflow_textdiff::compare_lines_inner` with word highlighting over 300 KB of synthetic Rust against a copy with 50 rewritten 10-line blocks. Its `p95` is the 25 ms target of EP-012. |
| `textdiff_5k_lines_one_word_each` | ns | The same call over 5 000 edited lines of eight words, each with one word changed and separated from the next by an identical line, so the line pass yields 5 000 one-line blocks and the word pass runs once per edited line. Target: 150 ms. |
| `textdiff_5k_lines_all_different` | ns | The same call over 5 000 dense lines of sixteen words split into four all-different blocks by identical separator lines. Each block exceeds the 20 000-chunk fine comparison threshold: the first three trip the bad-lines guard and the fourth is skipped without a word pass. Target: 50 ms. |

The corpus is `src-app/src/app/diff_dock/code/bench_corpus.rs`, seeded with
`EDITOR_CORPUS_SEED`. It is generated, never read from the repository's own
sources, so a run is byte-identical everywhere: synthetic Rust sized to 295 KB,
2 MB (both under the 2 MB highlight cap, the larger by 48 bytes), and 3.7 MB
(about 110 000 lines, past it); a
single-line minified JSON document of exactly 10 000 characters; and Markdown
carrying both inline and fenced code so the injection pass has work to do. The
`textdiff_*` metrics add three seeded pairs from the same file: 300 KB of Rust
with 50 rewritten 10-line blocks, 5 000 eight-word lines with one word changed
per line and an identical separator line between them, and 5 000 sixteen-word
lines in four all-different blocks.

### The PageDown stale-row probe

`pagedown_stale_rows`, `pagedown_stale_rows_max` and `pagedown_frames_to_fresh`
turn the 2 ms highlight budget into a number. Each of the 20 jumps moves a
60-row viewport to a pseudo-random row of the 300 KB Rust corpus, calls
`CodeHighlighter::fill_stale_rows` with `HIGHLIGHT_FRAME_BUDGET`, records the
rows the call left stale, then keeps calling until none is. Rows left stale are
rows the user reads in plain text; frames-to-fresh is how many frames the
editor needs before the viewport is fully colored. Since US-018 a starved fill
schedules those frames itself through `Window::request_animation_frame`, and
since US-019 a 60-row viewport is a single ranged query, so the probe reports 0
stale rows in 1 frame even at a zero budget. A file past the highlight cap
reports the same and spends no budget at all, and so does a file whose initial
parse has not landed yet: since US-030 a treeless highlighter fills nothing and
asks for no follow-up frame.

### The shaping probe and the US-013 threshold

`prepaint_60_rows_warm` measures the same sixty rows through the path the
editor actually takes since US-025. `shape_warm_60_rows` passes a
`SharedString` per row, so `layout_line` allocates one more copy of the text on
every hit; the prepaint probe passes a content hash instead and materializes
nothing when the layout is already cached. The two are not a like-for-like
timing pair, because the prepaint probe also walks the rope and hashes every
line before it reaches the cache, so compare them on allocations rather than
on nanoseconds. That count is the number US-025 caps at sixty for sixty rows.

`shape_cold_60_rows` and `shape_warm_60_rows` decide whether the ASCII grid of
US-013 is worth building. **The threshold is 1.0 ms cold per 60 rows on the
reference machine.** Below it, `shape_line` is not what makes scrolling
expensive and US-013 stays unbuilt; at or above it, the grid path is worth
its complexity. Like every other timing metric, both are stored in nanoseconds
and rendered by the table in milliseconds once they pass 1 ms, so the
threshold reads as `1.00 ms` in the table and `1000000.0` in the document.

The probe deliberately does not use GPUI's `TestAppContext`. That context
installs `NoopTextSystem`, a stub that returns synthetic metrics for every
font, so a measurement taken through it would describe the stub and not the
platform shaper the editor actually pays for. The probe instead resolves the
real platform text system through `gpui_platform::current_platform(true)` and
shapes through a `WindowTextSystem` built on it. When that platform cannot be
created, or when it shapes a zero-width line because no real font is
available, both metrics are reported as unavailable through
`PANEFLOW_BENCH_SKIP` lines, remain in the JSON with `available: false` and a
null value, and the suite carries on. `PANEFLOW_BENCH_SKIP_SHAPE=1` skips the
probe outright with the same unavailable result.

`reload_200_retained_bytes` allocates and retains several hundred megabytes by
design, which is the defect it measures. It runs last, after the timed
scenarios, so it never inflates them.

### The tree memory probe and the highlight caps

`MAX_HIGHLIGHT_BYTES` and `MAX_MARKDOWN_HIGHLIGHT_BYTES`
(`src-app/src/diff/highlighter.rs`) are set by measurement, not by guess. The
rule US-031 fixes them by: **a file at its cap must hold less than 128 MiB of
tree-sitter tree.** Both caps are read through `highlight_cap(ext)`, so the
editor's `CodeHighlighter` and the diff view's `highlight_lines` sit behind the
same rule.

The measurement is a second ignored test, `tree_memory_probe`, in the same
file as the editor suite. It routes tree-sitter's own C allocator to a counting
allocator through `tree_sitter::set_allocator`, so the bytes it reports are the
tree and nothing else. That counter is deliberately kept out of the timed
suite: installing it would change every parse timing.

The installer is the `unsafe fn`
`count_tree_sitter_allocations_in_a_process_that_has_not_parsed_yet`
(`src-app/src/bench_harness.rs`), and its name is its precondition: no
tree-sitter object may exist in the process when it runs. Every block
tree-sitter allocated before the switch would later be freed through the
counting allocator, which reads a size header that block never had, and the
heap is corrupted. A test binary shares one process across every test it runs,
so `tree_memory_probe` never installs the counter itself. It relaunches the
test binary as a child with `--exact` on
`tree_memory_probe_in_a_fresh_process`, which installs the counter first and
then measures, and it fails unless that child reports exactly one test run and
the measurement ran.

```bash
cargo test --release --locked -p paneflow-app --bin paneflow \
  app::diff_dock::code::perf_bench::tree_memory_probe \
  -- --ignored --exact --nocapture
```

Measured on Windows 11 x86_64, release profile, tree-sitter 0.26.13, on the
generated corpus:

| Grammar | Source | Tree | Bytes of tree per source byte | Cap the 128 MiB rule allows |
|---|---|---|---|---|
| Rust | 295 KB | 8.70 MB | 29.5 | 4.55 MB |
| Rust | 2 MB | 58.7 MB | 29.4 | 4.57 MB |
| Rust | 3.7 MB | 108.4 MB | 29.3 | 4.58 MB |
| Minified JSON | 295 KB | 16.1 MB | 54.6 | 2.46 MB |
| Minified JSON | 2 MB | 101.6 MB | 50.8 | 2.64 MB |
| Markdown (two passes) | 64 KB | 8.25 MB | 129.1 | 1.04 MB |
| Markdown (two passes) | 2 MB | 254.9 MB | 127.5 | 1.05 MB |

The ratio is a property of the grammar, not of the file size, so a cap set on
Rust alone does not hold. Minified JSON costs 1.7 times what Rust costs per
source byte, because a one-line document of short key-value pairs is nearly all
nodes and no text. Markdown costs about 4.3 times, because the inline injection
parses the whole document a second time; that second pass is why it keeps a cap
of its own.

`MAX_HIGHLIGHT_BYTES` is therefore 2 MB, set by the densest single-pass grammar
in the corpus rather than by Rust: JSON allows 2.64 MB, rounded down to the
megabyte. Rust alone would have allowed 4 MB, and a 3 MB cap put JSON at
143.7 MiB, past the budget. At 2 MB the measured grammars hold 56.0 MiB (Rust)
and 96.9 MiB (JSON), which also leaves headroom for the thirteen grammars the
corpus does not generate: anything up to 67 bytes per source byte stays inside
the budget. `MAX_MARKDOWN_HIGHLIGHT_BYTES` is 1 MB (121.8 MiB at the cap, 95%
of the budget: the tightest of the three margins). The probe asserts all three,
so raising a cap without re-measuring fails the test.

The probe also checks the other half of US-031: a deferred parse holds a second
tree while it is in flight, and `apply_parsed` drops the superseded one. That
second tree costs far less than the first, because an incremental parse shares
the subtrees the edit did not touch: on the 295 KB Rust corpus a one-line edit
adds 203 KB to the 8.70 MB the first tree holds, and the counter returns to
8.70 MB once `apply_parsed` has run.

## Scroll frame scenario

The editor suite runs without a window, so it cannot say what one wheel notch
costs when terminals share the frame. That number comes from a separate
ignored test, `layout::render::tests::editor_scroll_frame_by_pane_count`:

```bash
cargo test -p paneflow-app --release -- --ignored layout::render
```

It opens the 300 KB Rust corpus in a `CodeView` docked to the right of the pane
grid, fills every terminal pane with `deterministic_streams()` and lets them go
idle, places the caret at the top and scrolls away from it, then dispatches 120
`ScrollWheelEvent` notches of `Lines(3)` spaced 8 ms apart. It repeats that for
0, 2 and 6 terminal panes and prints one JSON line carrying
`scroll_frame_p50_us_panes_N` and `scroll_frame_p95_us_panes_N` for each N,
computed from GPUI's `dirty_to_draw_duration` over at least 100 frames per
configuration. `render_content_lock_samples_panes_N` counts the terminal grid
snapshots taken across those frames. Before EP-010 it read one snapshot per pane
per frame, the witness that a scroll which only moved the editor still repainted
every terminal. EP-010 hosts each `TerminalView` in a `ViewElement::cached`, so
an idle pane now takes none and the measurement asserts zero. A configuration
that cannot build its panes is reported with
`scroll_frame_available_panes_N: false` and the others still run.

**The measurement is relative, not absolute.** `TestAppContext` installs
`NoopTextSystem`, so no platform shaping is included: the numbers compare
configurations against each other and never bound the real cost of a frame.
The absolute cost is read from a release profile of the running application.

`terminal_share_p50_panes_6` and `terminal_share_p95_panes_6` are the fraction
of a six-pane scroll frame that disappears at zero panes. **US-029 hosts the
terminal panes behind `ViewElement::cached` only if that share reaches 0.30.**
Below it, caching the panes is not worth its complexity and the story is
canceled with the measured value recorded in the PRD changelog. The EP-006 run
measured 0.68 in p50 and 0.63 in p95, well past the threshold, so US-029
shipped.

`scroll_frame_p95_ratio_panes_N` is that configuration's p95 divided by the
zero-pane p95. It is a tracked measurement with no threshold attached. EP-010
dropped the 1.5 target it used to carry: a control run of the same tree with and
without `.cached` on the `TerminalView` moved the six-pane p95 from 1432 to
1426 us while the grid snapshots went from 720 to 0, so this harness cannot see
what the cache saves. Its `NoopTextSystem` excludes the shaping the cache skips
and keeps the scene replay and the `Pane` chrome, which carry the rest. The p95
ratio read 2.7 before EP-010 and reads 3.1 to 3.7 after it, the editor frame
having grown cheaper while the per-pane cost held.

## Running

```bash
scripts/bench-terminal.sh                 # Linux, macOS
scripts/bench-terminal.ps1                # Windows
scripts/bench-editor.sh                   # Linux, macOS
scripts/bench-editor.ps1                  # Windows
```

`scripts/bench-editor.sh --help` (and `scripts/bench-editor.ps1 -Help`)
prints the options and the environment variables both suites honor.

Each script builds the `paneflow` test binary under the release profile,
records the short commit SHA, whether the worktree is dirty, and a UTC stamp,
then writes its result under `bench/results/`. The run always prints a
Markdown table between the `PANEFLOW_BENCH_TABLE_BEGIN` and
`PANEFLOW_BENCH_TABLE_END` markers: a comparison table when the suite's
baseline exists, and the same table without its comparison columns when it
does not. That table is the artifact to share.

`--set-baseline` (or `-SetBaseline` on Windows) copies the fresh result over
the suite's baseline for the run's platform, and is refused from a dirty
tracked worktree (see "Baselines per platform").

`scripts/bench-editor` and `scripts/bench-terminal` refuse `--set-baseline`
when the run reports a `cpu_share` below 0.90: a contended run inflates every
timing it would freeze, and every later comparison against it would read as a
false improvement. Close the competing workload and run again.
`scripts/bench-startup` is exempt: its work runs in a child process, so its
`cpu_share` is `null` (not measured) until it measures that child's CPU time.
It still refuses a dirty tree.

**A change that moves a metric updates the baseline in the same pull request.**
A baseline older than the code it is compared against turns every table into
fiction: the editor baseline recorded before EP-002 to EP-005 reports 69 ms of
work per keystroke against a HEAD that measures 2.5 ms.

Both suites refuse to run under the debug profile, which would measure the
compiler rather than the code, and exit non-zero with an explicit message. Set
`PANEFLOW_BENCH_ALLOW_DEBUG=1` to override while developing a suite itself.

The timing comparison never runs in CI. The tables are local artifacts
compared against a local baseline, which is what makes them meaningful. Only
the allocation columns, which are deterministic, are gated in CI (see
"Performance gates").

## Fairness rules

A comparison is only meaningful between runs on the same machine, at the same
grid sizes, with the same corpus seed, and both built under the release
profile. The result document records OS, architecture, CPU model, profile,
seed, and commit so that a mismatched comparison is visible. Close heavy
applications before a run; the medians are robust to a stray interruption,
the p95 values are not.

Two runs of the same commit differ by a few percent on the microsecond
metrics. Treat a change below 5% as noise unless the allocation columns, which
are deterministic, moved with it.

The run measures its own CPU share over the timed scenarios (process CPU time
divided by wall time, recorded as `cpu_share` in the result). The scenarios
are single-threaded and never sleep, so an uncontended run reports close to
1.0. A run that prints `PANEFLOW_BENCH_WARNING` got less than 90% of a core:
something else was competing, its timings are inflated, and it should not be
published as a comparison.

## Reading the table

`Change` is the relative move of the headline value and, in parentheses, the
speedup: baseline over now for costs, now over baseline for throughput. A
timing that halved reads `-50.0% (2.00x)`. `Alloc/iter` columns show bytes
allocated per iteration and are exact.

## Result schema

```json
{
  "schema": 2,
  "suite": "paneflow-terminal-bench",
  "generated_unix": 0,
  "stamp": "20260901T120000Z",
  "git_sha": "4066faf6abcd",
  "git_dirty": "false",
  "os": "windows",
  "arch": "x86_64",
  "platform": "windows-x86_64",
  "cpu": "AMD Ryzen 7 7800X3D 8-Core Processor",
  "profile": "release",
  "corpus_seed": "0x...",
  "cpu_share": 0.98,
  "metrics": [
    {
      "metric": "publish_scroll_220x60",
      "unit": "ns",
      "direction": "lower_is_better",
      "value": 0.0,
      "p95": 0.0,
      "mean": 0.0,
      "alloc_bytes_per_iter": 0.0,
      "allocs_per_iter": 0.0,
      "iters": 300,
      "note": "..."
    }
  ]
}
```

The editor suite writes the same document with `"suite":
"paneflow-editor-bench"` and its own `corpus_seed`. Schema 2 added `platform`
and made `cpu_share` nullable. `cpu` is the processor model: `/proc/cpuinfo`
on Linux, `sysctl machdep.cpu.brand_string` on macOS, the registry value
`ProcessorNameString` on Windows, and `unknown` when none answers.

## Syntax query parity measurement

The Windows x86_64 release run
[editor-20260904T231427Z-d03590c2816f.json](results/editor-20260904T231427Z-d03590c2816f.json)
measures the Zed query integration on the working tree based on `d03590c2816f`.
CPU share was 0.965. `editor-baseline.json` remains unchanged. These are measured
pipeline times, not end-to-end GPU frame times.

| Metric | Measured | Required |
| --- | ---: | ---: |
| `keystroke_to_runs` p95 | 0.767 ms | < 1.5 ms |
| `viewport_query_60_rows` | 0.306 ms | < 0.6 ms |
| `fill_60_stale_rows` | 0.290 ms | < 0.6 ms |
| `resolve_runs_3750` | 18.4 us | < 500 us |
| `theme_switch` | 3.1 us | < 5 us |
| `open_markdown_injected` | 37.9 us | < 100 us |
| `pagedown_stale_rows_max` | 0 | 0 |
| `pagedown_frames_to_fresh` | 1 | 1 |
| `reload_200_retained_bytes` | 5,409,886 bytes | < 64 MiB |
| `plain_highlighter_retained_bytes` | 0 | 0 |
| `open_to_first_tree_300kb` | 45.365 ms | within 10% of the preceding implementation |

The historic baseline predates `open_to_first_tree_300kb`. Its comparison uses
[the last recorded EP-011 run](results/editor-20260904T210646Z-f76af1ef703c.json):
43.445 ms before, 45.365 ms after (+4.4%). Other comparisons use the baseline
selected by `scripts/bench-editor.ps1`.

## Startup suite

The benchmark is the ignored test `startup_first_frame_benchmark` in
`src-app/src/startup_bench.rs`. Unlike the other two suites it launches the
real release binary, because the cost it measures is the GPU window, the
platform text system, and the state the app builds before its first frame,
none of which exist in a window-free test. The app cooperates through the
startup trace in `src-app/src/startup_trace.rs`: when `PANEFLOW_STARTUP_TRACE`
names a file, the app records a mark at each stage of `main` and of
`PaneFlowApp::new`, writes the timeline as JSON once its first frame has been
presented, and quits. The probe ships in release builds so the shipping profile
is what gets measured.

Three scenarios (two on Windows) run back to back, each against its own
seeded `PANEFLOW_HOME` under the system temp directory:

| Scenario | Prefix | Home contents |
|---|---|---|
| Welcome | `welcome_` | An empty session, so the first frame is the welcome screen. |
| Restore | `restore3_` | A session of three workspaces with one terminal pane each, all in a scratch directory. The daily case: a restored layout whose panes spawn shells. |
| Stale socket (Linux, macOS) | `stale_socket_` | The welcome home, with a desktop IPC socket left behind by an unclean exit bound before every launch: the file exists and refuses connections. `stale_socket_step_ipc_server_started` is the US-010 startup budget the performance gates enforce. |

The seed writes `session.json`, `paneflow.json` (`{}`), `window-state.json`
(a fixed 1400x900 window) and `telemetry_id` before the first launch. Every
one of those files must exist: `migrate_legacy_home` copies the developer's
own files from the legacy `dirs::config_dir()` locations into any home that
lacks them, which would silently turn the fixture into the developer's real
session. One untimed warm-up launch per scenario absorbs whatever else the app
creates on first run; the timed launches that follow
(`PANEFLOW_BENCH_STARTUP_RUNS`, default 10) therefore measure a second launch.
The suite refuses a debug binary unless `PANEFLOW_BENCH_ALLOW_DEBUG` is set.
`PANEFLOW_BENCH_EXE` overrides the binary path, which otherwise resolves to
`paneflow` next to the test binary's profile directory.

| Metric | Unit | What it captures |
|---|---|---|
| `<scenario>_first_frame_total` | ns | From the first line of `main` to the first presented frame of the app window. The headline number of each scenario. |
| `<scenario>_step_<mark>` | ns | The time between one trace mark and the previous one, one metric per mark in launch order. `step_gpui_app_ready` is the platform and text system initialization inside GPUI, `step_fonts_loaded` the registration of the embedded fonts, `step_window_created` the GPU window, `step_ipc_server_started` the singleton guard and IPC thread, `step_workspaces_restored` the session restore, `step_first_render_built` the element tree construction, and `step_window_open_returned` the first layout and paint of that tree. |

The mark names are the metric names, so adding a mark adds a metric and the
comparison table reports it as new. The `cpu_share` field is `null`, not
measured, for this suite: the timed work happens in a child process, so the
harness cannot attribute a core share to it.
