# libghostty-vt third-party notices

PaneFlow's default Linux Ghostty backend statically links the pinned
`libghostty-vt` archive. The source, compiler, header, bindings, and build mode
are fixed in `manifest.toml`. Cargo never downloads or builds this native
source implicitly.

| Component | Pinned version | License | Distribution role |
|---|---|---|---|
| Ghostty / libghostty-vt | `ae52f97dcac558735cfa916ea3965f247e5c6e9e` | MIT | Statically linked terminal engine |
| simdutf | `5.2.8` | Apache-2.0 OR MIT | Vendored by the pinned Ghostty source when SIMD is enabled |
| Highway | `1.2.0@66486a1` | Apache-2.0 AND BSD-3-Clause | Vendored by the pinned Ghostty source |
| Zig | `0.15.2` | MIT | Build tool only, not shipped in PaneFlow packages |

The CI native-inventory artifact also contains the pinned `build.zig.zon`, its
SHA-256 digest, `manifest.toml`, and the generated native build information.
That artifact is the authoritative inventory of Zig packages resolved for a
given libghostty build. A source SHA change invalidates the artifact and reruns
the ABI, corpus, fuzz, benchmark, package, and license checks.

Copyright notices:

- Ghostty: Copyright (c) 2024 Mitchell Hashimoto and Ghostty contributors.
- Highway: Copyright Google LLC and Arm Limited.

The complete license texts remain in the pinned upstream source checkout used
by CI. This notice is included in tar, AppImage, deb, and rpm artifacts.
