# CPA bridge v2 (historical)

Superseded by the [native model contract](cpa-native-model-runtime.md). The current image does not use this startup or ownership contract.

This is the shared-file successor to v1. It requires CPA IPC major version 2 and
keeps v1 inference request fields, raw events, cancellation and single-response
semantics. The worker never executes returned tools or starts an agent turn.

## Startup and transport

Set `CODEX_CPA_AUTH_FILE` to one absolute CPA credential file path,
and `CODEX_CPA_WORKER_ID` to that worker's ID directly in Compose. These process
settings enable the worker. `CODEX_CPA_PORT` defaults
to 38317; assign 38318, 38319, etc. to additional workers. 18317 is reserved for
CPAMP. `CODEX_HOME` remains the worker's independent runtime/installation home.

The worker listens on `127.0.0.1` at `/cpa/v1/ws`; that URL path stays unchanged
across this IPC upgrade. CPA and Codex share a network namespace; the worker port
is not mapped externally. The bridge needs no key or Authorization header. Send
the normal `initialize` with `experimentalApi:true`, followed by
`initialized`. Only initialize and the RPCs below are exposed on the CPA worker.
The image's default entrypoint requires the fixed credential file and worker ID.
Existing official runtime components and background lifecycles remain available
inside the process. In v2 the old `[cpa_bridge]`/Unix v1 opt-in is superseded.

## Shared storage and ownership

Use the actual existing CPA filename. It is independent of worker ID and is never
renamed or copied into a worker-named file. Login and refresh write back to the
configured original path. The configured file uses CPA's flat shape:

```json
{"type":"codex","id_token":"...","access_token":"...","refresh_token":"...","account_id":"...","email":"...","expired":"...","last_refresh":"...","codex_cli":{"enabled":true,"worker_id":"worker-a","owner":"codex"}}
```

Codex uses and refreshes this credential only when `owner` is `codex` and
`worker_id` matches the configured worker. `enabled` is a CPA-managed user
preference; switching the master off sets owner to `cpa` while preserving that
preference. The runtime's AuthManager loads the same file directly, including on
401 recovery. It does not create or refresh a mirrored `CODEX_HOME/auth.json`.
An owner change cancels old inference and clears the old authentication source.
`cpa/credential/reload` applies a change immediately; the worker also reloads at
500 ms intervals. Every inference rechecks ownership and account identity.

Token saves merge the latest file, preserve unknown fields, and replace it
atomically using a same-directory 0600 `.tmp` file. This implementation deliberately
has no cross-process lock, lease, epoch or handoff protocol. A mode change and a
refresh already in progress are not guaranteed to be mutually exclusive.

## RPCs

- `cpa/capabilities/read {}` returns the previous capabilities with
  `protocolVersion:2`, `credentialFile` (basename), `authOwner` (`cpa`, `codex`, or
  null if unavailable), `manualOAuth:true`, `upstreamLogs:true`, and `upstreamBodyLogs:true`. `credentialId` remains the worker
  ID. `accountId`/`authMode` are null when no usable Codex-owned login is loaded.
- `cpa/credential/reload {}` reloads owner/tokens and returns the capabilities.
- `cpa/inference/start` and `cpa/inference/cancel` retain the v1 fields and
  `cpa/inference/event`, `cpa/inference/error`, `cpa/inference/completed` ordering.
- `cpa/auth/login/start {}` returns `{loginId,authUrl,state}`.
- `cpa/auth/login/callback {loginId,redirectUrl}` completes authorization and
  returns `{status,error}`.
- `cpa/auth/login/status {loginId}` returns `{status,error}`, with status
  `pending`, `completed`, or `error` and error either null or a safe message.

## Browser OAuth

CPA first provisions the configured file with `type:codex` and the ownership
metadata; token fields can be absent. CPAMP calls CPA, which calls login/start,
then displays the returned official browser authorization URL. The user completes
browser login and pastes the full `http://127.0.0.1:1455/auth/callback?...` URL into
CPAMP. CPA forwards it to login/callback. No device-code substitution is used.

The worker retains one pending official PKCE flow. It survives the initiating WS
connection and is replaced by another login/start. The manual callback parses the
bound redirect URL and validates the existing OAuth state before the official code
exchange. It never fetches the supplied URL. Successful authorization writes the
flat CPA file and reloads AuthManager. OAuth credentials never appear in RPC
responses. All file selection comes from process settings, never request params.

## Upstream request logs

Workers with `upstreamLogs:true` send ordered `cpa/inference/upstream` notifications.
These diagnostics may precede the start reply. Each actual HTTP attempt, including
attempts during 401 recovery, emits a request and then a response or transport error:

- `{requestId,kind:"request",url,method,headers,body,accessTokenSha256,oaiLbNode}`
- `{requestId,kind:"response",statusCode,headers,body,oaiLbNode}`
- `{requestId,kind:"body",bodyBase64}`
- `{requestId,kind:"error",message}`

Headers are maps of string arrays. Request body is the prepared JSON string sent
to the inference transport. Response body is the complete HTTP failure body, or
null for a successful stream. With `upstreamBodyLogs:true`, body notifications
carry each raw transport byte chunk as Base64, preserving SSE comments, event/id
lines and UTF-8 split across chunks. CPA decodes and joins those chunks for its
existing response logger instead of logging parsed events twice. The original
bytes and transport errors are passed unchanged to the official parser; the
existing event stream continues to drive model response processing. The URL, method and headers come from the prepared transport
request, including official defaults and applicable shared cookies. Transport-added
wire details such as Host framing are not synthesized. Sensitive credential/cookie
headers are represented by `[REDACTED]`; token SHA-256 and the parsed `__oailb` node
provide log correlation without transferring tokens. Absent correlation values
are null. CPA feeds these records through its existing request logging hooks.

HTTP failures also include `body` and `headers` in RPC `error.data`, alongside
`httpStatus`. Accepted-request errors expose nullable `body` and `headers` in
`cpa/inference/error`. Client response header filtering remains CPA's responsibility.
This extension does not alter usage accounting, inference retries, raw response
events, or OAuth state.
