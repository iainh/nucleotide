# Nucleotide remote protocol v5 design

Status: implemented; post-v5 hardening updated 2026-07-14

This document defines the implemented `nucleotide-remote` v5 workspace protocol around two protocol-level changes:

1. Slow operations must not block fast operations. The protocol needs multiplexing, cancellation, streaming results and backpressure.
2. Remote file tree freshness should be event-driven where possible. The helper should watch the target filesystem and send coalesced invalidation events instead of making the UI poll expanded directories every few seconds.

The user goal is a remote workspace that feels local: file tree changes appear quickly, foreground requests are not stuck behind searches or process output, and the connection sends only the data the UI still needs.

## Current behaviour

Protocol v5 is the active remote workspace protocol. It uses fixed binary frames, protobuf control messages and raw `DATA` bodies over SSH, WSL or local stdio. Each peer multiplexes requests through one reader and a dedicated wake-driven writer. The server admits work through bounded class-specific pools and schedules response traffic by priority.

The implementation includes:

- Concurrent odd/even streams, negotiated limits and capabilities, cancellation, deadlines, progress, partial results and per-stream compression.
- Flow-controlled file, process and search data with lazy, shared-buffer DATA producers instead of eager per-frame copies.
- Atomic reset and teardown that purge queued frames, flow state and request state before a terminal frame is sent.
- Receive-window validation, exact per-direction frame sequencing, decoded chunk limits, declared-length checks and cumulative request/response limits.
- Connection-terminal client write failures that fail all waiters, invalidate caches and preserve typed ambiguous outcomes for mutations.
- Cancel-on-drop client request handles and workspace futures, plus callback-driven explicit filesystem cancellation, that reset abandoned streams without blocking the dropping thread.
- Server cancellation on peer loss and enforced shutdown-grace cancellation for queued and cooperative active work.
- Count- and byte-bounded watch batches, explicit overflow/resync events and bounded client delivery.
- Directory generations, fingerprints, `not_modified` responses, deltas and bounded server-side directory caching.
- Safe read-only reconnect replay, configuration-derived startup options, bounded probe/handshake processes, OpenSSH connection reuse, transport keepalives, negotiated bidirectional heartbeat probes and an independent client-side watchdog.
- Bounded, success-only bootstrap reuse that single-flights platform probes and validated helper resolution for the same exact SSH identity or WSL distro without reusing a workspace backend across roots.

Post-v5 hardening also bounds the server producer-to-writer lanes, gives each peer's writer sole ownership of physical writes, propagates validated priority through worker admission and response scheduling, limits queued wire batches and gives application request ownership an explicit cancellation lifetime. The server writer revalidates extracted frames and reports terminal failures without blocking the service loop's heartbeat, deadlines, cancellation or shutdown grace. Reconnecting clients retain logical watch registrations, recreate their physical subscriptions and require a full resync before exposing replacement-connection batches. The associated regression tests cover partial writes, blocked-flow resets, dropped handles, decompression bounds, watch storms, EOF cancellation, blocked server writes, reconnect ambiguity and framed watch restoration.

Some architectural work remains. File reads, searches and process execution now expose incremental workspace streams, retain queued data within negotiated connection and stream bounds, and return receive credit only as callers consume it. Their compatibility APIs collect those streams when a whole result is required. The remote file picker and global-search worker consume search batches directly. Explicit `WorkspaceCancellationToken` callbacks remain attached for each stream's lifetime. Every existing token-aware remote filesystem method forwards `WorkspaceCancellationToken` through a callback to its live transport stream. Filesystem handlers cooperate with cancellation between user-space work units, but no user-space deadline can force a filesystem call already blocked in the operating system to return. Child handshakes and command probes have deadlines and terminate local descendants through a process group on Unix or a Job Object on Windows. A Windows Job Object contains the local SSH or WSL launcher tree, not processes on an SSH host or Linux processes inside WSL; those still rely on transport teardown and EOF/HUP behaviour. The browse backend is deliberately not handed off: only pathless bootstrap facts are shared, and every selected root starts a new contained service. These are follow-up lifecycle and integration changes, not missing v5 wire primitives.

## Design goals

- Lower perceived latency for foreground editor, file tree and status interactions.
- Lower bandwidth by replacing idle polling with watch invalidations, batching event traffic and avoiding repeated full directory snapshots.
- Let the UI cancel work that is no longer useful.
- Stream large payloads and partial results so the UI can render useful information before an operation finishes.
- Bound memory and network pressure with per-stream and per-connection flow control.
- Keep the current operational model: `nucleotide-remote` is launched over SSH or WSL stdio, with no listening port required.
- Preserve workspace write containment and the current read-only external-file escape hatch used for LSP/toolchain navigation.
- Detect incompatible older helpers and replace them atomically with a verified v5 binary.

## Non-goals

- No long-running per-host daemon in this protocol revision. A daemon can reuse the same v5 transport later, but v5 must work for one helper process per workspace.
- No replacement for SSH authentication, encryption or host verification.
- No attempt to make filesystem notifications perfect. Watch events are invalidation hints; authoritative state still comes from remote-side list/stat/read operations.
- No automatic retry of mutating operations after reconnect unless a later design adds durable mutation IDs.
- No generic HTTP/2 or gRPC dependency. v5 borrows proven transport ideas, but keeps a small protocol that fits stdio and the existing Rust workspace backend.

## Industry precedents used

- HTTP/2 shows why multiplexed streams, binary frames, flow control and prioritization reduce application-layer head-of-line blocking on one connection. RFC 9113 explicitly ties concurrent exchanges, interleaving, field compression and prioritization to reduced latency and better network use. [RFC 9113][rfc9113]
- LSP uses capability negotiation, request IDs, cancellation, progress notifications, partial-result progress and watched-file notifications. These are directly relevant to editor responsiveness. [LSP 3.17][lsp-317]
- gRPC cancellation guidance says a server should stop ongoing computation after client cancellation and propagate cancellation to upstream work. That maps to search walkers, git commands, process execution and environment capture. [gRPC cancellation][grpc-cancel]
- Protocol Buffers gives compact typed control messages with stable field numbers and explicit compatibility rules. Protobuf guidance says field numbers identify wire fields, lower field numbers use less space, and deleted fields must be reserved. [Protobuf proto3][protobuf-proto3]
- Linux `inotify` documents queue overflow and non-recursive directory watching. Watchman documents recrawl as the recovery path after losing sync and recommends avoiding overlapping watches in the same tree. These shape the watch reliability model. [inotify][inotify] [Watchman recrawl][watchman-recrawl]
- VS Code Remote SSH validates the architecture of running editor support code on the target host, and its docs call out SSHFS as significantly slower for bulk file activity than working through the remote server. [VS Code Remote SSH][vscode-remote-ssh]

