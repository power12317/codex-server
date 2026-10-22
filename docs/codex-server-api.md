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
    "businessNotification": "cpa/inference/body",
    "completionSemantics": "http-body-eof",
    "diagnosticsCompleted": false,
    "responsesWebsocket": false,
    "responseIdentityMapping": true,
    "ticketSnapshots": true,
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

`ticketSnapshots: true` optionally supports exchange-scoped source snapshots on
Responses requests. Set top-level `params.captureTicketSnapshot: true` to receive
`cpa/inference/ticket`; omission or `false` disables it. This flag never enters
the model request. A snapshot can precede the start reply or arrive during the
body stream, and is delivered before the terminal notification. Its params are
`{requestId, credentialId, model, observedAt, state, route}`, where `route` contains
`__cflb` and `__oailb`, each with raw `value` and optional RFC3339 `expires_at`.
`observedAt` is observation time, not a new ticket issue time. Incomplete, deleted
or expired combinations are not emitted. See the
[source snapshot contract and acceptance record](codex-runtime-source-ticket_CN.md).

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
methods receive error code `-32601`. Required fields retain their documented
types; optional additive envelope fields do not invalidate known CPA messages.

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

| Field          | Requirements and meaning                                                                                                                         |
| -------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| RPC `id`       | Correlates this control reply. It is distinct from the inference `requestId`.                                                                    |
| `requestId`    | Nonempty JSON string. Use a fresh UUID for each inference. Active IDs must be unique within the credential worker, including across connections. |
| `credentialId` | Existing enabled relative credential filename.                                                                                                   |
| `operation`    | `responses`, `images/generations` or `images/edits`.                                                                                             |
| `sourceFormat` | Nonempty source label; `openai-response` is used here. It does not convert Chat/Claude envelopes.                                                |
| `sessionId`    | Nonempty caller isolation scope. Keep it stable across requests belonging to the same logical scope.                                             |
| `request`      | Business JSON for the selected operation. Ordinary Responses requires a model string; image operations use the contract below.                   |

For `operation: "responses"`, pass required history in `request.input`. Nonempty
`previous_response_id` and `conversation` require caller history reconstruction;
null or empty references are absence. `generate: true` is normal execution;
`generate: false` requires WebSocket prewarming, which this HTTP entry does not
implement. `background: true` requires an asynchronous retrieval lifecycle that
is also unavailable. These unsupported modes fail before model generation.
Explicit `store` booleans enter the native DTO without creating local conversations.
`stream` describes downstream delivery; the worker's Responses transport remains
SSE, which a caller can aggregate for a nonstreaming public API.

The native builder preserves explicit tool choices, parallel booleans (including
Lite), `include: []`, structured-output names/schema/strictness, verbosity,
reasoning controls and service tiers. `max_tokens` and `max_completion_tokens`
normalize to `max_output_tokens`; conflicting values fail specifically. Token,
temperature, top-p, truncation and cache-retention controls use native DTO fields.
`top_k` and `prompt_cache_options` have no native Responses mapping. Existing
HTTP business extensions use a native extension map before serialization.

