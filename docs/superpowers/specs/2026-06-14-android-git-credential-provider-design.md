# Android Git Tool — Per-Operation Credential Provider FFI Design

Date: 2026-06-14
Status: Approved design (brainstormed + decisions locked)
Part of: the Android Git tool effort (P4 §G3 deferral — "per-operation
token-provider FFI vs resident token; revisit if a security review of mobile
credential residency demands it").

## Summary

Stop holding the two true Git secrets — the **HTTPS token** and the **SSH key
passphrase** — resident in `AndroidGitSecret` for the engine's lifetime. Instead
a Kotlin-implemented **`AndroidGitCredentialProvider`** (UniFFI
`callback_interface`, like the existing `AndroidVoice`/`AndroidCamera` foreign
traits) supplies them **per network operation**, invoked **synchronously** inside
libgit2's credentials callback. Between operations the process holds no plaintext
secret; the host (Android Keystore-backed) owns secret lifetime.

Non-secret credential material (CA dir, SSH private/public key *paths*, pinned
host-key hashes) stays resident — it is not sensitive. Host-key verification (G7)
and mbedTLS CA verification are unchanged.

## Decision log (this brainstorm, 2026-06-14)

| # | Decision | Choice |
|---|----------|--------|
| CP1 | Provider scope | **Secrets only.** The provider supplies the HTTPS token + SSH passphrase, fetched lazily in the matching credentials branch. CA dir / SSH key path / pubkey path / known-hosts hashes stay resident in `AndroidGitSecret` (paths/hashes/dirs, not secrets). |
| CP2 | Resident token | **Replace, no fallback.** Remove the `token` + `ssh_passphrase` fields from `AndroidGitSecret`; the provider is the only secret source. No provider (or provider returns `None`) → network ops report "credentials not configured" / auth failure. Internal FFI, so the breaking change is fine. |
| CP3 | Zeroization | **Deferred.** Per-op fetch (residency-window reduction) is the win; do NOT add the `zeroize` crate. Rationale: the secret is copied into libgit2's `Cred` (memory we don't control), so zeroizing only our transient copy is partial. Noted as a possible future hardening. |
| CP4 | Sync vs async | **Sync provider.** libgit2's credentials callback is synchronous (runs on the blocking git thread); the provider method must be sync-callable. (The existing async voice/camera callbacks use a tokio runtime; the git cred path cannot await.) |

## Goals

1. No plaintext Git secret resident between operations — the HTTPS token and SSH
   passphrase are fetched from the host per network op and used transiently.
2. Reuse the established UniFFI `callback_interface` + bridge pattern; no new
   build/NDK surface.
3. Keep host-key verification (G7) and mbedTLS CA verification unchanged.
4. Stay host-testable: a mock provider drives the credential-selection logic;
   `deny(unsafe_code)` holds.

## Non-goals

1. No zeroization of the transient secret copy (CP3).
2. No change to which operations need credentials, to host-key verification, or
   to CA/cert verification.
3. No per-op fetching of the non-secret material (CP1) — that stays resident.
4. No async credential path (CP4).

## Architecture

Follows the existing foreign-trait + bridge layering (`AndroidVoice` →
`AndroidVoiceBridge` → engine trait).

```text
lingxi-code/
├── tool-api/src/builtin_context.rs   MODIFY:
│   • + pub trait GitCredentialProvider: Send + Sync
│       { fn https_token(&self) -> Option<String>;
│         fn ssh_passphrase(&self) -> Option<String>; }   (engine-side trait)
│   • AndroidGitSecret: REMOVE `token` + `ssh_passphrase`;
│       ADD `credential_provider: Option<Arc<dyn GitCredentialProvider>>`.
│       Keep ca_dir / ssh_private_key_path / ssh_public_key_path /
│       ssh_known_hosts_sha256_hex. Redacting Debug drops the (now-absent)
│       secret fields; the provider prints opaquely (`<provider>`).
│   • AndroidGitToolCtx.has_token semantics → "a provider is configured"
│       (drives the prompt; registration gate is unaffected — it never used it).
├── tools/git-mobile/src/
│   ├── auth.rs   MODIFY:
│   │   • SshConfig: REMOVE the `passphrase` field (now from the provider).
│   │   • NetCallbacks: carry the provider instead of a borrowed token.
│   │   • + fn select_credential(allowed: CredentialType,
│   │         provider: Option<&dyn GitCredentialProvider>,
│   │         ssh: Option<&SshConfig>, username: &str) -> CredentialChoice
│   │       (pure, host-testable enum result: UserPass / SshKey / Username / None);
│   │       calls provider.https_token() for USER_PASS_PLAINTEXT and
│   │       provider.ssh_passphrase() for SSH_KEY. The credentials closure calls
│   │       select_credential then builds the git2::Cred.
│   ├── ops.rs    MODIFY: GitNetConfig replaces `token: Option<String>` with the
│   │             provider handle (Option<Arc<dyn GitCredentialProvider>>); SshConfig
│   │             no longer carries the passphrase.
│   └── lib.rs    MODIFY: git_net_config() builds GitNetConfig from the secret's
│                 provider + non-secret fields (no token/passphrase clone).
└── apps/android-aar/src/lib.rs   MODIFY:
    • #[uniffi::export(callback_interface)] pub trait AndroidGitCredentialProvider
        { fn https_token(&self) -> Option<String>; fn ssh_passphrase(&self) -> Option<String>; }
    • AndroidGitCredentialProviderBridge (wraps the foreign trait, impls
        tool_api::GitCredentialProvider).
    • build_android_engine: accept the provider (replacing the token/passphrase
        FFI inputs of AndroidGitConfigFfi); wire it into AndroidGitSecret;
        set has_token/provider-present from whether one was supplied.
```

