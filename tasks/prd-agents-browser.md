[PRD]
# PRD: A browser inside Paneflow that an agent can drive

## Changelog

| Version | Date | Author | Summary |
|---------|------|--------|---------|
| 1.0 | 2026-09-05 | Arthur Jean | Initial PRD (French): CEF and GPU qualification, native browser in the dock, distribution on four targets, agent interactions as the last delivery. 32 stories, none started. |
| 2.0 | 2026-11-24 | Arthur Jean | Full rewrite from the research of 2026-11-24 (`tasks/research/agent-browser-architecture-2026-11-24.md`, local evidence) and a reading of Zeron and t3code. Same engine choice (CEF in its own host process, off-screen rendering), four changes: agent control is proven before the GPU path, an explicit CPU presentation path exists, the engine ships as a separately signed runtime, and Windows is recognised as the most expensive GPUI integration. The GPUI pin cited by v1.0 (`fecc3273`) is replaced by the current one (`c016f436`; the research read `703b5ac3`, and R1 to R3 were re-read at `c016f436` after `main` moved the same day). Every macOS test and validation story sits in a final epic, EP-008; every Windows test and validation story sits in a final epic after it, EP-009. 9 epics, 57 stories. Version 1.0 is in git history. |
| 2.1 | 2026-11-25 | Arthur Jean | EP-001 run: "Kill test report" added. GO for the control plane; GO to build for the CPU and Linux GPU paths, which regress terminal latency on a native Wayland window (A3 failed there, holds on X11). A3 requires a revision; the external-Chrome fallback it names is rejected on the diagnostic evidence, pending Arthur's ratification (Open Questions), and US-025 and US-035 gain a latency gate on a native Wayland window with nine no-browser runs. Assumptions A1 to A5, A7, A8 and A15 marked. No story cancelled. |
| 2.2 | 2026-11-25 | Arthur Jean | Before EP-004: Arthur ratified the rejection of A3's external-Chrome fallback (an external Chrome carries both measured causes, the redraw at the video's frame rate and Chromium's own load, and displays worse) and closed that open question. The US-025 latency gate was corrected before its run: the diagnostic campaign's "Chromium only, no redraw" medians (2.973 to 2.980 ms) already exceed the largest no-browser median (2.940 ms), so a gate against a no-browser arm fails for any display path and turning the display off does not remove that load. The control arm is now the host playing the video with its tab not displayed, and the gate judges the 99th percentile as well as the median. US-035 keeps its wording until EP-005. |
| 2.3 | 2026-11-26 | Arthur Jean | Additions at Arthur's request after the EP-004 manual pass, each recorded in "EP-004 review": Helium's location bar, loading line, omnibox and Site information; the Browser settings page after Zeron; the guard now follows the tab's owner (a tab a person drives holds no site); `browser.display` removed; Zeron's element picker behind the Pointer button, sending picks straight to the chosen agent's prompt; Chromium's developer tools docked on F12. The Non-Goal "DevTools for the human" is withdrawn. |

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

