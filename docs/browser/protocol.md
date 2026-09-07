# Browser protocol contract v2

This is the portable R0 contract for US-003. The executable entry point is
`paneflow-browser-harness`; `Controller::dispatch` is shared with host adapters.
The deterministic adapter owns descriptors, operation slots and frame tokens.
It does not run a browser engine, allocate a GPU texture, expose agent tools or
certify a native target.

## Transport and trusted ownership

Control traffic consists of a four-byte unsigned big-endian payload length,
followed by one UTF-8 JSON envelope. `wire::read_message` rejects a payload above
262,144 bytes before allocating its body. Truncated prefixes/bodies, unknown
fields, malformed identities and unknown commands fail with `invalid_message`.
An unsupported envelope version fails with `incompatible_version`. Malformed
transport is fatal to the harness stream, which reports the error on stderr;
ordinary command errors are framed replies and leave the stream usable.

```json
{"version":1,"operation":"request-1","command":{"type":"capabilities"}}
```

Replies carry the contract version and echo the request correlation identifier.
Their `result` is either `{"Ok":{"type":"..."}}` or `{"Err":"..."}`.
`begin_operation` returns a separately allocated `OperationId` in
`Ok.operation`; `complete_operation.pending` names that ID. The controller never
reuses these operation IDs during its lifetime. Repeating a request correlation
identifier cannot resurrect a completed operation.

The `Owner` passed to `Controller::dispatch(&Owner, Envelope)` is trusted
transport context. It is not extracted from the JSON. Each targeted command
must provide the matching workspace, session, BrowserId and document generation.
A mismatch cannot select another workspace or the active page. Completion checks
ownership and the document generation again. The only exception to matching the
current document generation is releasing a frame already owned by that caller;
the full old frame token must still exist.

The standard harness stream is bound by its process launcher:

```sh
cargo run -p paneflow-browser-protocol --bin paneflow-browser-harness -- workspace session --deterministic
```

For deterministic multi-workspace tests, the launcher may instead supply
`--batch (workspace session framed-file)... --deterministic`. Each file is a
separate, preassigned transport route, processed in argument order by one
controller. JSON cannot choose another route. File paths and the deterministic
switch are harness launch arguments, never browser-page capabilities. Native
IPC authentication and agent authorization remain their later integration
stories' responsibility.

## Domain, lifetime and bounds

The domain contains BrowserId, ProfileId, WorkspaceId, SessionId, OperationId,
Owner, Document, BrowserSession, BrowserPresentation and explicit errors. IDs
contain 1 to 64 ASCII letters, digits, hyphens or underscores. There are no CEF,
GPU, operating-system handles or filesystem paths in these domain types.

Creation yields a Dormant descriptor. Starting yields Hidden, presenting yields
Visible, and unmounting/hiding preserves the document identity and live page
slot. Navigation advances a controller-wide monotonic document generation and
invalidates presentation and pending operations. Closing removes the descriptor
only after all accepted frame tokens are released; otherwise it returns busy.
Recreating the same durable BrowserId obtains a fresh document generation.
Generation exhaustion refuses further allocation rather than wrapping. A
supervisor must invalidate its transport epoch when replacing a controller;
these counters are scoped to one controller lifetime.

The controller applies these bounds through the same dispatch entry:

| Boundary | Accepted maximum | At maximum plus one |
| --- | --- | --- |
| Descriptors per owning Agents session | 8 | limit_reached |
| Descriptors per controller instance | 64 | limit_reached |
| Live pages per controller instance | 8 | limit_reached |
| Pending operations per workspace | 4 | busy |
| Pending operations per controller instance | 16 | busy |
| Pending mutations per BrowserId | 1 | busy |
| Canonical URL | 8 KiB UTF-8 | too_large |
| Input per operation | 64 KiB UTF-8 | too_large |
| Title | 512 Unicode scalar values | too_large |
| Control payload | 256 KiB | too_large before body allocation |

No limit removes an existing descriptor or affects terminals. Live descriptors
from the same workspace share one ProfileId; different workspaces cannot share
a profile while their descriptors are registered. Disk persistence, trusted
profile assignment, profile-root locking and CEF RequestContext isolation are
US-011/US-016 work, not claims made by this in-memory model. The canonical URL
boundary admits HTTP(S) and `about:blank`, rejects userinfo, control characters
and privileged schemes. Address-bar normalization belongs to its UI story.

Operation-slot commands are deterministic contract probes. They execute no
browser action and expose no CLI/MCP `browser.*` service. Navigation or closing
invalidates pending IDs, and a completed pending ID cannot complete again.
Agent permissions, leases, cancellation events, timeouts, snapshots, captures,
diagnostic buffers and their output quotas remain R3 work.

## Frame ownership contract

