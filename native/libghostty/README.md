# libghostty-vt native input

`manifest.toml` is the single source of truth for Paneflow's native
libghostty-vt inputs. Cargo never fetches Ghostty, runs bindgen, invokes Zig,
or mutates an artifact. Reviewed headers, bindings, and build metadata live
under `prebuilt/<rust-target>/`; the archives themselves are not tracked by
git. They are assets of the GitHub pre-release the manifest names
(`archive_release_repository` and `archive_release_tag`, one
`<target>-<archive>` asset per target), and one script places them:

```sh
scripts/fetch-libghostty.sh            # every target; --target <triple> for one
.\scripts\fetch-libghostty.ps1          # Windows twin
```

It verifies each download against the manifest's `archive_sha256` before the
archive reaches `prebuilt/<rust-target>/lib/`, and leaves an archive that is
already in place with the right hash alone. `paneflow-libghostty-sys/build.rs`
fails with a pointer to that script when an archive is missing; it performs no
downloads itself. Every CI job that runs cargo takes the same step through
`.github/actions/fetch-libghostty`.

Provenance of a published archive is a SLSA build-provenance attestation
signed by the bump workflow that built it:

```sh
gh attestation verify prebuilt/<rust-target>/lib/<archive> --repo arthjean/paneflow
```

The pinned source is Ghostty
`f2d5758f6305867dc36b36293c6165d8152b853e` built with Zig 0.16.0 in
`ReleaseFast`. `bindings.rs` is pregenerated from the pinned C header. Its
normalized UTF-8 checksum is verified both in the workspace and in every
prepared artifact. Regenerate bindings only from that exact header, then
update `bindings_sha256` and every reviewed target.

## Linux archives

Prepare both reviewed Linux targets from a clean pinned checkout:

```sh
PANEFLOW_GHOSTTY_SOURCE_DIR=/path/to/ghostty scripts/build-libghostty-linux.sh
```

Outputs are written below `target/libghostty/<rust-target>/`. Cargo uses one
only when `PANEFLOW_LIBGHOSTTY_DIR` explicitly selects it; otherwise it uses
the repository artifact. `--verify-reproducible` builds from two clean Zig
caches and compares the normalized archive, header, bindings, and build info.
Zig cache paths are removed from ELF debug data with `eu-strip`, then members
are repacked with deterministic `ar` mode.

The `zig build` step runs under `taskset` on one CPU. `zig build -j1` only
limits how many build steps the build runner drives at once; the
`zig build-lib` child sizes its own thread pool from the CPU count, and with
two or more threads the machine code it emits for
`terminal.formatter.PageFormatter.formatWithState` in
`libghostty-vt-static_zcu.o` differs by a few bytes between otherwise
identical builds. Every other archive member is stable. On one CPU the whole
archive is byte-identical from build to build, which is what the
`zig-build-seed0-j1-cpu1` recipe token records; see
[docs/release/libghostty-linux.md](../../docs/release/libghostty-linux.md).

## macOS arm64 archive

The reviewed Apple Silicon input is `prebuilt/aarch64-apple-darwin/`. It is
cross-built from a Linux host, because Ghostty only takes the Apple `libtool`
combine path when the build host is Darwin; a Linux host keeps the same
`zig ar -M` combine the Linux targets already normalize.

```sh
rustup component add llvm-tools
PANEFLOW_GHOSTTY_SOURCE_DIR=/path/to/ghostty \
  scripts/build-libghostty-macos.sh --verify-reproducible
```

Normalization tools come from the `llvm-tools` component of the repository's
own pinned Rust toolchain, so `macos_llvm_version` moves only when that
toolchain moves. `PANEFLOW_LLVM_BIN` overrides that search.

Debug sections embed absolute paths, and `llvm-strip -S` drops them without
renumbering the addresses that followed, so path *length* leaks into the
stripped object. The recipe therefore pins every path that reaches debug info:
the pinned tree is exported to `macos_canonical_source_path`, the Zig
executable and `lib/` tree are staged at `macos_canonical_zig_path` and invoked
through `--zig-lib-dir`, and the Zig cache and install prefix are nested under
the canonical source root. Both staged roots are removed on success and on
failure. `--seed 0 -j1` pins Zig's build order, `taskset` pins the compiler to
one CPU (see the Linux section), `llvm-strip -S` strips each member, and
`llvm-ar crsD --format=darwin` repacks them in `LC_ALL=C` basename order.

