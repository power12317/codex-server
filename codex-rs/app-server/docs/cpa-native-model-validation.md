# Native model execution validation

This report covers the experimental `codex/native-model-runtime` branch. It does
not certify the stable HTTP bridge or a published Linux image. The fixed reference
is official Codex `0.163.0`, commit
`83086188ae0614bd882818ff1aa23f3194962012`.

## 2026-10-23 official release and shared credential locking

The current source incorporates official `rust-v0.163.0`, commit
`83086188ae0614bd882818ff1aa23f3194962012`. The dated sections below describe
earlier checkpoints. Committed Cargo workspace and local lockfile versions remain
`0.0.0`; release preparation reproduces the tested `0.163.0` build metadata
byte-for-byte. The native request identity is checked against the same-platform
official release artifact, not inferred from source strings.

Normal token saves hold CPA's canonical credential-path `.lock` across reading
the latest JSON, enabled-state validation and atomic replacement. The deterministic
subprocess test observes actual lock contention before the parent changes nested
metadata, disables the credential or renames it. Symlink aliases use the same lock;
disabled/removed credentials are not overwritten or recreated.

Official output replay now represents message/function status and output-text
annotations/logprobs in native history. The compatibility parser retains these
fields, including empty arrays, so native incremental request comparisons remain
valid. The original continuation assertions passed without weakening cache
comparison or forcing a previous response ID. IPC diagnostics preserve original
header values; ordinary stderr receives a separately redacted copy.

Validation used synthetic credentials and local upstreams:

- **36** service library checks, **64** real master/worker integration cases,
  the dedicated root-path check and all **3** shared-lock subprocess scenarios passed.
- **316** protocol checks passed; stable/experimental exports and the Python SDK
  were generated with the repository writer.
- **57** core client, **15** login client, **196** API, **108** tools,
  **10** request-builder and **2** retry-policy checks passed.
- Exact executable version output and **4** complete initialize sequences matched
  the official macOS arm64 app-server. Actual worker headers and both example
  clients passed; the full worker User-Agent equals the official binary's result.
- The unchanged CPA native `d3df0a428a14add4b5280d45fb61599f5043658c`
  passed **199** real-worker leaf checks with the Go race detector against this
  candidate, including the previously failing continuation combinations.
- Release stamping, formatting, whitespace and source-placeholder round-trip checks passed.

The immutable local worker SHA256 is
`451a3ee01b7724d330c0167a3bb71436372b4f9a77912788fdafd48dba6bf78b`.
Artifact hashes are reproduction evidence, not compatibility gates. These local
results do not claim completion of the Linux image workflow.

## 2026-10-23 capability discovery and root WebSocket

The public master and dedicated worker use the root WebSocket path; the retired
versioned bridge route has no compatibility alias. Official initialization,
health routes, authentication and `0.162.1` identity remain unchanged.
The model-only `cpa/*` methods remain fork extensions carried by the app-server
envelope. Private protocol numbers and opaque replacement gates are absent.

Capabilities now describe the existing native behavior:
`businessNotification: "cpa/inference/event"`,
`completionSemantics: "native-terminal"`, `diagnosticsCompleted: true`,
`responsesWebsocket: true` and `ticketSnapshots: false`. Business events still
stream immediately, completion remains payload-free native success, and the
independent diagnostic tail ends with `cpa/inference/diagnostics/completed` in
either order with the business terminal. WebSocket support is conditional on the
provider/model/configuration for each request.

Required source-ticket capture is rejected before a model send. Omitted or false
`captureTicketSnapshot` is harmless; a successful model invocation does not
pretend to produce an unsupported ticket. Normal schemas, client tool handling,
identity mapping, Images paths, cancellation, original diagnostics and zero model
retries retain their existing execution paths. Runtime/source metadata are
informational and do not select an implementation.

Stable and experimental protocol exports were regenerated through the repository
script, including its Python SDK step; **320** protocol tests passed. The manual
fixture writer was run separately for both export modes.

