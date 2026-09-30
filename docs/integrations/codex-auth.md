# Codex Auth in Desktop providers

Settings → Provider → Codex Auth signs in with a ChatGPT account. The provider ID is
`openai-chatgpt`; ordinary `openai` remains an API-key provider. The desktop uses
browser authorization code + PKCE on localhost port 1455, with a ten-minute deadline,
cancellation, and state validation. An occupied callback port produces an actionable
error. It does not read or modify Codex/OpenCode's local credential files.

## Credential ownership

The Electron main process exchanges the authorization code and saves the OAuth
session in the existing Credential Broker under `openai-chatgpt`. The renderer only
receives credential status. OAuth session JSON cannot enter the generic API-key path,
credential preview, diagnostics, or renderer event replay.

A selected Codex session receives tokens through the private stdin `openai_oauth`
envelope before Rust builds its OAuth provider. The existing Rust refresh machinery
emits `openai_oauth_updated` to its authenticated host. The host consumes the event,
serializes secure-store writes, and stops the runtime on persistence failure. A
refresh during startup is retained until the host handshakes. This is an event-based
handoff, not a durable-write acknowledgment; a process crash between upstream token
rotation and broker persistence can require signing in again.

Only one Codex runtime can own this account's refresh token at a time. Switching to
another idle Codex chat stops the previous runtime and loads the latest saved token.
An active Codex chat must finish first. Login/logout serialize with runtime ownership;
logout stops the OAuth runtime before deleting the session. Existing external Codex
bridge processes are not adopted because their OAuth ownership cannot be proven.

This entry requires the desktop Credential Broker. On macOS, use the signed packaged
application for real credential persistence, following the repository packaging
instructions. The UI reports unavailable secure storage in unsupported builds.

## Reused implementation and validation

The integration reuses `llm-client`'s ChatGPT OAuth authenticator/refresh driver and
Responses codec. Codex requests omit unsupported sampling/output-limit fields and
set `store: false`; API-key requests retain their existing behavior. No dependencies
were added.

Automated checks cover PKCE and state rejection, callback release/cancellation,
secret redaction and routing, host storage/logout, startup refresh delivery, exclusive
runtime ownership, model switching, and the real Electron provider page with a mocked
account. They do not validate an actual ChatGPT account's entitlement or perform a
live OAuth sign-in.

References consulted:

- [Codex authentication](https://learn.chatgpt.com/docs/auth)
- [OpenCode Codex authentication implementation](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/plugin/openai/codex.ts)
- [Codex browser authentication implementation](https://github.com/openai/codex/blob/main/codex-rs/login/src/server.rs)
- [Codex device authentication implementation](https://github.com/openai/codex/blob/main/codex-rs/login/src/device_code_auth.rs)

## Changed implementation surfaces

- `apps/electron/src/main/codex-auth.ts`: browser login and validated session parsing.
- `apps/electron/src/main/{host,credential-broker,index}.ts`: IPC, secure persistence and launch wiring.
- `apps/electron/src/main/{bridge,host-utils}.ts`: private token transport, refresh persistence, runtime ownership and model switching.
- `apps/electron/src/{preload/index,shared/providers,shared/clientCommands}.ts` and renderer provider/bridge files: provider entry, login actions and status.
- `packages/bridge-client/src/{protocol,validation,protocolCoverage,client}.ts`: typed private event, validation and exclusion from replay.
- `apps/bridge-server/src/{boot,main,server}.rs` and `apps/engine-desktop/src/lib.rs`: prebuild OAuth injection and startup event delivery.
- `lingxi-code/secret/src/credential.rs`, `llm-client/src/client.rs`, and `client-protocol`: refresh observer, Codex request shape and appended wire event.
- Corresponding Electron/shared/Rust tests and protocol snapshots. The existing mobile/parity edits in the working tree are outside this change.

Electron final verification: typecheck and production build pass. The sequential
complete suite passes 965/968; the remaining failures are `/loop` wakeup grouping,
`ProviderEditorFields` native-title guard, and settings navigation group count.
The Codex Auth UI fixture and all targeted authentication checks pass. Shared client
suite: 61/61 pass.

Rust verification includes `cargo check -p bridge-server`; protocol snapshots (8)
and version guards (9) with UniFFI enabled; LLM authentication tests (16); and the
OAuth credential observer regression (1). All of these pass.
Bridge OAuth injection/startup-delivery regressions: 2/2 pass.
Final Rust result: all 36 targeted tests pass; `cargo clippy -p bridge-server -p
llm-client -p secret --lib` exits successfully with existing workspace warnings.