## Protocol overview

What changes:

- v5 keeps one long-lived stdio connection per remote workspace.
- The connection carries many independent streams.
- Each request, response body, process output channel and watch subscription uses a stream.
- Frames are binary, fixed-header records with a compact typed control header and optional raw body bytes.
- Control messages default to protobuf. Raw file bytes, process bytes and search result payloads stay outside protobuf bodies.
- Client and server negotiate capabilities during a hello/settings handshake.
- The server can initiate event streams for watch batches and health/status notifications.

Why:

- One stdio connection is still the lowest-friction fit for SSH and WSL.
- Multiplexing fixes the main latency issue: a visible file tree refresh can make progress while a search stream is still producing results.
- Typed control messages reduce repeated JSON field names on high-frequency frames and give a safer compatibility story than ad hoc JSON maps.
- Server-initiated events are required for file watching. Without them, the client can only discover remote changes by polling.

## Frame layer

All integers in v5 frames are network byte order. v5 uses the `NUC2` magic as the only supported remote workspace wire family. Peers that see any other magic must close the connection with a clear protocol diagnostic; no alternate remote protocol decode is attempted.

Version identifiers have separate jobs:

- `magic = "NUC2"` selects the v5 wire family.
- `frame_header_version = 2` selects the fixed header layout below. A peer that sees an unsupported header version must close the connection; the client reports the mismatch and may reinstall or update the helper, but it must not retry using an older protocol.
- `protocol_major` and `protocol_minor` are negotiated in `HELLO` and describe semantic compatibility above the frame header.

Fixed frame header:

```text
offset  size  field
0       4     magic = "NUC2"
4       2     frame_header_version = 2
6       2     frame_type
8       2     flags
10      1     priority
11      1     reserved
12      8     stream_id
20      8     frame_sequence
28      4     control_len
32      4     body_len
36      12    reserved
```

Rules:

- `control_len` and `body_len` describe this frame only, not the whole stream.
- `frame_sequence` starts at 1 and increments by exactly one per direction. Gaps, duplicates and regressions are connection protocol errors. Each peer owns the sequence numbers on frames it writes. It is not a retransmission mechanism.
- Default maximum frame body is 64 KiB. Peers may negotiate up to 1 MiB for bulk streams.
- Default maximum control message is 64 KiB. This prevents accidental unbounded metadata frames.
- A receiver must close the connection with `FRAME_TOO_LARGE` if a peer exceeds negotiated limits.
- `stream_id` must not wrap. A peer that cannot allocate another odd or even stream ID must send `GOAWAY` and reconnect.

Frame types:

| Type | Direction | Meaning |
| --- | --- | --- |
| `HELLO` | both | Version, codec and capability negotiation. |
| `SETTINGS` | both | Runtime limits such as max frame bytes, stream window and max concurrent streams. |
| `SETTINGS_ACK` | both | Confirms settings were applied. |
| `HEADERS` | both | Opens a stream or sends response/error/event metadata. |
| `DATA` | both | Carries body bytes for the stream. |
| `END_STREAM` | both | Marks that no more frames will be sent on this stream. |
| `RESET_STREAM` | both | Cancels or fails one stream without closing the connection. |
| `WINDOW_UPDATE` | both | Grants more per-stream or connection-level send credit. |
| `PING` | both | Health check with opaque payload. |
| `PONG` | both | Response to `PING`. |
| `GOAWAY` | both | Graceful shutdown; carries the highest stream accepted. |

Why:

- This is intentionally close to the proven HTTP/2 stream model without importing HTTP semantics. HTTP/2 associates each exchange with an independent stream so a stalled stream does not prevent progress on others, and effective multiplexing depends on flow control and prioritization. [RFC 9113][rfc9113]
- Fixed-size binary frame headers are cheap to parse and keep large file/process bytes out of control serialization.
- `END_STREAM` is a standalone frame rather than a flag so stream closure is represented the same way for requests, responses and event subscriptions. The 48-byte empty frame cost is acceptable next to remote round trips and keeps per-frame flags narrow.

## Stream model

Stream IDs:

- Client-initiated streams use odd IDs.
- Server-initiated streams use even IDs.
- Stream ID 0 is reserved for connection-level frames.
- A stream may carry request/response messages, a file body, a process channel or a watch/event subscription.
- In v5.0, `request_id` in stream metadata must equal the opening `stream_id` for client-initiated request streams. The `stream_id` is the transport routing key; `request_id` exists for logs, UI correlation and compatibility with request-oriented APIs. Server-initiated event streams use `request_id = 0` unless the event explicitly belongs to a client request.

Lifecycle:

```text
idle -> open -> half_closed_local/half_closed_remote -> closed
             \-> reset -> closed
```

Request flow:

1. Client sends `HEADERS` with a `RequestHeader`.
2. Client may send `DATA` frames for request bodies, such as file writes or process stdin.
3. Client sends `END_STREAM` when request input is complete.
4. Server may send progress, event or partial-result `HEADERS` and `DATA` frames while work runs.
5. Server sends a `HEADERS` frame with `role = final_response` or `role = final_error`.
6. Server sends `END_STREAM`.

The protocol allows multiple response messages before the final response. For example, `search.text` can send `role = partial_result` batches, then one `role = final_response` tail for any unflushed matches. `END_STREAM` closes the stream but does not replace the final response or final error marker. If a stream is aborted before a final message, the peer sends `RESET_STREAM`.

Why:

- Editor protocols already use request IDs, progress and partial results to keep UI responsive. LSP specifically supports request cancellation and progress/partial-result notifications. [LSP 3.17][lsp-317]
- Stream IDs let the transport demultiplex responses without relying on one blocking request/response loop.

## Control message encoding

