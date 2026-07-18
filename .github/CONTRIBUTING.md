# Contributing to Paneflow

Thanks for taking the time to contribute. Paneflow is a cross-platform native
terminal workspace for running AI coding agents in parallel, built in pure Rust
on Zed's GPUI. Issues, fixes, and well-scoped features are all welcome.

## Before you start

- For anything larger than a small fix, **open an issue or a discussion first**
  so we can agree on the approach before you invest time.
- Browse [open issues](https://github.com/arthjean/paneflow/issues) and
  [Discussions](https://github.com/arthjean/paneflow/discussions) to see what
  is already in flight.

## Development setup

Install the toolchain and system dependencies from the
[README](../README.md#prerequisites) (Rust is pinned via `rust-toolchain.toml`;
Linux needs the listed build libraries and a Vulkan driver).

```bash
cargo build                         # debug build
cargo run -p paneflow-app           # run it (needs GPU support: Vulkan on Linux)
RUST_LOG=info cargo run -p paneflow-app  # with logs
```

## Before you open a pull request

Run the same gates CI runs. A single formatting diff fails the release pipeline
on all four matrix legs, so do not skip `cargo fmt`.

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

- **No `panic!`, `unimplemented!`, or `dbg!`** in production code (denied by
  clippy). Prefer `?`, `ok_or(...)?`, `match`, or a documented
  `expect("invariant")` only when provably infallible.
- **Cross-platform by default.** Any change must compile and behave on Linux
  (Wayland + X11), macOS, and Windows. Guard OS-specific code with
  `#[cfg(target_os = "…")]` and provide a working path (or a documented stub)
  for the other two. Prefer cross-platform crates (`portable-pty`, `notify`,
  `dirs`, `which`) over POSIX-only APIs.

## Commit and branch conventions

```
feat(module): short description
fix(module): short description
refactor(module): short description
docs: short description
chore: short description
```

Keep commits atomic. Branch from `main` as `feat/<description>` or
`fix/<description>`.

## License

Paneflow is licensed under [GPL-3.0-or-later](../LICENSE). By submitting a
contribution you agree that it is licensed under the same terms.
