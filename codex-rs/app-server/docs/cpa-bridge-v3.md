# Codex Server inference protocol v3

Contract identifier: **V3-SEP-20260927**. This is the inference-only contract for
Codex Server clients, including CPA. The historical `cpa/*` methods, endpoint path
and environment variables remain wire-compatible; they do not require CPA.

Start with the [third-party API reference](../../../docs/codex-server-api.md) for
connection setup, complete request examples, credentials, errors and cancellation.
See the [deployment guide](../../../deploy/cpa-runtime/README.md) for images.

The `upstreamRevision` capability currently reports the adapter baseline
`afb436df8b70bb5bc57b86d9a3e829968988cd21`. It is not the current fork revision or
a complete record of subsequent upstream merges. Use the image's
`org.opencontainers.image.revision` label and Git history for the deployed source.

## Identity and state

`credentialId` is the case-sensitive path relative to the credential directory
on Linux. CPA uses its existing file-backed `Auth.ID` for this value. There is no separate worker ID,
account ID, credential filename setting, or owner field in the bridge contract.
The master uses that same ID for its per-credential process map and private state
directory: `CODEX_HOME/<credentialId>/`. Original names, extensions, spaces and
nested directories are preserved. For example, with the default state root,
`team/account.json` uses `/var/lib/codex/team/account.json/`. When that readable
directory is absent, an existing legacy hash directory is moved there before the
worker starts. If both exist, the readable directory takes precedence and the
legacy directory is left untouched.

The shared file has `codex_cli: {enabled: true|false}`. Only a Codex file with
`enabled: true` and without top-level `disabled: true` runs an account process.
Original file names and locations remain unchanged. A worker has a private,
persistent CODEX_HOME and reads/writes the shared original credential file.

## Connection and control

The default endpoint is `ws://127.0.0.1:38317/cpa/v1/ws`. No bridge key or
Authorization header is required. `initialize` / `initialized` remain.

The local bridge imposes no WebSocket frame or message size limit, including
complete inference requests and full upstream diagnostic bodies.

`cpa/capabilities/read {}` and `cpa/credential/reload {credentialId?: string}`
return master capabilities:

```json
{
  "protocolVersion": 3,
  "runtimeVersion": "...",
  "upstreamRevision": "...",
  "executionMode": "inference-only",
  "operations": ["responses", "images/generations", "images/edits"],
  "rawBody": true,
  "responseIdentityMapping": true,
  "manualOAuth": true,
  "upstreamLogs": true,
  "upstreamBodyLogs": true
}
```

Reload scans one credential or the directory and waits for the corresponding
processes to start/stop. It does not change credential control fields. The integrating application owns credential
selection and effective flags; the master only applies them.

## Inference

`cpa/inference/start` takes
`{requestId, credentialId, operation, sourceFormat, sessionId, imageApi?, captureTicketSnapshot?, request}` and returns
`{requestId, statusCode, headers}`. There is no `accountId` parameter. The worker
maps the semantic request into the native Codex request builder, sends once
through the native auth/provider transport, and does not start an agent/tool loop.

Ordered notifications:

- `cpa/inference/upstream`: existing `request`, `response`, and `error` log
  notifications with `requestId`, actual URL/method/headers/body/status/message.
  Header values are string arrays. Actual token SHA256 and gateway node remain
  optional logging metadata. Request `oaiLbNode` is extracted from the actual
  outbound Cookie header. Response `oaiLbNode` uses a response `__oailb` when
  present (an empty/invalid value remains empty), otherwise the same request's
  node, for both success and HTTP errors. Other Set-Cookie values do not clear it.
  This is an exchange-local observation, not an additional routing-state cache.
- `cpa/inference/ticket`: opt-in source material from one actual exchange, enabled
  only by `captureTicketSnapshot: true` when capabilities include `ticketSnapshots`.
  It carries `{requestId, credentialId, model, observedAt, state, route}` and is
  delivered before the terminal notification, including before acceptance when
  complete response headers suffice. See the [snapshot contract](../../../docs/codex-runtime-source-ticket_CN.md).
- **`cpa/inference/body {requestId, bodyBase64}`**: HTTP response bytes after the
  selective identity projection described below.
  This is the only business-response body channel. Do not emit an additional
  `upstream kind:"body"` or parsed `cpa/inference/event` copy.
