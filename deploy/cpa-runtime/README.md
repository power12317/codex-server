# Codex Server deployment

Pushes to main publish `ghcr.io/power12317/codex-server:latest`.
Use the service standalone or alongside CPA; a source checkout is not required
on a deployment host. The `deploy/cpa-runtime` directory name and compatibility
settings are retained so existing build scripts continue to work.

The build uses Rust 1.99.1, Debian bookworm and `Cargo.lock` (`--locked`). It builds
only `codex-app-server`, which contains the CPA master, credential workers, native
request builder, OAuth library and response bridge. The inference-only image
does not build or package the CLI, V8/Code Mode host or local execution helpers.
Both native Linux architectures must pass the startup check before either tag
is updated. The fork source commit is recorded in the image revision label.

## Standalone Linux deployment

Create `auths` and `codex-data` in a private deployment directory. For the image's
non-root default, make both writable by UID 10001, or explicitly select the UID
that owns them. The example below runs as UID 10001:

```sh
mkdir -p auths codex-data
sudo chown 10001:10001 auths codex-data
chmod 700 auths codex-data
```

Use the supplied [compose.yaml](compose.yaml), or this equivalent configuration,
on a Linux host. Place it in the deployment directory containing `auths` and
`codex-data`:

```yaml
services:
  codex-server:
    image: ghcr.io/power12317/codex-server:latest
    network_mode: host
    volumes:
      - ./auths:/cpa-auth
      - ./codex-data:/var/lib/codex
      - type: bind
        source: /etc/localtime
        target: /etc/localtime
        read_only: true
        bind:
          create_host_path: false
    restart: unless-stopped
```

Run `docker compose up -d` and check
`curl --fail http://127.0.0.1:38317/readyz`. An empty credential directory starts
the listener without account workers. Follow the
[third-party API guide](../../docs/codex-server-api.md) to provision credentials,
complete OAuth and submit inference. Any local application can connect directly;
CPA is not needed. For macOS/Windows development, use the
[native build instructions](../../docs/install.md) or an environment with Linux
host networking.

The master binds only to loopback. A Docker `ports:` mapping alone cannot reach a
listener bound to the container's `127.0.0.1`. Standalone host networking shares
that loopback with host applications; the CPA example below instead shares CPA's
network namespace.

## Existing CPA integration

The image runs one master and one native app-server child per enabled credential.
Add this fragment to CPA's Compose file; no additional environment,
command, bridge key, credential filename or port mapping is needed:

```yaml
services:
  codex-master:
    image: ghcr.io/power12317/codex-server:latest
    network_mode: service:cli-proxy-api
    user: "0:0"
    volumes:
      - ./auths:/cpa-auth
      - ./codex-data:/var/lib/codex
      - type: bind
        source: /etc/localtime
        target: /etc/localtime
        read_only: true
        bind:
          create_host_path: false
    restart: unless-stopped
```

Use the same runtime UID as CPA (the example matches a root CPA container).
The shared auth directory is `/cpa-auth`; private state is stored in the CPA
directory's `./codex-data`, mounted at `/var/lib/codex`.
The loopback endpoint is `ws://127.0.0.1:38317/`, with `/readyz` on that
same internal port. The existing CPA and gost services need no port changes.

`credentialId` is CPA's original path relative to the shared auth directory,
including nested directories, capitalization, spaces and Unicode. It is the
only account-routing identity. Each child has a persistent private CODEX_HOME
named after that original relative path, including the file extension:
`account.json` uses `/var/lib/codex/account.json/`, and `team/My Account.JSON`
uses `/var/lib/codex/team/My Account.JSON/`. A worker's optional configuration
file is directly inside that directory, for example
`/var/lib/codex/account.json/config.toml`; there is no extra `.codex` level.
When first starting an account after upgrading, the master moves its old hash
directory to the readable path if the latter does not exist, preserving all
state. If both paths exist, it uses the readable directory and leaves the old
one untouched. Credentials are still read and refreshed in the original shared
file. There is no mirrored auth.json.

