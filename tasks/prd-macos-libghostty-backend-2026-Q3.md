[PRD]
# PRD: macOS libghostty Backend

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-08-26 | Arthur Jean | Initial draft |

## Problem Statement

Paneflow runs the Ghostty VT engine (`libghostty-vt`, pinned at `ae52f97dcac558735cfa916ea3965f247e5c6e9e`) on Linux x86_64, Linux aarch64, and Windows x86_64 MSVC. macOS is the only shipped platform still served exclusively by `alacritty_terminal`.

1. **Emulation divergence across the three shipped platforms.** `native/libghostty/manifest.toml` declares exactly three targets and none is `*-apple-darwin`. A macOS user gets a different VT implementation from a Linux or Windows user on the same build: different Kitty keyboard-protocol coverage, different reflow, different OSC handling, different scrollback semantics. Every terminal bug report now needs a platform qualifier before it can be reproduced.
2. **The gate matrix hides macOS-only regressions in the Ghostty codepath.** `run_tests.yml`'s `macos_check` job compiles `--target aarch64-apple-darwin` with default features, but `paneflow-terminal-ghostty` resolves to `stub::DisplayTerminal` there, so `engine`, `grid`, `snapshot`, `callbacks`, and the other 14 native modules are never type-checked against a Darwin target. Roughly 1900 lines of Ghostty runtime code have zero macOS compile coverage.
3. **The platform predicate is copy-pasted 174 times and is unmaintainable.** `any(all(target_os = "linux", feature = "libghostty-linux"), all(target_os = "windows", target_arch = "x86_64", target_env = "msvc", feature = "libghostty-windows"))` appears across 9 files (`pty_session.rs` 178 gate sites, `view.rs` 54, `backend_corpus.rs` 42, `input.rs` 40, `service_detector.rs` 26). Adding a third platform by textual expansion would push a single predicate past 20 lines and make every future edit a 9-file sweep with no compiler protection against missing one.
4. **`ghostty_session.rs` uses `target_os = "linux"` as a stand-in for POSIX.** 27 gate sites, 1 `cfg(unix)`, 0 macOS. The PTY master handling, reader-worker teardown, `SHUTDOWN_GRACE`, resize permission, and the `getpgid`/`waitid`/`kill` reap helpers at lines 2890 to 3160 are all POSIX code wearing a Linux label. macOS currently compiles none of it and would fall into the `windows` arm's `not(linux)` complement if the gate were naively widened.
5. **No macOS artifact production path exists.** `scripts/` holds `build-libghostty-linux.sh` and `build-libghostty-windows.ps1`. `verify-libghostty-package.sh` is ELF-only: it requires `readelf`, maps `uname -m` to x86_64/aarch64 only, and greps for `NEEDED.*libghostty[^]]*\.so`. There is no Mach-O equivalent, no `libghostty-macos.yml` workflow, and no macOS entry in the manifest's closed `TargetConfig` enum.

**Why now:** Linux and Windows are both shipped and stable, so the doctrine (pinned source SHA, prebuilt hash-verified static archive committed in-repo, zero build-time fetch, zero bindgen, zero Zig on the developer machine) is proven twice over and macOS is the last hole. The supporting CI already exists and is paid for: `run_tests.yml` runs a hard-required `macos_check` job plus a `macos_render_smoke` visual job on `macos-14`, and `release.yml` has a hard-required `aarch64-apple-darwin` leg that has shipped signed and notarized `.dmg` artifacts since v0.2.9-rc.4. The marginal cost of closing the loop is the artifact pipeline, not the platform bring-up.

## Overview

This PRD adds `aarch64-apple-darwin` as the fourth entry in `native/libghostty/manifest.toml` and wires the existing three-layer stack (`paneflow-libghostty-sys` raw ABI, `paneflow-terminal-ghostty` safe wrapper, `src-app` runtime adapter) to select it. It follows the exact rollout shape used for Linux and Windows: artifact first, typing second, runtime third, policy last, shipping opt-in behind `"terminal_backend": "ghostty"` with promotion to `auto` deferred to a follow-on PRD.

Six decisions were arbitrated before drafting. **Per-arch archives, never universal:** one Rust target maps to one archive, matching the existing model; a `lipo` universal binary would force a Darwin build host and break that mapping. **Cross-compile from a Linux host with pinned Zig 0.15.2:** upstream's `src/build/CombineArchivesStep.zig` branches on `target.result.os.tag.isDarwin() and comptime builtin.os.tag.isDarwin()`, taking Apple `libtool` on a Darwin host and `zig ar -M` elsewhere. Because `simd_profile = "upstream-default"` keeps the simd source set non-empty, every archive goes through that step, so host choice changes archive bytes. Building on Linux keeps the `zig ar -M` path we already normalize for Linux and reuses the existing libghostty runner. **Post-build strip is mandatory:** `-Dstrip` is inert for lib-vt at the pin (`Config.zig` defines it, but `GhosttyZig.zig`, `SharedDeps.zig`, and `GhosttyLibVt.zig` contain zero references), so `llvm-strip -S` per member plus `llvm-ar` deterministic repack is the only way to reach a stable archive. **`cfg(unix)` widening over a macOS copy of the Linux arm**, with an explicit macOS branch only where Linux is genuinely Linux. **The cfg alias refactor lands first**, because it is the single largest chantier that is fully verifiable on the Linux development machine with gates green on both sides of the change. **macOS ships opt-in**, mirroring the `prd-linux-libghostty-backend` then `prd-linux-libghostty-promotion` sequence.

The cross-build decision carries the only genuine unknown in this plan, so EP-002 opens with a spike. Web research found no documented prior art for a committed, checksum-verified macOS static archive produced from a Linux host; it also confirmed that Zig's Mach-O linker emits a non-reproducible `LC_ID_DYLIB` and random `LC_UUID` for **dylibs** (ziglang/zig#11737), and `build.zig` at the pin builds and installs `libghostty_vt_shared` unconditionally. The spike targets exactly that step. If it fails or proves irreproducible, the fallback is to build on the `macos-14` runner Paneflow already pays for, at the cost of a different `archive_normalization` recipe (Apple `libtool` instead of `zig ar -M`). Both recipes are specified so the fallback is a manifest change, not a redesign.

## Goals

| Goal | Month-1 Target | Month-6 Target |
|------|---------------|----------------|
| VT engine parity across shipped platforms | 3 of 4 release targets on libghostty (Linux x64, Linux arm64, Windows x64) plus macOS arm64 available opt-in | 4 of 4 on libghostty by default (`auto` promotion PRD) |
| macOS compile coverage of the Ghostty runtime | `cargo clippy --target aarch64-apple-darwin` type-checks all 14 native modules with the backend feature on | Unchanged, plus the render smoke job running the Ghostty backend |
| Platform predicate maintenance cost | 174 gate sites reduced to a single `ghostty_native` alias emitted by 3 build scripts | Adding a 5th target touches the manifest and 1 build script, not 9 files |
| Reproducible macOS artifact | 2 consecutive clean builds produce a byte-identical archive (SHA-256 equality) | Reproducibility asserted on every `libghostty-macos.yml` run |

## Target Users

### Paneflow macOS end user
- **Role:** Developer running coding agents in parallel on Apple Silicon, installing the signed `.dmg` from GitHub Releases.
- **Behaviors:** Runs Claude Code, Codex, and dev servers in split panes; heavy scrollback search; relies on Kitty keyboard protocol for agent CLIs that bind Ctrl and Alt chords.
- **Pain points:** Terminal behavior differs from the Linux machine they also use; a keybinding or reflow bug reproduces on one and not the other with no way to tell which engine is at fault.
- **Current workaround:** None available in-app. `terminal.backend = "ghostty"` is accepted by the config parser but silently resolves to Alacritty on macOS because the feature is not compiled in.
- **Success looks like:** Setting `"terminal_backend": "ghostty"` switches the engine, the app reports which backend is live, and a bad archive degrades to Alacritty instead of losing the pane.

### Paneflow maintainer (Arthur)
- **Role:** Sole contributor, developing on Fedora Linux with a Windows 11 dual-boot and no local macOS machine.
- **Behaviors:** Verifies every change with `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`, and `cargo deny check sources` before commit; treats CI as the only macOS feedback loop.
- **Pain points:** `gpui_macos` compiles `.metal` shaders, so no macOS `cargo check` is possible from Linux; any macOS-only mistake costs a full push, CI round-trip, and context switch.
- **Current workaround:** Read the Linux and Windows arms and reason about the Darwin equivalent by inspection.
- **Success looks like:** Every story in this PRD is either verifiable locally on Linux or lands with a named CI job that verifies it, and no story requires guessing at Darwin behavior without a gate that proves it.

