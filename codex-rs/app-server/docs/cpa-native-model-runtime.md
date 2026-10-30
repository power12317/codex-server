# Experimental native model execution

This document describes the sole model execution path and model-only extensions on
`codex/native-model-runtime`.

The fixed source reference is official main
`98f79517ab2392879c581626ec0588851b03d183`; the engine release stamp is
`0.166.0`, selected from the latest official stable release. The source also
contains fork extensions. Unmanaged identity retains official behavior; managed
workers use the separately versioned Desktop compatibility profile described in
[the identity audit](cpa-official-identity.md).

See [validation and reproduction](cpa-native-model-validation.md) for the fixed
official comparison, regression coverage and delivery limits.

## Execution boundary

Caller input is mapped to native `Prompt`, model controls and session metadata.
`InferenceSession::execute` calls the native model client's single-attempt entry.
The existing `request_max_retries` and `stream_max_retries` are zero for this
model-only client. HTTP transport retries, 401 recovery/resend, stream recovery,
WebSocket reconnect after failure and WebSocket-to-HTTP fallback do not replay a
worker invocation. The default CLI/Agent entry retains its original recovery.

Native serialization, authentication before a send, compression, transport
selection, cookies, configured proxy routing and normal redirect policy are
retained. The bridge does not build its own inference HTTP client or patch the
final wire DTO. The native HTTP builder disables protocol-level error retries;
normal redirects are not reclassified as retry failures. Connection preparation
and control-plane model/auth discovery are distinct from repeating model calls.

A model-only invocation does not create `ToolCallRuntime`, start an Agent turn,
execute caller commands or persist a conversation. Function/custom tools,
including `shell`, `exec` and `apply_patch`, are data. Deltas and ready events
stream immediately. Results come from a subsequent caller request; no missing
or aborted tool output is synthesized. No recovery-history buffer is needed.
Standalone `responses/compact` is not an operation in this protocol.

Logical threads retain their native client across explicit caller requests. Each
invocation leases a native `ModelClientSession` and drops it before releasing the
thread lease, returning the WebSocket and its previous-response cache through the
original native lifecycle. Cached turn contexts retain their own first ticket,
so a new turn can reuse the socket without inheriting the previous turn's ticket.
Concurrent requests in the same logical thread serialize; independent threads
and credentials remain concurrent. Credential isolation, synthetic turn lifetime
and title-request ephemerality are unchanged. The worker never silently retries
a failed call.

The caller supplies complete history. Responses output envelopes are converted
to typed native history: message/function status, encrypted output and output-text annotations/logprobs
are retained in the native replay types introduced by 0.163.0. Non-native display
extensions are consumed only where the typed representation cannot carry them. Native custom-call status, namespace, call IDs, tool results, encrypted
compaction items and `compaction_trigger` controls retain their typed semantics.
Unknown business item types still fail explicitly. The native client alone selects a
full request or `previous_response_id` plus incremental input; changed model,
tools, rules or history do not force an artificial cache hit.

Window metadata follows the native thread/window form: `window_id` is
`<mapped-thread-id>:<window-number>` and `context_window_id` is a window UUID.
Ordinary turn changes do not advance it. Caller window-marker changes or a new
compaction item advance the window once; replaying that item does not. A trigger
alone does not represent completed compaction. Only compact markers are retained,
without persisted conversation history or an automatic Agent/summary loop. Each
invocation receives its own window metadata snapshot while the turn ticket stays
shared within that turn.

WebSocket routing diagnostics retain only the sanitized `__oailb` node on the
actual connection. Handshake response cookies take precedence over cookies sent
by that handshake; explicit deletion or invalid values clear the diagnostic node.
Every model request on the connection reports it through the existing `oaiLbNode`
field, including the initial and reused `response.create` frames. Reconnection
derives a fresh value and never inherits another socket's diagnostic node. Cookie storage, parent-affinity pinning and sending follow the current upstream
native lifecycle; diagnostics observe actual wire headers rather than recreating
routing from the sanitized node. Handshake responses are not replayed as model data.

### WebSocket turn-state metadata compatibility

This branch deliberately extends official main at the reference above in one parser rule: both
`response.metadata` and `codex.response.metadata` may supply
`headers.x-codex-turn-state` on a WebSocket response. The unmodified official
baseline uses its typed event turn-state extraction; the fork keeps its existing
compatibility coverage for both envelope spellings.