The focused run passed **36** app-server library checks, **64** real master/worker
integration cases and the dedicated worker root/retired-path transport check.
Coverage includes incremental HTTP/WS and Images output, native completion,
diagnostic tails, cancellation, complete schemas, continuation and zero retries.
Required ticket capture failed with no model request as declared. Both branch
examples accepted the live capability response. The actual identity probe,
shared nested credential metadata probe, **5** release checks, formatting and
whitespace checks also passed. The immutable interface checkpoint worker has
SHA256 `c436cba305ff8c0101fc5022be1a3b5a0a98f1a8607b4bd42e5e43d4033073af`;
this checksum is reproduction metadata and never a compatibility gate.

## Client tool schema fidelity (2026-10-21)

Caller-owned function and tool-search parameter schemas now use a complete JSON
object carrier inside native `JsonSchema`/`ToolSpec` values. They bypass the
internal tool normalization subset, and the adapter's runtime round-trip equality
admission checks have been removed. No schema-key or tool-name allowlist is used.
Native request construction, authentication and sending still own the request.

- `just test -p codex-tools -p codex-core --lib -E 'package(codex-tools) | test(client::inference_tools::tests)'`:
  **115 passed**, comprising 109 tool tests and six adapter tests.
- `just test -p codex-app-server --lib --test all cpa_`: **83 passed**.
- `cargo fmt --all --check` and `git diff --check`: passed.

The permanent fixture retains the WorkBuddy incident's exact 22 function tools.
Responses and Lite regressions compare every declaration/schema value in actual
local upstream HTTP requests, including all 41 fields lost by the old adapter,
original tool names and `strict:false`. Additional unit coverage includes nested
constraints, definitions, boolean subschemas, tuple items and future keywords.
Function/custom/namespace tools and the internal schema policy also pass.

A separate controlled HTTP replay used the complete original Chat request
through the paired CPA checkout at
`65f134152ad9696b1568b0372f57af02fad4eaa3`. The built Rust master/worker used
temporary fake credentials and a local model mock. Both Chat turns passed:
22 exact tool schemas, reasoning effort `high`, live tool chunks before upstream
completion, complete parameter deltas, `finish_reason:tool_calls`, usage, `[DONE]`
and function call/result associations. Model sends were exactly two; direct CPA
model sends and local tool execution were zero. This does not certify acceptance
by a production account's upstream model or a deployed image.

## Native continuation and caller-owned windows (2026-10-19)

The model-only entry now leases its logical thread's native session for one
invocation and returns the socket through `ModelClientSession::drop`. Idle cached
turn objects retain their own ticket but no longer hold the native socket cache.
The original native incremental comparison and sender remain unchanged. Typed
output-to-input adaptation accepts response-only message/function status and
output-text annotations/logprobs without forwarding those response fields into
native history. Call identity, namespaces, tool outputs and compaction controls
remain native typed input; unknown fields still fail explicitly.

Window metadata now uses the mapped thread and compaction window number, with a
separate context-window UUID. Caller window changes or newly replayed compaction
items advance it once; ordinary turn changes and compaction triggers alone do
not. The same turn keeps its first ticket through a window change. Client tools
remain caller-executed, worker retries remain zero, and incremental native business delivery is retained.

Three real master/worker tests verify four same/cross-turn requests over one
socket, concurrent turn serialization without ticket mixing, and an eight-request
tool/compaction sequence including native full sends after rules/tools/model
changes. All three pass. The full service regression passes 35 unit and 46
integration tests. The native client test group passes 53 tests, including the
new typed replay adapter and the ordinary client's existing regressions.
The unchanged native golden test
`responses_websocket_v2_incremental_requests_are_reused_across_turns` also passes
against this checkout (one test, no skipped cases).

