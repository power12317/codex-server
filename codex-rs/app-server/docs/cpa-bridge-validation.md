# Codex Server inference validation record

## 2026-10-23 official release and shared credential locking

The current source incorporates official `rust-v0.163.0`, commit
`83086188ae0614bd882818ff1aa23f3194962012`, while retaining the main HTTP inference
path and source-ticket behavior. Committed Cargo workspace/local lockfile versions
remain `0.0.0`; release preparation stamps the tested official `0.163.0` identity.
The native builder now retains the replay metadata supported by this release,
including message/function status and output-text annotations/logprobs.

Normal token updates participate in CPA's canonical credential-path `.lock`.
The lock covers reading current JSON, checking enabled state and atomically
replacing normal token fields. A deterministic subprocess test verifies concurrent
metadata updates, disabling and renaming before the worker acquires the lock;
nested SIWC metadata stays opaque and is retained.

Validation used local upstreams and synthetic credentials:

- **22** service library checks, **76** real master/worker cases, the dedicated
  root-path check and all **3** shared-lock subprocess scenarios passed.
- **316** protocol checks passed; stable/experimental exports and Python SDK types
  were regenerated with the repository script.
- **57** core client, **15** login client, **199** API, **108** tools and
  **10** request-builder checks passed.
- Exact version output and **4** complete initialize sequences matched the official
  macOS arm64 app-server. The actual worker User-Agent equals the official binary's
  result; request headers, both example clients and shared metadata probes passed.
- The source-ticket probe made **5** model requests and received **3** expected
  snapshots with original IPC headers and separate ordinary-log privacy.
- The unchanged CPA main `97b7abb6` passed **44** real-worker leaf checks with the
  Go race detector against this final candidate.

The immutable local worker SHA256 is
`cb71f1cc5e3853839386c80c60ba389eb4e6c5cb0c1e6e2dfd2eb070059f278c`.
Artifact hashes are reproduction evidence, never compatibility gates. Earlier
dated sections below describe their own checkpoints; local results do not claim
completion of the Linux image workflow.

## 2026-10-23 capabilities and root WebSocket

The public master and dedicated worker listener now accept the root WebSocket
path. The retired versioned bridge path rejects upgrades with HTTP 404. Official
initialization, health routes, authentication and runtime identity are retained.
Model-only `cpa/*` methods remain explicit fork extensions; there is no private
protocol number or replacement contract identifier.

Main advertises `businessNotification: "cpa/inference/body"`,
`completionSemantics: "http-body-eof"`, `diagnosticsCompleted: false`,
`responsesWebsocket: false` and `ticketSnapshots: true`. HTTP EOF keeps its
existing meaning; clients parse the business response to determine model success.
Source tickets, identity restoration, immediate business bytes, cancellation and
zero worker model retries retain their existing behavior. Clients select required
behaviors from capabilities rather than matching a whole branch template.

Validation used only local upstreams and synthetic credentials:

- **22** app-server library tests and **76** real master/worker integration tests
  passed, including the complete schemas, explicit controls, stream/error/cancel,
  independent credentials, identity/time replacement and source-ticket cases.
- The dedicated worker root-path/retired-path test passed. The actual master
  capability probe passed, including both branch examples against the same worker.
- **320** protocol tests passed; the manual fixture writer was run separately for
  stable and experimental exports, including the Python SDK generation step.
- The actual identity probe passed with `codex-app-server 0.162.1`, official
  `codex-tui` originator and the unchanged native User-Agent.
- The source-ticket probe made **5** model requests and received **3** expected
  snapshots, preserving raw IPC headers and private ordinary logs.
- **5** release-preparation checks, Rust formatting, example syntax, changed
  documentation links and the whitespace check passed.

The immutable local worker has SHA256
`bc6777f84d336e2934919db61e67310d2629a429b06ffc673d394abdfa7d3d45`.
This is an artifact checksum for reproduction, not an interface compatibility gate.
Earlier dated sections below describe their own historical checkpoints.

## 2026-10-21 caller-owned tool schema fidelity

The inference adapter now carries complete caller-owned parameter schema objects
in native `JsonSchema`/`ToolSpec` values. It no longer deserializes those objects
through the internal tool normalization subset or rejects a request using the
adapter's serialization round-trip comparison. No schema-key or tool-name
allowlist is used. Ordinary native tool construction keeps its own schema policy.

- `just test -p codex-tools --lib`: **109 passed**.
- `just test -p codex-core --lib client::inference_tools::tests`: **6 passed**.
- `just test -p codex-app-server --lib --test all cpa_`: **89 passed**.
- `cargo fmt --all --check` and `git diff --check`: passed.

The retained WorkBuddy fixture contains the incident's exact 22 function tools.
Both Responses and Lite tests compare the actual local upstream HTTP request,
including every parameter value, original names and `strict:false`. Tests also
cover recursive/boolean/tuple schemas, definitions, unknown future keywords,
function/custom/namespace tools, parameter deltas and call/result history. The
server does not execute the returned client command or start an extra model turn.

A separate controlled HTTP replay used the incident's complete original Chat
request through the paired CPA checkout at
`39322f24fa6d78adc36436af6adac005ee07c147`. It used temporary fake credentials,
the built Rust master/worker and a local model mock. Both Chat turns passed:
22 exact tool schemas, reasoning effort `high`, tool chunks before upstream
completion, reassembled arguments, `finish_reason:tool_calls`, usage and `[DONE]`.
The follow-up retained the function call/result association. Worker model sends
were exactly two; CPA direct model sends and local tool execution were zero.
This is local contract validation, not production-account or deployed-image
acceptance by the real upstream service.

## 2026-10-15 source snapshots and original IPC values

The current main extension retains business delivery and adds opt-in exchange-scoped ticket
snapshots. Per the latest user requirement, IPC request/response/error headers
now preserve original values; this supersedes historical notification-redaction
claims below. Ordinary logs use a separate redacted copy. The current contract,
audit and verification record is maintained in
[the source ticket document](../../../docs/codex-runtime-source-ticket_CN.md).

## 2026-10-13 upstream stable 0.162.1 merge