### `select_credential` (the host-testable seam)

```text
enum CredentialChoice { Username(String), UserPass{user,token}, SshKey{user,key_path,pubkey,passphrase}, None }

select_credential(allowed, provider, ssh, username_from_url):
  user = username_from_url or "git"
  if allowed has USERNAME            -> Username(user)
  if allowed has SSH_KEY && ssh.some -> SshKey{ user, ssh.key_path, ssh.pubkey,
                                                provider?.ssh_passphrase() }
  if allowed has USER_PASS_PLAINTEXT && provider has token
                                     -> UserPass{ "x-access-token", token }
  else                               -> None
```
The libgit2 credentials closure calls `select_credential` and maps the choice to
`git2::Cred::{username, ssh_key, userpass_plaintext}` (or an error for `None`).
`select_credential` is pure w.r.t. libgit2 (returns owned data), so a mock
provider can unit-test that the right provider method is invoked per branch.

### Data flow (per network op)

1. libgit2 requests a credential of some `allowed_types`.
2. The closure calls `select_credential(allowed, provider, ssh, user)`.
3. For `USER_PASS_PLAINTEXT`: `provider.https_token()` → Kotlin fetches from the
   Keystore, returns the token transiently → `Cred::userpass_plaintext`.
   For `SSH_KEY`: `provider.ssh_passphrase()` → `Cred::ssh_key(user, pubkey?,
   key_path, passphrase?)`.
4. The String is dropped when the closure returns (no resident copy; zeroization
   deferred per CP3).
5. No provider / `None` token → `CredentialChoice::None` → no usable credential →
   the existing named "credentials not configured" / libgit2 auth-failure path.

## Error handling / security

- No plaintext secret resident between ops; the host owns secret lifetime
  (Keystore-backed, fetched + decrypted per op).
- `deny(unsafe_code)` holds — the UniFFI bridge is safe Rust; no new unsafe.
- Host-key verification (G7) and mbedTLS CA verification unchanged; the provider
  only supplies the auth secret, not the trust decision.
- A provider that throws/returns `None` fails closed (no credential), never a
  silent unauthenticated success beyond what public/anonymous remotes already do.

## Testing

- **tool-api:** a mock `GitCredentialProvider` (call-counting) in test-support;
  assert `AndroidGitSecret` carries it; redacting `Debug` shows no secret + an
  opaque provider; `has_token`/provider-present semantics.
- **tool-git-mobile:** `select_credential` unit tests with the mock —
  `USER_PASS_PLAINTEXT` → calls `https_token()` and yields `UserPass`; `SSH_KEY` →
  calls `ssh_passphrase()` and yields `SshKey`; `USERNAME` → `Username`; no
  provider / `None` token → `CredentialChoice::None`. `make_network_callbacks`
  assembles with a provider (no panic). The existing HTTPS/SSH/file:// op tests
  still pass (the file:// tests use no provider → anonymous, still work).
- **android-aar:** the bridge adapts the foreign trait to
  `GitCredentialProvider`; `build_android_engine` host build + `cargo ndk check`.
- **PENDING-DEVICE:** the live per-op fetch from the Kotlin Keystore is
  device-only; host coverage is the mock-driven `select_credential` + bridge.

## Phasing

- **P1 — tool-api:** `GitCredentialProvider` trait; rework `AndroidGitSecret`
  (remove token/passphrase, add provider); Debug + `has_token` semantics (host TDD).
- **P2 — tool-git-mobile:** `select_credential` + `CredentialChoice`; GitNetConfig
  provider field; SshConfig drops passphrase; credentials closure calls
  `select_credential`; git_net_config build (host TDD).
- **P3 — android-aar:** `AndroidGitCredentialProvider` UniFFI callback_interface +
  bridge; `build_android_engine` wiring; provider-present gate/prompt (host TDD +
  ndk check).
- **P4 — gate:** workspace test/clippy/both-ABI build + PENDING-DEVICE note.

## Risks

| Risk | Mitigation |
|------|------------|
| Sync FFI into Kotlin from the git thread | The git op already runs on `spawn_blocking`; a sync `callback_interface` call is fine (CP4). |
| Breaking FFI (token/passphrase inputs removed) | Internal API (CP2); `build_android_engine`'s signature changes + the Kotlin call site updates. All Rust construction sites updated so the workspace compiles. |
| Closure lifetime with the provider | The closure captures an `Arc<dyn GitCredentialProvider>` (owned clone) — simpler than the prior borrowed-token `'a` lifetime; the fetched String is local to the call. |
| Live per-op fetch only device-verifiable | Host covers `select_credential` + bridge via a mock; live Keystore fetch is PENDING-DEVICE (P4/G7 posture). |
