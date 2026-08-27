# macOS libghostty engine

Status: the `aarch64-apple-darwin` archive is produced, reproducible, committed
under `native/libghostty/prebuilt/aarch64-apple-darwin/`, and declared in
`native/libghostty/manifest.toml` as a `platform = "macos"` target. Ghostty is
the only terminal engine, so an `aarch64-apple-darwin` build links it
unconditionally. `x86_64-apple-darwin` has no declared archive and is therefore
not a shipping target. This document records the cross-build spike so the
decision is auditable without re-running it, and documents the build recipe.

## Cross-build spike

Run on 2026-08-26 from a Linux host, Fedora, `x86_64-unknown-linux-gnu`, against
Ghostty `ae52f97dcac558735cfa916ea3965f247e5c6e9e` with Zig `0.15.2`.

The spike question was whether the pinned `lib-vt` emit mode cross-builds for
`aarch64-macos` from a Linux host at all. Ghostty's `CombineArchivesStep` takes
the Apple `libtool` path only when the build host is Darwin, so a Linux host
falls through to the same `zig ar -M` combine the Linux targets already use.
The unconditional `libghostty_vt_shared` link step was the other risk: a Mach-O
dynamic library link from a Linux host could have failed and aborted the whole
build graph.

### Verdict

| Question | Result |
|---|---|
| `zig build -Demit-lib-vt=true -Dtarget=aarch64-macos -Doptimize=ReleaseFast` from Linux | PASS, exit 0 |
| Does the `libghostty_vt_shared` link step succeed from Linux | Yes. `lib/libghostty-vt.0.1.0.dylib` plus its two version symlinks are produced |
| Does `lib/libghostty-vt.a` exist | Yes, 11 members, 7,842,128 bytes before normalization |
| Does `file` report Mach-O arm64 | Yes on every extracted member: `Mach-O 64-bit arm64 object, flags:<\|SUBSECTIONS_VIA_SYMBOLS>`. `file` on the archive container reports only `current ar archive`, so the check has to run on members |
| Does `llvm-nm -g --defined-only` list the build-info symbol | Yes, as `_ghostty_build_info` at `0000000000057b54 T`. Mach-O prefixes C symbols with an underscore |
| Do members carry `LC_UUID` or `N_OSO` | No. Zero of either in all 11 members. Both are link-product artifacts, not archive-member artifacts. Each member instead carries 5 or 6 `__debug*` sections |
| Does `llvm-strip -S` remove them | It removes every `__debug*` section, which is the actual reproducibility and size hazard here. `_ghostty_build_info` survives |
| Two clean builds byte-identical | See below |

Archive members: `base64.o`, `codepoint_width.o`, `index_of.o`, `vt.o`,
`libghostty-vt-static_zcu.o`, `compiler_rt.o`, `simdutf.o`, `abort.o`,
`per_target.o`, `targets.o`, `libhighway_zcu.o`. No duplicate basenames.

### Reproducibility

Two independent defects had to be fixed before two clean builds matched byte for
byte.

**Defect 1: unpinned Zig build order.** Two clean builds with Zig's default seed
and default job count are not reproducible, before or after normalization:

```
norm run 1  564221b79d2597941b178d07c240d7085b87b9bc84038531af6668ec9c8fa141
norm run 2  e6bbed2ba8c66ae4d6a73546ddd94685a8d22a7b5cd7de50162f1ea86bfe019e
first differing byte offset  251601
```

Exactly one member diverged: `libghostty-vt-static_zcu.o`, same 1,368,120-byte
size, 180,316 differing bytes. The defined-symbol set was identical, but
`__text` differed in size by 8 bytes (`000b376c` against `000b3764`) and the
`__thread_bss` and `__bss` addresses shifted by the same 8. That is
nondeterministic codegen ordering inside Zig, not a UUID, a timestamp, or an
archive-header artifact, so neither a normalization pass nor a macos-14 runner
would have fixed it. `--seed 0 -j1` pins it, mirroring the `zig-build-seed0-j1`
token already in the Windows recipe. Two further clean builds under that pin,
each from an emptied Zig cache, normalized to the same bytes.

