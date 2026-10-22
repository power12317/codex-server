# Codex Server

Codex Server exposes native Codex inference through a long-running service for
third-party applications. A master process accepts requests and supervises one
worker per enabled credential. Workers rebuild native requests, map session and
thread identities, and stream model responses back to the caller.

CPA is one integration. Any application that implements the documented WebSocket
protocol can connect without running CPA.

## What the service provides

- Credential workers with independent configuration, installation identity and
  authentication state, managed by a directory-watching master.
- Responses inference through Codex's native request builder and provider/auth
  transport, including supported function, custom, namespace and web-search tools.
- Logical session/thread hierarchies, bounded turn routing state and selective
  restoration of source identities in responses.
- Streaming response bytes, cancellation, manual OAuth and credential reload.
- A Linux amd64/arm64 image containing the app-server executable and its runtime
  dependencies. The image does not include the CLI or a local tool execution host.

The inference path does not create local conversations, persist message history,
execute returned tool calls or generate additional titles. Callers provide their
own history and execute their own client-side tools. Diagnostic notifications can
contain full request/response payloads; clients decide whether to retain them.

## Start here

1. [Install and start the service](docs/install.md).
2. [Connect a third-party application](docs/codex-server-api.md).
3. [Deploy with containers or integrate with CPA](deploy/cpa-runtime/README.md).
4. [Read the identity and request-mapping contract](codex-rs/app-server/docs/cpa-bridge.md).

The default endpoints are:

| Endpoint                            | Purpose                                          |
| ----------------------------------- | ------------------------------------------------ |
| `GET http://127.0.0.1:38317/readyz` | Listener readiness; not an account-health check. |
| `ws://127.0.0.1:38317/`             | JSON-RPC-style control and inference stream.     |

The `cpa/*` methods are model-only extensions carried by the official app-server
envelope. Clients select supported behaviors from capabilities. Existing
`CODEX_CPA_*` settings and credential-file formats are retained; these names do
not require the CPA application. The executable remains `codex-app-server`.
This listener does not expose an HTTP `POST /v1/responses` endpoint or native
`thread/start` / `turn/start` methods; callers use `cpa/inference/start`.

The listener binds to loopback and has no built-in client authentication. Local
applications can connect directly. Remote applications can use an SSH tunnel or
an authenticated WebSocket reverse proxy, as described in the deployment guide.

## Images and source

Repository: [power12317/codex-server](https://github.com/power12317/codex-server).
The main-branch workflow publishes `ghcr.io/power12317/codex-server:latest`
after native amd64/arm64 builds and startup checks succeed. A workflow
trigger alone does not confirm publication. See the
[deployment guide](deploy/cpa-runtime/README.md) for migration from the previous
`codex-cpa-runtime` image name and building locally.

Codex Server is a fork of [OpenAI Codex](https://github.com/openai/codex). Native
crate names, upstream component documentation and license notices are retained.
The upstream CLI, SDK and full app-server sources are present for maintenance;
their documentation describes those components, not the inference-only listener.

- [Development and issue reporting](docs/contributing.md)
- [Protocol validation record](codex-rs/app-server/docs/cpa-bridge-validation.md)
- [Upstream Codex documentation](https://developers.openai.com/codex)

Licensed under the [Apache-2.0 License](LICENSE).
