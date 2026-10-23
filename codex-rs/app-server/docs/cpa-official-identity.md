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
`/usr/local/bin/codex-app-server`, without a fork suffix. The native telemetry
identity and originator/User-Agent construction are retained. The master uses the
same native initialize identity helpers as the official app-server; it does not
invent a supervisor product identity.

A managed worker initializes as the retained `codex-tui` client with the compiled
official version. The actual HTTP identity check on the same macOS arm64 host
observes `originator: codex-tui`, `version: 0.164.0` and:

```text
codex-tui/0.164.0 (Mac OS 15.3.1; arm64) dumb (codex-tui; 0.164.0)
```

Platform and terminal components vary normally with the host. Comparisons must use
matching entry points, platforms, configuration and caller sequences, rather than
substituting one target's output for another's.

## Official artifact provenance

The reference executables are downloaded official `aarch64-apple-darwin` release
assets. Their compressed bytes are checked against the release API's SHA256 values
before execution:

| Official asset                              | SHA256                                                             |
| ------------------------------------------- | ------------------------------------------------------------------ |
| `codex-aarch64-apple-darwin.tar.gz`            | `aa8299c169d80111aac05543ad2621c9bdbedb9bc252c905fe3f5a5baf9a4142` |
| `codex-app-server-aarch64-apple-darwin.tar.gz` | `2eb75238651acb7d5c7cbb088b56433c71bbb680189c3bbd50318589858d57b1` |

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

The existing four-argument mode additionally compares separately built official
and candidate CLI targets. It must only be used when those actual CLI artifacts
are available; the two-argument service check does not claim a candidate CLI build.

The second probe exercises a real master and credential worker against a local
mock upstream using synthetic credentials, checking the actual transmitted
headers. That same worker identity probe runs in the image workflow. Root-path
migration and shared credential locking do not introduce identity overrides.

Previous 0.162.1 checkpoint results remain in the Git history and dated validation
records. They do not substitute for the current official-release comparison.