Worker command lines still show `--listen stdio://`: their private configuration
is selected by `CODEX_HOME`, not a config-file command-line flag. `docker logs
codex-master` now includes a `cpa.worker.started` JSON event mapping the PID to
its credential name, CODEX_HOME and `config.toml` path. Request lifecycle, identity
mapping and upstream status/header events are visible by default on stderr,
without setting `RUST_LOG`. The RPC UUID and upstream `x-request-id` correlate
worker events with CPA's existing detailed logs; the RPC UUID is not CPA's
eight-character outer log ID. Authentication headers and cookies are redacted.

The master scans on startup and every 500 ms. It starts an account only when the
file has `type: "codex"`, `codex_cli.enabled: true`, and no `disabled: true`.
CPA's reload RPC uses the same reconciliation code and waits for starts/stops.
Token-only updates keep the same child. Disabling/removing a credential ends its
child without draining inference or flushing exporters; private state survives
reenabling. An already submitted upstream operation cannot be recalled.
CPA owns global/per-account preferences and request selection; the master only
applies effective file flags. The runtime does not execute model tool calls.

Each credential worker holds logical session/thread mappings and bounded turn
routing contexts in memory. It does not create local conversation threads or
persist their message history. Root, child and parent identities are mapped
within the credential and CPA scope, and known response identity fields are
restored to the values CPA supplied. Explicit title requests use disposable
contexts and do not trigger additional title generation. Requests must include
their own history; worker restart clears turn state. Full native request/response
diagnostics still use the existing CPA log channel.

The master starts credential workers using the same `codex-app-server` binary.
OAuth uses the linked `codex-login` library. Enabled children retain applicable
native background activities. Refresh runs on credential access/401 recovery;
no independent account heartbeat or synthetic analytics event is added.

See the [third-party API](../../docs/codex-server-api.md), [model-only extension contract](../../codex-rs/app-server/docs/cpa-bridge.md) and
[validation record](../../codex-rs/app-server/docs/cpa-bridge-validation.md).

Pushes to `main` run `.github/workflows/cpa-runtime-image.yml`.
The workflow builds on native Ubuntu amd64 and arm64 runners and checks `/readyz`
on each architecture before publishing a multiarch manifest under
`ghcr.io/power12317/codex-server:latest`. Native images are
transferred between jobs by digest, without architecture or SHA image tags.
The source revision and image digests are recorded in the Actions summary.
A triggered workflow is not evidence of a successful publication. GHCR package
visibility and anonymous manifest access must be checked after publication.

## Remote applications

The service endpoint has no built-in client authentication or TLS. For access
from another machine, keep the listener on loopback and use an authenticated
transport. For example, when the service uses host networking on a Linux server:

```sh
ssh -N -L 38317:127.0.0.1:38317 user@server
```

The remote client then connects to `ws://127.0.0.1:38317/` on its own
machine. An authenticated reverse proxy is another option: run it in a network
namespace that can reach the listener, enable WebSocket upgrades, preserve the
`/` path, and set timeouts appropriate to inference streams. TLS and client
access control belong at that boundary. Possession of a `credentialId` is not a
client-authorization check; exposed clients can invoke the service's control API.

## Image-name migration and local builds

The repository is now `power12317/codex-server`, and new main-branch publications
use `ghcr.io/power12317/codex-server:latest`. The previous
`ghcr.io/power12317/codex-cpa-runtime` package is a separate registry path; it is
not renamed automatically and this workflow no longer updates its tags. Wait for
the new package to publish successfully, then change only the Compose image
reference and recreate the service. Retain the existing credential and state
mounts. Container service names such as `codex-master` may stay unchanged in an
existing deployment.

Build locally from the repository root if the registry image is not available:

```sh
docker build -f deploy/cpa-runtime/Dockerfile \
  --build-arg FORK_REVISION="$(git rev-parse HEAD)" \
  -t codex-server:local .
```

Use `codex-server:local` in Compose for that build. The existing
`.github/workflows/cpa-runtime-image.yml` filename is retained; its repository
guard and image labels point to `power12317/codex-server`. Runtime executable,
credential filenames and state locations are unchanged. Model-only extensions
are described through factual capabilities.

