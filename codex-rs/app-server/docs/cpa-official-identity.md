# Official release identity

The fixed official release is
[`rust-v0.164.0`](https://github.com/openai/codex/releases/tag/rust-v0.164.0),
commit `6aaac694803004a63c6f393d153c96d22521a6b9`. Both service branches incorporate
that release's source, including its native client and transport API changes.
This is a fork build with model-only extensions, not an unmodified official binary.

## Source placeholders and published identity

Committed Cargo workspace and local package versions remain `0.0.0`.
`deploy/cpa-runtime/release-version` selects `0.164.0`; `prepare-release.py` stamps
only the build copy before compilation, including implicit workspace packages.
It preserves binary/package names and third-party dependency versions. The image
build performs the same preparation. Protocol capabilities never gate on this
product version or on the source revision.

| Entry point                  | Official output for this release | Service packaging                                                |
| ---------------------------- | -------------------------------- | ---------------------------------------------------------------- |
| `codex --version`            | `codex-cli 0.164.0`              | Retained upstream CLI target; not packaged in the service image. |
| `codex-app-server --version` | `codex-app-server 0.164.0`       | This exact target is built and copied into the service image.    |

The version output includes the original trailing newline. The executable remains
`/usr/local/bin/codex-app-server`, without a fork suffix. The engine
telemetry version is retained. The unmanaged originator/User-Agent construction
follows upstream; managed workers use the Desktop profile below. The master uses the
same native initialize identity helpers as the official app-server; it does not
invent a supervisor product identity.

Managed CPA workers initialize as `Codex Desktop`. Their Desktop host version is
pinned separately in `deploy/cpa-runtime/desktop-version` to the verified installed
ChatGPT.app version `26.1023.51421`; the engine version remains `0.164.0` from the
actual stamped Cargo build. There is no update lookup at request time.

Official platform release metadata independently confirms this Desktop release:
the [Windows Store update metadata](https://persistent.oaistatic.com/codex-app-prod/windows-store-update.json)
reports `releaseVersion: 26.1023.51421` for `packageIdentity: OpenAI.Codex`.
Its `buildVersion: 26.1023.6621.0` is the Windows package build, not the Desktop
host release version used in clientInfo or UA. The first item in the
[Mac x64 appcast](https://persistent.oaistatic.com/codex-app-prod/appcast-x64.xml)
reports `shortVersionString: 26.1023.51421`, Sparkle build `22530` and minimum
system version `13.0`, and links the official `ChatGPT-darwin-x64-26.1023.51421.zip`.
This corroborates the host version for both compatibility profiles independently
of the installed ARM application; it does not change the fixed OS parameters.

With a shared credential's `codex_client_system=windows`, the fixed compatibility
profile sends:

```text
Codex Desktop/0.164.0 (Windows 10.0.19044; x86_64) dumb (Codex Desktop; 26.1023.51421)
```

Missing, `mac`, or legacy values use the verified x64 official app-server format:

```text
Codex Desktop/0.164.0 (Mac OS 15.3.1; x86_64) dumb (Codex Desktop; 26.1023.51421)
```

The Mac string was obtained by initializing the official 0.164.0 **x86_64**
app-server with Desktop clientInfo, using the existing Rosetta runtime on an Apple
Silicon host. It is not a claim that the worker or validation host is a physical
Intel Mac. Windows OS/build values are explicit compatibility parameters, not a
captured Windows installation. Both use the official headless terminal token
`dumb`; neither inherits a CLI/iTerm terminal environment from the supervisor.

Only the managed credential worker installs this identity before auth clients are
created. The master, standalone app-server, and original non-managed CLI retain
the official mutable initialize/originator/User-Agent behavior. Version output,
Cargo target names, release stamping, credentials, routing and installation ID
storage are unchanged. Subagent session sources remain distinct; managed root
inference uses the app-server's `VSCode` source instead of `Cli`.

The managed inference attestation provider assembles a platform-unavailable
status using the official CBOR/outer JSON format. Windows has inner `error_code=1`;
fixed Mac x64 has `error_code=2`. Both use outer `v=1,s=0` and a `v1.` base64url
CBOR value. No DeviceCheck addon is loaded and no client-supplied attestation is
used. The original native HTTP/WS send sites request these headers; general
OAuth, model and usage default headers contain Desktop identity but no proof.
Managed provider construction also protects User-Agent, originator and the real
engine version after static and environment headers are resolved, and removes
configured attestation so only native inference send sites can add it. Other
provider headers keep their existing values and precedence. This applies to new
provider instances too, including catalog refreshes after configuration reload.
The supported native compaction input (`compaction_trigger`) follows the same
Responses sender; the retired `responses/compact` RPC is not enabled.

Account/system seeds keep the selected display stable across credential filename
changes and token refresh. The fingerprint's session UUID rotates at midnight in
the effective service timezone, following codex2api PR827's compatibility strategy;
this differs from the official Desktop process-random session UUID. Locale
profiles cover common service zones with `en-US` fallback. Timezone selection
uses a valid `TZ`, then the system timezone, then `Etc/UTC` when neither supplies
an IANA zone. These are synthetic compatibility signals, not
hardware measurements or a successful Apple attestation. Historical files without
both account ID and email share a documented legacy seed bucket until CPA supplies
stable identity metadata; no new persistence or credential rewrite is introduced.

## Official artifact provenance

The reference executables are downloaded official release assets for the
matching architecture. Their compressed bytes are checked against the release API's SHA256 values
before execution:

| Official asset                              | SHA256                                                             |
| ------------------------------------------- | ------------------------------------------------------------------ |
| `codex-aarch64-apple-darwin.tar.gz`            | `aa8299c169d80111aac05543ad2621c9bdbedb9bc252c905fe3f5a5baf9a4142` |
| `codex-app-server-aarch64-apple-darwin.tar.gz` | `2eb75238651acb7d5c7cbb088b56433c71bbb680189c3bbd50318589858d57b1` |
| `codex-app-server-x86_64-apple-darwin.tar.gz` | `8bcb46d2eb100ddccd81446339496e35c4f9640639339691d19dda551bb67a3f` |

The artifact hashes are reproduction evidence, never capability or runtime gates.
Both reference executables' actual version outputs were inspected. The service
comparison uses the official standalone app-server, matching the packaged target.

## Reproduction

Prepare and build the service in a build checkout, keeping source placeholders in
the committed repository:

```sh
python3 deploy/cpa-runtime/prepare-release.py
cargo build --manifest-path codex-rs/Cargo.toml --locked \
  -p codex-app-server --bin codex-app-server
node deploy/cpa-runtime/check-official-identity.mjs \
  /absolute/path/to/official/codex-app-server \
  "$PWD/codex-rs/target/debug/codex-app-server"
CODEX_TEST_BINARY="$PWD/codex-rs/target/debug/codex-app-server" \
  node deploy/cpa-runtime/check-identity.mjs
```

The two-argument comparison checks the exact version output and four full
initialize sequences: standalone server and credential-directory master, each
with and without the native originator override. Each sequence includes daemon,
originating-client, backend, subsequent-client and invalid-header fixtures. The
complete response objects are compared without removing or normalizing fields.
Temporary state is shared consistently and all configured endpoints are loopback.
The worker probe separately checks Mac x64 and Windows Desktop identity plus
platform status envelopes; it does not change the unmanaged official comparison.

The existing four-argument mode additionally compares separately built official
and candidate CLI targets. It must only be used when those actual CLI artifacts
are available; the two-argument service check does not claim a candidate CLI build.

The second probe exercises a real master and credential worker against a local
mock upstream using synthetic credentials, checking the actual transmitted
headers. That same worker identity probe runs in the image workflow. Root-path
migration and shared credential locking preserve the existing installation ID.

Previous 0.162.1 checkpoint results remain in the Git history and dated validation
records. They do not substitute for the current official-release comparison.

Desktop host/engine separation follows official `rust-v0.164.0`
`app-server/src/request_processors/initialize_processor.rs` and
`login/src/auth/default_client.rs`. The installed Desktop's
`main-UOTY8NmV.js` uses `clientInfo.name/title = "Codex Desktop"` and its host
application version for `clientInfo.version`; the fixed local host version is
26.1023.51421. The Windows/error-1 and Mac-x64/error-2 codec and account-stable
screen/daily session strategy were cross-checked with
[codex2api PR827](https://github.com/james-6-23/codex2api/pull/827) and the fixed
reference snapshot `dfab070b32291c0a32d62c8941970a945d6ac3d3`. Pure-codec golden
vectors in `login/src/cpa_desktop_golden.json` were independently produced from
the official Desktop's extracted `Qi`/`Jne` functions without loading a native
module. The tests compare complete serialized envelope bytes, not just JSON keys.
