# Official engine identity and managed Desktop compatibility

The native-runtime branch synchronizes official `openai/codex` main at
`98f79517ab2392879c581626ec0588851b03d183` and retains the fork's model-only
master/worker extensions. Its release version is selected independently from the
latest official stable tag, `rust-v0.166.0`, commit
`6ff1e1ca3b30fc8b914720681fcf5fe62e95f856`. The fork source includes unreleased
main changes and fork extensions; it is not represented as byte-identical to the
stable release source.

## Engine names and versions

| Surface | Value | Source |
| --- | --- | --- |
| Committed workspace and local lock packages | `0.0.0` | Development placeholders remain committed. |
| Build-copy engine version | `0.166.0` | `deploy/cpa-runtime/release-version`, applied by `prepare-release.py`. |
| Cargo package and binary target | `codex-app-server` | The retained official app-server target. |
| Installed executable | `/usr/local/bin/codex-app-server` | The service image copies that target. |
| Version output | `codex-app-server 0.166.0\n` | Actual stamped Cargo build metadata. |

Do not rename the app-server target to `codex`, add a fork suffix, or substitute a
Desktop application version for its engine version. The official `codex-cli`
package still builds the separate `codex` target. Official release packages now
also contain a standalone executor; the model-only service image continues to
package the app-server target and does not execute client tools.

The existing native image remains
`ghcr.io/power12317/codex-server-native-runtime`, with `latest` and the existing
`sha-<fork-commit>` tag. The main-branch image remains separate. Synchronizing
engine identity does not rename the fork repository, image package, or workflow.

## Managed Desktop profiles

Only a CPA credential worker installs a process-local identity, before auth,
configuration and network clients are constructed. `codex_client_system`, read
from its existing shared credential JSON, selects Windows case-insensitively.
Missing, `mac`, and legacy values select the fixed Mac x64 profile. Neither the
client's headers nor the physical server's ARM architecture selects the profile.

The Desktop host version is pinned separately in
`deploy/cpa-runtime/desktop-version` to **26.1027.30824**, independently verified
from official platform release metadata on 2026-10-30:

