[PRD]
# PRD: A browser inside Paneflow that an agent can drive

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-05 | Arthur Jean | Initial PRD (French): CEF and GPU qualification, native browser in the dock, distribution on four targets, agent interactions as the last delivery. 32 stories, none started. |
| 2.0 | 2026-11-24 | Arthur Jean | Full rewrite from the research of 2026-11-24 (`tasks/research/agent-browser-architecture-2026-11-24.md`, local evidence) and a reading of Zeron and t3code. Same engine choice (CEF in its own host process, off-screen rendering), four changes: agent control is proven before the GPU path, an explicit CPU presentation path exists, the engine ships as a separately signed runtime, and Windows is recognised as the most expensive GPUI integration. The GPUI pin cited by v1.0 (`fecc3273`) is replaced by the current one (`c016f436`; the research read `703b5ac3`, and R1 to R3 were re-read at `c016f436` after `main` moved the same day). Every macOS test and validation story sits in a final epic, EP-008; every Windows test and validation story sits in a final epic after it, EP-009. 9 epics, 57 stories. Version 1.0 is in git history. |

## Problem Statement

1. A coding agent running in a Paneflow pane cannot check the web application it is building. It can read other panes through `list_panes`, `read_pane` and `search_pane` (`crates/paneflow-mcp/src/tools.rs`), but it has no way to load a page, read its console, see its network failures or click through it. The developer does that check by hand in another application and reports back to the agent.
2. Links and detected ports open in the system browser (`src-app/src/external_open.rs`). The page is detached from the workspace, the terminal and the agent session that started the server.
3. Agents that bring their own browser tool each start a separate Chrome with its own profile and often a debugging port on localhost. Chrome DevTools Protocol (CDP, Chrome's remote-control protocol) has no authentication, so any local process that reaches such a port can drive that browser. The user cannot see what those browsers do.
4. The 2026-09-05 plan never started: 32 of 32 stories are TODO, there is no qualification evidence, and it put agent tools last. It also cited a GPUI revision the tree no longer uses.

**Why now:** the control plane a browser needs has landed since v1.0: one method registry with one authorization function (`crates/paneflow-api/src/descriptor.rs`, `authorize.rs`), per-pane grants (`grants.rs`), kernel-verified caller identity (`crates/paneflow-peer`) and an approval broker. On the engine side, the Rust binding `cef` 156.7.0 was published on 2026-11-19 with an `accelerated_osr` feature that imports shared textures into wgpu on Linux, macOS and Windows, and CEF's Linux shared-texture path received its NVIDIA fix on 2026-10-09. Comparable products (Cursor, Codex, Claude Desktop, VS Code, t3code; R18, R19) all shipped an agent-drivable browser during 2026.

## Overview

Paneflow gains a browser surface, available as a pane and as a tab of the right dock, whose page is rendered by Chromium and can be driven by an agent through Paneflow's own API.

The engine is Chromium through CEF (Chromium Embedded Framework), running in a dedicated host process that Paneflow starts when the first browser tab opens. Paneflow never links Chromium. The window and the host exchange a private protocol: commands, events and frames. This mirrors the terminal boundary (`TerminalSessionBackend`): engine types stay inside the host.

The host renders off screen. Frames reach the window in one of two ways:

- **GPU path:** the host copies each CEF frame into a texture it owns and shares it with the window (dmabuf on Linux, IOSurface on macOS, a D3D11 shared handle on Windows). The window's renderer samples it directly.
- **CPU path:** the host writes BGRA pixels into shared memory and the window uploads them through `paint_image`, the path the terminal already uses for Kitty images. This path is built first, works on every platform, and stays as the visible, labelled fallback when the GPU path is unavailable.

Agent control never leaves the host as raw CDP. The host calls CDP internally through CEF (`ExecuteDevToolsMethod`, `AddDevToolsMessageObserver`), which needs no debugging port. Agents get a family of `browser.*` methods declared in the existing registry, gated by a new per-pane `browser` right, exposed through `paneflow-mcp` and the `paneflow` CLI. Page content returns to the agent fenced as untrusted text, exactly as terminal output is.

Each workspace has its own clean browser profile. Loopback origins are reachable without a prompt; every other origin needs a per-site decision from the user.

The engine ships as a separate runtime archive, signed with the release minisign key, downloaded the first time the user opens a browser tab, and updated independently of the application.

The work is ordered so that the riskiest beliefs die early and the user-visible value does not wait for the hardest part:

1. EP-001 runs three kill tests on Linux and records a GO or NO-GO per path.
2. EP-002 and EP-003 deliver agent control with no pixels at all.
3. EP-004 adds the visible surface on the CPU path.
4. EP-005 adds the GPU path on Linux.
5. EP-006 delivers the runtime.
6. EP-007 writes the macOS and Windows code paths. EP-008 proves macOS on hardware. EP-009 proves Windows on hardware.

## Research Findings

Sources are primary pages read on 2026-11-24 unless marked. The full notes are in `tasks/research/agent-browser-architecture-2026-11-24.md` (local evidence, not tracked).

| # | Finding | Source |
|---|---------|--------|
| R1 | At the pinned GPUI revision `c016f436`, `paint_surface` is compiled only for macOS and takes a `CVPixelBuffer`. `PaintSurface` exists on every platform but carries its image buffer only on macOS. | `gpui/src/scene.rs:1169-1175`, `gpui/src/window.rs:5675-5676` at the pinned checkout |
| R2 | The vendored Linux renderer ignores surface primitives, and the Windows DirectX renderer draws none. | `vendor/gpui_wgpu/src/wgpu_renderer.rs:1883`, `gpui_windows/src/directx_renderer.rs:845-850` |
| R3 | `paint_image` works on every platform and resolves a `RenderImage` through the platform atlas by `AtlasKey::Image(RenderImageParams)`. The atlas on Linux is vendored. | `gpui/src/window.rs:5574`, `gpui/src/platform.rs:1843-1900`, `vendor/gpui_wgpu/src/wgpu_atlas.rs` |
| R4 | `gpui_wgpu` and `gpui_linux` are vendored and patched in from the root manifest. `gpui_windows` is not. | `Cargo.toml:132-134` |
| R5 | Panes and dock tabs are closed enums: `PaneSurface { Terminal, Markdown }`, `DiffDockTab { Changes, Terminal, File, PendingFile }`. Opening the dock is tied to the Git snapshot. | `src-app/src/pane.rs:29`, `src-app/src/app/diff_dock/model.rs:23`, `src-app/src/app/diff_dock/mod.rs:46` |
| R6 | Every API method is one `Descriptor`; one pure `authorize` function decides; decisions are pinned in `crates/paneflow-api/tests/decision_table.txt`; `protocol/api-v1.json` is generated. | `crates/paneflow-api/src/descriptor.rs`, `authorize.rs:105-131` |
| R7 | Untrusted output is fenced with a random fence id and a neutralised closing sentinel. | `crates/paneflow-mcp/src/output.rs:12-33` |
| R8 | Release artifacts of v0.20.0 weigh 21 to 40 MiB. The updater replaces a whole artifact, capped at 500 MiB, and has no path for a separately downloaded component. | `gh release view v0.20.0`, `src-app/src/update/linux/targz.rs:7`, `macos/dmg.rs:20`, `windows/msi.rs:34` |
| R9 | CEF `OnAcceleratedPaint` delivers a D3D11 handle on Windows, an IOSurface on macOS and dmabuf planes on Linux. Handles are pooled and valid only during the callback. | [cef_render_handler.h](https://raw.githubusercontent.com/chromiumembedded/cef/master/include/cef_render_handler.h), [cef_types_linux.h](https://raw.githubusercontent.com/chromiumembedded/cef/master/include/internal/cef_types_linux.h) |
| R10 | On Linux, shared textures work under the X11 and Wayland display backends and not under the headless backend. An NVIDIA rendering fault was fixed on 2026-10-09; a second NVIDIA fault (frames read before they are complete) has a proposed fix that needs kernel 6.0 or later (not confirmed in a release). | CEF issues [#4356](https://github.com/chromiumembedded/cef/issues/4356), [#4237](https://github.com/chromiumembedded/cef/issues/4237), [#4313](https://github.com/chromiumembedded/cef/issues/4313) |
| R11 | CEF's DevTools methods "do not require an active DevTools front-end or remote-debugging session". `ExecuteDevToolsMethod` must be called on the CEF UI thread. | [cef_browser.h](https://raw.githubusercontent.com/chromiumembedded/cef/master/include/cef_browser.h) |
| R12 | The `cef` crate 156.7.0 (2026-11-19) has an `accelerated_osr` feature importing dmabuf through Vulkan, D3D11 shared handles and IOSurface into wgpu, and a `sandbox` feature on by default. | [docs.rs/crate/cef](https://docs.rs/crate/cef/latest/features), [tauri-apps/cef-rs](https://github.com/tauri-apps/cef-rs) |
| R13 | Tauri's CEF runtime (3.0 alpha) reports bundles "hundreds of megabytes larger", a first build that downloads about 1 GB, Windows running unsandboxed today, and a Linux sandbox that needs unprivileged user namespaces or a setuid helper that an AppImage cannot ship. | [v3.tauri.app/develop/cef](https://v3.tauri.app/develop/cef/) |
| R14 | CEF's sandbox since M138 needs the Windows client built as a DLL started through `bootstrap.exe`, and macOS helpers that load `libcef_sandbox.dylib` first. | [CEF sandbox setup](https://chromiumembedded.github.io/cef/sandbox_setup) |
| R15 | OpenAI Atlas runs Chromium's browser process outside the application and sends agent input directly to the renderer. Its pixel path uses a private macOS API. | [OpenAI, 2025-10-30](https://openai.com/index/building-chatgpt-atlas/) |
| R16 | System webviews do not give uniform control: WKWebView exposes no inspector protocol to its host, a WebView2 can only be composed through DirectComposition visuals, and a child webview on Wayland needs a GTK window. | [WebKit blog](https://webkit.org/blog/13936/enabling-the-inspection-of-web-content-in-apps/), [Microsoft Learn](https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/windowed-vs-visual-hosting), [wry](https://docs.rs/wry/latest/wry/struct.WebViewBuilder.html) |
| R17 | Zeron, a Rust application that draws its own window with wgpu, embeds system webviews, forces WebKitGTK to shared-memory frames on Linux, and offers agents no browser tool. | `/home/arthur/dev/zeron/apps/zgx/src/main.rs:222-233`, `/home/arthur/dev/zeron/crates/mcp/src/tools.rs:98-230` |
| R18 | t3code exposes 14 MCP tools over Playwright and CDP. One `preview_snapshot` call returns visible text, the accessibility tree, console, network, an action timeline and a PNG. Tabs have an owner (agent, human or unclaimed) and agent input pauses while the human annotates. It does not fence page text. | `/home/arthur/dev/t3code/apps/server/src/mcp/toolkits/preview/tools.ts:60-231`, `packages/contracts/src/previewAutomation.ts` |
| R19 | Shipping products start the embedded browser on a clean profile, ask before acting on new sites, and state that page text is untrusted. Agent tools converged on accessibility-tree snapshots with element references. | [Cursor](https://cursor.com/docs/agent/tools/browser), [Claude Desktop](https://code.claude.com/docs/en/desktop.md), [Codex](https://learn.chatgpt.com/docs/browser.md?surface=app), [VS Code](https://code.visualstudio.com/docs/debugtest/integrated-browser), [Playwright MCP](https://github.com/microsoft/playwright-mcp), [agent-browser](https://github.com/vercel-labs/agent-browser) |
| R20 | Prompt injection through page content is described as unsolved by both vendors that measured it. | [Anthropic](https://www.anthropic.com/news/claude-for-chrome), [OpenAI, 2025-12-22](https://openai.com/index/hardening-atlas-against-prompt-injection) |
| R21 | The FSF lists Modified BSD, Apache-2.0 (with GPLv3), LGPL-2.1 and MPL-2.0 as GPL-compatible. | [FSF license list](https://www.gnu.org/licenses/license-list.html) |
| R22 | `Page.startScreencast` streams JPEG or PNG frames with an acknowledgement per frame and is marked experimental. | [Page.pdl](https://raw.githubusercontent.com/ChromeDevTools/devtools-protocol/master/pdl/domains/Page.pdl) |

Not confirmed on a primary page: the exact size of the CEF distribution per target, whether the fence fix of R10 is in a published CEF release, which Linux distributions restrict unprivileged user namespaces, and whether official CEF builds include proprietary codecs. Each is settled by a story (US-001, US-002, US-006, US-040).

## Technical Decisions

| # | Decision | Options considered | Evidence and reason |
|---|----------|--------------------|---------------------|
| TD-1 | Chromium through CEF. | System webviews; Servo; a Chromium fork; an external Chrome driven over a CDP pipe with screencast; a terminal-rendered browser. | An agent must check the engine its users run, with the same control on four targets. System webviews fail on control (R16, R17). A fork adds continuous Chromium builds for no capability CEF lacks (R9, R11, R15). The external Chrome gives full control but second-class display (R22) and stays the fallback of last resort. |
| TD-2 | The engine runs in a dedicated host process, `paneflow-browser-host`, never in the window process. | In-process CEF. | CDP calls and CEF callbacks live on CEF's UI thread (R11); the GPUI main thread must not block (AGENTS.md); a Chromium crash must not end terminal sessions. Atlas made the same choice (R15). |
| TD-3 | The host crate sits at `crates/paneflow-browser-host`, excluded from the Cargo workspace, with its own lockfile and its own CI job. The shared protocol crate `paneflow-browser-proto` is a workspace member. | Host as a workspace member with CEF fetched by every CI job, as libghostty is. | A CEF build downloads about 1 GB (R13); every workspace gate runs `--workspace`, so membership would put that cost on every job and every contributor. |
| TD-4 | CDP is called inside the host through CEF's DevTools methods. The host never enables a remote debugging port or pipe. | `--remote-debugging-port`; `--remote-debugging-pipe`. | R11: no debugging session is needed. A port can be driven by any local process (Problem Statement 3). |
| TD-5 | Agents get semantic `browser.*` methods, not raw CDP. | Raw CDP passthrough. | One authorization point and one decision table already exist (R6). Semantic tools keep the engine replaceable and keep answers small (R18, R19). Raw CDP is a Non-Goal of this version. |
| TD-6 | The CPU path is built first and kept as an explicit, labelled fallback. | GPU only, as v1.0 required. | R10: the Linux GPU path is weeks old on NVIDIA. R17: a comparable Rust application gave up the GPU path on Linux. Without a fallback, affected machines would have no browser. |
| TD-7 | On Linux and Windows a GPU frame reaches the renderer through the image atlas: the surface paints a `RenderImage` whose id is registered with the platform atlas as externally backed, and the atlas returns a tile over the imported texture. | A cross-platform surface primitive in the `gpui` core crate. | R1, R3: `paint_surface` cannot be called off macOS without patching `gpui` core, which is a pinned git dependency that Paneflow stopped forking. The atlas and renderer of `gpui_wgpu` are already vendored (R4). Feasibility is proven by US-003 before any product code depends on it. |
| TD-8 | On macOS a GPU frame is an IOSurface wrapped in a `CVPixelBuffer` and painted with the existing `paint_surface`. | The atlas seam of TD-7. | R1: the primitive exists and Metal draws it. No vendored copy of the macOS renderer is needed. Proven in EP-008. |
| TD-9 | `gpui_windows` becomes a third vendored GPUI crate, carrying only the external-texture seam. | Waiting for an upstream surface API. | R2, R4. The seam is written so that it can be proposed to Zed; Paneflow does not depend on its acceptance. |
| TD-10 | The runtime (host binary plus the CEF distribution) is one archive per target, signed with the release minisign key, downloaded on first use into the Paneflow home and updated on its own schedule. | Bundled in every installer, as v1.0 required. | R8, R13: bundling multiplies every artifact by several times for users who never open a browser, and ties Chromium's security fixes to application releases. `paneflow-minisign` and `src-app/src/update/verified_download.rs` already verify signed downloads. |
| TD-11 | The Chromium sandbox is never disabled. Where it cannot start, the browser refuses to start and says why. | `--no-sandbox` on affected systems. | v1.0 constraint, kept. R13 names the systems at risk; US-006 measures them. |
| TD-12 | One browser profile per workspace, stored under the Paneflow home, never seeded from the user's own browsers. | A global profile; importing cookies as cmux does. | R19: every shipping product that embeds a browser starts clean, and Cursor isolates storage per workspace. |
| TD-13 | The window process owns the browser methods (`Owner` of each descriptor is the window). | The worker, so that a browser can run with no window. | The surface is a window feature and the host is a child of the window. A browser with no window is a Non-Goal. |
| TD-14 | Page-derived text returns through `wrap_untrusted` of `paneflow-mcp`, unchanged, with the header attribute `source="browser"`. | A new tag. | R7 exists and is tested; R18 shows a comparable product without it. |
| TD-15 | Every tab has one owner: an agent session, the human, or nobody (unclaimed). Only the owner drives it; human input on an agent's tab takes it over. | No ownership; one tab per agent. | R18: t3code ships this model and pauses agent input while the human acts. |
| TD-16 | On Windows the host is reached over a named pipe inside the home's endpoint namespace and lives in a kill-on-close Job Object. | A TCP loopback socket. | Paneflow's IPC already uses a per-home pipe (`ARCHITECTURE.md:666`) and Job Objects (`ARCHITECTURE.md:714`); a TCP socket would recreate the exposure of Problem Statement 3. |
| TD-17 | Latency is judged by an A/B run of `PANEFLOW_LATENCY_PROBE=1` on the same machine: three runs without the browser give three medians, and a with-browser median greater than the largest of them is a regression. | Comparing `scripts/bench-terminal.sh` with a baseline. | That suite runs in process and does not see a video playing in the dock; only `bench/baselines/linux-x86_64` exists. The number of runs is assumption A15, not a budget. |

## Assumptions & Constraints

### Assumptions to validate

| ID | Assumption | Risk | Validated by | If it fails |
|----|-----------|------|--------------|-------------|
| A1 | CEF delivers dmabuf frames under native Wayland and under X11 on the development machine. | High | US-002 | The Linux GPU path is NO-GO for this version; EP-005 is cancelled and the CPU path ships alone. |
| A2 | A frame exported by the host can be imported by the vendored `gpui_wgpu` through the atlas seam of TD-7, clipped to its rectangle and covered by a popup drawn above it. | High | US-003 | US-007 records the failure; the alternative (patching `gpui` core) is put to Arthur as an open question before EP-005. |
| A3 | Playing video in a browser tab causes no terminal latency regression, as TD-17 defines one, on either path. | High | US-003, US-005 | The path that regresses does not ship. If both regress, the display falls back to the external Chrome of TD-1 and the PRD is revised. |
| A4 | Every CDP domain the tools need (Accessibility, DOM, Input, Runtime, Network, Page, Log) answers in windowless mode with no port open and with the tab not displayed. | High | US-004 | The missing capability is listed in US-007; if a core one is missing, control moves to an external Chrome over a pipe and the PRD is revised. |
| A5 | The Chromium sandbox starts on the Linux install formats Paneflow ships. | High | US-006 | The formats that fail refuse to start the browser (TD-11) and are documented. |
| A6 | The CEF distribution contains no component whose license forbids distribution next to a GPL-3.0-or-later program. | Medium | US-040 | The offending component is removed from the runtime or the runtime is not published. |
| A7 | A snapshot bounded at 32 KiB of text is enough for an agent to locate the defects of the fixture application. The bound comes from no measurement (t3code truncates near 20 KB). | Medium | US-004, US-016 | The bound changes to the value US-004 measures. |
| A8 | The runtime archive fits under the 500 MiB download cap that application artifacts already respect. | Medium | US-001 | The cap for the runtime is set from the measured size and recorded. |
| A9 | The CEF sandbox can run on Windows from a bootstrap executable placed outside the application's install directory. It cannot be tested earlier: Windows validation is last by decision. | High | US-053 | The browser is disabled on Windows (TD-11) until it can. |
| A10 | An IOSurface can be transferred from the host to the window and painted by `paint_surface` on macOS. | Medium | US-051 | macOS ships the CPU path only. |
| A11 | The initial limits of the "Limits" table below suit real use. Only the URL length has a source. | Low | US-004, US-022 | A limit changes to the measured need and the table is updated. |
| A12 | The `cef` crate 156.7.0 builds for all four shipping targets. | Medium | US-009, US-036, EP-007's Definition of Done | The target that fails has no runtime until the binding is fixed or replaced by the C API. |
| A13 | The CPU path causes no latency regression on macOS and Windows. It is measured on Linux only before the final epics. | Medium | US-051, US-056 | The CPU path is not the default on that platform; if the GPU path also fails there, the browser is agent-only on it. |
| A14 | The Linux paths behave on `aarch64-unknown-linux-gnu` as on x86_64. CI builds the aarch64 runtime; no story tests it on aarch64 hardware. | Medium | Not validated in this PRD | The automatic fallback of US-033 and US-034 applies; a defect report opens a follow-up. |
| A15 | Three no-browser runs bound run-to-run noise well enough for TD-17 to discriminate. | Low | US-003 | If the three medians spread wider than the difference being judged, the report says so and the number of runs is raised and recorded. |

### Limits

Each limit is one named constant in `paneflow-browser-proto`. The values are initial and are assumptions (A11), except the URL length.

| Limit | Initial value | Source |
|-------|---------------|--------|
| URL length accepted from an agent | 2048 characters | t3code, `packages/contracts/src/previewAutomation.ts:15-18` |
| Snapshot text | 32 KiB | A7 |
| Host idle period before exit | 300 seconds | assumption |
| DevTools method timeout | 30 seconds | assumption |
| `browser.evaluate` timeout | 30 seconds | assumption |
| Maximum protocol message | 16 MiB | assumption |
| Console and network entries kept per tab | 1000 each | assumption |
| Network-idle quiet period for `browser.wait_for` | no request in flight for 500 milliseconds | assumption, after Playwright's `networkidle` |

### The browser API

The complete method list of this version, each one `Descriptor`: `browser.status`, `browser.install`, `browser.open`, `browser.list`, `browser.claim`, `browser.close`, `browser.navigate`, `browser.reload`, `browser.back`, `browser.forward`, `browser.snapshot`, `browser.act`, `browser.wait_for`, `browser.console`, `browser.network`, `browser.screenshot`, `browser.evaluate`.

The accessibility tree in a snapshot is plain text, one node per line, indented two spaces per depth, in the form `role "accessible name" [ref=eN]`. A reference `eN` exists only on interactive nodes, is unique within one snapshot, and belongs to that snapshot's generation number. A reference from a generation older than the tab's last navigation is stale.

### Fixtures

Tracked fixtures live in `crates/paneflow-browser-host/tests/fixtures/` and are served by a loopback static server the tests start: a fixture application with one planted console error and one planted request answering 500, a form page with a `<select>`, a password field, a file input and a submit button, a video page, and a page whose visible text tells the reader to open an external site. EP-001 uses spike copies of the same pages; US-009 commits them.

### Hard constraints

- Before any story starts, the worktree `/home/arthur/dev/paneflow-browser` (branch `feat/agents-browser`) is removed by Arthur or on his explicit request. No story reads it, copies from it or merges it.
- libghostty stays the only terminal engine. No CEF or Chromium type appears in `src-app` or in any workspace crate; they live only in `crates/paneflow-browser-host`.
- The GPUI main thread never waits on the host, on a frame, on file I/O or on the network. Work reaches it as bounded messages.
- The three embedded helpers keep their size caps (`scripts/check-helper-sizes.sh`). The browser tools of `paneflow-mcp` are thin proxies over IPC.
- No release build passes `--no-sandbox`, `--disable-web-security` or a certificate bypass to Chromium.
- The project stays GPL-3.0-or-later. `cargo deny` covers Rust crates only; the native CEF distribution is audited separately (US-040).
- The shipping targets are the four of AGENTS.md: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`.
- The GPUI pin is `c016f436` at the time of writing. A GPUI bump re-applies the browser seam to every vendored GPUI crate, as each vendor manifest header will state.
- A live test runs in a debug build on its own tagged home (`scripts/dev.sh --tag <name>`), never in the installed application.
- Source files carry no comments (AGENTS.md).
- macOS is validated on Apple Silicon hardware (a rented bare-metal Mac), not on a hosted CI runner. Windows is validated on Arthur's Windows 11 machine.

## Non-Goals

- **Raw CDP for agents.** No `browser.cdp` passthrough in this version (TD-5). `browser.evaluate` covers the cases the semantic tools miss.
- **Driving the user's own browser.** No extension, no attachment to an installed Chrome, no cookie import. The embedded browser is for checking what is being built.
- **A general-purpose browser.** No bookmarks, history UI, password manager, extensions, sync or autofill.
- **A browser with no window.** The host is a child of the window (TD-13).
- **Screen-reader exposure of page content.** The browser chrome is accessible like any Paneflow control; bridging Chromium's accessibility tree to the platform's assistive API is not in this version.
- **Element picker to an in-app chat.** Paneflow has no chat surface; agents are terminal programs.
- **Downloads.** A page-started download is cancelled (US-013); saving files from the browser is not in this version.
- **DevTools for the human.** No inspector window or "inspect element"; the agent tools are the debugging surface.
- **Offline or manual runtime install.** The runtime comes from the signed download of US-037 only.
- **Hardware validation on `aarch64-unknown-linux-gnu`.** The runtime is built for it and shares the Linux code (A14).
- **Video recording of sessions, viewport emulation presets, tracing.** Not needed to find a defect; t3code and Codex offer them and they can follow.
- **Proprietary codecs.** The runtime uses the codecs the official CEF builds carry.
- **macOS on Intel and Windows on ARM64.** Neither is a shipping target today.
- **Changes to the public user docs in this repository.** `docs/user/` is a mirror synced from the site repository.

## Quality Gates

These commands must pass for every user story:
- `cargo fmt --check` - formatting, the CI gate
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - lints, test modules included
- `cargo nextest run --workspace --locked --profile ci` - the full suite as CI runs it
- `bash scripts/check-test-wallclock.sh` - no new wall-clock budget in a unit test
- `bash scripts/check-helper-sizes.sh` - the shim, hook and MCP helpers stay under their caps (run after nextest on Linux x86_64)
- `cargo deny check advisories licenses sources` - when a dependency changes

When `crates/paneflow-browser-host/**` changes, the same three cargo gates run against its manifest:
- `cargo fmt --check --manifest-path crates/paneflow-browser-host/Cargo.toml` - formatting of the host crate
- `cargo clippy --manifest-path crates/paneflow-browser-host/Cargo.toml --all-targets --locked -- -D warnings` - lints of the host crate
- `cargo nextest run --manifest-path crates/paneflow-browser-host/Cargo.toml --locked --profile ci` - the host suite

When `.github/**` changes:
- `bash scripts/check-workflows.sh` - pinned actions, timeouts, runner images, cited scripts
- `uvx zizmor@1.32.0 --config .github/zizmor.yml --min-severity high .github` - the workflow audit

When `vendor/**` changes:
- `cargo fmt --all --check` - also formats path dependencies outside the workspace

When a CI script changes:
- `bash scripts/test-ci-scripts.sh` - runs the scripts against fixtures

## Epics & User Stories

### EP-001: Kill tests on Linux

Three experiments, a fallback measurement and a sandbox survey, run outside product code on the Fedora Wayland development machine. Each can end the recommendation alone. Spike code lives under `tasks/evidence/agents-browser/` (local evidence, not tracked) and builds into a `CARGO_TARGET_DIR` outside the repository.

**Definition of Done:** this PRD contains a section "Kill test report" with one dated result per story US-002 to US-006, each naming the machine, GPU, driver, kernel, compositor and CEF version; US-007 records GO or NO-GO for the GPU path and for the control plane, and every assumption A1 to A5 and A7 to A8 is marked confirmed or failed.

#### US-001: Pin the engine
**Description:** As a maintainer, I want the CEF version, its Rust binding and its archives pinned by hash so that every later story builds against the same engine.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given the repository, when `native/cef/manifest.toml` is read, then it names one CEF version, one `cef` crate version, and for each of the four shipping targets an archive URL and a SHA-256.
- [ ] Given a clean checkout, when `scripts/fetch-cef.sh` runs for the host target, then the archive is placed under `native/cef/prebuilt/<target>/` after its hash matches the manifest.
- [ ] Given an archive whose hash differs from the manifest, when `scripts/fetch-cef.sh` runs, then it exits non-zero and leaves no archive in place.
- [ ] Given the fetched archive for each target, when its size is measured, then the four sizes are written in the "Kill test report" and assumption A8 is marked.
- [ ] Given `.gitignore`, when `git status` runs after a fetch, then `native/cef/prebuilt/` shows no untracked file.

#### US-002: Kill test 1, shared textures under Wayland and X11
**Description:** As a maintainer, I want to know whether CEF hands out GPU frames on my Linux machine so that the GPU path is not built on a belief.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given the off-screen rendering example of the `cef` crate built with `accelerated_osr`, when it loads an animated WebGL page with `--ozone-platform=wayland`, then the report states whether `OnAcceleratedPaint` was called and carries two screenshots taken one second apart showing the animation advanced with no black region.
- [ ] Given the same build, when it runs with `--ozone-platform=x11`, then the report states the same two facts for X11.
- [ ] Given either run, when CEF falls back to software painting, then the report records the fallback and the log line that explains it, and A1 is marked failed for that backend.
- [ ] Given the pinned CEF version, when its source or release notes are checked for the fixes of CEF issues #4237 and #4313, then the report states for each whether the pinned version contains it.

#### US-003: Kill test 2, a frame crosses two processes into a GPUI rectangle
**Description:** As a maintainer, I want a host process to send GPU frames to a minimal GPUI window so that the cross-process path and the atlas seam are proven before product code depends on them.

**Priority:** P0
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [ ] Given a spike host that copies each CEF frame into a texture it owns, when it sends the dmabuf descriptor over a Unix socket, then a second process imports it into the wgpu device of a GPUI window.
- [ ] Given a copy of `vendor/gpui_wgpu` patched in the spike with the atlas seam of TD-7, when the window paints the frame inside a rectangle smaller than the window, then the page is clipped to the rectangle and a GPUI popup drawn above it covers it.
- [ ] Given the window being resized while a video plays, when frames are observed, then the report states whether tearing or stale-size frames appeared.
- [ ] Given a terminal pane beside the rectangle and the video fixture playing, when the A/B method of TD-17 is run, then the three no-browser medians, the with-browser median and the clip's frame rate are written in the report and A3 is marked for the GPU path.
- [ ] Given the atlas seam cannot return a tile over an imported texture, when the spike ends, then the report states the exact obstacle and A2 is marked failed.

#### US-004: Kill test 3, an agent finds planted defects with no port open
**Description:** As a maintainer, I want an agent to find a console error and a failed request through internal CDP only so that the control plane is proven without any display.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given a spike host with a windowless browser that is not displayed, when it calls `ExecuteDevToolsMethod` for the Accessibility, DOM, Input, Runtime, Network, Page and Log domains, then the report lists each domain as answering or not.
- [ ] Given the host running, when the listening TCP sockets of its process tree are listed, then none belongs to a DevTools endpoint.
- [ ] Given the fixture application served on loopback, when a real agent drives a throwaway MCP tool connected to the host, then the agent names both defects and the element involved without a screenshot.
- [ ] Given the accessibility snapshot of that page serialised as text with element references, when its size is measured, then the size is written in the report and A7 is marked or its bound is replaced.
- [ ] Given a domain that does not answer in windowless mode, when the spike ends, then A4 is marked failed and the missing capability is named.

#### US-005: Measure the CPU path
**Description:** As a maintainer, I want the cost of the shared-memory path measured so that the fallback is a known quantity and not a hope.

**Priority:** P0
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [ ] Given the spike host painting through `OnPaint` into a shared-memory segment, when the window uploads each frame through `paint_image`, then the page displays at the viewport size of the dock at its default width.
- [ ] Given the video fixture playing on the CPU path, when the A/B method of TD-17 is run, then the three no-browser medians and the with-browser median are written in the report and A3 is marked for the CPU path.
- [ ] Given the same run, when a `cargo flamegraph` profile of the window process is taken, then the share of time spent uploading frames is written in the report.
- [ ] Given the CPU path shows a regression as TD-17 defines one, when the report is written, then it says so in its first line.

#### US-006: Survey the Chromium sandbox on Linux install formats
**Description:** As a maintainer, I want to know where the Chromium sandbox starts so that the browser never ships unsandboxed by accident.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given the spike host unpacked in a user's home directory, when it starts on the Fedora development machine and in a virtual machine of each cloud image `scripts/sandbox-matrix.sh` names (ubuntu-22.04, ubuntu-24.04, debian-13, arch, opensuse-leap-16), then the report has one row per system stating whether the sandbox started and which mechanism it used.
- [ ] Given the product script `scripts/sandbox-matrix.sh`, when the survey is done, then it is unchanged; the survey boots the images from a spike script kept in the evidence directory.
- [ ] Given a distribution where unprivileged user namespaces are restricted, when the host starts, then the report records the exact refusal and what a package would have to install to lift it.
- [ ] Given the host started from inside an AppImage-launched application, when the sandbox state is read, then the report records it.
- [ ] Given any row where the sandbox did not start, when the report is written, then A5 is marked failed for that row and no flag disabling the sandbox was used to continue.

#### US-007: Record the verdict
**Description:** As Arthur, I want one GO or NO-GO per path with its evidence so that the rest of the PRD is engaged or revised knowingly.

**Priority:** P0
**Dependencies:** Blocked by US-002, US-003, US-004, US-005, US-006

**Acceptance Criteria:**
- [ ] Given the five results, when the "Kill test report" is read, then it states GO or NO-GO for the control plane, for the CPU path and for the Linux GPU path.
- [ ] Given a NO-GO for the Linux GPU path, when the status file is read, then US-031, US-032, US-033 and US-035 are `CANCELLED`, US-034 stays because the macOS and Windows paths depend on it, and the Changelog says so.
- [ ] Given a NO-GO for the control plane, when the status file is read, then every epic after EP-001 is `BLOCKED` and an Open Question names the alternative of TD-1.
- [ ] Given any assumption A1 to A5, A7 or A8, when the assumptions table is read, then it is marked confirmed or failed with a link to its report line.
- [ ] Given the spike code, when the tracked tree is listed, then nothing from `tasks/evidence/agents-browser/` is tracked.

---

### EP-002: The browser host and its private protocol

A host process that owns Chromium, started on demand, supervised, speaking a versioned private protocol, with internal CDP, per-workspace profiles and an origin policy. Nothing is displayed yet.

**Definition of Done:** on Linux, an integration test starts the host from a debug build, opens a tab on a local fixture page, receives its load event and its console message through the protocol, kills the host and observes the tab reported as crashed, with no DevTools port open at any point; no CEF type is nameable from any workspace crate.

#### US-008: Define the private protocol
**Description:** As a maintainer, I want one crate that names browser, tab, commands, events and frame descriptors in neutral types so that the window and the host agree without sharing engine types.

**Priority:** P0
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [ ] Given the workspace, when `crates/paneflow-browser-proto` is built, then it depends on no CEF crate and no GPUI crate.
- [ ] Given a window and a host built from different protocol versions, when they handshake, then the connection is refused with an error naming both versions.
- [ ] Given every command and event type, when it is serialised and deserialised in a unit test, then the value is unchanged.
- [ ] Given a frame larger than the maximum message size the crate declares, when it is decoded, then decoding fails without allocating the declared length.
- [ ] Given `src-app` and every workspace crate, when a guard test scans their manifests, then none names the `cef` crate.

#### US-009: Build the host crate outside the workspace
**Description:** As a maintainer, I want the host to build from its own manifest with its own CI job so that the workspace gates do not download Chromium.

**Priority:** P0
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [ ] Given the root `Cargo.toml`, when `cargo metadata` runs, then `crates/paneflow-browser-host` is not a workspace member.
- [ ] Given a checkout with the CEF archive fetched, when the host manifest is built with `--locked`, then it produces the `paneflow-browser-host` binary.
- [ ] Given `.github/workflows/run_tests.yml`, when a change touches the host crate or the protocol crate, then a job named for the browser host builds, lints and tests it on Linux x86_64.
- [ ] Given a checkout with no CEF archive, when the host build starts, then it fails with a message naming `scripts/fetch-cef.sh`.
- [ ] Given the host crate, when its `tests/fixtures/` directory is listed, then it holds the four tracked fixtures of the "Fixtures" section and a test serves each on loopback.
- [ ] Given `deny.toml`, when `cargo deny check` runs against the host manifest, then it passes with no new registry or git source allowed.

#### US-010: Supervise the host
**Description:** As a user, I want the browser engine to start only when I open a tab and to fail without taking my terminals with it.

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [ ] Given an application launch in which no browser tab is opened, when the process tree is listed, then no host process exists.
- [ ] Given the first tab being opened, when the host starts, then the window connects over an endpoint inside the Paneflow home's runtime directory and the host accepts only its parent process, checked with `paneflow-peer`.
- [ ] Given a running host, when it is killed, then every tab reports a crashed state through the protocol and every terminal session keeps running.
- [ ] Given a crashed host, when the user reloads a tab, then a new host starts and the tab loads.
- [ ] Given the last tab closed, when the host has been idle for the period the protocol crate declares, then the host exits.
- [ ] Given the window started from inside a Paneflow pane, when it starts the host, then the host's environment carries none of `paneflow_host::env::PANE_CONTEXT_ENV`.
- [ ] Given the host fails to start, when the window observes it, then the GPUI main thread has not waited on it and the tab shows the failure reason.
- [ ] Given a debug build, when the host binary is resolved, then it comes from the path in `PANEFLOW_BROWSER_HOST` or, when unset, from the host crate's local build output, and never from an installed application's home.

#### US-011: Bridge CDP inside the host
**Description:** As a maintainer, I want the window to call DevTools methods and receive DevTools events through the protocol so that agent tools need no debugging port.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given a tab on a fixture page, when the window sends a DevTools method through the protocol, then the result returns matched by request id.
- [ ] Given a fixture page that logs to the console, when the page loads, then the window receives the console event through the protocol.
- [ ] Given a DevTools method that never answers, when the timeout the protocol crate declares elapses, then the caller receives a timeout error and the host stays responsive.
- [ ] Given a running host, when its command line and its listening sockets are inspected in a test, then no remote-debugging flag and no DevTools TCP endpoint is present.
- [ ] Given a method sent for a tab that has closed, when the host handles it, then the caller receives an error naming the tab as closed.

#### US-012: Keep one profile per workspace
**Description:** As a user, I want each workspace to have its own cookies and storage so that test accounts of one project never appear in another.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given two workspaces, when each sets a cookie on the same loopback origin, then neither reads the other's cookie.
- [ ] Given a workspace profile, when its location is read, then it is under the Paneflow home resolved by `crates/paneflow-home`, never under `dirs::config_dir` or `dirs::cache_dir`.
- [ ] Given a workspace closed and reopened, when a tab loads the same origin, then the cookie set before is present.
- [ ] Given a profile directory already locked by another host, when a second host opens it, then the open fails with an error naming the lock.
- [ ] Given a fresh profile, when its cookie store is read, then it is empty and no file of an installed browser was read to create it.

#### US-013: Enforce origins, schemes and permissions in the host
**Description:** As a user, I want the browser to reach my local servers freely and nothing else without my decision so that a page cannot wander.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given a navigation to an `http` or `https` URL on a loopback address, when no decision exists, then the navigation proceeds.
- [ ] Given a top-level navigation to an origin that is not loopback and has no decision, when the host sees it, then the navigation is held and a decision request is emitted through the protocol.
- [ ] Given a navigation to `file:`, `chrome:`, `devtools:`, `view-source:`, `javascript:` or a top-level `data:` URL, when the host sees it, then it is refused with an error naming the scheme.
- [ ] Given a page that requests camera, microphone, geolocation or notifications, when the host sees the request, then it is denied.
- [ ] Given a download started by a page, when the host sees it, then the download is cancelled and an event reports it.
- [ ] Given a URL that embeds credentials, when it is submitted for navigation, then it is refused.

---

### EP-003: Agent control

The `browser.*` methods, their right, their approvals, and their MCP and CLI entry points. An agent can verify a local application with no visible surface.

**Definition of Done:** on Linux, an integration test drives `browser.open`, `browser.snapshot` and `browser.network` through the IPC client against the fixture application and finds the planted console error and the planted failing request in the results; a real agent in a pane does the same through the MCP tools (US-022); the same calls from a pane without the right are refused by `authorize`; `crates/paneflow-api/tests/decision_table.txt` and `protocol/api-v1.json` are regenerated and committed.

#### US-014: Declare the browser methods and the browser right
**Description:** As a maintainer, I want every browser method in the registry with one right so that authorization stays in one place.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [ ] Given `crates/paneflow-api`, when the method list is read, then each method of "The browser API" is one `Descriptor` owned by the window, and no other `browser.*` method exists.
- [ ] Given the rights of a pane grant, when they are listed, then `browser` appears beside `write`, `spawn` and `layout`.
- [ ] Given a caller with no `browser` right, when it calls any `browser.*` method, then `authorize` refuses it and the decision is a row of `decision_table.txt`.
- [ ] Given the regenerated `protocol/api-v1.json`, when it is compared with the registry, then every `browser.*` method and its schema is present.
- [ ] Given a caller whose identity cannot be established, when it calls a `browser.*` method, then the call is refused before reaching the host.

#### US-015: Open, list, navigate and close tabs with an owner
**Description:** As an agent, I want to open a tab on my workspace's server and know which tabs are mine so that I never act on a page the human is using.

**Priority:** P0
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [ ] Given an agent with the right, when it calls `browser.open` with a loopback URL, then a tab opens in its workspace's profile, owned by that agent's session, and its id is returned.
- [ ] Given tabs owned by the agent, by the human and by another agent, when the agent calls `browser.list`, then each tab is listed with its owner and only the agent's own tabs are marked drivable.
- [ ] Given a tab owned by the human, when an agent calls `browser.navigate` on it, then the call is refused with an error naming the owner.
- [ ] Given an agent-owned tab, when the agent calls `browser.navigate`, `browser.reload`, `browser.back` or `browser.close`, then the tab does so and the result reports the final URL.
- [ ] Given a URL longer than the limit the API declares or with a scheme other than `http` or `https`, when it is submitted, then the call is refused without reaching the host.
- [ ] Given an agent session that ends, when its tabs are listed afterwards, then they are reported as unclaimed.
- [ ] Given an unclaimed tab, when an agent with the right calls `browser.claim`, then the tab is owned by that agent's session; given a tab owned by someone else, then the call is refused naming the owner.

#### US-016: Return one grouped snapshot
**Description:** As an agent, I want one call that tells me what the page shows and what went wrong so that I do not spend turns assembling it.

**Priority:** P0
**Dependencies:** Blocked by US-015

**Acceptance Criteria:**
- [ ] Given a loaded tab, when the agent calls `browser.snapshot`, then the result carries the URL and title, the accessibility tree in the text form of "The browser API", and the console and network entries recorded since the previous snapshot.
- [ ] Given a page whose snapshot exceeds the bound of A7, when the snapshot is returned, then it is truncated at the bound and states how much was dropped.
- [ ] Given a snapshot, when it reaches the agent through MCP, then all page-derived text is inside one `wrap_untrusted` block whose header carries `source="browser"`.
- [ ] Given a password field with a value, when the snapshot is taken, then the value is absent from the result.
- [ ] Given a tab that is still loading, when `browser.snapshot` is called, then the result says so and returns what exists.
- [ ] Given a tab that is not displayed in any pane or dock, when `browser.snapshot` is called, then the result is the same as for a displayed tab.

#### US-017: Act on elements by reference and wait for conditions
**Description:** As an agent, I want to click, type and wait using the references of the snapshot so that my actions hit what I saw.

**Priority:** P0
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] Given a reference from the latest snapshot, when the agent calls `browser.act` with click, type, press, hover, select or scroll, then the action is applied to that element and the result reports whether a navigation followed.
- [ ] Given a reference from a snapshot taken before a navigation, when it is used, then the call fails with an error saying the reference is stale and a new snapshot is needed.
- [ ] Given a reference to an element that is hidden or disabled, when a click is requested, then the call fails naming the reason.
- [ ] Given `browser.wait_for` with a text, a reference or a network-idle condition, when the condition holds before the timeout, then the call returns; when it does not, the call returns a timeout error stating what was awaited.
- [ ] Given a JavaScript dialog opened by the action, when the result returns, then it reports the dialog and its message as fenced text, and the dialog is dismissed.

#### US-018: Read console, network and a screenshot on demand
**Description:** As an agent, I want to page through console and network history and get an image when text is not enough.

**Priority:** P0
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [ ] Given a tab with console history, when the agent calls `browser.console` with a cursor, then entries after the cursor are returned with level, text and source location, and a new cursor.
- [ ] Given a tab with network history, when the agent calls `browser.network`, then each entry carries method, URL, status, resource type and failure reason, and no request or response body.
- [ ] Given more entries than the per-tab buffer holds, when history is read, then the result states that older entries were dropped.
- [ ] Given `browser.screenshot`, when it is called on an agent-owned tab, then a PNG of the viewport is returned at the tab's device scale factor.
- [ ] Given a request header named `Authorization` or `Cookie`, when network entries are returned, then its value is absent.

#### US-019: Evaluate JavaScript behind an approval
**Description:** As an agent, I want to run a script in the page when the semantic tools are not enough, and as a user, I want to approve that the first time.

**Priority:** P1
**Dependencies:** Blocked by US-017, US-020

**Acceptance Criteria:**
- [ ] Given an agent that has never been approved for evaluation in this workspace, when it calls `browser.evaluate`, then the call waits on an approval that shows the agent, the origin and the script.
- [ ] Given the approval granted, when the script runs, then its JSON-serialisable result is returned as fenced text bounded by the snapshot bound.
- [ ] Given the approval denied, when the call returns, then it carries a refusal and the script did not run.
- [ ] Given a script that throws, when the call returns, then it carries the exception message as fenced text.
- [ ] Given a script still running at the timeout the API declares, when the timeout elapses, then the call returns a timeout error.

#### US-020: Ask the user before an agent acts on a new site
**Description:** As a user, I want to decide per site whether an agent may browse it so that a page cannot send my agent elsewhere.

**Priority:** P0
**Dependencies:** Blocked by US-015, US-013

**Acceptance Criteria:**
- [ ] Given an agent-owned tab navigating to an origin that is not loopback and has no decision, when the host holds the navigation, then the approval broker raises a request showing the agent and the origin, with allow once, always allow for this workspace, and deny.
- [ ] Given "always allow" chosen, when the same agent session or another one in the workspace navigates to that origin, then no request is raised.
- [ ] Given "deny" chosen, when the call returns, then it carries a refusal naming the origin and the tab stays on its previous page.
- [ ] Given a pending site approval, when a `browser.*` call or pane input from the requesting agent arrives, then it cannot answer the approval.
- [ ] Given a redirect from an allowed origin to an undecided one, when the host sees the redirect, then it is held and a request is raised for the new origin.

#### US-021: Expose the tools through MCP and the CLI
**Description:** As an agent, I want the browser methods as MCP tools and CLI verbs so that any coding agent in a pane can use them.

**Priority:** P0
**Dependencies:** Blocked by US-016, US-017, US-018

**Acceptance Criteria:**
- [ ] Given `paneflow-mcp`, when its tool list is read, then it contains one tool per `browser.*` method the agent may call, each described in one sentence that states page content is untrusted.
- [ ] Given the `paneflow` CLI, when `paneflow browser --help` runs, then it lists a verb per method with an example, and each verb has `--json` output.
- [ ] Given a browser tool called when no host binary can be resolved, when the call returns, then the error says the browser engine is not installed.
- [ ] Given `paneflow browser status --json`, when it runs, then it reports through `browser.status` whether a host runs, its runtime version, the sandbox state, and for each tab its owner and presentation path.
- [ ] Given the release-min build, when `scripts/check-helper-sizes.sh` runs, then `paneflow-mcp` is under its cap.
- [ ] Given an MCP client scoped to one workspace, when it lists tabs, then tabs of other workspaces are absent.

#### US-022: Prove the loop with a real agent
**Description:** As Arthur, I want a real agent to find planted defects through the shipped tools so that the control plane is proven end to end.

**Priority:** P0
**Dependencies:** Blocked by US-019, US-020, US-021

**Acceptance Criteria:**
- [ ] Given a debug build on a tagged home and the tracked fixture application, when an agent in a pane is asked to check the page, then it reports the console error and the failing request using only `browser.*` tools.
- [ ] Given the same session, when the agent fixes the fixture and checks again, then its second snapshot shows neither defect.
- [ ] Given the tracked fixture page whose visible text instructs the reader to open an external site, when the agent snapshots it, then no navigation to that site happens without a site approval being raised.
- [ ] Given the session transcript, when it is reviewed, then every page-derived string reached the agent inside a fence.

---

### EP-004: The browser surface

A browser tab in the right dock and as a pane, drawn through the CPU path on Linux, with navigation chrome, input, web popups, shared human and agent use, link routing and session restore.

**Definition of Done:** on Linux under Wayland and under X11, a user opens a browser tab in the dock without a Git repository, loads the fixture application, types in a form with an input method, uses a `<select>`, watches an agent drive an agent-owned tab, takes it over, opens a loopback link from a terminal into the dock with the setting of US-029 on, restarts Paneflow and finds the tabs restored without any request sent before activation; the manual pass of `DESIGN.md`'s UI delivery gate is recorded.

#### US-023: Open the dock without a Git snapshot
**Description:** As a user, I want to open the right dock on a browser tab in a folder that is not a repository so that browsing does not depend on Git.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given the dock's new-tab menu and the pane surface picker, when they are opened, then each offers a browser entry, backed by a `Browser` variant of `DiffDockTab` and of `PaneSurface` whose page area is empty until US-025.
- [ ] Given a workspace whose folder is not a Git repository, when the user opens a browser tab in the dock, then the dock opens and no `git` process is started.
- [ ] Given a repository workspace with the dock on a browser tab, when the tab is active, then no Git status refresh is triggered by browser activity.
- [ ] Given the dock closed with only browser tabs, when it is reopened, then it shows the browser tab that was active.
- [ ] Given a workspace with no repository, when the user selects the Changes tab, then the existing empty state is shown and the browser tab stays open on its page.

#### US-024: Add the browser chrome
**Description:** As a user, I want a browser tab with an address bar and navigation buttons that look like the rest of Paneflow.

**Priority:** P0
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [ ] Given a browser tab, when it is displayed, then it shows back, forward, reload or stop, an address field and a page area, built from the controls of `src-app/src/settings/kit.rs` and `ui_primitives.rs` with one tooltip per control.
- [ ] Given a page that is loading, crashed, held for a site decision or refused by policy, when the tab is displayed, then each state has its own visible message stating what happened and the available action.
- [ ] Given text typed in the address field, when Enter is pressed, then input with a scheme is used as is; input without one that is `localhost`, an IP literal, a `host:port` pair or contains a dot gets `http://` when the host is loopback and `https://` otherwise; any other input is refused with an inline message.
- [ ] Given no host binary can be resolved, when a browser tab is opened, then the page area states that the browser engine is not installed and no host is started (US-037 later turns this message into the install prompt).
- [ ] Given `DESIGN.md`, when it is read, then the browser chrome's dimensions and states are documented there.
- [ ] Given an address that the policy of US-013 refuses, when it is submitted from the address field, then the page area shows the refusal naming the scheme and the previous page stays loaded.

#### US-025: Present frames through the CPU path
**Description:** As a user, I want to see the page so that I can watch what I and the agent do.

**Priority:** P0
**Dependencies:** Blocked by US-011, US-023

**Acceptance Criteria:**
- [ ] Given a tab displayed in a pane or the dock, when the host paints, then pixels reach the window through shared memory and are drawn with `paint_image`, clipped to the page area and stacked under Paneflow's popups.
- [ ] Given the page area resized, when the next frame arrives, then it has the new size and no frame of the old size is stretched to fit.
- [ ] Given a window scale factor other than 1, when a page is displayed, then the host renders at that device scale factor.
- [ ] Given a tab that is not displayed, when the host is observed, then it produces no frames for that tab.
- [ ] Given a frame arriving while the main thread is busy, when it is handled, then only the most recent frame is uploaded and none is queued without bound.
- [ ] Given the shared-memory segment cannot be created, when the tab is displayed, then the page area shows the failure and the tab stays drivable by agents.

#### US-026: Route input, focus and shortcuts
**Description:** As a user, I want to click, scroll and type in the page, including with an input method, without losing Paneflow's own shortcuts.

**Priority:** P0
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [ ] Given the page area focused, when the user clicks, drags, scrolls and types, then a click on the fixture form's submit button, at its position in the snapshot, submits the form at scale factors 1 and 2.
- [ ] Given an input method composing text, when the composition is committed, then the page's field contains the committed text and the candidate window is positioned at the caret.
- [ ] Given the page area focused, when a Paneflow shortcut for pane focus, tab switching or the command palette is pressed, then Paneflow handles it and the page does not receive it.
- [ ] Given the page changing the cursor shape, when the pointer is over the page area, then the window shows that cursor and restores its own when the pointer leaves.
- [ ] Given focus moving from the page to a terminal, when the user types, then no keystroke reaches the page.
- [ ] Given the host crashes while the page area is focused, when the user types, then the keystrokes are dropped, the tab shows the crashed state and Paneflow's shortcuts still work.

#### US-027: Handle web popups, dialogs, new windows and files
**Description:** As a user, I want select menus, alerts, links that open a new window and file pickers to work so that the pages I build are usable.

**Priority:** P0
**Dependencies:** Blocked by US-026

**Acceptance Criteria:**
- [ ] Given a `<select>` opened in the page, when its list is shown, then it is drawn adjacent to the control and clipped to the window.
- [ ] Given a page calling `alert`, `confirm` or `prompt`, when the dialog opens, then it is rendered by Paneflow, names the origin, and the page receives the user's answer.
- [ ] Given a link that opens a new window, when it is followed, then a new browser tab opens with the same owner and profile, subject to the origin policy.
- [ ] Given a file input clicked by the human, when the picker opens, then it is the platform's file dialog and the page receives only the chosen file.
- [ ] Given a file input activated by an agent's `browser.act`, when the picker would open, then no dialog opens and the call returns an error saying file selection needs the human.

#### US-028: Share a tab between the human and the agent
**Description:** As a user, I want to see when an agent drives a tab and to take over at any moment.

**Priority:** P0
**Dependencies:** Blocked by US-024, US-026, US-015

**Acceptance Criteria:**
- [ ] Given an agent-owned tab displayed, when the agent acts, then the tab shows which agent session owns it.
- [ ] Given an agent-owned tab, when the human clicks or types in the page area, then ownership moves to the human and the agent's next `browser.act` is refused with an error naming the takeover.
- [ ] Given a human-owned tab, when the human chooses "hand to agent" for a session, then that session's calls on the tab succeed.
- [ ] Given an agent-owned tab that is not displayed anywhere, when the human opens the tab list, then the tab is listed with its owner and can be shown.

#### US-029: Open local links in the Paneflow browser
**Description:** As a user, I want a detected local URL to open beside my terminal when I choose so.

**Priority:** P1
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [ ] Given a URL in terminal output, when its context menu is opened, then it offers "Open in Paneflow browser" beside the existing external open.
- [ ] Given the setting `browser.open_local_links` at its default, when a loopback URL is activated, then it opens in the system browser as today.
- [ ] Given the setting turned on, when a loopback URL is activated from a terminal or from a detected port in the sidebar, then it opens in a browser tab of that workspace's dock.
- [ ] Given the setting turned on, when a URL that is not loopback is activated, then it opens in the system browser.
- [ ] Given the runtime is not installed and the setting is on, when a loopback URL is activated, then it opens in the system browser.

#### US-030: Restore tabs without touching the network
**Description:** As a user, I want my browser tabs back after a restart without every page reloading at once.

**Priority:** P1
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [ ] Given browser tabs open at quit, when Paneflow restarts, then each tab is restored with its URL, title and owner set to unclaimed.
- [ ] Given restored tabs, when none has been activated, then no host is running and no request has been sent.
- [ ] Given a restored tab activated, when it loads, then the origin policy applies as for a new navigation.
- [ ] Given a `session.json` whose browser entry holds a URL with a refused scheme, when the session is restored, then that tab is dropped and the others are restored.

---

### EP-005: GPU presentation on Linux

The shared-texture path under Wayland and X11, with the CPU path as its explicit fallback. US-031, US-032, US-033 and US-035 are engaged only if US-007 recorded GO for the Linux GPU path. US-034 is platform-neutral (the per-tab presentation state, its indicator and its setting) and is built either way; no story outside this epic depends on the other four.

**Definition of Done:** on the Linux development machine under native Wayland and under X11, a displayed tab presents through an imported dmabuf, a forced import failure switches the tab to the CPU path with a visible indicator, and the A/B latency method of TD-17 shows no regression while the video fixture plays in the dock; the measurement is a section of this PRD.

#### US-031: Add the external-texture seam to the vendored renderer
**Description:** As a maintainer, I want the vendored `gpui_wgpu` to draw an imported texture through the image atlas so that a GPU frame needs no change to the `gpui` core crate.

**Priority:** P1
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [ ] Given `vendor/gpui_wgpu`, when an image id is registered as externally backed, then `get_or_insert_with` for that id returns a tile covering the registered texture without calling the build closure.
- [ ] Given a registered id that is unregistered, when the next frame is drawn, then the texture is released and a later paint of that id uses the ordinary upload path.
- [ ] Given the header of `vendor/gpui_wgpu/Cargo.toml`, when it is read, then it names this change and when to drop it.
- [ ] Given the seam, when the vendored crate's own tests run, then a test covers register, draw and unregister.
- [ ] Given the registration API is called from a thread other than the one owning the renderer, when the call is made, then it is rejected or marshalled without a data race, proven by a test.

#### US-032: Export frames from the host as owned dmabufs
**Description:** As a maintainer, I want the host to hand out textures it owns so that a frame stays valid after CEF's callback returns.

**Priority:** P1
**Dependencies:** Blocked by US-031

**Acceptance Criteria:**
- [ ] Given a CEF accelerated frame, when the host receives it, then it copies it into a texture of a pool it owns and sends that texture's descriptors, format, modifier and size over the protocol.
- [ ] Given a frame sent, when the window has not released it, then the host does not write to that texture.
- [ ] Given the copy has not completed on the GPU, when the frame would be sent, then the host waits for completion on a host thread, never on the window.
- [ ] Given the window disconnects, when the host observes it, then every pooled texture is released.
- [ ] Given CEF falls back to software painting for a tab, when the host observes it, then it switches that tab to the CPU path and reports the reason.

#### US-033: Import and present GPU frames in the window
**Description:** As a user, I want the page drawn straight from GPU memory so that video and animation cost no CPU copy.

**Priority:** P1
**Dependencies:** Blocked by US-032, US-034

**Acceptance Criteria:**
- [ ] Given a frame descriptor received, when the window imports it, then the page area draws it clipped to its area and under Paneflow's popups, as on the CPU path.
- [ ] Given a host that stops answering while a tab presents, when a test stalls the host, then the window keeps drawing terminal frames and no main-thread call is waiting on the host.
- [ ] Given the page area resized, when frames of the new size arrive, then the old import is released.
- [ ] Given the graphics device is lost, when it is recreated, then the tab resumes presenting without a restart.
- [ ] Given a format or modifier the device cannot import, when import fails, then the tab moves to the CPU path through the presentation state of US-034 and the failure reason is recorded.
- [ ] Given a tab hidden, when it is no longer displayed, then its imported textures are released.

#### US-034: Make the fallback explicit
**Description:** As a user, I want to know when the browser runs on the slower path and why.

**Priority:** P1
**Dependencies:** Blocked by US-025, US-021

**Acceptance Criteria:**
- [ ] Given the per-tab presentation state (GPU, CPU by setting, CPU by fallback with a reason), when it is read, then it is platform-neutral code with a unit test for each transition.
- [ ] Given a tab on the CPU path by fallback, when it is displayed, then the browser chrome shows an indicator whose tooltip states the reason.
- [ ] Given the setting `browser.gpu_presentation` set to off, when a tab is displayed, then it uses the CPU path and no indicator of failure is shown.
- [ ] Given `paneflow browser status --json`, when it runs, then it reports the presentation path of each tab and the last fallback reason.
- [ ] Given a simulated GPU import failure in a session, when another tab opens in that session, then it starts on the CPU path.

#### US-035: Measure both paths against the terminal
**Description:** As a maintainer, I want evidence that the browser does not slow the terminal so that no performance claim ships unmeasured.

**Priority:** P1
**Dependencies:** Blocked by US-033

**Acceptance Criteria:**
- [ ] Given the video fixture playing in the dock on the GPU path, when the A/B method of TD-17 is run in a debug build on a tagged home, then the three no-browser medians and the with-browser median are recorded in this PRD.
- [ ] Given the same video on the CPU path, when the method is run, then its medians are recorded beside the first.
- [ ] Given either run shows a regression, when the section is written, then the regression is stated in its first line and the affected path is disabled by default.
- [ ] Given the measurement, when the test tree is searched, then no unit test asserts a duration for it.

---

### EP-006: Runtime delivery

The engine as a separately built, signed, downloaded and updated component, with the Linux sandbox policy applied and the licenses audited.

**Definition of Done:** on Linux, a fresh install with no runtime shows the install prompt, downloads the runtime for its target, verifies its minisign signature, starts the host from the Paneflow home, later replaces it with a newer runtime while the application version is unchanged, and rolls back when the new host fails its handshake; a tampered archive is refused; `THIRD_PARTY_NOTICES.md` covers the CEF distribution.

#### US-036: Build and sign the runtime archive for Linux
**Description:** As a maintainer, I want the release pipeline to produce one signed runtime archive per Linux target so that users download exactly what CI built.

**Priority:** P0
**Dependencies:** Blocked by US-009, US-040

**Acceptance Criteria:**
- [ ] Given a runtime release, when the workflow finishes, then one archive per Linux target exists, named with the runtime version and the target, with a `.minisig` and a `.sha256` beside it.
- [ ] Given the archive, when it is unpacked, then it contains the host binary, the CEF libraries and resources it needs, and a manifest stating the runtime version, the CEF version and the protocol versions it speaks.
- [ ] Given the workflow, when `scripts/check-workflows.sh` runs, then it passes and `release.yml` stays within its line limit.
- [ ] Given a build whose host binary does not match the pinned CEF hash, when the workflow runs, then it fails before signing.

#### US-037: Install the runtime on first use
**Description:** As a user, I want one clear prompt that tells me the size and installs the browser engine so that nothing executable is fetched behind my back.

**Priority:** P0
**Dependencies:** Blocked by US-036, US-024, US-021

**Acceptance Criteria:**
- [ ] Given no runtime installed, when a browser tab is opened, then a prompt states what will be downloaded, its size in megabytes and its source, and nothing is downloaded before the user confirms.
- [ ] Given confirmation, when the download completes, then the archive is verified with `paneflow-minisign` against the release key before any file is unpacked.
- [ ] Given a verified archive, when it is unpacked, then it lands in a versioned directory under the Paneflow home and becomes current through an atomic rename.
- [ ] Given an archive whose signature does not verify, when verification fails, then nothing is unpacked, the partial file is deleted and the prompt shows the failure.
- [ ] Given the network unavailable or the download cancelled, when the prompt is shown again, then it offers to retry and no partial runtime is used.
- [ ] Given the download and unpack, when they run, then they run off the GPUI main thread.
- [ ] Given no runtime installed, when a browser tool is called, then its error names `paneflow browser install`; that command calls `browser.install`, which shows this story's prompt in the window, and nothing is downloaded before the human confirms there.

#### US-038: Update the runtime independently and roll back
**Description:** As a user, I want engine security fixes without waiting for an application release, and a working browser if an update is bad.

**Priority:** P0
**Dependencies:** Blocked by US-037

**Acceptance Criteria:**
- [ ] Given a newer runtime published whose manifest speaks the application's protocol version, when the update check runs, then it is downloaded, verified and staged while the current host keeps running.
- [ ] Given a staged runtime, when the host next starts, then the new version is used and the previous one is kept until the new host completes a handshake.
- [ ] Given a new host that fails its handshake, when the failure is observed, then the previous runtime becomes current again and the failure is reported once.
- [ ] Given a newer runtime whose manifest does not speak the application's protocol version, when the check runs, then it is skipped.
- [ ] Given an application update that needs a newer runtime, when a browser tab is opened, then the install prompt of US-037 is shown for that runtime.
- [ ] Given the update check fails or the download is interrupted, when the host next starts, then the current runtime is used unchanged and no partial runtime is staged.

#### US-039: Apply the Linux sandbox verdict
**Description:** As a user, I want the browser to refuse to run unsandboxed and to tell me how to fix it.

**Priority:** P0
**Dependencies:** Blocked by US-037, US-006, US-021

**Acceptance Criteria:**
- [ ] Given a system where the sandbox starts, when the host starts, then every renderer process is sandboxed and `paneflow browser status` says so.
- [ ] Given a system where the sandbox cannot start, when a browser tab is opened, then the host does not start and the tab shows the reason and the remedy recorded in the report of US-006.
- [ ] Given any release build, when the host's command line is inspected in a test, then it carries no flag that disables the sandbox.
- [ ] Given the `.deb` and `.rpm` packages, when the remedy of US-006 is a file a package can install, then the packages install it and the row of US-006 is re-run and recorded.

#### US-040: Audit licenses and write the notices
**Description:** As a maintainer, I want the native engine's licenses checked against GPL-3.0-or-later so that the runtime is distributable.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [ ] Given the pinned CEF distribution, when its license files are read, then every component license is listed in `THIRD_PARTY_NOTICES.md` with its compatibility per the FSF license list.
- [ ] Given a component whose license is not GPL-compatible, when the audit finds it, then the story is `BLOCKED` and names the component.
- [ ] Given the distribution's build flags, when codec support is checked, then the notices state which media codecs the runtime includes.
- [ ] Given the runtime archive, when it is unpacked, then it contains the license texts its components require to accompany binaries.

#### US-041: Document the boundary
**Description:** As a contributor, I want the engine boundary written where agents and humans read it so that nobody links Chromium into the app by accident.

**Priority:** P1
**Dependencies:** Blocked by US-038

**Acceptance Criteria:**
- [ ] Given `ARCHITECTURE.md`, when it is read, then a section describes the host process, the private protocol, the two presentation paths, internal CDP and the runtime's delivery.
- [ ] Given `AGENTS.md`, when its boundaries section is read, then it states that the web engine lives only in `crates/paneflow-browser-host`, how to fetch CEF, and the host crate's own gates.
- [ ] Given `CHANGELOG.md`, when the unreleased section is read, then it describes the browser, the runtime download and the `browser` right in user terms.
- [ ] Given the documents, when they are searched, then none names a path under `tasks/` as a source without stating it is local evidence.
- [ ] Given a platform whose validation epic is not `DONE`, when the documents describe the browser there, then they say the behavior is not yet validated on that platform.

---

### EP-007: macOS and Windows code paths

Everything macOS and Windows need, written by inspection and compiled by their CI legs. Every criterion of this epic is proven by reading the source, by a platform-neutral unit test, or by a CI leg building, linting or packaging. No criterion here needs a Mac or a Windows machine in hand: the behaviors are proven in EP-008 and EP-009.

**Definition of Done:** the macOS and Windows CI legs of `run_tests.yml` build and lint the window, the protocol crate and the host crate for their targets with `-D warnings`; the runtime workflow produces a signed archive for `aarch64-apple-darwin` and for `x86_64-pc-windows-msvc`; every file with a `mod tests` keeps its platform-gated items before that module; assumption A12 is marked for both targets.

#### US-042: Lay out, sign and notarise the macOS runtime
**Description:** As a maintainer, I want the macOS runtime built as a signed, notarised bundle with its helper applications so that Gatekeeper and the Chromium sandbox have what they require.

**Priority:** P0
**Dependencies:** Blocked by US-036

**Acceptance Criteria:**
- [ ] Given the macOS runtime archive produced by CI, when it is unpacked on the CI runner, then it contains a host application bundle with the CEF framework and the helper applications CEF requires.
- [ ] Given the release workflow for macOS, when it runs, then every Mach-O in the bundle is signed inside-out with the hardened runtime and the bundle is notarised by submit and poll, as `docs/release/macos-signing.md` prescribes.
- [ ] Given each helper's `main`, when its source is read, then it initialises the CEF sandbox library before anything else.
- [ ] Given a bundle with one unsigned Mach-O, when the workflow's verification step runs, then the workflow fails.

#### US-043: Write the macOS frame transport
**Description:** As a maintainer, I want the macOS branches of both presentation paths written so that EP-008 has code to prove.

**Priority:** P0
**Dependencies:** Blocked by US-025, US-034

**Acceptance Criteria:**
- [ ] Given the macOS source of the CPU path, when it is read, then the host writes frames into a shared-memory segment and the window draws them with `paint_image`, and the code outside `cfg(target_os = "macos")` is the code Linux uses.
- [ ] Given the macOS source of the GPU path, when it is read, then the host copies each IOSurface frame into a pool it owns, transfers it to the window, and the window wraps it in a `CVPixelBuffer` painted with `paint_surface`.
- [ ] Given the platform-neutral fallback logic, when its unit test simulates a transfer failure, then the tab state moves to the CPU path with the reason recorded.
- [ ] Given the macOS CI leg, when it builds the window and the host, then it passes clippy with `-D warnings`.

#### US-044: Write the Windows host bootstrap and transport
**Description:** As a maintainer, I want the Windows host started the way the CEF sandbox requires and reachable over a named pipe.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [ ] Given the Windows build of the host on the CI leg, when its artifacts are listed, then the client is a DLL exporting the entry point CEF's bootstrap executable calls, and the bootstrap executable is beside it.
- [ ] Given the window's Windows source, when it is read, then the pipe is created with a security descriptor limited to the current user and the host is assigned to a kill-on-close Job Object (TD-16).
- [ ] Given the window's Windows source, when it checks the caller of the pipe, then it calls the `paneflow-peer` path Windows already has.
- [ ] Given the Windows CI leg, when it builds the window and the host for `x86_64-pc-windows-msvc`, then it passes clippy with `-D warnings`.
- [ ] Given a file that already has a `mod tests`, when a Windows-gated item is added to it, then the item is declared before that module.
- [ ] Given the bootstrap executable is absent from the runtime directory, when the platform-neutral start logic is unit-tested with a missing path, then it returns an error naming the missing file and starts nothing.

#### US-045: Vendor `gpui_windows` with the external-texture seam
**Description:** As a maintainer, I want the Windows renderer to draw an externally backed image so that a shared D3D11 texture can be presented without changing the `gpui` core crate.

**Priority:** P0
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [ ] Given the root `Cargo.toml`, when its patch section is read, then `gpui_windows` is patched to `vendor/gpui_windows`, vendored at the pinned GPUI revision with a manifest header naming the change and when to drop it.
- [ ] Given the source of `vendor/gpui_windows`, when it is read, then an image id registered as externally backed makes the DirectX atlas return a tile over the registered texture without calling the build closure, as TD-7 describes, with a registration call of the same shape as the Linux one where US-031 exists.
- [ ] Given `cargo fmt --all --check`, when it runs, then the vendored copy passes with its upstream `rustfmt.toml`.
- [ ] Given the Windows CI leg, when it builds with the vendored crate, then it passes clippy with `-D warnings`.
- [ ] Given `git diff` between the vendored crate and the upstream crate at the pinned revision, when it is read, then it touches only the manifest and the seam.

#### US-046: Write the Windows frame transport
**Description:** As a maintainer, I want the Windows branches of both presentation paths written so that EP-009 has code to prove.

**Priority:** P0
**Dependencies:** Blocked by US-044, US-045, US-034

**Acceptance Criteria:**
- [ ] Given the Windows source of the GPU path, when it is read, then the host copies each frame into a shared D3D11 texture it owns, duplicates its handle into the window process, and the window opens it on GPUI's device and registers it through the seam of US-045.
- [ ] Given the Windows source of the CPU path, when it is read, then frames travel through a file mapping and are drawn with `paint_image`.
- [ ] Given the platform-neutral fallback logic, when its unit test simulates a handle that cannot be opened, then the tab state moves to the CPU path with the reason recorded.
- [ ] Given the Windows CI leg, when it builds the window and the host, then it passes clippy with `-D warnings`.

#### US-047: Build the macOS and Windows runtime archives and their trust checks
**Description:** As a user on macOS or Windows, I want the downloaded runtime checked by the platform's own trust mechanism in addition to minisign.

**Priority:** P0
**Dependencies:** Blocked by US-042, US-044, US-046, US-038

**Acceptance Criteria:**
- [ ] Given a runtime release, when the workflows finish, then a signed archive exists for `aarch64-apple-darwin` and for `x86_64-pc-windows-msvc`, each with its `.minisig`.
- [ ] Given the macOS source of the install path, when it is read, then the code signature's Team ID is compared with the one the application pins before anything is unpacked.
- [ ] Given the Windows source of the install path, when it is read, then each executable is passed to `WinVerifyTrust` and its publisher compared with the one the application pins before anything is unpacked.
- [ ] Given the platform-neutral install logic, when its unit test feeds an archive that passes minisign and a platform check that fails, then nothing is unpacked.
- [ ] Given the Windows release workflow, when it signs the runtime, then it uses the signing setup of `docs/release/windows-signing.md`.

---

### EP-008: macOS tests and validation

Everything that needs a Mac, run in one pass on Apple Silicon hardware. No Linux or Windows check belongs here.

**Definition of Done:** each story below is proven on an Apple Silicon Mac running a notarised build, and this PRD contains a section "macOS validation report" with one dated entry per story naming the machine, macOS version and build; every defect found is fixed in this epic or recorded as a blocker on the story it belongs to; assumptions A10 and A13 are marked for macOS.

#### US-048: Install and start the runtime on macOS
**Description:** As a macOS user, I want the browser engine to install and start sandboxed from a notarised download.

**Priority:** P0
**Dependencies:** Blocked by US-047

**Acceptance Criteria:**
- [ ] Given a notarised Paneflow build with no runtime, when a browser tab is opened and the prompt confirmed, then the runtime downloads, passes minisign and the Team ID check, and the host starts.
- [ ] Given the running host, when its processes are inspected, then the renderer helpers run under the Chromium sandbox and `paneflow browser status` says so.
- [ ] Given the runtime bundle, when `spctl --assess` and `codesign --verify --deep --strict` run on it, then both exit zero.
- [ ] Given a runtime archive altered after signing, when it is installed, then installation is refused, nothing is unpacked and the report records the message shown.

#### US-049: Drive the browser from an agent on macOS
**Description:** As a macOS user, I want an agent to check a local application through the browser tools.

**Priority:** P0
**Dependencies:** Blocked by US-048

**Acceptance Criteria:**
- [ ] Given the tracked fixture application served on loopback on the Mac, when an agent in a pane uses the `browser.*` MCP tools, then it reports the planted console error and failing request.
- [ ] Given the host running, when its listening sockets are listed, then no DevTools endpoint exists.
- [ ] Given a pane without the `browser` right, when it calls a browser tool, then the call is refused.
- [ ] Given an agent navigating to an external origin, when no decision exists, then the site approval is raised and the navigation is held.

#### US-050: Use the surface on macOS
**Description:** As a macOS user, I want to browse, type and use web controls in the dock and in a pane.

**Priority:** P0
**Dependencies:** Blocked by US-048, US-043

**Acceptance Criteria:**
- [ ] Given a Retina display, when the fixture form page is displayed on the CPU path, then `paneflow browser status` reports device scale factor 2 for the tab and the page is clipped to its area with a Paneflow popup covering it.
- [ ] Given a Japanese or Chinese input method, when text is composed and committed in a page field, then the field holds the committed text.
- [ ] Given a `<select>`, a JavaScript dialog, a new-window link and a file input, when each is used, then each meets its criterion of US-027.
- [ ] Given the page area focused, when Paneflow's pane and tab shortcuts using the Command key are pressed, then Paneflow handles them.
- [ ] Given the window moved between a Retina display and an external display of scale factor 1, when a page is displayed, then the reported device scale factor follows.
- [ ] Given the host killed while a page is displayed, when the tab is observed, then it shows the crashed state and a reload starts a new host.

#### US-051: Present through IOSurface on macOS
**Description:** As a macOS user, I want the page drawn from GPU memory, with the labelled fallback when it cannot be.

**Priority:** P1
**Dependencies:** Blocked by US-050, US-034

**Acceptance Criteria:**
- [ ] Given the video fixture playing in a displayed tab, when `paneflow browser status` is read, then it reports the GPU path, and two screenshots one second apart show the video advanced with no black region.
- [ ] Given the page area resized during playback, when frames are observed, then no frame of the previous size is shown stretched.
- [ ] Given the GPU path forced to fail through `browser.gpu_presentation`, when the tab is displayed, then it uses the CPU path, and a simulated transfer failure shows the indicator of US-034.
- [ ] Given the A/B latency method of TD-17 on the Mac, when it is run with the video fixture on the GPU path and again on the CPU path, then both sets of medians are recorded, and A10 and A13 are marked for macOS.

#### US-052: Update and roll back the runtime on macOS
**Description:** As a macOS user, I want runtime updates and application updates to coexist.

**Priority:** P0
**Dependencies:** Blocked by US-048

**Acceptance Criteria:**
- [ ] Given a newer runtime published, when the check runs on the Mac, then it is staged, used at the next host start and the previous one removed after a handshake.
- [ ] Given a staged runtime whose host fails its handshake, when the failure is observed, then the previous runtime is current again.
- [ ] Given a `.dmg` application update installed while a runtime is present, when Paneflow restarts, then the runtime is still present and a browser tab loads the fixture application.
- [ ] Given the "macOS validation report", when it is read, then it has an entry for each of US-048 to US-052.

---

### EP-009: Windows tests and validation

Everything that needs Windows, run in one pass on Windows 11 x64 after EP-008. No Linux or macOS check belongs here.

**Definition of Done:** each story below is proven on Windows 11 x64 with a signed build, and this PRD contains a section "Windows validation report" with one dated entry per story naming the machine, Windows build and GPU; every defect found is fixed in this epic or recorded as a blocker on the story it belongs to; assumptions A9 and A13 are marked for Windows.

#### US-053: Install and start the runtime on Windows
**Description:** As a Windows user, I want the browser engine to install and start sandboxed from a signed download.

**Priority:** P0
**Dependencies:** Blocked by US-047, US-052

**Acceptance Criteria:**
- [ ] Given a signed Paneflow build with no runtime, when a browser tab is opened and the prompt confirmed, then the runtime downloads, passes minisign and `WinVerifyTrust`, and the host starts through the bootstrap executable.
- [ ] Given the running host, when its renderer processes are inspected, then they run under the Chromium sandbox, and A9 is marked.
- [ ] Given the sandbox cannot start, when a browser tab is opened, then the host does not start and the tab states the reason.
- [ ] Given the window closed, when the process list is read, then no host or renderer process remains.
- [ ] Given a runtime archive altered after signing, when it is installed, then installation is refused, nothing is unpacked and the report records the message shown.

#### US-054: Drive the browser from an agent on Windows
**Description:** As a Windows user, I want an agent to check a local application through the browser tools.

**Priority:** P0
**Dependencies:** Blocked by US-053

**Acceptance Criteria:**
- [ ] Given the tracked fixture application served on loopback on Windows, when an agent in a pane uses the `browser.*` MCP tools, then it reports the planted console error and failing request.
- [ ] Given the host running, when its listening sockets are listed, then no DevTools endpoint exists.
- [ ] Given a pane without the `browser` right, when it calls a browser tool, then the call is refused.
- [ ] Given the named pipe of the host, when a process of another user account tries to connect, then the connection is refused.

#### US-055: Use the surface on Windows
**Description:** As a Windows user, I want to browse, type and use web controls in the dock and in a pane.

**Priority:** P0
**Dependencies:** Blocked by US-053, US-046

**Acceptance Criteria:**
- [ ] Given a display at 150 percent scaling, when the fixture form page is displayed on the CPU path, then `paneflow browser status` reports device scale factor 1.5 for the tab and the page is clipped to its area with a Paneflow popup covering it.
- [ ] Given a Windows input method, when text is composed and committed in a page field, then the field holds the committed text.
- [ ] Given a `<select>`, a JavaScript dialog, a new-window link and a file input, when each is used, then each meets its criterion of US-027.
- [ ] Given the page area focused, when Paneflow's pane and tab shortcuts are pressed, then Paneflow handles them.
- [ ] Given the window moved between two monitors with different scaling, when a page is displayed, then the reported device scale factor follows.
- [ ] Given the host killed while a page is displayed, when the tab is observed, then it shows the crashed state and a reload starts a new host.

#### US-056: Present through a shared D3D11 texture on Windows
**Description:** As a Windows user, I want the page drawn from GPU memory, with the labelled fallback when it cannot be.

**Priority:** P1
**Dependencies:** Blocked by US-055, US-034

**Acceptance Criteria:**
- [ ] Given the video fixture playing in a displayed tab, when `paneflow browser status` is read, then it reports the GPU path, and two screenshots one second apart show the video advanced with no black region.
- [ ] Given the page area resized during playback, when frames are observed, then no frame of the previous size is shown stretched.
- [ ] Given the GPU path forced to fail through `browser.gpu_presentation`, when the tab is displayed, then it uses the CPU path, and a simulated handle failure shows the indicator of US-034.
- [ ] Given the A/B latency method of TD-17 on Windows, when it is run with the video fixture on the GPU path and again on the CPU path, then both sets of medians are recorded and A13 is marked for Windows.
- [ ] Given the graphics device removed and restored, when the tab is displayed again, then it presents without a restart of Paneflow.

#### US-057: Update and roll back the runtime on Windows
**Description:** As a Windows user, I want runtime updates and application updates to coexist.

**Priority:** P0
**Dependencies:** Blocked by US-053

**Acceptance Criteria:**
- [ ] Given a newer runtime published, when the check runs on Windows, then it is staged, used at the next host start and the previous one removed after a handshake.
- [ ] Given a staged runtime whose host fails its handshake, when the failure is observed, then the previous runtime is current again.
- [ ] Given an `.msi` application update installed while a runtime is present, when Paneflow restarts, then the runtime is still present and a browser tab loads the fixture application.
- [ ] Given a runtime file locked by a running host, when an update is staged, then the running host is not interrupted and the swap happens at the next host start.
- [ ] Given the "Windows validation report", when it is read, then it has an entry for each of US-053 to US-057.

## Edge Cases & Error States

| Scenario | Trigger | Expected behavior | User message |
|----------|---------|-------------------|--------------|
| Host crash | Chromium fault | Tabs report crashed; terminals unaffected; reload starts a new host | "The browser engine stopped. Reload to restart it." |
| Runtime missing | Browser tab or tool used before install | No host starts; prompt in the UI, explicit error to tools | "The browser engine is not installed (size shown). Install?" |
| Sandbox unavailable | System policy forbids it | Host refuses to start | The reason and the remedy recorded by US-006 |
| GPU import failure | Unsupported format, driver fault, device loss | Tab switches to the CPU path with an indicator | "Showing this page through the compatibility path: (reason)." |
| Stale element reference | Agent acts after a navigation | Call refused | Tool error: reference stale, take a new snapshot |
| Human takeover | Human interacts with an agent-owned tab | Ownership moves to the human; agent calls refused | Tool error naming the takeover |
| Undecided external origin | Agent or redirect leaves loopback | Navigation held; approval raised | The agent, the origin, three choices |
| Oversized snapshot | Very large page | Truncated at the bound with the dropped amount stated | None; stated in the tool result |
| Profile locked | Second host on the same profile | Open refused | "This workspace's browser profile is in use." |
| Protocol mismatch | Application and runtime versions differ | Handshake refused; previous runtime restored or install prompt shown | "The browser engine needs an update." |
| Download interrupted | Network loss during install | Partial file discarded; retry offered | "Download interrupted. Retry?" |

## Risks & Mitigations

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| The Linux GPU path fails on the development machine or on NVIDIA | Medium | Medium | US-002 and US-003 run first; the CPU path is built before it and stays as fallback (TD-6); EP-005 can be cancelled without touching the rest. |
| NVIDIA is not covered by the development machine | Medium | Medium | The fallback is automatic (US-033, US-034); the reports name the GPU tested so the gap stays visible. |
| The Chromium sandbox cannot start on a Linux format or on Windows | Medium | High | Surveyed in US-006 and proven on Windows in US-053; the browser refuses to start there (TD-11); packages install the remedy where one exists (US-039). |
| Chromium security fixes are not shipped in time | Medium | High | The runtime updates independently of the application (US-038); the pin and its hash live in one manifest (US-001). |
| Prompt injection through page content steers an agent | High | High | Clean profile (US-012), loopback by default with per-site approval (US-013, US-020), fenced page text (US-016), evaluation behind approval (US-019), approvals the agent cannot answer (US-020). The problem is not solved (R20); the embedded browser is scoped to checking what is being built. |
| The `cef` crate lags CEF or changes its API | Medium | Medium | All use is confined to the host crate (TD-2, TD-3); CEF's C API remains available. |
| A GPUI bump breaks the vendored seams | High | Medium | Each vendor manifest header names the change (US-031, US-045); the seam is one registration call and one atlas branch. |
| The atlas seam is not feasible | Low | High | Proven in a spike before product code (US-003); failure becomes an open question before EP-005 (A2). |
| The CEF distribution carries an incompatible license | Low | High | Audited before any runtime is published (US-040). |
| The runtime download is refused by users or blocked offline | Medium | Low | The prompt states size and source (US-037); agents receive an explicit error (US-021); the rest of Paneflow is unaffected. |

## Open Questions

- **Runtime host location.** TD-10 assumes runtime archives are published as assets of GitHub releases in this repository, under their own tags. Whether Arthur wants a separate repository or a CDN for them changes only US-036 and the update check of US-038.
- **Upstreaming the external-texture seam to Zed.** TD-9 does not depend on it. Whether to propose it, and when, is Arthur's call once US-031 and US-045 exist.
- **The NO-GO fallback.** If US-007 records NO-GO for the control plane, the alternative is an external Chrome over a pipe (TD-1). That would be a new PRD version, not a story swap.
- **Link routing default.** US-029 keeps the system browser as the default for loopback links. Changing the default once the browser has been used for a release is a product decision.

## Files NOT to Modify

- `native/libghostty/**`: the terminal engine boundary, unrelated.
- `protocol/api-v1.json`: generated from the catalog; regenerate it, never edit it by hand.
- `docs/user/**`: a mirror synced from the site repository.
- `crates/paneflow-shim/**` and `crates/paneflow-ai-hook/**`: size-capped helpers with no browser role.
- `crates/paneflow-host/src/sandbox/**`: the pane sandbox; the browser host is a child of the window and is not a pane.
- `vendor/cosmic-text/**`: unrelated to the browser seam.
- `vendor/gpui_wgpu/**` and `vendor/gpui_linux/**` outside the external-texture seam of US-031: upstream copies whose changes follow their `Cargo.toml` headers only.
- `scripts/wallclock-allowlist.txt`: an entry may only go down.
- `/home/arthur/dev/paneflow-browser/**`: an abandoned worktree, to be removed before work starts; never read or copied.
[/PRD]