**Defect 2: path length leaking through the strip.** The seed pin alone is not
enough. Two `--seed 0 -j1` builds whose Zig cache directories sit at paths of
different lengths still diverge, in `abort.o`, `targets.o`, and
`libghostty-vt-static_zcu.o`. Every section size is identical and no string
differs; only an address moves:

```
build A   __bss  00000010  0000000000000cd8
build B   __bss  00000010  0000000000000cc8
```

The unstripped object explains it. `abort.o` before normalization carries nine
`__debug*` and `__apple_*` sections, the last of which is
`__debug_line` at VMA `0xbb8` with size `0x10f`, ending at `0xcc7`, immediately
before `__bss` at `0xcc8`. Those debug sections embed absolute paths:

```
/home/arthur/dev/paneflow/target/libghostty-spike
/home/arthur/dev/paneflow/target/libghostty-spike/ghostty-source
/home/arthur/dev/paneflow/target/libghostty-spike/ghostty-source/pkg/highway/src/cpp/abort.cc
```

`llvm-strip -S` removes those sections but does not renumber the addresses that
followed them, so the surviving `__bss` VMA still encodes their pre-strip size,
and therefore the length of every absolute path the debug sections carried.

The first half of the fix is the one the Windows recipe already carries as
`fixed-source-cache-prefix`: export the pinned tree with `git archive` into one
canonical absolute path, build there with the Zig cache and the install prefix
nested under it, then remove it. `macos_canonical_source_path` pins that path,
and `.paneflow-zig-cache` and `.paneflow-zig-output` sit inside it.

**Defect 3: the Zig installation path is an input too.** Pinning the source,
cache, and prefix is not sufficient. `compiler_rt.o`,
`libghostty-vt-static_zcu.o`, and `targets.o` are compiled from Zig's own
`lib/` tree, so their debug sections carry the toolchain's absolute paths, and
the same post-strip address hole shifts with the length of wherever Zig happens
to be installed. Three builds of the same pinned source, differing only in the
Zig root, produce three archives:

```
26  chars  /tmp/pf-zigprobe-aaaa...                     f619814e6b1900ab...
52  chars  /tmp/pf-zigprobe-bbbb...                     63f8ccf7df036afd...
124 chars  <a long scratch path>                        b6d020ae0c4258e6...
```

Each of the three is internally reproducible: `--verify-reproducible` passes on
every one of them, because both of its passes run on the same host. Only a
second host reveals the drift, which is exactly the failure mode a committed
`archive_sha256` must not have.

The fix extends the canonical-prefix idea to the toolchain. `zig env` resolves
`.zig_exe` and `.lib_dir`, the script stages both at
`macos_canonical_zig_path`, and the build runs
`<canonical-zig>/zig build --zig-lib-dir <canonical-zig>/lib`, mirroring the
`--zig-lib-dir` the Windows recipe already passes. The staged copy is removed
on success and on failure alike. The recipe token becomes
`fixed-zig-source-cache-prefix`, and no host layout can shift a single address.

**Manifest hash gate.** The Windows script compares the produced archive hash
against the manifest before publishing (`build-libghostty-windows.ps1:854`).
The macOS script now does the same: a hash that differs from
`archive_sha256` aborts the build. `--allow-hash-drift` downgrades that to a
warning and exists only to mint a new reviewed archive when the recipe changes
deliberately. Without this gate a divergent artifact is emitted silently, which
is how Defect 3 survived its own reproducibility check.

The raw, un-normalized archives still differ because `zig ar` stamps member
timestamps, so normalization stays mandatory on top of the canonical paths.

With both fixes in place, `scripts/build-libghostty-macos.sh
--verify-reproducible` passes: two clean builds, each from a freshly created
canonical root, produce the same archive, header, `bindings.rs`, and
`build-info.txt`.

```
archive_sha256  d81dafad9975987fc582977f24af06c9255b901196c07b00be42b85ffe8dba03
```

That value is pinned as `archive_sha256` on the
`[targets."aarch64-apple-darwin"]` entry, and it is the same value the committed
`build-info.txt` records.

