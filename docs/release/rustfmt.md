# Rust toolchain pin

The repo pins the Rust toolchain at the root via `rust-toolchain.toml`. CI and
local dev share the exact same `rustc` / `rustfmt` / `clippy` versions, so a
new Rust point release cannot silently shift formatting or lint output
mid-release (the failure mode that broke CI on v0.2.11).

## Where the pin lives

- `rust-toolchain.toml` (repo root) - `channel`, `components`, `profile`
- `.github/workflows/*.yml` - every explicit `dtolnay/rust-toolchain@master`
  step passes the same `toolchain: "1.98.0"` input. Both must move together.

One step is deliberately exempt: the `llvm-tools` install in
`libghostty-macos.yml`. It is not a workspace toolchain, it is the LLVM that
normalized the committed macOS archive, recorded as `macos_llvm_version` in
`native/libghostty/manifest.toml`. It moves only with a fresh
`archive_sha256`, never with a workspace bump. See
[macos-libghostty.md](macos-libghostty.md).

## Quarterly bump procedure

Run this once per quarter (or on demand if a CVE / required toolchain feature
forces a sooner bump).

1. Read the [Rust release notes](https://blog.rust-lang.org/) for every stable
   version between the current pin and the candidate target. Pay attention to
   `rustfmt` formatting changes and any new `clippy` lints promoted to default.
2. Locally:

   ```bash
   rustup toolchain install <new-version>
   # From the repo root, with `rust-toolchain.toml` temporarily pointing at the new version:
   cargo fmt
   cargo clippy --workspace --all-targets -- -D warnings
   cargo test --workspace
   ```

3. If `cargo fmt` produced any diff, commit it as a single
   `style: apply rustfmt for stable <new-version> / rustfmt <fmt-version>`
   commit *before* the toolchain bump, so the bump itself is mechanical.
4. Update the pin and workflow inputs in the same commit:
   - `rust-toolchain.toml` -> `channel = "<new-version>"`
   - `Cargo.toml` -> `rust-version = "<new-version>"`
   - `.github/workflows/*.yml` -> every explicit `toolchain: "<new-version>"`,
     except the `llvm-tools` step in `libghostty-macos.yml`
   - `README.md` and `AGENTS.md` prose references
5. Open the PR titled `chore(rust): bump pinned toolchain to <new-version>`
   and let CI run. Both fmt and clippy must be green; if not, fix in the
   same PR.

## Why pin to a specific release (not `stable`)

`dtolnay/rust-toolchain@stable` resolves at job runtime. When a new Rust
release ships (every 6 weeks) the next CI run silently picks it up; if
`rustfmt` changes formatting, the very next push fails `cargo fmt --check`
even though no application code changed. That happened on v0.2.11. Pinning
to a specific release moves the bump from "automatic and surprising" to
"explicit and reviewed".

## When to override locally

Don't. The whole point is reproducibility. If you want to test against a
newer toolchain temporarily, pass it explicitly:

```bash
cargo +nightly clippy --workspace
```

Don't run `rustup default nightly` and assume it sticks - the
`rust-toolchain.toml` override always wins inside this repo (verified via
AC6 of US-001: `rustc --version` from the repo dir reports the pinned
version regardless of the user's `rustup default`).