The paired CPA `-race` matrix and diagnostic-blocking regression pass 106 leaf
cases (113 passing nodes including parent tests). Its seven-request continuation
fixture supplies business histories of lengths `1/3/5/7/9/2/4`; native WebSocket
business inputs are `1/1/1/7/9/2/1` over exactly one handshake, with the expected
previous-response IDs. The window advances from 0 to 1, the prompt-cache key stays
stable, native HTTP zstd remains enabled as configured, and CPA direct model
egress is zero. The matrix was repeated against the final artifact with all 106
leaf cases passing in 5.838 seconds; evidence is in
`/private/tmp/cpa-runtime-continuation-final-artifact.jsonl`.

The exact paired and identity-verified macOS arm64 artifact is
`/private/tmp/codex-native-continuation-20261019/codex-app-server-final`, SHA256
`257dabf059f27a2017a2714ced0ee7703c872c840bf35bb1827fb6860ad7603a`.
It is the final locked standalone build and passes the actual request identity
probe. After the Rust regression runs, the adapter's three verified-object
`expect` calls were replaced with conditional access; no conversion policy
changed. The final paired matrix above covers that exact rebuilt binary. This
local validation does not establish Linux image publication. Official executable
and request identity remain at `0.162.1`.

The unfiltered `cargo clippy -p codex-app-server -p codex-core --lib --tests --
-D warnings` is not clean at the branch baseline: `analytics.rs` has an existing
await-held analytics lock (line 193), `expect` (line 284) and collapsible condition
(line 913); `native_model_blackbox_tests.rs:174` has a redundant method closure.
Both files remain byte-for-byte equal to the starting HEAD. The follow-up check
uses command-only allowances for `clippy::await_holding_invalid_type`,
`clippy::expect_used`, `clippy::collapsible_if` and
`clippy::redundant_closure_for_method_calls`, after `-D warnings`; no product lint
allowance or unrelated cleanup is included. Logs are under
`/private/tmp/codex-native-continuation-20261019/`.
That filtered Clippy command, `cargo fmt --all --check`, documentation formatting
and diff whitespace checks pass. This is not a claim that unfiltered baseline
Clippy is clean.

## WebSocket connection node diagnostics (2026-10-14)

The connection now retains the sanitized `__oailb` node determined by its actual
handshake and supplies it to each request's independent diagnostic observer.
Previously, even the first `response.create` notification lost a node reported
by the handshake, and reused requests had no connection-level snapshot. Handshake
request observation now runs after native cookie-store insertion, so fallback
reflects the cookie actually sent. Native cookie policy, business bytes, client
identity, worker retry policy and native business delivery are unchanged.

Four real master/worker WebSocket cases pass. Each opens three connections under
the same source session/turn and sends two successful requests on each. The first
two sockets then receive a separate failing invocation, which must return without
retry before the caller opens the next connection. Tests verify first/reused
request nodes, response-cookie priority, actual-request-cookie fallback, deletion,
invalid values, reconnect updates, no stale node on a cookie-free new connection,
redaction, and absence of synthetic handshake responses or frame cookie headers.

CPA's existing production logging consumes the unchanged `oaiLbNode` field;
no CPA production change is required. Its `-race` matrix passes 97 real-worker
leaf cases and one diagnostic-blocking regression. The new case verifies six
model requests over three sockets, exact node values on both rounds, one handshake
per socket and zero direct CPA model sends. Evidence is recorded in
`/private/tmp/cpa-ws-oailb-final-matrix.log`, using the final standalone build
`/private/tmp/codex-ws-node-20261014/codex-app-server`, SHA256
`5624ff3e5e08ed3750869bc499d9ff1e6bfff2172afba156d26069b2f32b8519`.
This exact artifact also passes the actual client-identity probe
(`codex-app-server 0.162.1`, `codex-tui/0.162.1`).