- `cpa/inference/completed {requestId}`: normal HTTP body EOF.
- `cpa/inference/error {requestId, httpStatus, message, body?, headers?}`: a failed
  transfer; do not follow it with a successful completion.

Send the start result before pulling/transferring body bytes. Request/response
log entries may precede acceptance. CPA receives the business-response byte stream
and applies its normal SSE/response processing. The bridge examines only supported
identity paths; function-call contents remain opaque.
Concurrent requests for one credential must remain supported.

Source `session_id`, `thread_id`, `turn_id`, `prompt_cache_key`, `parent_turn_id`
and `root_turn_id`, plus `parent_thread_id`, `thread_source`, `subagent_kind`,
`x-openai-subagent`, `agent_name`, `window_id` and `request_kind`, are read from
top-level request fields, then flat
`client_metadata`, then its JSON-encoded `x-codex-turn-metadata` (in that order).
`x-codex-parent-thread-id` and `x-codex-window-id` are accepted aliases. Identity
values are limited to 4096 bytes each and cannot contain control characters.

The credential and outer client `sessionId` form an isolation namespace. Within it,
the source session identifies the root (falling back to the source thread, then
the RPC scope if both are absent). A deterministic UUID identifies that native
session. A missing thread or a thread equal to the source session uses the root
UUID; a distinct thread maps to its own UUID within that root. Parent references
use the same mapping, including grandchildren. `session-id` carries the root and
`thread-id` carries the current thread. The body cache key uses the root UUID;
the source cache key does not independently identify a conversation. Explicit
subagent metadata is preserved/rebuilt, including the native `collab_spawn`
compatibility header for `subagent_kind: "thread_spawn"`. Unequal session/thread
IDs alone do not classify a request as a subagent. Source turn and root/parent
turn IDs survive. Authentication and installation identities remain worker-owned.

This is logical mapping only: no ThreadManager thread, rollout, conversation
history or local tool/agent loop is created. The process model stays one worker
per enabled credential, with multiple isolated session/thread contexts inside it;
there is no operating-system process per source session. Different credentials
or outer CPA scopes do not automatically share a session or routing state.

When a source turn ID is missing, the worker generates a UUIDv7 on the first
request and reuses it and its native routing context for exactly one hour from
creation, within the same root, thread, provenance and RPC scope. Requests do not
extend that window. After expiry, new requests receive a new turn ID and empty routing state;
in-flight requests retain their old context. Explicit source turn IDs are separate
from synthetic turns and their contexts expire after one hour without access.
This one-hour fallback groups requests for compatibility; it does not infer real
user-turn boundaries. Parent/source/subagent provenance and parent/root turn IDs
are part of the context key, preventing stale ancestry from being reused. Contexts
retain identities and native routing state, not message history, and reset on
worker restart. Requests must supply their own history.

Title inference is recognized only by `thread_source: "thread_title"` or
`request_kind: "thread_title"` / `"title_generation"`. These calls associate with
the same logical IDs but get disposable contexts outside the turn cache, with no
previous turn state. Missing turn IDs are fresh for each title call. They neither
create another local conversation nor trigger another title-generation request.
Prompt wording alone is not a title marker.

The cache holds at most 1024 contexts per worker. Expired entries are removed on
access. At capacity, new contexts receive HTTP 429; existing unexpired contexts
are never evicted to silently discard their routing state. Same-key creation is
atomic under the worker's short-lived mutex. The first valid upstream
`x-codex-turn-state` from HTTP headers or native `response.metadata` is retained.
No function call is executed.

## Standalone image operations

Capabilities additionally advertise `images/generations` and `images/edits`.
These operations require the optional envelope field `imageApi`, with exactly
`images` or `responses`; ordinary `responses` rejects this field. Older v3 workers
must be treated as unsupported when the required operation is absent. There is
no direct-upstream fallback and no independent `responses/compact` operation.

`imageApi: "images"` parses typed image input and rebuilds it with the existing
`ImageGenerationRequest` / `ImageEditRequest` DTOs before calling `ImagesClient`
with the worker's provider, authentication and network client. Model and prompt
are required; edits require images. Native quality/background enums and image
references determine the serialized fields, and optional nulls are omitted.
Standalone mask, streaming and output options supplement the native DTOs.
The operation chooses the fixed generation/edit endpoint.