### Downstream packager and auditor
- **Role:** Consumer of the release artifacts and the supply-chain claims in `deny.toml`, `THIRD_PARTY_NOTICES.md`, and `sbom.cdx.json`.
- **Behaviors:** Verifies that no build script fetches from the network, that every committed binary has a declared checksum and license, and that the SBOM matches the tree.
- **Pain points:** A committed prebuilt binary blob is a trust liability unless its provenance is scripted, pinned, and independently reproducible.
- **Current workaround:** Reads `manifest.toml` and re-runs `scripts/build-libghostty-linux.sh --verify-reproducible`.
- **Success looks like:** The macOS archive carries the same guarantees: pinned source SHA, pinned toolchain, scripted normalization, byte-reproducible, license entries present, `cargo deny check sources` unchanged.

## Research Findings

Key findings that informed this PRD.

### Competitive Context
- **cmux (`/home/arthur/dev/cmux`):** ships two consumption models. The Swift app consumes the full `GhosttyKit.xcframework` (renderer plus surface) from a submodule fork of `manaflow-ai/ghostty` tracking a moving branch. `cmux-tui/crates/ghostty-vt-sys` is the true Rust analog but inverts Paneflow's doctrine: `build.rs` invokes `zig build -Demit-lib-vt=true -Demit-xcframework=false -Doptimize=ReleaseFast` at compile time and runs `bindgen`, requiring a Zig toolchain on every developer machine. Its `zig_target_for_rust_target()` maps `aarch64-apple-darwin` to `aarch64-macos`, confirming the target string. Its CI (`cmux-tui-build-package.yml`) builds both macOS arches on a macOS runner and documents that "cargo-zigbuild cannot resolve SDK frameworks when the host is arm64."
- **ghostling (`ghostty-org/ghostling`):** the reference libghostty-vt consumer. Its `CMakeLists.txt` links `ghostty-vt` with no Apple frameworks attached; the `if (APPLE)` block adds `-framework IOKit/Cocoa/OpenGL` for **raylib**, not for the VT library, and the `if (UNIX AND NOT APPLE)` block adds `util` for `forkpty`. Its README states libghostty-vt is "a zero-dependency library (not even libc)."
- **Upstream Ghostty:** publishes a minisign-signed prebuilt `ghostty-vt.xcframework.zip` from `release-tip.yml`'s `build-lib-vt-xcframework` job to the `tip` prerelease and Cloudflare R2. Evaluated and rejected as our artifact source: it is universal with dead iOS slices, `libtool`-produced so not reproducible by us, depends on `tip` and R2 availability, and would make macOS the only platform whose archive Paneflow cannot rebuild. Retained as an emergency fallback only.
- **Market gap:** no shipped cross-platform terminal workspace runs one VT engine on Linux, macOS, and Windows from a committed, hash-verified, reproducible prebuilt archive. Closing macOS makes that claim true and is the differentiator against mac-only competitors.