Local checks pass 24 service unit tests, 42 master/worker integration tests
(including the four new cases), 195 API tests, 115 HTTP-client tests and 18
WebSocket-client tests. The secure-handshake cookie fixture additionally compares
all observed request cookies with those received by the server, including cookies
inserted by the native store. One existing HTTP redirect log-capture assertion
failed in the parallel run; the complete HTTP suite passed with one test thread,
without changing the test or HTTP routing implementation. The standalone locked
build, strict Clippy for app-server/API/HTTP-client/WebSocket-client libraries and
tests, Rust formatting, documentation formatting and diff whitespace checks pass.

The zero-retry rollout results below are retained as the preceding baseline.

## Native zero-retry checkpoint

The user selected immediate native event delivery with worker retries set to
zero. This supersedes the earlier candidate's native recovery and attempt-boundary
contract. `event {requestId, bodyBase64}`, payload-free `completed`, post-start
`error`, and independent `diagnostics/completed` remain. Attempt notifications and
IDs have been removed from the schema and clients.

The model-only client config sets request and stream retries to zero. Its native
single-attempt entry also bypasses 401 replay and WebSocket error fallback, while
HTTP protocol retries are disabled in the native client builder. Normal redirects,
proxy policy, request construction, client identity and successful WebSocket
turn-state reuse are retained. No public switch or alternate sender was added.
Native model errors are returned immediately to CPA for its existing rules.

Tests cover HTTP 401/429/503, handshake errors without HTTP fallback, standalone
Images failures, partial EOF with no second model call, immediate rejected SSE/WS
payloads while the upstream connection remains open, realtime tool-ready events,
normal HTTP redirects, identity mapping and diagnostic lifecycle. Final local
verification on macOS arm64 with Rust 1.95.0 passed:

| Check                                                                    | Result                                                                                 |
| ------------------------------------------------------------------------ | -------------------------------------------------------------------------------------- |
| Service unit tests                                                       | 24 passed                                                                              |
| Real master/worker integration tests                                     | 38 passed                                                                              |
| Native model test groups                                                 | 4 passed                                                                               |
| Existing native client tests (`client::tests::`)                         | 32 passed                                                                              |
| Native API library                                                       | 195 passed                                                                             |
| Native HTTP-client library                                               | 115 passed                                                                             |
| Protocol library and generated export consistency                        | 320 passed; 1 generator test ignored by default and run explicitly during regeneration |
| Strict Clippy for app-server, API, HTTP-client and login libraries/tests | Passed with warnings denied                                                            |
| Rust formatting, final binary build and actual request identity          | Passed                                                                                 |
| Third-party example against the real master/worker                       | Passed with incremental event and payload-free completion                              |
| Paired CPA real HTTP/WS matrix                                           | 92 leaf cases passed with Go `-race`                                                   |

The paired matrix used
The retained correlated worker snapshot had SHA256
`5c30250a6bfb252cacfba04507a5dcdb7496ee98d811341831777a4dfaa8566d`.
It verifies delivery of tool deltas and ready events before upstream completion,
immediate native errors while upstream connections remain open, zero worker
retries or error-driven HTTP fallback, and CPA-owned retries without CPA model
egress. With two configured CPA retries, the fixture observes three worker sends
and zero direct CPA sends. The first 780-byte turn-state ticket is replayed
exactly, including prefixed metadata compatibility. Results are recorded in
the coordinator's retained final matrix log.

The final rebuilt binary is
The retained final worker snapshot had SHA256
`61911ad31c11ea928e84a3603f822b62621c1ec234aa0503acd9e2118f6b0e8e`.
Only lint-equivalent production edits and test/documentation corrections followed
the paired candidate. The full paired matrix was not repeated for those edits;
the local checks above and identity/example probes cover the final source and
binary. The executable remains `codex-app-server 0.162.1`, with native
`codex-tui/0.162.1` request identity. These local results do not certify Linux
image publication.

The following reference and earlier candidate results are historical. Their
native-recovery behavior is intentionally different from the current authorized
zero-retry policy; they must not be presented as current retry equivalence.

## Execution ownership and independent reference

