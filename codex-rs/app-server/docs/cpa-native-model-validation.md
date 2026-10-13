# Native model execution validation

This report covers the experimental `codex/native-model-runtime` branch. It does
not certify the stable v3 bridge or a published Linux image. The fixed reference
is official Codex `0.162.1`, commit
`092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`.

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

| Check | Result |
| --- | --- |
| Native model matrices, client-tool boundary and verbosity/fallback sharing | 4 passed; 16 reference comparisons matched |
| Existing recovery tests | 2 passed |
| Native context history tests | 91 passed |
| Inference input, time and tool adapter tests | 14 passed |
| Real master/worker IPC integration | 15 passed |
| Bridge unit tests | 16 passed |
| Master credential scanning and large-message IPC | 2 passed |
| Native API library | 193 passed; six unit tests for deleted bypass senders were removed with their implementation |
| Native HTTP-client library | 115 distinct tests passed |
| Protocol library and regenerated schema/export consistency | 320 passed; 1 intended generator test ignored |
| Python generated-contract tests | 5 passed |
| Release version preparation | 5 passed |
| Strict Clippy for core/API/HTTP/app-server/image extension libraries and binaries | Passed |
| Final binary build and actual request identity | Passed; `codex-tui`, version `0.162.1`, original User-Agent |
| CPA real-worker HTTP, WebSocket and native-fallback matrices against the final binary | All 3 passed with Go `-race`; 114 leaf cases |
| Experimental image workflow | `actionlint` passed |
| Rust formatting and diff whitespace checks | Passed |

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
changes. These tests use the matching experimental CPA IPC v4 implementation.

## Single implementation cleanup

Native model execution is unconditional. The extra execution capability field,
the retired business-chunk notification and both unused fork HTTP bypass senders
were removed with their registrations, generated exports and obsolete tests.
The Responses endpoint source now matches the fixed official commit byte-for-byte.
The remaining native model/observer tests cover the actual execution path.

IPC v4 describes the atomic completion and actual-transport diagnostic message
structure. Completion requires `bodyBase64`, including an empty byte sequence;
there is no legacy empty-completion form. Native failures and cancellation use
the start RPC error. The supervisor retains only the necessary asynchronous
failure notification for worker loss after a successful start reply and before
the completion frame. Neither that notification nor protocol versioning selects
an alternate model implementation.

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

- IPC v4 defines the message structure. The final
  successful body is in `cpa/inference/completed.bodyBase64`; diagnostics cannot
  substitute for an authoritative native result.
- Business bytes are buffered until native success, so token delivery is delayed.
  Cancellation during native work cancels the invocation; a later diagnostic
  delivery delay cannot reclassify a settled native result.
- Supported parameters and explicit input errors are documented in
  [the protocol guide](cpa-native-model-runtime.md). There is no wire overlay,
  alternative sender, local tool dispatcher, or automatic CPA replay.
- Native hosted-image declaration mapping was verified against the official
  executor and mock services. Live production hosted-image acceptance was not
  tested. Standalone Images uses the existing native Images backend.
- The local binary is macOS arm64. Linux amd64/arm64 build, startup, time-zone
  mounts and client identity checks belong to the isolated image workflow.
  Triggering that workflow does not establish that an image was published.
- The experiment publishes only `ghcr.io/power12317/codex-server-native-runtime`.
  Stable package tags and the existing stable workflow are not changed.