The produced archive hash is compared against the manifest's `archive_sha256`
before the bundle is written; a mismatch aborts the build.
`--allow-hash-drift` downgrades it to a warning and exists only to mint a new
reviewed archive after a deliberate recipe change. Full rationale and the
reproducibility findings are in
[docs/release/macos-libghostty.md](../../docs/release/macos-libghostty.md).

## Windows x64 MSVC archive

The production Windows input is the SIMD-enabled static archive
`prebuilt/x86_64-pc-windows-msvc/lib/ghostty-vt-static.lib`. Rebuild it from a
clean pinned checkout in an x64 Visual Studio environment:

```powershell
.\scripts\build-libghostty-windows.ps1 `
  -SourceDir C:\path\to\ghostty `
  -Zig C:\path\to\zig-0.16.0\zig.exe `
  -ZigSourceArchive C:\path\to\zig-0.16.0.tar.xz
```

The reviewed recipe requires:

| Input | Pinned value |
|---|---|
| Rust target | `x86_64-pc-windows-msvc` |
| Zig target | `x86_64-windows-msvc` |
| Zig | `0.16.0` |
| Zig distribution | official x64 Windows ZIP, SHA-256 `68659eb5f1e4eb1437a722f1dd889c5a322c9954607f5edcf337bc3684a75a7e` |
| Zig executable SHA-256 | `086ce9d47ba42f33a514e1a6e04eb1d4a8fa1d75e0868e0213caad447c91e864` |
| Zig source library | official source archive, SHA-256 `43186959edc87d5c7a1be7b7d2a25efffd22ce5807c7af99067f86f99641bfdf` |
| Zig codegen image base | `0x0000000140000000` |
| Zig PE DLL characteristics | `0x8160` |
| MSVC toolset | `14.38.33130` |
| Windows SDK | `10.0.26100.0` |
| Visual Studio LLVM tools | `19.1.5` |
| Optimization | `ReleaseFast` |
| SIMD | `true` |
| Build seed and jobs | `--seed 0 -j1` |

The upstream preparation command is:

```text
zig.exe build --zig-lib-dir <official-source-lib> --verbose --seed 0 -j1 -Demit-lib-vt=true -Dtarget=x86_64-windows-msvc -Doptimize=ReleaseFast -Dsimd=true --prefix <fixed-prefix>
```

The CI downloads the manifest-pinned official Zig ZIP and source archive, then
verifies both archives, the executable, and its PE metadata. The source archive
provides the complete Zig library used by `--zig-lib-dir`; the Windows ZIP omits
library test files that Zig 0.16.0 analyzes.

Clean upstream builds are not bit-for-bit reproducible on Windows with
`--seed 0 -j1`: the drift sits in the generated `ghostty-vt-static_zcu.obj`
code for `PageFormatter.formatWithState`. That drift is not Windows-specific:
it comes from the Zig compiler's thread pool, which `zig build -j1` does not
limit, and the Linux and macOS recipes remove it by pinning the compiler to
one CPU (see the Linux section). Paneflow used to force determinism on
Windows with a source patch that split that formatter into out-of-line
helpers, and retired it because the patch had to be rebased against every
upstream change to `src/terminal/formatter.zig`. The Windows recipe cannot
take the same pin: on Windows, the Zig `getCpuCount()` implementation reads
`NumberOfProcessors` from the PEB and ignores the process affinity mask, so
`start /affinity` would leave the compiler thread pool untouched. The Windows
archive is therefore still built once, by `libghostty-bump.yml`, and its
provenance is the build-provenance attestation that run publishes with the
release asset rather than a second matching build.
The recipe still fixes every input it can (seed, jobs, paths, toolchain
versions, normalization) and keeps `build-info.txt` an exact record of how it
was produced.

The export is built at fixed source, cache, and prefix paths. Ghostty asks Zig
to link ntdll and kernel32 into the static library, and a static Zig link
resolves a system library by archiving the SDK's whole import library, so
normalization drops those two members first: they are import libraries rather
than COFF objects, and both the Rust consumer and the MSVC smoke already link
them from `system_libraries`. LLVM then strips debug data from every remaining
member of Ghostty's emitted fat archive, zeros each COFF timestamp, and repacks
ordinally sorted members with deterministic `llvm-ar rcD` mode. It deliberately
does not replay the emitted `build-lib` command.
Header and symbol inventories use the same ordinal, case-sensitive ordering,
so hashes do not depend on the Windows locale. The build starts from empty
caches at the fixed `C:\Users\Public\paneflow-libghostty-f2d5758f` source
path, which `build-info.txt` records; the build aborts if that path is
unavailable or already occupied.

The fat archive contains Ghostty's Zig objects, Zig `compiler_rt`, simdutf,
Highway, and wuffs. Under Zig 0.15.2 its C and C++ members carried a `.drectve`
recording `RuntimeLibrary=MT_StaticRelease` and `/DEFAULTLIB:libcpmt.lib`;
under 0.16.0 no member declares a linker directive at all, so the static CRT
model is proved on the smoke executable instead: linking the archive with
`cl /MT` must yield an import table with no `vcruntime`, `ucrtbase`, `msvcp`,
or `api-ms-win-crt-` entry. Consumers link `ntdll.lib` and `kernel32.lib`.
There is no `ghostty-vt.dll`; C consumers of the static header must define
`GHOSTTY_STATIC`.

`windows-smoke.c` is the minimal MSVC reproducer. The build script compiles it
with `/MT /W4 /WX`, initializes a terminal, parses a deterministic VT fixture,
and releases the terminal through Ghostty. It also verifies x64 COFF headers,
the required C symbol inventory, and the executable's DLL dependencies used to
prove the static CRT model. A missing SIMD object, unresolved system symbol, wrong
architecture, hash drift, or dynamic Ghostty dependency aborts the build and
preserves the temporary evidence directory.

The prepared Windows directory includes the archive, installed headers,
normalized bindings, `headers.sha256`, `symbols.txt`, and `build-info.txt`.
Each top-level input is pinned by `manifest.toml`; the header index covers all
installed headers after deterministic line-ending, trailing-space, and comment
punctuation normalization. Publication replaces the complete previous target
tree by an atomic same-volume rename, so stale DLLs or metadata cannot survive
a rebuild. `paneflow-libghostty-sys/build.rs` selects this directory only for
`x86_64-pc-windows-msvc`, requires its exact file inventory, rejects incomplete
or incoherent metadata with an actionable rebuild command, and emits only
static plus reviewed system link directives.

## Bumping the pinned source

`.github/workflows/libghostty-bump.yml` performs the re-pin. It runs weekly and
on demand (`workflow_dispatch` accepts an explicit `source_sha`; `dry_run`
builds everything without publishing; `recipe_repin` rebuilds and republishes
the archives at the current pin after a build recipe change, under a
`libghostty-vt-<sha>-recipe-<paneflow sha>` tag that cannot collide with the
bump release). It stages the manifest onto the
target commit, regenerates the bindings, rebuilds all four reviewed targets
with `--allow-hash-drift` (Linux and macOS additionally with
`--verify-reproducible`), writes the hashes those builds produced, attests the
four archives with `actions/attest-build-provenance`, publishes them as the
`libghostty-vt-<sha>` pre-release, and opens a pull request carrying the
manifest, bindings, headers, and metadata. Nothing merges on its own. A bump
that is closed unmerged leaves its pre-release behind; delete it by hand.

Provenance is established in the bump run itself, not on the resulting pull
request: a pull request lane only checks a published archive against its
manifest hash, and a bump commit writes that hash, so the pull request alone
would be circular evidence.

One pin the workflow deliberately refuses to move, checked in seconds before
any build starts: **Zig.** A commit whose `minimum_zig_version` moved also
needs a new distribution URL, its checksums, `windows_zig_image_base`, and
`windows_zig_dll_characteristics`. Re-pin Zig by hand first.

`scripts/repin-libghostty-manifest.sh` performs the manifest edits and is the
supported way to re-pin a field by hand. Every edit requires the key to already
exist, so a renamed field fails the bump instead of adding a dead entry.

## ABI and licensing

`paneflow-libghostty-sys` is the only raw ABI surface. The safe wrapper owns
every native handle, copies borrowed callback data before returning, releases
Ghostty allocations through their matching Ghostty destructor, catches Rust
panics inside callbacks, and validates API version, discriminants, callback
signatures, sizes, alignments, and field offsets before constructing a
terminal.

The manifest records the archive-member license inventory and pins
`THIRD_PARTY_NOTICES.md`. Packaging must reject an artifact whose reviewed
notice is absent, truncated, or stale.