Normalization drops the archive from 7,842,128 to **1,803,544 bytes**, well
under the 3.0 MB prebuilt-tree cap. `ar tv` on the normalized archive reports
`0/0` uid and gid and `Jan 1 1970` timestamps for every member.

### macos-14 runner fallback

Not taken. The fallback existed for the case where the Linux-host cross-build
failed or produced a non-normalizable archive; it would have used
`archive_normalization = "apple-libtool-combine+llvm-strip-debug+llvm-ar-D"`.
The Linux-host build passes, and both reproducibility defects were
host-independent: Zig codegen ordering and debug-path length, neither of which a
Darwin runner would have fixed. Building on Linux also keeps the macOS
archive on the same runner class and the same `zig ar -M` combine as the Linux
targets.

### Zig objcopy is not a substitute for llvm-strip

`zig objcopy` refuses Mach-O input: `error: invalid elf file: InvalidElfMagic`.
A real `llvm-strip` is mandatory. `zig ar` could repack, but the strip step
cannot be served by Zig's bundled tooling.

## Toolchain pin

`macos_llvm_version` is pinned to `22.1.2-rust-1.96.1-stable`, the LLVM shipped
by the `llvm-tools` component of the repository's own pinned Rust toolchain:

```bash
rustup component add llvm-tools
```

The tools then live under
`$(rustc --print sysroot)/lib/rustlib/<host>/bin`, which
`scripts/build-libghostty-macos.sh` resolves automatically.
`PANEFLOW_LLVM_BIN` overrides that search for a differently pinned set.

The rationale is that this LLVM is exactly pinned by an existing project pin
and costs a roughly 30 MB rustup component in CI, against a 1.65 GB standalone
LLVM tarball. The tradeoff is that a Rust toolchain bump moves
`macos_llvm_version` and forces one archive re-normalization plus a new
`archive_sha256`, the same way a Zig bump does.

## Pinned inputs and required tools

Every value below lives in `native/libghostty/manifest.toml` and is restated
here so a rebuild needs no reading of `scripts/build-libghostty-macos.sh`. The
manifest stays authoritative: if the two disagree, the manifest wins and this
table is stale.

| Input | Manifest key | Value |
|---|---|---|
| Ghostty source revision | `source_sha` | `f2d5758f6305867dc36b36293c6165d8152b853e` |
| Zig | `zig_version` | `0.16.0` |
| LLVM binutils | `macos_llvm_version` | `22.1.2-rust-1.96.1-stable` |
| Rust target | `[targets."aarch64-apple-darwin"]` | `aarch64-apple-darwin` |
| Zig target | `zig_target` | `aarch64-macos` |
| Optimize mode | build flag | `ReleaseFast` |
| Build seed | `macos_build_seed` | `0` |
| Build jobs | `macos_build_jobs` | `1` |
| Canonical source path | `macos_canonical_source_path` | `/tmp/paneflow-libghostty-f2d5758f` |
| Canonical Zig path | `macos_canonical_zig_path` | `/tmp/paneflow-libghostty-zig-0.16.0` |
| Archive path in the bundle | `archive_path` | `lib/libghostty-vt.a` |
| Archive digest | `archive_sha256` | `d81dafad9975987fc582977f24af06c9255b901196c07b00be42b85ffe8dba03` |
| Header digest | `header_sha256` | `df3997ea5f3df0902df8a80a3db176bda7a0e6c4d389be21b90da8c9fb52be44` |
| Bindings digest | `bindings_sha256` | `95ecb9889cd5408f88a117f8bbca2cde89b911c3fc06849e99307dd9c172f95f` |
| Link name | `link_name` | `ghostty-vt` |
| System libraries | `system_libraries` | none |
| Build-info symbol | `build_info_symbol` | `ghostty_build_info` |

Required on the build host, all checked by preflight:

| Tool | Required version | Source |
|---|---|---|
| `zig` | exactly `0.16.0` | ziglang.org release tarball, or any pinned install on `PATH` |
| `llvm-strip`, `llvm-ar`, `llvm-nm`, `llvm-objdump` | exactly `22.1.2-rust-1.96.1-stable` | `rustup component add llvm-tools`, or `PANEFLOW_LLVM_BIN` |
| `git` | any | the Ghostty checkout must be a clean tree at `source_sha` |
| `sha256sum`, `file`, `tar` | any | coreutils, file, tar |

