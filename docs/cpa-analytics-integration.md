# CPA inference analytics integration

This document records the integration on `codex/native-model-runtime`, starting
from fork commit `6252da7200f479ff71d297c75d94c323c92471c3`. Its upstream baseline
is `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d` (release `0.162.1`). It is also a
porting guide for branches that retain the native inference observation hooks.

## Design boundary

Use the existing `AnalyticsEventsClient` instance, queue, reducer, event DTOs,
authentication, routing, and HTTP sender. The adapter never executes a tool or
sends a model request. It never builds an analytics HTTP request itself.

`track_request`, `track_response`, and `track_notification` are collection
methods, **not** three event types accepted by the analytics endpoint. The first
two accept selected native app-server RPC types. Fabricating `turn/start` or
`thread/start` objects merely to pass those filters incorrectly represents a
local RPC lifecycle. This implementation instead adds a small observed-context
fact to the existing reducer, then uses its existing turn configuration, usage,
profile, error, and notification methods.

There is one new production adapter, `app-server/src/cpa_bridge/analytics.rs`.
The wire extraction, tool conversion, and recording helpers live in that file.
The separate `analytics_tests.rs` contains tests only. No public CPA protocol or
generated schema changes are required.

## Data flow and event mapping

1. `MessageProcessor` passes its existing analytics client into `CpaBridge`.
2. Each admitted exchange gets the actual app-server connection ID. No synthetic
   connection, client name, version, transport, or initialize RPC is created.
3. `NativeObservation::request` observes the final HTTP body or WebSocket
   `response.create` frame. The adapter supports `/responses` and
   `/responses/lite`, including bounded zstd decoding.
4. `accepted_event` and `failed_event` observe original upstream events before
   CPA response identity restoration. `run` also passes native execution errors
   to the existing `TurnCodexErrorFact::from_codex_err` constructor.
5. The adapter aggregates observations and waits before recording facts. The
   ordinary reducer produces `codex_thread_initialized`, `codex_turn_event`, and
   applicable existing tool events. The ordinary sender performs delivery.

| CPA-facing lifecycle | Analytics input derived from the native observation |
| --- | --- |
| Inference start | Bind final transmitted thread/session/turn/model; retain actual connection ID |
| Inference body/event | Observe original response IDs, usage, tool items and errors; convert completed tool observations to existing `ItemStarted` / `ItemCompleted` notifications |
| Inference completed | After the delay, record configuration, token usage, profile and existing `TurnCompleted` notification |
| Inference error/cancel | Preserve native error classification or cancellation; emit a failed/interrupted completion after the delay |

The native business notification is `CpaInferenceEvent`; the stable HTTP bridge
uses `CpaInferenceBody`. Capabilities identify the active channel. Neither raw
notification is an analytics payload.
Validation failures before any upstream request have no final upstream identity;
the adapter does not manufacture one from the original CPA request.

## Aggregation and approved completion estimates

The aggregation key is the managed account, managed user, final upstream thread
ID, and final upstream turn ID. Multiple requests with that key contribute to
one turn. Completed observed turns are recorded serially into the shared native
queue so that caller-owned threads reusing the same turn ID do not overwrite
each other in the native reducer. Transport retries reuse their exchange; they increase attempt and retry
counts without creating a phantom active request. Usage is counted once per
response ID, or once per exchange for a response with no ID.

When the last active exchange finishes, schedule a random delay in the inclusive
range 30–233 seconds, anchored to the last upstream terminal observation. A continuation observed before the timer fires cancels the
old timer and schedules completion after its own final exchange. Business stream
delivery never awaits this timer or analytics HTTP delivery. Recording occurs
only after the timer actually expires. Scheduling overhead can increase the
measured interval slightly.

The reported turn duration is the elapsed time from its first final-wire send
through delayed completion. Measured sampling time is retained. The delayed tail
is `after_last_sampling_ms`. Unknown startup, compaction and tool-blocking phases
use zero; measured time between model sends is assigned to between-sampling
overhead. These are explicitly chosen completion estimates, not measurements of
execution on the CPA client.

There is no caller-side end-of-turn signal in this extension. A completed turn ID
is retained
as a tombstone to prevent duplicate reporting; a continuation arriving **after**
the delayed turn has already been emitted is not retroactively appended. This is
the boundary of the inactivity-based aggregation algorithm.

Missing approval observations use the existing `config_allowed` outcome, with
zero user and Guardian review counts. They do not create fictitious review events.
Missing command exit codes default to **0**, the process-success code; HTTP 200
is unrelated. Explicit nonzero codes and observed failures take precedence.
Structured tool results and native command headers (`Exit code:` or `Process
exited with code`) are recognized; arbitrary stdout following `Output:` is not
parsed as metadata. An observed `duration_ms` takes precedence; otherwise the tool
duration extends from its first observation to delayed completion.

