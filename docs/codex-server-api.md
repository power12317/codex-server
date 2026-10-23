# Codex Server API and third-party integration

Codex Server is a long-running inference service. A master process accepts client
connections and supervises one native worker per enabled credential. Any client
can implement this protocol; CPA is one existing integration, not a dependency.

This guide describes model-only `cpa/*` fork extensions carried by the official
app-server message envelope. The connection uses official `initialize` /
`initialized` shapes. Inference, credential, OAuth, diagnostics and source-ticket
methods are extensions; they do not start an Agent turn or execute client tools.
The existing `CODEX_CPA_*` environment names are retained.

## Transport and startup

Follow the [installation guide](install.md) or
[container deployment guide](../deploy/cpa-runtime/README.md) first.

| Endpoint                            | Behavior                                                                                         |
| ----------------------------------- | ------------------------------------------------------------------------------------------------ |
| `GET http://127.0.0.1:38317/readyz` | Returns `ok` once the listener is serving. It does not verify credentials or model availability. |
| `ws://127.0.0.1:38317/`             | One JSON object per WebSocket text message.                                                      |

`CODEX_CPA_PORT` changes the port. The master binds to loopback; there is no
listen-address setting. Clients on other machines use an SSH tunnel or an
authenticated WebSocket reverse proxy. There is no built-in client API key or
TLS layer. The proxy must be able to reach the same loopback/network namespace.

The service does not expose HTTP `POST /v1/responses`, a credential-upload API,
`model/list`, `thread/start`, or `turn/start`. Model access comes from the selected
credential and configured native provider; choose a model available to that
account. Supported service methods are listed below.

## Connection lifecycle

Messages use JSON-RPC-style `id`, `method`, `params`, `result` and `error` fields.
The implementation omits `jsonrpc: "2.0"` from replies. Use unique string or
integer RPC IDs and match replies by `id`; notifications have `method` and no
reply ID. Do not assume the next received message is the reply to the last call.

Initialize each new WebSocket connection and wait for its result:

```json
{
  "id": 1,
  "method": "initialize",
  "params": {
    "clientInfo": { "name": "example-app", "version": "1.0.0" },
    "capabilities": { "experimentalApi": true }
  }
}
```

The result includes `userAgent`, `codexHome`, `platformFamily` and `platformOs`.
Then send the notification and capability request:

```json
{ "method": "initialized" }
```

```json
{ "id": 2, "method": "cpa/capabilities/read", "params": {} }
```

These two objects are separate WebSocket messages. A capability reply has this
shape (version strings below are illustrative):

```json
{
  "id": 2,
  "result": {
    "runtimeVersion": "...",
    "upstreamRevision": "...",
    "manualOAuth": true,
    "upstreamLogs": true,
    "upstreamBodyLogs": true,
    "executionMode": "inference-only",
    "rawBody": true,
    "businessNotification": "cpa/inference/event",
    "completionSemantics": "native-terminal",
    "diagnosticsCompleted": true,
    "responsesWebsocket": true,
    "responseIdentityMapping": true,
    "ticketSnapshots": false,
    "operations": ["responses", "images/generations", "images/edits"]
  }
}
```

Require the behaviors your client uses, including `executionMode: "inference-only"`,
`rawBody` and the requested operation. Choose the byte channel from
`businessNotification`; decode `bodyBase64` once. Do not require every capability
to match one fixed template. Ignore unknown additive fields. `runtimeVersion` and
`upstreamRevision` are informational product/source metadata, not compatibility gates.
Use the image revision label for the exact fork commit.

The existing main and native implementations differ as follows:

| Capability             | main                 | native                |
| ---------------------- | -------------------- | --------------------- |
| `businessNotification` | `cpa/inference/body` | `cpa/inference/event` |
| `completionSemantics`  | `http-body-eof`      | `native-terminal`     |
| `diagnosticsCompleted` | `false`              | `true`                |
| `responsesWebsocket`   | `false`              | `true`                |
| `ticketSnapshots`      | `true`               | `false`               |