Supported tools include native `function`, `custom`, `namespace`, `web_search`,
`tool_search` and hosted `image_generation`, with the detailed field policy in the
[model-only extension contract](../codex-rs/app-server/docs/cpa-bridge.md#native-request-mapping).
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
before sending `request`. The worker then performs native construction and owns
the final upstream representation. Images represented as Responses use the same
`prepare_inference_request` pipeline as ordinary Responses, including message
normalization, native defaults, model-selected Lite layout, time context and
worker identity. For example, missing message `type` becomes `message`, string
content becomes `input_text` content. Explicit business controls enter the native
builder; omitted values use native defaults. Display/transport/Lite/auth/device
metadata is consumed or rebuilt. Native output is never trimmed back to the set
of fields supplied by the caller, and there is no final serialized-body overlay.

Direct Images requests are rebuilt with `ImageGenerationRequest` or
`ImageEditRequest`. Model and prompt are required strings; edits also require
`images`. Native enums validate quality/background, image references are
serialized through `ImageReference`, and optional nulls are omitted. Standalone
mask, stream, output format/compression, partial-image, response-format,
input-fidelity, moderation and user options extend these DTOs. `stream: false`
does not silently remove explicit partial-image options. Invalid native types produce
HTTP 400. Authentication, transport headers and worker identity remain locally
managed. No URL, raw HTTP header block or caller credential is accepted as a
transport override.

Multipart editing is converted by the caller before RPC dispatch: images become
ordered `images: [{"image_url":"data:image/png;base64,..."}]` entries and mask
becomes `mask: {"image_url":"data:image/png;base64,..."}`. Repeated form fields use
JSON arrays during caller conversion; a field whose native type is scalar must
still pass native validation. The RPC carries the converted business JSON and
no multipart boundary or temporary filename. The worker preserves the supplied image order, duplicate entries and
base64 content through native JSON serialization.

Image responses, including partial-image SSE events and JSON results, use the
existing `bodyBase64` channel without identity rewriting of image contents.
`cpa/inference/completed` indicates transport EOF; it does not require a
`response.completed` event. The caller interprets image events, or Responses
terminal events when `imageApi` is `responses`. Transport failures and cancellation
produce `cpa/inference/error`, including after a partial image was transferred.
The same `requestId` correlates the RPC, worker diagnostics and upstream exchange.

Both sides need image-aware code and must negotiate the
operation before sending the new `imageApi` field. Independent
`responses/compact` remains unsupported; ordinary Responses summary prompts,
`compaction_trigger` and `compaction` items use the existing Responses path.

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
[identity contract](../codex-rs/app-server/docs/cpa-bridge.md#response-identity-projection)
for supported locations and aliases.

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

## Streaming and completion

The start reply indicates that upstream response headers have been accepted:

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

| Notification              | Parameters                                          | Client behavior                                                                       |
| ------------------------- | --------------------------------------------------- | ------------------------------------------------------------------------------------- |
| `cpa/inference/upstream`  | Diagnostic exchange, described below                | Handle independently from business-response bytes. May arrive before the start reply. |
| `cpa/inference/body`      | `{requestId, bodyBase64}`                           | Base64-decode and append bytes in order.                                              |
| `cpa/inference/completed` | `{requestId}`                                       | Transport EOF. Finalize the stream parser.                                            |
| `cpa/inference/error`     | `{requestId, httpStatus, message, body?, headers?}` | Terminal transfer failure; do not expect successful completion afterward.             |

Register notification handlers before sending the request. Different requests
can interleave. Header values are arrays of strings. Each base64 payload is a
byte chunk, not a complete UTF-8 string, JSON document, SSE line or event. Use a
streaming UTF-8 decoder and SSE parser, or write decoded bytes directly to an
output stream. Do not concatenate base64 text before decoding.

Unchanged SSE events preserve their bytes; identity-bearing events may have
their JSON reserialized. IPC body chunks are at most 64 KiB after decoding without
limiting the complete response. The local WebSocket and pending SSE-event/JSON-response
input buffers have no size limit. Heartbeat/comment lines before event data are forwarded promptly.

`cpa/inference/completed` means HTTP body EOF, not model-level success. Inspect
Responses events such as `response.completed` or `response.failed` yourself.
Each inference is sent once, including upstream 401/429/5xx failures. Normal
managed credential acquisition before dispatch remains available; retrying a
failed inference belongs to the caller.

## Errors and cancellation

A failure before the start result is returned as the original RPC's error:

```json
{
  "id": 3,
  "error": {
    "code": -32000,
    "message": "generate=false requires WebSocket prewarming, which the HTTP entry does not implement; no model request was sent",
    "data": {
      "httpStatus": 400,
      "body": "{\"error\":{\"type\":\"request_adaptation_error\",\"message\":\"...\"}}"
    }
  }
}
```

Structured upstream failures may include `data.body` and `data.headers`. A failure
after acceptance uses `cpa/inference/error` instead. Optional fields may be null.
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

- `request`: `requestId`, `url`, `method`, `headers`, `body`, optional
  `accessTokenSha256` and `oaiLbNode`.
- `response`: `requestId`, `statusCode`, `headers`, optional `body` and `oaiLbNode`.
- `error`: `requestId` and `message`.

`oaiLbNode` describes this HTTP exchange's gateway node. Request notifications
use the actual outbound `Cookie` header. Response notifications use the response's
`__oailb` cookie when present, including an empty result for deletion or an invalid
value; otherwise they retain the node from that exact request. Other response
cookies do not clear it. The same rule applies to successful and HTTP error
responses. Network errors without an HTTP response leave the request notification
as the available observation. No additional node cache or cookie-jar reread is
used to infer the response node.

These describe native upstream exchanges, including rejected authentication
attempts. Diagnostic identities remain native; source identity restoration
applies to the business response. IPC headers preserve their actual values,
including Authorization, Cookie, Set-Cookie and turn-state. Start and error replies
also retain all upstream response headers. `accessTokenSha256` remains an additive
diagnostic field; it does not replace the original Authorization header. Request
bodies can contain full prompts and history. Diagnostics are not another
copy of the body stream and do not replace completion/error notifications.

stderr `cpa.worker.started` and `cpa.request.*` events identify credential workers
and request mapping. Correlate using the inference `requestId` and the actual
upstream `x-request-id`; no CPA-specific external log ID is required.
Ordinary logs use a separate redacted header copy and ticket lengths. Raw ticket
notifications are never serialized into stderr records; IPC values are unchanged.

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
are not new server environment variables. The example writes decoded response
bytes to stdout, ignores diagnostic notifications, and sends cancellation on
Ctrl+C. It demonstrates one request at a time; a production client needs a
dispatcher for concurrent requests and its own Responses event/tool handling.

The current schemas live in
[`cpa.rs`](../codex-rs/app-server-protocol/src/protocol/v2/cpa.rs). The enclosing
`v2` source directory is the official app-server type layout. It does not
introduce a versioned service URL or a private protocol-number gate.