Fork baseline: `7acdea942f41748653c4d3e81a4a98f21f7fe8ae`. The actual upstream
merge target is `rust-v0.162.1`, commit
`092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`. The latest development main snapshot
`97239fd052db455bd08c41afcdab738e0fe83b87` is not a merge target. Existing fork
features and the previously integrated official main commits remain preserved.

Most 0.162.0 features were already present. This stable merge adds default instant
interruption, fixes multiline asynchronous questions and their links, and limits
shared-daemon feature compatibility checks to explicit CLI overrides. Related
upstream test changes are included. It does not add a local agent loop to CPA.

The workspace, 161 local lockfile package versions and deployment release stamp
are synchronized to `0.162.1`. Third-party lockfile packages are unchanged.
Release preparation now accepts official stable manifests with development
lockfile versions and normalizes implicit workspace members and qualified local
dependency references. It still rejects unexpected versions before writing.
The capability revision now identifies the actual stable merge target rather
than the obsolete initial fork base. Model-only extension methods and client identity names remain
unchanged; the compiled version drives version headers and User-Agent.

Validation of this merged stable source:

- `cargo build --locked -p codex-app-server --bin codex-app-server` passed;
  the executable reports `codex-app-server 0.162.1`.
- CPA unit and real master/worker integration: **70/70 passed**, including
  capability/runtime version, native User-Agent, images/Responses, shared-token
  refresh, logs, cancellation, identity/turn-state and timezone regressions.
- API, protocol, login and feature suites: **571/571 passed**, including native
  image request construction and schema/export consistency.
- Stable-fix coverage: **10/10 CLI daemon-startup**, **47/47 selected TUI unit**,
  **1/1 TUI daemon compatibility**, and **2/2 Python SDK turn-control** tests passed.
- Core interruption/compaction/retained-context coverage: **66 distinct cases
  passed**. The first run passed 62 of 65 cases; three existing Code Mode cases
  timed out because the clean target directory lacked `codex-code-mode-host`.
  After building that test helper, all three passed along with the separately
  selected Guardian heartbeat case (**4/4**). Their test bodies were unchanged
  from the fork base; no production logic, assertions or wait limits were changed
  to obtain the passing result.
- Release preparation: **5/5 passed**, including stable-tag lockfile mismatch,
  implicit members, idempotence and rejection without writes.
- Strict production-library Clippy passed for API, core, app-server, protocol,
  login, features and TUI with `-D warnings`. Relevant Rust formatting,
  documentation examples and diff whitespace checks passed.
- CPA compatibility was independently tested using a clean export of its existing
  committed main `70ca1592`, without merging CPA upstream or modifying its working
  tree. `TestCodexRuntimeImagesRealWorker` passed with Go `-race` against the binary
  below: six basic image combinations and six public HTTP combinations, native
  normalization/streaming, ordinary Responses, hosted images, summary/compaction,
  failures, cancellation and mixed-account routing. Model HTTP counts remained
  **CPA = 0, worker = 1** in the basic image cases. Evidence:
  `/private/tmp/cpa-codex-01621-real-worker-race.log`.

The macOS arm64 development binary is
`/private/tmp/codex-stable-01621/codex-app-server-0.162.1`, SHA-256
`401eb27b38b5be890b3ede705f274c3498925ae9ea41a38cacb89b86c28f4f70`.
Local logs and the artifact manifest are under `/private/tmp/codex-stable-01621/`.
These checks use mock upstream services and synthetic credentials; they do not
establish live model behavior or Linux Docker execution. No full-workspace suite
was run.

No CPA source changes, SIWC integration or additional credential locks are part
of this Codex merge. No remote push, tag or image publication is performed.

## 2026-10-09 standalone image operations and native normalization correction

Audit and fetched main baseline: `1ddf40146790e805a9b3a488ffde36364b17b29c`.
Image operations and the optional `imageApi` enum remain on the model-only bridge.
Worker-owned authentication, provider/network selection, mask/multiple images,
raw JSON/SSE output, cancellation and transport EOF semantics are retained.

The initial image-specific Responses builder incorrectly trimmed and replaced
native output using the caller's original field presence. That implementation
and its exports were removed. The three existing core files were restored from
Git HEAD; both ordinary and image Responses now call the unchanged
`prepare_inference_request` pipeline. The old regression requiring absent
message types and shorthand content to survive native construction was replaced
with assertions that worker normalization determines actual upstream HTTP.

Direct Images input is rebuilt with the existing native generation/edit DTOs.
The missing/null/value mirror and repeated-scalar bypass were removed. Required
model/prompt and edit images are validated, native enums/reference serialization
apply, and optional nulls are omitted. Mask, streaming/output options and business
extensions remain supported. Ordered duplicate image data URLs remain intact.

Validation of the corrected implementation:

- CPA unit and real master/worker integration suite: **70/70 passed** with
  `just test -p codex-app-server --lib --test all cpa_`.
- Native Images client/request tests and complete protocol library suite:
  **327/327 passed**, including JSON/TypeScript schema and precomputed export
  consistency. The protocol representation did not change during this correction.
- Ordinary and Lite image Responses regressions inspect the actual upstream HTTP
  body: missing message types and shorthand text are normalized, instructions
  become developer input, historical item IDs are corrected, native stream/store
  defaults apply, Lite disables parallel tools, and native identity replaces
  caller identity. No caller-field pruning runs after native construction.
- Generation/edit and Images/Responses JSON/SSE matrices retain valid image
  options, duplicate images, mask and business extensions. Native required-field
  and scalar-type failures do not reach upstream model HTTP.
- Partial-image cancellation closes the HTTP connection; truncated chunked
  transfer emits an error instead of completion. Upstream failures do not replay.
- Existing ordinary Responses, hosted image tools, summary/compaction, identity
  and UTC/Singapore/Tokyo time-context tests passed.
- `cargo build --locked -p codex-app-server --bin codex-app-server` passed.
  Strict production-library Clippy passed for API, core, app-server and protocol
  with `-D warnings`. Scoped Rust formatting, API Markdown formatting,
  documentation examples and diff whitespace checks passed.
- CPA reran `TestCodexRuntimeImagesRealWorker` with Go `-race` against the
  corrected binary below. Native-normalization and native-stream cases passed,
  along with six generation/edit JSON/multipart/streaming combinations and six
  public HTTP combinations. The basic image cases recorded **CPA model HTTP = 0,
  worker model HTTP = 1**. Ordinary built-in images, summary/compaction, upstream
  failures, cancellation and mixed-pool refusal to retry locally also passed.
  Evidence: `/tmp/cpa-worker-native-normalization-race.log`. CPA separately
  reported its five affected package suites and required build passed.

