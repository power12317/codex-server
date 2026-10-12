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

This comparison covers controlled model execution, not the full Agent/tool loop.
The model-only boundary deliberately leaves newly generated client tool calls
uncommitted when an attempt fails: it neither inserts unpaired calls into recovery
history nor fabricates outputs. A separate test verifies function and custom
calls, retention of the caller's existing call/result history, native text-history
recovery, one successful delivery and absence of a local execution marker.

## Local checks

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
| Bridge unit tests | 14 passed |
| Master credential scanning and large-message IPC | 2 passed |
| Native API library | 199 distinct tests passed |
| Native HTTP-client library | 115 distinct tests passed |
| Protocol library and regenerated schema/export consistency | 320 passed; 1 intended generator test ignored |
| Python generated-contract tests | 5 passed |
| Release version preparation | 5 passed |
| Strict Clippy for core/API/HTTP/app-server/image extension libraries and binaries | Passed |
| Final binary build and actual request identity | Passed; `codex-tui`, version `0.162.1`, original User-Agent |
| CPA real-worker HTTP, WebSocket and native-fallback matrices against the final binary | All 3 passed with Go `-race` |
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

The final local worker is `codex-app-server 0.162.1`, SHA256
`6a19b9f60647fd5fe2a2a04b31644e1fc775b1ae3437183e6467b5801fef3f64`.
All three CPA matrices passed against this exact binary after the final worker
changes. These tests use the matching experimental CPA IPC v4 implementation.

## Compatibility and delivery limits

- IPC v4 and `nativeModelExecution: true` are required on both sides. The final
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