### Best Practices Applied
- **`system_libraries = []` on macOS is upstream-enforced, not inferred.** Upstream `test.yml`'s `test-lib-vt-pkgconfig` job asserts `! pkg-config --libs --static libghostty-vt | grep -qE -- '-lc\+\+|-lc\+\+abi'` and verifies the shared library carries no libc++ dependency. Combined with ghostling's link line and the README's zero-dependency claim, macOS needs no `cargo:rustc-link-lib=framework=` directive, which is why `build_support/mod.rs` needs no new emission path.
- **`llvm-ar` is deterministic by default and infers archive format from its inputs** ([llvm-ar docs](https://llvm.org/docs/CommandGuide/llvm-ar.html)). When cross-hosting on Linux, format inference can fall back to the host default, so `--format=darwin` should be passed explicitly rather than relied on.
- **`llvm-strip` supports Mach-O** including `-S`/`--strip-debug` ([llvm-strip docs](https://llvm.org/docs/CommandGuide/llvm-strip.html)); no Linux-host-specific limitation is documented, but LLVM notes that format-specific flags may error or be silently ignored on other formats.
- **Mach-O reproducibility hazards are concentrated in link products, not archive members.** `LC_UUID` is random bytes under LLVM's lld and `N_OSO` stabs embed absolute source paths with object timestamps ([ld64 determinism writeup](https://milen.me/writings/apple-linker-ld64-deterministic-builds-oso-prefix/)). ziglang/zig#11737 confirms Zig-built **dylibs** are non-reproducible via `LC_ID_DYLIB` and `LC_UUID`; static archive members do not carry those load commands.
- **Pin the LLVM version in the manifest.** The Windows target already does this with `windows_llvm_version = "19.1.5"`; `llvm-strip` and `llvm-ar` output can drift between LLVM releases, so the macOS recipe must pin equivalently.

### Research Gaps
- No documented prior art was found for an open-source project committing a checksum-verified macOS static archive cross-built from a Linux host. General tooling exists (osxcross, `shepherdjerred/macos-cross-compiler`), but every located Rust-ecosystem example builds macOS targets on macOS runners. **This makes US-004 a mandatory spike, not a formality.**
- Zig 0.15.2-specific cross-compilation behavior to `aarch64-macos` from Linux is UNKNOWN from documentation; ziglang/zig#16557 records Clang errors cross-compiling to `aarch64-macos` with C library dependencies.
- Whether `llvm-strip -S` removes the `LC_UUID` load command from Mach-O objects is UNKNOWN and must be measured by the spike rather than assumed.

*Sources: upstream Ghostty at pin `ae52f97dc` (`build.zig`, `src/build/CombineArchivesStep.zig`, `src/build/Config.zig`, `src/build/GhosttyLibVt.zig`, `.github/workflows/test.yml`, `.github/workflows/release-tip.yml`), ghostling `CMakeLists.txt` and README, cmux `cmux-tui/crates/ghostty-vt-sys/build.rs` and `.github/workflows/`, LLVM command guides, ziglang/zig#11737 and #16557.*

## Assumptions & Constraints

### Assumptions (to validate)
- **HIGH RISK.** Zig 0.15.2 on a Linux host can complete `zig build -Demit-lib-vt=true -Dtarget=aarch64-macos -Doptimize=ReleaseFast` including the unconditional `libghostty_vt_shared` link step. Basis: `build.zig` gates the xcframework block on a Darwin host but does not gate the shared-library install. Validated by US-004; failure routes to the macos-14 fallback.
- **HIGH RISK.** The resulting archive is byte-reproducible across two clean builds after `llvm-strip -S` plus deterministic repack. Basis: the Linux recipe achieves this with the same `zig ar -M` combine path; Mach-O adds `LC_UUID` and `N_OSO` hazards not present in ELF. Validated by US-006.
- **MEDIUM RISK.** Every `cfg(target_os = "linux")` site in `ghostty_session.rs` that is not `/proc`- or `prctl`-adjacent is correct as `cfg(unix)` on Darwin. Basis: the file contains zero `/proc` and zero `prctl` references; its POSIX surface is `getpgid`, `waitid(P_PID, WEXITED|WNOHANG|WNOWAIT)`, `kill(-pid, …)`, and `strsignal`, all of which exist on Darwin. Validated by US-010 and US-012.
- **LOW RISK.** `libghostty-vt` needs no Apple frameworks, so `system_libraries = []`. Basis: three independent upstream signals (pkg-config CI assertion, ghostling link line, README).
- **LOW RISK.** The existing `macos_check` job absorbs the feature-enabled build within its runner budget. Basis: the job already runs clippy, test, check, and a release build on `macos-14`.

### Hard Constraints
- **No local macOS.** `gpui_macos` compiles `.metal` shaders through `xcrun --find metal`, so `cargo check --target aarch64-apple-darwin` cannot run on the Linux development machine. Every macOS behavioral claim must be proven by a named CI job.
- **No build-time fetch, no bindgen, no Zig on the developer machine.** Enforced by `src-app/tests/dependency_source_policy.rs` and the project doctrine. `build.rs` reads a committed archive and emits link directives; it never clones, downloads, or invokes a compiler toolchain.
- **`x86_64-apple-darwin` is a CLOSED release target** (`release.yml` lines 176 to 199, decision dated 2026-04-21). No Intel Mac binary ships, so no Intel archive is produced by this PRD.
- **Alacritty stays the rollback on every platform.** `--no-default-features` must continue to produce a working Alacritty-only build on macOS.
- **Cross-platform rule from `CLAUDE.md`.** No change may regress Linux or Windows. The three existing prebuilt trees and their manifest entries are frozen.
- **Rust toolchain 1.96.1**, pinned in `run_tests.yml`.
- **GPUI pins are frozen.** `gpui` and `gpui_platform` stay at `fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8`; this PRD does not bump them.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting; the release pipeline runs this on all four matrix legs and a single diff fails the whole run
- `cargo clippy --workspace -- -D warnings` - lint on the host (Linux) target
- `cargo test --workspace` - `cargo build` does not compile test targets, so test-only breakage is otherwise invisible
- `cargo deny check sources` - proves no new git source entered the tree

For stories that touch macOS-gated code, additionally (executed by the `macos_check` job on `macos-14`, not locally):
- `cargo clippy --workspace --locked --target aarch64-apple-darwin -- -D warnings`
- `cargo test --workspace --locked --target aarch64-apple-darwin`

For stories that touch the prebuilt artifact:
- `scripts/verify-libghostty-macos.sh` - Mach-O identity, architecture, static-linkage, and notice verification

## Epics & User Stories

### EP-001: Collapse the platform predicate behind a `ghostty_native` cfg alias

Replace the 174 copy-pasted occurrences of the two-platform predicate with one build-script-emitted cfg, so adding macOS is a one-line manifest and build-script change rather than a nine-file textual sweep. This epic lands first because it is entirely verifiable on the Linux development machine with all gates green before and after, and because it is behavior-preserving by construction.

**Definition of Done:** No source file outside the three build scripts spells the platform predicate. `cargo build`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` produce identical results before and after the refactor on Linux, and the Windows leg of `run_tests.yml` and `libghostty-windows.yml` stays green.

#### US-001: Emit the `ghostty_native` cfg from the build scripts
**Description:** As a maintainer, I want a single build-script-emitted `ghostty_native` cfg so that the platform-and-feature predicate has exactly one definition site per crate.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-libghostty-sys/build.rs`, when the target and feature predicate holds, then it prints `cargo::rustc-cfg=ghostty_native`, and it always prints `cargo::rustc-check-cfg=cfg(ghostty_native)` so an unset cfg never triggers the `unexpected_cfgs` lint.
- [ ] Given `crates/paneflow-terminal-ghostty`, which has no build script today, when the crate is built, then a new minimal `build.rs` emits the same two directives, declares no dependencies, performs no filesystem or network I/O, and the crate's `Cargo.toml` gains `build = "build.rs"`.
- [ ] Given `src-app/build.rs`, when it runs, then it emits the same two directives alongside its existing bridge-staging work, without altering that work.
- [ ] Given the predicate is evaluated in a build script, then it reads `CARGO_CFG_TARGET_OS`, `CARGO_CFG_TARGET_ARCH`, `CARGO_CFG_TARGET_ENV`, and the `CARGO_FEATURE_*` variables, never `cfg!()` on the host.
- [ ] Given a target with no Ghostty support (for example `x86_64-unknown-freebsd`), when the crate is built, then `ghostty_native` is not emitted, the build succeeds, and no `unexpected_cfgs` warning is produced.
- [ ] Given `cargo build --no-default-features -p paneflow-app` on Linux, when the build runs, then `ghostty_native` is absent and the Alacritty-only build links successfully.

#### US-002: Collapse the predicate in `paneflow-terminal-ghostty`
**Description:** As a maintainer, I want `paneflow-terminal-ghostty` to gate on `cfg(ghostty_native)` so that its 15 hand-written predicates and the `native_modules!` macro read as one condition.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-terminal-ghostty/src/lib.rs`, when the refactor lands, then every `#[cfg(all(feature = "native", any(...)))]` is `#[cfg(ghostty_native)]`, every `#[cfg(not(all(...)))]` is `#[cfg(not(ghostty_native))]`, and the `native_modules!` macro body carries the single alias.
- [ ] Given the `DisplayTerminal` re-export, when `ghostty_native` is set, then `engine::DisplayTerminal` is exported, and otherwise `stub::DisplayTerminal` is exported, with no configuration in which both or neither is exported.
- [ ] Given `cargo tree -p paneflow-terminal-ghostty --target aarch64-apple-darwin`, when run before and after the change, then the dependency set is unchanged.
- [ ] Given `identity_tests::build_identity_is_derived_from_the_pinned_manifest`, when the test runs on Linux, then it still asserts `api_version == "0.1.0"`, `zig_version == "0.15.2"`, `optimization == "ReleaseFast"`, and `simd == "upstream-default"`.
- [ ] Given a build with `--features native` on a target where `ghostty_native` is unset, when the crate compiles, then the stub path is selected and no reference to `paneflow_libghostty_sys` is emitted, so the missing optional dependency cannot cause an unresolved-import error.

#### US-003: Collapse the predicate across the `src-app` terminal modules
**Description:** As a maintainer, I want the six `src-app` terminal modules to gate on `cfg(ghostty_native)` so that the 340-plus remaining gate sites collapse to one alias and future platform additions stop touching this file set.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `pty_session.rs`, `view.rs`, `input.rs`, `service_detector.rs`, `backend_corpus.rs`, and `terminal/mod.rs`, when the refactor lands, then `rg -c 'feature = "libghostty-linux"' src-app/src` returns zero matches outside `src-app/build.rs`.
- [ ] Given the `TerminalBackendEvents` stream impls that today carry `not(any(...))` fallback arms, when the refactor lands, then those arms are `not(ghostty_native)` and both the `Stream` and `FusedStream` impls remain exactly one per configuration.
- [ ] Given `auto_selects_ghostty_for_target()` in `view.rs`, when the refactor lands, then its body is unchanged in meaning and it remains the single policy switch, since EP-006 is the only epic permitted to change its return value.
- [ ] Given the Linux build, when `cargo test --workspace` runs before and after, then the same test count passes with zero failures, proving the refactor is behavior-preserving.
- [ ] Given the Windows leg, when `libghostty-windows.yml` runs on the branch, then it stays green, proving the collapse did not silently drop the `target_env = "msvc"` or `target_arch = "x86_64"` condition on Windows.
- [ ] Given `should_render_ghostty_wakeup_immediately`, which is gated on `all(target_os = "linux", feature = "libghostty-linux")` and is therefore narrower than the general predicate, when the refactor lands, then it keeps a Linux-specific gate rather than being widened to the alias.

---

### EP-002: Produce a reproducible `aarch64-apple-darwin` archive

Build, normalize, verify, and commit the macOS static archive under the existing doctrine: pinned source, pinned toolchain, scripted normalization, hash-verified, byte-reproducible. This epic opens with a spike because no documented prior art exists for cross-building a committed macOS static archive from Linux.

**Definition of Done:** `native/libghostty/prebuilt/aarch64-apple-darwin/` contains an archive, header, `bindings.rs`, and `build-info.txt` whose SHA-256 values match a fresh `scripts/build-libghostty-macos.sh --verify-reproducible` run, and `manifest.toml` declares the target with its normalization recipe and LLVM pin.

#### US-004: Validate the Linux-host cross-build of `lib-vt` for `aarch64-macos`
**Description:** As a maintainer, I want a spike that proves or disproves the Linux-host cross-build so that the artifact pipeline is designed against measured behavior instead of the HIGH-risk assumption.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given a clean checkout of Ghostty at `ae52f97dcac558735cfa916ea3965f247e5c6e9e` and Zig exactly `0.15.2` on a Linux host, when `zig build -Demit-lib-vt=true -Dtarget=aarch64-macos -Doptimize=ReleaseFast --prefix <out>` runs, then the outcome is recorded verbatim as pass or fail with the full error text.
- [ ] Given the build reaches the `libghostty_vt_shared` link step, which `build.zig` installs unconditionally at the pin, when that step runs, then the spike records whether the macOS dylib link succeeds from Linux and, if it fails, whether `-Demit-lib-vt` can be satisfied without it.
- [ ] Given a successful build, when the output is inspected, then `lib/libghostty-vt.a` exists, `file` reports a Mach-O arm64 archive, and `llvm-nm -g --defined-only` lists `ghostty_build_info`.
- [ ] Given the produced archive, when `llvm-objdump --macho --all-headers` is run over its members, then the spike records whether any member carries `LC_UUID` or `N_OSO` entries, and whether `llvm-strip -S` removes them.
- [ ] Given two consecutive builds from separate clean `ZIG_GLOBAL_CACHE_DIR` and `ZIG_LOCAL_CACHE_DIR` directories, when both archives are normalized and compared with `cmp`, then the spike records byte equality or the exact first differing offset.
- [ ] Given the spike fails at any of the above, when it concludes, then it records the macos-14 fallback as selected, notes that `CombineArchivesStep` will take the Apple `libtool` branch on a Darwin host, and specifies `apple-libtool-combine+llvm-strip-debug+llvm-ar-D` as the resulting `archive_normalization` value in place of `llvm-strip-debug+llvm-ar-D`.
- [ ] Given the spike concludes either way, when it is closed, then its findings are written to `docs/release/macos-libghostty.md` so the decision is auditable without re-running it.

#### US-005: Add `scripts/build-libghostty-macos.sh`
**Description:** As a maintainer, I want a manifest-driven macOS build script so that the archive is produced by pinned, scripted, auditable steps rather than by hand.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-004

**Acceptance Criteria:**
- [ ] Given the script starts, when it parses configuration, then every pinned input (`source_sha`, `zig_version`, `header_sha256`, `bindings_sha256`, `build_mode`, `macos_llvm_version`) is read from `native/libghostty/manifest.toml`, never hardcoded, matching the parser pattern in `scripts/build-libghostty-linux.sh`.
- [ ] Given a required tool is missing or mismatched, when the preflight runs, then the script exits non-zero before any build work with a message naming the tool and the expected version, covering at minimum `zig` (equality against `zig_version`), `llvm-strip`, `llvm-ar`, `llvm-nm`, `llvm-objdump`, `sha256sum`, and `file`.
- [ ] Given the checked-out Ghostty source, when its HEAD does not equal `source_sha`, then the script exits non-zero and does not build.
- [ ] Given the build completes, when normalization runs, then `llvm-strip -S` is applied per member, the archive is repacked with `llvm-ar` in deterministic mode with `--format=darwin` passed explicitly, and duplicate member basenames are rejected exactly as the Linux script does.
- [ ] Given the normalized archive, when the export check runs, then `llvm-nm -g --defined-only` must list `ghostty_build_info` or the script exits non-zero.
- [ ] Given the script finishes, when `build-info.txt` is written, then it carries the same ten keys as the Linux script (`source_sha`, `zig_version`, `header_sha256`, `bindings_sha256`, `rust_target`, `zig_target`, `optimize`, `archive_normalization`, `archive_sha256`, `build_info_symbol`) with `rust_target = aarch64-apple-darwin` and `zig_target = aarch64-macos`.
- [ ] Given `scripts/build-libghostty-windows.ps1` and `scripts/build-libghostty-linux.sh`, when this story lands, then neither file is modified.

#### US-006: Gate the macOS archive on byte reproducibility
**Description:** As an auditor, I want `--verify-reproducible` on the macOS script so that the committed binary blob carries the same reproducibility guarantee as the Linux one.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-005

**Acceptance Criteria:**
- [ ] Given `scripts/build-libghostty-macos.sh --verify-reproducible`, when it runs, then it performs a second build from a freshly emptied Zig cache and compares the two normalized archives with `cmp`, exiting non-zero on any difference.
- [ ] Given the two archives differ, when the script reports, then it prints the first differing byte offset and the output of `llvm-objdump --macho --all-headers` for the differing member, so a `LC_UUID` or `N_OSO` regression is diagnosable from the log alone.
- [ ] Given reproducibility depends on a normalization step, when that step is chosen, then `archive_normalization` in the manifest names every tool in the recipe in application order, matching the precedent set by the Windows entry.
- [ ] Given `SOURCE_DATE_EPOCH` or an equivalent timestamp control is required to reach determinism, when it is introduced, then it is recorded as a `macos_source_date_epoch` manifest key rather than an environment assumption.

#### US-007: Commit the prebuilt tree and declare the manifest target
**Description:** As a maintainer, I want the `aarch64-apple-darwin` artifact committed and declared so that `cargo build` links it with no toolchain on the developer machine.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006, US-008

**Acceptance Criteria:**
- [ ] Given `native/libghostty/manifest.toml`, when the target is declared, then `[targets."aarch64-apple-darwin"]` carries `platform = "macos"`, `archive_path = "lib/libghostty-vt.a"`, `archive_sha256`, `archive_normalization`, `zig_target = "aarch64-macos"`, `link_name = "ghostty-vt"`, `system_libraries = []`, and `build_info_symbol = "ghostty_build_info"`.
- [ ] Given the global manifest keys, when the target is added, then `macos_llvm_version` is present and pinned, matching the `windows_llvm_version = "19.1.5"` precedent.
- [ ] Given `native/libghostty/prebuilt/aarch64-apple-darwin/`, when it is committed, then it contains `lib/libghostty-vt.a`, `include/ghostty/vt.h`, `bindings.rs`, and `build-info.txt`, and the tree is at most 3.0 MB.
- [ ] Given the committed archive, when its SHA-256 is computed, then it equals the `archive_sha256` value in the manifest and the `archive_sha256` line in `build-info.txt`.
- [ ] Given `native/libghostty/THIRD_PARTY_NOTICES.md` and `native/libghostty/sbom.cdx.json`, when the macOS artifact is added, then both are regenerated if their content changes and `notice_sha256` and `sbom_sha256` are updated to match, so the checksum gate cannot pass against a stale notice.
- [ ] Given the three existing prebuilt trees, when this story lands, then none of their files and none of their manifest entries are modified.

---

### EP-003: Open the build-support type system to a third platform

`build_support/manifest.rs` and `build_support/artifact.rs` model exactly two platforms in closed enums with exhaustive matches. Add the macOS variant so an undeclared or malformed macOS entry fails loudly at build time with an actionable message, matching how Linux and Windows already behave.

**Definition of Done:** `cargo test -p paneflow-libghostty-sys` passes with macOS coverage, a macOS manifest entry that violates a platform rule produces a named error naming `scripts/build-libghostty-macos.sh`, and Linux and Windows validation behavior is unchanged.

#### US-008: Add the macOS variant to the manifest contract types
**Description:** As a maintainer, I want `TargetConfig`, `NativePlatform`, and `PlatformContract` to model macOS so that manifest validation covers the new target instead of silently rejecting it.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given `#[serde(tag = "platform", rename_all = "lowercase")] enum TargetConfig`, when a `platform = "macos"` entry is parsed, then it deserializes into a `Macos` variant rather than failing with an unknown-variant error.
- [ ] Given `NativePlatform` and `PlatformContract`, when the macOS variant is added, then every exhaustive match over them compiles without a catch-all arm, so the compiler enforces coverage of any future platform.
- [ ] Given `corrective_action()`, when it is called for a macOS target, then the returned message names `scripts/build-libghostty-macos.sh`, mirroring how the Linux and Windows arms name their own scripts.
- [ ] Given `expected_build_info()`, when it is called for a macOS target, then it requires the same ten keys the macOS build script writes, and a `build-info.txt` missing any one of them fails validation with the missing key named.
- [ ] Given a macOS manifest entry that declares a non-empty `system_libraries`, when validation runs, then it is rejected with a message stating that libghostty-vt requires no system libraries on macOS, matching the Linux rule and inverting the Windows rule.
- [ ] Given the existing `rejects_undeclared_link_target` test, which asserts on `aarch64-apple-darwin`, when macOS becomes a declared target, then the test is repointed to a still-undeclared triple such as `x86_64-apple-darwin` and continues to assert the `linking is unsupported` message.
- [ ] Given a `platform` value that is neither `linux`, `windows`, nor `macos`, when it is parsed, then deserialization fails with an error naming the unrecognized value.

#### US-009: Extend the artifact inventory rules to macOS
**Description:** As a maintainer, I want `ArtifactBundle` to know which files a macOS bundle requires so that a truncated or tampered artifact directory fails resolution instead of producing a broken link.

**Priority:** P0
**Size:** S (2 pts)
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given `ArtifactBundle::resolve` and `validate`, when the target is macOS, then the required inputs are the archive, the header, `bindings.rs`, and `build-info.txt`, and the Windows-only `headers.sha256` and `symbols.txt` are not required.
- [ ] Given the `is_windows` predicate that today drives inventory selection, when macOS is added, then the selection is driven by the platform contract rather than by a boolean, so a third platform cannot be misrouted into the Windows inventory.
- [ ] Given any required file is missing, when resolution runs, then the build fails with an error naming the missing path and the corrective script from US-008.
- [ ] Given a symlink placed at any required path inside the macOS prebuilt directory, when `validate_regular_file_beneath` runs, then resolution is rejected, preserving the existing symlink-hardening behavior.
- [ ] Given `build_support/mod.rs`, when the macOS contract is linked, then it emits `cargo:rustc-link-search=native=<dir>` and `cargo:rustc-link-lib=static=ghostty-vt`, and because `system_libraries` is empty it emits no `dylib` and no `framework` directive.

---

### EP-004: Wire the macOS runtime

Widen the POSIX surface of `ghostty_session.rs` from Linux-only to `cfg(unix)`, declare the `libghostty-macos` feature and its target dependencies, and prove the Darwin process-lifecycle semantics hold.

**Definition of Done:** `cargo clippy --workspace --locked --target aarch64-apple-darwin --features paneflow-app/libghostty-macos -- -D warnings` and the equivalent `cargo test` pass on `macos-14`, and the Alacritty-only build still succeeds with `--no-default-features`.

#### US-010: Widen the POSIX gates in `ghostty_session.rs`
**Description:** As a maintainer, I want the POSIX code in `ghostty_session.rs` gated on `cfg(unix)` so that macOS compiles the same session lifecycle as Linux instead of falling into the Windows complement.

**Priority:** P0
**Size:** L (5 pts)
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Given the 27 `cfg(target_os = "linux")` sites, when the widening lands, then each is either `cfg(unix)` or retains an explicit Linux gate with a comment stating why, and no site is left as an implicit complement of `cfg(windows)`.
- [x] Given the PTY master handling, when built for macOS, then the `let master = pair.master` path is taken rather than the Windows `DrainablePtyMaster` path, and `try_clone_reader()` and `take_writer()` are used identically to Linux.
- [x] Given the loop locals `eof`, `exit`, `exit_seen_at`, and `child_cleaned`, when built for macOS, then they are compiled rather than the Windows `RuntimeLifecycle` struct.
- [x] Given `SHUTDOWN_GRACE`, `resize_allowed`, `active_master`, and the `*runtime_failed = !expected_close` assignment, when built for macOS, then each is present with the same value and semantics as on Linux.
- [x] Given the environment the session exports, when a macOS Ghostty session spawns, then `TERM_PROGRAM=ghostty` and `TERM_PROGRAM_VERSION` equal to `GHOSTTY_APP_VERSION` are set, matching Linux and Windows.
- [x] Given the file contains zero `/proc` and zero `prctl` references today, when the widening lands, then that remains true and any newly introduced Linux-only branch is justified in a comment naming the Linux-specific API it needs.
- [x] Given a build for a Unix target that is neither Linux nor macOS, when it compiles, then either it succeeds through the `cfg(unix)` arms or it fails at a single named location rather than producing a partially-configured session type.

#### US-011: Declare the `libghostty-macos` feature and its target dependencies
**Description:** As a maintainer, I want a `libghostty-macos` feature symmetric with the Linux and Windows features so that the backend is selectable on macOS without disturbing the other two platforms.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-002, US-009, US-010

**Acceptance Criteria:**
- [x] Given `src-app/Cargo.toml`, when the feature is added, then `libghostty-macos = ["dep:portable-pty", "paneflow-terminal-ghostty/native"]` mirrors the existing two features, and `default` becomes `["libghostty-linux", "libghostty-windows", "libghostty-macos"]`.
- [x] Given `[target.'cfg(target_os = "macos")'.dependencies]`, when the feature is added, then `portable-pty = { version = "0.9.0", optional = true }` is declared there, and the existing macOS entries (`libproc`, `core-text`, `core-foundation`, `raw-window-handle`, `cocoa`, `objc`) are unchanged.
- [x] Given `crates/paneflow-terminal-ghostty/Cargo.toml`, when the feature is added, then the target-gated `paneflow-libghostty-sys` dependency section includes `target_os = "macos"`.
- [x] Given `crates/paneflow-libghostty-sys/src/lib.rs`, whose bindings module is gated on `any(target_os = "linux", target_os = "windows")`, when the feature is added, then `target_os = "macos"` is included.
- [x] Given `crates/paneflow-ghostty-smoke/Cargo.toml`, whose dependencies are target-gated to Linux and Windows, when the feature is added, then macOS is included in the `paneflow-terminal-ghostty` gate so the smoke crate compiles on Darwin.
- [x] Given `cargo build --no-default-features -p paneflow-app --target aarch64-apple-darwin`, when it runs on `macos-14`, then it succeeds and produces an Alacritty-only binary, preserving the rollback path.
- [x] Given `src-app/tests/dependency_source_policy.rs`, when the feature is added, then the test still passes, proving no build script gained a network or VCS fetch.

#### US-012: Prove the Darwin process-lifecycle semantics
**Description:** As a maintainer, I want the process reap and teardown helpers exercised on macOS so that a Darwin difference in `waitid` or process-group signaling surfaces in CI rather than as an orphaned agent process on a user's machine.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given the helper that returns the process-group leader via `libc::getpgid(pid) == pid`, when it runs on macOS, then it returns the same result as on Linux for a spawned shell that is its own group leader.
- [x] Given `libc::waitid(P_PID, pid, WEXITED | WNOHANG | WNOWAIT)`, when it is called on macOS for a still-running child, then it does not block and does not consume the exit status, so a subsequent reap still observes it.
- [x] Given a child that exits normally, when the reap path runs on macOS, then `CLD_EXITED` maps to `ExitStatus::with_exit_code` with the same code Linux reports.
- [x] Given a child killed by a signal, when the reap path runs on macOS, then `CLD_KILLED` or `CLD_DUMPED` is handled and `libc::strsignal` is used without a null-pointer dereference.
- [x] Given the graceful-shutdown path that sends `SIGTERM` to `-pid` then escalates to `SIGKILL` after `SHUTDOWN_GRACE`, when a macOS session closes, then the process group is gone within the grace window plus 1 second, verified by `kill(-pid, 0)` returning `ESRCH`.
- [x] Given the existing Linux test at `ghostty_session.rs:4364` that asserts `libc::kill(child_pid, 0) == -1` with `ESRCH`, when this story lands, then an equivalent assertion runs under `cfg(unix)` and executes in the `macos_check` job.
- [x] Given a child that ignores `SIGTERM`, when the grace window elapses on macOS, then `SIGKILL` is delivered and the session still reports a clean close rather than hanging.

---

### EP-005: Gate macOS in CI, packaging, and supply chain

Extend the existing macOS CI surface to exercise the Ghostty backend and add the macOS analogs of the artifact verification and libghostty workflow that Linux and Windows already have.

**Definition of Done:** A pull request touching any macOS Ghostty path runs a `macos_check` job with the feature enabled, a `libghostty-macos.yml` workflow that verifies the archive, and a release leg that ships a `.dmg` containing a statically linked Ghostty engine.

#### US-013: Exercise the macOS backend in `macos_check`
**Description:** As a maintainer, I want the existing `macos_check` job to build with `libghostty-macos` so that the 14 native modules are type-checked against a Darwin target on every relevant pull request.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given `run_tests.yml`'s `macos_check` job, when it runs, then it executes `cargo clippy --workspace --locked --target aarch64-apple-darwin -- -D warnings` and `cargo test --workspace --locked --target aarch64-apple-darwin` with `libghostty-macos` active.
- [x] Given the same job, when it runs, then it additionally executes one `--no-default-features` check so the Alacritty rollback is gated on every pull request rather than only at release time.
- [x] Given the job's existing Xcode and Metal preflight step, when this story lands, then that step is unchanged, since it guards `gpui_macos` and is unrelated to libghostty.
- [x] Given the `orchestrate` path filter, when a pull request touches `native/libghostty/**`, `crates/paneflow-libghostty-sys/**`, `crates/paneflow-terminal-ghostty/**`, or `src-app/src/terminal/**`, then `macos_check` runs; when a pull request touches only documentation, then it is skipped.
- [x] Given the job's total wall time, when measured over the first five runs after this story lands, then the median is at most 25 minutes; if it exceeds that, the runner is switched to `macos-14-xlarge` and the rationale is recorded in the pull request per the existing US-001 AC5 note in the workflow.
- [x] Given the archive is missing or its checksum is wrong, when the job builds, then it fails at the `paneflow-libghostty-sys` build script with the corrective message from US-008 rather than at link time with an unresolved symbol.

#### US-014: Add `scripts/verify-libghostty-macos.sh`
**Description:** As an auditor, I want a Mach-O verification script so that a packaged macOS binary can be proven to carry the pinned, statically linked engine.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given the packaged `paneflow` binary, when the script runs, then it verifies via `file` or `llvm-objdump --macho` that the binary is Mach-O 64-bit `arm64`, exiting non-zero on any other architecture.
- [x] Given the binary's load commands, when the script inspects them with `otool -L` or `llvm-objdump --macho --dylibs-used`, then no `libghostty` dynamic library appears, proving static linkage, mirroring the ELF `NEEDED.*libghostty.*\.so` rejection in `verify-libghostty-package.sh`.
- [x] Given the binary's contents, when the script searches them, then the pinned `source_sha` string is present, matching the existing Linux identity check.
- [x] Given `native/libghostty/THIRD_PARTY_NOTICES.md`, when the script runs, then its SHA-256 is compared against `notice_sha256` and a mismatch exits non-zero.
- [x] Given the script runs on a host without `otool`, when it starts, then it falls back to `llvm-objdump` or exits non-zero with a message naming the missing tool, never skipping a check silently.
- [x] Given `scripts/verify-libghostty-package.sh`, when this story lands, then it is not modified, so the Linux verification path is untouched.

#### US-015: Add the `libghostty-macos.yml` workflow
**Description:** As a maintainer, I want a macOS libghostty workflow so that archive production, reproducibility, and ABI checks are gated on the same paths the Linux and Windows workflows already gate.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-006, US-014

**Acceptance Criteria:**
- [x] Given `.github/workflows/libghostty-macos.yml`, when it is added, then its `paths` filter covers `native/libghostty/**`, `scripts/*libghostty*`, `crates/paneflow-libghostty-sys/**`, `crates/paneflow-terminal-ghostty/**`, and `src-app/src/terminal/**`, matching the Linux workflow's shape.
- [x] Given the workflow runs, when the archive job executes, then it rebuilds the archive from the pinned source, asserts reproducibility per US-006, and fails if the produced SHA-256 differs from the committed `archive_sha256`.
- [x] Given the workflow runs, when the verification job executes, then it invokes `scripts/verify-libghostty-macos.sh` against a freshly built binary.
- [x] Given the workflow has a `validation-summary` job, when any upstream job fails or is skipped, then the summary fails, matching the `Require every automated check` pattern in `libghostty-linux.yml`.
- [x] Given the spike selected the Linux-host cross-build, when the workflow is authored, then the archive job runs on `ubuntu-22.04`; given it selected the fallback, then it runs on `macos-14` and the workflow comment records which and why.

#### US-016: Ship the macOS Ghostty backend in the release pipeline
**Description:** As a macOS user, I want the released `.dmg` to contain the Ghostty engine so that the opt-in config switch has something to select.

**Priority:** P0
**Size:** M (3 pts)
**Dependencies:** Blocked by US-013, US-014

**Acceptance Criteria:**
- [x] Given `release.yml`'s build step, whose current logic sets `feature_args=(--no-default-features)` and clears it only when `RUNNER_OS = Linux`, when this story lands, then the macOS leg builds with `libghostty-macos` enabled.
- [x] Given the macOS leg, when the build completes, then a `Verify packaged libghostty identity and static linkage` step runs `scripts/verify-libghostty-macos.sh`, matching the Linux step that exists today at line 556.
- [x] Given the macOS clippy step, which currently runs `--no-default-features`, when this story lands, then it runs with the macOS feature enabled so release-time lint matches pull-request-time lint.
- [x] Given the `.dmg` pipeline, when the release runs, then bundling, signing, notarization, and stapling are unchanged and the artifact stays signed, since none of those steps depend on the terminal backend.
- [x] Given the released binary, when its size is measured, then it grew by at most 2.5 MB relative to the previous release, and a larger increase fails the existing binary-size budget rather than shipping silently.
- [x] Given the macOS archive is absent or corrupt at release time, when the leg builds, then it fails hard rather than falling back to `--no-default-features`, so a release can never silently ship without the engine it claims.
- [x] Given `cargo deny check sources`, when it runs on the release branch, then the `allow-git` list in `deny.toml` is unchanged, proving no new git source entered the tree.

---

### EP-006: Ship macOS opt-in and document the rollout

Make `"terminal_backend": "ghostty"` functional on macOS while leaving `auto` on Alacritty, so the engine gets real-world exposure before it becomes the default. Promotion to `auto` is a separate PRD.

**Definition of Done:** A macOS user setting `"terminal_backend": "ghostty"` gets the Ghostty engine, `auto` still selects Alacritty, a failed attach falls back without losing the pane, and the documentation set states the macOS status accurately.

#### US-017: Honor the explicit `ghostty` backend selection on macOS
**Description:** As a macOS user, I want `"terminal_backend": "ghostty"` to actually switch engines so that I can opt into the new backend without a custom build.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given `auto_selects_ghostty_for_target()` in `view.rs`, when this story lands, then it still returns `false` on macOS, so `auto` continues to select Alacritty and this PRD makes no default-behavior change.
- [x] Given `policy_requests_ghostty` with `TerminalBackendConfig::Ghostty`, when a pane spawns on macOS with the feature compiled in, then `should_start_ghostty` returns true and the Ghostty session is created.
- [x] Given `TerminalBackendConfig::Alacritty` on macOS, when a pane spawns, then the Alacritty session is created regardless of feature state.
- [x] Given a config file containing an unrecognized backend value such as `"terminal_backend": "gostty"`, when it is parsed on macOS, then the manual `Deserialize` logs a warning and falls back to Alacritty, and the rest of the config (theme, shell, shortcuts, agent settings) is preserved.
- [x] Given `"terminal_backend": "ghostty"` on a macOS build compiled with `--no-default-features`, when a pane spawns, then it falls back to Alacritty and `warn_ghostty_unavailable_once` logs exactly one warning for the process lifetime.

#### US-018: Fall back and report on macOS backend failure
**Description:** As a macOS user, I want a failed Ghostty attach to degrade to Alacritty so that a bad artifact costs me a log line, not a dead pane.

**Priority:** P1
**Size:** M (3 pts)
**Dependencies:** Blocked by US-017

**Acceptance Criteria:**
- [x] Given `backend_diagnostics()`, when it is called on macOS, then it reports the live backend, the `PANEFLOW_TARGET_TRIPLE` value `aarch64-apple-darwin`, and the `BuildIdentity` fields (`source_sha`, `api_version`, `zig_version`, `optimization`, `simd`).
- [x] Given a Ghostty attach that fails before the PTY spawns, when the failure is handled on macOS, then `fallback_from_ghostty` produces `BackgroundSpawnOutcome::GhosttyFallback`, an Alacritty session is created, and the user sees a working pane.
- [x] Given a Ghostty failure after a successful spawn, when it is handled on macOS, then `fail_ghostty_after_spawn` produces `BackgroundSpawnOutcome::GhosttyPostSpawnFailed` and `record_backend_failure` is called exactly once.
- [x] Given repeated failures across multiple panes, when they occur, then the warning is emitted once per process, not once per pane.
- [x] Given `RUST_LOG=info`, when a macOS pane spawns successfully with the Ghostty backend, then a single line names the selected backend so a bug report can state which engine produced the behavior.

#### US-019: Document the macOS backend status
**Description:** As a user and as an auditor, I want the documentation to state the macOS backend status accurately so that nobody infers a default that does not exist.

**Priority:** P1
**Size:** S (2 pts)
**Dependencies:** Blocked by US-016, US-017

**Acceptance Criteria:**
- [x] Given `crates/paneflow-config/src/schema/terminal.rs`, whose doc comment currently states that macOS uses Alacritty, when this story lands, then it states that macOS supports Ghostty as an explicit opt-in and that `auto` still selects Alacritty.
- [x] Given `docs/user/configuration/schema.md`, `docs/user/settings.md`, and `schemas/paneflow.schema.json`, when this story lands, then each reflects the macOS opt-in status.
- [x] Given `docs/release/windows-libghostty.md` as the precedent, when this story lands, then `docs/release/macos-libghostty.md` exists and documents the build host decision from US-004, the normalization recipe, the pinned LLVM version, and the rebuild procedure.
- [x] Given `CLAUDE.md`, when this story lands, then its libghostty and cross-platform sections name macOS alongside Linux and Windows, and no statement claiming macOS is Alacritty-only remains.
- [x] Given every document touched, when it is written, then it is in US English and contains no em dash glyph.
- [x] Given a reader who wants to rebuild the macOS archive from scratch, when they follow `docs/release/macos-libghostty.md`, then every pinned input and every required tool version is stated in the document without needing to read the script.

## Functional Requirements

- FR-01: The system must link `libghostty-vt` statically into the macOS binary from a committed, checksum-verified archive, with no network access, no bindgen invocation, and no Zig toolchain required at `cargo build` time.
- FR-02: The system must reject a macOS artifact whose SHA-256 does not match `manifest.toml`, failing the build with a message naming `scripts/build-libghostty-macos.sh`.
- FR-03: The system must select the Ghostty backend on macOS when and only when `terminal.backend` is explicitly `"ghostty"` and the `libghostty-macos` feature is compiled in.
- FR-04: The system must continue to select Alacritty on macOS when `terminal.backend` is `"auto"`, until a separate promotion PRD changes it.
- FR-05: When a Ghostty session cannot be created or fails after spawning on macOS, the system must fall back to Alacritty, keep the pane usable, and log the failure exactly once per process.
- FR-06: The system must NOT emit any `cargo:rustc-link-lib=framework=` or `cargo:rustc-link-lib=dylib=` directive for the macOS target, because `system_libraries` is empty.
- FR-07: The system must NOT produce or commit a universal (`lipo`) archive, and must NOT commit an `x86_64-apple-darwin` archive while that release target is closed.
- FR-08: The build scripts must expose `ghostty_native` as the single cfg alias for "the native Ghostty FFI layer is linked", and no source file outside a build script may spell the underlying platform-and-feature predicate.
- FR-09: `--no-default-features` must continue to produce a working Alacritty-only build on all three platforms.
- FR-10: The macOS archive must be reproducible: two clean builds from the pinned source and toolchain must produce byte-identical normalized archives.

## Non-Functional Requirements

- **Artifact size:** the committed `native/libghostty/prebuilt/aarch64-apple-darwin/` tree is at most 3.0 MB, within the range set by the three existing trees (2.2 MB, 2.5 MB, 2.8 MB). Total `native/libghostty/` at most 11.0 MB.
- **Binary size:** the released macOS binary grows by at most 2.5 MB versus the preceding release, enforced by the existing binary-size budget step in `run_tests.yml`.
- **Reproducibility:** 2 of 2 consecutive clean builds produce byte-identical archives (SHA-256 equality), asserted on every `libghostty-macos.yml` run.
- **CI wall time:** the `macos_check` job completes in at most 25 minutes at the median over its first 5 runs after US-013; exceeding it triggers the documented `macos-14-xlarge` switch.
- **Reliability:** a Ghostty attach failure on macOS costs at most 1 additional spawn attempt, results in 0 lost panes, and emits exactly 1 warning per process.
- **Process cleanup:** on macOS session close, the child process group is fully reaped within `SHUTDOWN_GRACE` plus 1 second, verified by `kill(-pid, 0)` returning `ESRCH`.
- **Supply chain:** 0 new entries in `deny.toml`'s `allow-git` list; 0 new runtime crate dependencies; `cargo deny check sources` passes unchanged; `src-app/tests/dependency_source_policy.rs` passes unchanged.
- **Compile coverage:** 100% of the 14 `native_modules!` entries type-check under `--target aarch64-apple-darwin` with the feature enabled, up from 0% today.
- **Maintenance:** after EP-001, adding a 5th target requires editing at most 3 build scripts and 1 manifest, versus 9 source files today.

## Edge Cases & Error States

| # | Scenario | Trigger | Expected Behavior | User Message |
|---|----------|---------|-------------------|--------------|
| 1 | Missing macOS artifact | `native/libghostty/prebuilt/aarch64-apple-darwin/` absent or incomplete | `paneflow-libghostty-sys` build script fails before link, naming the missing path | "libghostty artifact for aarch64-apple-darwin is incomplete: <path> not found. Run scripts/build-libghostty-macos.sh." |
| 2 | Checksum mismatch | Committed archive edited or corrupted in transit | Build fails at checksum verification, never links | "archive_sha256 mismatch for aarch64-apple-darwin: expected <a>, found <b>." |
| 3 | Cross-build fails at the dylib link step | Zig 0.15.2 on Linux cannot link the macOS shared library | US-004 records the failure and the pipeline moves to the macos-14 host with the `apple-libtool-combine` normalization recipe | Maintainer-facing only, recorded in `docs/release/macos-libghostty.md` |
| 4 | Non-reproducible archive | `LC_UUID` or `N_OSO` survives normalization | `--verify-reproducible` exits non-zero, prints the first differing offset and the differing member's Mach-O headers | "archive is not reproducible: first difference at byte <n> in member <name>." |
| 5 | Wrong architecture in the artifact | An x86_64 or universal archive placed in the aarch64 directory | `verify-libghostty-macos.sh` rejects it via `file` or `llvm-objdump` | "expected Mach-O arm64 archive, found <actual>." |
| 6 | Dynamic rather than static linkage | Packaging regression leaves a `libghostty` dylib reference | `verify-libghostty-macos.sh` rejects the binary | "packaged binary links libghostty dynamically; static linkage is required." |
| 7 | Backend unavailable at runtime | `"terminal_backend": "ghostty"` on a `--no-default-features` macOS build | Fall back to Alacritty, one warning per process, pane works | "Ghostty backend is not available in this build; using Alacritty." |
| 8 | Attach failure before spawn | Engine construction fails on macOS | `BackgroundSpawnOutcome::GhosttyFallback`, Alacritty session created, pane usable | "Ghostty backend failed to start; falling back to Alacritty." |
| 9 | Failure after successful spawn | Engine faults mid-session on macOS | `GhosttyPostSpawnFailed`, `record_backend_failure` called once, session closes cleanly rather than hanging | "Ghostty backend failed; this pane has been closed." |
| 10 | Unrecognized config value | `"terminal_backend": "gostty"` | Manual `Deserialize` logs a warning, falls back to Alacritty, preserves the rest of the config | "unknown terminal backend 'gostty'; using alacritty." |
| 11 | Child ignores SIGTERM | An agent CLI traps SIGTERM on macOS | `SIGKILL` after `SHUTDOWN_GRACE`, process group reaped, session reports clean close | n/a |
| 12 | Cfg alias unset on an unsupported target | Build for `x86_64-unknown-freebsd` | `ghostty_native` not emitted, `rustc-check-cfg` suppresses `unexpected_cfgs`, stub backend selected, build succeeds | n/a |
| 13 | Stale notice or SBOM | Artifact regenerated without refreshing `THIRD_PARTY_NOTICES.md` | `notice_sha256` or `sbom_sha256` mismatch fails the gate | "notice_sha256 mismatch; regenerate THIRD_PARTY_NOTICES.md." |
| 14 | Release without the engine | macOS release leg silently falls back to `--no-default-features` | Leg fails hard; no release is published | Maintainer-facing CI failure |

## Risks & Mitigations

| # | Risk | Probability | Impact | Mitigation |
|---|------|------------|--------|------------|
| 1 | Zig 0.15.2 cannot cross-build lib-vt to `aarch64-macos` from Linux, because `build.zig` installs `libghostty_vt_shared` unconditionally and ziglang/zig#16557 records Clang failures on that path | Med | High | US-004 is a mandatory spike gating all downstream artifact work. Fallback is the `macos-14` runner Paneflow already pays for, with `apple-libtool-combine+llvm-strip-debug+llvm-ar-D` as the normalization recipe. Both recipes are specified, so the fallback is a manifest edit. |
| 2 | The macOS archive is not byte-reproducible because `LC_UUID` or `N_OSO` survives normalization | Med | Med | US-006 gates on `cmp` and prints the differing member's Mach-O headers. Escalation path: `-no_uuid`, `-oso_prefix`, or a pinned `macos_source_date_epoch`, each recorded in `archive_normalization`. |
| 3 | No documented prior art exists for a committed macOS static archive cross-built from Linux, so unknown failure modes may surface late | Med | Med | Treat US-004 as a real spike with a written record in `docs/release/macos-libghostty.md`; do not begin US-005 until it concludes either way. |
| 4 | The `cfg(unix)` widening is wrong somewhere and no local verification is possible | Med | High | `run_tests.yml` already has a hard-required `macos_check` job running clippy, test, check, and release build on `macos-14`, plus a `macos_render_smoke` visual job. EP-001 lands first so the widening is a small diff against a collapsed predicate rather than a sweep. |
| 5 | The EP-001 refactor silently drops a condition (for example `target_env = "msvc"`) and changes Windows behavior | Low | High | US-003 requires the Windows leg green on the branch, and requires the same `cargo test --workspace` count before and after on Linux. The narrower `should_render_ghostty_wakeup_immediately` gate is called out explicitly so it is not swept into the alias. |
| 6 | `macos_check` wall time grows past the runner budget once the feature is enabled | Med | Low | NFR sets a 25-minute median over 5 runs with a documented `macos-14-xlarge` escape hatch already noted in the workflow. |
| 7 | `llvm-strip` or `llvm-ar` output drifts between LLVM releases, breaking reproducibility on a rebuild | Med | Med | Pin `macos_llvm_version` in the manifest and assert it in the script preflight, exactly as `windows_llvm_version = "19.1.5"` already does. |
| 8 | Shipping macOS opt-in means low real-world exposure, so engine bugs surface only after promotion | Med | Med | Accepted deliberately: it mirrors the Linux precedent (`prd-linux-libghostty-backend` then `prd-linux-libghostty-promotion`). The promotion PRD should require a stated exposure period before flipping `auto`. |
| 9 | `waitid` with `WNOWAIT` behaves differently on Darwin, leaking zombie processes | Low | Med | US-012 asserts the non-blocking, non-consuming semantics explicitly and runs the assertions in `macos_check`. |

## Non-Goals

Explicit boundaries. This version does NOT include:

- **Promoting Ghostty to `auto` on macOS.** Deferred to a follow-on PRD mirroring `prd-linux-libghostty-promotion`, after a stated exposure period on the opt-in path.
- **An `x86_64-apple-darwin` archive.** That release target has been CLOSED since 2026-04-21 (`release.yml` lines 176 to 199); no Intel binary ships, so an Intel archive would be dead weight. The per-arch decision makes adding it later a purely additive manifest entry if the leg reopens. This narrows the original per-arch decision, which had assumed both arches.
- **A universal or `lipo` archive.** It would break the one-Rust-target-to-one-archive model and force a Darwin build host.
- **Consuming upstream's prebuilt `ghostty-vt.xcframework.zip`.** It is universal with dead iOS slices, `libtool`-produced and therefore not reproducible by us, and depends on `tip` release and R2 availability. Retained as an emergency fallback only.
- **Renderer or surface integration.** Only `libghostty-vt` is in scope. Paneflow keeps its own GPUI `TerminalElement`; the cmux Swift `GhosttyKit.xcframework` model is explicitly not transposed.
- **Bumping the GPUI or Ghostty pins.** Both stay where they are. A GPUI bump is a separate change with its own gate set per `CLAUDE.md`.
- **Removing `alacritty_terminal`.** It remains the cross-platform rollback on all three platforms.
- **iOS, iPadOS, or Mac Catalyst targets.**
- **Changing the `.dmg` signing, notarization, or stapling pipeline.**

## Files NOT to Modify

- `native/libghostty/prebuilt/x86_64-unknown-linux-gnu/**` - shipped Linux x64 artifact, frozen
- `native/libghostty/prebuilt/aarch64-unknown-linux-gnu/**` - shipped Linux arm64 artifact, frozen
- `native/libghostty/prebuilt/x86_64-pc-windows-msvc/**` - shipped Windows artifact, frozen
- `scripts/build-libghostty-windows.ps1` - Windows recipe; its normalization chain is load-bearing and unrelated to macOS
- `scripts/build-libghostty-linux.sh` - read as the template for US-005, never edited by it
- `scripts/verify-libghostty-package.sh` - ELF-only Linux verification; macOS gets its own script
- `native/libghostty/patches/windows-formatter-emit-specialization.patch` - Windows-only source patch with pinned input and output hashes
- `src-app/src/terminal/element/**` - GPUI rendering layer; the backend swap must not reach the paint pass
- The `gpui` and `gpui_platform` declarations in `src-app/Cargo.toml` - pinned to `fecc3273ed32643c2ea1b04a74c8780e2c9ffaf8`; a bump is out of scope
- `.github/workflows/libghostty-windows.yml` - Windows workflow, frozen
- `.github/workflows/update_cask.yml` and `.github/workflows/repo_publish.yml` - distribution workflows unaffected by the engine choice

## Technical Considerations

Framed as questions for engineering input.

- **Build host:** cross-compile from `ubuntu-22.04` with pinned Zig 0.15.2, or build on the existing `macos-14` runner? Recommended: cross-compile, because `CombineArchivesStep` takes the `zig ar -M` path we already normalize on a non-Darwin host, and the libghostty Linux runner already exists. US-004 must confirm the unconditional `libghostty_vt_shared` link step survives. If it does not, `macos-14` is the fallback and `archive_normalization` becomes `apple-libtool-combine+llvm-strip-debug+llvm-ar-D`.
- **Cfg alias mechanism:** a hand-written build script emitting `cargo::rustc-cfg=ghostty_native`, or the `cfg_aliases` crate? Recommended: hand-written, roughly 15 lines per crate, no new dependency, keeps `cargo deny check sources` and `dependency_source_policy.rs` untouched. Note that `paneflow-terminal-ghostty` has no build script today and gains one.
- **POSIX gating shape:** widen `target_os = "linux"` to `cfg(unix)`, or add a parallel `target_os = "macos"` arm? Recommended: widen, because `ghostty_session.rs` contains zero `/proc` and zero `prctl` references, so its Linux label is already a misnomer. Trade-off: `cfg(unix)` also matches BSD targets that have no CI coverage; edge case 12 defines the expected behavior there.
- **Archive format flag:** rely on `llvm-ar` format inference, or pass `--format=darwin` explicitly? Recommended: explicit, because inference falls back to the host toolchain default when it fails, and the host here is Linux.
- **Normalization recipe naming:** `archive_normalization` currently carries the full tool chain in application order for Windows. Recommended: keep that convention so the macOS value is self-documenting and a recipe change is visible in the manifest diff.
- **Migration and rollback:** no data migration. Rollback is `terminal.backend = "alacritty"` for users and `--no-default-features` for builds; both paths are gated on every pull request by US-013.

## Success Metrics

| Metric | Baseline (current) | Target | Timeframe | How Measured |
|--------|-------------------|--------|-----------|-------------|
| Release targets on libghostty | 3 of 4 (Linux x64, Linux arm64, Windows x64) | 4 of 4 available, 3 of 4 default | Month-1 | `manifest.toml` target count and `auto_selects_ghostty_for_target()` |
| Native modules type-checked on Darwin | 0 of 14 | 14 of 14 | Month-1 | `macos_check` clippy step with the feature enabled |
| Platform predicate spellings in source | 174 | 0 outside build scripts | Month-1 | `rg -c 'feature = "libghostty-linux"' src-app/src crates` |
| macOS archive reproducibility | N/A (no archive) | 2 of 2 clean builds byte-identical | Month-1 | `scripts/build-libghostty-macos.sh --verify-reproducible` in `libghostty-macos.yml` |
| macOS terminal defects requiring a platform qualifier to reproduce | Current issue backlog count at PRD start | Reduced, with the residue attributable to the renderer rather than the VT engine | Month-6 | GitHub issue triage labels |
| New git sources in the dependency tree | 0 | 0 | Month-1 | `cargo deny check sources` and the `allow-git` list in `deny.toml` |
| macOS `auto` promotion | Not started | Promotion PRD opened with a stated exposure period | Month-6 | Follow-on PRD exists in `tasks/` |

## Open Questions

- Does the Zig 0.15.2 Linux-host cross-build survive the unconditional `libghostty_vt_shared` link step? Answered by US-004 before US-005 begins; determines the build host, the workflow runner, and the `archive_normalization` value.
- Does `llvm-strip -S` remove `LC_UUID` from Mach-O archive members, or is an explicit `-no_uuid` or post-processing step required? Answered by US-004's Mach-O header inspection; determines whether US-006 needs an additional normalization stage.
- Should `macos_render_smoke` be extended to launch with the Ghostty backend once US-016 lands, given it is the only gate that proves first-frame rendering? Maintainer decision during EP-005; would strengthen coverage at the cost of a second macOS job run.
- What exposure period on the opt-in path should gate the `auto` promotion? Maintainer decision, needed before the follow-on promotion PRD is written, not before this PRD is implemented.
- If `x86_64-apple-darwin` reopens as a release target, is the Intel archive added in the promotion PRD or in a dedicated one? Maintainer decision; no work in this PRD blocks either choice.
[/PRD]