The existing native `OnceLock` still retains the first value within the turn.
Subsequent requests on the same native session send that value in their existing
WebSocket client metadata; later tickets do not overwrite it. Other metadata,
model events and native request identity retain their existing handling.
The separate single-attempt policy below applies to this worker entry.
This does not add handshake-header capture, HTTP SSE metadata capture, a separate
ticket cache, or a runtime switch. Official executable names and version remain
unchanged.

## IPC contract and delivery

The `cpa/*` methods extend the official app-server envelope. Native model execution
is the only implementation; there is no execution capability switch, legacy sender or CPA
direct model fallback. The official runtime version is `0.166.0`.

The root WebSocket path is `/`; the retired versioned bridge route is not an
alias. Official initialization, health endpoints and authentication behavior are
retained. Read `cpa/capabilities/read` for actual behavior: `businessNotification`
is `cpa/inference/event`, `completionSemantics` is `native-terminal`,
`diagnosticsCompleted` and `responsesWebsocket` are true, and `ticketSnapshots`
is false. Native WebSocket support still depends on the selected provider/model
and configuration. A required `captureTicketSnapshot: true` is rejected before
model execution; absent or false is harmless. Runtime/source metadata never act
as a private compatibility gate. See the
[capability comparison](../../../docs/codex-server-api.md#connection-lifecycle)
for the stable HTTP implementation's different lifecycle and ticket support.

The delivery sequence is:

1. `cpa/inference/start` replies with public headers when the first accepted native
   business event is ready. It does not wait for model completion or diagnostics.
2. `cpa/inference/event {requestId, bodyBase64}` carries incremental accepted
   business bytes. Responses events are SSE even with a WebSocket upstream.
3. `cpa/inference/completed {requestId}` reports terminal native success, without
   a body field or replay of previously delivered events. HTTP 200 and EOF alone
   do not establish success.
4. `cpa/inference/diagnostics/completed {requestId}` ends the independent diagnostic
   FIFO. Keep reading its tail without delaying downstream business output.

There is no `attempt` notification or `attemptId`: each invocation has one native
attempt. Errors are returned to CPA, which applies its existing error/retry rules.
This contract does not introduce a new CPA retry engine or force every native
error to be non-retryable. Once downstream content is visible, the caller must
report the stream error rather than append a replacement response to it. A client
may explicitly retry with a new request. An IPC failure with an unknown native
outcome must not be treated as proof that the model call did not occur.

The master correlates worker requests using its private RPC sequence. It restores
the source request ID on all public messages, allowing CPA to reuse that ID on a
new connection while the previous diagnostic tail is pending. Same-connection
active IDs remain unique; cancellation targets only that connection's invocation.
The stderr `cpa.master.request` record retains the source ID in `rpcRequestId`
and the private worker ID in `details.workerRequestId`, so Docker logs remain
traceable from a CPA request without adding a public protocol field.

### Errors and immediate delivery

Before start, a failure is the start RPC error. After start it is
`cpa/inference/error {requestId, httpStatus, message, body?, headers?, nativeError?}`.
The original native HTTP status, response body and headers are preserved before
typed error classification can discard details. Rejected SSE/WS response payloads
retain their original JSON body and supplied error headers. `nativeError` is the
native typed classification. Source identity restoration still applies only to
recognized fields, not arbitrary text or opaque IDs.

The native HTTP parser returns a rejected response immediately in this entry;
it does not wait for EOF or let a later completed event hide the failure. The
ordinary CLI retains its original parser behavior. Worker loss ends unfinished
business and diagnostics without reversing an already delivered completion.

All accepted nonterminal events, including function/custom argument done and
client-tool output-item done, stream in observed order. Only terminal response
events wait for the native final result. No tool-specific buffer or completion
gate remains. The worker does not execute tools or invent their results.

## Supported input

| Input                                   | Native mapping / constraint                                                                                                                                                                                                                                                                                                     |
| --------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `model`                                 | Selects native model information from the worker's model manager.                                                                                                                                                                                                                                                               |
| `input`                                 | Text or native history items. Infer message/text shorthand, deserialize native metadata and preserve business text, opaque reasoning and call relationships. Unknown business item types and missing call IDs receive specific errors.                                                                                                |
| `instructions`                          | Native prompt base instructions.                                                                                                                                                                                                                                                                                                |
| Function/custom/namespace tools         | Native declarations with complete input and explicit output schemas. Chat/input_schema wrappers normalize without tool-name templates or local executors; missing description and non-strict function defaults are completed.                                                                                                   |
| `web_search`, `tool_search`             | Native declarations; tool-search parameter schemas retain all caller-supplied keywords and nested values.                                                                                                                                                                                                                       |
| Hosted `image_generation` declaration   | Native `AdditionalTools` developer input. The fork-only `ToolSpec::ImageGeneration` variant is not used by this entry.                                                                                                                                                                                                          |
| `tool_choice`                           | Native `auto`, `none`, `required`, and declared function/custom/namespace or hosted choices. Selection must reference a declared tool.                                                                                                                                                                                          |
| `parallel_tool_calls`                   | Explicit booleans remain explicit, including Lite; omitted values use native defaults.                                                                                                                                                                                                                                          |
| `reasoning.effort`, `reasoning.summary` | Native request controls; defaults come from worker configuration/native model policy.                                                                                                                                                                                                                                           |
| `reasoning.context`                     | Explicit native `auto`, `current_turn` or `all_turns`; absence uses native derivation. Old client Lite mirrors are consumed, not equality gates.                                                                                                                                                                                |
| `text.format`                           | `text`, `json_object` or `json_schema`; complete schema, explicit strictness and caller name are retained. An omitted schema name uses the native name.                                                                                                                                                                         |
| `text.verbosity`                        | Explicit verbosity enters native request controls; absence uses worker/model defaults.                                                                                                                                                                                                                                          |
| `service_tier`                          | Explicit tiers are retained; omission uses worker configuration/native defaults.                                                                                                                                                                                                                                                |
| `include`                               | Explicit lists, including `[]`, are retained. Omission uses native encrypted-reasoning inclusion.                                                                                                                                                                                                                               |
| `store`, `background`                   | `store` maps to the native bool without creating local conversations. `background=true` requires an asynchronous retrieval lifecycle not implemented by this streaming interface.                                                                                                                                               |
| `stream`                                | A downstream delivery preference; native model execution still owns its upstream transport.                                                                                                                                                                                                                                     |
| Session/thread/turn provenance          | Existing identity mapping and source response projection. Worker-owned authentication/device/routing fields cannot be overridden.                                                                                                                                                                                               |
| Existing environment date/time blocks   | Replace recognized current context fields in place using the worker's local time zone. No additional context message is appended.                                                                                                                                                                                               |
| Request controls and extensions         | Generation/token/temperature/top_p/truncation/cache-retention controls enter native DTOs; `max_tokens` and `max_completion_tokens` normalize to `max_output_tokens` with conflicting values rejected. Transport/display metadata is consumed or rebuilt. Native DTOs determine supported business fields; the adapter adds no business-key admission table. |

`top_k` and `prompt_cache_options` have no native request mapping in this entry
and return specific compatibility errors. Missing output `call_id` is inferred
only from a single matching unfinished call in the current input; ambiguous
associations are rejected. An explicit native function name/namespace association
can remain name-based without a fabricated ID.

Native additional-tool and compaction history items can also be supplied directly
in `input`. A successful controlled mock test proves the mapping and native path;
it does not prove that a production model service accepts every hosted-tool option.

Caller-owned function and tool-search schemas use the native `JsonSchema` object's
complete JSON carrier. They bypass the internal tool normalization/compaction
policy and retain declarations, defaults, numeric/string/array constraints,
definitions, boolean subschemas, tuple items and future keywords at every depth.
There is no schema-key allowlist or runtime round-trip equality gate. Chat-style
nested function and `input_schema` envelopes normalize to the same native tool
declarations. Tool names never select executors: `WebSearch`, `ToolSearch`, `Bash`
and `Agent` declared as functions stay client functions. Both Responses and Lite
use the native tool serializers; there is no final-wire tools-array overlay.

## Standalone Images input

`imageApi: "images"` delegates to the same `CodexImagesBackend` used by the native
image extension. No local image tool is installed or executed.

- Generations: `model`, `prompt`, optional `background`, `n`, `quality`, `size`.
- Edits: generation fields plus native `images` and optional `mask` references.
- Both accept native `stream`, `output_format`, `output_compression`, `partial_images`,
  `response_format`, `input_fidelity`, `moderation` and `user` options.
- Explicit `stream: false` does not remove `partial_images`; incompatible combinations
  remain visible to the native endpoint instead of silently changing caller intent.
- Omitted options, including `n`, stay absent. Unknown business options and malformed
  types receive concrete adaptation errors.

The active entry uses typed native image DTOs, `CodexImagesBackend` and `ImagesClient`.
It does not invoke the old prepared/raw inference sender. JSON responses are validated
as image envelopes and delivered without stripping response extensions. Native SSE
parsing immediately forwards accepted partial images and extensions; success requires
`image_generation.completed` or `image_edit.completed`. A stream ending before that
terminal event is an error. Business completion does not wait for diagnostic drain or
HTTP EOF after an accepted native terminal image event.

`generate: false` uses native WebSocket prewarming when available. HTTP-only entries
report that prewarming is unsupported before sending a model generation. Non-null
`previous_response_id` and `conversation` require caller-side history reconstruction;
the runtime retains native continuation caches rather than a stored-conversation API.
Null references are absence. Native cache reuse still chooses incremental requests.

## Diagnostics

`upstreamLogs` and `upstreamBodyLogs` are true. `cpa/inference/upstream` identifies
actual `transport: "http" | "websocket"` and has `request`, `response`, `body` and
`error` variants. `body` notifications contain diagnostic `bodyBase64` for the
request (native HTTP error bodies retain the native text-decoding representation). These notifications are never business completion. CPA must not log the
final business bytes a second time when `upstreamBodyLogs` is true.

Business and diagnostics use separate unbounded queues. The worker allows one
acknowledged diagnostic write at a time; the master prioritizes queued business
frames. Business does not wait for diagnostic callbacks or FIFO drain. They still
share a physical stdio/WebSocket writer: an in-progress write cannot be preempted,
so physical head-of-line blocking is not eliminated.

Request diagnostics include the native encoded bytes in `bodyBase64`; readable
`body` is included when those bytes are UTF-8. Decoding for a log must not alter
native request compression. HTTP observations occur after native default/trace
headers are applied. WebSocket handshake observations and `response.create`
frames are distinct; frame records retain their connection's actual endpoint and
do not invent new handshake headers for reused connections. Credentials/cookies
are redacted; an access-token digest may be used for correlation. Authentication
refresh payloads are outside the model diagnostic observer's request filter.

## Isolated image

The experimental workflow is `native-model-runtime-image.yml`. It accepts only
`codex/native-model-runtime` and publishes native amd64/arm64 images to:

```text
ghcr.io/power12317/codex-server-native-runtime:latest
ghcr.io/power12317/codex-server-native-runtime:sha-<full-commit>
```

It does not publish to the stable package and requires no `rust-v*` release tag.
The deployment time-zone mounts and minimal runtime image layout remain the same.

## Minimal third-party client exchange

Connect to `ws://127.0.0.1:38317/`, send `initialize`, then the
`initialized` notification and a capabilities request. For example:

```json
{"id":1,"method":"initialize","params":{"clientInfo":{"name":"third-party-integration","version":"4"},"capabilities":{"experimentalApi":true}}}
{"method":"initialized"}
{"id":2,"method":"cpa/capabilities/read","params":{}}
```

After checking the required capabilities, submit a request for an enabled credential
file already managed by the service:

```json
{
  "id": 3,
  "method": "cpa/inference/start",
  "params": {
    "requestId": "request-1",
    "credentialId": "account.json",
    "operation": "responses",
    "sourceFormat": "openai-response",
    "sessionId": "integration-instance-1",
    "request": {
      "model": "gpt-6.1-sol",
      "input": "Say hello.",
      "stream": true,
      "session_id": "caller-session",
      "thread_id": "caller-session",
      "turn_id": "caller-turn"
    }
  }
}
```

`params.sessionId` is the integration's stable isolation namespace. The body
`session_id` and `thread_id` identify the caller's conversation hierarchy; a child
thread may have a different thread ID. Credentials and installation identity are
owned by the worker and are not supplied in the business request.

Register handlers before starting inference: diagnostic notifications may arrive
before start. Consume business `event` frames incrementally and associate them
for their `requestId`; `completed` reports success without replaying the body.
Continue reading diagnostics until `diagnostics/completed`, even after business
success/error. An early RPC error also has a diagnostic tail for a valid request
ID. Use `cpa/inference/cancel` with `{"requestId":"request-1"}` to cancel it;
closing the connection cancels active work. Never silently replay uncertain
requests after an IPC failure.

The existing credential reload and manual OAuth methods retain their field
shapes. Capabilities describe actual business, diagnostics and transport support.
The native error's typed classification is additionally exposed as
`error.data.nativeError` when available.

## Optional IPC extensions and errors

The required frame identities and execution mode remain unchanged. Additional
optional envelope fields do not invalidate known messages. Business delivery and
transport diagnostics remain independent; optional diagnostic extensions cannot
establish or revoke native model success. Local request adaptation errors use
`request_adaptation_error`; native/upstream failures retain their native classification
and original captured response. No new local message capacity, execution switch,
retry owner or client-tool executor is introduced by compatibility adaptation.

See [native input fidelity](../../../docs/native-input-fidelity.md) for the field-ownership audit and type-complete replay fixtures.