The corrected macOS arm64 development binary is
`/private/tmp/codex-images-runtime-20261009/codex-app-server-native`, SHA-256
`cb75e72470d81d959ed05f76d498fd626d2b537dd6c288b043090551200c6edd`.
Evidence logs use `/private/tmp/codex-images-native-*.log`; the artifact manifest
is `/private/tmp/codex-images-runtime-20261009/native-validation.json`.
Earlier results for `codex-app-server-final` validate superseded behavior and
must not be used as evidence for this correction. HTTP services and credentials
in these tests are local fixtures; live model and Linux container execution are
not covered by this local build.

No production client identity/User-Agent, deployment or CI settings were changed.
No local tool execution, agent session, automatic compaction or new capacity limit
was introduced. Both projects must negotiate the image capabilities; an older
worker remains unsupported and must not trigger direct upstream fallback.

## 2026-10-07 native hosted-tool compatibility

Baseline: `3628694b0`. The inference tool parser now accepts hosted
`image_generation` and native `tool_search` declarations, including forced tool
selection. Hosted image options are serialized by `ToolSpec`; they do not
register local tool execution. The model-only request envelope, native inference builder,
identity handling and replace-only time processing are unchanged.

- CPA unit and real-process integration suite: **52/52 passed** with
  `just test -p codex-app-server --lib --test all cpa_`.
- Protocol/tools library tests and selected core inference/client tests:
  **472/472 passed**. Stable and experimental schema fixtures match the original
  protocol definitions.
- New Responses and Lite cases compare actual HTTP bodies with IPC diagnostics,
  preserve hosted image options, apply native tool-search defaults, normalize
  historical item IDs, rebuild the cache identity and construct the native Lite
  tool/instruction prefix. Each case sends two requests and observes exactly two
  upstream model POSTs.
- Existing UTC/Singapore/Tokyo regressions still replace time fields in place
  across tool continuations and never append a synthetic time message.
- Strict Clippy passed for the core and tools production libraries with
  `-D warnings`. Scoped Rust formatting, documentation links/JSON examples and
  `git diff --check` passed.

Tests use local mock HTTP services and fake credentials. The rebuilt
`codex-rs/target/debug/codex-app-server` is a macOS arm64 development artifact;
these tests do not exercise a live model account. Deployment configuration is
unchanged. The existing main-branch workflow builds native amd64/arm64 images,
verifies startup and host timezone mounts, and publishes
`ghcr.io/power12317/codex-server:latest`. The workflow and deployment guide remain
unchanged. Image build completion is tracked by GitHub Actions separately from
the local test results above.

## 2026-10-07 replace-only time context and host timezone mounts

Baseline: `4ca102b7b`. Inspection of the reported request showed 1,270 incoming
items and 1,271 outgoing items: the current user/developer time declarations
preceded tool continuations, so the previous adapter skipped them and appended
another user context. The regression fixture preserves that ordering and item
count using synthetic content; no production request log or credentials are
included in the repository.

- Time-context unit tests: **5/5 passed**. The latest declaration of each kind
  is replaced in place across tool continuations, older declarations and paths
  stay intact, repeated normalization is idempotent, and absent fields/blocks
  are never added.
- CPA unit and real-process integration suite: **50/50 passed**. The three new
  UTC/Singapore/Tokyo subprocess cases compare actual HTTP request bodies with
  IPC diagnostics, verify both current time declarations, preserve tool-result
  position/count, and verify that ordinary text gets no synthetic time message.
- Local Docker checks passed for UTC, Singapore and Tokyo tzfile mounts with
  `TZ` unset, both with and without a name-file mount. The image's UTC database
  remains unchanged. These checks used the cached Debian runtime with the new
  regular-file `/etc/localtime` layout; full new Linux images are validated on
  both architectures by the publication workflow.
- Strict Clippy passed for the core and app-server production libraries with
  `-D warnings`. Rust formatting, changed Markdown/YAML formatting, documentation
  links and `git diff --check` passed.
- Shellcheck, actionlint, standalone Compose validation and composition of the
  timezone overlay with CPA's existing Compose file passed. CPA files were read
  for validation and were not edited.

The standard Compose file and CPA overlay now supply the host tzfile mount.
Existing containers must be recreated with that mount; an image alone cannot
read the host's unmounted timezone configuration. The publication image name and
`latest` tag policy, native identities, User-Agent and protocol fields remain
unchanged. Documentation remains in English.

## 2026-10-07 exchange-local gateway node reporting

Baseline: `d9470b49e`. Response `oaiLbNode` now prefers an explicit response
`__oailb` value; absent that cookie, it reports the node captured from the exact
outbound request. Explicit empty/invalid response values remain null. Success
and HTTP error responses use the same rule. No response-time cookie-jar lookup,
node cache, RPC field, protocol version or retry policy was added.

- CPA unit and real-process integration suite: **47/47 passed**, including ten
  new gateway-node cases. Eight cases each exercise HTTP 200 and HTTP 400:
  request-only, unrelated response cookie, response replacement, response-only,
  neither, unrelated cookie without a request node, deletion and invalid value.
- The mock HTTP server's received Cookie headers are checked against the request
  observations delivered over the real master/worker WebSocket IPC. Cookie and
  Set-Cookie values remain redacted in notifications.
- Four overlapping exchanges across two credential workers confirm that response
  replacement on one exchange cannot overwrite another request's fallback node.
- A local TLS peer closes before returning any HTTP response. The original request
  notification retains its node, an error notification follows, and no response
  notification is invented. TLS setup is used because a host proxy can translate
  a plain HTTP disconnect into an actual HTTP 502 response.
- Strict production-library Clippy passed with `-D warnings`. Extending Clippy
  to the entire `all` integration target reports 14 existing `unwrap_used`
  diagnostics in unchanged `hierarchy_tests.rs` and `parser_tests.rs`; the new
  tests have no remaining diagnostics. Those unrelated files were not modified.
  Rust formatting, API documentation formatting and `git diff --check` passed.