`http-body-eof` means normal HTTP body EOF; the client must parse Responses or
Images output to determine model success. `native-terminal` means the native
parser accepted model/image success. Both use the payload-free
`cpa/inference/completed {requestId}` notification. When `diagnosticsCompleted`
is true, keep the independent diagnostic route until
`cpa/inference/diagnostics/completed`, whether it arrives before or after business
completion/error. When false, do not wait for that notification.

`responsesWebsocket` describes implementation support, not a guarantee for every
provider, model or configuration. Unsupported per-request transport requirements,
including prewarming on an HTTP-only route, fail specifically. Missing optional
`ticketSnapshots` means false; require support before setting
`captureTicketSnapshot: true`. Native rejects required snapshots before model
execution rather than claiming ticket success. `responseIdentityMapping` describes
selective restoration of caller identities.

## Methods

| Method                    | Parameters                                | Result                                                        |
| ------------------------- | ----------------------------------------- | ------------------------------------------------------------- |
| `initialize`              | Client information as above               | Connection metadata.                                          |
| `cpa/capabilities/read`   | `{}`                                      | Capability object.                                            |
| `cpa/credential/reload`   | `{}` or `{"credentialId":"account.json"}` | Capability object after worker reconciliation.                |
| `cpa/auth/login/start`    | `{"credentialId":"account.json"}`         | `{loginId, authUrl, state}`.                                  |
| `cpa/auth/login/callback` | `{loginId, redirectUrl}`                  | `{status, error}`.                                            |
| `cpa/auth/login/status`   | `{loginId}`                               | `{status, error}`.                                            |
| `cpa/inference/start`     | Inference envelope below                  | `{requestId, statusCode, headers}` before body notifications. |
| `cpa/inference/cancel`    | `{requestId}`                             | `{}` acknowledgment.                                          |

`initialized` is a notification. Other methods above expect an RPC ID. Unsupported
methods receive error code `-32601`. Use documented parameter fields; typed
parameter objects retain required field checks and tolerate optional additions.

## Credentials and OAuth

The operator or integrating application provisions JSON files in
`CODEX_CPA_AUTH_DIR` through the filesystem. For example, `account.json` can start
as this tokenless placeholder:

```json
{ "type": "codex", "codex_cli": { "enabled": true }, "disabled": false }
```

Use private file permissions and an owner the service can read and write. Both
the file and directory must allow the worker's atomic token updates. The master
recursively scans `.json` files every 500 ms. A credential is enabled when its
`type` is `codex`, `codex_cli.enabled` is `true`, and `disabled` is not `true`.
Its `credentialId` is the relative filename, such as `team/account.json`;
case, extensions and nested directories are significant on Linux.

Token saves participate in CPA's cross-process credential lock: the canonical
credential path plus `.lock`, retained as a stable sidecar. The worker holds an
exclusive OS file lock while reading the latest JSON, checking enabled state and
atomically replacing normal token fields. Nested `siwc`, `use_siwc`, control flags
and unknown metadata are preserved. Other writers must use the same lock; a
disabled or removed credential is not silently recreated by a pending token save.

1. Create the enabled placeholder on the server.
2. Call `cpa/credential/reload` with that credential ID to reconcile immediately.
3. Call `cpa/auth/login/start` with the same ID.
4. Open the returned `authUrl` in a browser and complete account authorization.
5. Send the complete final redirect URL, including `code` and `state`, to
   `cpa/auth/login/callback` with the returned `loginId`.
6. Require `status: "completed"` before inference. Status can also be `pending`
   or `error`; `error` is a nullable string.

The manual flow does not require an additional callback listener in this
service. A browser may fail to load the final loopback redirect; submit that
complete URL to the callback RPC. Treat it as sensitive authorization material.
The worker validates OAuth state and writes the tokens into the original file.
Starting another login for the same credential supersedes its previous pending
login. Login state is not preserved across worker/master restarts.

