# Official runtime identity audit

Date: 2026-10-13 (UTC+09:00). Main baseline:
`138acc222ee9510204f17c0d34bdf78995a4ca1b`. Official reference:
[`rust-v0.162.1`](https://github.com/openai/codex/releases/tag/rust-v0.162.1),
commit `092d3acd6bec3e3a14bdc7e7a2810ab628ab759d`.

## Findings and correction

`codex-app-server` is an official package and binary target, not a fork-specific
replacement name. The official release publishes both CLI and app-server
artifacts. Their version output is intentionally different:

| Entry | Official output | Main output |
| --- | --- | --- |
| `codex --version` | `codex-cli 0.162.1` | `codex-cli 0.162.1` |
| `codex-app-server --version` | `codex-app-server 0.162.1` | `codex-app-server 0.162.1` |

The output includes the same trailing newline. Main's CLI and app-server Cargo
package/bin definitions match the official tag. The CLI entry source, native
initialize processor and native User-Agent implementation are byte-for-byte
unchanged from that reference. The app-server telemetry service name remains
the official `codex-app-server`; telemetry implementation is unchanged.

The Dockerfile builds `-p codex-app-server --bin codex-app-server` and copies that
binary unchanged. Its entrypoint executes `/usr/local/bin/codex-app-server`.
There is no binary rename, CLI impersonation or custom version suffix to remove.
The workspace and deployment stamp both select `0.162.1`. Release preparation
normalizes only local workspace package versions and does not rename packages or
alter third-party dependency versions. Model-only `cpa/*` extensions advertise
actual behavior through capabilities; their metadata does not replace or gate
the official product identity.

One actual mismatch existed in the supervisor: its `initialize` response used
the hand-written `codex-cpa-master/0.162.1`. It now parses the official initialize
client information and uses native originator, suffix and User-Agent APIs. The
official daemon/backend probe exceptions, first-originating-client behavior,
environment override and user-agent sanitization are preserved. The response
uses the official `InitializeResponse` type. There is no official `serverInfo`
field in this protocol revision; the identity fields are `userAgent`,
`codexHome`, `platformFamily` and `platformOs`.

This correction does not change repeated-initialize policy, worker startup,
model execution, credential sharing or the worker's `codex-tui` initialization.
The supervisor still does not initialize account authentication or an Agent.
Project documentation/image package names remain separate from runtime identity.

## Official artifact provenance

The comparison uses downloaded official **aarch64-apple-darwin** binaries and
locally built binaries on the same macOS host. The compressed assets were checked
against the SHA256 digests published by the release API before execution:

| Official asset | SHA256 |
| --- | --- |
| `codex-aarch64-apple-darwin.zst` | `8b9a9ee96c7d765ae721771a845bc31db3dcdfc936933e62a1cfc5ad67bdf9d7` |
| `codex-app-server-aarch64-apple-darwin.zst` | `1133291038c8fb2bd777b5f037d791a410f212cee5fc59f858b3fc776003baca` |

These are real release artifacts, not renamed fork binaries or binaries copied
from a shared Cargo cache. Source-level comparisons use the fixed official Git
commit separately.

## Reproduction

Build main with the pinned toolchain and native prerequisites:

```sh
cd codex-rs
cargo build --locked -p codex-app-server --bin codex-app-server -p codex-cli --bin codex
```

Download and verify matching official CLI/app-server artifacts for your host,
then run from the repository root:

```sh
node deploy/cpa-runtime/check-official-identity.mjs \
  /absolute/path/to/official/codex \
  /absolute/path/to/official/codex-app-server \
  "$PWD/codex-rs/target/debug/codex" \
  "$PWD/codex-rs/target/debug/codex-app-server"

CODEX_TEST_BINARY="$PWD/codex-rs/target/debug/codex-app-server" \
  node deploy/cpa-runtime/check-identity.mjs
```

The first script compares exact version output and full initialize responses on
the same host and state path. It checks official CLI `app-server`, fork CLI
`app-server`, fork standalone app-server and fork master against the official
standalone app-server. Each process receives daemon, originating-client, backend,
subsequent-client and invalid-header fixtures, both with and without the native
originator environment override. No response field is normalized or removed.
Temporary credentials/state are isolated and model endpoints are loopback-only.

The second script exercises the real master and credential worker against a mock
upstream with synthetic credentials. It checks actual version output, supervisor
initialize identity and the worker's transmitted `version`, `originator` and
User-Agent headers. The same check already runs in the image workflow.

## Validation results

- Both locally built executable version outputs match the official artifacts
  byte-for-byte, including the trailing newline and empty stderr.
- All **8 handshake sequence comparisons passed**: four entries against the
  official standalone server, under two originator configurations. Each sequence
  has five client cases; complete success/error response objects match exactly.
- Actual worker HTTP identity passed: `originator: codex-tui`, `version: 0.162.1`.
  Its entire User-Agent equals the official initialized User-Agent byte-for-byte:

  ```text
  codex-tui/0.162.1 (Mac OS 15.3.1; arm64) dumb (codex-tui; 0.162.1)
  ```

- All **70 CPA regressions passed** (15 library tests and 55 integration tests),
  including credential lifecycle, large-message IPC, cancellation, identity,
  Responses/Images and timezone replacement.
- Both main binaries built successfully. Strict app-server library/binary Clippy,
  Rust formatting, JavaScript syntax and diff whitespace checks passed.
- All **5 release preparation tests passed**.

Local evidence, official digest verification and compiled artifacts are under
`/private/tmp/codex-main-identity-20261013/`, including `source-audit.json`,
`official-comparison.json`, `wire-identity.json`, `cpa-tests.log`, `clippy.log`
and `build.log`. The artifact hashes are recorded with the delivery result.

This is a local main correction. It does not push main, create release tags,
publish images or modify the native-runtime/gateway worktrees. Host comparison
does not substitute for a future Linux container release check.
