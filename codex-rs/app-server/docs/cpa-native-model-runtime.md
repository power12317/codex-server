# Experimental native model execution (IPC v4)

This document describes the sole model execution path and IPC v4 contract on
`codex/native-model-runtime`.

The fixed native reference is Codex `0.162.1`, commit
`092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`. The client identity, originator and
User-Agent remain the existing Codex identities.

See [validation and reproduction](cpa-native-model-validation.md) for the fixed
official comparison, regression coverage and delivery limits.

## Execution boundary

Caller input is mapped to native `Prompt`, model controls and session metadata.
`InferenceSession::execute` calls `ModelClientSession::stream` through the native
attempt driver. The existing Agent sampler also uses that driver and the same
recovery implementation. Native provider configuration, authentication recovery,
compression, HTTP/WebSocket selection and fallback retain their original owners.
The bridge does not construct a separate inference HTTP client or patch the
serialized native request.

A model-only invocation does not create `ToolCallRuntime`, start an Agent turn,
execute caller commands or persist a conversation. Function/custom tools are
model data, including tools named `shell`, `exec` and `apply_patch`. Their results
come from a subsequent caller request. Native history processing is shared for
recovery, except that the model-only entry never synthesizes aborted tool results. Client
tool calls from an unsuccessful response are uncommitted: they are neither
delivered nor inserted as unpaired calls into recovery history. Completed text
items remain in native recovery history. This is an explicit client-tool handoff
boundary, not a claim of equivalence to the full Agent's tool execution/history.
Caller history and native compaction items remain supported; standalone
`responses/compact` is not an operation in this protocol.

A logical caller thread reuses its native model client, including fallback state,
across turn contexts. Each turn serializes access to its own native client session.
Independent contexts remain concurrent. Existing credential isolation, synthetic
turn lifetime and title-request ephemerality remain in force.

## IPC contract and delivery

The IPC contract is `protocolVersion: 4`: successful completion carries the final
business body, and upstream diagnostics carry their actual transport. Native
model execution is the only implementation; there is no execution capability
switch, legacy sender or CPA direct model fallback.

A successful invocation returns `cpa/inference/start` with public response headers,
followed by `cpa/inference/completed` containing `requestId` and `bodyBase64`.
`bodyBase64` is required, including when the final byte sequence is empty.
The completion frame carries the final business bytes and native success together.
EOF or HTTP 200 alone cannot produce this notification. Native final errors use
the authoritative RPC error's `httpStatus`, `body` and `headers`; diagnostic errors
from earlier attempts must not overwrite that result.

If a worker exits after the start reply but before the completion frame, the
supervisor sends `cpa/inference/error` for the interrupted IPC transfer. This
notification reports worker loss only; model failure and cancellation use the
start RPC error. It does not trigger replay or choose another execution path.

Accepted events are buffered per attempt and released after native model success.
This delays first-byte delivery until that success. It is not real-time token
streaming. Internal errors and failed attempt bytes remain diagnostic data.
Accepted unknown extensions and function/custom/image deltas retain their JSON
payloads; upstream WebSocket events are framed as downstream SSE. Existing
identity projection applies only to recognized identity fields, never opaque
response IDs, call IDs, arguments or user text.

## Supported input

| Input | Native mapping / constraint |
| --- | --- |
| `model` | Selects native model information from the worker's model manager. |
| `input` | Text or a native `ResponseItem` array. Message shorthand is normalized once. Unknown or lossy typed input fields are rejected. |
| `instructions` | Native prompt base instructions. |
| Function/custom/namespace tools | Native `ToolSpec` declarations, without registering executors. |
| `web_search`, `tool_search` | Native declarations, subject to their typed schema. |
| Hosted `image_generation` declaration | Native `AdditionalTools` developer input. The fork-only `ToolSpec::ImageGeneration` variant is not used by this entry. |
| `tool_choice` | Absent or `auto`; other choices cannot be expressed by this native prompt entry and are rejected. |
| `parallel_tool_calls` | Native prompt preference; native model policy still applies (including Lite). |
| `reasoning.effort`, `reasoning.summary` | Native request controls; defaults come from worker configuration/native model policy. |
| `reasoning.context` | Optional assertion of the context derived by the native builder. It cannot enable Lite or override native configuration. |
| `text.format` | `json_schema`, its schema and strictness; native schema name is `codex_output_schema`. |
| `text.verbosity` | Native client configuration for this invocation, with worker/model defaults when absent. The client view retains shared authentication/fallback state; the native builder still enforces model support. |
| `service_tier` | Native model tier resolution. |
| `include` | If supplied, must match native `reasoning.encrypted_content` inclusion. |
| `store`, `background` | Only `false` is compatible. |
| `stream` | A downstream delivery preference; native model execution still owns its upstream transport. |
| Session/thread/turn provenance | Existing identity mapping and source response projection. Worker-owned authentication/device/routing fields cannot be overridden. |
| Existing environment date/time blocks | Replace recognized current context fields in place using the worker's local time zone. No additional context message is appended. |
| Arbitrary wire extensions, persistent response state, unsupported tool choices | Explicit input error; no final-wire overlay or alternative sender. |

Native additional-tool and compaction history items can also be supplied directly
in `input`. A successful controlled mock test proves the mapping and native path;
it does not prove that a production model service accepts every hosted-tool option.

## Standalone Images input

`imageApi: "images"` delegates to the same `CodexImagesBackend` used by the native
image extension. No local image tool is installed or executed.

- Generations: `model`, `prompt`, optional `background`, `n`, `quality`, `size`.
- Edits: the generation fields plus native `images` references.
- Optional `stream` describes downstream delivery only and is absent from the native Images DTO.
- Absent `n` stays absent; the adapter does not substitute its own default.
- `mask`, `partial_images`, `output_format`, `output_compression` and arbitrary extensions are rejected.

After the native backend validates success, the original JSON response is delivered
atomically, preserving response fields outside the typed image DTO.

## Diagnostics

`upstreamLogs` and `upstreamBodyLogs` are true. `cpa/inference/upstream` identifies
actual `transport: "http" | "websocket"` and has `request`, `response`, `body` and
`error` variants. `body` notifications contain diagnostic `bodyBase64` for each
attempt (native HTTP error bodies retain the native text-decoding representation). These notifications are never business completion. CPA must not log the
final business bytes a second time when `upstreamBodyLogs` is true.

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

After checking the v4 message contract, submit a request for an enabled credential
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

The client may receive diagnostic `cpa/inference/upstream` notifications while
waiting. Success requires both the start response and the matching completion
notification. Decode `completed.params.bodyBase64` as the final SSE payload (or
JSON for standalone Images). An authoritative RPC error ends the invocation.
Use `cpa/inference/cancel` with `{"requestId":"request-1"}` to cancel it; closing
the connection also cancels its active work. Do not silently replay uncertain
requests after an IPC failure.

The existing credential reload and manual OAuth methods retain their v3 field
shapes; their capability result now advertises version 4. The native error's typed
classification is additionally exposed as `error.data.nativeError` when available.