## Sources for the native turn schema

All fields continue to be serialized by the upstream `CodexTurnEventParams` DTO.
Its existing omission rules remain intact. Fields that the upstream DTO normally
serializes as `null` still serialize as `null`; the adapter adds no JSON keys to
force nulls and does not globally remove native nulls.

| Field or field group | Source / behavior |
| --- | --- |
| `thread_id`, `session_id`, `turn_id` | Final transmitted `client_metadata`, including decoded native turn metadata; required to bind an observation |
| `root_turn_id`, `turn_trigger`, `codex_turn_source` | Final transmitted turn metadata; unknown values retain the native optional representation |
| `thread_source`, `subagent_source`, `parent_thread_id` | Final transmitted lineage metadata; `subagent_kind` supplies the subagent source |
| `app_server_client` | Actual initialized connection; product originator is the final upstream originator, with the native runtime originator used for headerless WS frames |
| `runtime` | Existing native runtime metadata collector, unchanged |
| `initialization_mode`, `ephemeral` | New ephemeral observed inference context; first observation creates one native thread-initialized event |
| `active_plugin_ids_at_turn_start`, `voice_session_id`, `submission_type` | Unknown; upstream optional defaults, with no fictitious plugin scan or voice/RPC lifecycle |
| `model` | Last final-wire model for the logical turn |
| `model_provider` | `openai`; the bridge already requires the managed OpenAI provider |
| `reasoning_effort`, `reasoning_summary`, `service_tier` | Final body, converted using the existing native types and service-tier mapping |
| `sandbox_policy`, `sandbox_network_access` | Final `sandbox_mode` when recognized; otherwise the worker's effective permission profile |
| `approval_policy`, `approvals_reviewer` | Approved fallback `never` / native user reviewer representation; tool outcomes use `config_allowed` without creating reviews |
| `guardian_v2_enabled` | Final `auto_review_enabled`, otherwise false |
| `multi_agent_version`, `collaboration_mode`, `personality` | Native V1/default mode and absent personality; these controls are not observed in the final model payload |
| `workspace_kind` | Final metadata if present |
| `num_input_images` | Image parts in the last user message of the first final request in the turn |
| `image_preparations` | Native empty collection; no local image-preparation observations are invented |
| `is_first_turn` | First observed turn within the retained upstream thread context |
| `status`, `turn_error` | Actual upstream terminal event / native execution failure / worker cancellation |
| `codex_error_kind`, `codex_error_http_status_code`, `usage_limit_window_minutes` | Existing native `TurnCodexErrorFact`, when an execution error is available |
| `explicit_client_interrupt_requested_at_ms`, `steer_count` | Native defaults; CPA cancellation is not represented as a successful native `turn/interrupt` RPC, and inference continuation is not a `turn/steer` RPC |
| `total_tool_call_count`, `shell_command_count`, `file_change_count`, `mcp_tool_call_count`, `dynamic_tool_call_count`, `subagent_tool_call_count`, `web_search_count`, `image_generation_count` | Existing reducer counts from normalized original upstream tool items |
| `input_tokens`, `cached_input_tokens`, `cache_write_input_tokens`, `output_tokens`, `reasoning_output_tokens`, `total_tokens` | Deduplicated upstream response usage, accumulated with saturating arithmetic |
| `before_first_sampling_ms`, `compaction_ms`, `tool_blocking_ms` | Zero when not observable |
| `sampling_ms` | Measured transport-attempt intervals, bounded by the observed pre-completion span |
| `between_sampling_overhead_ms` | Pre-completion elapsed time minus measured sampling time |
| `after_last_sampling_ms` | Actual elapsed time between the last upstream terminal observation and delayed completion |
| `sampling_request_count`, `sampling_retry_count` | Actual transmitted attempts and attempts minus independently started exchanges |
| `tools_change_count` | Changes in the serialized final tool-definition signature between requests |
| `duration_ms`, `started_at`, `completed_at` | Observed start and actual delayed completion clock; existing timestamp units are preserved |

Tool conversion recognizes shell/unified-exec, apply-patch, collaboration, hosted
web search, image generation, MCP and generic external tools. Apply-patch path and
operation information is parsed using the existing native parser without applying
the patch or reading the workspace. No applied diff is available, so it does not
invent accepted-line fingerprints. Generic external tools use the existing dynamic
tool item type. This mapping does not run a local agent loop.

## Account isolation and resource behavior

CPA workers already use one managed credential. Their existing analytics
destination now retains the account/user identity captured when the client is
created. The existing sender checks that binding against its current auth snapshot
before sending. Token refresh for the same identity remains permitted. Delayed
observations also check identity before enqueueing.