- Native User-Agent/originator, master/worker identities, protocol definitions,
  entrypoint and image publication configuration are byte-identical to baseline.
  All credentials, node cookies and HTTP services in these tests are fixtures.

## 2026-10-06 Codex Server naming and integration documentation

Source baseline: `77e714af9a421cf3bd9668bdd30f51136744bcdf`. The fork is now
`power12317/codex-server`. Product documentation remains in English, matching the
existing documents. The service protocol, Rust crate/binary names and runtime
behavior are unchanged by this documentation and packaging update.

- Updated the project overview, installation and deployment guides, and added
  a third-party API reference with a dependency-free Node.js client example.
- Fixed the image workflow's repository guard after the GitHub rename; new image
  publications target `ghcr.io/power12317/codex-server:latest`.
- Ran the example against the existing local debug master/worker executable with
  temporary state, fake credentials and a local HTTP model fixture. Readiness,
  initialization and capabilities worked with an empty credential directory.
  Inference restored source session IDs; a rejected HTTP request exited with an
  error; Ctrl+C produced cancellation status 499. Three inference calls made
  exactly three model POSTs and wrote no conversation rollout JSONL files.
- Workflow actionlint, entrypoint shellcheck, JavaScript syntax, changed-document
  local links/JSON examples, formatting and whitespace checks passed. No Rust
  production code changed, so the full Rust suite was not rerun for this rename.

The example verification did not use real model-service credentials or billable
inference. Container builds and startup checks run in the main-branch image
workflow. New registry tags are usable only after that workflow publishes them.
Historical validation records below retain their original repository/image names.

## 2026-10-05 logical hierarchy and response identity mapping

Baseline: `17e357cce98ad3fe257a508ae55713c453f0e1b9`; official upstream remains
`afb436df8b70bb5bc57b86d9a3e829968988cd21`. These results use local HTTP mocks and
fake credentials. They do not establish real model-service acceptance or replay
of production CPA traffic.

- CPA unit and real-process integration tests: **37/37 passed** using
  `just test -p codex-app-server --lib --test all cpa_`.
- All app-server-protocol library tests and selected core inference/client tests:
  **360/360 passed** using
  `just test -p codex-app-server-protocol -p codex-core --lib -E 'package(codex-app-server-protocol) | test(client::inference_) | test(client::tests)'`.
- The experimental app-server schema was regenerated; stable and experimental
  schema fixture checks passed in the protocol run.
- Strict library Clippy passed for core, app-server and app-server-protocol with
  `-D warnings`; Rust formatting and `git diff --check` completed afterward.
- Hierarchy fixtures cover root, child and grandchild identity/header mapping,
  parent references, explicit subagent metadata, root cache affinity and separate
  routing state even when threads share a source turn ID.
- Two title requests get distinct synthesized turns and no previous routing
  state. Root state survives title calls. **Eight RPC requests produce exactly
  eight model POSTs**, without automatic title-generation requests. The worker
  state directory has no conversation rollout JSONL files.
- Response fixtures exercise source session/thread/cache restoration on each
  request, including changed cache keys on a reused context; absent source fields
  remove matching synthesized values. Structured HTTP errors are projected while
  their diagnostic copies retain native identities. Model/tool content,
  `response.id`, `call_id`, business metadata and opaque state are preserved.
- SSE tests cover every split position in a CRLF/multiline/UTF-8 fixture, lone CR,
  exact bytes for unchanged events, EOF fragments, immediate heartbeat forwarding,
  JSON bodies and the framing buffer limit.

The capability extension keeps `rawBody: true` and the existing
base64 body channel, and advertises `responseIdentityMapping: true`. Read-only
inspection of CPA's current Go adapter confirmed ordinary additive JSON decoding
and its then-current inference-only/rawBody checks; no CPA or CPAMP files changed.
The outer CPA session scope remains an isolation boundary. Only source values
actually received by Codex can be restored. Context state is worker-local and is
not restored after restart; requests still carry their own conversation history.
Linux builds and image smoke checks run in the dedicated image workflow after
push. These local results do not claim that a new image is already published.

## 2026-10-05 request normalization

Baseline: `9e40d2bca`, official source `afb436df8b70bb5bc57b86d9a3e829968988cd21`.
Validation uses fake credentials and local HTTP mocks, not a replay of business
requests or billable inference against the real service.

- CPA/master unit and real-process integration tests: **27/27 passed**.
- Native client, new identity/time/tool parser units, and all shared tool units:
  **147/147 passed**, including tool-continuation history preservation.
- Strict library Clippy passed for core, app-server and tools with `-D warnings`;
  Rust formatting and whitespace validation completed afterward.
- Both standard Responses and Responses Lite compare the prepared request with
  the actual HTTP body received by the mock. The sanitized incident fixture keeps
  six top-level tools: custom exec, two functions, two namespaces (eight nested
  functions), and web search. Namespace names, descriptions, schema `encrypted`,
  strict/defer-loading flags, custom grammar, `external_web_access: false`, and
  `search_content_types: [text, image]` survive native serialization. Lite uses
  native `additional_tools` placement. No caller tools array overwrites the
  builder's output.
- Namespace/call-ID history associations and exact returned SSE bytes are checked.
  Unsupported tool errors identify the failing array position.
- Session/thread/cache identity equality is checked in body and headers. Tests
  cover explicit turns, synthesized turns, same-session concurrency, credential
  and conversation isolation, same-value session/thread field fallback, first-state
  reuse, expiry with an in-flight holder,
  and capacity behavior without eviction of fresh state.
- UTC, Singapore and Tokyo are subprocess test configurations, not application
  defaults. All three replace only the current date/timezone context, preserving
  prior history, paths and OS declarations.
- The already-published ARM64 baseline image was run directly and returned
  `UTC +0000`, `/etc/localtime -> /usr/share/zoneinfo/Etc/UTC`, and `Etc/UTC` in
  `/etc/timezone`. Both published architecture configs have no `TZ` environment
  default. No regional timezone was added to the Dockerfile.

These checks establish local transport and serialization behavior. Acceptance of
all tool combinations by the real upstream service has not been exercised.
Synthetic turn continuity is worker-local and resets on restart; identity mapping
changes mean an upgrade uses different upstream conversation/cache IDs.

## Historical model-only validation