For an existing managed credential, the flat fields are `id_token`,
`access_token`, `refresh_token`, `account_id` and optional `last_refresh`, in
addition to the control fields above. A native CLI `auth.json` with nested
`tokens` is not the same format. OAuth populates the managed format; no mirrored
private `auth.json` is created. Reauthorization uses the existing filename.

To disable an account, set `disabled: true`, set `codex_cli.enabled: false`, or
remove the file, then optionally call reload. The master stops its worker and
in-flight requests; private state remains. The application owns account choice
and scheduling. There is no automatic credential selection in inference requests.

## Inference request

Example for an already authorized `account.json`; replace the illustrative model
name with one available to the account:

```json
{
  "id": 3,
  "method": "cpa/inference/start",
  "params": {
    "requestId": "c6ed28d6-3cee-4dd2-ab75-179f41ba9d21",
    "credentialId": "account.json",
    "operation": "responses",
    "sourceFormat": "openai-response",
    "sessionId": "example-app:conversation-42",
    "request": {
      "model": "MODEL_AVAILABLE_TO_YOUR_ACCOUNT",
      "stream": true,
      "store": false,
      "input": "Reply with one short sentence.",
      "session_id": "conversation-42",
      "thread_id": "conversation-42",
      "turn_id": "turn-1",
      "prompt_cache_key": "conversation-42"
    }
  }
}
```

| Field          | Requirements and meaning                                                                                                                                                                        |
| -------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| RPC `id`       | Correlates this control reply. It is distinct from the inference `requestId`.                                                                                                                   |
| `requestId`    | Nonempty JSON string. Active IDs must be unique on one connection until both business and diagnostics end. A new connection may reuse an ID while an older connection still drains diagnostics. |
| `credentialId` | Existing enabled relative credential filename.                                                                                                                                                  |
| `operation`    | `responses`, `images/generations` or `images/edits`.                                                                                                                                            |
| `sourceFormat` | Nonempty source label; `openai-response` is used here. It does not convert Chat/Claude envelopes.                                                                                               |
| `sessionId`    | Nonempty caller isolation scope. Keep it stable across requests belonging to the same logical scope.                                                                                            |
| `request`      | Business JSON for the selected operation. Ordinary Responses requires a model string; `stream` is a downstream delivery preference. Image operations use the contract below.                    |

For `operation: "responses"`, pass required history in `request.input`. Non-null
`previous_response_id` and `conversation` require history reconstruction by the
caller; null references are treated as absent. `generate: false` uses native WS
prewarming and fails specifically on an HTTP-only worker without generating.
`store` enters the native request without creating a local conversation;
`background: true` needs an asynchronous retrieval lifecycle not provided here.
Explicit `tool_choice`, `parallel_tool_calls`, `include` (including `[]`) and
text/structured-output controls are preserved by the native builder. Client
transport/timing mirrors and display metadata are consumed or regenerated.
Unknown business controls, including `top_k` and `prompt_cache_options`, are
reported as specific compatibility gaps.