No Xcode, no Apple `libtool`, and no macOS host are required: the archive
cross-builds from Linux, which is the whole point of the spike above.

## Building the archive

```bash
rustup component add llvm-tools
export PANEFLOW_GHOSTTY_SOURCE_DIR=/path/to/ghostty   # clean checkout at source_sha
scripts/build-libghostty-macos.sh --verify-reproducible
```

Every pinned input is read from `native/libghostty/manifest.toml`. The script
hardcodes no SHA, version, or target. Preflight exits non-zero, naming the tool
and the expected version, when Zig is not exactly `zig_version`, when
`llvm-strip`, `llvm-ar`, `llvm-nm`, or `llvm-objdump` is absent or not
`macos_llvm_version`, when `sha256sum`, `file`, or `tar` is missing, when the
Ghostty checkout is not a clean tree at `source_sha`, or when the header or
bindings checksums do not match.

The build never runs in place. `PANEFLOW_GHOSTTY_SOURCE_DIR` is only read and
verified; the tree that actually compiles is a `git archive` export at
`macos_canonical_source_path`, removed again on both success and failure. The
Zig toolchain is staged the same way at `macos_canonical_zig_path`, so the
installed location on the build host never reaches the artifact.

The produced archive hash is compared against the target's `archive_sha256`
before the bundle is written. A mismatch aborts; `--allow-hash-drift` turns it
into a warning and is reserved for deliberately re-pinning the recipe.

The recipe is:

```
archive_normalization = "fixed-zig-source-cache-prefix+zig-build-seed0-j1+llvm-strip-debug+llvm-ar-D-darwin"
```

read in application order:

1. `fixed-zig-source-cache-prefix`: the pinned tree is exported with
   `git archive` to `macos_canonical_source_path`, the resolved Zig executable
   and `lib/` tree are staged at `macos_canonical_zig_path`, and the build runs
   there with `ZIG_GLOBAL_CACHE_DIR` and `ZIG_LOCAL_CACHE_DIR` under
   `<canonical>/.paneflow-zig-cache`, `--prefix
   <canonical>/.paneflow-zig-output`, and `--zig-lib-dir
   <canonical-zig>/lib`. Every path that reaches debug info is therefore a
   constant, including the toolchain's own.
2. `zig build -Demit-lib-vt=true -Dtarget=aarch64-macos -Doptimize=ReleaseFast --seed 0 -j1`,
   from `macos_build_seed` and `macos_build_jobs`.
3. `llvm-strip -S` on each member, `-S` being `--strip-debug`. Each member is
   first confirmed to be a `Mach-O 64-bit arm64 object` with `file`, so a
   wrong-target build cannot reach the repack.
4. `llvm-ar crsD --format=darwin` to repack, in `LC_ALL=C` basename order.
   `D` is deterministic mode: zeroed timestamps, uid, and gid. `--format` is
   explicit because `llvm-ar` otherwise infers the archive flavor and can fall
   back to the host default when cross-hosting.

Duplicate member basenames are rejected rather than silently collapsed, since
the repack is basename-addressed.

No `SOURCE_DATE_EPOCH` equivalent is needed. `llvm-ar` deterministic mode zeroes
every timestamp the archive carries, and the members themselves embed none.
There is therefore no `macos_source_date_epoch` manifest key, unlike Windows,
where the COFF object header carries its own timestamp.

`--verify-reproducible` copies the first result aside, runs a second build from
a freshly created canonical root and therefore a freshly emptied Zig cache, and
compares the normalized archive, the installed header, `bindings.rs`, and
`build-info.txt`. The second pass reuses the same output directory on purpose:
the build itself must always run from the canonical prefix. On a mismatch it
exits non-zero, prints the first differing byte offset, and dumps
`llvm-objdump --macho --all-headers` for both copies of every member that
differs.

## Re-pinning the recipe