v5 stream-control payloads use protobuf by default. They appear in `HEADERS` frames, and optional per-frame metadata may appear in `DATA` frames.

```protobuf
message StreamEnvelope {
  uint64 request_id = 1;
  string method = 2;
  uint64 correlation_id = 3;
  uint64 deadline_unix_ms = 4;
  Priority priority = 5;
  MessageRole role = 6;
  string cancellation_group = 7;
  uint64 supersedes_stream_id = 8;
  ContentEncoding content_encoding = 9;
  oneof message {
    RequestHeader request = 10;
    ResponseHeader response = 11;
    ErrorHeader error = 12;
    Progress progress = 13;
    Event event = 14;
  }
}

message DataEnvelope {
  DataChannel channel = 1;
  uint64 uncompressed_len = 2;
}
```

Rules:

- Field numbers 1 through 15 are reserved for hot-path fields.
- `correlation_id` is an optional trace ID for diagnostics. It is not a routing key. A value of `0` means "not set."
- `MessageRole` is one of `request`, `partial_result`, `final_response`, `final_error`, `progress` or `event`.
- `content_encoding` is set on the opening `HEADERS` for a stream. It applies only to `DATA` frames on that stream and cannot change mid-stream. Control payloads are never compressed.
- `DataEnvelope.channel` distinguishes `stdin`, `stdout`, `stderr`, `file_body` and `search_payload` when a method needs multiple logical byte channels.
- Deleted fields must be added to a `reserved` block in the `.proto` file.
- Unknown fields are ignored unless the peer advertises a strict compatibility mode for tests.
- The first v5 implementation should keep a JSON debug dump for logs/tests, but JSON is not the default wire codec.

Connection-control frames use their frame type plus a small typed payload, not `StreamEnvelope`:

- `HELLO` carries `ClientHello` or `ServerHello` on stream 0.
- `SETTINGS` carries runtime limits on stream 0.
- `SETTINGS_ACK` carries no payload.
- `RESET_STREAM` carries a reason code and optional diagnostic.
- `WINDOW_UPDATE` carries `credit_bytes`; `stream_id = 0` updates the connection window and any other stream ID updates that stream.
- `PING` and `PONG` carry an opaque token.
- `GOAWAY` carries `last_accepted_stream_id`, an error code, an optional message and `drain_grace_ms`.

Why:

- Protobuf gives compact field encoding and stable schema evolution rules. Its documentation warns that field numbers identify the wire format and should never be reused; it also recommends reserving deleted fields to prevent future ambiguity. [Protobuf proto3][protobuf-proto3]
- A typed schema makes compatibility testable and avoids ad hoc growth of JSON headers.

## Handshake and capability negotiation

The handshake uses connection-level `HELLO`, `SETTINGS` and `SETTINGS_ACK` frames on stream 0. It does not use a `session.hello` method.

Client starts with `HELLO` carrying `ClientHello`:

```text
protocol_major = 5
protocol_minor = 0
client_name = "nucleotide"
client_version
control_codecs = ["protobuf"]
capabilities = [
  "multiplex",
  "cancel",
  "progress",
  "partial_results",
  "streaming_read",
  "streaming_write",
  "process_streams",
  "watch",
  "watch_overflow",
  "directory_not_modified",
  "compression_zstd",
  "external_read_only"
]
required_capabilities = []
desired_settings = {
  max_concurrent_streams = 128
  initial_stream_window = 1048576
  initial_connection_window = 4194304
  max_frame_body = 65536
}
```

Server replies with accepted version, codec, settings, target platform, helper version, workspace root and supported capabilities.

Capability meanings:

- `watch_overflow` means watch batches can explicitly report overflow and `resync_required`.
- `directory_not_modified` means `fs.list_dir` and `fs.list_dirs` can return `not_modified = true` when a supplied generation or fingerprint is still current.
- `compression_zstd` means a stream may set `content_encoding = zstd` in its opening `HEADERS`.
- `external_read_only` means absolute paths outside the workspace root are accepted only by `fs.read` and `fs.stat`.

Compatibility rules:

- Major versions must match.
- Minor versions are additive and capability-gated.
- `capabilities` lists optional features the client can use; `required_capabilities` lists features the client refuses to run without.
- If the helper does not understand v5, the client fails the connection with an actionable helper update or reinstall diagnostic.
- If a v5 helper lacks `watch`, the client keeps the existing remote polling path.
- A v5 server must reject unknown required capabilities with `UNSUPPORTED_CAPABILITY`.

Why:

- LSP uses initialization-time capability flags to keep features compatible across client/server versions. [LSP 3.17][lsp-317]
- Remote helpers can lag behind the app binary, especially on SSH hosts where auto-install may be disabled.

## Priorities and scheduling

Priority values:

| Priority | Examples | Scheduler intent |
| --- | --- | --- |
| `0 user_input` | Save, rename, open selected file, explicit refresh | Run first; small bounded work should finish immediately. |
| `1 foreground_document` | Read visible file, stat visible path, completion support file reads | Prefer over background tree/search work. |
| `2 visible_file_tree` | List expanded dirs, apply watch invalidations | Keep navigation fresh. |
| `3 lsp_support` | Read external read-only toolchain file, project environment | Important but may be slower. |
| `4 background` | Text search, file search, git status, reconciliation polling | Must not starve foreground streams. |
| `5 bulk` | Large file transfer, long process output | Always flow-controlled. |

Implementation model:

- The client transport has one reader thread, one wake-driven writer thread and an in-flight stream map.
- The server transport has one reader thread, one wake-driven writer thread and a service-loop-owned scheduler.
- Handlers run in bounded task pools by class: metadata, file body, search, git/env and process.
- The writer picks frames using priority plus round-robin fairness so a single bulk stream cannot monopolize the connection.

Why:

- HTTP/2's performance model uses interleaving plus prioritization so more important requests can complete quickly on a shared connection. [RFC 9113][rfc9113]
- The current single service loop can make a cheap request wait behind an expensive request even when the remote host and network are otherwise idle.

## Flow control and backpressure

Settings:

- Default per-stream window: 1 MiB.
- Default connection window: 4 MiB.
- Default data frame body: 64 KiB.
- Peers may lower these values during handshake.

Rules:

- `DATA` consumes stream and connection window according to its decoded length, so compression cannot bypass retained-memory backpressure.
- `HELLO`, `SETTINGS`, `SETTINGS_ACK`, `HEADERS`, `END_STREAM`, `RESET_STREAM`, `WINDOW_UPDATE`, `PING`, `PONG` and `GOAWAY` do not consume flow-control window.
- A receiver sends `WINDOW_UPDATE` after bounded transport/backend storage accepts the bytes, not merely after reading from stdio. Incremental application consumers should delay this update until they release channel capacity.
- When the peer closes its sending side or resets a stream, the receiver removes that stream's receive window without refunding unconsumed bytes. Those bytes remain bounded connection-level debt; later consumption restores connection credit only and never sends `WINDOW_UPDATE` for a terminal stream.
- If a sender exhausts stream credit, it must pause that stream and allow other streams with credit to progress.
- Non-`DATA` frames are still bounded. Default queued control bytes are 1 MiB per connection and 256 KiB per stream. A peer that exceeds either budget gets `RESOURCE_EXHAUSTED` or connection-level `GOAWAY`. Small `RESET_STREAM`, `WINDOW_UPDATE`, `PING` and `PONG` frames use a reserved urgent lane so saturation cannot prevent teardown or liveness traffic.
- Both peers send unsolicited health-check `PING` frames at the negotiated idle cadence; the default is 30 seconds and negotiation never lowers it below the accepted unsolicited-ping minimum. Each permits only one outstanding heartbeat and requires the exact encoded control payload in `PONG`. Replies to received `PING` frames do not count as unsolicited pings.

Default retained decoded-byte budgets are 260 MiB for all client outbound requests, 260 MiB for all server request accumulators, 64 MiB for all client response accumulators and 64 MiB for server completion payloads being serialized. The server worker-output channel retains at most 64 events of 64 KiB each, and the scheduler admits at most another 64 lazy items. These are logical payload bounds; allocator spare capacity, domain objects constructed before serialization and parsed object overhead are not byte-exact.

Why:

- Flow control is what makes multiplexing reliable under large reads, writes and process output. HTTP/2 calls out flow control as necessary so a receiver only gets data it can handle. [RFC 9113][rfc9113]
- Without explicit backpressure, streaming can trade latency problems for memory growth problems.

## Cancellation, deadlines and reset

Request metadata may include:

- `deadline_unix_ms`
- `cancellation_group`
- `supersedes_stream_id`
- `idempotency = read_only | mutation | process`

Every production multiplexed-client request has one immutable request context that can carry two independent limits:

- The absolute deadline bounds every request attempt and any safe reconnect replay from one context creation point. It does not restart when a stream opens, makes progress or reconnects, and no new stream may open after it expires.
- The inactivity deadline bounds one transport attempt. It restarts only after targeted progress on that stream. A safe replay starts a new inactivity interval but retains the original absolute deadline.

For a finite absolute deadline, the client records a monotonic local deadline and derives `deadline_unix_ms` once when it creates the context. It sends that same wire deadline on every attempt. Wall-clock changes cannot extend the local budget, and reconnect must not calculate a new wire deadline. The inactivity limit is local client policy; it is not encoded in request metadata or negotiated with the helper. A reconnect or startup factory already in progress cannot yet be interrupted at the request deadline; the startup-cancellation work below must close that remaining outer-bound gap.

The client uses these conservative defaults:

| Methods | Absolute deadline | Inactivity deadline |
| --- | ---: | ---: |
| `fs.stat`, `fs.list_dir`, `fs.list_dirs`, `fs.find_ancestor`, `git.head`, `git.status` | 60 seconds | 30 seconds |
| `env.project`, `fs.create_file`, `fs.create_dir`, `fs.rename`, `fs.delete`, `fs.copy` | 120 seconds | 30 seconds |
| `fs.read`, `fs.write` | 5 minutes | 60 seconds |
| `search.files`, `search.text` | 10 minutes | 120 seconds |
| `process.run` with `timeout_ms` | `timeout_ms` plus 15 seconds | No inactivity limit |
| `process.run` without `timeout_ms` | Unlimited | Unlimited |
| `session.shutdown`, `watch.start`, `watch.update`, `watch.stop`, `watch.resync` | 15 seconds | 10 seconds |

The synchronous protocol conformance client also sends the fixed absolute wire deadline, but it has no independent watchdog for a generic blocking `Read`. The workspace backend uses the multiplexed client and its local deadline worker.

The `process.run` runtime timeout and protocol deadline have different purposes. `timeout_ms` limits child execution on the helper and returns a normal process result with `timed_out = true`. The additional 15 seconds bounds request delivery, response delivery and cancellation cleanup; it does not extend the child's runtime. A process without `timeout_ms` is explicitly unlimited because a valid process may remain silent indefinitely. Heartbeats, explicit cancellation and process-output limits still protect that connection.

Only progress attributable to the target stream extends its inactivity deadline:

- Successfully writing that stream's request `HEADERS`, `DATA` or `END_STREAM` frame.
- Accepting and routing that stream's `HEADERS`, `DATA`, `END_STREAM` or `RESET_STREAM` response frame.
- Accepting a `WINDOW_UPDATE` addressed to that stream.

Queue admission, buffered but unwritten bytes, connection-level frames, heartbeat traffic and frames for other streams do not extend the interval. Traffic on another stream may prove that the connection is healthy, but it is not progress for the stalled request.

On local expiry, the client keeps the failure stream-scoped only when recent accepted inbound traffic and heartbeat state prove that the peer is healthy. Health is known only while the last accepted inbound frame is newer than the negotiated idle-ping interval and no local heartbeat is queued or awaiting its exact `PONG`. The client removes a healthy read-only waiter, sends `RESET_STREAM DEADLINE_EXCEEDED` once and returns a typed request-deadline error. It uses the same stream-local result for a mutation or process request whose final response metadata has already established the outcome. The connection remains available to other streams. A peer-sent `DEADLINE_EXCEEDED` reset has the same typed read-only result.

If peer health is unknown, the client treats the expiry as connection-terminal and fails every waiter exactly once. A read-only request may replay once after reconnect only when its original absolute deadline still permits it. Before final response metadata arrives, mutations and process requests are never replayed: their expiry or a peer-sent deadline error closes the connection and returns an outcome-unknown error because the helper may already have performed the operation. `session.shutdown` expiry is also terminal and is not replayed.

