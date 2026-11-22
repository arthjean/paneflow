[PRD]
# PRD: Layered syntax highlighting for the Files dock: Zig, a native lexical layer, and LSP semantic tokens

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.1 | 2026-11-22 | Arthur Jean | Every Windows-only proof moves to a final epic, EP-009, run on Arthur's Windows 11 dual boot: the Windows CI leg and the PowerShell twin of US-005 and US-006, US-033 from EP-007, US-036 from EP-008, the Windows behaviors of US-020 and US-025, and the PowerShell twin of EP-001 on Windows (new US-039 and US-040). EP-007 and EP-008 keep macOS and Linux. 9 epics, 40 stories. |
| 1.0 | 2026-11-22 | Arthur Jean | Initial PRD, from the investigation of missing Zig colors in the Files dock, a code map of t3code's highlighter, a probe of the Zig grammar under tree-sitter 0.26, a native compile of t3code's wasm2c output, a code map of the editor's highlighter, the LSP process model and the C build constraints, and two web research passes (LSP semantic tokens, wasm2c). Implements the two-layer decision of 2026-11-19 (`tasks/prd-files-experience.md`, version 1.4) with one change: layer 1 is Pierre's lexers translated to C by wasm2c instead of a hand rewrite. Adds LSP semantic tokens as layer 3. Reviewed against the PRD rubric by an independent reader in two passes; its findings are applied. 8 epics, 38 stories. |

## Problem Statement

The Files dock editor colors a file only when `grammar_for_ext` knows its extension, and the list ends in `_ => None` (`src-app/src/diff/highlighter.rs:137`). Five gaps follow from that, all verified in the code:

1. **Zig, and every language outside 18 tree-sitter grammars, is plain text.** Arthur reported Zig on 2026-11-22; `.zig` and `.zon` hit the fallback, `CodeHighlighter` gets no pass and sets `enabled = false` (`src-app/src/app/diff_dock/code/highlight.rs:182-209`). The file tree shows a Zig icon (`src-app/src/file_icons/tables.rs:226`), which makes the missing colors look like a bug rather than a missing language.
2. **The first frame is plain even for Rust.** `runs_into` returns nothing for a row that is not `Fresh` (`highlight.rs:273`), and `fill_stale_rows` returns early until a tree exists (`highlight.rs:644`). The first tree of the 300 KB corpus arrives after 18.67 ms median (`open_to_first_tree_300kb`, `bench/baselines/linux-x86_64/editor.json`).
3. **An edited row goes plain until it is queried again**, although its shifted runs were kept (`highlight.rs:318-412`, asserted by the test at `highlight.rs:1537`).
4. **Files between 2 MB (1 MB for Markdown) and the 10 MiB editor cap open with no colors** (`MAX_HIGHLIGHT_BYTES`, `MAX_MARKDOWN_HIGHLIGHT_BYTES` at `highlighter.rs:10-12`; `MAX_FILE_BYTES` = 10 MiB at `src-app/src/app/diff_dock/code/load.rs:10`, `src-app/src/markdown/parser.rs:5`).
5. **Nested code is plain**: a Markdown fence body has no capture (`src-app/src/diff/queries/markdown/highlights.scm:41-44`) and HTML `<script>`/`<style>` bodies have none either (the crate query has no `raw_text` capture).

On top of these, syntax highlighting cannot tell what only the compiler knows: a field from a local, a mutable binding, an unresolved name. Paneflow has no LSP client at all (no `lsp` code under `src-app/src` or `crates/`, no `lsp-types` in `Cargo.lock`).

The people affected are Paneflow users who read and edit code next to their agents in the Files dock: the dock is the product's code surface (`tasks/prd-files-experience.md`). Arthur works daily in Rust (this repository), Zig (the libghostty build) and TypeScript (the site).

**Why now:** Arthur decided on 2026-11-19 to move to progressive two-layer highlighting (`tasks/prd-files-experience.md:879`), with semantic tokens "maybe later"; the Zig report makes the coverage gap concrete. Two facts found on 2026-11-22 make a better layer 1 possible than the hand rewrite the decision assumed: t3code already ships Pierre's lexers translated to C by wasm2c (`/home/arthur/dev/t3code/apps/mobile/scripts/generate-highlights.mts`), and that C compiles natively to about 735 KiB of code (R7), not the 9.67 MiB that ruled out the embedded-WASM option. Pierre's WAT source is also public (R9), so lexer fixes can go upstream.

## Overview

Three layers, each painted over the one below, each usable alone.

**Layer 2 first, because it is the final word for languages with a grammar (EP-001).** Zig joins the tree-sitter registry through the `tree-sitter-zig` 1.1.2 crate and the highlights query of Zed's Zig extension, vendored byte for byte with its provenance. The crate's own query is not usable: it is written for Neovim, and its three `#lua-match?` predicates are not evaluated by the Rust runtime, which colors almost every identifier `constant` (R2). A new test makes that class of defect a build failure: any shipped query with a predicate the runtime ignores fails CI, and so does any vendored query whose bytes no longer match its manifest.

**Layer 1: Pierre's lexers, compiled to native code (EP-002, EP-003).** A new crate, `paneflow-highlights`, vendors the wasm2c translation of `@pierre/highlights` 0.5.2 (152 language lexers), the wasm2c runtime and SIMDe, all regenerated by a script from pinned, hashed inputs. It compiles with `cc` on every shipping target with bounds-checked memory, no `mmap`, no signal handlers and no JIT (R7, R8). A small C shim catches traps so a lexer fault never unwinds through Rust. The editor uses layer 1 to color the first frame before the tree, rows being edited, files above the tree-sitter cap, the languages without a grammar, and nested blocks inside Markdown and HTML. tree-sitter replaces it row by row once its tree is ready, except inside nested-code regions, where layer 1 stays visible (TD-05).

**An oracle keeps layer 1 honest (EP-004).** The correctness comparison built for the prototype (`code/lexer_compare.rs`) becomes a deterministic test with per-language agreement floors against tree-sitter, plus a report of how many bytes change role when layer 2 replaces layer 1, which is the flicker a user sees. A native Rust rewrite of a lexer is scheduled only when that evidence shows a blocking gap for a language where layer 1 is final (TD-04).

**Layer 3: LSP semantic tokens (EP-005, EP-006).** A new crate, `paneflow-lsp`, implements the part of the protocol this needs: `Content-Length` framing over stdio, server lifecycle, incremental document sync with negotiated position encoding, and the semantic token requests (`full`, `full/delta`, `range`, refresh). The feature is off by default, as in Zed (R11), resolves servers only from `PATH` or an absolute path in the user's own `paneflow.json`, never downloads anything, and asks once per project root before starting a server, because a language server can run the project's build scripts (R13). Tokens overlay layers 1 and 2 only where they map to a palette role; everything else stays transparent.

**Real hardware last (EP-007, EP-008, EP-009).** Layers 1 and 2 are checked on Apple Silicon and on Linux Wayland and X11 (EP-007), and the language servers in a separate epic (EP-008) so layers 1 and 2 never wait for EP-006. Everything that only Windows can prove, the MSVC build on the Windows CI leg, the PowerShell twins, the Windows process and path behaviors of the LSP client, and both layers on screen, is gathered in a final epic run on Arthur's Windows 11 dual boot (EP-009). The Windows code itself still lands with each story, as `AGENTS.md` requires; only its proof waits for EP-009.

## Goals

| Goal | Baseline | Target | Measured by |
|------|----------|--------|-------------|
| Languages colored in the editor | 18 tree-sitter grammars plus Markdown inline (`tasks/prd-files-experience.md:865`) | 19 tree-sitter grammars, and every one of the 152 lexer ids reachable from at least one extension or file name | US-002, US-008 |
| First frame of the 300 KB corpus | Plain until the first tree, 18.67 ms median | Colored rows in the first painted frame | US-011, US-016 |
| A row being edited | Plain until requeried (`highlight.rs:1537`) | Colored at the next paint | US-012 |
| Files between the tree-sitter cap and 10 MiB | Plain | Colored by layer 1 | US-015 |
| Binary size added by layer 1 | 9.67 MiB for the wasmtime option (`tasks/prd-files-experience.md:843`) | Measured, recorded, and below 9.67 MiB | US-006 |

## Non-Goals