`imageApi: "responses"` uses the existing `prepare_inference_request` pipeline
without an image-specific override. Worker normalization owns the final request:
it restores message types, expands shorthand text, applies native defaults,
normalizes historical IDs, rebuilds identity/time metadata and selects the
model's ordinary or Lite layout. The caller's field presence never trims or
overwrites the native result. CPA applies its payload rules before dispatch;
those rules do not bypass subsequent worker normalization.

CPA converts multipart image edits to JSON before its payload rules: ordered
`images` entries and `mask` carry data URLs, and repeated fields carry arrays.
The worker does not read caller paths or accept raw multipart/HTTP requests.
Image order, duplicate images and binary content are preserved by native image
references. Scalar native fields reject arrays instead of blindly forwarding
invalid multipart values. Ordinary Responses construction remains unchanged.

Both image representations return original JSON/SSE bytes through `bodyBase64`,
with EOF reported by `completed`. Image events do not need a Responses terminal
event; the caller applies the parser appropriate to `imageApi`. Cancellation
and transport errors fail the stream even after partial output. Auth recovery,
request IDs, upstream diagnostics and credential-worker routing use the existing
bridge lifecycle. No local image execution or agent loop is introduced.

## Response identity projection

`rawBody: true` identifies the existing `bodyBase64` byte-stream transport, not a
promise that every upstream byte is unchanged. The additive
`responseIdentityMapping: true` capability announces selective inverse mapping;
protocol version 3 and `inference-only` mode remain unchanged.

Each request has its own source/native mapping. Known session/thread/parent-thread,
cache, window and turn/root-turn/parent-turn fields are restored only when their
value exactly matches that request's mapped native value. Supported locations are
the root envelope, nested `response` / `error` envelopes, their `client_metadata`
and `headers`, and encoded `x-codex-turn-metadata`. Snake/camel spellings and the
native identity header aliases are recognized. If the corresponding source field
was absent, a matching generated field is removed instead of exposing a synthetic
source identity. Server-owned values that do not match the mapping remain intact.

Successful response headers retain all actual values, including Cookie,
Set-Cookie, Authorization and turn-state, with the existing selective identity
projection. Structured HTTP error bodies
and headers receive the same projection; stale body validators are removed when
an error body changes. The separate `cpa/inference/upstream` diagnostic channel
retains actual native request/response identities for troubleshooting.

This is not global string replacement: `response.id`, output/item IDs, `call_id`,
tool arguments/results, model text, generic business `metadata`, `x-request-id`
and opaque `turn_state` are untouched. Only what the client actually passed to Codex Server can
be restored; values already replaced inside a client cannot be recovered here.

SSE frames are buffered until their data can be examined. Unchanged events retain
their exact bytes, while modified JSON data is reserialized with non-data lines
preserved. Leading heartbeat/comment/event/id lines are forwarded immediately.
JSON HTTP responses are projected at EOF. Pending SSE events and JSON responses
have no size limit. Output IPC chunks are at most 64 KiB without limiting the
complete response. These transient buffers are not persisted conversation storage.

## OAuth

- `cpa/auth/login/start {credentialId}` -> `{loginId, authUrl, state}`.
- `cpa/auth/login/callback {loginId, redirectUrl}` -> `{status,error?}`.
- `cpa/auth/login/status {loginId}` -> `{status,error?}`.

The master remembers the login-to-credential association internally. The integrating application creates
an enabled credential placeholder before starting a new authorization; it chooses
the relative filename. Reauthorization uses the existing ID and original file.
CPA conventionally uses a UUID filename for new accounts.
There is no separate independently refreshed auth.json copy.

## Native request mapping

This Responses construction also applies to standalone image operations with
`imageApi: "responses"`. Direct Images use the native image DTOs described above.

The child uses the retained native model catalog, `ModelClient` prompt/request
builder, and request options. Tool parsing has one owner in the core CPA adapter;
the IPC envelope validator does not maintain a second tool-type allowlist.
Supported tools are native `function`, `custom`, `namespace` (function/custom
children), `web_search`, `tool_search` and hosted `image_generation`. Web search
and image generation run upstream. Image options such as `output_format` are
carried by the native tool specification. Tool search retains its declared
client/server execution mode; omission uses the native server mode. No local
image executor or client-tool execution is added. The parser
preserves namespace/name/description, grammar, `strict`, `defer_loading`, and
explicit search booleans and content types. Caller-owned function and tool-search
parameter schemas are carried as complete JSON objects in the native `JsonSchema`
representation, including all nested values, declarations, defaults, constraints,
boolean subschemas and future keywords. They do not pass through the internal
tool normalization/compaction policy or a schema-key allowlist. Schema fidelity is
checked in regression tests, not by a runtime round-trip equality gate. Chat-style
nested function definitions and `input_schema` tool definitions are normalized
without renaming tools. Function names do not select hosted tools or local
executors: a function named `WebSearch`, `ToolSearch`, `Bash` or `Agent` remains a
client function. Invalid tool envelopes still report their array location.
Function `output_schema` is not a supported wire field and is rejected.