A frame message carries envelope version plus frame `contract_version`, owner,
BrowserId, document generation, pool generation, buffer slot and sequence. Pool
generations increase for each dimensions, format or device replacement within a
document. Sequence numbers increase within a pool. Slots are 0, 1 and 2.

After `frame_accepted`, the producer must treat that token as consumer-owned.
It must not write its resource or close its handle until `frame_released` for
the exact document/pool/slot/sequence tuple. An occupied slot cannot be offered
again. Unknown or duplicate acknowledgements do not free another token. A new
document can reuse a slot number while its old pool retires because its complete
token is different. The platform adapter must preserve this complete identity
when binding native handles.

There is one active pool of at most three buffers and at most one old pool still
holding buffers. A replacement that would create a third pool returns busy,
leaving state unchanged. The caller coalesces resize requests until it can retry.
A new document invalidates the former presentation immediately, while its exact
old acknowledgements remain admissible. A stale frame cannot be presented.

The native adapter must signal GPU completion before acknowledging, copy any
callback-scoped CEF resource before returning it, and keep the render thread
nonblocking. Retirement beyond one second requires a device-error/recovery path
without freeing a resource still in use. These timing/fence and physical buffer
allocation obligations belong to the native adapter; token tests do not measure
or certify them. The contract does not prescribe a CPU refresh fallback.

## Presentation geometry, input and the frame channel

`BrowserPresentation.width` and `height` are logical pixels, the CSS viewport
the page lays out in. `scale_percent` (50 to 400, default 100) is the device
scale the host reports to Chromium; the produced frames measure
`width * scale_percent / 100` device pixels. A scale change is a generation
change exactly like a size change: the same `present` with a stale generation
returns `stale_generation`, and frames of the previous pool become stale.

`input` carries one `InputEvent` per command: `mouse_move`, `mouse_leave`,
`mouse_button` (left, middle, right; `clicks` 1 to 3), `mouse_wheel` and `key`
(`raw_down`, `down`, `up`, `char` with UTF-16 `character` units) plus `focus`.
Coordinates are logical pixels within plus or minus 32,768 and modifiers use
the CEF event-flag bits exported as `MODIFIER_*`; anything else fails with
`invalid_message`. Input is refused with `unavailable` on a Dormant page and
answered with `input_accepted` otherwise; the host forwards it to the page.

IME composition travels through the same command as three additive events:
`ime_composition` (`text` plus a UTF-16 `cursor` at or before the end of the
text), `ime_commit` (`text`) and `ime_cancel`. Composition and commit text are
bounded by the 64 KiB per-operation input limit and refuse control characters.
The host maps them to CEF `ImeSetComposition`, `ImeCommitText` and
`ImeCancelComposition`.

Navigation chrome uses dedicated commands, added in contract version 2 as an
additive extension: `history` (`direction`: `back` or `forward`), `reload`
(`ignore_cache`), `stop`, `zoom` (`percent`, 25 to 500) and `mute` (`muted`).
Each needs a live, generation-matched document: a Dormant page answers
`unavailable`, a stale document `stale_generation`, an out-of-range zoom
`invalid_message`, and success is `navigation_accepted`. The Linux host
reports the resulting state through native events: `loading` (`is_loading`,
`can_go_back`, `can_go_forward`), `title`, `address`, `loaded` and
`load_failed` (`main`, `error_code`, bounded `error_text`). An address bar or
command entry normalizes its input with `normalize_address` before `navigate`:
`localhost`, `*.localhost`, IPv4, bracketed IPv6 and any host with a port
take `http`, a dotted host without a port takes `https`, explicit `http`/`https`
schemes are kept, `about:blank` is accepted, and every other scheme, userinfo,
whitespace or unparsable string is refused with `invalid_url` instead of
becoming a search. The 8 KiB URL bound applies before parsing.

Frames never travel over the control pipes. The parent creates one
`AF_UNIX` `SOCK_SEQPACKET` socketpair, passes its child end as descriptor 3 and
names it in `PANEFLOW_BROWSER_FRAME_FD`. `FrameChannel::from_environment`
refuses standard streams and any socket that is not sequenced-packet. Each
datagram is the common four-byte length plus JSON, at most 256 KiB and at most
12 descriptors carried by `SCM_RIGHTS` and received with `MSG_CMSG_CLOEXEC`;
truncated data or control payloads reject the message. Host to consumer:

| Message | Content | Descriptors |
| --- | --- | --- |
| `pool_created` | document, `pool_generation`, size in device pixels, `bgra8` or `rgba8`, DRM modifier, three `buffers` with one plane each (stride, offset, size) | one DMA-BUF per plane, ordered by slot then plane |
| `frame` | document, pool, `buffer`, `sequence`, `callback_ns` and `ready_ns` (host CLOCK_MONOTONIC), `capture_timestamp_us`, optional `dirty` rectangle | none |
| `pool_retired` | document, pool | none |
| `failed` | document, `reason` (`unsupported_modifier`, `wrong_device`, `invalid_handle`, `retire_timeout`, `copy_failed`, `unsupported_format`), bounded `detail` | none |