## Runtime release identity

The current upstream release base is `rust-v0.164.0` at
`6aaac694803004a63c6f393d153c96d22521a6b9`. Existing fork changes and previously
integrated upstream commits remain in the merge history. This is a fork build,
not an unmodified official release archive.

`release-version` selects the official client version used for the executable,
version header and User-Agent. Committed workspace and local Cargo.lock package
versions remain the source placeholder `0.0.0`. Docker runs `prepare-release.py`
in its build copy to stamp the exact official release `0.164.0`, including implicit
workspace members, before compilation. The script also accepts already prepared
stable manifests; it does not resolve or upgrade third-party dependencies. `cpa/capabilities/read` reports the compiled runtime version and selected
upstream release commit as informational metadata. Client identity names remain
unchanged; capabilities describe supported extension behavior.

The official release contains two distinct targets: the `codex-cli` package
builds `codex` (`codex-cli 0.164.0`), while the `codex-app-server` package builds
`codex-app-server` (`codex-app-server 0.164.0`). The image uses the latter official
target; it is not a renamed CLI. The supervisor's `initialize` response uses
native client identity generation rather than a fork-specific User-Agent name.
See [the official identity audit](../../codex-rs/app-server/docs/cpa-official-identity.md)
for release artifact checks and the comparison command.

## Runtime timezone

The supplied Compose configurations bind the deployment host's `/etc/localtime`
read-only into the container. With no `TZ` override, the container and its workers
therefore use the host's timezone rules, including date rollover and daylight
saving changes. No country or language determines a default.

A Docker image cannot discover an unmounted host timezone. Pulling a new image
or restarting an existing container does not add a mount. Existing deployments
must add the mount and recreate the container. For CPA's current
`docker-compose.codex.yml`, the supplied
[compose.cpa-timezone.yaml](compose.cpa-timezone.yaml) overlay adds it without
changing credentials, data paths, image references or networking. Run from the
CPA deployment directory, substituting the overlay's actual path:

```sh
docker compose -f docker-compose.codex.yml \
  -f /path/to/codex-server/deploy/cpa-runtime/compose.cpa-timezone.yaml \
  up -d --force-recreate codex-server
```

Keep using both files for subsequent Compose operations, or copy the time mount
into the existing service configuration. The overlay targets the `codex-server`
service; use the existing service name if a deployment calls it something else.
The bind source is resolved on the Docker daemon host. The standalone example
assumes Linux; Docker Desktop has its own VM and file-sharing behavior.

`/etc/localtime` is a regular file in the image, so a bind mount cannot replace
the image's `Etc/UTC` zoneinfo file through Debian's default symlink. The image
includes `tzdata` for explicit IANA `TZ` settings and native timezone lookup.
An explicit `TZ` takes precedence over the mounted file; remove a conflicting
`TZ` to follow the host. An unconfigured container with no mount retains UTC.

On hosts that also provide `/etc/timezone`, it can additionally be mounted
read-only to preserve the IANA name. This file is optional because not every
Linux distribution has it. If only `/etc/localtime` is mounted and the image's
name file is stale, the request adapter reports the actual offset, such as
`UTC+09:00`, rather than an incorrect `Etc/UTC` name. This is the host's effective
offset, not UTC+00:00.

Compare the effective offset on the deployment host and in the container:

```sh
date '+%Y-%m-%d %H:%M:%S %Z %z'
docker compose exec codex-server date '+%Y-%m-%d %H:%M:%S %Z %z'
docker inspect codex-server --format '{{json .Mounts}}'
```

The request adapter replaces existing date/timezone fields in the latest
`environment_context` and `codex_apps_client_time_context` declarations, including
when later items are tool calls/results. It does not append messages or add
missing time fields. Older declarations, paths, OS descriptions and tool payloads
remain intact.

The image workflow runs [check-timezone.sh](check-timezone.sh) against both native
architectures before publication. It verifies UTC, Singapore and Tokyo tzfile
mounts with `TZ` unset, and confirms the image's UTC database stays unchanged.
These are test fixtures, not deployment defaults.