Both ordinary Responses and Responses Lite serialize native `ToolSpec` values.
Lite uses the native `additional_tools` prefix and namespace convention; no raw
caller tools array overwrites the builder's output. Tool choice objects retain
their namespace semantics. Historical namespaces, call IDs, arguments and results
are preserved. There is no local tool router or agent loop. Response identity
projection does not rename tools or translate call IDs.

Current date and timezone are read from the worker's actual system clock and
runtime timezone configuration, with no country or language based default. An
unavailable IANA zone name is represented using the actual UTC offset. Only
the latest standalone declaration of each kind (`environment_context` and
`codex_apps_client_time_context`) is rewritten in place, even when assistant or
tool items follow it. Older declarations, quoted examples, code fences, paths,
OS declarations and tool payloads remain unchanged. Missing blocks and fields
stay missing: no synthetic message or date/timezone field is appended. Flat caller metadata
date/timezone fields are updated consistently; top-level date/timezone parameters
are consumed instead of being sent as unknown Responses parameters.

Field handling:

| Fields                                                                | Policy                                                                                                                       |
| --------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| model/input/instructions/tools/reasoning/service_tier                 | Parse and build using native types; normalize text messages and supported tool envelopes.                                    |
| session/thread/cache/turn identities                                  | Resolve once using the identity and lifecycle rules above.                                                                   |
| headers/authorization/cookies and reserved device metadata            | Worker-owned; caller values cannot override the transport.                                                                   |
| tool_choice/text/include and unknown top-level extensions             | Retain as documented request options, subject to tool-choice validation; extensions are not a guarantee of upstream support. |
| non-reserved flat client_metadata                                     | Retain unless managed above.                                                                                                 |
| previous_response_id/conversation/generate/store=true/background=true | Reject; stream=true is required.                                                                                             |
| response HTTP body                                                    | Restore matching known envelope identity fields, then forward as ordered base64 chunks; preserve opaque content.             |

The `sourceFormat` label does not convert whole Chat/Claude conversations into
Responses. Every client supplies the Responses request envelope and history.

HTTP authentication recovery can refresh credentials and retry a rejected 401;
accepted HTTP streams are never replayed. Body EOF is transport completion, not
an interpretation of any SSE event. All account activity ends when the master
has reaped the disabled child's process. Previously transmitted upstream work
cannot be recalled. The private state directory persists for reenable.

## Historical item IDs

Historical item IDs with a wrong type prefix are corrected using the native
item prefix and original suffix. Valid IDs, `call_id`, tool inputs and output
associations remain unchanged.

## Worker diagnostics

Default-visible JSONL stderr events include `cpa.worker.started` with credential
name, PID, CODEX_HOME and config path; `cpa.request.start`, `.mapping`, `.upstream`,
`.completed` and `.error` with `credentialId` and `rpcRequestId`. No `RUST_LOG`
setting is required. Validation failures and master dispatch errors are logged.
Mapping events distinguish source IDs, native metadata IDs, transport affinity,
cache key and turn-state length. Upstream stderr events include status, redacted headers
and structured failure code/type/message. Authorization and cookies stay masked;
full request/response payloads continue through the existing CPA notifications.
IPC notification, start and error headers carry original values. Log redaction
uses a separate copy and never changes IPC. Dedicated ticket notifications are
not serialized into ordinary logs.

`rpcRequestId` is the RPC UUID, not CPA's eight-character external log ID.
Use the actual upstream `x-request-id`, also present in CPA's detailed request
log, to correlate the two. stdout remains exclusively the JSON-RPC channel.

## Running outside the image

Set `CODEX_CPA_AUTH_DIR` to an existing shared directory and `CODEX_HOME` to the
private state root, then run `codex-app-server`. `CODEX_CPA_PORT` optionally changes
the default loopback port 38317. Only the master sets the child-only credential
path and ID environment variables; deployment never specifies a credential file.
