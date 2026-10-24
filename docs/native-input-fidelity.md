# Native model input fidelity

The model-only adapter translates protocol envelopes into native input types. It
does not maintain a second list of business fields that are allowed to survive.
The native request builder, authentication, transport, continuation, and recovery
remain the owners of model execution. Client tools are declarations, not local
executors.

## Input and declaration ownership

- History enters `ResponseItem` without removing `status` by item type. Native
  optional states remain optional; native required states remain required. A
  supplied state is never replaced with `completed`.
- Message content enters the native content types without pre-cleaning
  annotations or logprobs. A malformed native field produces its native parsing
  error instead of being deleted to make parsing succeed.
- Function, custom, namespace, search, and image declarations deserialize into
  native `ToolSpec` types. Complete caller-owned input/output schemas use native
  schema carriers. Image options remain in the native flattened options object.
  Custom formats and tool choices retain their native shapes.
- Reasoning, text, stream, and generation controls deserialize native DTOs. There
  is no independent business-key admission table. The mainline branch retains its
  native generation extensions; the native runtime follows its typed DTOs.
- The native runtime's direct Images bridge deserializes native generation/edit
  requests and options. It does not project an options allowlist into a new
  request.

Protocol conversions are narrow: message/text shorthand, the existing compaction
alias, Chat function wrappers, `input_schema`, token-limit aliases, native
optional defaults, and unambiguous input-local tool-result association. Chat
`stream_options.include_usage` is a downstream delivery preference. Worker-owned
authentication, installation, session/thread/window provenance, and transport
headers remain separate from business controls. The mainline extension collector
consumes the request envelope by structure and excludes worker-owned provenance,
instead of rejecting an unfamiliar business key.

Necessary structural and operation checks remain: object/array shape, native
enum parsing, duplicate declaration identities, referenced tool identities,
ambiguous or contradictory call/result associations, credential ownership, and
operation-specific requirements. They do not test whether a function name is in
the Codex built-in catalog. Unsupported stored-conversation or asynchronous
operations retain their existing handling. No final wire-body patch, alternative
sender, new runtime switch, retry, or synthetic completion was added.

## Coverage follows native types

`codex-rs/core/src/test_support/inference_fixtures.rs` constructs every native
history variant, content variant, and tool declaration with explicit fields. It
also exhaustively matches the native enums and destructures the native request.
New native variants or fields therefore require fixture review at compilation,
before they can become silent adapter omissions. Reasoning, text, stream, and
generation DTO constructors cover their complete field sets too.

The adapter tests compare native fixture serialization with adapted values.
These comparisons are test assertions, never request admission checks. The
history corpus includes web search `completed`/`in_progress`/`failed`, every native
shell status, image calls, tool-search output, all native business item types,
multimodal content, and legal missing optional status. String states are fixtures
for the native String fields, not a claim that every upstream deployment accepts
every state.

Real master/worker tests compare the HTTP mock's received history and declarations
in Responses and Lite. Expected image detail changes come from the native prompt
formatter. They preserve the stream and caller execution boundary.

To export input arrays and independently generated fallback-model expectations:

```sh
CODEX_INFERENCE_FIXTURE_DIR=/absolute/output/directory \
  just test -p codex-core --lib \
  -E 'test(client::inference::input::tests) | test(client::inference_tools::tests)'
```

`native-history.full.json` is the native field corpus, including native harness
metadata. `native-history.json` omits that attribution metadata for external
HTTP replay. `native-tools.json`, `native-controls.json`, and
`source-metadata.json` keep declarations, controls, and caller provenance separate.
The `native-history.expected-gpt-6.1-sol-{responses,lite}.json` arrays use the
native formatter for the local fallback model. Model capability snapshots are
exported alongside them.

Native normalization remains in force: Lite removes image detail; unsupported
`original` detail becomes `high`; unsupported reasoning configuration updates
are filtered by native model policy; reasoning content and host-only attribution
follow native serialization. These are not adapter schema-key filters.

## Audit boundary

The audit covers `client/inference*`, tool schema/declaration types, request DTOs,
and app-server request/Images/identity/delivery bridges. Remaining explicit field
handling concerns protocol conversions, worker-owned provenance, response-header
transport handling, scoped inverse identity mapping, diagnostics, or resource
lifetime. Those paths do not classify arbitrary business fields as disposable.
Native request formatting, sender, recovery, zero-retry settings, refresh
ownership, and official identity code are unchanged.