The tests at this checkpoint started a real directory master and per-account
app-server children against mock HTTP services. Earlier results below are
historical and do not certify the current capabilities or root-path migration.

- All 11 CPA/master tests pass, including a truly tokenless placeholder followed
  by OAuth and immediate inference, 401 refresh into the original file, concurrent
  inference on one child, token-only reload without losing pending OAuth,
  disabling one of two accounts, and nested Unicode/uppercase-extension IDs.
- Raw-byte transport coverage preserves split UTF-8, comments, event/id lines,
  opaque non-JSON data and the original midstream transport error.
- The expanded run exercised 682 tests across app-server, protocol, API and
  transport. 681 passed including the corrected raw-byte header assertion.
  `codex-api::files::tests::upload_openai_file_reports_blob_transport_diagnostics_without_sas`
  remains failing locally at its existing `message.contains("failed after")`
  assertion. This file-upload implementation and test were not modified.
- CPA's real-process `the then-current OAuth/mode-switch matrix` passed against
  the new binary: tokenless OAuth, immediate inference, 401 refresh, original-file
  persistence, actual logs, native routing after disable, and streaming after
  reenable. The coordinator retained the corresponding historical run log.
- An additional 239 native client, login and TUI notification-routing tests passed.
- Final focused rerun: 12/12 master/bridge and raw-byte transport tests passed.
  Scoped `just fix` completed without warnings across the seven changed crates.
  `just fmt` and the final whitespace check completed afterward; tests were not
  rerun after fix/format, following repository instructions.
- Stable/experimental schema regeneration, entrypoint shellcheck, and workflow
  actionlint passed. No live credentials or billable inference were used.
- The initial debug build exhausted local disk. After it stopped, only this
  checkout's rebuildable Rust incremental cache was removed. Subsequent commands
  use `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`.

The native request boundary deliberately does not create agent turns; ordinary
agent-turn analytics/cost observations are therefore not synthesized. Enabled
runtime components retain their actual configured behavior, not an invented
account heartbeat. A disabled child is killed and reaped rather than gracefully
flushing account exporters. Already transmitted upstream work cannot be recalled.

Validated locally on macOS arm64, 2026-09-27, with Rust 1.95.0. Upstream baseline
`985cf47a4eb6084b2ff6b30ebdb1216acda85bb4`, branch `codex/cpa-managed-auth`.
No real OAuth credentials or billable model requests are used by the new tests.

Build/test environment used `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
CARGO_PROFILE_TEST_DEBUG=0` after the first ordinary build exhausted disk space.
The code-mode companion uses official Codex V8 150.4.0 sandbox artifacts verified
against the repository's pinned release-manifest SHA-256, using
`deploy/cpa-runtime/fetch-v8.py`. No V8 version or upstream pin was changed.

## Historical v2 shared network namespace configuration

The CPA listener uses only `127.0.0.1` inside CPA's shared network namespace,
without a bridge key or Authorization header. Compose directly supplies the fixed
existing credential path, worker ID and port; no `.env` file, binding RPC,
worker-derived filename, rename or copy is introduced. AuthManager, OAuth state,
raw wire logs and the runtime lifecycle are unchanged.

**163/163 tests passed**: all 154 app-server transport tests plus 9 CPA tests,
using a client with no bridge authentication header. The existing `shared.json`
fixture deliberately differs from worker ID `worker`, and OAuth/refresh assertions
read tokens and preserved metadata from that same original file. Log:
`/tmp/cpa-loopback-tests.log`. Entrypoint shell syntax and Actions actionlint pass.
Earlier bearer-related validation below describes historical implementations only.

## Upstream logging extension and image workflow

The logging follow-up preserves the existing inference/auth flow and adds actual
prepared HTTP request, response and failure metadata. Validation covered:

- **326/326 passed**: 313 protocol tests, 4 raw event tests and 9 CPA tests,
  including the byte-tap test (`/tmp/cpa-body-tests.log`).
- Request logs are compared with the requests received by Wiremock, including
  method, target, headers and JSON body. Recovery logs are compared with every
  actual model POST rather than assuming a fixed retry count. Tests verify token
  SHA-256, header masking, response node extraction, and complete HTTP error
  bodies/metadata in notifications and RPC errors. Raw-body tests compare exact
  SSE bytes, including comments/event/id lines and UTF-8 split inside a codepoint,
  and verify that transport errors pass through unchanged.
- The coordinating CPA test `TestCodexRuntimeForkV2OAuthAndModeSwitch` passed with
  the new binary (`/tmp/cpa-runtime-logging-fork.log`): official OAuth, runtime
  inference and 401 recovery, native mode after switch-off, and runtime streaming
  after switch-on all produce records through the existing CPA logging path. The
  final raw-body run also passed with SSE comments/event/id preserved, attempt
  counts matched to real HTTP calls, and tokens masked.
- Stable/experimental schemas and generated Python/TypeScript exports were
  regenerated. Scoped app-server/protocol Clippy passed. Existing usage accounting
  was not changed. A read-only byte tap now supplies complete wire-body logs,
  while the official SSE parser and complete JSON event stream remain unchanged.

The branch image workflow builds `deploy/cpa-runtime/Dockerfile` on native amd64
and arm64 runners, checks `/readyz` for each image, and merges a multiarch manifest.
It publishes
`ghcr.io/power12317/codex-cpa-runtime:cpa-managed-auth` and `sha-<full commit>`, and
uses no QEMU emulation. Actions records the digest and
source SHA. Anonymous manifest access is verified separately from build success.
The early v2 local-only results below remain historical.

## V2 shared-file validation (initial milestone)

The v2 change starts from `8ee538eb3`. Tests use fake credentials, a real local TCP
WebSocket, and Wiremock; no live-account authorization or model billing was used.

- **714/714 passed** with `just test -p codex-login -p codex-app-server-transport
-p codex-app-server-protocol -p codex-app-server -E 'package(codex-login) |
package(codex-app-server-transport) | package(codex-app-server-protocol) |
test(cpa_bridge)'`: 239 login tests, 154 transport tests, 313 protocol tests and
  8 CPA bridge tests. 1,820 nonselected/ignored tests were skipped. Log:
  `/tmp/cpa-v2-tests.log`.
