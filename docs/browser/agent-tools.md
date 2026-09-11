# Browser agent tools

The `browser.*` namespace is the shared agent surface for Browser tabs. The
CEF host remains the browser boundary. The terminal engine, arbitrary script
execution, public CDP passthrough and automatic page-to-terminal handoff are
outside this API.

## Scope and identity

Every IPC request carries its authorized workspace in the private
`_paneflow_context.workspace_id` field. The CLI and MCP bridge inherit that
identity from `PANEFLOW_WORKSPACE_ID`; a workspace-scoped MCP bridge is required
for Browser calls. The requested `browser_id` and document `generation` are
checked against the owning workspace. There is no active-page fallback.

`browser.list` returns owned pages, including parked pages, without waking
them. `browser.state` requires an explicit BrowserId and generation. Successful
responses include the workspace owner, session owner, BrowserId, document
generation, origin, reduced URL, title, state, parked flag and timestamp. Page
content, URLs, snapshots and diagnostics are untrusted data.

## Access and interaction

Agent Browser access is disabled by default and is changed only from the human
Browser menu at workspace scope. The three states are `disabled`, `read` and
`interact`; each change increments the authorization generation. Read access
exposes `browser.state`, `browser.snapshot`, `browser.console` and
`browser.network`. Interact access additionally permits `browser.navigate`,
`browser.back`, `browser.forward`, `browser.reload`, `browser.click`,
`browser.type` and `browser.scroll`.

Mutations return `accepted` with an operation ID. Poll that ID with
`browser.operation`, or subscribe to the scoped `browser_operation` event. A
mutation has one lease per BrowserId, a maximum 30 second lease, a 30 second
navigation deadline, a 10 second deadline for other actions, four in-flight
operations per workspace and sixteen per instance. Human input, navigation,
close and permission revocation cancel pending control. Each operation produces
one terminal state: `completed`, `failed`, `cancelled` or `timed_out`.

Long-running callers may renew a pending operation with `browser.renew`; the
extension is clamped to the same 30 second lease maximum and cannot revive a
completed, cancelled or timed-out operation. Navigation stays `accepted` after
the native request acknowledgement and completes only after the main document
commits, including HTTP redirects.

Browser controls never take keyboard focus. Files, external protocols, OS
dialogs, downloads and page permissions remain human-controlled. The page
cannot call this IPC namespace from its document context.

## Bounds and privacy

Snapshots are limited to 2,000 nodes and 256 KiB. Console and network buffers
retain at most 1,000 entries per category, 4 MiB per category and 15 minutes of
history. Network bodies are not captured. Exported URLs drop query strings and
fragments; credential headers are redacted if a future diagnostic adapter
provides them. Oversized or inaccessible data returns a bounded error and does
not invent a partial result. `browser.screenshot` uses a native PNG capture,
capped at 8 MiB and 16 megapixels; large captures are returned in bounded pages
through `browser.operation`.

## Human selection

The Browser menu action `Select page context` enables a bounded viewport
selection overlay backed by the live accessibility tree. Password fields,
hidden nodes and protected values are excluded. The selected document reference,
URL, rectangle, semantic summary and visible text are previewed in the owning
session's composer. The composer marks the content as untrusted and never
submits it to a PTY or agent automatically. A document change, inaccessible
frame or removed node invalidates the selection before sharing.

## Qualification boundary

The shared controllers, CLI/MCP registries, ownership checks and bounded app
service are internal implementation foundation. EP-006's Linux agent scope is
qualified on the reference x86_64 host after native SEC-09 to SEC-12,
concurrency, stale-identity, parked-page and restart checks. The R3 release
verdict remains independent and `NOT_QUALIFIED` while the deferred EP-007
presented-pixel, M1 and platform-matrix gates remain open. Human Browser
qualification is independent of this agent verdict. Windows and macOS may reuse
these contracts, but this Linux evidence does not qualify either port.

The Windows client emits the same `agent_console` and `agent_network` shapes and
terminalizes an agent navigation at the main-document commit, so the shared
`browser.*` controller is the only controller on that target as well. Its agent
verdict is tracked separately in
[windows-qualification-contract.toml](../../native/browser/windows-qualification-contract.toml)
and stays `NOT_QUALIFIED` until SEC-09 to SEC-12, the C3 quotas, a host restart
and a human takeover have been observed on a live Windows page.
