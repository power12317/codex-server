# Agent instructions

## Official identity

- For the same official release, entry point and target platform, preserve upstream names and version information byte-for-byte: Cargo packages/binaries, version output, initialize metadata, User-Agent, originator and telemetry identity.
- Never invent product/client names, version numbers or suffixes. Compare with the matching official source and release artifact before changing identity-related code.
- Preserve official target distinctions: `codex-cli` builds `codex`; `codex-app-server` builds `codex-app-server`. Do not rename a target or fake another entry point's version output.
- CPA methods are model-only app-server extensions. Describe actual behavior with capabilities; do not add private protocol numbers or use product/source metadata as a gate. Keep isolated experimental image package names separate from official runtime identity.

## Working conventions

- Keep repository documentation in its existing language (English).
- Inspect branch/worktree state before editing; preserve unrelated changes and other worktrees.
- Follow `docs/install.md` for service builds and checks; use the pinned Rust toolchain and the root `justfile`.
- Regenerate protocol artifacts through `codex-rs/app-server-protocol/scripts/write_schema_fixtures.py`; do not hand-edit generated schemas or SDK types.

## Focused checks

| Change                  | Command                                                                                           |
| ----------------------- | ------------------------------------------------------------------------------------------------- |
| Service/worker behavior | From `codex-rs`: `cargo test -p codex-app-server --lib --test all cpa_`                           |
| Runtime identity        | `CODEX_TEST_BINARY=/absolute/path/to/codex-app-server node deploy/cpa-runtime/check-identity.mjs` |
| Release stamping        | `python3 deploy/cpa-runtime/test_prepare_release.py` (Python 3.11+)                               |
| Rust formatting         | From `codex-rs`: `cargo fmt --all --check`                                                        |

## References

- Service API: `docs/codex-server-api.md`
- Deployment and image layout: `deploy/cpa-runtime/README.md`
- Official identity comparison: `codex-rs/app-server/docs/cpa-official-identity.md`
- Release source selection: `deploy/cpa-runtime/release-version`, `deploy/cpa-runtime/prepare-release.py`
- Contribution policy: `docs/contributing.md`; security policy: `SECURITY.md`

## Commit attribution

- Include `Co-Authored-By: Codex <noreply@openai.com>` in commits authored by this agent.