- **tree-sitter injections.** The 2026-11-19 decision named them; layer 1 already lexes nested code, so this PRD keeps the nested regions on layer 1 (TD-05). Injections can come later without changing the layer contract.
- **A hand-written Rust rewrite of any lexer in this PRD.** The trigger is defined (TD-04); no rewrite is scheduled until the oracle meets it.
- **LSP features other than semantic tokens**: diagnostics, completion, hover, go to definition, formatting, code actions, inlay hints. The client answers the server requests the protocol requires and ignores the rest (US-021).
- **Downloading or installing language servers.** Servers come from `PATH` or an absolute path the user wrote in `paneflow.json`.
- **Project-local configuration of servers** (a file inside the repository). Per-directory configuration was cut on 2026-05-01 (project memory); a repository must not be able to choose what Paneflow executes.
- **Multi-root workspaces.** One server instance serves one root, the Files dock root (TD-07).
- **A "semantic tokens replace tree-sitter" mode** (Zed's `full`, R11). Layer 3 only overlays.
- **New palette roles.** Every layer resolves through the existing 29 roles (`crate::theme::SyntaxPalette`, `src-app/src/theme/model.rs:41-71`); for example `type.builtin` keeps the `type` color.
- **Font styles** (bold, italic, underline) for any layer. The palette has colors only, and the vendored queries exclude font styles (`src-app/src/diff/queries/NOTICE`).
- **Semantic tokens in the diff view.** A diff shows text that is not an open document.
- **A user setting for layer 1.** It is the baseline every file gets.
- **The read-only large-text preview above 10 MiB** (`src-app/src/app/diff_dock/preview/large_text.rs`): it stays plain.
- **Windows ARM64.** Not a shipping target (`AGENTS.md`, libghostty targets).

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, test modules included
- `cargo nextest run --workspace --locked --profile ci` - the full suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget in a unit test
- `bash scripts/check-helper-sizes.sh` - the shim, hook and MCP helpers stay under their caps (run after nextest on Linux x86_64)
- `cargo deny check advisories licenses sources` - when a dependency changes

## Research Findings

| # | Finding | Source | Shapes |
|---|---------|--------|--------|
| R1 | `tree-sitter-zig` 1.1.2 is the latest crate, MIT, from tree-sitter-grammars, ABI 14; tree-sitter 0.26.13 accepts ABI 13 to 15. It exports `LANGUAGE`, `HIGHLIGHTS_QUERY`, `INJECTIONS_QUERY`. Zed's Zig extension pins grammar commit `6479aa13`, 8 commits after the `v1.1.2` tag (`b670c8df`), which moves the parser to ABI 15 and reuses the argument rule for call arguments. | `cargo search tree-sitter-zig`; `~/.cargo/registry/.../tree-sitter-zig-1.1.2/src/parser.c:7`, `bindings/rust/lib.rs`; `tree-sitter-0.26.13/binding_rust/bindings.rs:3-4`; GitHub compare `b670c8df...6479aa13` | US-001, TD-01 |
| R2 | The crate's highlights query uses `#lua-match?` three times. The Rust runtime evaluates only `eq?`, `match?`, `any-of?` and their variants; others become general predicates that never filter. A probe on a Zig sample colored `self`, `list`, `msg`, `i` and `Point` as `constant`. Zed's extension query (`#match?` instead) gave `Point` type, `len` function.definition, `x` property, `self` variable.parameter, `MAX_LEN` constant. | Probe in the session scratchpad (`zigprobe`); `tree-sitter-0.26.13/binding_rust/lib.rs:2565-2676` | US-001, US-003 |
| R3 | Zed's Zig query lives at `languages/zig/highlights.scm` in `zed-extensions/zig` (Apache-2.0); main is `996432e66cb5bb2eed5a30ad2afaaa90349a3817`; the file last changed in `64c847fe`; SHA-256 `8f7f115b4a48a3336609ee8a087e634290131816d18d00fc11ef840be880df8e`. Zed maps both `zig` and `zon` to it. | GitHub API on 2026-11-22; `~/.local/share/zed/extensions/installed/zig/languages/zig/config.toml` | US-001 |
| R4 | A `build.zig.zon` sample parses with an error node (the grammar's `source_file` is container members), yet every capture was right: keys `property`, strings, `0x1234abcd` number, `true` boolean. | Probe `zigprobe`, binary `zon` | US-001, US-002 |
| R5 | Every query Paneflow ships today has zero general predicates: the 15 Zed queries plus the crate queries of TOML, HTML, Java and Ruby (C++ not probed, git dependency). | Probe `predprobe` on 2026-11-22 | US-003 can land without breaking existing languages |
| R6 | t3code highlights opened files with `@pierre/highlights` (`PREFERRED_HIGHLIGHTER = "highlights"`, `apps/web/src/lib/syntaxHighlighting.ts:22`), not Shiki. On mobile it runs the same lexers translated by wasm2c 1.0.42 (`apps/mobile/scripts/generate-highlights.mts:16,150-156`), with `WASM_RT_USE_MMAP=0 WASM_RT_MEMCHECK_BOUNDS_CHECK=1 WASM_RT_SKIP_SIGNAL_RECOVERY=1` (`modules/t3-highlights/android/CMakeLists.txt:31-32`) and a C shim that catches traps with `WASM_RT_SETJMP` (`cpp/t3_highlights.c`). Its `getFiletypeFromFileName` has no `zon` entry, so t3code shows `build.zig.zon` plain. | t3code at `30372d0f` | TD-02, US-007, US-008 |
| R7 | t3code's wasm2c output (`highlights.c`, 5,576,251 bytes, from 0.5.0) compiled with those flags and `-std=c11`: gcc `-O2` 12.6 s, 349 MiB peak RSS, 752,230 bytes of text; clang `-O2` 7.1 s, 230 MiB, 849,578 bytes. The wasm2c runtime adds 3.4 KB. | Measured on this Linux x86_64 machine, 2026-11-22 | TD-02, US-006 |
| R8 | WABT 1.0.42 (2026-09-15) is the latest release, with Linux x64/arm64, macOS arm64 and Windows x64 binaries and SHA-256 files. Its CI runs tests on Ubuntu, macOS and Windows; whether Windows compiles wasm2c output with MSVC is not stated. The trap jump buffer and call depth are thread-local (`_Thread_local` under C11, `__declspec(thread)` on MSVC); `__thread` is disabled on Apple, so the build must be C11. Call depth defaults to 500. | [WABT releases](https://github.com/WebAssembly/wabt/releases), [build.yml](https://raw.githubusercontent.com/WebAssembly/wabt/main/.github/workflows/build.yml), [wasm-rt.h](https://raw.githubusercontent.com/WebAssembly/wabt/main/wasm2c/wasm-rt.h); vendored `wasm-rt.h:54-86` in t3code | US-006, US-007, A3 |
| R9 | `@pierre/highlights` 0.5.2 (2026-11-20) is the latest of 15 releases since 2026-10-05, Apache-2.0, 303,786-byte WASM, 152 lexers plus `plain`; its `dist/languages.js` maps both `zig` and `zon` to id 152 (lines 350-351) and `dockerfile`, `makefile` to 35 and 86. Its source is public in `pierrecomputer/pierre` (`packages/highlights`, Apache-2.0, WAT under `src/`), whose package is still at 0.5.1 (last change 2026-11-11). Between 0.5.1 and 0.5.2 two token types were inserted mid-table (`emphasis.strong.italic`, `markup.strikethrough`), shifting ids; the language table did not change. | [npm registry](https://registry.npmjs.org/@pierre/highlights); tarball read in the scratchpad; GitHub API | US-004, US-008, US-009, TD-03 |
| R10 | The prototype measured layer 1 under wasmtime on the 295 KB Rust corpus: full coloring 3.40 ms (87 MB/s) against 45.16 ms for tree-sitter; a keystroke 4.5 us median; a 60-row viewport query 3.6 us; 2.08 MiB retained against 9.79 MiB; agreement with tree-sitter 84.1 % on Rust, 96.6 % TypeScript, 92.1 % Python, 67.3 % Markdown, 49.8 % HTML (the last two because only the lexer colors nested code). | `tasks/prd-files-experience.md:827-866` | TD-04, TD-05, US-016, US-018 |
| R11 | Zed's `semantic_tokens` setting is `off` by default, per language, with modes `off`, `combined` (over tree-sitter) and `full`; styling rules map token types and modifiers to theme keys. VS Code draws semantic tokens over TextMate; Neovim adds them over treesitter with higher priority. | [Zed semantic tokens](https://zed.dev/docs/semantic-tokens), [VS Code guide](https://code.visualstudio.com/api/language-extensions/semantic-highlight-guide), [Neovim lsp.txt](https://raw.githubusercontent.com/neovim/neovim/master/runtime/doc/lsp.txt) | TD-08, US-024 |
| R12 | LSP 3.17 semantic tokens: client capability `requests.{range, full.delta}`, `tokenTypes`, `tokenModifiers`, `formats: ["relative"]`, `overlappingTokenSupport`, `multilineTokenSupport`, `augmentsSyntaxTokens`; 5 integers per token (deltaLine, deltaStart, length, type, modifier bitset); delta edits refer to the old array; `workspace/semanticTokens/refresh` behind `refreshSupport`; `general.positionEncodings` defaults to `utf-16`, which servers must support. Messages are framed by a `Content-Length` header and a blank line. | [LSP 3.17](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/) | US-021, US-023, US-028 |
| R13 | Zed starts every worktree in Restricted Mode, which does not spawn project language servers until the user trusts the worktree; VS Code's Workspace Trust restricts tasks, debugging and extensions. rust-analyzer runs build scripts and proc macros. | [Zed worktree trust](https://zed.dev/docs/worktree-trust), [Zed blog](https://zed.dev/blog/secure-by-default), [VS Code Workspace Trust](https://code.visualstudio.com/docs/editing/workspaces/workspace-trust) | TD-10, US-027 |
| R14 | rust-analyzer adds custom token types including `lifetime`, `macroBang`, `procMacro`, `selfKeyword`, `builtinType`, `unresolvedReference`, `formatSpecifier`. gopls semantic tokens default to off. | [rust-analyzer semantic_tokens.rs](https://raw.githubusercontent.com/rust-lang/rust-analyzer/master/crates/rust-analyzer/src/lsp/semantic_tokens.rs), [gopls settings](https://go.dev/gopls/settings) | TD-09, US-024 |
| R15 | `lsp-types` has had no release since 0.97.0 (2024-06-04); the community fork `ls-types` is at 0.0.6. | [crates.io lsp-types](https://crates.io/api/v1/crates/lsp-types), [docs.rs ls-types](https://docs.rs/crate/ls-types/latest) | TD-06 |
| R16 | `paneflow-process::spawn_line_session` already runs a stdio JSON-RPC child cross-platform (`CREATE_NO_WINDOW`, own process group, Job Object kill), but frames by newline (`crates/paneflow-process/src/lib.rs:839-885`); its callers are the Codex usage probes in `paneflow-serve`. The crate keeps at most `STDERR_CAP` = 64 KiB of a child's stderr (`lib.rs:13`). | Code map | US-020 |
| R17 | The macOS entitlements already grant `allow-jit` and `allow-unsigned-executable-memory` (`packaging/macos/paneflow.entitlements:73-76`), so the JIT objection to the wasmtime option was weaker than recorded; its 9.67 MiB and the Cranelift dependency remain. wasm2c output needs neither entitlement. | Code read | TD-02 |
| R18 | The highlighter is built by `open_blocking` (`src-app/src/app/diff_dock/code/load.rs:157-160`), which runs through `spawn_blocking_then` on `smol::unblock` (`src-app/src/app/diff_dock/code/mod.rs:28-49`), so work added there is off the render thread. The Files dock root is the workspace's working directory (`src-app/src/app/files_sidebar/mod.rs:123`). | Code map | US-011, TD-07 |

Research ran on 2026-11-22. Not verified: zls's semantic token support and position encodings, MSVC's handling of a 5.6 MB generated C file, whether WABT's Windows and Linux binaries emit identical C, and Helix's approach. Each one that a decision rests on is an assumption below with a validation story.

## Technical Decisions

**TD-01: Zig from the 1.1.2 crate with Zed's extension query.** Options: (a) the crate and its query, (b) the crate and Zed's query, (c) the git commit Zed pins, `6479aa13`, as `tree-sitter-cpp` already is (`src-app/Cargo.toml:168`, `deny.toml:152-154`). (a) is wrong by R2. (b) compiles and colors correctly on 1.1.2 (R2, R4) and stays on crates.io. (c) matches Zed exactly but adds a git source for an untagged commit. Choice: (b); revisit when a crate release includes the ABI 15 parser.

**TD-02: layer 1 is Pierre's WASM translated to C by wasm2c, not a hand rewrite and not wasmtime.** The 2026-11-19 report weighed a Rust rewrite (cost: write and maintain each lexer), wasmtime (9.67 MiB, a frozen third-party module, JIT), and tree-sitter only. wasm2c was not considered (`wasm2c` appears nowhere in `tasks/`). It keeps the 152 lexers and nested blocks of the wasmtime option at about 0.75 to 0.85 MiB of code (R7), with no runtime engine and no JIT (R17), and with memory safety from explicit bounds checks. t3code ships this exact pipeline (R6), and Pierre's source is public for upstream fixes (R9). What remains of "frozen module" is handled by TD-03 and TD-04. This departs from the decision's "lexer natif en Rust"; Arthur may overturn it.

**TD-03: pin the npm artifact, generate everything from it.** Inputs: the `@pierre/highlights` 0.5.2 tarball checked against its npm `sha512` integrity, the WASM checked against a recorded SHA-256, WABT 1.0.42 binaries checked against their published SHA-256, SIMDe's amalgamated `wasm/simd128.h` at a pinned commit (t3code vendors 0.8.2, `71fd833d`, the latest SIMDe release). The public source lags npm (R9), so building from source cannot reproduce 0.5.2 today; the manifest records the source repository for provenance. Token and language ids are an ABI that moved between 0.5.1 and 0.5.2 (R9), so the Rust tables are generated in the same run as the C, never written by hand.

**TD-04: when a lexer gets a native rewrite.** For a language with a grammar, layer 1 lives only until the tree arrives (18.67 ms median on the 300 KB corpus), so its gaps cost a short color change, measured by US-019. For a language without a grammar, layer 1 is final, and its gaps are what the user keeps. A rewrite is justified when, for a final-layer language, a reported defect or a test-only tree-sitter oracle shows agreement below the floor Rust had in the prototype report (84.1 %, R10), which the 2026-11-19 report judged blocking for a final layer. Until then, fixes go upstream to Pierre. This PRD builds the measurement; it schedules no rewrite.

**TD-05: how layers combine in a row.** Per byte, the topmost layer that has a color wins: layer 3 (where a token maps to a role), then layer 2 when the row is `Fresh`, then layer 1. One exception: inside a nested-code region of a layer-2 tree, layer 1 shows through even when layer 2 is fresh. The regions are a fixed table of node kinds, checked to exist in the grammars: Markdown `code_fence_content` (`tree-sitter-md` node types) and HTML `raw_text` (`tree-sitter-html` node types). This gives nested colors with no injection queries and keeps one layer contract.

**TD-06: the LSP types are written in `paneflow-lsp`, not taken from a crate.** `lsp-types` is unreleased since 2024 and `ls-types` is at 0.0.x (R15). The client needs 16 message shapes: `initialize`, `initialized`, `shutdown`, `exit`, `textDocument/didOpen`, `didChange`, `didClose`, `textDocument/semanticTokens/full`, `full/delta`, `range`, `workspace/semanticTokens/refresh`, `workspace/configuration`, `window/workDoneProgress/create`, `client/registerCapability`, `$/cancelRequest` and `window/logMessage`. A small serde module with round-trip tests against the spec's examples keeps them exact.

**TD-07: servers live in the window process, on background threads, one per server and root.** Options: window, worker (`paneflow-serve`), host (`paneflow-host`). The host owns PTYs and child processes of panes (ARCHITECTURE.md:29). The worker survives UI restarts, which language servers do not need: their state is the open documents, which live in the window. The window already runs child processes off the main thread (git through `src-app/src/git_command.rs`). Every read and write to a server runs on background threads; the render thread only reads finished token arrays. The root of a document is the Files dock root of its workspace (R18), canonicalized; a document outside that root gets no server.

**TD-08: semantic token types map to the capture names the palette already resolves.** Each type becomes a capture-style name resolved by `DiffSyntax::color_for_capture` (`src-app/src/diff/syntax.rs:22-89`), the same chain layers 1 and 2 use, so a theme recolors all three layers alike. Names were chosen to agree with Zed's Rust query where it colors the same construct (uppercase identifiers as `type`, `self` as `variable.special`, lifetimes uncolored, `src-app/src/diff/queries/rust/highlights.scm:11,70-72,225-227`). As in layer 2, `variable.parameter` resolves to the `property` role (`syntax.rs:69`), so parameters and fields share a color.

**TD-09: the token mapping table.**

| LSP token type | Capture name |
|---|---|
| `namespace` | `namespace` |
| `type`, `class`, `enum`, `interface`, `struct`, `typeParameter` | `type` |
| `parameter` | `variable.parameter` |
| `variable` | `variable` |
| `property` | `property` |
| `enumMember` | `type` |
| `event` | `variable` |
| `function`, `method`, `macro` | `function` |
| `keyword`, `modifier` | `keyword` |
| `comment` | `comment` |
| `string` | `string` |
| `number` | `number` |
| `regexp` | `string.regex` |
| `operator` | `operator` |
| `decorator` | `attribute` |
| rust-analyzer `selfKeyword` | `variable.special` |
| rust-analyzer `builtinType` | `type.builtin` |
| rust-analyzer `procMacro` | `function` |
| rust-analyzer `formatSpecifier` | `string.special` |
| rust-analyzer `lifetime`, `macroBang`, `unresolvedReference` | none (transparent) |
| any other type | none (transparent) |

Modifiers: `defaultLibrary` on `function`, `type` or `variable` appends `.builtin`; `documentation` on `comment` gives `comment.doc`; every other modifier is ignored. The client advertises `augmentsSyntaxTokens: true` so servers may leave keywords, strings and comments to the lower layers.

**TD-10: consent per server and project root, stored as user state.** A language server can execute the project (R13). Off by default is not enough on its own: once a user enables rust-analyzer, opening a file from a freshly cloned repository, which agents do routinely, would run its build scripts silently. The first time a (server, root) pair would start, the editor asks in its banner area (`src-app/src/app/diff_dock/code/view/render.rs:49`). The answer is stored in `~/.paneflow/language-server-trust.json`, owned by `crates/paneflow-home` like `window-state.json` (`crates/paneflow-home/src/lib.rs:96`), never in the repository. This is narrower than the per-directory trust cut on 2026-05-01: it gates one action, it configures nothing.

**TD-11: the query manifest gains per-entry sources.** `src-app/src/diff/queries/MANIFEST.toml` has one top-level `repository`, `commit`, `license` and `license_evidence` for Zed (`MANIFEST.toml:1-4`). A `[[queries]]` entry may now carry its own `repository`, `commit`, `license` and `license_evidence`, which override the top-level values for that entry only. The Zig entry uses them; the 15 Zed entries are unchanged. The sync scripts read Zed queries from `ZED_DIR` and the Zig query from `ZED_ZIG_DIR`.

## Assumptions & Constraints

| # | Assumption | Risk | Validated by |
|---|------------|------|--------------|
| A1 | The wasm2c translation of 0.5.2 produces the same tokens as the WASM under wasmtime. | Medium: wasm2c passes the spec tests in WABT CI (R8), but SIMD goes through SIMDe | US-009 |
| A2 | Native lexing is at least as fast as the wasmtime figures of R10. | Medium: no native measurement yet | US-016 |
| A3 | MSVC compiles the 5.6 MB generated C with `/std:c11`, `_Thread_local` works there, and the `Windows x86_64 libghostty check` job still finishes inside its 50-minute timeout (`.github/workflows/run_tests.yml:875`). | High (R8: not shown in WABT CI) | US-039, then US-033 |
| A4 | Layer 1 retains about 7 bytes per input byte (2.08 MiB for 295 KB is 7.05, R10), so a 10 MiB file needs about 70 MiB. The cap rule of US-015 allows up to 8 bytes per input byte, a 13 % margin over the one measured point; the margin itself is not evidenced. | Medium: one measured point | US-015 |
| A5 | zls implements `textDocument/semanticTokens/full` and accepts `utf-8` position encoding. | Medium (not researched) | US-029 test against zls, US-036 |
| A6 | rust-analyzer accepts `utf-8` position encoding. | Low (well-known, not fetched) | US-023 |
| A7 | A server answers `shutdown` within 5 s; past that the client kills it. 5 s mirrors `INITIAL_PARSE_TIMEOUT` (`highlight.rs:22`), not a measured server figure. | Low | US-022 |
| A8 | No legitimate LSP message exceeds 64 MiB. Reasoning: semantic tokens for the largest editable file (10 MiB) at one token per 6 bytes is about 1.7 million tokens, about 21 MB of JSON. | Medium | US-021 |
| A9 | One outstanding semantic token request per document, re-sent once when edits arrived meanwhile, keeps tokens current without a timer. | Low | US-028 |
| A10 | Rust's `Command` runs an npm `.cmd` server shim on Windows when resolved with `which` (Rust 1.77 hardened batch argument handling). | Medium (from memory) | US-036 |
| A11 | The first frame with layer 1 changes few enough bytes at the swap to help rather than distract. No threshold is evidenced; US-019 measures it and Arthur sets the policy (Open Question 2). | Medium | US-019, US-035 |
| A12 | WABT 1.0.42's Windows x64 `wasm2c` emits the same C as its Linux x64 binary for the same WASM, so the PowerShell `-Check` agrees with the shell `--check`. | Medium (not researched) | US-039 |
| A13 | The lexer's linear memory stays under 8 bytes per input byte (A4) plus its 81,920-byte static area (`INPUT_OFFSET` in t3code's shim), so a memory ceiling of 8 × `MAX_FILE_BYTES` (80 MiB) refuses only pathological inputs. | Medium, tied to A4 | US-007, US-015 |

Constraints from `AGENTS.md`: every change works on Linux, macOS and Windows, with Windows verified on hardware after a push; no comments in Rust, shell or PowerShell sources, and the same rule applies to the C shim Paneflow writes (vendored and generated files keep their upstream text); the render thread never blocks; helper binaries keep their size caps (the new crates are not linked into them); vendored query bytes change only through their sync scripts.

## Epics & User Stories

### EP-001: Zig through tree-sitter, and queries that cannot lie silently

Zig and ZON files are colored by tree-sitter, and a shipped query with an ignored predicate or drifted bytes fails CI.

**Definition of Done:** `grammar_for_ext("zig")` and `grammar_for_ext("zon")` return the same compiled grammar; the parity, token and malformed-input tests cover Zig; the predicate guard and the manifest hash check run in `cargo nextest` on every CI leg.

#### US-001: Register Zig with a vendored query and its provenance
**Description:** As a user reading Zig in the Files dock, I want `.zig` and `.zon` files colored by tree-sitter so that they read like Rust files do.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given `src-app/Cargo.toml`, when the dependency list is read, then it contains `tree-sitter-zig = "1.1.2"` from crates.io
- [x] Given `Cargo.lock`, when it is read, then it pins `tree-sitter-zig` 1.1.2
- [x] Given the commit that adds the dependency, when its file list is read, then it contains both `src-app/Cargo.toml` and `Cargo.lock`
- [x] Given `grammar_for_ext("zig")`, when it is called, then it returns a grammar whose query is `src-app/src/diff/queries/zig/highlights.scm`
- [x] Given `grammar_for_ext("zon")`, when it is called, then it returns the same `&'static Grammar` as `"zig"`
- [x] Given `src-app/src/diff/queries/zig/highlights.scm`, when its SHA-256 is computed, then it is `8f7f115b4a48a3336609ee8a087e634290131816d18d00fc11ef840be880df8e`
- [x] Given `MANIFEST.toml`, when it is read, then its Zig entry carries `repository = "https://github.com/zed-extensions/zig"`, `commit = "996432e66cb5bb2eed5a30ad2afaaa90349a3817"`, `source = "languages/zig/highlights.scm"`, its SHA-256 and `license = "Apache-2.0"` (TD-11)
- [x] Given `MANIFEST.toml`, when it is read, then the top-level Zed fields and the 15 Zed entries are unchanged
- [x] Given `src-app/src/diff/queries/NOTICE`, when it is read, then it attributes the Zig query to zed-extensions/zig under Apache-2.0
- [x] Given `src-app/src/diff/queries/zig/LICENSE`, when it is read, then it holds the Apache-2.0 text of zed-extensions/zig
- [x] Given `ZED_DIR` and `ZED_ZIG_DIR` pointing at local checkouts, when `scripts/sync-zed-queries.sh --check` runs, then it exits 0 and prints `15 Zed queries and 1 Zed extension query verified`
- [x] Given the same checkouts, when `scripts/sync-zed-queries.ps1 -Check` runs under PowerShell 7, then it exits 0 with the same message
- [x] Given one byte of the vendored Zig query changed, when `--check` runs, then it exits 1 and names `zig/highlights.scm`
- [x] Given `ZED_ZIG_DIR` unset, when the script runs, then it exits non-zero with a message naming the variable
- [x] Given `ZED_ZIG_DIR` unset, when the script exits, then no file under `src-app/src/diff/queries/` changed
- [x] Given `ARCHITECTURE.md`, when the vendored-queries section is read, then it documents the second source, `ZED_ZIG_DIR` and the Apache-2.0 license

#### US-002: Pin Zig colors with parity and token tests
**Description:** As a maintainer, I want the Zig and ZON colors asserted by tests so that a grammar or query bump that breaks them fails CI.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given `parity_tests.rs` `CORPUS`, when it is read, then it has a `sample.zig` with a `//!` comment, a `///` comment, a `const` in capitals, a `struct`, a `fn` declaration, a method call, `@import`, a string, a character, an integer, a float, `try`, `defer`, a `for` with a `|i|` payload and an `if`
- [x] Given the test `zig_query_compiles`, when it runs, then `grammar_for_ext("zig")` is `Some`
- [x] Given `extension_aliases_select_the_intended_query`, when it runs, then it checks the group `["zig", "zon"]`
- [x] Given `stock_queries_report_every_same_node_priority_change`, when it runs, then its match reads `tree_sitter_zig::HIGHLIGHTS_QUERY` for `zig`
- [x] Given `fixtures/stock-priority-audit.txt`, when the test compares it, then it contains the reviewed Zig lines
- [x] Given `every_vendored_query_is_exercised_by_at_least_five_classes`, when it runs on `sample.zig`, then it finds at least five capture classes
- [x] Given `token_expectations_cover_every_vendored_language`, when it runs, then its Zig case asserts `Point` is `type`, `len` is `function.definition`, `MAX_LEN` is `constant`, `f32` is `type.builtin` and the string literal is `string`
- [x] Given a ZON sample in a test, when its captures are read, then `.name` is `property`, a string is `string`, `0x1234abcd` is `number` and `true` is `boolean`
- [x] Given `DiffSyntax::color_for_capture("comment.documentation")`, when it is called, then it returns the `comment_doc` color (today it returns `comment`, because `cap_has` matches whole segments, `syntax.rs:5-6`)
- [x] Given `malformed_and_empty_inputs_never_panic`, when it runs, then its extension list includes `zig` and `zon` and none panics

#### US-003: Fail on predicates the runtime ignores, and on vendored bytes that drift
**Description:** As a maintainer, I want a test that rejects any shipped query with a predicate tree-sitter does not evaluate, and any vendored query whose bytes do not match its manifest, so that a query never colors wrongly without an error.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given every extension `grammar_for_ext` accepts plus the Markdown inline grammar, when the guard test runs, then it asserts `Query::general_predicates` is empty for every pattern of every query, except the one predicate named in `PREDICATES_ZED_IGNORES_TOO` (see the implementation note below)
- [x] Given a test-only query containing `#lua-match?`, when the guard function checks it, then it returns a failure naming the operator and the pattern index
- [x] Given a query using `#set!`, when the guard checks it, then it passes, because `set!` is a property setting and not a general predicate
- [x] Given `MANIFEST.toml`, when the manifest test runs, then the SHA-256 of every listed query file equals the recorded hash, with no network and no upstream checkout
- [x] Given one byte changed in any vendored query, when the manifest test runs, then it fails naming that file
- [x] Given a `highlights.scm` under `src-app/src/diff/queries/` absent from the manifest, when the manifest test runs, then it fails naming the file

**EP-001 implementation notes (2026-11-22, awaiting review):**

- **Correction to R5.** Zed's C++ query, which R5 did not probe, has one general predicate: `#has-ancestor?` in pattern 13 (`src-app/src/diff/queries/cpp/highlights.scm:54`). The file is vendored bytes and Zed ignores that predicate too (`src-app/src/diff/fixtures/README.md`), so the guard cannot require zero. It names that single (query, pattern, operator) in `PREDICATES_ZED_IGNORES_TOO` (`src-app/src/diff/query_guard.rs`); any other unevaluated predicate fails, and so does the entry once it stops matching. Arthur may prefer another treatment.
- **Script message.** Both scripts print `15 Zed queries and 1 Zed extension query verified at <zed commit> and <zig commit>`; the two commits follow the phrase the criterion names. They also take `--zig-commit` / `-ZigCommit` and sync `zig/LICENSE` from the same commit.
- **PowerShell twin.** `sync-zed-queries.ps1` was run under PowerShell 7.4.2 in a Linux container (check, sync with byte-identical output, tamper, unset variable). It has not run on Windows.
- **Commit criterion.** The dependency criterion about the commit's file list stays open until Arthur commits; `src-app/Cargo.toml` and `Cargo.lock` are both changed in the working tree.
- **Zig entry extras.** The manifest entry also records `license_evidence` and one deviation: the 1.1.2 crate parser instead of the grammar commit the extension pins (TD-01).

**EP-001 review (2026-11-22):** US-001, US-002 and US-003 are `DONE`; no code defect was found.

- **Evidence.** On the final tree, `cargo fmt --check`, clippy with `--all-targets`, `cargo nextest run --workspace --locked --profile ci` (5278 passed), `check-test-wallclock.sh`, `check-helper-sizes.sh` and `cargo deny check advisories licenses sources` pass. `sync-zed-queries.sh --check` and `sync-zed-queries.ps1 -Check` (PowerShell 7.4.2 in a Linux container, not Windows) print the expected message against Zed `ee3b5558` and a fresh clone of zed-extensions/zig at `996432e6`; a one-byte change exits 1 naming `zig/highlights.scm`; an unset `ZED_ZIG_DIR` exits 1 and leaves the queries untouched. Zig reaches both real entry points: `CodeHighlighter` (`code/highlight.rs:185`) and the diff view (`row_syntax.rs:118`, `highlight_lines`).
- **Commits.** `fc2b683a` (US-001) holds both `src-app/Cargo.toml` and `Cargo.lock`; `61343e36` is US-002 and `8726a227` is US-003.
- **Dependency gate.** `cargo deny` first failed advisories on RUSTSEC-2026-0592 (`url` 2.5.8), an advisory unrelated to this epic. `9eb4c009` moves `url` to 2.5.9 and `form_urlencoded` to 1.2.3.

---

### EP-002: Pierre's lexers as a native crate

A workspace crate, `paneflow-highlights`, compiles the wasm2c translation of `@pierre/highlights` 0.5.2 on every shipping target and exposes a safe Rust API over its live tokenizer.

**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/mobile/modules/t3-highlights/Vendor/` lays out the same three vendored parts this epic ships (`highlights/` for the generated C, `wasm2c/` for the runtime, `simde/` for SIMD), and `apps/mobile/modules/t3-highlights/cpp/` holds the shim. Inspiration, not a copy: see the notes on US-007.

**Definition of Done:** the regeneration scripts reproduce the vendored files byte for byte from pinned inputs; the crate builds and its tests pass on the Linux x86_64, Linux aarch64 and macOS aarch64 CI jobs (the Windows leg is proven in US-039); the native engine's token ids equal the wasmtime prototype's on the bench corpora; wasmtime and the `lexer-prototype` feature are gone.

#### US-004: Regenerate the vendored lexer C from pinned inputs
**Description:** As a maintainer, I want one shell script that rebuilds every vendored lexer file from hashed inputs so that a Pierre update is a reviewed, reproducible diff.

**Priority:** P0
**Dependencies:** None
**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/mobile/scripts/generate-highlights.mts`: pinned wasm2c version check (`:16`, `:132-135`), the `wasm2c highlights.wasm -n highlights -o highlights.c` call (`:150-156`), the generated banner with version and WASM hash, the runtime copy from WABT's `share/wabt/wasm2c/` (`:169-173`). Paneflow adds what t3code lacks: tarball integrity, WABT download with hash, a `--check` mode.

**Acceptance Criteria:**
- [x] Given `scripts/sync-pierre-highlights.sh`, when it runs, then it downloads the `@pierre/highlights` 0.5.2 tarball into a new empty directory
- [x] Given the downloaded tarball, when it is checked, then its npm `sha512` integrity is verified before extraction
- [x] Given the extracted package, when the script continues, then the SHA-256 of `dist/highlights.wasm` equals the value recorded in `crates/paneflow-highlights/c/MANIFEST.toml`
- [x] Given no `WASM2C` variable, when the script needs wasm2c, then it downloads the WABT 1.0.42 archive for the host and verifies it against the published `.sha256` file
- [x] Given a `WASM2C` variable pointing at a binary whose `--version` is not `1.0.42`, when the script runs, then it exits non-zero naming the expected version
- [x] Given the inputs, when generation completes, then `crates/paneflow-highlights/c/generated/highlights.c` and `highlights.h` start with a banner naming the package version and the WASM SHA-256
- [x] Given the WABT archive, when generation completes, then the wasm2c runtime files the build uses are under `crates/paneflow-highlights/c/wasm2c/`
- [x] Given SIMDe commit `71fd833d9666141edcd1d3c109a80e228303d8d7`, when generation completes, then `c/simde/wasm/simd128.h` is vendored and its SHA-256 recorded in the manifest
- [x] Given `dist/languages.js` and `dist/token-types.js`, when generation completes, then `crates/paneflow-highlights/src/tables.rs` is regenerated from them in the same run
- [x] Given `MANIFEST.toml`, when it is read, then it records every input's version, URL and hash and the source repository `https://github.com/pierrecomputer/pierre`
- [x] Given `--check`, when the vendored files match a fresh generation, then the script exits 0 and prints the package version
- [x] Given `--check` and one changed byte in any generated or vendored file, when it runs, then it exits 1 and names the file
- [x] Given a download whose hash does not match, when the script runs, then it exits non-zero and names the input
- [x] Given a download whose hash does not match, when the script exits, then no vendored file changed

#### US-005: PowerShell twin of the generator, and the license files
**Description:** As a maintainer on any platform, I want the same generator in PowerShell and every license that travels with the vendored code so that the crate can be regenerated on any platform and redistributed lawfully.

**Priority:** P0
**Dependencies:** Blocked by US-004
**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/mobile/modules/t3-highlights/Vendor/wasm2c/LICENSE` and `apps/mobile/modules/t3-highlights/Vendor/simde/LICENSE`: the license files t3code vendors next to the runtime and SIMDe.

**Acceptance Criteria:**
- [x] Given `scripts/sync-pierre-highlights.ps1` with `-Check` under PowerShell 7, when the vendored files match, then it exits 0 and prints the package version
- [x] Given `-Check` and one changed byte, when it runs, then it exits 1 and names the file
- [x] Given `crates/paneflow-highlights/c/NOTICE`, when it is read, then it reproduces Pierre's `NOTICE.md`
- [x] Given the same file, when it is read, then it names Apache-2.0 as the license of the generated code
- [x] Given `c/wasm2c/LICENSE`, when it is read, then it holds WABT's Apache-2.0 text
- [x] Given `c/simde/LICENSE`, when it is read, then it holds SIMDe's MIT text
- [x] Given `THIRD_PARTY_NOTICES.md`, when it is read, then it lists `@pierre/highlights`, the wasm2c runtime and SIMDe with their licenses and vendored paths
- [x] Given a vendored file listed in the manifest but missing on disk, when either script runs `--check`, then it exits 1 and names the file

#### US-006: Compile the lexers on every shipping target
**Description:** As a maintainer, I want the vendored C built by `cargo` on Linux, macOS and Windows so that layer 1 ships on every platform with no extra toolchain. The Windows leg is proven in US-039.

**Priority:** P0
**Dependencies:** Blocked by US-004
**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/mobile/modules/t3-highlights/android/CMakeLists.txt:31-32` and `apps/mobile/modules/t3-highlights/T3HighlightsNative.podspec:27`: the three `WASM_RT_*` defines on Android and iOS. Paneflow adds `-std=c11` / `/std:c11`, which t3code does not set (R8).

**Acceptance Criteria:**
- [x] Given the workspace `Cargo.toml`, when it is read, then `crates/paneflow-highlights` is a member with `license.workspace = true`, `publish = false` and the workspace lints
- [x] Given the crate's `build.rs`, when it compiles the C with `cc`, then it defines `WASM_RT_USE_MMAP=0`, `WASM_RT_MEMCHECK_BOUNDS_CHECK=1` and `WASM_RT_SKIP_SIGNAL_RECOVERY=1` on every target
- [x] Given a gcc or clang target, when `build.rs` compiles, then it passes `-std=c11`
- [x] Given an MSVC target, when `build.rs` compiles, then it passes `/std:c11`
- [x] Given the `Tests (Linux x86_64)` job on the story's commit, when it runs, then the crate's tests pass
- [x] Given the Linux aarch64 job on the story's commit, when it runs, then the crate's tests pass
- [x] Given the `macos_check` job on the story's commit, when it runs, then the crate's tests pass
- [x] Given a target outside the four shipping targets, when the crate builds, then `build.rs` fails with a message naming the target, as `src-app/build.rs` does for libghostty
- [x] Given stripped release builds of `paneflow` with and without the crate on Linux x86_64, when their sizes are compared, then the delta is recorded in this PRD
- [x] Given the recorded delta, when it is compared, then it is below 9.67 MiB
- [x] Given a release build on Linux x86_64, when its build log is read, then the compile time of `highlights.c` is recorded in this PRD
- [x] Given `paneflow-shim`, `paneflow-ai-hook` and `paneflow-mcp`, when their dependency trees are read, then none depends on `paneflow-highlights`

#### US-007: A safe Rust API over the live tokenizer, with traps contained in C
**Description:** As a developer of the editor, I want a Rust type that owns one lexer instance and its document so that the editor lexes, edits and queries lines without touching C.

**Priority:** P0
**Dependencies:** Blocked by US-006
**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/mobile/modules/t3-highlights/cpp/t3_highlights.c`: trap catch with `WASM_RT_SAVE_STACK_DEPTH` and `WASM_RT_SETJMP` around the generated call, instance reset after a trap or after memory grew past `RESET_AFTER_PAGES`, memory growth before staging, the host imports `w2c_env_is_id_start`/`w2c_env_is_id_continue`, and `apps/mobile/modules/t3-highlights/cpp/generated/id_ranges.inc` for their Unicode tables. Do not copy two choices: t3code keeps one static instance (Paneflow needs one per document, on several threads) and uses the one-shot UTF-16 line-record mode (Paneflow uses the live API with byte ranges, as `lexer_prototype.rs` does).

**Acceptance Criteria:**
- [x] Given `Lexer::new()`, when it is called, then it allocates one wasm2c instance on the heap through the C shim
- [x] Given a `Lexer`, when it is dropped, then its instance is freed (checked by an instance counter in a test build)
- [x] Given a `LexDocument` built from a text and a language id, when it is settled, then `line_tokens(line)` returns token ids with byte ranges into the line, not UTF-16 columns
- [x] Given an edit with a byte range and new text, when it is applied, then the document re-lexes until its state matches the old one
- [x] Given that edit, when it completes, then the document reports the range of lines whose tokens changed
- [x] Given two documents on two threads, when both are lexed concurrently, then both results equal their single-threaded results
- [x] Given `LexDocument`, when a compile-time assertion checks it, then it is `Send` and not `Sync`
- [x] Given the host imports `is_id_start` and `is_id_continue`, when the lexer calls them, then they answer from `unicode-ident` with the prototype's rules (XID, plus ZWNJ and ZWJ for continuation)
- [x] Given the host-import functions, when their source is read, then they are `extern "C"` Rust functions that call no shim entry point
- [x] Given the C shim, when its source is read, then every exported function that runs lexer code calls `WASM_RT_SETJMP` before entering the generated code
- [x] Given a test-only shim entry that forces a trap, when it is called, then it returns an error whose variant names a trap
- [x] Given that trap, when a new `Lexer` lexes a document afterwards, then its tokens equal a clean run
- [x] Given an input whose staging would pass `MAX_MEMORY_BYTES` = 8 × `MAX_FILE_BYTES` (assumption A13), when it is staged, then the call returns an error and memory does not grow
- [x] Given the C shim written for Paneflow, when it is read, then it has no comments, per `AGENTS.md`

#### US-008: Language and token tables generated from the pinned release
**Description:** As a user opening any file, I want layer 1 to know every language Pierre ships so that a language without a grammar is still colored.

**Priority:** P0
**Dependencies:** Blocked by US-004
**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/mobile/src/lib/highlights.generated.ts` (language and token id tables generated from the same package version as the C, `zig: 152`, `zon: 152` at `highlights.generated.ts:351-352`) and `node_modules/.pnpm/@pierre+diffs@1.7.0-beta.4_patch_hash=b24b206d14c79de95be10322b96490b82e41cc13c8a02c737_31f9dca5c277eb0b5b13f052c5640fbe/node_modules/@pierre/diffs/dist/utils/getFiletypeFromFileName.js` (extension and file-name lookup, no `zon` entry at `:359`, fallback to `text` at `:363-380`), which Paneflow's `language_for_path` improves on.

**Acceptance Criteria:**
- [x] Given `tables.rs`, when it is read, then it maps every key of the pinned `languages.js` to its id
- [x] Given `tables.rs`, when it is read, then it holds every name of `token-types.js` in the file's order
- [x] Given `language_for_path`, when it is called with `foo.zig`, `build.zig.zon`, `main.kt`, `init.lua` and `query.sql`, then it returns the ids of `zig`, `zon`, `kt`, `lua` and `sql`
- [x] Given `language_for_path` with `Dockerfile` and `Makefile`, when it is called, then it matches the lowercased file name and returns ids 35 and 86
- [x] Given `README.unknownext`, when it is called, then it returns `None`
- [x] Given every id from 1 to 152, when the coverage test runs, then each is reachable from at least one key
- [x] Given every token name, when it is resolved through `DiffSyntax::color_for_capture`, then it gets a color or is in an explicit, tested list of uncolored names
- [x] Given a generated token table whose length differs from the count the WASM reports, when the crate's tests run, then a test fails naming both counts

#### US-009: Prove the native build matches the wasmtime prototype
**Description:** As a maintainer, I want evidence that wasm2c changed nothing in the lexers' output so that the prototype's measurements carry over to the native engine.

**Priority:** P0
**Dependencies:** Blocked by US-007, US-008

**Acceptance Criteria:**
- [x] Given the 0.5.2 `highlights.wasm` loaded by the existing prototype through `PANEFLOW_HIGHLIGHTS_WASM`, and the native crate, when both lex the bench corpora (Rust 300 KB, Markdown, TypeScript, Python, HTML) and `sample.zig`, then every line's list of (byte range, token id) is identical
- [x] Given the comparison, when it completes, then its per-corpus line and token counts are recorded in this PRD
- [x] Given a mismatch, when the comparison runs, then it reports the first differing line, its byte column and both token ids
- [x] Given the comparison, when its code is read, then it compares token ids and does not use the prototype's 0.5.1 `TOKEN_NAMES`

#### US-010: Retire wasmtime and move the lexer bench to the native crate
**Description:** As a maintainer, I want one lexer engine in the tree so that the bench and the oracle measure what ships.

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Given `src-app/src/app/diff_dock/code/`, when the story is done, then `lexer_prototype.rs` no longer exists
- [x] Given `src-app/Cargo.toml`, when it is read, then the `lexer-prototype` feature is gone
- [x] Given `src-app/Cargo.toml`, when it is read, then the optional `wasmtime` and `unicode-ident` entries are gone
- [x] Given `lexer_compare.rs`, when it runs, then it uses `paneflow-highlights`
- [x] Given the editor bench's lexer scenarios, when the default suite runs, then they run on `paneflow-highlights` with no feature flag
- [x] Given `scripts/bench-editor.sh --lexer-prototype`, when it runs, then it exits non-zero with a message saying the lexer metrics are part of the default suite
- [x] Given `scripts/bench-editor.ps1 -LexerPrototype`, when it runs, then it exits non-zero with the same message

**EP-002 implementation notes (2026-11-22, reviewed):**

- **Inputs, all verified before anything is written.** npm tarball `sha512-CV+yFa8d...` (integrity), `highlights.wasm` SHA-256 `ee1152cd...`, the WABT 1.0.42 archive of the host against its published `.sha256` and the value pinned in `c/MANIFEST.toml` (all four hosts pinned), SIMDe from the `simde-amalgamated-0.8.2.tar.xz` release asset (its `wasm/simd128.h` is byte-identical to t3code's copy and names commit `71fd833d` on line 2), WABT `LICENSE` at tag `1.0.42` and SIMDe `COPYING` at `71fd833d`. A clean regeneration takes 1.6 s. The manifest's pins are hand-edited; its `[[files]]` list is written by the scripts.
- **PowerShell twin.** `sync-pierre-highlights.ps1` ran under PowerShell 7.4.2 in a Linux container: `-Check` passes, a full write is byte-identical to the shell output, a changed byte, a missing file and a wrong `WASM2C` version each exit 1 naming the cause. Its Windows run is US-039 (A12). It normalizes CRLF in wasm2c's output to LF, so a Windows wasm2c that writes CRLF cannot make `-Check` fail on line endings alone. `.gitattributes` pins LF for the vendored files and both scripts.
- **CI, run 45049982695 on `cefbc532` (which contains the US-006 commit `9479ecd9`).** `Tests (Linux x86_64)`, `Linux aarch64 (aarch64-unknown-linux-gnu)` and `macOS aarch64 smoke build` (the `macos_check` job) each ran the 21 tests of `paneflow-highlights` and passed them (5303, 5303 and 5112 tests in all). No workflow change was needed: every job already runs `cargo nextest run --workspace` and the `rust` path filter covers `crates/**`.
- **MSVC defect found and fixed.** The same run's Windows job failed: `cl.exe` under `/std:c11` rejects the `_Atomic volatile uint8_t*` shared-memory declarations of `wasm-rt.h:513` (`error C2061: syntax error: identifier '_Atomic'`); `_Thread_local` itself compiled. `881aad39` adds `/experimental:c11atomics` on MSVC (Visual Studio 2022 17.5 and later; the runner has MSVC 14.44), which accepts one-byte lock-free atomics, so A3's `/std:c11` choice and the US-006 criterion stand. On `881aad39` (run 45053247908) the Windows job passed in 17 min 18 s, with the 21 crate tests among 5139 passed: evidence for US-039, which certifies it with the dual-boot run.
- **Allocation gate.** The `Performance gates (Linux x86_64)` job of run 45049982695 failed on 12 missing allocation baselines, the six lexer and tree-sitter comparison metrics that US-010 moved into the default suite. `1be94073` records them with `scripts/perf-gates.sh --refresh-alloc-baselines` on a clean tree at `881aad39`, and run 45053530681 on `1be94073` passed every required job, the performance gates and Windows included; no existing metric moved beyond the gate's 1 % tolerance, and the new values equal the CI runner's (`lexer_keystroke_mid_file` 39,845.92 bytes and 9 allocations per iteration on both).
- **Measured on Linux x86_64 (2026-11-22, gcc, `-O2`).** Stripped release `paneflow` with a probe that links the crate: 101,137,992 bytes; without it: 100,383,048 bytes; delta **754,944 bytes (0.72 MiB)**, below 9.67 MiB. The `paneflow-highlights` build-script run, which compiles `highlights.c` with the runtime and the shim, took **11.3 s** in that release build (`cargo --timings`). The crate is a dev-dependency of `paneflow-app` until EP-003 links it, so today's release binary does not carry it.
- **Shim design.** The host imports are Rust `extern "C"` functions passed to the shim as function pointers; the C `w2c_env_is_id_start` / `w2c_env_is_id_continue` only bounds-check the pointer and forward. That avoids a link-order cycle between the C archive and the Rust crate and keeps every `longjmp` inside C frames. The memory ceiling is enforced twice: `max_pages` is lowered to `MAX_MEMORY_BYTES` after instantiation, and a staging that would pass it is refused before `liveStage` runs. `paneflow_hl_probe_trap` is compiled into the archive but only declared by the crate's tests.
- **Per-call cost.** `wasm-rt.h` uses `sigsetjmp(buf, 1)` on Unix, which saves the signal mask through a system call on every shim entry: the bench measured 274 ns per empty call. The first bench run read rows one call each and gave 23.2 us per keystroke and 18.7 us per 60-row viewport, against 4.5 us and 3.6 us under wasmtime (R10). `paneflow_hl_read_lines` and `LexDocument::for_each_line_tokens` now read up to 64 rows under one `setjmp`. Final run (`scripts/bench-editor.sh`, release, cpu_share 0.995, a dirty tree so not a baseline): `lexer_full_color_300kb` 3.04 ms (wasmtime 3.40 ms), `lexer_keystroke_mid_file` 6.2 us (4.5 us), `lexer_viewport_query_60_rows` 2.1 us (3.6 us), `lexer_retained_bytes` 2.19 MiB (2.08 MiB), `lexer_instantiate` 10.1 us, `lexer_call_ns` 274 ns. The keystroke still pays about seven shim entries (edit, settle, changes, line remap); US-016 owns the A2 comparison. EP-003 should read rows through `for_each_line_tokens`.
- **Line model for EP-003.** The lexer breaks lines on `\r\n`, `\n` and a lone `\r`; the editor's ropey breaks on `\n` only (`src-app/Cargo.toml`, ropey comment). `LexDocument` therefore takes edits as document byte ranges and exposes `line_start(line)` in its own line numbering; EP-003 must map editor rows to lexer lines through byte offsets when a file contains a lone `\r`. Tokens cover whitespace with the neighboring token's id (`"    "` before `let` is `punctuation.bracket` when the previous line ended on `{`), so a consumer that compares roles should skip whitespace as `lexer_compare.rs` does.
- **US-008 uncolored token types.** 14 of the 75 names resolve to no palette role: `none`, `diff.minus`, `diff.plus`, `embedded`, `predictive`, `primary`, `selector`, `text.jsx`, `enum`, `hint`, `markup.strikethrough`, `variant`, `background`, `foreground` (`UNCOLORED_LEXER_TOKENS`, `src-app/src/diff/syntax.rs`). `enum` and `variant` staying uncolored may matter to the US-018 floors.
- **US-009 parity, 2026-11-22.** The wasmtime prototype loaded the 0.5.2 `highlights.wasm` and the native crate lexed the same texts; every line's (byte range, token id) list was identical:

| Corpus | Lines | Tokens |
|---|---|---|
| Rust, the 300 KB bench corpus | 9,246 | 67,175 |
| Markdown, the bench corpus | 2,733 | 7,606 |
| TypeScript, `scripts/generate-file-icons.ts` | 202 | 1,909 |
| Python fixture | 43 | 244 |
| HTML with script and style | 26 | 200 |
| Zig, `sample.zig` | 22 | 164 |

  The comparison was deleted with wasmtime in US-010. `lexer_correctness_report`, now on the native crate, reproduces R10's agreement figures exactly: Rust 84.1 %, TypeScript 96.6 %, Python 92.1 %, Markdown 67.3 %, HTML 49.8 %. Its result survives as `RECORDED_WASMTIME_FINGERPRINTS` (`src-app/src/app/diff_dock/code/lexer_parity.rs`): line count, token count and an FNV-1a hash of every (line, range, id) per corpus, which `native_tokens_match_the_fingerprints_recorded_from_the_wasmtime_prototype` checks on every CI run.

**EP-002 review (2026-11-22):** US-004 to US-010 are `DONE`; no code was changed.

- **Parity, reproduced independently.** The wasmtime comparison was never committed, so the review rebuilt it outside the repository: `lexer_prototype.rs` from `9479ecd9` under wasmtime 50.0.2, loading the 0.5.2 `highlights.wasm` from a fresh download whose `sha512` integrity and SHA-256 match `c/MANIFEST.toml`, against the native crate at `1c36b9ed`. Every line of the six corpora is identical, and the six wasmtime fingerprints equal `RECORDED_WASMTIME_FINGERPRINTS` exactly.
- **Generators.** In a copy of the tree, `sync-pierre-highlights.sh --check` passes, a full sync is byte-identical, and a changed byte (`c/wasm2c/wasm-rt-impl.c`), a missing file (`c/simde/LICENSE`), a `WASM2C` reporting 1.0.41 and a wrong `wasm_sha256` each exit 1 naming the cause; the hash failure leaves every vendored file unchanged. `sync-pierre-highlights.ps1 -Check` under PowerShell 7.4.2 in a Linux container (not Windows) passes, and a changed byte in `src/tables.rs` and a missing `c/NOTICE` exit 1 naming them. `c/NOTICE` ends with Pierre's `NOTICE.md` byte for byte.
- **Build and CI.** `build.rs` refuses `x86_64-apple-darwin` naming the target. `cargo tree` shows no path from `paneflow-shim`, `paneflow-ai-hook` or `paneflow-mcp` to the crate, and only a dev-dependency edge from `paneflow-app`. Run 45053530681 on `1be94073` is green, and its Linux x86_64, Linux aarch64, macOS aarch64 and Windows jobs each passed the 21 crate tests. The refreshed allocation baselines move no existing `alloc_bytes_per_iter` or `allocs_per_iter` value by more than 1 %; nine metrics are new.
- **Unsafe boundary.** Read in full, plus one independent read-only review: every shim entry that runs generated code arms the jump buffer first, every C pointer is bounds- and alignment-checked before Rust builds a slice, the line accessors behind `&self` are pure loads so no borrow outlives a `realloc`, and the only shared runtime state is thread-local under these flags. Ten pathological inputs (2 million nested `(`, `{`, `[`, `/*`, `[[`, `$(`, and similar) lex without fault on a 256 KiB thread stack.
- **Open, not blocking.** If `remap_lines` (`src/document.rs:357`) ever fails after the lexer accepted an edit, `apply_edit` returns an error but keeps the old `line_starts`, so a later edit would be positioned wrongly instead of refused. Only a lexer returning an inconsistent line table reaches it; EP-003 should treat any edit error as the end of that `LexDocument` and re-lex.
- **Reachability.** The crate is reached today only from tests and the editor bench (`lexer_parity.rs`, `lexer_compare.rs`, `perf_bench.rs`, `syntax.rs`), as the Definition of Done intends; EP-003 links it into the editor.
- **Evidence.** On the final tree: `cargo fmt --check`, clippy with `--all-targets`, `cargo nextest run --workspace --locked --profile ci` (5303 passed), `check-test-wallclock.sh`, `check-helper-sizes.sh` and `cargo deny check advisories licenses sources` pass. `bench-editor.sh --lexer-prototype` and `bench-editor.ps1 -LexerPrototype` exit 2 with the retirement message.

---

### EP-003: Progressive highlighting in the editor

The editor paints layer 1 wherever layer 2 is not ready or not available, and keeps nested code colored after the tree arrives.

**Definition of Done:** in the editor bench, the 300 KB corpus has colored rows in its first frame and edited rows are colored at the next paint; a Kotlin file, a Lua file and a Rust file between 2 MB and 10 MiB open colored; a Markdown fence and an HTML script stay colored after the tree; `scripts/perf-ab.sh` between the commit before US-011 and the commit of US-016 reports no regressed existing editor metric, or each one is listed in this PRD and accepted by Arthur.

#### US-011: Color the first frame from layer 1
**Description:** As a user opening a file, I want it colored in the first frame so that I do not watch plain text turn into colors.

**Priority:** P0
**Dependencies:** Blocked by US-007, US-008
**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/web/src/lib/syntaxHighlighting.ts:13-44`: extension overrides for dialects the lexers lack (`luau`, `xsl`, `xslt`) and the fallback to `"text"` when a language fails to load.

**Acceptance Criteria:**
- [ ] Given a file at or under its tree-sitter cap with a known lexer language, when `open_blocking` builds the highlighter off the render thread (R18), then it lexes the whole document before the document is shown
- [ ] Given that document, when its first frame paints, then `runs_into` returns layer-1 runs for every row that has a non-whitespace token
- [ ] Given layer-2 rows that become `Fresh`, when they paint, then they show layer-2 colors and no layer-1 color outside nested-code regions
- [ ] Given a file whose extension has neither a grammar nor a lexer language, when it opens, then it renders plain and editable as today
- [ ] Given a lexer error while opening, when the highlighter is built, then layer 1 is off for that document and layer 2 still runs
- [ ] Given that lexer error, when it happens, then one warning is logged with the file's extension and the error
- [ ] Given the theme switched while a document is open, when its rows repaint, then layer-1 colors follow the new palette without re-lexing

#### US-012: Keep edited rows colored through layer 1
**Description:** As a user typing, I want the row I am editing to stay colored so that the code does not flash plain on every keystroke.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given an edit batch, when `edit_batch` runs, then the same byte edits are applied to the layer-1 document on the render thread
- [ ] Given the edited row, when the next frame paints before layer 2 requeries it, then it shows layer-1 colors
- [ ] Given the test at `highlight.rs:1537`, when it runs, then it asserts layer-1 colors on the edited row instead of a plain row
- [ ] Given a layer-1 re-lex that exceeds `SYNC_PARSE_BUDGET`, when the budget is reached, then the rest of the re-lex runs off the render thread
- [ ] Given that deferred re-lex, when the next frame paints before it finishes, then the affected rows show their previous layer-1 runs shifted by the edit
- [ ] Given an undo of the edit, when it is applied, then the layer-1 and layer-2 rows equal those of a fresh open of the same text

#### US-013: Color languages that have no grammar
**Description:** As a user opening Kotlin, Lua, Swift, SQL or any of Pierre's languages without a tree-sitter grammar, I want them colored so that only truly unknown files are plain.

**Priority:** P0
**Dependencies:** Blocked by US-011
**t3code reference** (local checkout `/home/arthur/dev/t3code` at `30372d0f`): `apps/web/src/lib/syntaxHighlighting.ts:35-41`: an unknown or failing language degrades to plain text, never to an error.

**Acceptance Criteria:**
- [ ] Given a file with a lexer language and no grammar, when it opens, then the highlighter is enabled with layer 1 only
- [ ] Given that file, when `fill_stale_rows` is called, then it returns without querying and leaves the layer-1 rows unchanged
- [ ] Given that file, when it is edited, then the edited row is colored at the next paint as in US-012
- [ ] Given that file with the minimap on, when the minimap paints, then it shows the same layer-1 colors as the editor
- [ ] Given that file larger than 10 MiB, when it is opened, then the large-text preview shows it plain, unchanged from today

#### US-014: Keep nested code colored after the tree arrives
**Description:** As a user reading Markdown with code blocks or HTML with scripts, I want the nested code colored so that a README reads like the code it shows.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given a Markdown file with a fenced Rust block, when the layer-2 tree is fresh, then the bytes of `code_fence_content` show layer-1 colors
- [ ] Given the same file, when the fence delimiter and info string paint, then they keep their layer-2 `punctuation.embedded.markup` color
- [ ] Given an HTML file with `<script>`, when the tree is fresh, then the bytes of `raw_text` inside it show layer-1 colors
- [ ] Given an HTML file with `<style>`, when the tree is fresh, then the bytes of `raw_text` inside it show layer-1 colors
- [ ] Given the nested-region table, when the guard test runs, then every node kind in it exists in its grammar's node types
- [ ] Given a fence whose info string Pierre does not know, when the file paints, then it renders without a panic and the fence delimiters keep their layer-2 color

#### US-015: Color files between the tree-sitter cap and the editor cap
**Description:** As a user opening a generated or vendored file of a few megabytes, I want colors so that large files are not plain by default.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given a file above `highlight_cap` and at most `MAX_FILE_BYTES`, when it opens, then its first frame is plain
- [ ] Given that file, when it opens, then its layer-1 lex runs off the render thread
- [ ] Given that lex finished, when the next frame paints, then the visible rows show layer-1 colors
- [ ] Given a 10 MiB Rust file, when the bench measures it, then the retained layer-1 memory is recorded in this PRD
- [ ] Given the recorded memory above 8 bytes per input byte (assumption A4), when the story completes, then a named constant caps layer 1 at the largest size that stays at or under 8 bytes per input byte
- [ ] Given a lexer error on a large file, when it happens, then the file stays plain and editable
- [ ] Given that lexer error, when it happens, then one warning is logged

#### US-016: Measure the layers in the editor bench
**Description:** As a maintainer, I want the layered pipeline in the editor bench so that every claim in this PRD has a number from the suite.

**Priority:** P0
**Dependencies:** Blocked by US-010, US-012, US-013, US-014, US-015

**Acceptance Criteria:**
- [ ] Given the editor suite, when it runs, then it reports `open_to_first_color_300kb`
- [ ] Given the editor suite, when it runs, then it reports `lexical_full_300kb`, `lexical_keystroke`, `lexical_viewport_60_rows` and `lexical_memory_300kb`
- [ ] Given one run, when its results are read, then `open_to_first_color_300kb` is lower than `open_to_first_tree_300kb` of the same run
- [ ] Given the native figures, when they are compared with the wasmtime figures of R10, then the comparison is recorded in this PRD (assumption A2)
- [ ] Given a clean tree on the reference machine, when `scripts/bench-editor.sh --set-baseline` runs, then `bench/baselines/linux-x86_64/editor.json` holds the new metrics
- [ ] Given a build with layer 1 disabled by a test-only switch, when the suite runs, then `open_to_first_color_300kb` is within 10 % of `open_to_first_tree_300kb`, the A/B comparator's regression threshold (`bench/README.md`), which proves the metric measures layer 1
- [ ] Given `scripts/perf-ab.sh` between the commit before US-011 and this story's commit, when it completes, then no existing editor metric is reported as regressed
- [ ] Given a metric reported as regressed, when the story is handed to review, then the metric, its cause and Arthur's acceptance are recorded in this PRD, or the story stays `IN_PROGRESS`

#### US-017: Use layer 1 in the diff view for languages without a grammar
**Description:** As a user reviewing an agent's change to a Lua or Kotlin file, I want the diff colored so that reviews read like the editor.

**Priority:** P1
**Dependencies:** Blocked by US-007, US-008

**Acceptance Criteria:**
- [ ] Given a diff side whose extension has no grammar but a lexer language, when `side_key` is computed, then it is `Some`
- [ ] Given that side, when it is highlighted, then its runs come from layer 1
- [ ] Given a diff side whose extension has a grammar, when it is highlighted, then it uses layer 2 exactly as today
- [ ] Given a side larger than `highlight_cap`, when it is highlighted, then it stays plain as today
- [ ] Given a lexer error on a diff side, when it happens, then the side is plain and the diff still renders

---

### EP-004: The differential oracle

Layer 1's agreement with tree-sitter and the visible change at the layer swap are measured per language by deterministic tests.

**Definition of Done:** an agreement floor for each of the 19 languages with both engines is recorded and enforced by a test; the swap report covers the same 19 languages and is recorded in this PRD; the first-frame policy table resolves every lexer language.

#### US-018: Per-language agreement floors as a deterministic test
**Description:** As a maintainer, I want layer 1's agreement with tree-sitter enforced per language so that a Pierre update that degrades a language fails CI.

**Priority:** P1
**Dependencies:** Blocked by US-009, US-002

**Acceptance Criteria:**
- [ ] Given each `CORPUS` sample whose extension has a grammar, when the oracle pairs it, then the layer-1 language is `language_for_path` of the sample's file name, the same function the editor uses
- [ ] Given each of the 19 paired languages, when the oracle test runs, then it computes the byte agreement of displayed roles outside whitespace and outside nested-code regions
- [ ] Given `src-app/src/app/diff_dock/code/oracle-floors.toml`, when the test runs, then each language's agreement is at or above its recorded floor
- [ ] Given a language below its floor, when the test runs, then it fails naming the language, the floor, the measured value and the three roles with the most disagreeing bytes
- [ ] Given a paired language absent from the floors file, when the test runs, then it fails naming the language
- [ ] Given the test, when its code is read, then it measures no time

#### US-019: Measure the change at the layer swap and set the first-frame policy
**Description:** As Arthur, I want to know how many bytes change color when tree-sitter replaces the lexer so that I decide where the lexical first frame helps.

**Priority:** P1
**Dependencies:** Blocked by US-018, US-011

**Acceptance Criteria:**
- [ ] Given each of the 19 paired languages, when the swap report runs on the first 60 rows of its sample, then it prints the share of colored bytes whose role changes at the swap
- [ ] Given the report, when it completes, then its table is recorded in this PRD
- [ ] Given `FIRST_FRAME_POLICY` in `src-app/src/app/diff_dock/code/first_frame.rs`, a slice of (lexer language name, `Lexical` or `Plain`), when the editor opens a file whose language is `Plain`, then its first frame is plain
- [ ] Given a `Plain` language, when its file is edited or has nested regions, then layer 1 still colors the edited rows and the nested regions
- [ ] Given a language absent from the table, when its file opens, then the policy is `Lexical`
- [ ] Given the table, when this story completes, then every entry is `Lexical` unless Arthur chose otherwise from the report (Open Question 2)

---

### EP-005: A minimal LSP client

Paneflow can start a language server for a project root after consent, keep its open documents in sync, and stop it cleanly.

**Definition of Done:** `paneflow-lsp` passes its tests against the fake server on the Linux x86_64, Linux aarch64 and macOS CI targets (Windows in US-040); with `editor.semantic_tokens` false, no test path or UI path starts a server; on the reference machine with rust-analyzer installed, opening two Rust files of this repository after consent starts one rust-analyzer process, and closing both tabs stops it.

#### US-020: A byte-stream stdio child in paneflow-process
**Description:** As a developer of the LSP client, I want a child process with raw stdin and stdout so that I can frame messages by `Content-Length`. Its Windows path (`CREATE_NO_WINDOW`, Job Object) is written here and proven in US-040.

**Priority:** P1
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `spawn_stdio_session` on Unix, when it starts a child, then the child gets its own process group as in `spawn_line_session`
- [ ] Given the session, when it is read, then stdout is a byte reader with no newline framing
- [ ] Given the session, when it is written, then stdin is a byte writer with no newline framing
- [ ] Given the child's stderr, when it writes, then the last `STDERR_CAP` bytes (R16) are kept and readable for logs
- [ ] Given the session dropped, when the drop completes, then the child and its descendants are killed
- [ ] Given a command that does not exist, when it is spawned, then the error is a distinct not-found variant
- [ ] Given a child that exits at once, when its stdout is read, then the reader returns end of file
- [ ] Given that child, when its status is read, then the exit status is available

#### US-021: Content-Length JSON-RPC transport with bounded reads
**Description:** As a developer of the LSP client, I want a transport that frames, routes and bounds messages so that a broken server cannot hang or exhaust the app.

**Priority:** P1
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given a message, when it is written, then it is `Content-Length: <bytes>\r\n\r\n` followed by the UTF-8 JSON body
- [ ] Given a stream split at arbitrary byte boundaries, when it is read, then every message is decoded once and in order
- [ ] Given a response, when it arrives, then it completes the request with the same id
- [ ] Given a response with an unknown id, when it arrives, then it is logged and dropped
- [ ] Given the server requests `workspace/configuration`, `window/workDoneProgress/create` and `client/registerCapability`, when they arrive, then the client answers each with a result
- [ ] Given any other server request, when it arrives, then the client answers error `-32601`
- [ ] Given `window/logMessage`, when it arrives, then it is logged at debug level with the server id
- [ ] Given a header without `Content-Length`, when it is read, then the connection is closed
- [ ] Given that closed connection, when its end is reported to the lifecycle, then the cause is a protocol error
- [ ] Given a `Content-Length` above 64 MiB (assumption A8), when it is read, then the connection is closed before the body is allocated
- [ ] Given a body that is not valid JSON, when it is read, then it is logged and skipped
- [ ] Given that invalid body, when the next valid message arrives, then it is decoded on the same connection
- [ ] Given the transport tests, when they run, then they use in-memory pipes and measure no time

#### US-022: Server lifecycle: initialize, shutdown, and crashes
**Description:** As a user, I want one server per language server and root that starts when needed and stops when unused so that servers neither pile up nor linger.

**Priority:** P1
**Dependencies:** Blocked by US-020, US-021

**Acceptance Criteria:**
- [ ] Given a start for (server id, root), when it runs, then `initialize` carries `rootUri` and one workspace folder for the root
- [ ] Given that `initialize`, when it is sent, then its capabilities carry the semantic token fields of R12 with `formats: ["relative"]`, `augmentsSyntaxTokens: true`, `multilineTokenSupport: false`, `overlappingTokenSupport: false` and `refreshSupport: true`
- [ ] Given that `initialize`, when it is sent, then `general.positionEncodings` is `["utf-8", "utf-16"]`
- [ ] Given the `initialize` result, when it arrives, then the client records the negotiated position encoding and the token legend
- [ ] Given that result recorded, when the client continues, then it sends `initialized` before any document message
- [ ] Given a second document of the same (server id, root), when it opens, then no second process starts
- [ ] Given the last document of a (server id, root) closed, when the close completes, then the client sends `shutdown` followed by `exit`
- [ ] Given a server still alive 5 s after `exit` (assumption A7), when the timeout passes, then the client kills it
- [ ] Given Paneflow quitting, when servers are running, then each receives `shutdown` and `exit` off the render thread
- [ ] Given an unexpected exit or a protocol error, when it is the first for that key since it started, then the server restarts once and its open documents are opened again
- [ ] Given a second unexpected exit or protocol error, when it happens, then the server stays stopped in the `Crashed` state with its last stderr lines kept
- [ ] Given an `initialize` error response, when it arrives, then the server is stopped in the `Failed` state with the error message kept
- [ ] Given the lifecycle tests, when they run, then they drive a fake server that is the test binary re-executed with an environment variable

#### US-023: Document sync from editor edits
**Description:** As a user editing a file, I want the server to see my unsaved text so that its tokens match what I see.

**Priority:** P1
**Dependencies:** Blocked by US-022, US-024

**Acceptance Criteria:**
- [ ] Given a document opened for a running server, when it opens, then `didOpen` sends its URI, the catalog's `languageId`, version 1 and the full text
- [ ] Given an edit batch, when it is applied, then one `didChange` sends the incremental ranges and texts in the negotiated encoding
- [ ] Given that `didChange`, when it is sent, then its version is the previous version plus one
- [ ] Given a server whose `textDocumentSync.change` is Full, when an edit is applied, then `didChange` sends the full text
- [ ] Given a server that offers no sync, when a document opens, then no `didOpen` is sent and the document is marked unsupported
- [ ] Given the negotiated encoding `utf-16` and a line with a character outside the BMP, when an edit after it is sent, then the column counts that character as two units
- [ ] Given the negotiated encoding `utf-8`, when an edit is sent, then columns are byte offsets in the line
- [ ] Given "Reload from disk", when the document is replaced, then one full-text `didChange` is sent
- [ ] Given a tab closed, when the close completes, then `didClose` is sent for its URI
- [ ] Given a document outside its workspace's Files dock root (TD-07), when it opens, then no server is involved
- [ ] Given rust-analyzer on `PATH`, when an integration test initializes it, then the negotiated encoding is recorded in this PRD (assumption A6)
- [ ] Given rust-analyzer absent, when that integration test runs, then it is skipped with a message naming the binary

#### US-024: Language server settings and the built-in catalog
**Description:** As a user, I want semantic highlighting and its servers declared in my own settings so that nothing runs unless I ask.

**Priority:** P1
**Dependencies:** Blocked by US-022

**Acceptance Criteria:**
- [ ] Given `EditorDisplayConfig`, when it is read, then it has `semantic_tokens: Option<bool>` resolved by `semantic_tokens_enabled()` to `false` by default
- [ ] Given a new top-level `language_servers` section, when it is parsed, then each entry has `command`, `args`, `extensions`, `language_id`, `enabled`, `initialization_options` and `settings`
- [ ] Given no user entry, when the catalog resolves, then it contains `rust-analyzer` for `rs` with `languageId` `rust`
- [ ] Given no user entry, when the catalog resolves, then it contains `zls` for `zig` with `languageId` `zig`
- [ ] Given a user entry with the id of a built-in, when the catalog resolves, then the user's fields replace the built-in's
- [ ] Given `settings` in an entry, when the server sends `workspace/configuration`, then the client answers with those values
- [ ] Given a malformed `language_servers` entry, when the config loads, then that entry is dropped with a log line
- [ ] Given that malformed entry, when the config loads, then the other entries still load
- [ ] Given `editor.semantic_tokens` false, when a Rust file opens, then no language server process starts

#### US-025: Resolve server commands, and the Missing state
**Description:** As a user, I want servers found only where I put them so that a repository can never choose what Paneflow runs. The Windows `PATHEXT` resolution is written here and proven in US-040.

**Priority:** P1
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [ ] Given a `command` without a path separator, when it resolves, then it is searched on `PATH` with `which`
- [ ] Given a `command` with an absolute path, when it resolves, then that path is used
- [ ] Given a `command` with a relative path, when it resolves, then it is rejected with a log line
- [ ] Given a command that does not resolve, when a matching file opens, then the server enters the `Missing` state and no process starts
- [ ] Given a `Missing` server, when the user's `PATH` gains the binary and Paneflow restarts, then the next matching file starts it

#### US-026: The Settings controls and their documentation
**Description:** As a user, I want semantic highlighting and its permissions in the Settings page and the docs so that I can find them without editing JSON.

**Priority:** P1
**Dependencies:** Blocked by US-024, US-027, US-031

**Acceptance Criteria:**
- [ ] Given the Settings editor tab, when it renders, then a "Semantic highlighting" switch built on `kit::switch` shows the value of `editor.semantic_tokens`
- [ ] Given the switch clicked, when the click completes, then `persist_setting` writes the new value to `paneflow.json`
- [ ] Given the switch turned off while servers run, when it is applied, then layer-3 colors disappear at the next paint
- [ ] Given the schema tests, when they run, then `schemas/paneflow.schema.json` describes `editor.semantic_tokens`
- [ ] Given the schema tests, when they run, then `schemas/paneflow.schema.json` describes `language_servers`
- [ ] Given the schema tests, when they run, then `docs/user/configuration/schema.md` describes `language_servers`
- [ ] Given `docs/user/configuration/schema.md`, when it is read, then it shows a gopls entry with `semanticTokens: true` in `settings` (R14) as an example of a user entry
- [ ] Given `paneflow.json` unwritable, when the switch is clicked, then the error is logged and the switch shows the value actually stored
- [ ] Given the Settings editor tab, when "Forget language server permissions" is clicked, then the trust file of US-027 is emptied
- [ ] Given that click, when servers are running, then they keep running until their last tab closes

#### US-027: Ask once per server and root before the first start
**Description:** As a user who opens repositories cloned by agents, I want Paneflow to ask before a language server runs a project for the first time so that opening a file never executes unknown code silently.

**Priority:** P1
**Dependencies:** Blocked by US-022, US-025

**Acceptance Criteria:**
- [ ] Given semantic tokens on, a resolved server and a root with no recorded answer, when a matching file opens, then the editor shows a banner naming the server and the root
- [ ] Given that banner, when it is read, then it states that the server can run the project's build scripts
- [ ] Given that banner, when it renders, then it offers "Start" and "Not now"
- [ ] Given "Start", when it is clicked, then the canonical root and server id are stored in `~/.paneflow/language-server-trust.json`
- [ ] Given "Start", when the answer is stored, then the server starts
- [ ] Given a stored answer, when another file of that root opens after a Paneflow restart, then the server starts with no banner
- [ ] Given "Not now", when it is clicked, then no server starts for that pair
- [ ] Given "Not now", when another file of that root opens before a restart, then no banner appears
- [ ] Given a symlinked path to an allowed root, when a file opens through it, then it matches the stored canonical root
- [ ] Given a trust file that cannot be parsed, when it is read, then it is treated as empty and the banner appears
- [ ] Given that unparseable file, when no "Start" has been clicked, then the file is left unchanged on disk
- [ ] Given `crates/paneflow-home`, when `language_server_trust_path()` is called, then it returns the file under the Paneflow home, covered by a test

---

### EP-006: Semantic tokens as layer 3

Tokens from the server color the editor over layers 1 and 2, stay aligned through edits, and never leave wrong colors on screen.

**Definition of Done:** the decoding, mapping and overlay tests pass against the fake server on all four CI targets; on the reference machine with rust-analyzer, a struct field access and a local variable in `src-app/src/app/diff_dock/code/highlight.rs` show different roles; the `Missing`, `Crashed` and `Failed` banners each appear in a test driven by the fake server.

#### US-028: Schedule semantic token requests
**Description:** As a user, I want tokens fetched for the files I open and kept current while I type so that layer 3 follows the code on screen.

**Priority:** P1
**Dependencies:** Blocked by US-023

**Acceptance Criteria:**
- [ ] Given a document opened on a server with `full`, when `didOpen` is sent, then a `full` request follows
- [ ] Given a server with only `range`, when the visible rows change, then a `range` request covers the visible rows
- [ ] Given edits and no request in flight, when they are applied on a server with delta support, then one `full/delta` request with the last `resultId` is sent
- [ ] Given edits and no request in flight, when they are applied on a server without delta support, then one `full` request is sent
- [ ] Given edits while a request is in flight, when its response arrives, then exactly one follow-up request is sent (assumption A9)
- [ ] Given a response for a document version older than the current one, when it arrives, then it is not delivered to the document's layer-3 token store
- [ ] Given `workspace/semanticTokens/refresh`, when it arrives, then each open document of that server is requested again
- [ ] Given a document closed with a request in flight, when the close completes, then `$/cancelRequest` is sent for it
- [ ] Given a request answered with an error, when the error arrives, then the previous tokens stay and the next edit sends a new request

#### US-029: Decode semantic tokens and deltas
**Description:** As a developer of layer 3, I want server responses turned into byte ranges per line so that the editor paints them without knowing the wire format.

**Priority:** P1
**Dependencies:** Blocked by US-028

**Acceptance Criteria:**
- [ ] Given the relative encoding, when a response is decoded, then each token becomes an absolute line, a byte range in that line, a type and a modifier set
- [ ] Given the negotiated encoding `utf-16`, when a token follows a character outside the BMP on its line, then its byte range starts after that character's four UTF-8 bytes
- [ ] Given a token past the end of its line, when it is decoded, then that token is dropped and the others kept
- [ ] Given a type index outside the legend, when it is decoded, then that token is dropped and the others kept
- [ ] Given a delta response, when it is applied, then its edits are applied to the previous array from the last edit to the first
- [ ] Given a delta whose edit starts beyond the previous array, when it is applied, then the cache is discarded and a `full` request is sent
- [ ] Given zls on `PATH`, when an integration test opens `sample.zig`, then it records in this PRD whether zls returned tokens (assumption A5)
- [ ] Given that test, when it initializes zls, then it records the encoding zls accepted
- [ ] Given zls absent, when that test runs, then it is skipped with a message naming the binary

#### US-030: Map token types and modifiers to palette roles
**Description:** As a user, I want server tokens drawn in the theme's colors so that layer 3 looks like the rest of the editor.

**Priority:** P1
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given each LSP 3.17 standard token type, when it is mapped, then it gives the capture name of TD-09
- [ ] Given the rust-analyzer types of TD-09, when they are mapped, then they give the listed names
- [ ] Given a custom type not in the table, when it is mapped, then it is transparent
- [ ] Given `defaultLibrary` on a `function`, `type` or `variable` token, when it is mapped, then `.builtin` is appended
- [ ] Given `documentation` on a `comment` token, when it is mapped, then the name is `comment.doc`
- [ ] Given every non-transparent name, when it is resolved through `DiffSyntax::color_for_capture`, then it gets a color
- [ ] Given a legend that repeats a type name, when it is mapped, then each index maps independently and the mapping never panics

#### US-031: Overlay layer 3 in the editor without wrong colors
**Description:** As a user, I want semantic colors over the syntax colors with no flicker and no stale tokens so that layer 3 only ever adds information.

**Priority:** P1
**Dependencies:** Blocked by US-029, US-030, US-011

**Acceptance Criteria:**
- [ ] Given a row with layer-3 tokens, when it paints, then each byte covered by a mapped token shows the token's color
- [ ] Given that row, when it paints, then every byte not covered by a mapped token shows layer 2 or layer 1 per TD-05
- [ ] Given an edit, when the next frame paints, then tokens on unedited rows are shifted by the edit
- [ ] Given an edit, when the next frame paints, then no token is painted on an edited row
- [ ] Given a new response, when the next frame paints, then it shows only that response's tokens and none of the previous response's
- [ ] Given the theme switched, when rows repaint, then layer-3 colors follow the new palette with no new request
- [ ] Given the server stopped or crashed, when the next frame paints, then no layer-3 color remains
- [ ] Given `editor.semantic_tokens` false, when a file is edited, then no layer-3 storage is allocated for it

#### US-032: Report server state in the editor
**Description:** As a user who turned semantic highlighting on, I want to know when a server is missing or has crashed so that I can fix it instead of guessing.

**Priority:** P1
**Dependencies:** Blocked by US-022, US-025, US-027

**Acceptance Criteria:**
- [ ] Given a server in the `Missing` state, when a matching file opens, then the banner says the server was not found and names the command
- [ ] Given that banner shown once for a server and root, when another file of that root opens in the same session, then it does not appear again
- [ ] Given a server in the `Crashed` state, when a file of its root is shown, then the banner says the server stopped
- [ ] Given that banner, when it renders, then it shows the server's last stderr line
- [ ] Given that banner, when it renders, then it offers "Restart"
- [ ] Given "Restart", when it is clicked, then the server starts again and reopens its documents
- [ ] Given the server running again, when the next frame paints, then the banner is gone
- [ ] Given a server in the `Failed` state, when a file of its root is shown, then the banner shows the `initialize` error message
- [ ] Given a server that offers no semantic tokens, when its file is shown, then no banner appears and one debug line is logged

---

### EP-007: Real-hardware validation of layers 1 and 2 on macOS and Linux

What CI cannot show for the lexical and tree-sitter layers, colors on screen and the Apple build, is checked on macOS and Linux. Windows is EP-009.

**Definition of Done:** each story below has its report written as a section of this PRD; every defect found is fixed or recorded as a blocker on the story it belongs to.

#### US-034: Validate layers 1 and 2 on macOS Apple Silicon
**Description:** As the maintainer, I want the lexical and tree-sitter layers run on Apple Silicon so that the C11 thread-local build works there.

**Priority:** P0
**Dependencies:** Blocked by US-006, US-013, US-014, US-016

**Acceptance Criteria:**
- [ ] Given a signed release build, when a Zig file opens, then its first frame is colored, with a screenshot in the report
- [ ] Given the same build, when a Kotlin file opens, then its first frame is colored, with a screenshot
- [ ] Given the same build, when a Markdown file with a Rust fence opens, then the fence body is colored after the tree arrives, with a screenshot
- [ ] Given two documents lexed at the same time in two tabs, when the report is written, then it states that their colors match a single-tab open
- [ ] Given the signed build, when `codesign --verify --deep --strict` runs on the app, then the report records that it passes with the unchanged entitlements
- [ ] Given a file whose extension Pierre does not know, when it opens, then the report states that it is plain and editable

#### US-035: Visual pass of layers 1 and 2 on Linux, Wayland and X11
**Description:** As the maintainer, I want the swap from layer 1 to layer 2 and the large-file delay watched on Linux so that they are judged by eye, not only by numbers.

**Priority:** P0
**Dependencies:** Blocked by US-015, US-016

**Acceptance Criteria:**
- [ ] Given a Rust file just under the 2 MB tree-sitter cap, the largest file where the swap happens, under Wayland, when it opens, then the report states whether the swap is visible and how
- [ ] Given the same file under X11, when it opens, then the report states the same
- [ ] Given a Rust file between 2 MB and 10 MiB, when it opens, then the report states how long it stays plain before layer 1 colors it
- [ ] Given a file whose extension Pierre does not know, when it opens, then the report states that it is plain and editable

---

### EP-008: Real-hardware validation of the language servers on macOS and Linux

What CI cannot show for layer 3, real servers on real machines, is checked on macOS and Linux. Windows is EP-009.

**Definition of Done:** each story below has its report written as a section of this PRD; every defect found is fixed or recorded as a blocker on the story it belongs to.

#### US-037: Validate language servers on macOS Apple Silicon
**Description:** As the maintainer, I want rust-analyzer and zls run from Paneflow on macOS so that spawning and the lifecycle are facts there.

**Priority:** P1
**Dependencies:** Blocked by US-031, US-032

**Acceptance Criteria:**
- [ ] Given rust-analyzer from rustup and zls from Homebrew, semantic tokens on and consent given, when their files open, then both processes start
- [ ] Given both running, when their files paint, then layer-3 colors appear, with a screenshot in the report
- [ ] Given both servers, when their last tabs close, then the report states that both processes are gone
- [ ] Given a `language_servers` entry whose command is not installed, when its file opens, then the missing-server banner appears

#### US-038: Visual pass of layer 3 on Linux
**Description:** As the maintainer, I want the semantic overlay watched on this repository so that wrong or stale tokens would be seen.

**Priority:** P1
**Dependencies:** Blocked by US-031, US-032

**Acceptance Criteria:**
- [ ] Given rust-analyzer on this repository, when `src-app/src/app/diff_dock/code/highlight.rs` opens, then a struct field access and a local variable show different colors, with a screenshot
- [ ] Given fast typing in that file, when the report is written, then it states whether any token was seen on the wrong bytes
- [ ] Given the server killed while the file is open, when the next frame paints, then the report states that no layer-3 color remains

---

### EP-009: Windows validation on the Windows 11 dual boot

Every proof that only Windows can give, for every epic of this PRD, gathered in one pass on Arthur's Windows 11 dual boot (x64), after a push and a reboot. The Windows code ships with each story; this epic runs it.

**Definition of Done:** each story below has its report written as a section of this PRD, with the commit it ran on; every defect found is fixed or recorded as a blocker on the story it belongs to.

#### US-039: Windows proof of the generators and the native lexer build
**Description:** As the maintainer, I want the MSVC build of `paneflow-highlights` and both PowerShell generators run on Windows so that A3 and A12 are facts.

**Priority:** P0
**Dependencies:** Blocked by US-005, US-006, US-001

**Acceptance Criteria:**
- [ ] Given the `Windows x86_64 libghostty check` job on the commit that adds the crate or a later one, when it runs, then the crate's tests pass
- [ ] Given that job's run, when its duration is read, then it finished inside its 50-minute timeout, and the report records it (assumption A3)
- [ ] Given that job's log, when the report is written, then it records the compile time of `highlights.c` under MSVC (assumption A3)
- [ ] Given `cargo nextest run -p paneflow-highlights --locked` on the dual boot, when it runs, then every test passes
- [ ] Given `scripts/sync-pierre-highlights.ps1` under PowerShell 7 on the dual boot, when it runs without `-Check`, then `git status` shows no change under `crates/paneflow-highlights/` (assumption A12)
- [ ] Given the same script with `-Check`, when it runs, then it exits 0 and prints the package version
- [ ] Given `ZED_DIR` and `ZED_ZIG_DIR` checkouts on the dual boot, when `scripts/sync-zed-queries.ps1 -Check` runs, then it exits 0 with the message of US-001 (EP-001 ran it only in a Linux container)
- [ ] Given `scripts/bench-editor.ps1 -LexerPrototype` on the dual boot, when it runs, then it exits non-zero with the message of US-010

#### US-040: Windows behaviors of the LSP client
**Description:** As the maintainer, I want the Windows paths of the process and command-resolution code run on Windows so that a language server starts there the way it does on Unix.

**Priority:** P1
**Dependencies:** Blocked by US-020, US-025

**Acceptance Criteria:**
- [ ] Given the `Windows x86_64 libghostty check` job on a commit after US-020 and US-025, when it runs, then `paneflow-lsp` and `paneflow-process` pass their tests against the fake server
- [ ] Given `spawn_stdio_session` on Windows, when it starts a child, then it applies `CREATE_NO_WINDOW` and the Job Object setup of `spawn_line_session`
- [ ] Given that session dropped on Windows, when the drop completes, then the child and its descendants are gone from Task Manager
- [ ] Given Windows, when a `command` without a path separator resolves, then `PATHEXT` extensions are tried

#### US-033: Validate layers 1 and 2 on Windows 11
**Description:** As the maintainer, I want the lexical and tree-sitter layers run on Arthur's Windows 11 dual boot so that the MSVC build is a fact.

**Priority:** P0
**Dependencies:** Blocked by US-039, US-013, US-014, US-016

**Acceptance Criteria:**
- [ ] Given a release build from the story's commit, when a Zig file opens, then its first frame is colored, with a screenshot in the report
- [ ] Given the same build, when a Kotlin file opens, then its first frame is colored, with a screenshot
- [ ] Given the same build, when a Markdown file with a Rust fence opens, then the fence body is colored after the tree arrives, with a screenshot
- [ ] Given a file of 10,001-character lines, when it opens, then the report states that it opened read-only and colored
- [ ] Given that file, when it is scrolled to its end, then the report states that Paneflow stayed responsive and did not crash

#### US-036: Validate language servers on Windows 11
**Description:** As the maintainer, I want rust-analyzer and zls run from Paneflow on Windows so that spawning, consent and crash handling are facts there.

**Priority:** P1
**Dependencies:** Blocked by US-040, US-031, US-032

**Acceptance Criteria:**
- [ ] Given rust-analyzer installed as an `.exe`, semantic tokens on and consent given, when a Rust file opens, then its process starts
- [ ] Given that process running, when the file paints, then layer-3 colors appear, with a screenshot in the report
- [ ] Given that server, when the last Rust tab closes, then the report states that the process is gone
- [ ] Given zls installed as an `.exe`, when a Zig file opens with consent, then the report records whether its process started (assumption A5)
- [ ] Given zls running, when the Zig file paints, then the report records whether layer-3 colors appeared
- [ ] Given a server installed by npm as a `.cmd` shim and declared in `language_servers`, when its file opens, then the report states whether it started (assumption A10)
- [ ] Given a server killed from Task Manager twice, when it dies the second time, then the crash banner appears

## Edge Cases & Error States

| Scenario | Trigger | Expected behavior | User message |
|----------|---------|-------------------|--------------|
| Lexer trap | A lexer fault in the generated code | Layer 1 off for that document; layer 2 unaffected | None; one warning in the log |
| Lexer memory ceiling | Staging that would pass `MAX_MEMORY_BYTES` | Staging refused; the document is plain where layer 2 does not cover it | None; one warning |
| Unknown language | Extension and file name unknown to both engines | Plain and editable, as today | None |
| CRLF files | Lines ending in `\r\n` | Lines split on `\n` only, as ropey and `str::lines` already do (`src-app/Cargo.toml:250-255`) | None |
| Giant line | A line over 10,000 characters | Read-only as today (`load.rs:12`), still colored | Existing read-only banner |
| Server missing | Catalog command not resolved | No process; `Missing` state (US-025) | Banner naming the command (US-032) |
| Server crash or protocol error | First occurrence | One automatic restart (US-022) | None |
| Repeated crash or protocol error | Second occurrence, including an oversized message | `Crashed` state | Banner with "Restart" (US-032) |
| Late tokens | Response for a version older than the document | Not painted; the follow-up request sent for those edits replaces it (US-028) | None |
| Malformed tokens | Out-of-range index or position | Token dropped | None |
| No consent | First file of a new root | No process | Consent banner (US-027) |
| Corrupt trust file | Unparseable JSON | Treated as empty; asked again | Consent banner |
| File outside root | Opened through a path outside the Files dock root | No server | None |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| MSVC chokes on the 5.6 MB generated file (time, memory, `_Thread_local`) | Medium | Windows build blocked | The existing Windows CI job builds it on the first commit of US-006, proven in US-039; fallbacks are `/O1` for that file or dropping `/std:c11` so `wasm-rt.h` picks `__declspec(thread)` (R8) |
| Pierre ships often and shifts token ids | High (15 releases in 7 weeks, ids moved in 0.5.2, R9) | Wrong colors after a bump | Pinned inputs, tables generated in the same run (US-004, US-008), oracle floors (US-018) |
| The public Pierre source lags npm | Certain today | No source for 0.5.2 | Pin the npm artifact by hash; record the repository; fixes go upstream (TD-03) |
| A language server runs a malicious repository's build scripts | Medium | Code execution as the user | Off by default, consent per server and root, never from project files, no downloads (TD-10, US-024, US-025, US-027) |
| Visible color change at the layer swap | Medium | Distracting flicker | Swap report and per-language first-frame policy (US-019), visual pass (US-035) |
| Layer 3 and layer 2 disagree on a construct | Medium | Colors jump when tokens land | Mapping aligned with the Zed Rust query, transparent fallback, `augmentsSyntaxTokens` (TD-08, TD-09) |
| A `longjmp` crosses Rust frames | Low | Undefined behavior | `setjmp` in every shim entry, host imports in Rust that never re-enter the shim (US-007) |

## Open Questions

1. **wasm2c instead of a hand rewrite (TD-02).** It changes the letter of the 2026-11-19 decision. If Arthur prefers the rewrite, EP-002 becomes per-language Rust lexers and EP-004 their acceptance gate.
2. **First-frame policy threshold (US-019).** No evidence gives a share of changed bytes above which a lexical first frame hurts more than it helps; Arthur sets it from the US-019 report and the US-035 visual pass.
3. **Consent banner (TD-10).** It is a narrow trust prompt in a product that cut per-directory trust; Arthur may prefer a single global opt-in.
4. **Built-in servers beyond rust-analyzer and zls.** gopls needs `semanticTokens` turned on in its settings (R14); clangd and TypeScript servers are user entries until Arthur asks for them as built-ins.

## Files NOT to Modify

- `src-app/src/diff/queries/*/highlights.scm`: vendored bytes, changed only by `scripts/sync-zed-queries.sh` or `.ps1`
- `crates/paneflow-highlights/c/generated/**`, `c/wasm2c/**`, `c/simde/**`: generated or vendored, changed only by `scripts/sync-pierre-highlights.sh` or `.ps1`
- `crates/paneflow-highlights/src/tables.rs`: generated by the same scripts
- `vendor/**`: GPUI, cosmic-text and portable-pty copies with their own update rules
- `native/libghostty/**`: the terminal engine pins
- `packaging/macos/*.entitlements`: layer 1 needs no entitlement (R17)
- `bench/baselines/**`: written only by `--set-baseline` from a clean tree
- `crates/paneflow-shim/**`, `crates/paneflow-ai-hook/**`, `crates/paneflow-mcp/**`: size-capped helpers, which must not link the new crates
[/PRD]