Each workspace has its own clean browser profile. Loopback origins are reachable without a prompt; in a tab an agent drives, every other origin needs a per-site decision from the user (a tab a person drives opens any site, changed at Arthur's request on 2026-11-25).

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

| ID | Assumption | Risk | Validated by | If it fails | Result |
|----|-----------|------|--------------|-------------|--------|
| A1 | CEF delivers dmabuf frames under native Wayland and under X11 on the development machine. | High | US-002 | The Linux GPU path is NO-GO for this version; EP-005 is cancelled and the CPU path ships alone. | Confirmed: Wayland, and X11 with `--use-angle=vulkan` ([US-002](#us-002-result)) |
| A2 | A frame exported by the host can be imported by the vendored `gpui_wgpu` through the atlas seam of TD-7, clipped to its rectangle and covered by a popup drawn above it. | High | US-003 | US-007 records the failure; the alternative (patching `gpui` core) is put to Arthur as an open question before EP-005. | Confirmed ([US-003](#us-003-result)) |
| A3 | Playing video in a browser tab causes no terminal latency regression, as TD-17 defines one, on either path. | High | US-003, US-005 | The path that regresses does not ship. If both regress, the display falls back to the external Chrome of TD-1 and the PRD is revised. | Confirmed on an X11 window at 60 and 144 Hz; failed on a native Wayland window for both paths ([Latency A/B](#latency-ab)) |
| A4 | Every CDP domain the tools need (Accessibility, DOM, Input, Runtime, Network, Page, Log) answers in windowless mode with no port open and with the tab not displayed. | High | US-004 | The missing capability is listed in US-007; if a core one is missing, control moves to an external Chrome over a pipe and the PRD is revised. | Confirmed; agent-driven tabs must not be marked hidden ([US-004](#us-004-result)) |
| A5 | The Chromium sandbox starts on the Linux install formats Paneflow ships. | High | US-006 | The formats that fail refuse to start the browser (TD-11) and are documented. | Confirmed on Fedora 45, Ubuntu 22.04, Debian 13, Arch, openSUSE Leap 16; failed on Ubuntu 24.04 without an AppArmor profile ([US-006](#us-006-result)) |
| A6 | The CEF distribution contains no component whose license forbids distribution next to a GPL-3.0-or-later program. | Medium | US-040 | The offending component is removed from the runtime or the runtime is not published. | Open |
| A7 | A snapshot bounded at 32 KiB of text is enough for an agent to locate the defects of the fixture application. The bound comes from no measurement (t3code truncates near 20 KB). | Medium | US-004, US-016 | The bound changes to the value US-004 measures. | Confirmed for the fixture (at most 1,881 bytes); real pages need the truncation ([US-004](#us-004-result)) |
| A8 | The runtime archive fits under the 500 MiB download cap that application artifacts already respect. | Medium | US-001 | The cap for the runtime is set from the measured size and recorded. | Confirmed: largest runtime 162 MiB compressed ([US-001](#us-001-result)) |
| A9 | The CEF sandbox can run on Windows from a bootstrap executable placed outside the application's install directory. It cannot be tested earlier: Windows validation is last by decision. | High | US-053 | The browser is disabled on Windows (TD-11) until it can. | Open |
| A10 | An IOSurface can be transferred from the host to the window and painted by `paint_surface` on macOS. | Medium | US-051 | macOS ships the CPU path only. | Open |
| A11 | The initial limits of the "Limits" table below suit real use. Only the URL length has a source. | Low | US-004, US-022 | A limit changes to the measured need and the table is updated. | Open |
| A12 | The `cef` crate 156.7.0 builds for all four shipping targets. | Medium | US-009, US-036, EP-007's Definition of Done | The target that fails has no runtime until the binding is fixed or replaced by the C API. | Open |
| A13 | The CPU path causes no latency regression on macOS and Windows. It is measured on Linux only before the final epics. | Medium | US-051, US-056 | The CPU path is not the default on that platform; if the GPU path also fails there, the browser is agent-only on it. | Open |
| A14 | The Linux paths behave on `aarch64-unknown-linux-gnu` as on x86_64. CI builds the aarch64 runtime; no story tests it on aarch64 hardware. | Medium | Not validated in this PRD | The automatic fallback of US-033 and US-034 applies; a defect report opens a follow-up. | Open |
| A15 | Three no-browser runs bound run-to-run noise well enough for TD-17 to discriminate. | Low | US-003 | If the three medians spread wider than the difference being judged, the report says so and the number of runs is raised and recorded. | Failed: three runs did not discriminate; raised to nine ([Latency A/B](#latency-ab)) |

### Limits

Each limit is one named constant in `paneflow-browser-proto`. The values are initial and are assumptions (A11), except the URL length.

| Limit | Initial value | Source |
|-------|---------------|--------|
| URL length, from an agent, a person or a page | 2097152 characters (2 MiB), shown up to 32768 in the address field | Chromium's `url::kMaxURLChars` and `kMaxURLDisplayChars`, which Helium keeps; changed from t3code's 2048 at Arthur's request on 2026-11-26, since he often gives agents longer URLs |
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
- **DevTools for the agent.** The developer tools docked on F12 (v2.3) are for the human; agents keep the semantic tools (TD-5) and never reach the inspector's session.
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

## Kill test report

Raw evidence, logs, screenshots and spike code are in `tasks/evidence/agents-browser/` (local evidence, not tracked; `NOTES.md` there indexes it). Every run below used the same machine unless a row says otherwise.

**Machine.** Fedora 45 Workstation, kernel 7.3.4-300.fc45.x86_64, GNOME Shell 51.1 (mutter 51.1, Wayland session), XWayland 24.1.15. GPU: NVIDIA GeForce RTX 4070 Ti SUPER, driver 620.70.06 (open kernel module), driving both monitors; an AMD Radeon 610M iGPU (Mesa 26.2.7) is present and unused. **CEF** 156.0.31+ge603d97+chromium-156.0.8078.100, Rust binding `cef` 156.7.0+156.0.31, wgpu 30.0.2 on the spike's import side.

### Verdict (US-007)

| Path | Verdict | Deciding evidence |
|------|---------|-------------------|
| Control plane (internal CDP, no port) | **GO** | Every needed domain answers in a windowless browser with no port open, and a real agent found both planted defects and the element from text alone (US-004). One constraint: agent-driven tabs are never marked hidden. |
| CPU path | **GO to build; not shippable as measured on a native Wayland window** | The page displays at the dock's default width at 30 frames per second (US-005). Terminal latency does not regress when Paneflow's window is an X11 client, at 60 or 144 Hz; it regresses when the window is a native Wayland client (A3 failed there). |
| Linux GPU path | **GO to build; not shippable as measured on a native Wayland window** | A1 and A2 hold: frames are shared under Wayland and X11 (X11 needs `--use-angle=vulkan`), cross two processes as dmabuf into the vendored atlas without a copy, clip and stack correctly (US-002, US-003). Latency: same result as the CPU path, for the same causes. |
| Linux sandbox | Confirmed on 5 of 6 systems | Ubuntu 24.04 refuses without an AppArmor profile; the remedy is known (US-006). |

A GPU NO-GO in the sense of US-007 (the path cannot work: A1 or A2 failed) did not happen, so US-031 to US-035 stay. A3 failed for both paths on a native Wayland window. Its rule says the path that regresses does not ship and, when both regress, the display falls back to the external Chrome of TD-1 and the PRD is revised. The fallback is rejected on evidence: the diagnostic runs below split the regression into Chromium's own load, which an external Chrome would carry too, and the window redrawing at the video's frame rate, which a screencast display would need too. The revision made instead (changelog 2.1): US-025 and US-035 carry the latency gate on a native Wayland window, with nine no-browser runs, and a path that fails it is off by default. Agent control has no display and is not affected.

US-007 therefore cancels no story and blocks no epic. The GPU path was tested on one NVIDIA GPU only; AMD and Intel GPUs rely on the automatic fallback of US-034 until a machine with one runs it.

### US-001 result

Run 2026-11-24, re-checked 2026-11-25.

- `native/cef/manifest.toml` pins CEF 156.0.31+ge603d97+chromium-156.0.8078.100 (stable channel), the crate `cef` 156.7.0+156.0.31 (tag `cef-v156.7.0+156.0.31` of tauri-apps/cef-rs), and the `minimal` archive of each shipping target by URL and SHA-256. Each archive's SHA-1 equals the one published in the CDN index (`cef-builds.spotifycdn.com/index.json`).
- `scripts/fetch-cef.sh` run against an empty copy of the manifest downloaded the x86_64 Linux archive, checked its hash and placed it under `prebuilt/x86_64-unknown-linux-gnu/` (exit 0). A wrong hash exits 1 and leaves no archive in place; `scripts/test-ci-scripts.sh` runs both cases on a fixture (6 CEF cases, 208 in total, all pass).
- `/native/cef/prebuilt/` is ignored: `git status` after a fetch lists no file under it.
- Sizes, in bytes. "Archive" is the pinned download; "runtime" is what a runtime archive would carry (the `Release` and `Resources` directories, no static libraries, `libcef.so` stripped of debug information), compressed with `tar | zstd -12`. The host binary is not included.

| Target | Archive | Unpacked archive | Runtime unpacked | Runtime compressed |
|--------|---------|------------------|------------------|--------------------|
| x86_64-unknown-linux-gnu | 333,321,290 | 1,597,439,662 | 578,777,850 | 157,943,294 (151 MiB) |
| aarch64-unknown-linux-gnu | 431,212,107 | 2,740,632,426 | 672,830,622 | 169,784,001 (162 MiB) |
| aarch64-apple-darwin | 134,916,866 | 353,036,893 | 341,808,663 | 129,024,162 (123 MiB) |
| x86_64-pc-windows-msvc | 175,156,160 | 444,778,709 | 433,391,819 | 165,970,221 (158 MiB) |

The Linux archives are large because `libcef.so` ships with debug information (1,480,586,408 bytes on x86_64; 473,033,808 without debug information, as measured above; 278,330,520 with every symbol stripped, so the table is an upper bound). The largest runtime is 162 MiB, under the 500 MiB cap: A8 confirmed.

### US-002 result

Run 2026-11-24.

Spike `spike-gpu`: the `cef` crate's off-screen example with `accelerated_osr`, loading a WebGL animation, importing each frame into a wgpu Vulkan device and saving two screenshots one second apart.

| Backend | `OnAcceleratedPaint` | Frames | Screenshots one second apart |
|---------|----------------------|--------|------------------------------|
| `--ozone-platform=wayland` | Called, about 53 per second | 1 plane, BGRA_8888, DRM modifier `0x300000000606014` (NVIDIA block-linear); 0 import errors, 0 software paints | Frame 221 then 264: 0 black pixels, 24.1 % of pixels differ (animation advanced); `us002/wayland/wayland-shot1.png`, `wayland-shot2.png` |
| `--ozone-platform=x11`, default flags | Never called; `OnPaint` never called either | None in 8 seconds | None |
| `--ozone-platform=x11 --use-angle=vulkan`, sandbox on | Called, about 50 per second | Same format and modifier | Frame 219 then 261: 0 black pixels, 24.2 % of pixels differ (animation advanced); `us002/x11-angle-vulkan/x11-shot1.png`, `x11-shot2.png` |
| `--ozone-platform=x11`, shared textures off | Not requested | 346 software paints in 6 seconds | Not taken (CPU path control) |

The X11 failure with default flags is not a software fallback: no frame of any kind arrives. The GPU process logs `MESA-LOADER: failed to open dri: /usr/lib64/gbm/dri_gbm.so: ... Permission denied` (the GPU sandbox forbids the GBM driver) and then `gbm_support_x11.cc:133 Can't create buffer -- gbm device is missing.` A diagnostic run with the GPU sandbox disabled (never a shipping configuration) still produced no frame (`OzoneImageBacking::ProduceSkiaGanesh failed to create GL representation`). With `--use-angle=vulkan` the GL path through GBM is not used and frames arrive with every sandbox on.

Fixes in the pin: #4237 is fixed by commit `3adefb829c` on branch 8078 and #4313 by `57389f04a6` on branch 8078; both are ancestors of the pinned `e603d97`.

A1: confirmed under Wayland; confirmed under X11 on the condition that the host passes `--use-angle=vulkan` there (the default flags give no frame). Tested under XWayland, not under an Xorg server.

Side finding: CEF's Chrome runtime loaded the system-installed KDE Plasma Integration extension (`cimiefiiaegbelhefglklhhakcgmhkai`) into a fresh profile, and its content script made requests from fixture pages.

### US-003 result

Run 2026-11-24, latency re-measured 2026-11-25.

Spike: `spike-share` (host) and a throwaway copy of Paneflow at GPUI `703b5ac3` (window). The host copies each CEF shared texture into a pool of three Vulkan images it owns, exported as dmabuf with a DRM modifier from the driver's list, waits for the copy to complete, sends each slot's file descriptor once over a Unix socket (`SCM_RIGHTS`), then one "frame k is ready" message per frame; the window releases slots.

- **Import.** The window's copy of `vendor/gpui_wgpu` carries the seam of TD-7: `external_texture::register_dmabuf(ImageId, frame)` registers an image id; `WgpuAtlas::get_or_insert_with` returns, for a registered id, a polychrome tile covering the whole imported texture (index at or above `1 << 30`), and `get_texture_info` resolves it. The device requests `VULKAN_EXTERNAL_MEMORY_DMA_BUF`. Each slot is imported once and sampled in place: no copy per frame in the window. 0 import failures.
- **Clip and popup.** The page paints inside a rectangle smaller than the window, clipped by a content mask; a GPUI element drawn after it covers it (`us003/gpu-window-1.png`). A2 confirmed.
- **Resize while a video plays.** Six window resizes produced 7 pool generations and 11 stale-size frames in total (about two per resize), each drawn at its own size, clipped or with a gap, never stretched. No tearing in 12 captures (`us003/resize-sheet.png`). A fully black capture 50 ms after a resize is GPUI's own X11 resize, also seen without a browser.
- **Frame rate.** The clip is 30 frames per second; 30 arrive and 30 are painted per second.
- **Spike shortcuts** that product code must not keep: no queue-family ownership transfer on the exported images, and the window wraps an import with an initial state of `RESOURCE`.

Latency: see "Latency A/B" below.

### US-004 result

Run 2026-11-24.

Spike `spike-cdp`: a windowless browser created on `about:blank`, `WasHidden(true)`, then navigated to the fixture with `Page.navigate`; CDP through `SendDevToolsMessage` and a `DevToolsMessageObserver` (the same machinery as `ExecuteDevToolsMethod`); a Unix socket to a throwaway Python MCP server exposing five tools (open, snapshot, console, network, click).

- **Domains, tab hidden.** Accessibility (`getFullAXTree`), DOM (`getDocument`), Input (`dispatchMouseEvent`), Runtime (`evaluate`), Network (`getCookies`), Page (`getNavigationHistory`) and Log (`clear`) all answer. Events arrive: `Runtime.exceptionThrown`, `Runtime.consoleAPICalled`, `Log.entryAdded`, `Network.*`, `Page.*`.
- **Input gap.** On a tab marked hidden with `WasHidden(true)`, `Input.dispatchMouseEvent` answers `{}` but the page receives nothing: a press and release on the submit button, whose listener adds 100 to a counter, leaves the counter at 0, and `document.visibilityState` is `hidden`. The same tab not marked hidden, still never displayed and with no frame consumer, reads 100. Every other domain behaves the same either way.
- **Ports.** Process tree of 10 processes, 0 listening TCP sockets, no remote-debugging switch in any command line, no `DevToolsActivePort` file.
- **Real agent.** Claude Code 2.1.331 (model `claude-opus-5-5`), headless (`-p`), built-in tools disabled, only the five spike tools. Run 1 (tab hidden) named the `/api/prices` 500, the `TypeError` at line 14 and the paragraph "Total: unknown"; it also reported a false defect ("links do nothing") caused by the input gap. Run 2 (tab not hidden) named the same two defects and the element, navigated to Checkout and Video, found that "Refresh cart" does nothing (a true consequence: the `TypeError` stops the script before the listener is attached), and recognised the KDE extension's request as not the application's. It also reported "Save for later does nothing": that button calls `alert()`, which CEF cancels in a windowless browser with no dialog handler (`javascript_dialog_manager.cc:331`), the case US-027 handles. No screenshot tool was offered.
- **Snapshot size.** Fixture pages: 391 to 1,881 bytes in total, of which 27 to 1,206 bytes of accessibility tree. Real pages, full tree including static text: a GitHub repository page 60,279 bytes (172 references), the Wikipedia "Rust" article 649,495 bytes (1,694), an MDN reference page 685,304 bytes (1,666).
- **Unexpected traffic.** With only loopback pages loaded, the network service held 6 outbound connections to Google addresses (`2a00:1450::/32`, `2600:1900::/28`).

A4 confirmed, with one constraint: a tab an agent drives must not be marked hidden; the host keeps it visible to Chromium whether or not it is displayed. A7 confirmed for the fixture (the largest snapshot is 6 % of the 32 KiB bound); real pages exceed it by 2 to 21 times, so the truncation the PRD already specifies (Edge Cases, "Oversized snapshot") is required, not optional.

### Latency A/B

For US-003 and US-005, by the method of TD-17, run 2026-11-25.

Method: a debug build of the spike window with `PANEFLOW_LATENCY_PROBE=1`, a terminal pane beside the browser rectangle, the video fixture playing at 30 frames per second in the rectangle at the dock's default width (`DIFF_DOCK_PANEL_WIDTH`, 540 logical pixels), 3 × 104 letters typed at 45 ms intervals plus `ctrl-u`; one latency sample per keystroke, from the terminal's key handler to its paint (n = 315 per run). Each round interleaves three no-browser runs with one run per path. A third mode, "GPU, not painted", runs the host on the GPU path and still wakes the window for every frame, but paints none, to separate the host's GPU load from the window's import. The host is windowless and uses Chromium's Wayland backend in every run; the window is an X11 client unless a campaign says otherwise.

For an X11 window, the probe span ends at the next frame that GPUI's X11 refresh timer schedules, and that timer follows the display's RandR refresh rate. The rate therefore sets the baseline: about 10.7 ms at 60 Hz and about 6.3 ms at 144 Hz.

**Campaign on the development desktop, window as an X11 client, 143.9 Hz, no synthetic X input.** The window typed into itself through `Window::dispatch_keystroke` (focus on the terminal, then the same keystrokes), so no input could reach any other window. This method was checked against real X input on an isolated display at 144 Hz: median 6.173 ms against 6.021 to 6.169 ms. Rounds l, m and n used a rule written down before they ran (A15): the nine no-browser medians form the reference, and a path regresses if any of its three medians exceeds the largest of them.

| Round | No-browser medians (ms) | CPU path | GPU path | GPU, not painted |
|-------|-------------------------|----------|----------|------------------|
| i | 6.451, 6.404, 6.507 | 6.070 | 6.445 | 6.293 |
| j | 6.479, 6.323, 6.292 | 6.187 | 6.428 | 6.355 |
| k | 6.414, 6.197, 6.417 | 6.069 | 6.463 | 6.233 |
| l | 6.397, 6.152, 6.602 | 6.074 | 6.336 | 6.357 |
| m | 6.203, 6.254, 6.355 | 6.085 | 6.395 | 6.377 |
| n | 6.360, 6.376, 6.328 | 6.164 | 6.399 | 6.414 |

- With TD-17 applied per round of three, the CPU path never regresses. The GPU path regresses only in round k, by 0.046 ms, while that round's three no-browser medians spread over 0.220 ms. Three runs therefore do not discriminate, so A15 fails as written, and the number of no-browser runs was raised to nine.
- With the pre-registered rule on rounds l to n (reference 6.602 ms), neither path regresses: CPU 6.074 to 6.164 ms, GPU 6.336 to 6.399 ms. A3 holds for an X11 window.
- The GPU path costs about 0.3 ms more than the CPU path, and "GPU, not painted" costs the same as the GPU path: the difference comes from Chromium's GPU work in the host, not from importing or sampling frames in the window.
- Frames: 30 per second arrive and 30 are painted on both paths, 0 import failures, 0 slots dropped.

**Campaign on the development desktop, window as a native Wayland client** (how Paneflow runs under GNOME), same self-typing method, rule written down before the run. GPUI paces a Wayland window by the compositor's frame callbacks, so the baseline drops to about 2.9 ms and the noise between runs to about 0.05 ms.

| Round | No-browser medians (ms) | CPU path | GPU path | GPU, not painted |
|-------|-------------------------|----------|----------|------------------|
| wa | 2.936, 2.923, 2.913 | 2.995 | 3.029 | 3.031 |
| wb | 2.909, 2.930, 2.903 | 3.048 | 3.009 | 3.035 |
| wc | 2.950, 2.897, 2.933 | 3.025 | 3.004 | 3.018 |

**Both paths regress** (reference 2.950 ms): every with-browser median is above every no-browser median, by 0.05 to 0.10 ms. The 99th percentile rises from 3.97 to 5.05 ms without the browser to 7.21 to 11.01 ms on the GPU path and 8.14 to 12.23 ms on the CPU path. **A3 fails on a native Wayland window, for both paths.**

**Diagnostic campaign, native Wayland window**, rule and hypothesis written down before the run (rounds da, db, dc; reference = nine no-browser medians, largest 2.940 ms). Two modes separate the causes. In "redraw only" there is no browser: the window redraws an empty rectangle 30 times per second. In "Chromium only", the host plays the video on the GPU path and sends frames, but the window is never woken for them.

| Mode | Medians (ms) | 99th percentiles (ms) |
|------|--------------|-----------------------|
| No browser (9 runs) | 2.892 to 2.940 | 4.10 to 4.59 |
| Redraw only, no browser | 3.026, 2.980, 2.978 | 10.63, 8.74, 11.31 |
| Chromium only, no redraw | 2.976, 2.980, 2.973 | 4.36, 4.08, 4.12 |
| GPU path | 3.040, 3.031, 2.991 | 10.12, 12.21, 10.11 |

- Redrawing the window at the video's frame rate, **with no browser at all**, reproduces the whole tail and most of the median shift. It is the cost of any 30 frames per second animation in Paneflow's window today: on each frame GPUI renders the root view again, and every child view not marked cached with it.
- Chromium playing the video while the window ignores its frames shifts the median by 0.03 to 0.05 ms and leaves the tail unchanged: load that Chromium adds whether or not its page is displayed, as when only an agent drives it.
- The hypothesis "the regression is only the redraw" is therefore only partly true. The transport is not the cause: copying or importing frames adds nothing measurable beyond these two effects ("GPU, not painted" equals the GPU path).

**Supporting campaigns, isolated rootful Xwayland, real X input into it only.** At 60 Hz (rounds c, d, e) the no-browser medians are 10.500 to 11.242 ms; CPU 9.601, 10.965, 10.274; GPU 9.569, 11.026, 10.215; round d exceeds its largest no-browser median by 0.05 ms (CPU) and 0.11 ms (GPU) within a 0.41 ms spread, and with the nine no-browser medians pooled (reference 11.242 ms, applied after the run) neither path regresses. At 144 Hz (rounds f, g, h) the no-browser medians are 6.021 to 6.169 ms; CPU 5.789, 5.883, 5.791; GPU 5.968, 6.047, 6.049: no regression.

**Superseded run of 2026-11-24.** The first runs, on the shared desktop with XTEST input, gave GPU medians of 11.430 and 10.428 ms against no-browser maxima of 6.713 and 6.549 ms, and CPU 5.561 and 7.053 ms. That XTEST input also reached other Paneflow windows, the installed application among them, and one run lost keystrokes. Twelve later GPU-path runs with clean input and an X11 window, six of them on the same desktop, did not reproduce it. Its cause was not established; those runs are not used for the verdict.

### US-005 result

**The CPU path regresses terminal latency on a native Wayland window** (median +0.05 to +0.10 ms, 99th percentile ×2 to ×3) and does not on an X11 window (see "Latency A/B").

Run 2026-11-25.

- **Display.** The host paints through `OnPaint` into a pool of three `memfd` slots shared once with the window (`SCM_RIGHTS`); the window copies each frame into a `RenderImage` and paints it with `paint_image`. At the dock's default width the page arrives at 540 × 738 pixels and fills the rectangle (`ab/smoke-cpu540/window.png`); 30 frames per second in and painted.
- **Copy cost.** The window's copy out of shared memory, on its receive thread, totals 39.9 to 41.2 ms for about 836 frames per run in the decisive campaign: 0.05 ms per 540 × 738 frame.
- **Profile.** `perf record` (cycles in user space, frame-pointer call graphs on a build with `-C force-frame-pointers=yes`; kernel call chains are not allowed at `perf_event_paranoid = 2`) of the window process on the desktop while the video played and the keystrokes ran: 4,980 samples, rendered with `flamegraph` 0.6.14 (`ab/prof-cpu-fp/flamegraph.svg`). Uploading frames takes **6.0 %** of the window's user-space CPU time: 4.1 % in `flush_uploads` (wgpu's `write_texture` staging copy), 1.4 % in the receive thread's copy out of shared memory, 0.5 % in `upload_texture` (staging and swizzle), under 0.1 % in dropping old frames. On the GPUI main thread alone the share is 6.05 % (229 of 3,787 samples). This is a debug build; a release build changes the absolute cost, not the order.

### US-006 result

Run 2026-11-25.

Spike: `spike-cdp` built in an `ubuntu:22.04` container (glibc 2.34 floor), unpacked with the CEF runtime in the guest user's home, started headless, observed 15 seconds; each child process's user namespace, PID namespace and seccomp mode read from `/proc`. Virtual machines: the cached cloud images of `scripts/sandbox-matrix.sh`, booted by `tasks/evidence/agents-browser/us006/sbx-vm.sh` (the product script is unchanged). The AppImage row starts the same host from inside a probe AppImage built with `appimagetool`, the runtime staying outside the AppImage as TD-10 places it.

| System | Kernel | Sandbox | Mechanism | Started from an AppImage |
|--------|--------|---------|-----------|--------------------------|
| Fedora 45 (development machine) | 7.3.4 | Started | Unprivileged user namespaces; renderers under seccomp-BPF | Same |
| Ubuntu 22.04 | 5.15.0 | Started | User namespaces | Same |
| Ubuntu 24.04 | 6.8.0 | **Refused** | None: AppArmor restricts unprivileged user namespaces | Same refusal |
| Debian 13 | 6.12.118 | Started | User namespaces | Same (after installing `fuse3`, which the cloud image lacks) |
| Arch Linux | 7.3.4 | Started | User namespaces | Same (after installing `fuse3`) |
| openSUSE Leap 16.0 | 6.12.0 | Started | User namespaces | Same |

On Ubuntu 24.04 (`kernel.apparmor_restrict_unprivileged_userns=1`) the host aborts with exit status 134: `FATAL:sandbox/linux/suid/client/setuid_sandbox_host.cc:172] The SUID sandbox helper binary was found, but is not configured correctly. Rather than run without sandboxing I'm aborting now. You need to make sure that .../chrome-sandbox is owned by root and has mode 4755.` Two remedies were each tested alone, and each started the sandbox:

1. An AppArmor profile for the host binary granting `userns` (`profile ... <path> flags=(unconfined) { userns, }` in `/etc/apparmor.d/`): sandbox by user namespaces, as on the other systems. A `.deb` can install this profile; because the runtime lives under each user's Paneflow home, the profile has to match that path with `@{HOME}`.
2. `chrome-sandbox` owned by root with mode 4755: sandbox by the setuid helper and a PID namespace. This needs root on a file in the user's home and fails on a home mounted `nosuid`, so it suits no install format Paneflow ships.

No run used a flag that disables the sandbox. A5 confirmed on five systems, failed on Ubuntu 24.04 without a remedy.

### What the later stories inherit

These findings change how later stories are built, not what they must achieve; apart from the latency gates the verdict adds to US-025 and US-035, no acceptance criterion was edited.

- **US-010 (supervise the host).** The spike host crashes at shutdown when its reader thread calls `process::exit` while CEF and Vulkan are alive. The product host shuts CEF down on its UI thread before exiting.
- **US-012 (one profile per workspace).** A fresh profile still loaded a system-wide extension (KDE Plasma Integration) and its content script ran in fixture pages. The host must refuse external and component extensions, or the profile is not clean.
- **US-013 (origins and permissions).** With only loopback pages open, the network service held six connections to Google. The host turns off Chromium's background traffic (component updates, safe-browsing lookups and similar) unless a decision keeps one of them; the loopback-by-default policy is not met otherwise.
- **US-015 and US-017 (tabs and actions).** A tab an agent drives is never marked hidden: Chromium drops input to hidden pages.
- **US-016 (snapshot).** Real pages produce 60 to 685 KB of tree, so the 32 KiB truncation with the dropped amount stated is load-bearing.
- **US-027 (dialogs).** Without a dialog handler, CEF cancels `alert()` in a windowless browser, and an agent reads that as a dead button.
- **US-032 and US-033 (GPU transport).** On X11 the host passes `--use-angle=vulkan`. The product transport adds the queue-family ownership transfer and the explicit synchronisation the spike skipped.
- **US-025 and US-035 (latency).** A displayed video makes the window redraw at its frame rate, and today each redraw re-renders the root view and every uncached child. That redraw, not the transport, is what the terminal pays for. The surface has to keep a browser frame from re-rendering the rest of the window, or the gate added to both stories fails.
- **US-036 (runtime archive).** Expect 123 to 162 MiB per target, compressed, before the host binary.
- **US-039 (sandbox verdict).** Ubuntu 24.04 and later need an AppArmor profile granting `userns` to the host at its path under the Paneflow home; a `.deb` can ship it. Without root (tarball, AppImage), the browser refuses to start there and names that remedy.

## Epics & User Stories

### EP-001: Kill tests on Linux

Three experiments, a fallback measurement and a sandbox survey, run outside product code on the Fedora Wayland development machine. Each can end the recommendation alone. Spike code lives under `tasks/evidence/agents-browser/` (local evidence, not tracked) and builds into a `CARGO_TARGET_DIR` outside the repository.

**Definition of Done:** this PRD contains a section "Kill test report" with one dated result per story US-002 to US-006, each naming the machine, GPU, driver, kernel, compositor and CEF version; US-007 records GO or NO-GO for the GPU path and for the control plane, and every assumption A1 to A5 and A7 to A8 is marked confirmed or failed.

#### US-001: Pin the engine
**Description:** As a maintainer, I want the CEF version, its Rust binding and its archives pinned by hash so that every later story builds against the same engine.

**Priority:** P0
**Dependencies:** None

**Acceptance Criteria:**
- [x] Given the repository, when `native/cef/manifest.toml` is read, then it names one CEF version, one `cef` crate version, and for each of the four shipping targets an archive URL and a SHA-256.
- [x] Given a clean checkout, when `scripts/fetch-cef.sh` runs for the host target, then the archive is placed under `native/cef/prebuilt/<target>/` after its hash matches the manifest.
- [x] Given an archive whose hash differs from the manifest, when `scripts/fetch-cef.sh` runs, then it exits non-zero and leaves no archive in place.
- [x] Given the fetched archive for each target, when its size is measured, then the four sizes are written in the "Kill test report" and assumption A8 is marked.
- [x] Given `.gitignore`, when `git status` runs after a fetch, then `native/cef/prebuilt/` shows no untracked file.

#### US-002: Kill test 1, shared textures under Wayland and X11
**Description:** As a maintainer, I want to know whether CEF hands out GPU frames on my Linux machine so that the GPU path is not built on a belief.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given the off-screen rendering example of the `cef` crate built with `accelerated_osr`, when it loads an animated WebGL page with `--ozone-platform=wayland`, then the report states whether `OnAcceleratedPaint` was called and carries two screenshots taken one second apart showing the animation advanced with no black region.
- [x] Given the same build, when it runs with `--ozone-platform=x11`, then the report states the same two facts for X11.
- [x] Given either run, when CEF falls back to software painting, then the report records the fallback and the log line that explains it, and A1 is marked failed for that backend.
- [x] Given the pinned CEF version, when its source or release notes are checked for the fixes of CEF issues #4237 and #4313, then the report states for each whether the pinned version contains it.

#### US-003: Kill test 2, a frame crosses two processes into a GPUI rectangle
**Description:** As a maintainer, I want a host process to send GPU frames to a minimal GPUI window so that the cross-process path and the atlas seam are proven before product code depends on them.

**Priority:** P0
**Dependencies:** Blocked by US-002

**Acceptance Criteria:**
- [x] Given a spike host that copies each CEF frame into a texture it owns, when it sends the dmabuf descriptor over a Unix socket, then a second process imports it into the wgpu device of a GPUI window.
- [x] Given a copy of `vendor/gpui_wgpu` patched in the spike with the atlas seam of TD-7, when the window paints the frame inside a rectangle smaller than the window, then the page is clipped to the rectangle and a GPUI popup drawn above it covers it.
- [x] Given the window being resized while a video plays, when frames are observed, then the report states whether tearing or stale-size frames appeared.
- [x] Given a terminal pane beside the rectangle and the video fixture playing, when the A/B method of TD-17 is run, then the three no-browser medians, the with-browser median and the clip's frame rate are written in the report and A3 is marked for the GPU path.
- [x] Given the atlas seam cannot return a tile over an imported texture, when the spike ends, then the report states the exact obstacle and A2 is marked failed.

#### US-004: Kill test 3, an agent finds planted defects with no port open
**Description:** As a maintainer, I want an agent to find a console error and a failed request through internal CDP only so that the control plane is proven without any display.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given a spike host with a windowless browser that is not displayed, when it calls `ExecuteDevToolsMethod` for the Accessibility, DOM, Input, Runtime, Network, Page and Log domains, then the report lists each domain as answering or not.
- [x] Given the host running, when the listening TCP sockets of its process tree are listed, then none belongs to a DevTools endpoint.
- [x] Given the fixture application served on loopback, when a real agent drives a throwaway MCP tool connected to the host, then the agent names both defects and the element involved without a screenshot.
- [x] Given the accessibility snapshot of that page serialised as text with element references, when its size is measured, then the size is written in the report and A7 is marked or its bound is replaced.
- [x] Given a domain that does not answer in windowless mode, when the spike ends, then A4 is marked failed and the missing capability is named.

#### US-005: Measure the CPU path
**Description:** As a maintainer, I want the cost of the shared-memory path measured so that the fallback is a known quantity and not a hope.

**Priority:** P0
**Dependencies:** Blocked by US-003

**Acceptance Criteria:**
- [x] Given the spike host painting through `OnPaint` into a shared-memory segment, when the window uploads each frame through `paint_image`, then the page displays at the viewport size of the dock at its default width.
- [x] Given the video fixture playing on the CPU path, when the A/B method of TD-17 is run, then the three no-browser medians and the with-browser median are written in the report and A3 is marked for the CPU path.
- [x] Given the same run, when a `cargo flamegraph` profile of the window process is taken, then the share of time spent uploading frames is written in the report.
- [x] Given the CPU path shows a regression as TD-17 defines one, when the report is written, then it says so in its first line.

#### US-006: Survey the Chromium sandbox on Linux install formats
**Description:** As a maintainer, I want to know where the Chromium sandbox starts so that the browser never ships unsandboxed by accident.

**Priority:** P0
**Dependencies:** Blocked by US-001

**Acceptance Criteria:**
- [x] Given the spike host unpacked in a user's home directory, when it starts on the Fedora development machine and in a virtual machine of each cloud image `scripts/sandbox-matrix.sh` names (ubuntu-22.04, ubuntu-24.04, debian-13, arch, opensuse-leap-16), then the report has one row per system stating whether the sandbox started and which mechanism it used.
- [x] Given the product script `scripts/sandbox-matrix.sh`, when the survey is done, then it is unchanged; the survey boots the images from a spike script kept in the evidence directory.
- [x] Given a distribution where unprivileged user namespaces are restricted, when the host starts, then the report records the exact refusal and what a package would have to install to lift it.
- [x] Given the host started from inside an AppImage-launched application, when the sandbox state is read, then the report records it.
- [x] Given any row where the sandbox did not start, when the report is written, then A5 is marked failed for that row and no flag disabling the sandbox was used to continue.

#### US-007: Record the verdict
**Description:** As Arthur, I want one GO or NO-GO per path with its evidence so that the rest of the PRD is engaged or revised knowingly.

**Priority:** P0
**Dependencies:** Blocked by US-002, US-003, US-004, US-005, US-006

**Acceptance Criteria:**
- [x] Given the five results, when the "Kill test report" is read, then it states GO or NO-GO for the control plane, for the CPU path and for the Linux GPU path.
- [x] Given a NO-GO for the Linux GPU path, when the status file is read, then US-031, US-032, US-033 and US-035 are `CANCELLED`, US-034 stays because the macOS and Windows paths depend on it, and the Changelog says so.
- [x] Given a NO-GO for the control plane, when the status file is read, then every epic after EP-001 is `BLOCKED` and an Open Question names the alternative of TD-1.
- [x] Given any assumption A1 to A5, A7 or A8, when the assumptions table is read, then it is marked confirmed or failed with a link to its report line.
- [x] Given the spike code, when the tracked tree is listed, then nothing from `tasks/evidence/agents-browser/` is tracked.

---

### EP-002: The browser host and its private protocol

A host process that owns Chromium, started on demand, supervised, speaking a versioned private protocol, with internal CDP, per-workspace profiles and an origin policy. Nothing is displayed yet.

**Definition of Done:** on Linux, an integration test starts the host from a debug build, opens a tab on a local fixture page, receives its load event and its console message through the protocol, kills the host and observes the tab reported as crashed, with no DevTools port open at any point; no CEF type is nameable from any workspace crate.

#### US-008: Define the private protocol
**Description:** As a maintainer, I want one crate that names browser, tab, commands, events and frame descriptors in neutral types so that the window and the host agree without sharing engine types.

**Priority:** P0
**Dependencies:** Blocked by US-007

**Acceptance Criteria:**
- [x] Given the workspace, when `crates/paneflow-browser-proto` is built, then it depends on no CEF crate and no GPUI crate.
- [x] Given a window and a host built from different protocol versions, when they handshake, then the connection is refused with an error naming both versions.
- [x] Given every command and event type, when it is serialised and deserialised in a unit test, then the value is unchanged.
- [x] Given a frame larger than the maximum message size the crate declares, when it is decoded, then decoding fails without allocating the declared length.
- [x] Given `src-app` and every workspace crate, when a guard test scans their manifests, then none names the `cef` crate.

#### US-009: Build the host crate outside the workspace
**Description:** As a maintainer, I want the host to build from its own manifest with its own CI job so that the workspace gates do not download Chromium.

**Priority:** P0
**Dependencies:** Blocked by US-008

**Acceptance Criteria:**
- [x] Given the root `Cargo.toml`, when `cargo metadata` runs, then `crates/paneflow-browser-host` is not a workspace member.
- [x] Given a checkout with the CEF archive fetched, when the host manifest is built with `--locked`, then it produces the `paneflow-browser-host` binary.
- [x] Given `.github/workflows/run_tests.yml`, when a change touches the host crate or the protocol crate, then a job named for the browser host builds, lints and tests it on Linux x86_64.
- [x] Given a checkout with no CEF archive, when the host build starts, then it fails with a message naming `scripts/fetch-cef.sh`.
- [x] Given the host crate, when its `tests/fixtures/` directory is listed, then it holds the four tracked fixtures of the "Fixtures" section and a test serves each on loopback.
- [x] Given `deny.toml`, when `cargo deny check` runs against the host manifest, then it passes with no new registry or git source allowed.

#### US-010: Supervise the host
**Description:** As a user, I want the browser engine to start only when I open a tab and to fail without taking my terminals with it.

**Priority:** P0
**Dependencies:** Blocked by US-009

**Acceptance Criteria:**
- [x] Given an application launch in which no browser tab is opened, when the process tree is listed, then no host process exists.
- [x] Given the first tab being opened, when the host starts, then the window connects over an endpoint inside the Paneflow home's runtime directory and the host accepts only its parent process, checked with `paneflow-peer`.
- [x] Given a running host, when it is killed, then every tab reports a crashed state through the protocol and every terminal session keeps running.
- [x] Given a crashed host, when the user reloads a tab, then a new host starts and the tab loads.
- [x] Given the last tab closed, when the host has been idle for the period the protocol crate declares, then the host exits.
- [x] Given the window started from inside a Paneflow pane, when it starts the host, then the host's environment carries none of `paneflow_host::env::PANE_CONTEXT_ENV`.
- [x] Given the host fails to start, when the window observes it, then the GPUI main thread has not waited on it and the tab shows the failure reason.
- [x] Given a debug build, when the host binary is resolved, then it comes from the path in `PANEFLOW_BROWSER_HOST` or, when unset, from the host crate's local build output, and never from an installed application's home.

#### US-011: Bridge CDP inside the host
**Description:** As a maintainer, I want the window to call DevTools methods and receive DevTools events through the protocol so that agent tools need no debugging port.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given a tab on a fixture page, when the window sends a DevTools method through the protocol, then the result returns matched by request id.
- [x] Given a fixture page that logs to the console, when the page loads, then the window receives the console event through the protocol.
- [x] Given a DevTools method that never answers, when the timeout the protocol crate declares elapses, then the caller receives a timeout error and the host stays responsive.
- [x] Given a running host, when its command line and its listening sockets are inspected in a test, then no remote-debugging flag and no DevTools TCP endpoint is present.
- [x] Given a method sent for a tab that has closed, when the host handles it, then the caller receives an error naming the tab as closed.

#### US-012: Keep one profile per workspace
**Description:** As a user, I want each workspace to have its own cookies and storage so that test accounts of one project never appear in another.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given two workspaces, when each sets a cookie on the same loopback origin, then neither reads the other's cookie.
- [x] Given a workspace profile, when its location is read, then it is under the Paneflow home resolved by `crates/paneflow-home`, never under `dirs::config_dir` or `dirs::cache_dir`.
- [x] Given a workspace closed and reopened, when a tab loads the same origin, then the cookie set before is present.
- [x] Given a profile directory already locked by another host, when a second host opens it, then the open fails with an error naming the lock.
- [x] Given a fresh profile, when its cookie store is read, then it is empty and no file of an installed browser was read to create it.

#### US-013: Enforce origins, schemes and permissions in the host
**Description:** As a user, I want the browser to reach my local servers freely and nothing else without my decision so that a page cannot wander.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given a navigation to an `http` or `https` URL on a loopback address, when no decision exists, then the navigation proceeds.
- [x] Given a top-level navigation to an origin that is not loopback and has no decision, in a tab opened guarded (every tab an agent drives), when the host sees it, then the navigation is held and a decision request is emitted through the protocol. A tab a person drives is opened unguarded and loads any origin, in frames too; every other tab, an unclaimed one included, stays guarded. Only a person taking a tab over lifts its guard, and a site it was holding is then dropped and reported, never loaded (changed at Arthur's request on 2026-11-25, `a_tab_a_person_drives_opens_a_public_origin_and_frames_it_without_a_decision`).
- [x] Given a navigation to `file:`, `chrome:`, `devtools:`, `view-source:`, `javascript:` or a top-level `data:` URL, when the host sees it, then it is refused with an error naming the scheme.
- [x] Given a page that requests camera, microphone, geolocation or notifications, when the host sees the request, then it is denied.
- [x] Given a download started by a page, when the host sees it, then the download is cancelled and an event reports it.
- [x] Given a URL that embeds credentials, when it is submitted for navigation, then it is refused.

---

### EP-003: Agent control

The `browser.*` methods, their right, their approvals, and their MCP and CLI entry points. An agent can verify a local application with no visible surface.

**Definition of Done:** on Linux, an integration test drives `browser.open`, `browser.snapshot` and `browser.network` through the IPC client against the fixture application and finds the planted console error and the planted failing request in the results; a real agent in a pane does the same through the MCP tools (US-022); the same calls from a pane without the right are refused by `authorize`; `crates/paneflow-api/tests/decision_table.txt` and `protocol/api-v1.json` are regenerated and committed.

#### US-014: Declare the browser methods and the browser right
**Description:** As a maintainer, I want every browser method in the registry with one right so that authorization stays in one place.

**Priority:** P0
**Dependencies:** Blocked by US-011

**Acceptance Criteria:**
- [x] Given `crates/paneflow-api`, when the method list is read, then each method of "The browser API" is one `Descriptor` owned by the window, and no other `browser.*` method exists.
- [x] Given the rights of a pane grant, when they are listed, then `browser` appears beside `write`, `spawn` and `layout`.
- [x] Given a caller with no `browser` right, when it calls any `browser.*` method, then `authorize` refuses it and the decision is a row of `decision_table.txt`.
- [x] Given the regenerated `protocol/api-v1.json`, when it is compared with the registry, then every `browser.*` method and its schema is present.
- [x] Given a caller whose identity cannot be established, when it calls a `browser.*` method, then the call is refused before reaching the host.

#### US-015: Open, list, navigate and close tabs with an owner
**Description:** As an agent, I want to open a tab on my workspace's server and know which tabs are mine so that I never act on a page the human is using.

**Priority:** P0
**Dependencies:** Blocked by US-014

**Acceptance Criteria:**
- [x] Given an agent with the right, when it calls `browser.open` with a loopback URL, then a tab opens in its workspace's profile, owned by that agent's session, and its id is returned.
- [x] Given tabs owned by the agent, by the human and by another agent, when the agent calls `browser.list`, then each tab is listed with its owner and only the agent's own tabs are marked drivable.
- [x] Given a tab owned by the human, when an agent calls `browser.navigate` on it, then the call is refused with an error naming the owner.
- [x] Given an agent-owned tab, when the agent calls `browser.navigate`, `browser.reload`, `browser.back` or `browser.close`, then the tab does so and the result reports the final URL.
- [x] Given a URL longer than the limit the API declares or with a scheme other than `http` or `https`, when it is submitted, then the call is refused without reaching the host.
- [x] Given an agent session that ends, when its tabs are listed afterwards, then they are reported as unclaimed.
- [x] Given an unclaimed tab, when an agent with the right calls `browser.claim`, then the tab is owned by that agent's session; given a tab owned by someone else, then the call is refused naming the owner.

#### US-016: Return one grouped snapshot
**Description:** As an agent, I want one call that tells me what the page shows and what went wrong so that I do not spend turns assembling it.

**Priority:** P0
**Dependencies:** Blocked by US-015

**Acceptance Criteria:**
- [x] Given a loaded tab, when the agent calls `browser.snapshot`, then the result carries the URL and title, the accessibility tree in the text form of "The browser API", and the console and network entries recorded since the previous snapshot.
- [x] Given a page whose snapshot exceeds the bound of A7, when the snapshot is returned, then it is truncated at the bound and states how much was dropped.
- [x] Given a snapshot, when it reaches the agent through MCP, then all page-derived text is inside one `wrap_untrusted` block whose header carries `source="browser"`.
- [x] Given a password field with a value, when the snapshot is taken, then the value is absent from the result.
- [x] Given a tab that is still loading, when `browser.snapshot` is called, then the result says so and returns what exists.
- [x] Given a tab that is not displayed in any pane or dock, when `browser.snapshot` is called, then the result is the same as for a displayed tab.

#### US-017: Act on elements by reference and wait for conditions
**Description:** As an agent, I want to click, type and wait using the references of the snapshot so that my actions hit what I saw.

**Priority:** P0
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [x] Given a reference from the latest snapshot, when the agent calls `browser.act` with click, type, press, hover, select or scroll, then the action is applied to that element and the result reports whether a navigation followed.
- [x] Given a reference from a snapshot taken before a navigation, when it is used, then the call fails with an error saying the reference is stale and a new snapshot is needed.
- [x] Given a reference to an element that is hidden or disabled, when a click is requested, then the call fails naming the reason.
- [x] Given `browser.wait_for` with a text, a reference or a network-idle condition, when the condition holds before the timeout, then the call returns; when it does not, the call returns a timeout error stating what was awaited.
- [x] Given a JavaScript dialog opened by the action, when the result returns, then it reports the dialog and its message as fenced text, and the dialog is dismissed.

#### US-018: Read console, network and a screenshot on demand
**Description:** As an agent, I want to page through console and network history and get an image when text is not enough.

**Priority:** P0
**Dependencies:** Blocked by US-016

**Acceptance Criteria:**
- [x] Given a tab with console history, when the agent calls `browser.console` with a cursor, then entries after the cursor are returned with level, text and source location, and a new cursor.
- [x] Given a tab with network history, when the agent calls `browser.network`, then each entry carries method, URL, status, resource type and failure reason, and no request or response body.
- [x] Given more entries than the per-tab buffer holds, when history is read, then the result states that older entries were dropped.
- [x] Given `browser.screenshot`, when it is called on an agent-owned tab, then a PNG of the viewport is returned at the tab's device scale factor.
- [x] Given a request header named `Authorization` or `Cookie`, when network entries are returned, then its value is absent.

#### US-019: Evaluate JavaScript behind an approval
**Description:** As an agent, I want to run a script in the page when the semantic tools are not enough, and as a user, I want to approve that the first time.

**Priority:** P1
**Dependencies:** Blocked by US-017, US-020

**Acceptance Criteria:**
- [x] Given an agent that has never been approved for evaluation in this workspace, when it calls `browser.evaluate`, then the call waits on an approval that shows the agent, the origin and the script.
- [x] Given the approval granted, when the script runs, then its JSON-serialisable result is returned as fenced text bounded by the snapshot bound.
- [x] Given the approval denied, when the call returns, then it carries a refusal and the script did not run.
- [x] Given a script that throws, when the call returns, then it carries the exception message as fenced text.
- [x] Given a script still running at the timeout the API declares, when the timeout elapses, then the call returns a timeout error.

#### US-020: Ask the user before an agent acts on a new site
**Description:** As a user, I want to decide per site whether an agent may browse it so that a page cannot send my agent elsewhere.

**Priority:** P0
**Dependencies:** Blocked by US-015, US-013

**Acceptance Criteria:**
- [x] Given an agent-owned tab navigating to an origin that is not loopback and has no decision, when the host holds the navigation, then the approval broker raises a request showing the agent and the origin, with allow once, always allow for this workspace, and deny.
- [x] Given "always allow" chosen, when the same agent session or another one in the workspace navigates to that origin, then no request is raised.
- [x] Given "deny" chosen, when the call returns, then it carries a refusal naming the origin and the tab stays on its previous page.
- [x] Given a pending site approval, when a `browser.*` call or pane input from the requesting agent arrives, then it cannot answer the approval.
- [x] Given a redirect from an allowed origin to an undecided one, when the host sees the redirect, then it is held and a request is raised for the new origin.

#### US-021: Expose the tools through MCP and the CLI
**Description:** As an agent, I want the browser methods as MCP tools and CLI verbs so that any coding agent in a pane can use them.

**Priority:** P0
**Dependencies:** Blocked by US-016, US-017, US-018

**Acceptance Criteria:**
- [x] Given `paneflow-mcp`, when its tool list is read, then it contains one tool per `browser.*` method the agent may call, each described in one sentence that states page content is untrusted.
- [x] Given the `paneflow` CLI, when `paneflow browser --help` runs, then it lists a verb per method with an example, and each verb has `--json` output.
- [x] Given a browser tool called when no host binary can be resolved, when the call returns, then the error says the browser engine is not installed.
- [x] Given `paneflow browser status --json`, when it runs, then it reports through `browser.status` whether a host runs, its runtime version, the sandbox state, and for each tab its owner and presentation path.
- [x] Given the release-min build, when `scripts/check-helper-sizes.sh` runs, then `paneflow-mcp` is under its cap.
- [x] Given an MCP client scoped to one workspace, when it lists tabs, then tabs of other workspaces are absent.

#### US-022: Prove the loop with a real agent
**Description:** As Arthur, I want a real agent to find planted defects through the shipped tools so that the control plane is proven end to end.

**Priority:** P0
**Dependencies:** Blocked by US-019, US-020, US-021

**Acceptance Criteria:**
- [x] Given a debug build on a tagged home and the tracked fixture application, when an agent in a pane is asked to check the page, then it reports the console error and the failing request using only `browser.*` tools.
- [x] Given the same session, when the agent fixes the fixture and checks again, then its second snapshot shows neither defect.
- [x] Given the tracked fixture page whose visible text instructs the reader to open an external site, when the agent snapshots it, then no navigation to that site happens without a site approval being raised.
- [x] Given the session transcript, when it is reviewed, then every page-derived string reached the agent inside a fence.

---

### EP-004: The browser surface

A browser tab in the right dock and as a pane, drawn through the CPU path on Linux, with navigation chrome, input, web popups, shared human and agent use, link routing and session restore.

**Definition of Done:** on Linux under Wayland and under X11, a user opens a browser tab in the dock without a Git repository, loads the fixture application, types in a form with an input method, uses a `<select>`, watches an agent drive an agent-owned tab, takes it over, opens a loopback link from a terminal into the dock with the setting of US-029 on, restarts Paneflow and finds the tabs restored without any request sent before activation; the manual pass of `DESIGN.md`'s UI delivery gate is recorded.

#### US-023: Open the dock without a Git snapshot
**Description:** As a user, I want to open the right dock on a browser tab in a folder that is not a repository so that browsing does not depend on Git.

**Priority:** P0
**Dependencies:** Blocked by US-010

**Acceptance Criteria:**
- [x] Given the dock's new-tab menu and the pane surface picker, when they are opened, then each offers a browser entry, backed by a `Browser` variant of `DiffDockTab` and of `PaneSurface` whose page area is empty until US-025.
- [x] Given a workspace whose folder is not a Git repository, when the user opens a browser tab in the dock, then the dock opens and no `git` process is started.
- [x] Given a repository workspace with the dock on a browser tab, when the tab is active, then no Git status refresh is triggered by browser activity.
- [x] Given the dock closed with only browser tabs, when it is reopened, then it shows the browser tab that was active.
- [x] Given a workspace with no repository, when the user selects the Changes tab, then the existing empty state is shown and the browser tab stays open on its page.

#### US-024: Add the browser chrome
**Description:** As a user, I want a browser tab with an address bar and navigation buttons that look like the rest of Paneflow.

**Priority:** P0
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [x] Given a browser tab, when it is displayed, then it shows back, forward, reload or stop, an address field and a page area, built from the controls of `src-app/src/settings/kit.rs` and `ui_primitives.rs` with one tooltip per control.
- [x] Given a page that is loading, crashed, held for a site decision or refused by policy, when the tab is displayed, then each state has its own visible message stating what happened and the available action. Since 2026-11-25 only a tab no person drives can be held: the page stays shown and the strip names the site awaiting approval through the broker, the in-window Allow once / Always allow here / Deny panel being removed at Arthur's request.
- [x] Given text typed in the address field, when Enter is pressed, then input with a scheme is used as is; input without one that is `localhost`, an IP literal, a `host:port` pair or contains a dot gets `http://` when the host is loopback and `https://` otherwise; any other input opens a search with `browser.search_engine` (changed from a refusal at Arthur's request on 2026-11-25, after Helium), and blank input is refused with an inline message.
- [x] Given no host binary can be resolved, when a browser tab is opened, then the page area states that the browser engine is not installed and no host is started (US-037 later turns this message into the install prompt).
- [x] Given `DESIGN.md`, when it is read, then the browser chrome's dimensions and states are documented there.
- [x] Given an address that the policy of US-013 refuses, when it is submitted from the address field, then the page area shows the refusal naming the scheme and the previous page stays loaded.

#### US-025: Present frames through the CPU path
**Description:** As a user, I want to see the page so that I can watch what I and the agent do.

**Priority:** P0
**Dependencies:** Blocked by US-011, US-023

**Acceptance Criteria:**
- [x] Given a tab displayed in a pane or the dock, when the host paints, then pixels reach the window through shared memory and are drawn with `paint_image`, clipped to the page area and stacked under Paneflow's popups.
- [x] Given the page area resized, when the next frame arrives, then it has the new size and no frame of the old size is stretched to fit.
- [x] Given a window scale factor other than 1, when a page is displayed, then the host renders at that device scale factor.
- [x] Given a tab that is not displayed, when the host is observed, then it produces no frames for that tab.
- [x] Given a frame arriving while the main thread is busy, when it is handled, then only the most recent frame is uploaded and none is queued without bound.
- [x] Given the shared-memory segment cannot be created, when the tab is displayed, then the page area shows the failure and the tab stays drivable by agents.
- [x] Given the video fixture playing in the dock of a native Wayland window, when the A/B method of TD-17 is run with nine runs of the control arm, in which the host plays the same video with its tab not displayed (the Chromium load that agent control keeps whether or not a page is shown), and three runs of the CPU path, then the medians and 99th percentiles are recorded in this PRD; the CPU path regresses when any of its medians exceeds the largest control median or any of its 99th percentiles exceeds the largest control 99th percentile, a rule fixed before the run; and if the CPU path regresses, the display of browser tabs is off by default while agents keep full control. Changed at Arthur's request on 2026-11-25: pages always show, because a person who opens a browser tab wants to see the page; the measured regression stands and is owned by the follow-up that lowers the window's per-redraw cost.

#### US-026: Route input, focus and shortcuts
**Description:** As a user, I want to click, scroll and type in the page, including with an input method, without losing Paneflow's own shortcuts.

**Priority:** P0
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [x] Given the page area focused, when the user clicks, drags, scrolls and types, then a click on the fixture form's submit button, at its position in the snapshot, submits the form at scale factors 1 and 2.
- [x] Given an input method composing text, when the composition is committed, then the page's field contains the committed text and the candidate window is positioned at the caret.
- [x] Given the page area focused, when a Paneflow shortcut for pane focus, tab switching or the command palette is pressed, then Paneflow handles it and the page does not receive it.
- [x] Given the page changing the cursor shape, when the pointer is over the page area, then the window shows that cursor and restores its own when the pointer leaves.
- [x] Given focus moving from the page to a terminal, when the user types, then no keystroke reaches the page.
- [x] Given the host crashes while the page area is focused, when the user types, then the keystrokes are dropped, the tab shows the crashed state and Paneflow's shortcuts still work.

#### US-027: Handle web popups, dialogs, new windows and files
**Description:** As a user, I want select menus, alerts, links that open a new window and file pickers to work so that the pages I build are usable.

**Priority:** P0
**Dependencies:** Blocked by US-026

**Acceptance Criteria:**
- [x] Given a `<select>` opened in the page, when its list is shown, then it is drawn adjacent to the control and clipped to the window.
- [x] Given a page calling `alert`, `confirm` or `prompt`, when the dialog opens, then it is rendered by Paneflow, names the origin, and the page receives the user's answer.
- [x] Given a link that opens a new window, when it is followed, then a new browser tab opens with the same owner and profile, subject to the origin policy.
- [x] Given a file input clicked by the human, when the picker opens, then it is the platform's file dialog and the page receives only the chosen file.
- [x] Given a file input activated by an agent's `browser.act`, when the picker would open, then no dialog opens and the call returns an error saying file selection needs the human.

#### US-028: Share a tab between the human and the agent
**Description:** As a user, I want to see when an agent drives a tab and to take over at any moment.

**Priority:** P0
**Dependencies:** Blocked by US-024, US-026, US-015

**Acceptance Criteria:**
- [x] Given an agent-owned tab displayed, when the agent acts, then the tab shows which agent session owns it.
- [x] Given an agent-owned tab, when the human clicks or types in the page area, then ownership moves to the human and the agent's next `browser.act` is refused with an error naming the takeover.
- [x] Given a human-owned tab, when the human chooses "hand to agent" for a session, then that session's calls on the tab succeed.
- [x] Given an agent-owned tab that is not displayed anywhere, when the human opens the tab list, then the tab is listed with its owner and can be shown.

#### US-029: Open local links in the Paneflow browser
**Description:** As a user, I want a detected local URL to open beside my terminal when I choose so.

**Priority:** P1
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [x] Given a URL in terminal output, when its context menu is opened, then it offers "Open in Paneflow browser" beside the existing external open.
- [x] Given the setting `browser.open_local_links` turned off, when a loopback URL is activated, then it opens in the system browser as before the browser existed. The default was off when this story was written; Arthur turned it on on 2026-11-25 (see "Removed at Arthur's request on 2026-11-25" in the EP-004 review).
- [x] Given the setting turned on, when a loopback URL is activated from a terminal or from a detected port in the sidebar, then it opens in a browser tab of that workspace's dock.
- [x] Given the setting turned on, when a URL that is not loopback is activated, then it opens in the system browser.
- [x] Given the runtime is not installed and the setting is on, when a loopback URL is activated, then it opens in the system browser.

#### US-030: Restore tabs without touching the network
**Description:** As a user, I want my browser tabs back after a restart without every page reloading at once.

**Priority:** P1
**Dependencies:** Blocked by US-024

**Acceptance Criteria:**
- [x] Given browser tabs open at quit, when Paneflow restarts, then each tab is restored with its URL, title and owner set to unclaimed.
- [x] Given restored tabs, when none has been activated, then no host is running and no request has been sent.
- [x] Given a restored tab activated, when it loads, then the origin policy applies as for a new navigation.
- [x] Given a `session.json` whose browser entry holds a URL with a refused scheme, when the session is restored, then that tab is dropped and the others are restored.

#### EP-004 implementation notes

Recorded 2026-11-25 by the implementation run, for the review.

- **The host runs on Ozone's headless backend.** Under Wayland, Chromium 156 (`RenderWidgetHostViewBase::UpdateScreenInfo`) replaces the device scale factor that CEF reports with the backend's own when the backend supports per-window scaling, so a windowless tab stayed at scale 1 on a scale 2 window. The headless and X11 backends keep the reported factor, and only headless needs no display, so `--ozone-platform=headless` is always passed (approved by Arthur). The GPU path of EP-005 must therefore import from a host that has no Wayland or X11 connection, or bring this question back.
- **Site permissions are denied by profile setting, not by a callback.** Under headless, Chromium denies geolocation and notifications before CEF asks, so `PermissionDenied` is no longer emitted for them. Each profile sets geolocation, notifications, camera and microphone to BLOCK once its request context is initialized, and creates no tab before that; a profile that cannot set them refuses its tabs (approved by Arthur).
- **The terminal had no context menu.** US-029's "beside the existing external open" assumed one. A right click on a URL, when the program has not enabled mouse reporting, now opens a menu with Open link, Open in Paneflow browser and Copy link; no other terminal right-click behavior changed.
- **US-025 latency A/B, native Wayland window, 2026-11-25.** Measured with `scripts/latency-ab.sh` (method and rule in `bench/README.md`, "Browser latency A/B"; the results are local evidence under `target/latency-ab/`). Development desktop, GNOME on Wayland, debug build at `e9b8db8e` plus this epic's uncommitted changes; each run 315 keystrokes; the rule was written into `rule.json` before each campaign's first run.
  - Campaign 1 (`20261125T130122Z`): **invalid**. The fixture server stopped answering after the eighth run, so the video did not play in runs 9 to 12, and the terminal host of run 1 outlived its run. The script now starts one server per run and checks that it answers, and stops every process of a run's home with SIGTERM then SIGKILL.
  - Campaign 2 (`20261125T131221Z`): formally **no regression**, but only because control run 7 had a median of 22.352 ms against 3.659 to 3.762 ms for the other eight, which lifted the reference to 22.352 ms and 39.621 ms. The CPU runs had medians of 3.503 to 3.532 ms and 99th percentiles of 9.529 to 10.286 ms, against at most 5.900 ms for the other control runs. The run is kept as recorded. A profile then showed the worker's first-start import of the agents' history (thread `paneflow-usage-import`, about three times the CPU of the window's main thread) running in every run, because every run had a fresh home; runs now copy one usage ledger imported before the first run. The rule gained a validity criterion before campaign 3: a run whose median exceeds 1.5 times the median of its arm's run medians invalidates the campaign.
  - Between campaigns 2 and 3 the dock's browser view became a cached view, as in panes. It did not change the window's redraws (66 to 70 root renders per second while the page plays, 44 to 47 in the control arm), because GPUI renders the root view on every redraw. A frame-pointer profile of a CPU run attributes 49.6 % of the main thread's samples in the typing window to `Window::draw`, spread over GPUI's prepaint and paint, with the root view's own render at 4.5 % and frame uploads at 4.2 %: no single avoidable cost.
  - Campaign 3 (`20261125T135816Z`, seed 2026112503): **valid, the CPU path regresses.** Control medians 3.526 to 3.595 ms, 99th percentiles 4.662 to 5.840 ms. CPU medians 3.044 to 3.086 ms, below every control median, and 99th percentiles 8.640, 8.697 and 9.540 ms, all above 5.840 ms. Video frames decoded per run: 738 or 739 in every run; frames copied to the window: 741 to 747 on the CPU path, 0 in the control arm. Pooled median difference, for information: -0.497 ms (95 % bootstrap interval -0.515 to -0.469).
  - Consequence, as the criterion requires: **page display is off by default** (`browser.display`, default `false`). A browser tab then shows "Page display is off" with Show pages; the palette's Show browser pages toggles it; agents drive every tab whatever the setting. The cost is the redraw of the whole window at the page's frame rate, which any animation in Paneflow's window pays today; a follow-up should lower the per-redraw cost of the window, after which the default can be measured again with the same script.
- **`browser.open_local_links`** is documented in the site's schema reference (six languages) and in the `docs/user` mirror with the same text.

#### EP-004 review

Reviewed 2026-11-25 (tier deep) against the working tree on `e9b8db8e`. A ticked criterion is proven by a test through its entry point or by the recorded measurement; an unticked one waits for the manual pass of the Definition of Done, because its proof is a window observation (GPUI input, focus, the platform dialogs, a restart) that no test reaches.

Defects found and fixed in the review:

- **A refused address typed in a tab that already showed a page was dropped without a word** (US-024). The window sent it to the host, which answered with an error nobody read, so no refusal appeared. The address is now classified by the host's own policy before anything is sent, and the refusal names the scheme above the page, which stays as it was (`browser/address.rs`, `refusal`).
- **Clicks, keys, wheel and IME text reached a page that was not shown** (US-026, US-028). The state panels (Page display is off, the site decision, the crash, the presentation failure) covered the page without stopping input, so a click on one of them, the "Show pages" button included, took the tab from its agent and clicked at the same spot in the hidden page. Only a displayed page takes input now (`PageState::takes_input`).
- **Closing a browser pane left its page running in the host.** Only the dock closed its tab. A released view now closes a tab the human or no one owns; an agent's tab keeps running for the agent and stays in Browser tabs (US-028).
- **Every DevTools event of every tab re-rendered the whole window**, displayed or not. A view now ignores a change that leaves its tab unchanged, and the change signals of one tab are coalesced until the window has taken them, like frames. The latency campaigns above ran before this change and it was not measured; the verdict and the default stand until the same script is run again.

Arthur's first manual pass (2026-11-25, native Wayland, `~/.paneflow-dev`) found three defects. Their causes, checked against the code and against t3code and Zeron, and the fixes:

- **The tab stayed on `about:blank` and Show pages gave a white page.** The host creates every tab on `about:blank`, then starts the requested navigation; an address outside loopback is held for a decision. The placeholder's own load events arrived after the hold and cleared it in the window, so the decision prompt never showed, the title read `about:blank`, and the page shown was the blank placeholder. The host now reports none of the placeholder's page events when the tab was opened on another address (`Host::page_event`), and the window never shows `about:blank` as a title or an address, as t3code and Zeron do not. Test: `policy::a_tab_opened_on_a_public_origin_reports_only_the_held_site_until_it_is_decided` (red before the fix). Typing paneflow.dev now asks "Allow this page to open https://paneflow.dev?"; Allow once loads it.
- **Pages with no declared background were painted transparent.** CEF's windowless browser enables transparent painting when `background_color` is left at 0 (`cef_types.h`); t3code sets an opaque white base for the same reason. The host now sets opaque white. Test: `surface::a_page_without_a_background_is_painted_on_opaque_white` (corner `[0, 0, 0, 0]` before the fix).
- **The address text was near black on the dark well.** The view set no text color, so the field inherited GPUI's default; the view now uses the primary ink.
- **Show pages sat to the right of the message.** The action row was a sibling of the centered column in a row layout; every state's actions are now stacked under the message, 8px below it.

Found while checking these paths:

- Turning page display off did not redraw a cached browser view, which kept its last frame instead of "Page display is off".
- The page area was white before the first frame and around a resized frame; it now takes the frame's top-left pixel, as Zeron does, and the surface color before any frame.
- The held-site prompt said the page wanted to leave the local servers when the human had typed the address; it now says the address is outside them.

Added after the manual pass, at Arthur's request: the address field reproduces Helium's location bar and its loading line (`src-app/src/browser/loader.rs`, DESIGN.md "Browser tab"). The host forwards CEF's load progress as a new `load_progress` event (protocol version 3), the control layer keeps it per tab in thousandths, and the field shows the site icon, the host and the rest of the address at rest, and the whole address selected on a click. The "Loading <host>" strip line is gone, since the loading line and Stop carry it. Checked in a nested headless sway against a page whose images take 8 to 30 s: the line at the field's bottom edge measured (49,116,179) on the (44,44,44) fill, the accent at 64% as in Helium, present from the first load and gone once the page loaded. Helium's Screen blend for the glow has no GPUI equivalent and is blended normally.

Back led to a blank page after the human allowed a held site, or after an agent's `browser.open`: the host creates every browser on `about:blank`, and that page became the first history entry whenever it finished loading before the real navigation (the held decision, or the agent's navigate after waiting for the blank load). Chromium treats a new tab's first document as an initial entry that the first navigation replaces; the host now does the same: the first real load of a tab whose only committed page is the blank one runs as `location.replace`, and an open on `about:blank` no longer loads it a second time (`a_tab_opened_on_a_public_origin_reports_only_the_held_site_until_it_is_decided` asserts one history entry, `the_first_page_after_a_blank_start_replaces_it_in_the_history` and `going_back_reloads_the_previous_page_and_never_reaches_the_blank_placeholder` cover the agent path and Back).

A failed load kept its strip line after the next page loaded: the view showed the control layer's `last_failure`, which is never cleared because agents read it. The view now shows a failure or a refused navigation only until the next navigation starts (`a_load_failure_is_shown_until_the_next_navigation_starts`); the error page CEF commits after a failure sends no `LoadStarted`, so the line stays on it.

Added at Arthur's request, after Helium's omnibox: typing in the address field suggests pages (`src-app/src/browser/omnibox/`, DESIGN.md "Browser tab"). Text that is not an address now offers a search with `browser.search_engine` (DuckDuckGo by default, as in Helium) instead of being refused; nothing reaches the engine before Enter, and its site is held for a decision like any other. Sources: a shared browsing history of human-opened pages, ranked like Chromium's history providers (word-start matches, typed and visit counts, recency), with inline completion of a known site; and the dev servers the window detected, which complete inline from `localhost`. Remote engine suggestions and agent pages were left out on purpose. Checked in a nested labwc session: "verc" completed to `vercel.com/strive-x` over the seeded history with the list measured against Helium's (panel 63 against 61, selected row 79 against 79, rows 28px), and "local" completed to the `python3 -m http.server 5173` started in the terminal.

Removed at Arthur's request on 2026-11-25: the in-window "Allow this page to open <site>?" panel. A tab a person drives no longer holds any site; the guard now follows the tab's owner (protocol version 4: `OpenTab.guarded` and a `Guard` command the control layer fires when a person takes a tab over or hands it to an agent). The guard fails closed: a tab an agent leaves becomes unclaimed and stays guarded, and lifting the guard drops a held site instead of loading it, so an agent cannot get an unapproved address opened by navigating and then exiting (raised by an automated security review of the first version, which released it). Every held site now goes to the approval broker and shows as a strip line over the page. Control test `only_a_tab_a_person_drives_opens_a_site_outside_the_local_servers_without_a_decision`, host test `a_tab_a_person_drives_opens_a_public_origin_and_frames_it_without_a_decision`.

Added at Arthur's request on 2026-11-25, after Zeron's Browsing settings: a Browser page in Settings (DESIGN.md 5.5) with Open local links in Paneflow, the search engine (now with a custom search address, `browser.custom_search`), the default page zoom (`browser.default_zoom`) and the sites zoomed on their own, Keep history (`browser.keep_history`) and Clear history by time range. Page zoom is new in the browser: protocol version 5 adds a `Zoom` command the host applies with CEF's `SetZoomLevel` (Chromium's logarithmic level, base 1.2), and the text size keys zoom a browser tab, the site keeping its zoom in `~/.paneflow/browser/zoom.json`. The history and zoom files share one store (`src-app/src/browser/kept.rs`) that replays edits made before the file loaded, so a clear made early is not undone by the file. Left out: Zeron's Developer section (inspecting pages needs a DevTools window the windowless engine does not give) and its History page listing visits. Tests: `the_zoom_keys_give_the_site_its_own_zoom_and_reset_brings_the_default_back`, `edits_made_before_the_file_loaded_are_replayed_on_it_in_order`, `page_zoom_steps_through_the_browser_levels_and_ignores_an_unknown_default`, `a_zoom_percent_becomes_chromiums_logarithmic_zoom_level`, `the_zoom_is_sent_again_for_each_page_that_starts_or_arrives` (a zoom sent while a restored tab still sat on its blank start page was lost at the real page's commit; found in the nested labwc check, where a site seeded at 150 % then showed at 150 %, its percentage reset it to 100 %, and the settings page saved a custom search address, flagged one without `%s`, and cleared the history).

Removed at Arthur's request on 2026-11-25: `browser.display`, its "Page display is off" panel, the palette's Show browser pages and the settings row. A person who opens a browser tab wants to see the page, so pages always show; a config that still sets `display` loads and ignores it. This reverses the default US-025's latency criterion set: while a page animates, the terminal's 99th percentile keystroke latency on a native Wayland window rises from at most 5.840 ms to 8.640 to 9.540 ms (campaign 3 above), and that cost is now paid whenever a page plays. Owner: the follow-up that lowers the window's per-redraw cost (half of the main thread's time while typing goes to `Window::draw`), to be measured again with `scripts/latency-ab.sh`. Also changed: `browser.open_local_links` now defaults to on, so a localhost link from a terminal or the sidebar opens in Paneflow's browser.

Added at Arthur's request on 2026-11-25, after Helium: Site information behind the address field's site icon (DESIGN.md, Browser surfaces). Helium's source holds only patches over Chromium, so its Page Info bubble was rebuilt natively rather than copied. Protocol version 6 adds a `Security` command: the host reads the visible navigation entry's `SSLStatus` (secure connection, certificate status, insecure content, TLS version) and its X.509 certificate (subject, issuer, validity) and answers `SiteSecurity`. Cookies and storage come from the tab's DevTools (`Network.getCookies`, `Storage.getUsageAndQuota`, `Storage.clearDataForOrigin`, `Network.deleteCookies`), and the control layer now keeps the permissions it refused per origin (`TabView::blocked`, 16 per tab). Tests: `a_page_served_over_plain_http_reports_no_secure_connection_and_no_certificate` (host, real CEF), `a_certificate_error_outranks_mixed_content_and_clean_content_is_secure`, `a_chromium_time_converts_to_unix_seconds`, `each_permission_a_site_was_refused_is_kept_once_and_the_oldest_leave_first`, `the_standing_follows_the_engine_and_falls_back_to_the_scheme_until_it_answers`, `cookies_and_storage_read_from_devtools_and_summarize_in_words`. Checked in the nested labwc session: wikipedia.org showed Connection is secure with its Let's Encrypt certificate and TLS 1.3, its 5 cookies were deleted, `localhost` showed Local server, and Browser settings opened the Browser page. Permissions stay blocked: letting a site use the camera or the location would be a policy change, not part of this panel.

Added at Arthur's request on 2026-11-25, after Zeron: the address field's Pointer button opens a menu with Annotate elements and Hand to agent (DESIGN.md, Browser tab). Annotate elements runs Zeron's element picker, ported to `src-app/src/browser/annotate/annotator.js` (MIT, THIRD_PARTY_NOTICES.md), in an isolated world created over DevTools (`Runtime.enable`, `Runtime.addBinding` with an `executionContextName`, `Page.createIsolatedWorld`), so the page can neither see nor call the binding. The control layer turns `Runtime.bindingCalled` for `__paneflowAnnotate` into `SurfaceEvent::Annotation` and keeps it out of the console; the window accepts a message only from the world it created and with the nonce of the current run. Picks stack as numbered chips in a tray under the strip (24 at most, a hover card with the element's HTML), and Send pastes a `<browser_annotations>` block, page text escaped out of its envelope, into the chosen agent's prompt without submitting it. Tests: `a_binding_bound_to_an_isolated_world_answers_from_it_and_the_page_cannot_see_it`, `a_shift_click_reaches_the_page_as_a_click_with_shift_held`, `escape_as_the_window_sends_it_reaches_the_page_as_the_escape_key` (host, real CEF), `only_the_annotation_binding_reaches_the_window_and_it_never_lands_in_the_console`, `a_pick_from_the_page_is_read_only_with_its_nonce_and_bounded`, `the_prompt_numbers_each_element_and_keeps_page_text_inside_its_envelope`. Checked in the nested labwc session on a local pricing page: a Shift+click and a click picked two elements, numbered on the page and in the tray, the hover card showed `button.btn` and its HTML, Send to Claude Code pasted both annotations into a stand-in agent's prompt and emptied the tray, and Escape ended the mode.

Changed at Arthur's request the same day: the tray and its confirmation are gone. The agent is chosen in the Pointer menu before annotating (Annotate for <agent>, a submenu with several), a click sends the pick straight to its prompt, and Escape sends what Shift+click gathered. The picker also keeps the arrow cursor: Zeron's `cursor: crosshair` asked the cursor theme for a crosshair, and MacTahoe's at 24px is a faint grey cross at 35% to 70% opacity that vanished over the page, in Zeron too. Test: `the_annotator_keeps_the_arrow_over_text_fields_instead_of_a_theme_crosshair` (host, real CEF; it read `Crosshair` before the fix). Checked in the nested labwc session: Shift+click then click, and Shift+click then Escape, each pasted the picks into a stand-in agent with no tray shown.

Added at Arthur's request on 2026-11-26, after Helium: F12 docks Chromium's developer tools on the right of the page (DESIGN.md, Browser tab; `toggle_developer_tools`, `inspect_element`, `show_developer_console` in the Browser key context). CEF's `ShowDevTools` opens a native window even for a windowless browser under the Alloy style, so the tools are a second off-screen browser instead: protocol command `Inspect` (version 7) creates it in the page's profile, the host attaches a flattened DevTools session to the page (`Target.attachToTarget`), loads the bundled frontend after `Page.addScriptToEvaluateOnNewDocument` installs a WebSocket stand-in, and relays each message through a `Runtime.addBinding` binding one way and `on_dev_tools_message` the other (`crates/paneflow-browser-host/src/inspector.rs`); the bridge accepts only its own session and the child sessions it saw attached. The frontend's language is pinned to English through `Emulation.setUserAgentOverride` (its locale bar otherwise offers French), and it draws its menus in its own frame (`setUseSoftMenu`), since an off-screen browser has no native menus. Test: `surface::an_inspector_opens_off_screen_beside_its_page_and_closes_with_it` (frontend loads, English, the overflow menu opens, a second inspector is refused, closing the page closes it; the menu check is red without the soft-menu switch). On Linux and Windows `Ctrl+Shift+J` stays with `jump_next_waiting`, a global default that would shadow it, so `show_developer_console` has a default only on macOS.

Second review, 2026-11-26 (tier deep, working tree on `e9b8db8e` with every addition above). Defects found and fixed:

- **A paint at the size before a resize reached the window** (US-025, and the host CI job). `a_frame_too_large_to_share_is_reported_and_the_tab_stays_drivable` failed once in fifteen runs before the fix: CEF delivered a paint at the default 1280x800 view after the 6000x6000 present, and the host sent it because the tab was displayed. A present that changes the pixel size by more than one pixel now drops paints at the previous view's pixel size until a paint of another size arrives (`present.rs`, `superseded`). Test: `a_paint_at_the_size_before_a_resize_is_dropped_until_the_new_size_arrives`; the host test then passed 20 runs out of 20.
- **A page opened new tabs without a click** (US-027). `NewWindowRequested` carried CEF's `user_gesture` flag, but the control layer opened every request; under the Alloy style no Chromium popup blocker runs, so a page in a human tab could open any number of unguarded tabs on any http(s) site from a timer. A request without a gesture is now dropped with a console warning. Test: `a_new_window_opens_as_a_tab_with_its_openers_owner_workspace_and_place_only_after_a_click`.
- **Every URL past 2048 characters was refused** (found in Arthur's manual pass, 2026-11-26). `Host::before_browse` ran the same `classify` as agent commands, so a frame or page Google loaded with a 2241-character address was cancelled and the strip read "Paneflow did not open this address: the URL has 2241 characters". At Arthur's request the limit is now Chromium's own, 2 MiB, for agents, the person and pages alike (Helium patches neither), and the address field shows at most 32 KiB of an address, as Chromium does. Tests: `addresses_past_2048_characters_load_in_frames_pages_and_commands` (host, real CEF; it timed out on the frame before the fix), `a_web_address_is_shown_without_its_scheme_with_the_host_apart`.
- Hardening: the window computes a frame row with a checked multiplication, so a host announcing a width past `u32::MAX / 4` gets its frame released instead of a debug panic or an oversized allocation (`frames.rs`, `accept`).

Left open (not blocking):

- **A decision for Arthur:** a caller outside every pane (`Principal::External`) is the human for the browser (`browser_methods.rs`, `browser_context`), so since the guard follows the owner its tabs open any site without a decision, and its `browser.evaluate` needs no approval (the latter since EP-003). An agent that leaves its pane's process tree (a double fork reparented to the session's subreaper) reaches both. The same user can read the profile directory anyway; a separate guarded identity for `External` would close the browser side.
- The DevTools bridge relays any method the frontend sends, `Target.attachToTarget` and `Browser.*` included, on the inspected tab's session (`inspector.rs`, `Bridge::outgoing`). Only the person using the tools drives the frontend, so no page or agent reaches it; filtering the Target and Browser domains is hardening that needs a check against every DevTools panel.
- The window maps the host's segments without checking their memfd seals, so a compromised host could shrink one and end the window with SIGBUS on the next copy.
- `docs/user/configuration/schema.md` and `docs/user/keybindings.md` document `search_engine`, `custom_search`, `default_zoom`, `keep_history` and the developer tools keys, but the site repository, which the mirror is synced from, still documents only `open_local_links` and the removed `display` key, with `open_local_links` defaulting to `false`. The next sync reverts the mirror unless the site gets the same text.
- The owner badge and the Hand to agent menu name the agent (Claude Code), not the session, so two sessions of one agent in a workspace look the same.
- A dialog from a human-owned tab that no view holds (a tab a human opened from the CLI) waits until a view shows it.
- An address the human types outside loopback is held for a decision like a page's navigation. t3code loads any site without asking; keeping the prompt follows US-013 and is a decision for Arthur.
- t3code reloads a crashed page automatically with a backoff; Paneflow shows the crash with Reload (US-024).
- Outside the epic: a URL that the terminal soft-wraps across rows is detected only up to the wrap, so `http://localhost:18751/app.html` split after `app.htm` opened `app.htm`. `terminal/element/hyperlink.rs` is unchanged by the epic and Open link cuts it the same way.

Manual pass, 2026-11-26, run by the review on two tagged debug homes against the fixture server (`tests/fixtures`), with a `git` wrapper on the PATH counting every call. X11: Xvfb 1600x1000 with openbox, lavapipe, at scale 1 and at scale 2 (`Xft.dpi: 192`, which is also the 800 by 500 minimum window). Wayland: headless sway 1.12, one output at scale 2, with a virtual pointer and keyboard (wlr-virtual-pointer, virtual-keyboard) driven by a small client written for the pass, `wtype` and `grim`. An agent was a script started as `claude` in a pane, so the window detected Claude Code and its `paneflow browser` calls carried the pane's session. The captures are local evidence, kept outside the repository.

- US-023: opening the dock, the Browser entry and loading the fixture started no `git` process under either display server, while opening Changes started one; Changes showed "Not a Git repository" and the browser tab stayed on its page; the reopened dock showed the browser tab that was active.
- US-025: the page stayed inside the page area in every capture, and the palette, the dock's new-tab menu, the Browser tabs and Pointer menus, the approval prompt and the tooltips were drawn over it.
- US-026: a click at the submit button's position in the page (its bounding box centre plus the page origin) submitted the form at scale 1 (X11) and at scale 2 (X11 and Wayland, `devicePixelRatio` 2); a drag selected the heading text and three wheel notches scrolled the page by 360 px. With the page focused, the palette, Escape, pane focus (a toast for no pane in that direction), a new tab and the previous tab were handled by Paneflow and a key logger in the page saw none of them; after a click into a terminal, typed text reached the terminal and not the page; the X cursor was the I-beam over a text field, the arrow over a button and over Paneflow's sidebar, and the I-beam again on return; with the host killed (SIGKILL) while the page was focused, the tab showed the crash with Reload, typed keys reached nothing and the palette still opened. Input methods: under X11, IBus over XIM with Anthy and Intelligent Pinyin composed in the page field (にほんご, then 日本語 after conversion) and Enter committed it (the field read `abc二本`); IBus's GTK panel drew no candidate window in Xvfb, for a GTK 3 entry as well, so a logging IBus panel took its place and recorded the lookup table (11 candidates) at the cursor location the window reported, the end of the composition on the field's bottom line. Under Wayland, an input-method-v2 client written for the pass sent preedit and commit through sway's text-input-v3 relay: the composition showed in the page field, the commit gave `abc日本語`, and its popup surface, which sway places at the text input's cursor rectangle, opened right under the caret and followed the end of the composition.
- US-027: `alert`, `confirm` and `prompt` were drawn by Paneflow naming `http://127.0.0.1:18751`, and the page received false for Cancel and the typed answer for the prompt; the file input opened the desktop portal's file chooser and the page received only the chosen file.
- US-028: under both display servers an agent's undisplayed tab was listed in Browser tabs with its owner and shown from there; the tab showed "Claude Code drives this tab"; a click in the page, and on X11 a key typed with the page focused, moved it to the human, and the agent's next `browser act` failed with "the human took over tab 2 in the Paneflow window"; Hand to agent from the Pointer menu gave it back and the agent's click then succeeded. The state panels take no input (`PageState::takes_input`, unit test): keys typed over the crash panel reached nothing.
- US-029: a right click on a URL in a terminal offered Open link, Open in Paneflow browser and Copy link; with the default setting a Ctrl+click on a loopback URL (X11 and Wayland) and the sidebar's Open Vite entry for a detected port opened a dock tab, and `https://example.com/` went to the system opener; with `PANEFLOW_BROWSER_HOST` pointing at a missing file the dock showed that the engine is not installed, and the loopback link and the detected port both went to the system opener. `xdg-open` and `gio` were stand-ins that only logged, so nothing reached a real browser.
- US-030: after a restart (X11, then Wayland through Ctrl+Q and Keep sessions running) with the dock closed and a pane browser tab in the background, no browser host ran and the fixture server logged no request; activating a tab loaded it with its URL and title, owner unclaimed; `session.json` held the pane tab's address and title.
- DESIGN.md delivery gate: Paneflow Dark and Light and Vercel Dark and Light (X11); the material on Wayland with `background_style: "blended"` and the Linux chrome and terminal materials, which sway, offering no blur protocol, renders as the thick veil, the browser chrome unchanged and legible; no compositor under openbox; macOS and Windows by inspection only (the chrome sets no fill of its own and takes the dock's surface; its popups are opaque); `reduce_motion`: the address field's loading line changed 95 pixels between two captures 300 ms apart and none once it was on; the 800 by 500 window with the sidebar hidden and the dock open; a long page title, a long address and the Browser tabs rows truncated without wrapping; disabled back and forward, hover tooltips, loading with Stop loading, the refused scheme, a load error, the denied site, the approval prompt, the crash, the empty tab, the engine not installed, the takeover strip and the owner badge.

Defects found by the manual pass and fixed:

- **The pane's new-tab menu could not add a browser** (US-023). Choosing Browser showed "Maximum tab count reached (8)": the layout model never builds a surface that is not a terminal, so the staged browser was never added. `apply_layout_with` now builds the staged surfaces the model spawns no session for. Pin: `pin_a_browser_chosen_from_a_panes_new_tab_menu_joins_the_pane_as_its_active_tab`.
- **A pane's browser tab was saved without its title** once the layout was attached to the worker (US-030): `adopt_surface` copied the address but not the name. Pin: `pin_a_browser_tab_in_a_pane_is_saved_with_its_address_and_title`.
- **An agent's tab chosen from Browser tabs showed nothing when the dock was closed** (US-028): it was added to the closed dock. Showing a tab in the dock now opens it. Pin: `pin_a_browser_tab_shown_from_the_tab_list_opens_the_closed_dock_on_it`.
- **Opening or closing the dock was recorded only by a later save** (US-030), so a crash or a SIGTERM after closing it restored the dock open and loaded its page at startup (six requests seen under Wayland). Toggling the dock now saves the session. Pin: `pin_closing_a_dock_of_browser_tabs_is_recorded_so_a_crash_does_not_reopen_it`.
- **The dock cut a page title from the left** (`…ensible in the chrome`) with the address as its tooltip, the rule for file names, while a pane tab keeps the start and shows the full title. A browser tab in the dock now does the same. Test: `a_long_page_title_keeps_its_start_like_a_pane_tab`.
- **At the minimum window the owner badge left two characters of the address** (DESIGN.md gate, US-028). The badge now shrinks and truncates, with its full text as tooltip, and the address field keeps 120 px.

Verdict, 2026-11-26: EP-004 is DONE. Every criterion of US-023 to US-030 is proven by a test through its entry point or by the manual pass above, and no blocking finding remains. Certifying run after the last change: `cargo fmt --check`; `cargo clippy --workspace --all-targets --locked -- -D warnings`; `cargo nextest run --workspace --locked --profile ci` (6282 passed, 120 skipped); `scripts/check-test-wallclock.sh`; `scripts/check-helper-sizes.sh`; and for `crates/paneflow-browser-host`, fmt, clippy `--all-targets` and nextest (77 passed).

---

### EP-005: GPU presentation on Linux

The shared-texture path under Wayland and X11, with the CPU path as its explicit fallback. On a native Wayland window a displayed tab is composed by the compositor in a subsurface of its own (US-058 to US-062, added 2026-11-26 after US-035 measured the cost of redrawing the window for every page frame). US-031, US-032, US-033 and US-035 are engaged only if US-007 recorded GO for the Linux GPU path. US-034 is platform-neutral (the per-tab presentation state, its indicator and its setting) and is built either way; no story outside this epic depends on the other four.

**Definition of Done:** on the Linux development machine under native Wayland and under X11, a displayed tab presents through an imported dmabuf (composed in a subsurface on a native Wayland window, drawn by GPUI elsewhere), a forced import failure switches the tab to the CPU path with a visible indicator, and the A/B latency method of TD-17 shows no regression while the video fixture plays in the dock; the measurement is a section of this PRD.

#### US-031: Add the external-texture seam to the vendored renderer
**Description:** As a maintainer, I want the vendored `gpui_wgpu` to draw an imported texture through the image atlas so that a GPU frame needs no change to the `gpui` core crate.

**Priority:** P1
**Dependencies:** Blocked by US-025

**Acceptance Criteria:**
- [x] Given `vendor/gpui_wgpu`, when an image id is registered as externally backed, then `get_or_insert_with` for that id returns a tile covering the registered texture without calling the build closure.
- [x] Given a registered id that is unregistered, when the next frame is drawn, then the texture is released and a later paint of that id uses the ordinary upload path.
- [x] Given the header of `vendor/gpui_wgpu/Cargo.toml`, when it is read, then it names this change and when to drop it.
- [x] Given the seam, when the vendored crate's own tests run, then a test covers register, draw and unregister.
- [x] Given the registration API is called from a thread other than the one owning the renderer, when the call is made, then it is rejected or marshalled without a data race, proven by a test.

#### US-032: Export frames from the host as owned dmabufs
**Description:** As a maintainer, I want the host to hand out textures it owns so that a frame stays valid after CEF's callback returns.

**Priority:** P1
**Dependencies:** Blocked by US-031

**Acceptance Criteria:**
- [x] Given a CEF accelerated frame, when the host receives it, then it copies it into a texture of a pool it owns and sends that texture's descriptors, format, modifier and size over the protocol.
- [x] Given a frame sent, when the window has not released it, then the host does not write to that texture.
- [x] Given the copy has not completed on the GPU, when the frame would be sent, then the host waits for completion on a host thread, never on the window.
- [x] Given the window disconnects, when the host observes it, then every pooled texture is released.
- [x] Given CEF falls back to software painting for a tab, when the host observes it, then it switches that tab to the CPU path and reports the reason.

#### US-033: Import and present GPU frames in the window
**Description:** As a user, I want the page drawn straight from GPU memory so that video and animation cost no CPU copy.

**Priority:** P1
**Dependencies:** Blocked by US-032, US-034

**Acceptance Criteria:**
- [x] Given a frame descriptor received, when the window imports it, then the page area draws it clipped to its area and under Paneflow's popups, as on the CPU path.
- [x] Given a host that stops answering while a tab presents, when a test stalls the host, then the window keeps drawing terminal frames and no main-thread call is waiting on the host.
- [x] Given the page area resized, when frames of the new size arrive, then the old import is released.
- [x] Given the graphics device is lost, when it is recreated, then the tab resumes presenting without a restart.
- [x] Given a format or modifier the device cannot import, when import fails, then the tab moves to the CPU path through the presentation state of US-034 and the failure reason is recorded.
- [x] Given a tab hidden, when it is no longer displayed, then its imported textures are released.

#### US-034: Make the fallback explicit
**Description:** As a user, I want to know when the browser runs on the slower path and why.

**Priority:** P1
**Dependencies:** Blocked by US-025, US-021

**Acceptance Criteria:**
- [x] Given the per-tab presentation state (GPU, CPU by setting, CPU by fallback with a reason), when it is read, then it is platform-neutral code with a unit test for each transition.
- [x] Given a tab on the CPU path by fallback, when it is displayed, then the browser chrome shows an indicator whose tooltip states the reason.
- [x] Given the setting `browser.gpu_presentation` set to off, when a tab is displayed, then it uses the CPU path and no indicator of failure is shown.
- [x] Given `paneflow browser status --json`, when it runs, then it reports the presentation path of each tab and the last fallback reason.
- [x] Given a simulated GPU import failure in a session, when another tab opens in that session, then it starts on the CPU path.

#### US-035: Measure both paths against the terminal
**Description:** As a maintainer, I want evidence that the browser does not slow the terminal so that no performance claim ships unmeasured.

**Priority:** P1
**Dependencies:** Blocked by US-033, US-058, US-059, US-060, US-061, US-062

**Acceptance Criteria:**
- [x] Given the video fixture playing in the dock on the GPU path, when the A/B method of TD-17 is run in a debug build on a tagged home, then the nine no-browser medians (A15) and the with-browser medians are recorded in this PRD.
- [x] Given the same video on the CPU path, when the method is run, then its medians are recorded beside the first.
- [ ] Given either run shows a regression, when the section is written, then the regression is stated in its first line and the affected path is disabled by default.
- [x] Given the measurement, when the test tree is searched, then no unit test asserts a duration for it.
- [x] Given both runs, when they are made, then each runs once with Paneflow's window as a native Wayland client and once as an X11 client, with nine no-browser runs each (A15).

#### US-058: Compose a displayed tab's frames in a Wayland subsurface
**Description:** As a user on a native Wayland window, I want the compositor to show the page's frames itself so that a playing page never makes Paneflow's window redraw.

**Priority:** P1
**Dependencies:** Blocked by US-033

**Acceptance Criteria:**
- [ ] Given a tab displayed on the GPU path in a native Wayland window, when the window presents it, then `src-app` creates a `wl_subsurface` of the window's `wl_surface`, from the `wl_display` and `wl_surface` GPUI exposes through `raw-window-handle`, on an event queue of its own, desynchronized and with an empty input region, and `vendor/gpui_linux` is unchanged.
- [ ] Given a pool announced by `GpuSurface`, when the presenter receives it, then each slot becomes one `wl_buffer` through `zwp_linux_dmabuf_v1` with the pool's fourcc, modifier and plane layout, created once per pool generation and destroyed when the generation is replaced.
- [ ] Given a frame descriptor for the tab, when it arrives, then a presenter thread attaches the slot's buffer, damages it and commits the subsurface, and no call reaches the GPUI main thread and no redraw of the window is requested, proven by a counter of root renders that does not move while frames are committed.
- [ ] Given the compositor sends `wl_buffer.release` for a slot, when the presenter dispatches it, then that slot is released to the host, and a slot is never released while the compositor holds it, proven against the in-process test compositor of this story.
- [ ] Given the in-process test compositor (a `wayland-server` display in the test that records requests and sends releases on demand), when the tests run on the Linux CI runner, then they cover create, attach, commit, release, a new pool generation and the window closing, with no real compositor.
- [ ] Given the window as an X11 client, when a tab is displayed, then it keeps the GPUI atlas path of US-033.

#### US-059: Place, scale and hide the subsurface with the page
**Description:** As a user, I want the page to sit exactly in its area at any scale so that the subsurface looks like the page GPUI would have drawn.

**Priority:** P1
**Dependencies:** Blocked by US-058

**Acceptance Criteria:**
- [ ] Given the page element's bounds in a frame, when its prepaint runs, then it publishes the bounds and visibility to the presenter without waiting on it, and the presenter applies `set_position` and a `wp_viewport` destination in logical pixels, the buffer staying in device pixels.
- [ ] Given a fractional scale (1.25, 1.5, 1.75), when the page is presented, then the buffer size equals the destination size times the scale, rounded as GPUI rounds the page area, proven against the test compositor.
- [ ] Given the page area resized, when frames of the new size arrive, then the old generation's buffers are destroyed after their release and no buffer of one size is shown at the other size's destination.
- [ ] Given a tab hidden, a pane closed or the window minimized, when it is no longer displayed, then the subsurface has no buffer attached and its slots return to the host.
- [ ] Given a page in a container whose corners are rounded, when it is displayed, then it keeps the atlas path, and DESIGN.md states which browser containers have square corners and therefore take the subsurface.
- [ ] Given a live debug build on a tagged home under GNOME on Wayland, when the dock and a pane each show a page at scales 1 and 2, then screenshots show each page in its area with no offset, recorded in this PRD.

#### US-060: Draw Paneflow's own surfaces above a page
**Description:** As a user, I want Paneflow's menus, dialogs and tooltips to stay above the page so that the subsurface never hides them.

**Priority:** P1
**Dependencies:** Blocked by US-058, US-031

**Acceptance Criteria:**
- [ ] Given a frame of the window, when the scene is drawn, then the external-texture seam of `vendor/gpui_wgpu` reports for each registered page image whether any primitive drawn after its sprite intersects the sprite's bounds inside the content mask, and its `Cargo.toml` header names this extension.
- [ ] Given a menu, the site information panel, a dialog, the omnibox suggestions or a tooltip crossing the page, when that frame is drawn, then the tab is drawn through the GPUI atlas in that frame and the subsurface has no buffer, and when nothing crosses it any more the tab returns to the subsurface.
- [ ] Given a switch between the two, when it happens, then exactly one of them shows a page frame in every window frame, proven by a scene test that checks no frame shows both and none shows neither.
- [ ] Given a page frame arriving while the tab is drawn through the atlas, when the presenter receives it, then it hands the frame to the window instead of committing it.
- [ ] Given the seam's report, when the scene is drawn, then computing it adds no allocation per frame beyond the first, proven by the seam's tests.

#### US-061: Fall back when the compositor cannot take the page
**Description:** As a user, I want the page to keep showing when the compositor does not accept Chromium's buffers so that delegated compositing is an optimization and never a failure.

**Priority:** P1
**Dependencies:** Blocked by US-058, US-034

**Acceptance Criteria:**
- [ ] Given the compositor's dmabuf feedback, when the presenter starts, then it reads the formats and modifiers of every tranche and of the main device, and a pool whose fourcc and modifier the feedback lists takes the subsurface.
- [ ] Given a pool the feedback does not list, when the window sees it, then it asks the host for a pool with a modifier from the feedback through a field of `Present`, and the host exports one when its device can, otherwise keeps its own.
- [ ] Given neither, or a compositor without `zwp_linux_dmabuf_v1` version 4, `wl_subcompositor` or `wp_viewporter`, when a tab is displayed, then it stays on the GPUI atlas path, and `paneflow browser status --json` reports per tab how it is composed (`subsurface` or `window`) and why not the subsurface.
- [ ] Given a buffer the compositor rejects (`zwp_linux_buffer_params_v1.failed`) or a protocol error on the presenter's queue, when it happens, then the tab moves to the atlas path for the rest of the session with the reason recorded, and the window keeps running, proven against the test compositor.
- [ ] Given a fallback of this story, when the tab is displayed, then no indicator of US-034 is shown, since the page still comes from GPU memory.

#### US-062: Attribute the window's cost in the latency A/B
**Description:** As a maintainer, I want the A/B to tell the transport's cost from the window's redraw cost so that a regression names its cause.

**Priority:** P1
**Dependencies:** None

**Acceptance Criteria:**
- [ ] Given `scripts/latency-ab.sh`, when it runs with `--path redraw`, then its third arm redraws an empty rectangle at the video's frame rate with no browser, as EP-001's diagnostic campaign did, and `verdict.py` judges it by the same rule.
- [ ] Given every run, when its report is written, then it records how its tab was composed (`subsurface`, `window` or none) and the window's root renders per second, and a `gpu` run on a native Wayland window whose tab was not composed in the subsurface makes the campaign invalid.
- [ ] Given the measurement, when the test tree is searched, then no unit test asserts a duration for it.
- [ ] Given the method, when `bench/README.md` is read, then it documents the arms, the rule and the expected root renders per second while a page plays.

#### EP-005 implementation notes

Recorded 2026-11-26 by the implementation run, for the review.

- **Ozone's headless backend gives no accelerated frame.** A probe on 2026-11-26 (local evidence under `tasks/evidence/agents-browser/ep005-headless-probe/`) ran the product host with shared textures on under `--ozone-platform=headless`: `OnAcceleratedPaint` was never called, so EP-004's headless choice and the GPU path exclude each other. Arthur's decision: when `browser.gpu_presentation` is on and an X display answers (XWayland under GNOME), the host runs `--ozone-platform=x11 --use-angle=vulkan --disable-accelerated-video-decode`, which keeps the reported device scale factor; otherwise it stays headless on the CPU path and the reason is shown. The window names its Vulkan device (`--paneflow-gpu-frames=<device UUID>`), and the host's `Welcome` answers `gpu: available` or `unavailable` with the reason (protocol version 8).
- **Transport.** For each accelerated paint the host imports Chromium's dmabuf, copies it into an image it owns (queue-family transfer from and back to `VK_QUEUE_FAMILY_EXTERNAL`, fence wait on CEF's UI thread), then composes the page and its popup into a free slot of a pool of three DRM-modifier images it exports (`GpuSurface` carries the descriptors, fourcc, modifier and plane layout; `Frame` names the slot). The window imports each slot once into the GPUI atlas through the US-031 seam and releases a slot only when the GPU work that sampled it is done. A tab presented on the CPU path by a GPU host reads its captured frame back into shared memory, so a running host serves both paths. `crates/paneflow-browser-host/src/gpu.rs` holds the Vulkan code behind `FrameDevice`, which the presentation tests replace with a fake.
- **Found while documenting the A/B:** the window first named its device only when a tab was displayed, so a host started by an agent's tab ran headless for the rest of its life. The device is now handed to the client when the runtime starts and whenever the setting changes (`browser_control::sync_gpu_device`). The A/B's new check (a `gpu` run whose tab ended on the CPU path is invalid) caught it in a live run.
- **Fallback reasons in this version:** the host has no X display or no Vulkan device that shares dmabufs, Chromium painted the page in software, the window's device cannot import the modifier, or the browser engine was started without GPU presentation. After the first failure in a window, new tabs start on the CPU path (US-034).
- **Tests.** Seam: `vendor/gpui_wgpu/tests/external_texture.rs` (`cargo test --locked -p gpui_wgpu -p paneflow-app --test external_texture`). Host, with the real engine: `crates/paneflow-browser-host/tests/gpu.rs`, which takes the GPU branch when the host reports it available and the fallback branch otherwise; `PANEFLOW_EXPECT_GPU_FRAMES=1` makes it fail unless the GPU branch runs, and `env -u DISPLAY` forces the fallback branch. Host presentation against a fake device: `present.rs`, module `gpu_path`. Window: `src-app/src/browser/gpu_frames.rs` on GPUI's headless renderer, and the ignored `a_page_painted_by_the_browser_host_is_imported_and_drawn_by_the_window`, which drives a real host (`PANEFLOW_BROWSER_HOST=crates/paneflow-browser-host/target/debug/paneflow-browser-host cargo nextest run -p paneflow-app --run-ignored only -E 'test(a_page_painted_by_the_browser_host)'`) and checks the color the window draws from the imported dmabuf. Presentation state: `crates/paneflow-browser-control/src/presentation.rs` and three tests in `control.rs`; the indicator: `a_tab_on_the_cpu_path_by_failure_shows_why_and_one_by_setting_shows_nothing`.
- **Not verified:** `aarch64-unknown-linux-gnu` (A14), an Xorg server rather than XWayland, AMD and Intel GPUs (the development machine is NVIDIA, modifier `0x300000000606014`), and a window and host on two different GPUs.

#### US-035 result

**The GPU path regresses terminal latency on a native Wayland window, in the 99th percentile** (6.57 to 8.29 ms against at most 5.27 ms with no page displayed), while its median drops (3.03 to 3.08 ms against 3.48 to 3.54 ms). The CPU path's tail is worse on the same window (7.19 to 16.60 ms). Neither path regresses on an X11 window. The criterion "the affected path is disabled by default" cannot be met as written, because both displayed paths pay the cost and pages always show (2026-11-25): on 2026-11-26 Arthur chose to remove the cause rather than pick a path, so this story is blocked on the proposal below, and `browser.gpu_presentation` keeps its default, on.

Method: `scripts/latency-ab.sh` (`bench/README.md`, "Browser latency A/B"), now with `--path gpu|cpu` and `--x11`; debug build at `f66fe31e` plus this epic's uncommitted changes, GNOME on Wayland, NVIDIA; nine control runs and three runs of the path per campaign, rule written into `rule.json` before each first run; each run 315 keystrokes; results are local evidence under `target/latency-ab/`.

| Campaign | Window | Path | Decision | Control medians (ms) | Control 99th (ms) | Path medians (ms) | Path 99th (ms) |
|---|---|---|---|---|---|---|---|
| `20261126T110237Z` | Wayland | GPU | invalid: control run 3 at 5.673 ms, over 1.5 times its arm | 3.505 to 5.673 | 4.62 to 12.39 | 3.275, 3.045, 3.072 | 7.14, 6.80, 6.81 |
| `20261126T114357Z` | Wayland | GPU | **regression** (99th percentile, all three runs) | 3.480 to 3.535 | 4.71 to 5.27 | 3.082, 3.031, 3.077 | 7.10, 6.57, 8.29 |
| `20261126T111251Z` | Wayland | CPU | no regression, only because control run 6 (99th 21.65 ms) lifted the reference; the other eight reach at most 5.69 ms | 3.510 to 4.418 | 4.95 to 21.65 | 3.040, 3.293, 3.016 | 9.41, 16.60, 7.19 |
| `20261126T115414Z` | Wayland | CPU | invalid: runs 11 and 12 at 23.454 and 20.531 ms, the desktop in use | 3.451 to 20.531 | 4.44 to 39.65 | 3.018, 3.000, 23.454 | 8.09, 5.93, 40.05 |
| `20261126T112257Z` | X11 | GPU | no regression | 6.235 to 7.304 | 9.44 to 10.65 | 5.129, 5.154, 5.132 | 9.42, 10.15, 9.92 |
| `20261126T113304Z` | X11 | CPU | no regression | 6.741 to 7.170 | 10.10 to 10.82 | 4.919, 5.076, 4.960 | 9.20, 10.56, 9.66 |

- Every run of the path played the video (735 to 754 frames decoded) and the window received 736 to 1490 frames; every GPU run ended on the GPU path. The window renders its root view 64 to 68 times per second while the page plays on Wayland, against 44 to 45 in the control arm: the cost EP-001's diagnostic campaign attributed to redrawing the whole window, not to the transport.
- No unit test asserts a duration for this measurement; `scripts/check-test-wallclock.sh` passes.

#### Delegated compositing on Wayland

Written 2026-11-26 at Arthur's request, before any implementation, and approved the same day: it became US-058 to US-062, and US-035 now depends on them. The criterion of US-035 is unchanged; it is met by removing the cause, not by choosing a path. The page's frames stop waking GPUI: on a native Wayland window each displayed tab gets its own `wl_subsurface`, the compositor composes Chromium's dmabuf directly, and the window redraws only when its own content changes, as Chromium and Firefox do for video. The measured cause disappears for both paths; the X11 window, which does not regress, keeps the atlas path.

- **The surface.** GPUI's Wayland window exposes its `wl_surface` and `wl_display` (`raw-window-handle`, libwayland through `wayland-backend`'s `client_system`), so `src-app` opens a second event queue on the same display and creates, per displayed tab, a subsurface of the window's surface, desynchronized, with an empty input region (input keeps reaching GPUI) and a `wp_viewport` (buffer in device pixels, destination in logical pixels, so fractional scales stay sharp). No change to `vendor/gpui_linux`.
- **The buffers.** Each slot of the host's pool becomes one `wl_buffer` through `zwp_linux_dmabuf_v1` (fourcc, modifier and plane layout from `GpuSurface`), created once per pool generation. A frame is `attach`, `damage_buffer`, `commit` on the subsurface, done by a presenter thread that owns the queue; the slot returns to the host on `wl_buffer.release`. The GPUI main thread takes no part in a frame.
- **Placement.** The page element's prepaint publishes its bounds and visibility; the presenter applies `set_position` (applied on the window's next commit, which a layout change makes anyway) and the viewport destination. A hidden tab attaches no buffer.
- **What draws above the page.** A subsurface above the window hides GPUI's menus, dialogs, tooltips and site information panel where they cross the page. The seam therefore reports, per frame, whether any primitive drawn after the page's sprite intersects its bounds (an extension of the US-031 seam in `vendor/gpui_wgpu`, which therefore needs Arthur's approval for that file); while one does, the tab is drawn through the atlas as today and the subsurface is unmapped. Overlay promotion as Chromium's overlay processor does it: the subsurface is an optimization the window may decline frame by frame, never a second source of truth for what is visible.
- **Capability.** The compositor's dmabuf feedback must list the pool's fourcc and modifier, or the host is asked for a modifier from the feedback; without either, the tab stays on the atlas path with that reason, through the presentation state of US-034. Rounded corners of a pane cannot clip a subsurface, so a page in a rounded container keeps the atlas path or the container gives up its corners there (a DESIGN.md decision).
- **Proof.** The A/B of US-035 on a native Wayland window must show no regression on both paths and root renders while the page plays back at the control arm's rate; a scene test proves a menu over the page demotes the tab and its closing promotes it again; a test with a compositor that refuses the modifier proves the fallback. macOS (`CALayer` with an `IOSurface`, EP-007) and Windows (DirectComposition with a shared D3D11 texture, EP-007) have the same structure, so the presenter's interface is platform-neutral and only its backends differ.

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
- **Link routing default.** US-029 kept the system browser as the default for loopback links; Arthur turned `browser.open_local_links` on by default on 2026-11-25, before any release, so a loopback link from a terminal or the sidebar opens in the dock unless the setting is turned off.

## Files NOT to Modify

- `native/libghostty/**`: the terminal engine boundary, unrelated.
- `protocol/api-v1.json`: generated from the catalog; regenerate it, never edit it by hand.
- `docs/user/**`: a mirror synced from the site repository.
- `crates/paneflow-shim/**` and `crates/paneflow-ai-hook/**`: size-capped helpers with no browser role.
- `crates/paneflow-host/src/sandbox/**`: the pane sandbox; the browser host is a child of the window and is not a pane.
- `vendor/cosmic-text/**`: unrelated to the browser seam.
- `vendor/gpui_wgpu/**` outside the external-texture seam of US-031 and its occlusion report of US-060, and `vendor/gpui_linux/**` entirely: upstream copies whose changes follow their `Cargo.toml` headers only. The Wayland subsurface of US-058 lives in `src-app`.
- `scripts/wallclock-allowlist.txt`: an entry may only go down.
- `/home/arthur/dev/paneflow-browser/**`: an abandoned worktree, to be removed before work starts; never read or copied.
[/PRD]
