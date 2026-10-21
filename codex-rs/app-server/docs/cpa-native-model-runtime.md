# Experimental native model execution (IPC v5)

This document describes the sole model execution path and IPC v5 contract on
`codex/native-model-runtime`.

The fixed native reference is Codex `0.162.1`, commit
`092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`. The client identity, originator and
User-Agent remain the existing Codex identities.

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
to typed native history: response-only message/function status and output-text
annotations/logprobs are validated and omitted where native input has no such
fields. Native custom-call status, namespace, call IDs, tool results, encrypted
compaction items and `compaction_trigger` controls retain their typed semantics.
Unknown extensions still fail explicitly. The native client alone selects a
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
derives a fresh value and never inherits another socket's diagnostic node. Native
cookie storage and sending remain unchanged; no handshake response is replayed.

### WebSocket turn-state metadata compatibility

This branch deliberately extends official `0.162.1` in one parser rule: both
`response.metadata` and `codex.response.metadata` may supply
`headers.x-codex-turn-state` on a WebSocket response. The unmodified official
baseline accepts only the former for turn-state extraction.

The existing native `OnceLock` still retains the first value within the turn.
Subsequent requests on the same native session send that value in their existing
WebSocket client metadata; later tickets do not overwrite it. Other metadata,
model events and native request identity retain their existing handling.
The separate single-attempt policy below applies to this worker entry.
This does not add handshake-header capture, HTTP SSE metadata capture, a separate
ticket cache, or a runtime switch. Official executable names and version remain
unchanged.

## IPC contract and delivery

The IPC contract is `protocolVersion: 5`. Native model execution is the only
implementation; there is no execution capability switch, legacy sender or CPA
direct model fallback. The official runtime version remains `0.162.1`.

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

| Input                                                                          | Native mapping / constraint                                                                                                                                                                            |
| ------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `model`                                                                        | Selects native model information from the worker's model manager.                                                                                                                                      |
| `input`                                                                        | Text or a native `ResponseItem` array. Message shorthand is normalized once. Unknown or lossy typed input fields are rejected.                                                                         |
| `instructions`                                                                 | Native prompt base instructions.                                                                                                                                                                       |
| Function/custom/namespace tools                                                | Native `ToolSpec` declarations; complete caller-owned parameter schemas are retained without registering executors.                                                                                    |
| `web_search`, `tool_search`                                                    | Native declarations; tool-search parameter schemas retain all caller-supplied keywords and nested values.                                                                                              |
| Hosted `image_generation` declaration                                          | Native `AdditionalTools` developer input. The fork-only `ToolSpec::ImageGeneration` variant is not used by this entry.                                                                                 |
| `tool_choice`                                                                  | Absent or `auto`; other choices cannot be expressed by this native prompt entry and are rejected.                                                                                                      |
| `parallel_tool_calls`                                                          | Native prompt preference; native model policy still applies (including Lite).                                                                                                                          |
| `reasoning.effort`, `reasoning.summary`                                        | Native request controls; defaults come from worker configuration/native model policy.                                                                                                                  |
| `reasoning.context`                                                            | Optional assertion of the context derived by the native builder. It cannot enable Lite or override native configuration.                                                                               |
| `text.format`                                                                  | `json_schema`, its schema and strictness; native schema name is `codex_output_schema`.                                                                                                                 |
| `text.verbosity`                                                               | Native client configuration for this invocation, with worker/model defaults when absent. The client view retains shared authentication/session state; the native builder still enforces model support. |
| `service_tier`                                                                 | Native model tier resolution.                                                                                                                                                                          |
| `include`                                                                      | If supplied, must match native `reasoning.encrypted_content` inclusion.                                                                                                                                |
| `store`, `background`                                                          | Only `false` is compatible.                                                                                                                                                                            |
| `stream`                                                                       | A downstream delivery preference; native model execution still owns its upstream transport.                                                                                                            |
| Session/thread/turn provenance                                                 | Existing identity mapping and source response projection. Worker-owned authentication/device/routing fields cannot be overridden.                                                                      |
| Existing environment date/time blocks                                          | Replace recognized current context fields in place using the worker's local time zone. No additional context message is appended.                                                                      |
| Arbitrary wire extensions, persistent response state, unsupported tool choices | Explicit input error; no final-wire overlay or alternative sender.                                                                                                                                     |

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
- Edits: the generation fields plus native `images` references.
- Optional `stream` describes downstream delivery only and is absent from the native Images DTO.
- Absent `n` stays absent; the adapter does not substitute its own default.
- `mask`, `partial_images`, `output_format`, `output_compression` and arbitrary extensions are rejected.

After the native backend validates success, the original JSON response is delivered
as a JSON business event followed by the payload-free completion, preserving
response fields outside the typed image DTO. The native Images backend is unary;
this does not claim incremental upstream image generation.

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

Connect to `ws://127.0.0.1:38317/cpa/v1/ws`, send `initialize`, then the
`initialized` notification and a capabilities request. For example:

```json
{"id":1,"method":"initialize","params":{"clientInfo":{"name":"third-party-integration","version":"4"},"capabilities":{"experimentalApi":true}}}
{"method":"initialized"}
{"id":2,"method":"cpa/capabilities/read","params":{}}
```

After checking the v5 message contract, submit a request for an enabled credential
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

The existing credential reload and manual OAuth methods retain their v3 field
shapes; their capability result now advertises version 5. The native error's typed
classification is additionally exposed as `error.data.nativeError` when available.