The bridge maps business input to `Prompt`, native configuration and session
metadata, then invokes `InferenceSession::execute`. Both this entry and the
original Agent sampler call `execute_response_attempts`; recovery decisions live
in the shared `recover_response_stream`. Authentication, request serialization,
compression, WebSocket selection and fallback stay in the existing model client.
The former bridge sender and final JSON overlay are removed.

The reference checkout is an archive of the fixed official commit, with only
workspace build-version normalization and a test-only executor/fixture module.
Its executor calls the original `ModelClientSession::stream` and original
`handle_response_stream_error`, not the new inference entry or shared driver.
Separate Cargo target directories prevent official and candidate artifacts with
identical package names from being mixed.
The reproduced fixture matches the tested reference fixture byte-for-byte. Eight
production files covering model execution, recovery, history, HTTP and WebSocket
transport match the fixed official commit byte-for-byte in both reference trees.

`deploy/native-runtime/compare-traces.py` reports **16 of 16 matching scenarios**:
success, EOF recovery, content-filter recovery, failure followed by completion,
final invalid prompt, final EOF, interrupted result, hosted image declaration,
tool calls, partial text item recovery, Lite, compaction history, verbosity,
WebSocket success, WebSocket recovery, and native WebSocket-to-HTTP fallback.
Only dynamic mapped identities, native item IDs, times and trace values are
normalized. Field presence, identity relationships, request bodies, attempt
counts/order, model results and final error classifications remain compared.

These 16 scenarios do not cover prefixed turn-state metadata. The explicitly
authorized compatibility extension described below intentionally differs from
the unmodified official baseline for that event; the comparison is not a claim
that both event names have identical ticket behavior in official `0.162.1`.

This comparison covers controlled model execution, not the full Agent/tool loop.
The model-only boundary deliberately leaves newly generated client tool calls
uncommitted when an attempt fails: it neither inserts unpaired calls into recovery
history nor fabricates outputs. A separate test verifies function and custom
calls, retention of the caller's existing call/result history, native text-history
recovery, one successful delivery and absence of a local execution marker.

## Baseline local checks (834df84f)

The test host is macOS arm64, using Rust 1.95.0, Python 3.14 and local mock
upstreams/test credentials. Counts below are distinct tests within each row;
scenario comparisons are not additional Rust unit tests.

| Check                                                                                 | Result                                                                                       |
| ------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Native model matrices, client-tool boundary and verbosity/fallback sharing            | 4 passed; 16 reference comparisons matched                                                   |
| Existing recovery tests                                                               | 2 passed                                                                                     |
| Native context history tests                                                          | 91 passed                                                                                    |
| Inference input, time and tool adapter tests                                          | 14 passed                                                                                    |
| Real master/worker IPC integration                                                    | 15 passed                                                                                    |
| Bridge unit tests                                                                     | 16 passed                                                                                    |
| Master credential scanning and large-message IPC                                      | 2 passed                                                                                     |
| Native API library                                                                    | 193 passed; six unit tests for deleted bypass senders were removed with their implementation |
| Native HTTP-client library                                                            | 115 distinct tests passed                                                                    |
| Protocol library and regenerated schema/export consistency                            | 320 passed; 1 intended generator test ignored                                                |
| Python generated-contract tests                                                       | 5 passed                                                                                     |
| Release version preparation                                                           | 5 passed                                                                                     |
| Strict Clippy for core/API/HTTP/app-server/image extension libraries and binaries     | Passed                                                                                       |
| Final binary build and actual request identity                                        | Passed; `codex-tui`, version `0.162.1`, original User-Agent                                  |
| CPA real-worker HTTP, WebSocket and native-fallback matrices against the final binary | All 3 passed with Go `-race`; 114 leaf cases                                                 |
| Experimental image workflow                                                           | `actionlint` passed                                                                          |
| Rust formatting and diff whitespace checks                                            | Passed                                                                                       |

