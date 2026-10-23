# Installing and building Codex Server

Codex Server runs as a master process and one worker per enabled credential.
Clients connect through the [service API](codex-server-api.md). CPA is optional.

## Container deployment

Use `ghcr.io/power12317/codex-server:latest` after its main-branch workflow has
published successfully. See the [deployment guide](../deploy/cpa-runtime/README.md)
for standalone Linux, CPA sidecar, remote access and timezone configuration.
Only the app-server executable and runtime dependencies are packaged.

## Build from source

Clone this fork, not the upstream CLI distribution:

```sh
git clone https://github.com/power12317/codex-server.git
cd codex-server/codex-rs
rustup show
python3 ../deploy/cpa-runtime/prepare-release.py
cargo build --locked --release -p codex-app-server --bin codex-app-server
```

Run release preparation in a build checkout. It stamps the official release
version into the build metadata; committed Cargo source placeholders remain
`0.0.0`. Docker performs the same preparation automatically.

Install Rust through [rustup](https://rustup.rs/) if needed. The workspace's
`rust-toolchain.toml` selects the supported toolchain. Native build dependencies
for the Linux image are listed in the
[Dockerfile](../deploy/cpa-runtime/Dockerfile); building on another operating
system also requires its native compiler and development libraries. Building
the full workspace or the CLI is not required to run this service.

From the repository root, create private credential and state directories outside
this checkout and launch the executable:

```sh
mkdir -p "$HOME/.local/share/codex-server/auths" "$HOME/.local/share/codex-server/state"
chmod 700 "$HOME/.local/share/codex-server/auths" "$HOME/.local/share/codex-server/state"
CODEX_CPA_AUTH_DIR="$HOME/.local/share/codex-server/auths" \
CODEX_HOME="$HOME/.local/share/codex-server/state" \
  ./codex-rs/target/release/codex-app-server
```

The credential directory must exist. An empty directory starts the listener
without any account workers. In another terminal:

```sh
curl --fail http://127.0.0.1:38317/readyz
```

The response is `ok`. Provision a credential using the
[credential and OAuth instructions](codex-server-api.md#credentials-and-oauth)
before submitting inference. Keep credential files out of version control.

## Runtime settings

| Setting              | Meaning                                                                                                         |
| -------------------- | --------------------------------------------------------------------------------------------------------------- |
| `CODEX_CPA_AUTH_DIR` | Existing credential directory; setting it selects the master service mode.                                      |
| `CODEX_HOME`         | Persistent worker-state root; the master defaults to `/var/lib/codex`. Set a writable directory for local runs. |
| `CODEX_CPA_PORT`     | Loopback HTTP/WebSocket port; default `38317`.                                                                  |

Workers use `CODEX_HOME/<credentialId>/config.toml`, where the credential ID is
its filename relative to the credential directory, including nested directories.
Only the master supplies the child-only `CODEX_CPA_AUTH_FILE` and
`CODEX_CPA_CREDENTIAL_ID` variables. Do not set them on the master.

The current master binds to `127.0.0.1`; there is no listen-address setting in
this API. It does not automatically load account credentials from an upstream
CLI installation or use an arbitrary `OPENAI_API_KEY` supplied by a caller.

## Development checks

The workspace uses `just` and `cargo-nextest`. Install them if needed:

```sh
cargo install --locked just cargo-nextest
rustup component add rustfmt clippy
```

From the repository root:

```sh
just test -p codex-app-server --lib --test all cpa_
just test -p codex-app-server-protocol --lib
just fmt
```

Use checks appropriate to the modified component. Native CLI, TUI and SDK
development remain available in the source tree, with their own upstream
documentation. Avoid `--all-features` for routine service builds.

## Logs

Master and worker diagnostics go to stderr. The default-visible `cpa.worker.started`
event identifies the credential, PID, private state directory and config path.
`cpa.request.*` events identify request mapping and lifecycle. The `cpa/*` log
prefixes are retained for compatibility. Worker stdout is reserved for internal
JSON-RPC messages. See the [API reference](codex-server-api.md#diagnostics) for
diagnostic notifications delivered to clients.
