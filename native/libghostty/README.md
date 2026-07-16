# libghostty-vt native input

`manifest.toml` is the single source of truth for the Linux native backend. The
source repository itself is not fetched by Cargo. Verified static archives for
x86_64 and ARM64 are stored under `prebuilt/`, so a standard Linux `cargo run`
works from a clean clone without Zig or a Ghostty checkout.

To update the bundled inputs, prepare both Linux archives explicitly:

```sh
PANEFLOW_GHOSTTY_SOURCE_DIR=/path/to/ghostty scripts/build-libghostty-linux.sh
```

The source checkout must be clean and exactly at the recorded SHA, and
`zig version` must be exactly 0.15.2. Outputs live under
`target/libghostty/<rust-target>/`, including the stripped, path-normalized
static archive, installed headers and `build-info.txt`. Cargo prefers that
generated directory only when `PANEFLOW_LIBGHOSTTY_DIR` explicitly selects it;
otherwise it uses the bundled archive. `cargo build` only verifies and links
those inputs. The manifest pins the archive hash for each supported target, so
an unreviewed file under `target/` cannot change a standard build.

The manifest also records the complete archive-member license inventory and
pins `THIRD_PARTY_NOTICES.md` by SHA-256. The package verifier rejects a Linux
artifact when its notice differs from that reviewed file, including truncated
or stale copies.

`bindings.rs` is the complete pregenerated bindgen output for the pinned
header. Its checksum is recorded in `manifest.toml`, copied into every
prepared target and verified before Cargo links the archive. The
`paneflow-libghostty-sys` crate is the only raw ABI surface. The
`paneflow-terminal-ghostty` crate owns every native handle, copies all borrowed
data before returning it, releases libghostty allocations with `ghostty_free`,
contains callback panics and checks the runtime API version plus C layout
sizes, alignments and field offsets before constructing a terminal.

Regenerate the bindings only from the pinned header, then update
`bindings_sha256` and rebuild both prepared targets. Cargo never runs bindgen,
downloads Ghostty or mutates the prepared artifacts.

For a reproducibility proof, run the script twice from separate clean Zig
caches and compare the normalized archive, installed header, bindings and
generated build info. Zig 0.15.2 records ephemeral cache paths in debug data
and archive member names, so architecture-neutral `eu-strip --strip-debug`
plus deterministic `ar` repacking runs before hashing. The script requires
`elfutils` and binutils-compatible `ar`; its `--verify-reproducible` mode
performs the comparison.
