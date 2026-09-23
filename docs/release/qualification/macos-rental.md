# macOS qualification on a rented Apple Silicon Mac

The required macOS evidence comes from the hosted runner
([macOS on the hosted runner](../persistent-qualification.md#macos-on-the-hosted-runner)).
This runbook is the optional route that adds what a shared VM cannot give: a
Scaleway Mac mini rented for 24 hours for dedicated-hardware performance, an
interactive manual smoke on a physical Mac, and local-display latency.
Everything that can be done without the Mac is done before the rental starts,
so the paid window is spent on native evidence only. Its results add cells to
the macOS report; they do not rewrite the hosted-runner verdicts. macOS
Intel, physical peripherals, and notarized distribution stay `unavailable`
unless exercised separately.

Provisioning, payment, and deletion are the operator's actions. Nothing in
this repository creates or deletes a rental.

## Gate before renting

Rent only when every item holds at one candidate SHA:

1. Every implementation story of the persistent-session PRD has completed
   review.
2. `run_tests.yml` is green at the candidate SHA in a manual dispatch run,
   which forces every leg including `macOS aarch64 smoke build` and
   `macOS aarch64 render smoke (visual)`:
   `gh workflow run run_tests.yml --ref main`, then check the run for the
   candidate SHA.
3. `macos_qualification_package.yml` is green for the same SHA with the full
   protocol:
   `gh workflow run macos_qualification_package.yml -f candidate_ref=<sha> -f full_protocol=true`.
   Its artifact is kept 30 days; a package older than that must be rebuilt.
   The job summary shows the package SHA-256 and the preflight ledger.
4. The current quote below has been re-read on the day of provisioning.

Dispatched with `-f full_protocol=true`, the same workflow runs the full
protocol, the W08 endurance rehearsal, and the host profiles from the
extracted package on the hosted runner. That run is the required macOS
evidence; a rental adds to it and never replaces a failing one.

A candidate change after the package was built follows the invalidation
table in the runbook: a change to the session lifecycle, IPC, or engine needs
a new package and a new rental plan, never a package from an older SHA.

## Package contents

`paneflow-macos-qualification-<sha12>.tar.gz`, with `SHA256SUMS` over every
file inside and a `.sha256` for the archive itself:

| Path | Content |
|---|---|
| `candidate/PaneFlow.app` | the candidate desktop and host, byte-identical to the release build the manifest hashes |
| `candidate/candidate.json` | the candidate manifest (`scripts/candidate-manifest.sh`): SHA, `dirty: false`, desktop, host, fixture, embedded helpers, engine archive |
| `candidate/bin/paneflow-session-fixture` | the deterministic fixture every workload drives |
| `candidate/bin/persistent_baseline` | the prebuilt harness; `scripts/bench-persistent.sh --prebuilt` runs it without Cargo |
| `candidate/bin/paneflow-worker-replacement` | a second copy of the candidate desktop at another path, for the W04 and D-03 build replacement |
| `profiling/candidate/paneflow-host`, `profiling/baseline/paneflow-host` | hosts at the candidate and at the pre-refactor baseline `f881c8bc`, with line tables and a packed dSYM for `sample` symbolication |
| `scripts/` | `bench-persistent.sh`, `qualify-macos-preflight.sh`, `profile-host-macos.sh` |
| `PACKAGE.txt` | candidate and baseline SHAs, the workflow run, toolchain, runner, signing state |

The package is signed only by the linker's ad-hoc signatures. It is not
Developer ID signed or notarized, so a qualification from it says nothing
about Gatekeeper or notarized distribution; the `M09-signature` preflight
cell records that state. No signing secret, GitHub token, or other credential
is ever copied to the rental. A signed and notarized DMG from a `release.yml`
dry run of the same SHA may be used for the installed-app cell when one
exists; record which route each cell used.

## Current quote

Verified on 2026-09-23 from the
[Scaleway Apple silicon pricing page](https://www.scaleway.com/en/pricing/apple-silicon/);
prices exclude VAT and change without notice, so re-read them on the day and
record the quote in the report.

| Type | Chip | Memory / SSD | EUR per hour | 24 hours |
|---|---|---|---|---|
| M1-M | M1 | 8 GB / 256 GB | 0.11 | 2.64 |
| M2-M | M2 | 16 GB / 256 GB | 0.17 | 4.08 |
| M4-S | M4 | 16 GB / 256 GB | 0.22 | 5.28 |
| M4-M | M4 | 32 GB / 1 TB | 0.29 | 6.96 |

Use `M4-S` in `fr-par-1`. The workloads hold 50 sessions with desktop, host,
and worker attached; the 8 GB M1 risks memory pressure that would distort the
NFR-03 and NFR-05 decisions. Expected total: EUR 5.28 before VAT, about
EUR 6.34 with French VAT at 20 %, plus every hour the machine exists past the 24th. One
IPv4 and one IPv6 address are included.

Billing rules from the
[Scaleway Apple silicon FAQ](https://www.scaleway.com/en/docs/apple-silicon/faq/)
and the
[creation guide](https://www.scaleway.com/en/docs/apple-silicon/how-to/how-to-create-mac-mini/):
the minimum lease is 24 hours and the machine cannot be deleted before; it is
billed for as long as it exists on the account and must be deleted
explicitly. No documented automatic or scheduled deletion exists; do not
assume the machine expires at 24 hours.

## Provisioning

In the console (Apple silicon, Create Mac mini) or with the CLI:

```bash
scw apple-silicon os list zone=fr-par-1
scw apple-silicon server create type=M4-S os-id=<default image uuid> name=paneflow-qualify zone=fr-par-1 commitment-type=none
```

Keep the default macOS image: a non-default version adds about an hour of
deployment. Record the server ID, IP, macOS image, and the creation time; hour
0 of the schedule starts when the machine is reachable. Do not enable
FileVault: it blocks remote access after a reboot.

## Access

The server's Overview page shows the SSH command, the user name, the VNC port,
and the password.

- Windows: `ssh <user>@<ip>` from Windows Terminal (built-in OpenSSH), and
  RealVNC Viewer on `<ip>:<vnc port>` for the graphical session.
- Linux: `ssh`, and Remmina with a color depth of 16 bits or more.

Log in once through VNC before anything else. GPUI needs the logged-in
graphical session and its Metal device; a desktop started from a plain SSH
shell has no window server. Run the preflight, the desktop cells, and the
endurance run from a Terminal inside the VNC session. Keep the machine awake
for the whole rental with `caffeinate -dimsu &` in that Terminal.

## Transfer and preflight

On the operator machine:

```bash
gh run download <package run id> -n paneflow-macos-qualification-<sha12>
shasum -a 256 -c paneflow-macos-qualification-<sha12>.tar.gz.sha256
scp paneflow-macos-qualification-<sha12>.tar.gz* <user>@<ip>:
```

On the Mac, in the VNC Terminal:

```bash
shasum -a 256 -c paneflow-macos-qualification-<sha12>.tar.gz.sha256
tar -xzf paneflow-macos-qualification-<sha12>.tar.gz
cd paneflow-macos-qualification-<sha12>
scripts/qualify-macos-preflight.sh
```

The preflight writes `~/paneflow-qualification/evidence/preflight-<stamp>/`
with a ledger, hardware and OS identity, `system_profiler` output, the
signature state, host and desktop logs, and a screenshot, then packs it into
an archive and prints the `scp` command that fetches it. It exits nonzero when
any cell fails. Fetch that archive once and verify it locally: it proves the
export path before the evidence matters.

| Cell | Pass rule |
|---|---|
| M01-arch | `arm64` |
| M02-os | recorded |
| M03-gui-session | the console user is the SSH user and owns `gui/<uid>` |
| M04-metal | `system_profiler` reports Metal support; the display resolution is recorded |
| M05-disk | at least 20 GiB free under the qualification root |
| M06-filevault | off |
| M07-package-integrity | every file matches `SHA256SUMS` |
| M08-quarantine | no `com.apple.quarantine` attribute (`xattr -dr com.apple.quarantine <package>` clears it) |
| M09-signature | recorded; informational |
| M10-no-competing-paneflow | no Paneflow process before the run |
| M11-socket-path | `TMPDIR` short enough for the 104-byte `sun_path` limit |
| M12-host | the packaged host serves a private home and runs one fixture session |
| M13-desktop | the packaged desktop reaches its first font configuration under Metal and stays up; then its worker and host stop and no process of its home remains |
| M14-evidence-export | the evidence archive is written and lists |

A failed M03, M04, M12, or M13 stops the qualification: record the Mac as
unavailable for the cells that need it and never substitute a pass. The
package workflow runs on a virtual `macos-14` runner whose
paravirtual GPU has no Graphics/Displays section in `system_profiler`, so M04
fails there while M13 still renders through Metal; on the physical Mac mini
both must pass.

## Qualification commands

Every command runs from the package root; results land in
`bench/results/` inside it. Each harness run creates and removes its own
private home. Interactive cells use a dedicated home under
`~/paneflow-qualification/homes/`, never `~/.paneflow`.

| Evidence | Command |
|---|---|
| W01-W05 host only, full protocol | `scripts/bench-persistent.sh --prebuilt candidate` |
| W01-W05 host and worker | `scripts/bench-persistent.sh --prebuilt candidate --with-worker` |
| W01-W05 with the native desktop | `scripts/bench-persistent.sh --prebuilt candidate --with-desktop` |
| W04 build replacement | `scripts/bench-persistent.sh --prebuilt candidate --worker-replacement candidate/bin/paneflow-worker-replacement` |
| W08 endurance rehearsal (required) | `PANEFLOW_BENCH_BURST_MINUTES=2 PANEFLOW_BENCH_WORKER_CYCLES=6 PANEFLOW_BENCH_DESKTOP_CYCLES=4 PANEFLOW_BENCH_SAMPLE_SECONDS=30 scripts/bench-persistent.sh --prebuilt candidate --with-desktop --endurance 12 --idle-minutes 4` |
| W08 8-hour run (optional) | `scripts/bench-persistent.sh --prebuilt candidate --with-desktop --endurance 480 --idle-minutes 30` |
| Rerun after a failure | add `--prior bench/results/<failed result>.json` |
| Host profiles, memory | `scripts/profile-host-macos.sh profiling/<side>/paneflow-host candidate/bin/paneflow-session-fixture memory <out> <side>` for `baseline` and `candidate` |
| Host profiles, CPU | the same with `cpu`; `sample` writes `sample-idle-<side>.txt` and `sample-output-<side>.txt` |
| Interactive D-cells and W07 | `PANEFLOW_HOME=~/paneflow-qualification/homes/d-cells candidate/PaneFlow.app/Contents/MacOS/paneflow` from the VNC Terminal, then the D-01 to D-11 table of the runbook |
| Upgrade from the last release | install the latest published release DMG in a dedicated home, keep fixtures running, then start the candidate against that home |

On macOS the harness samples resident memory, thread count, descriptor count,
and per-thread CPU through `proc_pidinfo`, so W01, W05, and W08 decide their
memory, thread, and descriptor thresholds natively. The profile driver adds
`vmmap` physical footprint, `ps` thread counts, `lsof` descriptors, and
`sample` call trees. NFR-08 echo timing is measured by the harness against a
local monotonic clock; VNC latency is never a measurement.

## 24-hour schedule

Elapsed from the moment the Mac is reachable:

| Window | Work |
|---|---|
| Before rental | the gate above; package downloaded and verified on the operator machine |
| Hours 0-2 | VNC login, `caffeinate`, transfer, preflight, first evidence fetch; environment capture |
| Hours 2-6 | host-only, host and worker, and desktop full protocols; D-cells; upgrade cell |
| Hours 6-10 | repeated short runs, W04 build replacement, baseline and candidate profiles |
| Hours 10-12 | W08 endurance rehearsal, untouched except for the scheduled cycles |
| Hours 12-22 | analysis, focused reruns with `--prior`, unresolved cases documented; the optional 8-hour run only if it ends before hour 22 |
| Hours 22-24 | export and verify, cleanup, deletion once the 24-hour minimum lease has elapsed |

Since PRD v1.5 the required W08 evidence is the rehearsal, as on Windows and
Linux, so the active work fits in the first twelve hours; the lease still
bills 24.

A delay, an unavailable machine, or a failure that consumes the window leaves
the affected cells `pending`; arrange a separate rental rather than shortening
a protocol. A fix produces a new candidate SHA, a new package, and a rerun of
the cells its changed boundary invalidates. The rental ending is never a
waiver.

## Export and teardown

On the Mac:

```bash
stamp=$(date -u +%Y%m%dT%H%M%SZ)
tar -czf ~/macos-evidence-$stamp.tar.gz -C ~ paneflow-qualification/evidence -C ~/paneflow-macos-qualification-<sha12> bench/results PACKAGE.txt candidate/candidate.json
shasum -a 256 ~/macos-evidence-$stamp.tar.gz >~/macos-evidence-$stamp.tar.gz.sha256
```

On the operator machine, `scp` both files back, verify the checksum, and list
the archive before anything on the Mac is removed. Then, on the Mac, confirm
no fixture survives (`pgrep -fl paneflow`) and remove the qualification root,
the homes, and the package. Finally:

```bash
scw apple-silicon server delete <server id> zone=fr-par-1
scw apple-silicon server list zone=fr-par-1
```

The report records the deletion time and confirms the server is gone from the
list and from the console's billing view. The measurement artifacts go under
`bench/results/`, and the report under this directory as
`macos-<date>.md`, with one row per required cell as the runbook requires.