A Ghostty bump, a Zig bump, or a Rust toolchain bump that moves
`llvm-tools` all invalidate `archive_sha256`. The procedure is the same in all
three cases:

1. Update the moved key in `native/libghostty/manifest.toml`: `source_sha`,
   `zig_version`, or `macos_llvm_version`. A `source_sha` bump also moves
   `macos_canonical_source_path`, and a `zig_version` bump moves
   `macos_canonical_zig_path`, because both embed the pin.
2. Rebuild with `--allow-hash-drift` so the expected-hash check warns instead
   of aborting:

   ```bash
   scripts/build-libghostty-macos.sh --allow-hash-drift --verify-reproducible
   ```

3. Copy the new `archive_sha256` from the emitted `build-info.txt` into the
   `[targets."aarch64-apple-darwin"]` block, along with `header_sha256` and
   `bindings_sha256` if the header or the bindings moved. Those two are global
   keys shared with the Linux trees, so a change there means every tree is
   re-pinned in the same commit.
4. Copy the four reviewed files into
   `native/libghostty/prebuilt/aarch64-apple-darwin/` and rerun
   `scripts/verify-libghostty-macos.sh` plus
   `cargo test -p paneflow-libghostty-sys`.
5. Rerun the build once more without `--allow-hash-drift`. It must now pass the
   expected-hash check against the manifest value you just wrote.

`THIRD_PARTY_NOTICES.md` carries the archive fingerprint, so a re-pin also
moves `notice_sha256`.

## Observing the engine on macOS

There is nothing to select: every Apple Silicon build links the pinned archive
and uses it. `RUST_LOG=info` prints one `Terminal backend selected:` line per
pane naming the failure phase, the target triple, and the pinned Ghostty build
identity, so a bug report can state exactly which archive produced the
behavior. A startup failure is reported in the pane with its phase and OS
error; no second engine takes over.

## Emitted build-info.txt

Ten keys, the same set the Linux and Windows scripts emit:

```
source_sha, zig_version, header_sha256, bindings_sha256, rust_target,
zig_target, optimize, archive_normalization, archive_sha256, build_info_symbol
```

with `rust_target = aarch64-apple-darwin` and `zig_target = aarch64-macos`.
`build_info_symbol` stays the C name `ghostty_build_info`; the script's symbol
check accepts the Mach-O `_` prefix.

## The committed tree

`native/libghostty/prebuilt/aarch64-apple-darwin/` carries the four files the
macOS bundle needs, the same set the Linux trees carry:

```
bindings.rs
build-info.txt
include/ghostty/vt.h
lib/libghostty-vt.a
```

The script's output directory is the full Zig install prefix, so it also holds
the `libghostty-vt` dylibs, `share/`, and the rest of the installed headers.
Copy exactly those four paths into the committed tree; the extra install output
is not reviewed and would break both the four-file inventory and the 3.0 MB
per-tree cap.

It is 1.85 MiB, under the 3.0 MB per-tree cap, and brings `native/libghostty/`
to 9.49 MiB, under the 11.0 MB total cap. `bindings.rs` and
`include/ghostty/vt.h` are byte-identical to the Linux trees' copies, which is
why the manifest keeps `header_sha256` and `bindings_sha256` as global keys
rather than per-target ones.

`validates_the_reviewed_macos_bundle_as_one_unit` in
`crates/paneflow-libghostty-sys/src/build_support/artifact.rs` validates the
committed tree against the manifest on every host, not only on Darwin, because
`ArtifactBundle::validate` is pure path and checksum work. A corrupted archive
or a manifest digest that drifts from the committed bytes fails
`cargo test -p paneflow-libghostty-sys` on Linux.

`THIRD_PARTY_NOTICES.md` gained the macOS archive fingerprint and now names
macOS arm64 alongside Linux and Windows, so `notice_sha256` moved with it.
`sbom.cdx.json` is unchanged: it is scoped to the Windows integration
(`bom-ref: paneflow-libghostty-windows`), and the macOS archive is built from
the same pinned Ghostty source, the same Zig version, and the same bundled
third-party components, so it introduces no component the SBOM does not already
list. A macOS SBOM is a separate artifact if one is ever required.