The first API/HTTP runs inherited a machine-wide HTTP proxy: one API and three
HTTP assertions failed because the proxy changed localhost routing, header casing
or connection errors. The affected API test and all 31 route-aware HTTP tests
passed with proxy variables removed only from those test processes. Native proxy
behavior and assertions were not changed.

Worker integration covers atomic completion, authoritative errors, cancellation,
credential refresh/reload, manual OAuth, credential-named homes and migration,
tool/unknown event preservation, caller identity projection, turn continuity,
child hierarchy, ephemeral title requests, absence of persisted conversations,
unsupported input, standalone Images, in-place time replacement, and concurrent
source/credential isolation. All 15 integration tests passed together after the
final completion-versus-diagnostic-delivery correction.

## Reproducing the reference comparison

From the repository root, prepare an empty reference directory with Python 3.11
or newer:

```sh
python3 deploy/native-runtime/prepare-reference.py /tmp/codex-native-reference
```

Use the repository's Rust toolchain and normal native build prerequisites for
both trees. Run the first command from the reference's `codex-rs` directory and
the second from the candidate's `codex-rs` directory:

```sh
CARGO_TARGET_DIR=/tmp/codex-official-target \
CODEX_NATIVE_REPORT=/tmp/codex-official.jsonl \
cargo test -p codex-core --lib native_model_blackbox -- --test-threads=1

CARGO_TARGET_DIR=/tmp/codex-candidate-target \
CODEX_NATIVE_REPORT=/tmp/codex-candidate.jsonl \
cargo test -p codex-core --lib native_model_ -- --test-threads=1
```

The report files are append-only; use fresh files for each comparison. From the
candidate repository root:

```sh
python3 deploy/native-runtime/compare-traces.py \
  /tmp/codex-official.jsonl /tmp/codex-candidate.jsonl \
  --output /tmp/codex-native-comparison.json
```

Run worker contract tests from the candidate's `codex-rs` directory:

```sh
cargo test -p codex-app-server --lib cpa_bridge
cargo test -p codex-app-server --test all suite::v2::cpa_bridge::
```

The CPA experiment uses `CODEX_RUNTIME_TEST_BINARY` to point its three
`TestCodexRuntime.*RealWorker` matrices at this worker. These exercise real worker
processes, HTTP and WebSocket sockets, native fallback, public Responses/Images
handlers, downstream streaming/nonstreaming forms, tool-result continuation,
native errors and cancellation. The corresponding `-race` test requires zero
CPA model egress and zero server-side client-tool execution.

The single-implementation cleanup worker was `codex-app-server 0.162.1`, SHA256
`902df54637d8820621d78cb782b6d7bdc0ad9e08c6b9fe03c9e8eec292e9287c`.
All three CPA matrices passed against this exact binary after the final worker
changes. These tests use the then-current experimental CPA implementation.

## Single implementation cleanup

Native model execution is unconditional. The extra execution capability field,
the retired business-chunk notification and both unused fork HTTP bypass senders
were removed with their registrations, generated exports and obsolete tests.
The Responses endpoint source now matches the fixed official commit byte-for-byte.
The remaining native model/observer tests cover the actual execution path.

At that historical checkpoint, the retired atomic-completion contract carried
actual-transport diagnostic messages. Completion then required `bodyBase64`,
including an empty byte sequence, and native failures/cancellation used the start
RPC error. The supervisor retained an asynchronous failure notification for worker
loss after the start reply and before completion. These historical completion
shapes have since been replaced by incremental native events and payload-free
completion; the current capability result and contract above define the lifecycle.

The authorized main identity correction from `c56e646d` is included separately,
without merging other main changes. The master uses native initialize identity
APIs, while the worker retains official `codex-tui` and `0.162.1` identity. The
updated third-party example passed a real master/worker invocation against the
local mock upstream using the sole completion-body path.

Cleanup evidence is recorded in `/private/tmp/codex-native-runtime/single-path-checks.json`,
`single-path-comparison.json`, and `/private/tmp/cpa-native-only-real-worker.log`.
The worker artifact is
`/private/tmp/codex-native-runtime/artifacts/codex-app-server-single-path-final`.