- **4/4 raw event tests passed** with `just test -p codex-api -E
'test(raw_responses)'`. These cover unknown/tool events, terminal fields,
  missing-terminal errors, long idle reads, cancellation and backpressure. Log:
  `/tmp/cpa-v2-raw-tests.log`.
- CPA tests cover exact request/event preservation, single-response inference,
  401 refresh persisted to the shared CPA file, unknown-field preservation, no
  mirror `auth.json`, callback PKCE correspondence, cross-connection pending
  login, invalid state and URL rejection without fetching the callback, Bearer
  authentication, the dedicated route and method whitelist, and owner changes
  cancelling old inference before reloading the shared credential.
- The coordinating CPA thread also ran its real v2 cross-repository test
  `TestCodexRuntimeForkV2OAuthAndModeSwitch` against this macOS binary and reported
  PASS (`/tmp/cpa-v2-fork3.log`): management OAuth start/callback across separate
  sockets, shared-file login, Codex inference/401 refresh, switching to native CPA
  with the refreshed token, then switching back to Codex streaming. No native
  mirror `auth.json` was created.
- Stable and experimental app-server schemas were regenerated. Only the
  precomputed experimental bundle changed. No Cargo dependency or config type
  changed, so Cargo/Bazel locks and config schema did not require updates.
- Scoped `just fix` completed for login, app-server transport, app-server protocol
  and app-server. The one test-helper `unwrap` warning was removed; the final
  app-server Clippy pass completed without warnings. `just fmt` completed. Tests
  preceded these final lint/format passes, following repository instructions.
- Final local `cargo build --locked -p codex-app-server --bin codex-app-server`
  completed after lint/format. Binary: `codex-rs/target/debug/codex-app-server`
  (macOS arm64), SHA-256 `ed8f0f0e59f541f35a32031ab49ae5f8d616975e30dbe3b08ee0ca85d4f34bd8`.
- The deploy entrypoint requires the fixed file, worker ID and bridge key env
  variables and starts the TCP worker automatically. Shell syntax was checked.
  Docker daemon queries did not respond within the bounded check; the Linux
  container image has **not been built or published** in this run.

The v2 implementation deliberately adds no cross-process locks, leases, epochs,
CAS or handoff protocol. An already-running token exchange is not guaranteed to
be mutually exclusive with owner switching. The unknown-field-preserving atomic
replacement is ordinary file persistence. No in-flight request-count API was added.

For review staging, the smallest coherent first stage is the login crate's shared
storage/manual OAuth foundation plus the transport endpoint helper. Those changes
leave the original app-server entry path in place until v2 integration lands. The
second stage connects the v2 RPCs, worker environment and lifecycle with the TCP
mock tests; the deploy entrypoint and contract/validation docs form a final stage.
The complete local change exceeds the usual 800-line target because it includes
all three stages and replaces the v1 integration fixtures.

The prior broader failures below remain historical and unassigned. This run did
not attempt to fix those unrelated areas or claim a full app-server regression.

## V1 results (historical)

- **10/10 CPA and lossless transport tests passed**:
  `just test -p codex-api -p codex-app-server -E 'test(cpa_bridge) | test(raw_responses)'`.
  Coverage includes full request equality, forced tools, unknown extensions and
  events, completed/incomplete/failed usage, long idle reads, bounded queues,
  cancellation/disconnect, account binding, opt-in/signed-out behavior, 503 and
  truncation with no replay, and managed 401 recovery with native token storage.
- **665/665 config and protocol tests passed**:
  `just test -p codex-config -p codex-app-server-protocol` (one ignored test).
- **TUI compatibility**: `just test -p codex-tui` ran 5,571 tests; 5,567 passed.
  Four cursor ANSI assertions failed because this execution environment sets
  `NO_COLOR=1`. All four passed when rerun with `env -u NO_COLOR` (eight other
  tests are skipped by the suite). No unrelated snapshot updates were accepted.
- The first broader API/config/protocol/app-server run executed 2,689 tests:
  2,626 passed, 62 failed, one timed out. After generating schemas and building
  the missing `codex-code-mode-host` and `test_stdio_server` helpers, the 60 failed
  API/app-server cases were rerun: 32 passed, 27 failed, one timed out. The three
  protocol failures passed after schema regeneration. The remaining failures are
  in untouched proxy/file-upload diagnostics, code-mode timing/prewarm and selected
  MCP/plugin capability tests. This is **not a green full app-server regression
  result**; no baseline comparison establishes causality. These remain upgrade/
  deployment validation items. Do not treat the successful CPA path as proof that
  every upstream test passes in this host environment.
- `just write-config-schema`, stable and `--experimental`
  `just write-app-server-schema` completed. The Python SDK generator needs a
  supported Python version; this run used `UV_PYTHON=3.13` because the generator
  does not recognize the host's Python 3.14. Generated SDK changes are included.
- Required scoped `just fix` completed for codex-api, codex-config,
  codex-app-server-protocol, codex-app-server and codex-tui. `just fmt` completed.
  Tests were run before this final lint/format pass, per repository instructions.
- `just bazel-lock-update` completed; the workspace-only dependency addition did
  not change `MODULE.bazel.lock`.
- The real CPA Go executor was exercised against this fork by the coordinating
  CPA task. It reported passing managed refresh/storage, raw tool/unknown events,
  streaming/nonstreaming, exactly one 503 POST, unsupported metadata rejection,
  cancellation closing the HTTP request, and persistent worker survival. Its
  validation record is maintained in the separate CPA repository.

The app-server and code-mode-host binaries are available under
`codex-rs/target/debug/`. The local executable is a macOS arm64 artifact. The
Dockerfile describes a Linux build with sibling runtime components and official
CLI login, but the container image has **not** been built or published here.

## Important fixture and protocol details

Managed fake JWTs need both a user identity and matching workspace identities.
AuthManager deliberately treats incomplete owner identity as an owner change on
credential refresh and revokes the previous network policy. The Rust fixture
therefore supplies `chatgpt_user_id` in addition to matching account claims and
`tokens.account_id`; this is a test fix, not a relaxation of managed policy.

Official unauthorized recovery first reloads native auth storage, then refreshes.
An unchanged reload may send the old credential once more: two rejected 401 POSTs
followed by one accepted POST are possible, with one refresh. Once upstream
accepts a request, no model retry or agent continuation occurs. Transport retry
configuration is zero because the upstream `max_attempts` field counts additional
attempts after the initial request.