The consumer answers with `release` carrying the exact document, pool, slot
and sequence. The host keeps three buffers per pool and at most two pools, so
a replacement holds six images; the consumer keeps at most two frames pending
behind the presented one per live pool and treats more than three outstanding
frames per live pool (six while an old pool drains) as a host fault. When no
slot is free the host drops
the CEF frame instead of blocking or copying to system memory. A retiring pool
still held after 1,000 ms is reported as `retire_timeout`; the host forgets its
tokens and leaks that memory deliberately rather than freeing an image the
consumer may still sample. Every `failed` reason maps to the Browser error the
adapter reports; none of them enables a CPU refresh path.

## Availability and evidence

The versioned availability vocabulary is `absent`, `development`,
`human_qualified`, `agent_qualified`. The harness reports its target architecture
and OS. Its default is absent; `--deterministic` enables development contract
probes only. Neither mode grants a native qualification. `capabilities` always
retains `terminal_available: true`; an absent adapter refuses creation with
unavailable and creates no fake descriptor. Native per-target availability must
also be enforced by the launcher and runtime manifest.

Every acceptance test below starts the actual harness binary and exchanges the
length-prefixed JSON protocol. Multi-workspace cases use its trusted batch
transport. Run:

```sh
cargo test -p paneflow-browser-protocol --locked
cargo clippy -p paneflow-browser-protocol --all-targets --locked -- -D warnings
cargo fmt --package paneflow-browser-protocol --check
```

| US-003 criterion | Entry and implementation | Executable proof in tests/harness.rs |
| --- | --- | --- |
| Portable identities and explicit errors | main::serve -> wire::read_message -> Controller::dispatch; domain.rs | scope_comes_from_the_transport_and_never_falls_back_to_an_active_page |
| Versioned messages and C1/C3 input limits | wire.rs; controller.rs::create and command dispatch | framing_rejects_oversize_truncation_unknown_fields_and_versions; bounded_url_title_and_input_accept_the_limit_and_reject_limit_plus_one; descriptor_and_live_page_limits_are_global_and_do_not_remove_existing_pages; operations_enforce_workspace_global_and_per_browser_mutation_limits |
| Session/presentation lifetime and versioned frame ownership | Controller::dispatch Present/Navigate/Frame/ReleaseFrame -> Frames | presentation_visibility_does_not_own_document_lifetime; frame_pools_bound_live_handles_and_require_exact_acknowledgements; navigation_retires_old_frames_and_recreated_identity_never_reuses_a_generation |
| Absent/development capabilities and terminal preservation | main::run -> Controller::new -> Capabilities/Create | absent_backend_preserves_terminal_capability_without_creating_a_browser; scope_comes_from_the_transport_and_never_falls_back_to_an_active_page |
| Bounded failure without active-page fallback | wire::read_message; Controller::session and frame checks | framing_rejects_oversize_truncation_unknown_fields_and_versions; scope_comes_from_the_transport_and_never_falls_back_to_an_active_page; navigation_retires_old_frames_and_recreated_identity_never_reuses_a_generation |
| Navigation chrome, zoom, mute and IME (EP-003) | Controller::dispatch History/Reload/Stop/Zoom/Mute/Input; domain::normalize_address | navigation_commands_need_a_live_page_and_validate_zoom; ime_events_are_bounded_and_need_a_live_page; domain::tests::* |
| Sleep returns a page to Dormant and frees its live slot (EP-003) | Controller::dispatch Sleep, idempotent, identity and generation kept | descriptor_and_live_page_limits_are_global_and_do_not_remove_existing_pages |

Linux execution validates this portable harness. No macOS/Windows execution,
CEF bootstrap, native terminal preservation experiment or GPU frame measurement
is implied by these tests.

## Native pool initialization

The native frame channel uses the control connection's version 2 contract.
`PoolCreated` exports three GPU-initialized images in GENERAL layout owned by
QUEUE_FAMILY_FOREIGN_EXT. The producer waits for `PoolReady` with the exact
Document and pool generation before copying useful content. The consumer imports
all three images, initializes their wgpu resource tracking, returns ownership,
and observes GPU completion and successful validation before acknowledging.

`PoolRejected` means that the consumer never imported or submitted work for that
pool. It releases a PoolCreated made obsolete before delivery by navigation.
Only an initializing pool with the exact identity accepts this acknowledgement.
It must never substitute for PoolReady after a GPU operation was submitted.

Initializing pools count against the two-pool, six-buffer maximum. Deadlines stop
production and retain outstanding allocations; they never imply GPU completion.
All asynchronous acknowledgements remain attached to their original host
connection. Each frame transfers GENERAL/FOREIGN ownership to the consumer's
shader-read layout and returns it after scene replacement and GPU completion.