The adapter retains at most 256 thread contexts, 1,024 turns including completed
tombstones, and 1,024 distinct tool items per turn. Tombstones expire after one
hour; the oldest tombstones may be evicted sooner to admit new turns. Deduplication
is bounded by that retention window. Existing active turns can
continue at the turn limit. Empty idle thread contexts expire after one hour;
active contexts are not evicted by that idle test. Analytics retains the native
best-effort queue/delivery semantics, including no durable outbox across worker
exit. Tool facts are enqueued in chunks with existing flush barriers to avoid
filling the queue with a large single-turn burst.

## Porting steps, in dependency order

1. Confirm the target branch's native version, client identity and analytics DTOs.
   Do not transplant a release number, originator, event name or generated schema
   from this branch into a different upstream release.
2. Preserve or port the final-wire `TransportObserver` and original-event
   `ResponsesObserver` hooks. Do not observe `delivery.rs`, whose identities may
   already have been restored for CPA.
3. In `analytics/src/facts.rs` and `lib.rs`, add/export
   `ObservedInferenceContext` and add the internal observed-context/config-allowed
   fact variants. These are internal facts, not new wire event types.
4. In `analytics/src/client.rs`, expose the two thin fact-recording methods and
   add the CPA account binding to the existing destination/sender. Preserve
   native client behavior outside CPA credential mode.
5. In `analytics/src/reducer.rs`, ingest observed context using the **existing
   initialized connection**; register native thread/turn state and use the native
   thread-initialized DTO. Apply config-allowed only when no review outcome exists.
6. Add the single `app-server/src/cpa_bridge/analytics.rs` adapter. Add workspace
   dependencies `rand`, `zstd`, and `codex-apply-patch` to app-server, then let Cargo
   update the lockfile. The patch parser is read-only in this integration.
7. Pass the existing client from `message_processor.rs` to `CpaBridge::new`.
   Attach one exchange observer in `CpaBridge::start` using the real connection ID.
   Forward native execution errors before conversion to the bridge error DTO.
8. In `cpa_bridge/upstream.rs`, forward final requests, original accepted/failed
   events, and final execution outcomes. Leave business scheduling, native retry
   behavior, response identity restoration and tool execution ownership intact.
9. Port `analytics_tests.rs`, the account-binding test in existing
   `analytics/src/client_tests.rs`, and the end-to-end case in the existing
   `native_runtime_tests.rs`. The integration-test launcher fixes the debug delay
   at 30 seconds. The override is ignored by release builds and cannot select a
   value outside 30–233 seconds.
10. Run the checks below before adopting the change. No generated protocol
    artifacts need regeneration because the public protocol did not change.

## Verification

From `codex-rs`, using the repository's pinned Rust toolchain:

```sh
cargo test -p codex-analytics
cargo test -p codex-app-server --lib --test all cpa_
cargo fmt --all --check
```

From the repository root, with the binary produced by the tested build:

```sh
CODEX_TEST_BINARY=/absolute/path/to/codex-app-server node deploy/cpa-runtime/check-identity.mjs
python3 deploy/cpa-runtime/test_prepare_release.py
```

The Python check requires Python 3.11 or newer. Unit tests use virtual Tokio time
for the actual delay gate. The end-to-end test uses real elapsed time and a local
mock upstream, verifies that business delivery precedes analytics delivery, checks
the final transformed identity/model and worker auth headers, and asserts exactly
one model request. No live OpenAI account is needed for these tests.

Validation recorded on 2026-10-16 with Rust 1.95.0:

| Check | Result |
| --- | --- |
| `cargo test -p codex-analytics` | 125 passed |
| `cargo test -p codex-app-server --lib --test all cpa_ -- --test-threads=1` | 34 library tests and 43 integration tests passed |
| `cargo fmt --all --check` | Passed; stable rustfmt prints the repository's existing warning about the nightly-only `imports_granularity` option |
| `check-identity.mjs` against the tested app-server binary | Passed |
| `test_prepare_release.py` using Python 3.14 | 5 passed |
| `git diff --check` | Passed |

One parallel regression run hit `Address already in use` while starting an Images
test worker, before it could issue its expected model request. The complete suite
was rerun serially and passed, including that case. No production port allocation
or Images retry behavior was changed to accommodate the test.

## Coverage limits

This adapter covers native Responses and Responses Lite over HTTP and WebSocket.
The standalone `/images/generations` and `/images/edits` path does not carry the
same required thread/session metadata or `response.completed` lifecycle. It remains
outside this adapter; reporting it would require a separate, explicitly specified
identity/timing mapping. Image-generation tool calls within Responses are covered.

The bridge cannot observe local client execution, user interactions, plugin
inventory, applied workspace changes, or durable conversation history. The
completion/approval/exit-code fallbacks above are the approved estimates, while
other unavailable optional fields retain native defaults. Queue pressure, worker
shutdown and account changes may drop telemetry under the existing best-effort
delivery policy. A delayed event is not a record that a command actually ran on
codex-server.