Supported tools include native `function`, `custom`, `namespace`, `web_search`,
`tool_search` and hosted `image_generation`, with the detailed field policy in the
[native input contract](../codex-rs/app-server/docs/cpa-native-model-runtime.md#supported-input).
Client-side function/custom tools execute in the calling application. Preserve
their call IDs and send results plus required history in subsequent requests.
There is no local tool loop. Native web search and image generation run at the
upstream service. Tool search retains its declared client/server execution mode;
the worker does not execute client tools.

### Standalone image generation and editing

Before starting an image request, require its exact operation in
`capabilities.operations`: `images/generations` or `images/edits`. A server that
advertises only `responses` cannot execute these operations; return an explicit
unsupported-operation error instead of sending the request directly upstream.

Image operations also require `imageApi: "images"` or `imageApi: "responses"`
beside `request` in the RPC envelope. This selects the native business request
representation. It is not valid on the ordinary `responses` operation.

- `images` uses native image fields and the corresponding `/images/generations`
  or `/images/edits` endpoint.
- `responses` uses native Responses input and tools, including the hosted
  `image_generation` tool, and the `/responses` endpoint. The calling application
  continues to convert those events into its image API response.

The caller completes protocol conversion, model mapping and its payload rules
before sending `request`. The worker maps supported business fields to native
model inputs and delegates a single execution and completion to the native
model client. Unsupported fields fail explicitly. There is no final wire overlay
or alternate sender.

Standalone Images uses the existing native `CodexImagesBackend`. Generation
accepts `model`, `prompt`, `background`, `n`, `quality` and `size`; edits also
require `images` and accept `mask` references. Supplied `stream`, `partial_images`,
`output_format` and `output_compression` enter the native DTO without being erased.
Other native options include `response_format`, `input_fidelity`, `moderation` and
`user`. Omitted options, including `n`, stay absent. Unknown business options and
malformed types receive specific compatibility errors.

Multipart editing is converted by the caller into native image references,
for example `images: [{"image_url":"data:image/png;base64,..."}]`. The worker
preserves the supplied image order, duplicate entries and base64 content.

Responses delivers accepted SSE events incrementally through
`cpa/inference/event`, including supported hosted image events. Standalone Images
uses the native Images backend: validated JSON is one business event; native image
SSE partials stream immediately and require an image completed event for success.
Both end with payload-free `cpa/inference/completed` after native success.
Failures before start use the RPC error; later native failure/cancellation uses
`cpa/inference/error`. Attempts and client-tool handoff follow the rules below.

Independent `responses/compact` remains unsupported. Ordinary Responses summary
prompts and native compaction history use the same native execution entry.

### Session, thread and turn identities

Within a credential and outer `sessionId` scope, source `session_id` identifies
the logical root. A different `thread_id` identifies a separate child/thread;
`parent_thread_id` uses the same mapping. Root and children share root cache
affinity while keeping separate routing contexts. Explicit subagent metadata is
supported; unequal IDs alone do not imply a subagent.

An explicit source `turn_id` is retained. If absent, the service synthesizes one
for a fixed one-hour window within the same root/thread/provenance. Explicit-turn
contexts expire after one hour without access. Contexts hold identity and routing
state only, are limited to 1024 per worker, and reset on worker restart. No local
conversation or history is created. Use explicit turn IDs when the client knows
the real turn boundary.

Title requests marked with `thread_source: "thread_title"` or
`request_kind: "thread_title"` / `"title_generation"` use disposable contexts.
They do not reuse conversation turn state or trigger another title request.

Known response identity fields matching the request's mapped native IDs are
restored to source values. A matching generated field is removed when that source
field was absent. Model text, `response.id`, item IDs, `call_id`, tool payloads,
business `metadata` and opaque `turn_state` are not rewritten. See the
[native contract](../codex-rs/app-server/docs/cpa-native-model-runtime.md)
for the execution and identity boundary.

### Runtime date and timezone

The adapter replaces existing date/timezone fields in the latest standalone
`environment_context` and `codex_apps_client_time_context` declarations in input
messages. These declarations remain current across assistant/tool continuations;
they do not need to be repeated after each tool result. Older declarations and
other message/tool content remain intact. No context message or missing time
field is added. Supplied flat date/timezone metadata is updated consistently.

Values come from the worker's effective system clock and timezone. Use the
[host timezone mount](../deploy/cpa-runtime/README.md#runtime-timezone) to follow
the deployment host; a container does not inherit unmounted host files merely
because its image was rebuilt.

## Completion delivery

The start reply is sent when accepted native business output is ready. It does not wait for completion
or diagnostic drain. It contains the response status and public headers:

```json
{
  "id": 3,
  "result": {
    "requestId": "c6ed28d6-3cee-4dd2-ab75-179f41ba9d21",
    "statusCode": 200,
    "headers": { "content-type": ["text/event-stream"] }
  }
}
```

The same connection receives notifications keyed by `params.requestId`:

| Notification                          | Parameters                                                        | Client behavior                                             |
| ------------------------------------- | ----------------------------------------------------------------- | ----------------------------------------------------------- |
| `cpa/inference/upstream`              | Diagnostic exchange, described below                              | Keep logs separate from business output; may precede start. |
| `cpa/inference/event`                 | `{requestId, bodyBase64}`                                         | Decode incremental business bytes once.                     |
| `cpa/inference/completed`             | `{requestId}`                                                     | Native success, without a body replay.                      |
| `cpa/inference/error`                 | `{requestId, httpStatus, message, body?, headers?, nativeError?}` | Final native error/cancellation or worker loss after start. |
| `cpa/inference/diagnostics/completed` | `{requestId}`                                                     | End of this request's diagnostic FIFO.                      |

The master uses its existing RPC sequence for private worker correlation and
restores the caller's request ID in replies, events, errors and diagnostics. A
retry on a new connection cannot collide with or receive the preceding call's
diagnostic tail. Cancellation is restricted to the owning connection.
The stderr `cpa.master.request` record retains the source ID in `rpcRequestId`
and the private worker ID in `details.workerRequestId`, so Docker logs remain
traceable from a CPA request without adding a public protocol field.

Register handlers before sending the request. Different requests can interleave.
Header values are arrays of strings. Start replies as soon as the first accepted
business event is available, before native completion. Responses bytes are SSE;
standalone Images bytes use the actual native JSON or image SSE representation.
Decode every business frame once.
The completion notification never contains `bodyBase64`.

Accepted text, tool-argument deltas, argument `.done` and client-tool
`response.output_item.done` stream immediately in their observed order. Their
payloads and sequence numbers are preserved. Only terminal response events wait
for native success; there is no special tool-completion gate. The server never
executes caller tools or fabricates their results.

Each invocation makes one native model attempt. Worker request/stream retries
are zero; HTTP retries, 401 recovery/resend, stream recovery and WebSocket failure
fallback do not issue another model call. Native construction, authentication
before sending, compression, transport choice and normal redirects are preserved.
Ordinary CLI entry points retain their existing behavior.

There is no attempt notification or attempt ID. Errors immediately return to CPA
for its existing error/retry rules; the worker supplies neither a replacement
request nor an additional retry policy. Once content has reached a downstream
client, that stream must end with the error instead of concatenating another
response. An explicit new client request is a separate invocation.

Native completion is authoritative: HTTP 200, partial output and EOF alone do
not establish success. The independent diagnostic FIFO can finish after business
completion/error. Keep the IPC reader alive for its tail marker without holding
back downstream business events or adding a diagnostic timeout/capacity limit.

## Errors and cancellation

A failure before the start result is returned as the original RPC's error:

```json
{
  "id": 3,
  "error": {
    "code": -32000,
    "message": "model is required",
    "data": { "httpStatus": 400 }
  }
}
```

Native final failures include the original `data.body`, `data.headers` and HTTP
status, and may include `data.nativeError`. Rejected SSE/WS payloads preserve their
original error JSON and supplied headers. The worker returns those errors without
waiting for EOF or attempting recovery. After start, the same native failure information is carried
by `cpa/inference/error`. Worker loss also uses that notification for unfinished
business and closes the diagnostic lifetime. It does not reverse a completed
native outcome. Optional fields may be null. A client cancellation during
post-completion delivery is a delivery cancellation, not a model execution error.
Representative `httpStatus` values include 400 for invalid requests, 401 for
managed authentication, 403 for credential restrictions, 429 for context capacity,
499 for cancellation and 502/504 for transport/authentication failures.
Master dispatch failures, including a missing/disabled credential or duplicate
active request ID, currently report 503; do not infer HTTP status solely from a
message string. Worker validation may report duplicate IDs as 409.

Cancel on the connection that owns the active inference, using a fresh RPC ID:

```json
{
  "id": 4,
  "method": "cpa/inference/cancel",
  "params": { "requestId": "c6ed28d6-3cee-4dd2-ab75-179f41ba9d21" }
}
```

The `{}` result acknowledges the signal. The original request terminates
separately; if it already finished, cancellation is a no-op. Closing the client
connection cancels its outstanding inference. Reconnect and initialize again for
new requests; there is no stream-resume or exactly-once replay facility. Work
already sent to the upstream service cannot be recalled.

## Diagnostics

`cpa/inference/upstream` has a `kind` discriminator:

- `request`: `requestId`, `transport`, `url`, `method`, `headers`, `body`, encoded
  `bodyBase64`, optional `accessTokenSha256` and `oaiLbNode`.
- `response`: `requestId`, `transport`, `statusCode`, `headers`, optional `body` and `oaiLbNode`.
- `body`: `requestId`, `transport`, `bodyBase64` from an actual native attempt.
- `error`: `requestId`, `transport` and `message`.

The worker and master keep business and diagnostic backlogs separate, with
business priority at the master. A blocked diagnostic callback/acknowledgement
does not gate business. The physical stdio/WebSocket connection is still shared;
a write already in progress cannot be preempted. `diagnostics/completed` is
queued behind all preceding records in the same diagnostic FIFO.

`transport` is the actual `http` or `websocket` transport. WebSocket handshake
records and response.create frame records are separate observations.

For HTTP, `oaiLbNode` describes this exchange's gateway node. Request notifications
use the actual outbound `Cookie` header. Response notifications use the response's
`__oailb` cookie when present, including an empty result for deletion or an invalid
value; otherwise they retain the node from that exact request. Other response
cookies do not clear it. The same rule applies to successful and HTTP error
responses. Network errors without an HTTP response leave the request notification
as the available observation. No additional node cache or cookie-jar reread is
used to infer the response node.

For WebSockets, the actual handshake determines a connection-scoped diagnostic
node using the same response-cookie priority and request-cookie fallback. Request
headers are observed after the native cookie store supplies them. Every
`response.create` request notification, including the first and reused requests,
carries that connection's `oaiLbNode`. A new connection derives its own node;
an absent, deleted or invalid cookie cannot inherit a previous connection's node.
Only the sanitized node is retained, not the handshake observer or raw cookie.
Reused requests do not emit a synthetic HTTP response or add cookie headers to
WebSocket frames. This diagnostic state does not change routing, retries or
business-event delivery, and uses the existing model-only extension fields.

These describe native upstream exchanges, including rejected authentication
attempts. Diagnostic identities remain native; source identity restoration
applies to the business response. Authorization/cookie headers are redacted, but
request bodies can contain full prompts and history. Per-attempt body bytes are
diagnostic data and do not replace the final completion/error result. Do not log
the final business body again when upstream body logging is enabled.

stderr `cpa.worker.started` and `cpa.request.*` events identify credential workers
and request mapping. Correlate using the inference `requestId` and the actual
upstream `x-request-id`; no CPA-specific external log ID is required.

## Runnable client example

The [Node.js example](../examples/codex-server-client.mjs) uses the built-in
WebSocket API in Node.js 22 or later and needs no npm dependencies.

Check initialization and capabilities without a credential:

```sh
node examples/codex-server-client.mjs --capabilities
```

After provisioning and authorizing a credential:

```sh
CODEX_SERVER_CREDENTIAL=account.json \
CODEX_SERVER_MODEL=MODEL_AVAILABLE_TO_YOUR_ACCOUNT \
CODEX_SERVER_SESSION=example-app:conversation-42 \
  node examples/codex-server-client.mjs "Reply with one short sentence."
```

`CODEX_SERVER_URL` overrides the default WebSocket URL, for example when using a
reverse proxy. These `CODEX_SERVER_*` names configure the example client; they
are not new server environment variables. The example writes JSONL records for
business events and their final outcome. Event records contain decoded UTF-8
`body`. It drains diagnostics without printing their body, cancels on Ctrl+C,
and never automatically retries a failed request. Production clients need a
concurrent dispatcher and must apply their own existing error/retry rules without
joining a new response to an already visible partial stream.

The current schemas live in
[`cpa.rs`](../codex-rs/app-server-protocol/src/protocol/v2/cpa.rs). The enclosing
`v2` source directory is the official app-server type layout. It does not
introduce a versioned service URL or a private protocol-number gate.