Watch-control deadlines are connection-terminal because a timed-out control may already have changed helper state. After closing the stale transport, `watch.start` may replay once on a fresh connection with its original request context. `watch.update`, `watch.stop` and `watch.resync` are not replayed against a possibly different subscription. The persistent server-initiated watch event stream has no request inactivity deadline; connection heartbeats and watch reconciliation govern its liveness.

Cancellation forms:

- `RESET_STREAM CANCELLED` cancels one stream.
- Dropping a live `RemoteWorkspaceV5RequestHandle`, or a polled remote workspace future that has not consumed its worker result, queues cancellation without taking the session or waiter locks on the dropping thread. The client control worker removes the waiter, returns a typed cancellation result, resets an open stream exactly once and owns request/response reservation release.
- The cancellation action remains armed until the application consumes the worker result. If an early final response has already closed the logical stream while a request body is still flow-blocked, dropping the future purges the unsent scheduler state and releases its reservation directly; it does not emit a second terminal frame.
- Dropping a raw watch-control request closes the connection because the helper may already have changed persistent watch state. A dropped successful `watch.start` result also attempts compensating `watch.stop` cleanup.
- A client cancels a group by sending `RESET_STREAM CANCELLED` for each open stream it owns in that `cancellation_group`.
- `supersedes_stream_id` lets a new request declare that it replaces one older stream. If the older stream is still active, the server treats it as cancelled.
- A server-side deadline expiry cancels its work and sends `RESET_STREAM DEADLINE_EXCEEDED`.
- A healthy client-side read-only deadline expiry behaves like cancellation with reason `DEADLINE_EXCEEDED`. Ambiguous or unhealthy client cases use the connection-terminal rules above.

Server obligations:

- Search walkers check cancellation between directory/file batches.
- Git and process handlers terminate child process groups on cancellation.
- Project environment capture terminates the shell process group on cancellation.
- Every filesystem request receives the stream's shared cancellation token. Directory listing checks between entries and paths, ancestor lookup checks between candidates, and file read/copy/write checks between bounded chunks or stages.
- `fs.list_dirs` treats cancellation as a request-level result and does not start the next path or return cancellation as one embedded per-path error.
- File writes check cancellation immediately before their atomic rename and remove the temporary file when cancellation is observed. A zero-byte write uses the same staged path.
- Other filesystem mutations do not promise rollback. Cancellation may leave a partial copy or an outcome-unknown create, rename or delete once irreversible operating-system work has started.
- Response sizing, serialization and bounded output enqueue poll the same token. Worker-finished bookkeeping uses a separate control lane so stale DATA backpressure cannot retain a worker-class permit after cancellation or peer loss.
- If a request is cancelled before a final response, the stream closes with `RESET_STREAM CANCELLED` or a structured `final_error` with code `CANCELLED`, not a silent hang.

Why:

- gRPC treats cancellation as a signal that the client is no longer interested and says the server should stop ongoing computation and propagate cancellation through work started for that request. [gRPC cancellation][grpc-cancel]
- LSP also keeps cancelled requests explicit rather than leaving them open, and allows partial results on cancellation. [LSP 3.17][lsp-317]

## Method namespaces

v5 uses method names instead of one large enum. Namespaces keep compatibility clear:

| Namespace | Methods |
| --- | --- |
| `session.*` | `session.shutdown` |
| `fs.*` | `fs.stat`, `fs.list_dir`, `fs.list_dirs`, `fs.read`, `fs.write`, `fs.create_file`, `fs.create_dir`, `fs.rename`, `fs.delete`, `fs.copy`, `fs.find_ancestor` |
| `search.*` | `search.files`, `search.text` |
| `git.*` | `git.head`, `git.status` |
| `env.*` | `env.project` |
| `process.*` | `process.run` |
| `watch.*` | `watch.start`, `watch.update`, `watch.stop`, `watch.resync` |

Path model:

- Workspace methods use normalized workspace-relative paths.
- Mutating methods must reject paths that escape the workspace root after canonicalization.
- External absolute paths are allowed only for read-only methods and only if the server advertises `external_read_only`.
- Responses may include both a remote-native path and a display path when the UI needs stable presentation.

Why:

- Method namespaces make feature negotiation and compatibility easier than growing a single request enum forever.
- Workspace-relative paths reduce payload size and avoid repeating long remote roots in high-frequency watch and file tree messages.
- The read-only external path rule preserves existing LSP/toolchain navigation without broadening mutation authority.

## Streaming operations

### `fs.read`

What:

- Small files may return metadata plus one `DATA` frame.
- Larger files stream `DATA` chunks.
- A read may stream up to the 256 MiB method limit unless the caller requests a smaller prefix; the per-frame body limit controls chunking and never truncates the complete response.
- The final response includes bytes sent, content length if known, mtime, executable bit, file type and optional checksum.
- The client may cancel once it has enough preview data or after a newer read supersedes it.
- The server checks cancellation before and immediately after each bounded read and before and after each output enqueue. A chunk read concurrently with cancellation is discarded before emission.

Why:

- A large read should not fill memory or block unrelated metadata/file tree requests.
- Partial data lets the UI show progress or decide that the file is too large before the whole payload arrives.

### `fs.write`

What:

- Client sends a `RequestHeader` with path, expected mtime and write mode.
- Client streams body chunks.
- Server writes to a temp file inside the target directory, validates cancellation/mtime, then commits with rename.
- Empty writes create and commit the same temporary-file representation instead of bypassing the streaming path.
- Server returns final metadata.

Why:

- This preserves the current atomic-write behaviour while making large writes bounded and cancellable.

### `search.text` and `search.files`

What:

- Search streams result batches every 50 ms or every 100 matches, whichever comes first.
- The final response carries only the unflushed result tail and truncation status; clients aggregate earlier `partial_result` batches with that tail into the API result.
- Cancelled searches end with `RESET_STREAM CANCELLED` instead of a final response. Search telemetry such as searched/skipped file counts can be added later as optional payload fields.
- Changing the query cancels the old stream, or every open stream in the old cancellation group, with `RESET_STREAM CANCELLED`.

Why:

- Search feels faster when the first useful matches appear early.
- Batching limits frame overhead while avoiding one huge response.