## 2026-10-07 official main merge

Merged official `openai/codex` main at
`5b0b2530354052b9194156d70d4c94a439368342` ("Stabilize Guardian snapshot
prefixes by separating assistant context", #51627), incorporating 102 upstream
commits after the previous merge baseline. The fork parent is
`0ce49f5ac55bd644e9a9e4bb5d185c74ab6ea4b8`. Both histories are retained.

The five Git conflicts were resolved after maintainer confirmation. The
app-server README retains the service introduction and the new upstream thread
list documentation. Code Mode retains upstream ranked tool search, including its
feature gate, and the fork's hosted `ImageGeneration` exclusions. Both compressed
protocol export bundles were regenerated from the combined Rust definitions via
`just write-app-server-schema` and its `--experimental` variant. The new tool
search identity match also handles the fork's `ImageGeneration` variant.

Two upstream Guardian compilation issues required small compatibility repairs:
new retained-assistant code now wraps and matches `ContentItem` through the
existing `SectionContent` type, and the cache-prefix integration test uses the
current `prepare_transcript` API. Both affected files were identical to the pinned
official commit before these repairs. Guardian's 47 selected tests, including
both public cache-prefix integration tests, pass with the repairs.

Upstream now represents base instructions as a developer input message for both
Responses and Responses Lite. CPA tests assert the preserved instruction text and
role in that native representation. Eight log-event test calls now explicitly
pass the same 10-second timeout that the previous helper supplied internally.
All 60 fork-added files remain present.

Validation on the macOS arm64 development host with Rust 1.95.0:

- Selected Rust regressions: **1,062 passed in total**. The first nextest run
  passed 1,018 tests; 44 MCP integration tests failed without their supporting
  executables. After building `test_stdio_server` and `codex`, an exact-name retry
  passed all 44 without further source changes. Coverage includes all selected
  CPA tests (52), app-server protocol tests (320), Guardian tests (47), tools
  tests (110), login tests (243), and native client, tool-plan, MCP, SSE and
  prepared-request byte tests. This is not a full workspace test run.
- `cargo build --locked -p codex-rmcp-client --bin test_stdio_server -p codex-cli
--bin codex` passed. The nextest build also compiled the app-server executable
  and all selected test targets.
- Stable and experimental protocol generation passed; stable generation also
  refreshed the Python SDK artifacts. Protocol export consistency tests pass.
- GitHub script tests: **80 passed**, run with the required `packaging` Python
  dependency available.
- Python SDK artifact workflow tests: **80 passed, 1 pre-existing failure** in
  `test_root_format_driver_covers_all_formatter_groups`. The test expects direct
  `rustfmt` commands while `scripts/format.py` emits `cargo fmt`. Both files are
  byte-identical in the fork parent and pinned official commit. Running the exact
  test against copies of the fork parent's files reproduces the same failure.
  The assertion and formatter were not changed as part of this merge.
- Formatting checks pass for all eight manually edited Rust files. The staged
  non-snapshot diff passes `git diff --check`. Whitespace warnings are limited to
  14 upstream TUI snapshots, each byte-identical to the official input.

Local logs are `/tmp/codex-server-sync-rust-tests.log`,
`/tmp/codex-server-sync-rust-retry.log`,
`/tmp/codex-server-sync-test-fixtures-build.log`,
`/tmp/codex-server-sync-github-tests.log`,
`/tmp/codex-server-sync-python-tests.log`, and
`/tmp/codex-server-sync-python-baseline.log`. Linux image validation remains the
responsibility of the existing GitHub Actions image workflow.

## 2026-10-04 official main merge

Fetched the official `https://github.com/openai/codex.git` default branch `main`
at 2026-10-04 20:42:10 +08:00. The pinned merge input is
`afb436df8b70bb5bc57b86d9a3e829968988cd21` ("Honor server reasoning summary
defaults in new TUI threads", #50811), 356 commits after the previous upstream
baseline. The fork parent is `46f9fcc77397db2033bb010c15838684aa8c8719`.
This is an ordinary merge preserving both histories, not a release-tag rebase.

The five conflicts were generated artifacts: the two precomputed export bundles,
two TypeScript notification exports, and the config schema. They were regenerated
from the combined Rust types using `just write-app-server-schema`, its
`--experimental` variant, and `just write-config-schema`. The CPA notification
definitions are identical to the fork parent's definitions. `just bazel-lock-update`
succeeded and left the merged upstream lock unchanged; Bazel reported advisory
dependency-version/annotation warnings.

All 41 fork-added files survived. Aside from this validation record, the only
changed fork-added files are the current upstream provenance in the extension document,
bridge capability constant, and Docker label. No compatibility source rewrite
was required for the upstream model catalog/provider changes, Responses Lite
tool-catalog construction, auth-storage telemetry, or Responses error handling.

Validation on the macOS arm64 development host:

- CPA tests plus all `codex-api`, `codex-app-server-protocol`, and `codex-login`
  tests: **792 passed, 1 failed** out of 793 selected tests (nextest run
  `3f1fc1b2-2e3b-41d9-96c4-b567d3b96251`). All selected CPA, protocol, and login
  tests passed, including manual OAuth, shared-file refresh/disable, readable-home
  migration, source-turn/concurrent/credential isolation, 30 historical tool-call
  pairs, original response bytes, and default-visible redacted logs.
- Native `client::` tests plus model-provider and models-manager unit tests:
  **192/192 passed** (run `195529f7-65d9-47bd-b954-391265c0f3cd`).
- `cargo check --locked -p codex-cli -p codex-code-mode-host --bins` passed.
  The default Deno V8 download returned HTTP 404; rerunning with the existing
  deployment fetcher's checksum-verified Codex V8 150.4.0 archive/binding pair
  succeeded. This uses the same artifact source already configured in Docker,
  without a V8 version change or a new download policy.
- Scoped `just fix` for app-server, core, codex-api, login and app-server-protocol
  completed successfully. It simplified a time-provider coercion while preserving
  its inferred type and removed one unused import in an upstream scenario test;
  neither change alters the CPA path or weakens an assertion.
- `just fmt` completed. The fork delta against the pinned official commit passes
  `git diff --check`. Checking the entire merge also reports trailing padding in
  11 upstream TUI snapshot files; each is byte-identical to upstream and retained
  as rendered snapshot content. The remaining staged files pass the whitespace
  check without exceptions.
- The sole API failure remains
  `files::tests::upload_openai_file_reports_blob_transport_diagnostics_without_sas`
  at `message.contains("failed after")`. An isolated rerun also failed (run
  `79b7bc2a-548a-40fd-bfd2-714274b0b8f3`). The same failure was recorded before
  this merge below, and its source file is unchanged by this upgrade. Its root
  cause remains unresolved; no assertion was removed or relaxed. This is not a
  claim that the complete API suite passed.

The model-only boundary retains the existing TCP/WebSocket methods and port 38317.
Worker home names, legacy migration, source identity/turn state, no-turn-ID
fallback, inference-only behavior and CPA log correlation remain unchanged.
The dedicated `cpa-managed-auth`/immutable-SHA dual-architecture image workflow
is unchanged. No CPA/CPAMP files, deployment settings, extra ports/keys, capacity
policies, refresh locks, or server configurations were introduced or modified.
Linux image execution is delegated to that workflow; this merge does not claim
local Linux container validation or deployment.

## Remaining broader regression failures (historical)

These are the 27 failures and one timeout from the retry run. This table records
observations, not a claim that the baseline necessarily has the same failures.
No product changes were made to suppress them. Raw local logs are in
`/tmp/cpa-regression-retry.log` for this development session.

| Test                                                                                                                                                        | Observed failure / diagnostic                                                                       |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `suite::v2::code_mode_host::app_server_shares_flag_selected_grpc_code_mode_host_across_threads`                                                             | Equality assertion at `code_mode_host.rs:333` failed; root cause not established.                   |
| `suite::v2::account_system_proxy::browser_login_bootstraps_through_system_proxy`                                                                            | Mock browser-login bootstrap received HTTP 502 from the token endpoint; root cause not established. |
| `suite::v2::code_mode_host::code_mode_model_output_uses_structured_host_timing::exec_grpc_host_only`                                                        | Timed out waiting for the `codex.code_mode.host_timing` JSON log event; root cause not established. |
| `suite::v2::code_mode_host::code_mode_model_output_uses_structured_host_timing::exec_grpc_with_overhead`                                                    | Timed out waiting for the `codex.code_mode.host_timing` JSON log event; root cause not established. |
| `suite::v2::executor_mcp::cached_executor_mcp_cannot_read_host_token_after_capability_downgrade`                                                            | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::executor_mcp::guardian_review_does_not_discover_executor_mcp`                                                                                   | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::executor_mcp::legacy_executor_blocks_plugin_host_credentials_and_keeps_host_owned_mcp`                                                          | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::executor_mcp::selected_executor_discovers_browser_mcp_with_executor_only_bearer_token`                                                          | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::executor_mcp::selected_executor_plugin_exposes_its_mcps_only_to_that_thread`                                                                    | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::code_mode_host::code_mode_model_output_uses_structured_host_timing::wait_grpc_host_only`                                                        | Timed out waiting for the `codex.code_mode.host_timing` JSON log event; root cause not established. |
| `files::tests::upload_openai_file_reports_blob_transport_diagnostics_without_sas`                                                                           | Transport diagnostic did not contain the expected `failed after` text; root cause not established.  |
| `suite::v2::selected_capability_stack::managed_plugins_requirement_disables_selected_executor_plugin_capabilities::batched_executor_capability_discovery`   | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::selected_capability_stack::managed_plugins_requirement_disables_selected_executor_plugin_capabilities::direct_selected_root_discovery`          | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::selected_capability_stack::selected_capabilities_become_available_between_samples_in_one_turn::with_a_steered_plugin_mention`                   | Wiremock expected-request verification failed; root cause not established.                          |
| `suite::v2::selected_capability_stack::selected_capabilities_become_available_between_samples_in_one_turn::with_a_steered_server_mention`                   | Wiremock expected-request verification failed; root cause not established.                          |
| `suite::v2::selected_capability_stack::selected_capabilities_become_available_between_samples_in_one_turn::with_an_initial_plugin_mention`                  | Wiremock expected-request verification failed; root cause not established.                          |
| `suite::v2::selected_capability_stack::selected_capabilities_become_available_between_samples_in_one_turn::with_an_initial_plugin_mention_across_a_restart` | Wiremock expected-request verification failed; root cause not established.                          |
| `suite::v2::selected_capability_stack::selected_capabilities_become_available_between_samples_in_one_turn::with_an_initial_server_mention`                  | Wiremock expected-request verification failed; root cause not established.                          |
| `suite::v2::selected_capability_stack::selected_capabilities_become_available_between_samples_in_one_turn::without_a_mention`                               | Wiremock expected-request verification failed; root cause not established.                          |
| `suite::v2::selected_capability_stack::selected_plugin_mcp_startup_respects_explicit_mentions::optional_batched`                                            | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::selected_capability_stack::selected_capability_stack_tracks_environment_availability_and_resume`                                                | Wiremock expected-request verification failed; root cause not established.                          |
| `suite::v2::selected_capability_stack::selected_plugin_mcp_startup_respects_explicit_mentions::optional_direct`                                             | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::selected_capability_stack::selected_plugin_mcp_startup_respects_explicit_mentions::plugin_link_batched`                                         | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::selected_capability_stack::selected_plugin_mcp_startup_respects_explicit_mentions::structured_mention_batched`                                  | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::selected_capability_stack::selected_plugin_mcp_startup_respects_explicit_mentions::plugin_link_direct`                                          | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::selected_capability_stack::selected_plugin_mcp_startup_respects_explicit_mentions::structured_mention_direct`                                   | Missing local `target/debug/codex` test executable; the standalone CLI was not built.               |
| `suite::v2::code_mode_host::code_mode_model_output_uses_structured_host_timing::wait_grpc_with_overhead`                                                    | Timed out waiting for the `codex.code_mode.host_timing` JSON log event; root cause not established. |
| `suite::v2::code_mode_host::app_server_prewarms_flag_selected_grpc_code_mode_host_before_first_turn`                                                        | Test exceeded the 60-second nextest limit; root cause not established.                              |