## Authorized WebSocket turn-state compatibility

The parser now also extracts `headers.x-codex-turn-state` from
`codex.response.metadata`. This is an intentional one-condition extension of
official `0.162.1`; standard `response.metadata` remains supported. No sending,
recovery, completion, session-lifetime or native first-write rule is changed.
WS handshake headers and HTTP SSE metadata do not gain new capture behavior.

Regression coverage includes both event names, missing/unrelated fields,
preserved model metadata, and four real master/worker cases: standard only,
prefixed only, standard before prefixed, and prefixed before standard. Each case
uses the same full source turn and exactly one upstream WebSocket connection,
completes two requests, and inspects the actual second request frame. It must
carry the first 780-character ticket and the previous response ID, preserving
the first value when a later metadata event supplies a different ticket.

The read-only pre-fix probe and independent official probe are recorded in
`/private/tmp/codex-turn-state-20261013/results.json` and `official-results.json`.
Both showed standard metadata replaying the ticket and prefixed metadata failing
to do so. Post-fix validation must expect that prefixed-event behavior to differ;
no extra capability or transport selection is introduced to hide the difference.

The compatibility worker is
`/private/tmp/codex-turn-state-20261013/codex-app-server-metadata-fix`, SHA256
`7f1bd59f3ae4b5be5381cb42d0b274acb04e17ac49824a8c206cf835d68c3602`.
The same CPA test-only overlay fails against the pre-fix binary and passes against
this artifact: the second successful request's actual ticket equals the first
response's 780-character ticket, and its CPA log changes from `0/780` to `780/780`.
All 115 paired real-worker leaf cases pass with Go `-race`; CPA production code
and its image do not need a change for this fix.

All 19 app-server master/worker integration cases pass, including the four new
standard/prefixed/mixed-order cases. The second frame replays the first ticket
exactly, not merely another value of the same length, and normal text/completion
events remain delivered.

The worker build, strict API/app-server Clippy, Rust formatting and actual client
identity checks pass. The executable remains `codex-app-server 0.162.1` and its
upstream identity remains `codex-tui/0.162.1` with the native platform suffix.

The native API library passes all 195 tests, including both new extraction tests
and the existing metadata/model-event cases. The initial run inherited a local
HTTP proxy and failed the existing blob-transport diagnostic fixture; all 195
passed with proxy variables removed only for the test process. No assertion or
native proxy behavior was changed.

Direct worker probes also confirm unchanged exclusions: handshake-only WS tickets
and HTTP SSE metadata-only tickets are not captured. A different full source turn
starts with empty routing state. Logs and JSON traces for these probes are under
`/private/tmp/codex-turn-state-20261013/`; the paired CPA run is recorded in
`/private/tmp/cpa-turn-state-fix-20261013/after.log`.

## Compatibility and delivery limits

- The native extension carries incremental business bytes without attempt fields. Native success
  is a separate payload-free completion, and errors before/after start use the
  RPC error/error notification respectively.
- Every accepted nonterminal event, including tool-ready events, streams without
  waiting for complete model success. Only terminal events await the native result.
- Worker retries are zero across Responses, auth-error resend, HTTP and Images;
  a WebSocket failure does not switch to HTTP. CPA owns its existing downstream
  error/retry policy. Unknown IPC outcomes must not be assumed safe to replay.
- Diagnostic queues are independent and unbounded; a shared physical write cannot
  be preempted. Log completion does not delay business or native execution.
- There is no local caller-tool execution, fabricated tool output, persistent
  conversation, alternative HTTP sender, runtime selector, or new capacity limit.
- The local candidate is macOS arm64. Linux amd64/arm64 publication belongs to the
  isolated experimental workflow after paired acceptance. Starting that workflow
  does not establish image publication.
- Stable main/gateway worktrees and the stable image package are unchanged.