### `process.run`

What:

- Process stdout and stderr are separate logical channels on the process stream.
- Output uses `DATA` frames with `DataEnvelope.channel = stdout` or `DataEnvelope.channel = stderr`.
- Process stdin, when supported, uses client-to-server `DATA` frames with `DataEnvelope.channel = stdin`.
- The final response includes exit status, signal, timed-out flag, truncated flag and resource limits reached.
- Cancellation is signalled with `RESET_STREAM CANCELLED` on the `process.run` stream and kills the process group.

Why:

- Long-running tools can surface output immediately without blocking other protocol work.
- Backpressure prevents unbounded output buffering.

## Remote watch protocol

### Watch setup

Client sends `watch.start`:

```text
roots = ["."]
mode = "expanded_dirs"
recursive = false
debounce_ms = 200
max_events_per_batch = 500
ignore_policy = "workspace"
include_hidden = true
send_initial_snapshot = false
```

Server returns:

```text
watch_id
event_stream_id
backend = "notify/inotify" | "notify/fsevents" | "notify/poll"
recursive_coverage = "none" | "partial" | "full"
degraded = false
requires_reconciliation = true
```

`event_stream_id` is an even server-initiated stream. The server opens it with a `HEADERS` frame where `role = event`, `method = "watch.batch"` and `request_id = 0`. The server must not send watch batches until after the final `watch.start` response. The stream stays open until `watch.stop`, connection shutdown or `RESET_STREAM`.

Initial implementation should watch expanded directories, matching the current polling scope. A later phase may add recursive project-root watching when the target backend can do it safely.

Why:

- Expanded-directory watching gives the same UI coverage as current polling with much lower idle bandwidth.
- Linux inotify directory monitoring is not recursive, and adding watches for every subdirectory can be expensive on large trees. [inotify][inotify]
- Watchman recommends avoiding multiple overlapping watches and watching project roots rather than subtrees when possible. [Watchman recrawl][watchman-recrawl]

### Watch updates

When the file tree expands or collapses directories, the client sends `watch.update`:

```text
watch_id
add_roots = ["src", "crates/nucleotide-remote"]
remove_roots = ["target"]
```

The server updates the watched set and returns accepted roots plus any degraded/unsupported roots.

Why:

- The watched set should track what the user can see instead of keeping a large idle recursive watch tree by default.

### Watch events

The server sends `watch.batch` events on the server-initiated `event_stream_id` returned by `watch.start`:

```text
watch_id
sequence
directory_generations = [
  { path = "src", generation = 42 }
]
events = [
  { kind = "created", path = "src/new.rs", is_dir = false },
  { kind = "modified", path = "Cargo.toml", is_dir = false },
  { kind = "renamed", old_path = "src/a.rs", path = "src/b.rs", is_dir = false }
]
overflow = false
resync_required = false
```

Client behaviour:

- Treat watch events as invalidations, not final state.
- Refresh affected parent directories with `fs.list_dirs`.
- Trigger VCS refresh for changed paths under the workspace.
- Ignore stale batches with sequence numbers older than the last applied sequence.
- Send the last locally applied `known_generation` on the next directory refresh. The server must bump the affected directory generation before it sends the watch batch, so an old `known_generation` cannot return `not_modified = true`.
- If `resync_required` is true, refresh all expanded directories and reset local watch-derived generations.

Why:

- Filesystem APIs can drop or coalesce events. Linux documents `IN_Q_OVERFLOW` when the event queue overflows; Watchman recovers by recursively scanning to resync after losing sync. [inotify][inotify] [Watchman recrawl][watchman-recrawl]
- The existing file tree code already has a safe model for applying authoritative directory refreshes. The watch protocol should feed that model rather than trying to mutate UI state from raw events alone.

### Watch fallback

Fallback order:

1. Native watch backend through `notify` on the remote helper.
2. Helper-side lightweight polling with hashes/generations for watched roots.
3. Existing client-side remote polling of expanded directories.

Even with native watches, the client should run a low-frequency reconciliation pass, for example every 60 seconds, while the file tree is visible.

Why:

- Some targets will lack a usable watcher or will hit kernel/user limits.
- A low-frequency reconciliation pass is cheap compared with current 2-16 second polling and catches missed/degraded watch states.

## Directory listing cache and deltas

What:

- `fs.list_dir` and `fs.list_dirs` responses include `generation`, `fingerprint` and `complete`.
- Requests may include `known_generation` and `known_fingerprint`.
- If unchanged, the server returns `not_modified = true` with no entry list.
- If changed and the server has the previous generation in cache, it may return a compact delta: added, removed and updated entries.
- If the server cannot compute a safe delta, it returns a full listing.
- Watch-detected changes bump the affected directory generation before the server emits `watch.batch`.
- After overflow or `resync_required`, the client omits `known_generation` for affected directories until it receives a fresh full listing.

Why:

- Watch events tell the client what likely changed, but directory listings remain authoritative.
- `not_modified` and deltas reduce bandwidth during reconciliation and after bursty watch invalidations.

## Compression

What:

- Compression is negotiated per connection and enabled per stream.
- A stream enables compression by setting `content_encoding = zstd` in its opening `HEADERS`.
- Compression applies only to `DATA` frames on that stream.
- Mid-stream compression changes are not allowed.
- Default codec: none.
- Optional codec: zstd for large text/search/directory-result streams.
- Never compress already-compressed or unknown binary file reads by default.
- Compression dictionaries are not part of v5.0.

Why:

- Compression can help repeated textual metadata, but it adds CPU and latency. It should be used where payload type and size justify it, not globally.
- The largest bandwidth reduction should come from not sending polling snapshots and cancelled/obsolete results in the first place.

## Error model

Error headers include:

```text
code
message
retryable
details
remote_errno
```

Standard codes:

| Code | Meaning |
| --- | --- |
| `CANCELLED` | Client or server cancelled the stream. |
| `DEADLINE_EXCEEDED` | Deadline expired. |
| `UNSUPPORTED_METHOD` | Method is unknown or capability-gated. |
| `UNSUPPORTED_CAPABILITY` | Required capability is not available. |
| `INVALID_ARGUMENT` | Malformed request, bad path or unsupported option. |
| `NOT_FOUND` | Path or resource does not exist. |
| `PERMISSION_DENIED` | OS or workspace policy denied the operation. |
| `CONFLICT` | Expected mtime/generation did not match. |
| `RESOURCE_EXHAUSTED` | Frame/window/queue/process/output limit reached. |
| `INTERNAL` | Unexpected helper failure. |
| `PROTOCOL_ERROR` | Invalid frame sequence or malformed control payload. |
| `UNAVAILABLE` | Helper shutting down or connection lost. |

Why:

- Stable error codes make UI recovery deterministic.
- Retryability must be explicit because read-only operations and mutations have different safety rules.
- Request errors are sent as `HEADERS` with `role = final_error`, followed by `END_STREAM`. `RESET_STREAM` is used only when the stream is aborted without a structured final error.

## Reliability rules

- Unknown stream frames after `GOAWAY` are ignored or reset.
- Unknown frame types are connection errors unless negotiated by extension.
- Unknown optional fields in protobuf control messages are ignored, except that heartbeat `PING` and `PONG` control bytes are opaque echo tokens. A solicited `PONG` must match the original encoded `PING` control bytes exactly.
- Unknown required capabilities fail during handshake.
- A peer must always close streams with `END_STREAM` or `RESET_STREAM`.
- The client may retry read-only operations after reconnect if no final response was received.
- The client must not automatically retry mutations after reconnect.
- Server logs must go to stderr only; stdout remains protocol bytes.
- Each peer sends `PING` after 30 seconds without accepted inbound traffic. The independent client watchdog closes the connection if the writer stalls or an exact `PONG` is missing for 90 seconds. The helper's service loop enforces its heartbeat and shutdown timers independently of a blocked physical write.
- On helper shutdown, server sends `GOAWAY` and drains accepted streams until their deadlines or a shutdown grace period expires. The default grace period is 5 seconds unless peers negotiate a different setting.

Why:

- The protocol should fail one stream when possible, not the whole connection.
- The connection still needs clear fatal-error boundaries for malformed frames and incompatible peers.
- Read retry safety differs from write retry safety.

## Security and containment

What:

- SSH/WSL continues to provide process launch, authentication and transport confidentiality.
- The helper does not open a listening socket for v5.
- Mutating filesystem methods are constrained to the workspace root.
- `external_read_only` paths are permitted only for read/stat operations and only when enabled in server capabilities.
- Watch roots are constrained to the workspace root in v5.0.
- Process execution inherits the existing remote workspace environment policy and carries explicit output and working-directory constraints. Callers that require a child-runtime bound must supply a process timeout; the protocol deadline is a separate request-lifecycle bound.

Why:

- Running code on the target side is the right performance model for remote development. VS Code Remote SSH uses a remote server so source code need not be local and warns that SSHFS is significantly slower for bulk file work. [VS Code Remote SSH][vscode-remote-ssh]
- The protocol should improve responsiveness without widening filesystem mutation authority.

## Client integration

Current integration:

- Backend calls use the v5 multiplexed transport handle through the existing synchronous workspace-client boundary.
- Each backend method opens a stream and awaits the final response. The central reader validates and accumulates partial/data events under connection and per-stream limits.
- The owned v5 request handle and each asynchronous backend bridge share a latched cancellation token. Dropping either cancels the live stream, wakes the control worker and prevents reconnect replay. Each existing token-aware filesystem method also registers a detachable callback that forwards explicit `WorkspaceCancellationToken` cancellation to the same live stream without polling.
- File tree starts `watch.start` when `watch_filesystem` is enabled and the server advertises `watch`.
- The reconnecting client preserves desired watch roots behind a stable logical watch ID. It recreates the physical watch, sends `watch.resync`, suppresses pre-resync batches and exposes one mandatory resync batch before later deltas.
- File tree treats that resync as an application barrier: every expanded directory must refresh successfully before the sequence advances, and results from an older watch epoch are discarded.
- Remote startup is one owned attempt with a shared cancellation token and monotonic deadline, five minutes by default. Home discovery, helper probes and transfer, service launch, handshake, reinstall retry and the first directory listing all consume the same remaining budget. Replacing, dismissing or dropping the owning UI cancels and reaps its active local process tree through a Unix process group or Windows Job Object; dropping the first-listing future resets its live transport request.
- Browse and selected-workspace startup share a cheap-clone bootstrap handle. Its five-minute, 32-entry caches single-flight pathless platform/cache-root probes and resolved helper paths by exact transport identity. Failed or cancelled leaders publish nothing, follower cancellation does not cancel a leader, and a startup retry invalidates only the matching helper result. The selected workspace always launches a new service rooted at the selected path.
- Existing remote polling remains as fallback and as low-frequency reconciliation when watching is active.
- UI components receive the same domain-level events they receive today: directory refreshes, file system changed, VCS changed and process output.

Next integration:

- Exercise the implemented bounded incremental consumers in real Linux SSH and Windows WSL fault/load environments.
- Measure interactive request latency while a remote peer stalls file, search or process consumption.

Why:

- Most application code should not know whether freshness came from polling or a remote watch.
- The backend abstraction remains valuable; the transport change should not leak into every feature.

## Server integration

What:

- Split the service into transport, scheduler and handlers.
- Transport owns frame encoding/decoding, stream maps, windows and pings.
- Scheduler owns priority, fairness, concurrency limits and cancellation tokens.
- Handlers own workspace operations and report progress/data through a stream sink.
- Watch service owns `notify` watchers, coalescing, overflow detection and resync signalling.

Why:

- This separates protocol correctness from filesystem/search/git implementation.
- It also gives tests clear seams: frame tests, scheduler tests, handler tests and watch tests.

## Implementation status and remaining plan

The v5 baseline is complete:

1. v5 is the sole helper, CLI `serve`, test and documentation wire protocol.
2. Multiplexing, bounded worker classes, cancellation metadata, deadlines, progress and priorities are active.
3. `fs.read`, `fs.write`, `process.run` and search use flow-controlled DATA frames.
4. `watch.start`, `watch.update`, `watch.stop`, `watch.resync` and `watch.batch` are active with polling fallback.
5. Directory generations, fingerprints, `not_modified` responses and optional deltas are active.