- [Windows Store update metadata](https://persistent.oaistatic.com/codex-app-prod/windows-store-update.json):
  `releaseVersion=26.1027.30824`, `packageIdentity=OpenAI.Codex`,
  `storeProductId=9PLM9XGG6VKS`. Its `buildVersion=26.1027.3384.0` is the Windows
  package build and is not the host release version used in clientInfo or UA.
- [Mac x64 appcast](https://persistent.oaistatic.com/codex-app-prod/appcast-x64.xml):
  first item `shortVersionString=26.1027.30824`, Sparkle build `22943`, minimum
  system version `13.0`, published `Thu, 29 Oct 2026 18:03:41 +0000`, linking
  `ChatGPT-darwin-x64-26.1027.30824.zip`. No Desktop archive is required at runtime.

The managed compatibility profiles use these exact UA shapes:

```text
Codex Desktop/0.166.0 (Windows 10.0.19044; x86_64) dumb (Codex Desktop; 26.1027.30824)
Codex Desktop/0.166.0 (Mac OS 15.3.1; x86_64) dumb (Codex Desktop; 26.1027.30824)
```

The engine segment comes from the actual stamped `CARGO_PKG_VERSION`; source
placeholder tests therefore honestly use `0.0.0`. Originator and the UA
prefix/suffix use `Codex Desktop`. `dumb` is the headless terminal token. The
fixed Windows build and Mac OS 15.3.1/x86_64 values are compatibility parameters,
not new measurements or an inference from a deployment machine. The official x64
reference can run under existing Rosetta on an Apple Silicon validation host;
that does not make the host a physical Intel Mac.

The master initializes workers with Desktop clientInfo and the separate host
version. Managed root inference uses the native app-server's `VSCode` session
source; subagent source selection remains distinct. The master, standalone
app-server and unmanaged CLI retain their official initialize/originator/UA
behavior. Managed initialize does not request an IPC attestation callback or
activate local device verification.

## Identity header ownership

Managed bootstrap also installs immutable provider identity headers. Every API
provider resolves its static `http_headers` and `env_http_headers` first, then
protects `User-Agent`, `originator` and the actual engine `version` and removes
configured `x-oai-attestation`. Header names are case-insensitive. Other headers
retain their existing static/environment values and precedence. New provider
instances and model-catalog originator scoping use the same construction path.

This prevents an old worker configuration from restoring CLI identity. Native
inference send sites remain the only source of request attestation; the general
default HTTP headers do not contain proof. Model catalog, usage and OAuth
requests must not inherit a configured proof. Each native client's original
header shape is retained; a UA-only backend client does not acquire unrelated
provider headers.

No final request-body rewrite, alternative sender, new runtime switch, transport
fallback or retry state machine is introduced by the Desktop compatibility layer.
The native HTTP/WS flow, authentication owner, continuation, client tool execution
boundary and diagnostics/business separation remain in force.

## Platform-unavailable attestation status

The managed `InferenceSession` supplies the original `AttestationProvider`
interface. Existing native HTTP and WebSocket send sites request a fresh status.
The outer JSON field order is `{"v":1,"s":0,"t":"v1.<base64url CBOR>"}`.
The inner CBOR contains only `error_code`, `bundle_id="com.openai.codex"` and
binary `f`; Windows uses error 1, fixed Mac x64 error 2. There is no successful
Apple token, latency field or native DeviceCheck addon.

The nested binary fingerprint contains schema version, languages, locale,
service timezone, screen-size sum, scale and a daily UUID. Account/system seeds
exclude credential filenames, paths and access/refresh tokens. Display selection
is stable and the UUID rotates at midnight in the effective service timezone,
following the codex2api PR827 strategy rather than the official Desktop's
process-random UUID. A valid `TZ` is preferred, then a system IANA zone, then
`Etc/UTC`. Common service zones have fixed compatibility locales, with `en-US`
fallback. Files missing both account ID and email share an explicit legacy seed
bucket. Metadata is fixed for the existing worker lifetime; changing account or
platform metadata requires its normal process restart.

These are synthetic compatibility signals and an unavailable-platform status,
not hardware measurements or successful Apple attestation. Native compaction
input (`compaction_trigger`) uses the same Responses sender; the retired
`responses/compact` RPC is not restored.

Pure-codec golden vectors in `login/src/cpa_desktop_golden.json` retain their
actual provenance: synthetic signals encoded using the extracted official
Desktop 26.1023.51421 `Qi`/`Jne` functions, without loading a native module. Updating
the advertised host release does not relabel those historical vectors as a new
application extraction. The format and platform status remain the approved v1
compatibility scheme.

## Official reference artifacts and checks

The official 0.166.0 release API reports these archive SHA256 values, verified
before the reference executables are run:

| Asset | SHA256 |
| --- | --- |
| `codex-app-server-aarch64-apple-darwin.tar.gz` | `af848c48b91253faa5a693a71b30d963109efc2674b398aedd0ca29d13089ca4` |
| `codex-app-server-x86_64-apple-darwin.tar.gz` | `2d2deadd2318b5eeb10c506962e7ec95560e41c635e5c2add120bc5bb5292921` |

Reference hashes are provenance, never runtime capability gates. Compare actual
version output, entry point, target architecture, configuration and caller
sequence. Source identity construction is retained from official
`app-server/src/request_processors/initialize_processor.rs` and
`login/src/auth/default_client.rs`.

```sh
CODEX_TEST_BINARY=/absolute/path/to/codex-app-server \
  node deploy/cpa-runtime/check-identity.mjs
node deploy/cpa-runtime/check-official-identity.mjs \
  /absolute/path/to/official-codex-app-server \
  /absolute/path/to/candidate-codex-app-server
```

The managed probe launches a real master and Windows/Mac workers against
loopback with synthetic credentials. It includes stale static/environment
identity and proof headers, checks exact native output and preserves a custom
header. The unmanaged comparison checks full official initialize sequences
without normalizing response fields. Its optional four-argument mode requires
actual official and candidate CLI artifacts; two-argument service validation
does not claim a candidate CLI build.

Service regressions exercise actual HTTP/WS headers, connection reuse, native
compaction input, failure boundaries, catalog/usage clients and process isolation.
They do not establish real upstream acceptance or DeviceCheck success. Historical
0.164.0 results remain separate and must not be reused as 0.166.0 evidence.
