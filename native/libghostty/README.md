# libghostty-vt native input

`manifest.toml` is the single source of truth for Paneflow's native
libghostty-vt inputs. Cargo never fetches Ghostty, runs bindgen, invokes Zig,
or mutates an artifact. Reviewed archives live under `prebuilt/<rust-target>/`,
so a standard checkout only verifies and links repository content.

The pinned source is Ghostty
`ae52f97dcac558735cfa916ea3965f247e5c6e9e` built with Zig 0.15.2 in
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

## Windows x64 MSVC archive

The production Windows input is the SIMD-enabled static archive
`prebuilt/x86_64-pc-windows-msvc/lib/ghostty-vt-static.lib`. Rebuild it from a
clean pinned checkout in an x64 Visual Studio environment:

```powershell
.\scripts\build-libghostty-windows.ps1 `
  -SourceDir C:\path\to\ghostty `
  -Zig C:\path\to\zig-0.15.2\zig.exe `
  -VerifyReproducible
```

The reviewed recipe requires:

| Input | Pinned value |
|---|---|
| Rust target | `x86_64-pc-windows-msvc` |
| Zig target | `x86_64-windows-msvc` |
| Zig | `0.15.2` |
| Zig distribution | official x64 Windows ZIP, SHA-256 `3a0ed1e8799a2f8ce2a6e6290a9ff22e6906f8227865911fb7ddedc3cc14cb0c` |
| Zig executable SHA-256 | `d408dd38eed3e5204af841bcebf70502a4dbbb8399a3a3262be55059370bc018` |
| Zig codegen image base | `0x0000000140000000` |
| Zig PE DLL characteristics | `0x8160` |
| Windows formatter patch | SHA-256 `860bb22e782960e368d19f94596b29d0370edbdd0f94da28f8e09a0b25268797` |
| MSVC toolset | `14.38.33130` |
| Windows SDK | `10.0.26100.0` |
| Visual Studio LLVM tools | `19.1.5` |
| Optimization | `ReleaseFast` |
| SIMD | `true` |
| Build seed and jobs | `--seed 0 -j1` |

The upstream preparation command is:

```text
zig.exe build --zig-lib-dir <official-zig-lib> --verbose --seed 0 -j1 -Demit-lib-vt=true -Dtarget=x86_64-windows-msvc -Doptimize=ReleaseFast -Dsimd=true --prefix <fixed-prefix>
```

The CI downloads the manifest-pinned official Zig ZIP and verifies the archive,
executable, and PE metadata. The script exports the exact clean Ghostty commit,
then applies the repository patch recorded by path, normalized checksum, target,
and before/after target checksums in `manifest.toml`. The patch specializes the
formatter by its three compile-time output formats, keeps large calls out of the
hot loop, and splits header and blank-row formatting into bounded helpers. This
bounds Zig 0.15.2 ReleaseFast code generation that otherwise produced different
register allocations across clean Windows builds. The formatter's behavior and
public ABI are unchanged.

The patched export is built at fixed source, cache, and prefix paths. LLVM strips
debug data from every member of Ghostty's emitted fat archive, zeros each COFF
timestamp, and repacks ordinally sorted members with deterministic `llvm-ar
rcD` mode. It deliberately does not replay the emitted `build-lib` command.
Header and symbol inventories use the same ordinal, case-sensitive ordering,
so hashes do not depend on the Windows locale. Two complete builds start from
empty caches at the same canonical paths and must match byte for byte before
publication. The fixed `C:\Users\Public\paneflow-libghostty-ae52f97d` source
path is part of the hash contract; the build aborts if that path is unavailable
or already occupied.

The fat archive contains Ghostty's Zig objects, Zig `compiler_rt`, simdutf,
and Highway. COFF directives record `RuntimeLibrary=MT_StaticRelease` and
`/DEFAULTLIB:libcpmt.lib`. Consumers link `ntdll.lib` and `kernel32.lib`.
There is no `ghostty-vt.dll`; C consumers of the static header must define
`GHOSTTY_STATIC`.

`windows-smoke.c` is the minimal MSVC reproducer. The build script compiles it
with `/MT /W4 /WX`, initializes a terminal, parses a deterministic VT fixture,
and releases the terminal through Ghostty. It also verifies x64 COFF headers,
the required C symbol inventory, CRT directives, and the executable's DLL
dependencies. A missing SIMD object, unresolved system symbol, wrong
architecture, hash drift, or dynamic Ghostty dependency aborts the build and
preserves the temporary evidence directory.

The prepared Windows directory includes the archive, installed headers,
normalized bindings, `headers.sha256`, `symbols.txt`, and `build-info.txt`.
Each top-level input is pinned by `manifest.toml`; the header index covers all
installed headers. Publication replaces the complete previous target tree by
an atomic same-volume rename, so stale DLLs or metadata cannot survive a
rebuild. `paneflow-libghostty-sys/build.rs` selects this directory only for
`x86_64-pc-windows-msvc`, requires its exact file inventory, rejects incomplete
or incoherent metadata with an actionable rebuild command, and emits only
static plus reviewed system link directives.

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