The 2026-07-13 hardening pass adds executable failure and memory invariants around the baseline. It includes atomic stream teardown, decoded-data bounds, receive-window enforcement, bounded closed-stream receive-credit debt, lazy producers, bounded server and watch queues, dedicated client and server writers, owned aggregate-deadline startup attempts with descendant cleanup through Unix process groups and Windows Job Objects, child-handshake and client connection-heartbeat watchdogs, bidirectional heartbeat probes, terminal transport errors, typed recovery outcomes, reconnecting logical watches with mandatory resync, end-to-end priority propagation, cooperative filesystem cancellation, callback-driven explicit remote cancellation, cancellation-aware response production, cancel-on-drop client ownership, shutdown cleanup and containment-safe bootstrap reuse. File reads, searches and process execution additionally expose bounded incremental consumers: queued chunks and decoded batches retain connection budget, polling releases their exact receive credit, terminal metadata follows the data, and dropping a live or completed-but-unconsumed stream releases its receive debt without making the connection unusable. Successful bounded commands release their Windows Job Object before closing it so OpenSSH `ControlPersist` processes remain reusable; cancellation, timeout and failure retain kill-on-close containment. Compatibility fixtures pin additive protobuf-field handling, capability intersection and minor-version skew. A recurring short-read, short-write and interrupted-I/O duplex exercises framing end to end, while a platform-independent real-helper loopback fixture verifies that metadata and file requests remain responsive during a live process stream. Linux CI also executes the SSH and WSL command fixtures.

The next integration work should:

1. Expand the command-level fixtures into real Linux SSH and Windows WSL fault/load environments, including stalled-peer memory and latency assertions. The platform-independent local-service loopback and Linux SSH/WSL command shims now run in CI.

Multiplexing remains the foundation. Future encoding or daemon work should follow measured queue, latency and recovery results rather than replace the implemented v5 state machine.

## Testing strategy

Protocol tests:

- Encode/decode every frame type.
- Reject oversized control/body frames.
- Reject invalid stream state transitions.
- Verify unknown optional protobuf fields are tolerated.
- Verify required missing capabilities fail handshake.

Concurrency tests:

- A long search stream must not block a `fs.stat` stream.
- A bulk file read with exhausted stream window must not block `PING`, `RESET_STREAM` or a small high-priority response.
- Out-of-order stream completion must route responses to the right callers.

Cancellation tests:

- Drop a live request handle after partial response DATA and verify one `CANCELLED` reset, complete budget release and continued use of another stream.
- Drop a polled workspace request future and verify its blocking worker observes cancellation and exits.
- Cancel an explicit filesystem token while its remote future remains polled and verify prompt local completion plus one transport reset.
- Drop a pending `watch.start` future and verify its ambiguous control connection closes with no raw waiter or byte reservation left behind.
- Cancel during reconnect recovery and verify no replay stream opens.
- Cancel search mid-walk and verify no more result batches arrive.
- Cancel process execution and verify its Unix process group or Windows Job Object is terminated.
- Cancel a complete staged write immediately before commit and verify the temp file is removed and destination is unchanged.
- Cancel `fs.list_dirs` while its first backend call is blocked and verify the next path never starts.
- Cancel a read during its operating-system read and after its first emitted chunk; verify no post-cancellation chunk arrives.
- Fill the producer-to-service queue, cancel response serialization and verify the producer exits without a terminal event or leaked byte reservation.
- Close the peer while filesystem work is blocked and verify its worker finishes without retaining a task-class permit.
- Stall the server writer, request shutdown and verify grace expiry cancels active work and lets the service exit before the write resumes.
- Send a zero-byte write and verify it commits through the staged atomic path without leaving a temp file.
- Verify every method receives the documented absolute and inactivity defaults.
- Verify only target-stream progress extends inactivity and never extends the absolute deadline.
- Expire a healthy read-only stream and verify one reset, one typed error and continued use of another stream.
- Expire a stream with unknown peer health and verify terminal teardown, one safe read replay within the original budget and no mutation replay.
- Verify replay preserves the exact request context and race a final response against expiry to prove exactly one completion.
- Verify process runtime timeouts, unlimited silent processes and watch-control deadlines follow their distinct rules.

Watch tests:

- Fake watch backend emits create/modify/delete/rename batches.
- Overflow emits `resync_required`.
- Client refreshes parent directories after watch invalidation.
- Unsupported watch capability falls back to remote polling.

Integration tests:

- Loopback v5 helper.
- SSH fixture for file tree watch, search cancellation, process output and reconnect.
- WSL fixture for path mapping and watcher availability.
- Helper-mismatch fixture where a stale or incompatible helper fails with an actionable v5 diagnostic.

Performance checks:

- File tree idle traffic should be zero watch traffic except heartbeat and low-frequency reconciliation.
- A remote file change in a watched expanded directory should trigger refresh after debounce plus one round trip.
- Search should deliver the first result batch before final completion.
- High-priority metadata requests should complete while background search/process streams are active.

## Open questions

- Which high-volume JSON method payloads merit a measured migration to protobuf now that control messages use protobuf exclusively?
- Should recursive project-root watch be enabled by default on platforms with efficient recursive notifications, or should expanded-directory watching remain the default everywhere?
- Should a future helper daemon share watch state, environment capture and git caches across multiple windows?
- Should local workspaces optionally use the same v5 backend for parity testing?

## References

- [RFC 9113: HTTP/2][rfc9113]
- [Language Server Protocol 3.17 specification][lsp-317]
- [gRPC cancellation guide][grpc-cancel]
- [Protocol Buffers proto3 language guide][protobuf-proto3]
- [Linux inotify manual page][inotify]
- [Watchman troubleshooting: recrawl][watchman-recrawl]
- [VS Code Remote Development using SSH][vscode-remote-ssh]

[rfc9113]: https://www.rfc-editor.org/rfc/rfc9113.html
[lsp-317]: https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/
[grpc-cancel]: https://grpc.io/docs/guides/cancellation/
[protobuf-proto3]: https://protobuf.dev/programming-guides/proto3/
[inotify]: https://man7.org/linux/man-pages/man7/inotify.7.html
[watchman-recrawl]: https://facebook.github.io/watchman/docs/troubleshooting#recrawl
[vscode-remote-ssh]: https://code.visualstudio.com/docs/remote/ssh
