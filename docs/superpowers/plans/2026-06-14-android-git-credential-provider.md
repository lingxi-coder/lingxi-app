# Android Git Per-Op Credential Provider FFI Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop holding the HTTPS token + SSH passphrase resident in `AndroidGitSecret`; supply them per network op via a Kotlin-implemented `AndroidGitCredentialProvider` (UniFFI `callback_interface`), invoked synchronously inside libgit2's credentials callback.

**Architecture:** A `GitCredentialProvider` engine-trait (tool-api) is supplied by an android-aar UniFFI `callback_interface` + bridge (the established `AndroidVoice`→`AndroidVoiceBridge` pattern). A pure, host-testable `select_credential(allowed, provider, ssh, user)` resolves the per-op credential (calling `provider.https_token()` / `provider.ssh_passphrase()`); the credentials closure maps its result to a `git2::Cred`. Non-secret material (CA dir, SSH key/pubkey paths, host-key hashes) stays resident.

**Tech Stack:** Rust workspace `lingxi-code/`. `git2` (vendored, mbedTLS backend). UniFFI `callback_interface`. Crates: `tool-api`, `tool-git-mobile` (`#![deny(unsafe_code)]`), `android-aar`. No build/NDK gate — pure Rust + UniFFI wiring; host-testable with a mock provider.

**Spec:** `docs/superpowers/specs/2026-06-14-android-git-credential-provider-design.md` (CP1-CP4).

**Predecessor:** P4+G2+G7+mbedTLS on `main` (`0d785ab1`). This branch (`android-git-cred-provider`) is cut from there.

**Invariants:**
- `tool-git-mobile` stays `#![deny(unsafe_code)]`; `tool-api`/`android-aar` stay `#![forbid(unsafe_code)]`. The UniFFI bridge is safe.
- No plaintext secret resident between ops (CP1/CP2). Non-secrets stay resident.
- Host-key verification (G7) + mbedTLS CA verification unchanged — the provider supplies only the auth secret, not the trust decision.
- No `zeroize` (CP3, deferred).

---

## File structure

```text
lingxi-code/
├── tool-api/src/builtin_context.rs   MODIFY: + GitCredentialProvider trait;
│       AndroidGitSecret remove token+ssh_passphrase, add credential_provider;
│       rework Debug; AndroidGitToolCtx.has_token doc → "provider configured".
├── tools/git-mobile/src/
│   ├── auth.rs   MODIFY: + CredentialChoice + select_credential; SshConfig drop
│   │             passphrase; NetCallbacks carry provider; closure calls select_credential.
│   ├── ops.rs    MODIFY: GitNetConfig replace token with provider handle.
│   └── lib.rs    MODIFY: git_net_config builds from secret.credential_provider.
└── apps/android-aar/src/lib.rs   MODIFY: + AndroidGitCredentialProvider uniffi
        callback_interface + bridge; AndroidGitConfigFfi drop https_token/
        ssh_passphrase; build_android_engine takes the provider param + wires it;
        has_token = provider present; fix android_git_probe construction site.
```

---

# Phase P1 — tool-api: provider trait + AndroidGitSecret rework

### Task 1: `GitCredentialProvider` trait + rework `AndroidGitSecret`

**Files:** Modify `lingxi-code/tool-api/src/builtin_context.rs`.

- [ ] **Step 1: Failing tests** in builtin_context.rs's `#[cfg(test)] mod tests`:
```rust
    /// A mock provider for tests — counts calls and returns fixed secrets.
    struct MockProvider {
        token: Option<String>,
        passphrase: Option<String>,
        token_calls: std::sync::atomic::AtomicUsize,
        pass_calls: std::sync::atomic::AtomicUsize,
    }
    impl GitCredentialProvider for MockProvider {
        fn https_token(&self) -> Option<String> {
            self.token_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.token.clone()
        }
        fn ssh_passphrase(&self) -> Option<String> {
            self.pass_calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.passphrase.clone()
        }
    }

    #[test]
    fn android_git_secret_carries_provider_and_debug_has_no_secret() {
        let provider: std::sync::Arc<dyn GitCredentialProvider> = std::sync::Arc::new(MockProvider {
            token: Some("tok".into()), passphrase: Some("pp".into()),
            token_calls: Default::default(), pass_calls: Default::default(),
        });
        let s = AndroidGitSecret {
            credential_provider: Some(provider.clone()),
            ca_dir: Some("/system/etc/security/cacerts".into()),
            ssh_private_key_path: Some("/data/k".into()),
            ..Default::default()
        };
        // The provider is reachable and returns the secret on demand.
        assert_eq!(s.credential_provider.as_ref().unwrap().https_token().as_deref(), Some("tok"));
        // Debug shows NO secret value and an opaque provider marker.
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("tok") && !dbg.contains("pp"), "no secret in Debug: {dbg}");
        assert!(dbg.contains("ca_dir"), "non-secrets still shown: {dbg}");
    }
```

- [ ] **Step 2: Run → FAIL** `cargo test -p tool-api android_git_secret_carries_provider`.

- [ ] **Step 3: Implement.**
  - Add the trait (near `AndroidGitSecret`):
```rust
/// Per-operation Git credential provider (spec: per-op credential FFI). Supplies
/// the two true Git secrets — the HTTPS token and the SSH key passphrase —
/// fetched lazily by `tool-git-mobile` inside libgit2's credentials callback,
/// once per network op. Implemented by the host (android-aar bridges a UniFFI
/// `AndroidGitCredentialProvider` onto this); the secrets are never held resident
/// between ops. Sync (libgit2's cred callback is synchronous).
pub trait GitCredentialProvider: Send + Sync {
    /// The HTTPS token (PAT) for `userpass_plaintext`, or `None` for anonymous.
    fn https_token(&self) -> Option<String>;
    /// The SSH private-key passphrase, or `None` if the key is unencrypted.
    fn ssh_passphrase(&self) -> Option<String>;
}
```
  - In `AndroidGitSecret`: REMOVE `pub token: Option<String>` and `pub ssh_passphrase: Option<String>`. ADD (keep `#[derive(Clone, Default)]` working — `Option<Arc<..>>` is Clone + Default=None):
```rust
    /// Per-op secret provider (HTTPS token + SSH passphrase). `None` → no secrets
    /// available (anonymous/public remotes only). Replaces the former resident
    /// `token`/`ssh_passphrase` fields — secrets are no longer held resident.
    pub credential_provider: Option<std::sync::Arc<dyn GitCredentialProvider>>,
```
  (Keep `ca_dir`, `ssh_private_key_path`, `ssh_public_key_path`, `ssh_known_hosts_sha256_hex`.)
  - Rework the manual `Debug` impl: drop the `token` + `ssh_passphrase` fields; print the provider opaquely:
```rust
impl std::fmt::Debug for AndroidGitSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AndroidGitSecret")
            .field("credential_provider", &self.credential_provider.as_ref().map(|_| "<provider>"))
            .field("ca_dir", &self.ca_dir)
            .field("ssh_private_key_path", &self.ssh_private_key_path)
            .field("ssh_public_key_path", &self.ssh_public_key_path)
            .field("ssh_known_hosts_sha256_hex", &self.ssh_known_hosts_sha256_hex)
            .finish()
    }
}
```
  - Update the `AndroidGitToolCtx.has_token` doc comment: its meaning is now "a credential provider is configured" (drives the prompt; the registration gate does not use it). Do NOT rename the field (avoid churn) — only the doc.

- [ ] **Step 4: Run → PASS** `cargo test -p tool-api` + `cargo clippy -p tool-api --all-targets -- -D warnings`. (Other tool-api construction sites of `AndroidGitSecret` — if any in tool-api tests — updated to the new fields. `cargo check --workspace` will reveal downstream sites; those are fixed in P2/P3, so a workspace check may fail here — that's expected until P3. Confirm `tool-api` itself compiles+tests.)

- [ ] **Step 5: Commit** `git add -A && git commit -m "feat(tool-api): GitCredentialProvider trait + AndroidGitSecret per-op provider (replace resident token/passphrase) (P1)"`

---

# Phase P2 — tool-git-mobile: select_credential + wiring

### Task 2: `select_credential` + `CredentialChoice`; thread the provider

**Files:** Modify `lingxi-code/tools/git-mobile/src/auth.rs`, `ops.rs`, `lib.rs`.

- [ ] **Step 1: Failing tests** in auth.rs's `#[cfg(test)] mod tests`:
```rust
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct MockProvider { token: Option<String>, pass: Option<String>, tc: AtomicUsize, pc: AtomicUsize }
    impl tool_api::GitCredentialProvider for MockProvider {
        fn https_token(&self) -> Option<String> { self.tc.fetch_add(1, Ordering::SeqCst); self.token.clone() }
        fn ssh_passphrase(&self) -> Option<String> { self.pc.fetch_add(1, Ordering::SeqCst); self.pass.clone() }
    }
    fn mock(token: Option<&str>, pass: Option<&str>) -> MockProvider {
        MockProvider { token: token.map(Into::into), pass: pass.map(Into::into), tc: AtomicUsize::new(0), pc: AtomicUsize::new(0) }
    }

    #[test]
    fn select_credential_userpass_calls_https_token() {
        let p = mock(Some("tok"), None);
        let c = select_credential(git2::CredentialType::USER_PASS_PLAINTEXT, Some(&p), None, "git");
        assert_eq!(c, CredentialChoice::UserPass { user: TOKEN_USERNAME.to_owned(), token: "tok".to_owned() });
        assert_eq!(p.tc.load(Ordering::SeqCst), 1, "https_token fetched once, per-op");
    }
    #[test]
    fn select_credential_sshkey_calls_passphrase() {
        let p = mock(None, Some("pp"));
        let ssh = SshConfig { private_key_path: "/k".into(), public_key_path: Some("/k.pub".into()), known_hosts_sha256_hex: vec![] };
        let c = select_credential(git2::CredentialType::SSH_KEY, Some(&p), Some(&ssh), "git");
        assert_eq!(c, CredentialChoice::SshKey { user: "git".into(), key_path: "/k".into(), pubkey: Some("/k.pub".into()), passphrase: Some("pp".into()) });
        assert_eq!(p.pc.load(Ordering::SeqCst), 1, "ssh_passphrase fetched once, per-op");
    }
    #[test]
    fn select_credential_username_first() {
        let c = select_credential(git2::CredentialType::USERNAME, None, None, "git");
        assert_eq!(c, CredentialChoice::Username("git".to_owned()));
    }
    #[test]
    fn select_credential_none_without_provider_or_token() {
        // USER_PASS requested but no provider → None.
        assert_eq!(select_credential(git2::CredentialType::USER_PASS_PLAINTEXT, None, None, "git"), CredentialChoice::None);
        // provider present but returns no token → None.
        let p = mock(None, None);
        assert_eq!(select_credential(git2::CredentialType::USER_PASS_PLAINTEXT, Some(&p), None, "git"), CredentialChoice::None);
    }
```

- [ ] **Step 2: Run → FAIL** `cargo test -p tool-git-mobile select_credential` (unresolved `select_credential`/`CredentialChoice`; SshConfig still has `passphrase`).

- [ ] **Step 3: Implement** in auth.rs:
  - `SshConfig`: REMOVE the `pub passphrase: Option<String>` field (now from the provider). Keep `private_key_path`, `public_key_path`, `known_hosts_sha256_hex`.
  - Add the choice enum + selector:
```rust
/// The credential `select_credential` resolved for a libgit2 request — owned so
/// it's testable independent of libgit2's `Cred` (which the closure builds from it).
#[derive(Debug, PartialEq, Eq)]
pub enum CredentialChoice {
    Username(String),
    UserPass { user: String, token: String },
    SshKey { user: String, key_path: String, pubkey: Option<String>, passphrase: Option<String> },
    None,
}

/// Resolve the credential for a libgit2 `allowed` request, fetching secrets from
/// the per-op `provider` (HTTPS token / SSH passphrase) lazily. Pure: returns
/// owned data, so a mock provider unit-tests the per-op fetch.
#[must_use]
pub fn select_credential(
    allowed: git2::CredentialType,
    provider: Option<&dyn tool_api::GitCredentialProvider>,
    ssh: Option<&SshConfig>,
    username: &str,
) -> CredentialChoice {
    if allowed.contains(git2::CredentialType::USERNAME) {
        return CredentialChoice::Username(username.to_owned());
    }
    if let Some(ssh) = ssh {
        if allowed.contains(git2::CredentialType::SSH_KEY) {
            return CredentialChoice::SshKey {
                user: username.to_owned(),
                key_path: ssh.private_key_path.clone(),
                pubkey: ssh.public_key_path.clone(),
                passphrase: provider.and_then(tool_api::GitCredentialProvider::ssh_passphrase),
            };
        }
    }
    if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
        if let Some(token) = provider.and_then(tool_api::GitCredentialProvider::https_token) {
            return CredentialChoice::UserPass { user: TOKEN_USERNAME.to_owned(), token };
        }
    }
    CredentialChoice::None
}
```
  (Note: `provider.and_then(tool_api::GitCredentialProvider::https_token)` passes `&dyn` to the method — if the fn-pointer form fights the borrow, use a closure `|p| p.https_token()`.)
  - `NetCallbacks`: replace `pub token: Option<&'a str>` with `pub provider: Option<&'a dyn tool_api::GitCredentialProvider>`. Keep `pub ssh: Option<&'a SshConfig>`.
  - Rewrite the `make_network_callbacks` credentials closure to delegate to `select_credential` and map the result:
```rust
    let provider = p.provider;
    let ssh = p.ssh;
    callbacks.credentials(move |_url, username_from_url, allowed| {
        let user = username_from_url.unwrap_or("git");
        match select_credential(allowed, provider, ssh, user) {
            CredentialChoice::Username(u) => git2::Cred::username(&u),
            CredentialChoice::UserPass { user, token } => git2::Cred::userpass_plaintext(&user, &token),
            CredentialChoice::SshKey { user, key_path, pubkey, passphrase } => git2::Cred::ssh_key(
                &user,
                pubkey.as_deref().map(Path::new),
                Path::new(&key_path),
                passphrase.as_deref(),
            ),
            CredentialChoice::None => Err(git2::Error::from_str(
                "no usable git credential for the requested authentication type",
            )),
        }
    });
```
  (The `certificate_check` SSH host-key block below is UNCHANGED.)
  - In `ops.rs`: `GitNetConfig` — replace `pub token: Option<String>` with `pub provider: Option<std::sync::Arc<dyn tool_api::GitCredentialProvider>>`. Everywhere a network op builds `NetCallbacks { token: net.token.as_deref(), ssh: ... }`, change to `NetCallbacks { provider: net.provider.as_deref(), ssh: ... }` (`Option<Arc<dyn T>>::as_deref()` → `Option<&dyn T>`). The `GitNetConfig` `Debug` (if manual) drops the token; if derived, `Arc<dyn T>` isn't Debug — add a manual Debug printing `provider: <provider>` (mirror the prior one that printed `ssh: <configured>`).
  - In `lib.rs` `git_net_config`: build `provider: self.ctx.android_git_secret.as_ref().and_then(|s| s.credential_provider.clone())`; the `ssh` SshConfig no longer sets `passphrase` (field gone). Non-secret fields unchanged.

- [ ] **Step 4: Run → PASS** `cargo test -p tool-git-mobile` (the new select_credential tests + all existing op/auth tests — the file:// tests use no provider → anonymous, still pass) + `cargo clippy -p tool-git-mobile --all-targets -- -D warnings`.

- [ ] **Step 5: Commit** `git add -A && git commit -m "feat(tool-git-mobile): select_credential + per-op provider in GitNetConfig/closure; SshConfig drops passphrase (P2)"`

---

# Phase P3 — android-aar: UniFFI provider + bridge + wiring

### Task 3: `AndroidGitCredentialProvider` callback interface + bridge + engine wiring

**Files:** Modify `lingxi-code/apps/android-aar/src/lib.rs`.

- [ ] **Step 1:** Add the UniFFI callback interface + bridge (mirror `AndroidVoice`/`AndroidVoiceBridge`; this provider is SYNC — no async_runtime):
```rust
/// Host-implemented per-op Git credential provider (spec: per-op credential FFI).
/// Called synchronously inside libgit2's credentials callback, once per network
/// op — the host fetches the secret (e.g. from the Android Keystore) on demand so
/// no plaintext secret is held resident in the engine between ops.
#[cfg_attr(feature = "uniffi", uniffi::export(callback_interface))]
pub trait AndroidGitCredentialProvider: Send + Sync {
    /// HTTPS token (PAT), or `None` for anonymous/public remotes.
    fn https_token(&self) -> Option<String>;
    /// SSH private-key passphrase, or `None` if the key is unencrypted.
    fn ssh_passphrase(&self) -> Option<String>;
}

#[cfg(feature = "uniffi")]
struct AndroidGitCredentialProviderBridge {
    inner: Box<dyn AndroidGitCredentialProvider>,
}
#[cfg(feature = "uniffi")]
impl tool_api::GitCredentialProvider for AndroidGitCredentialProviderBridge {
    fn https_token(&self) -> Option<String> { self.inner.https_token() }
    fn ssh_passphrase(&self) -> Option<String> { self.inner.ssh_passphrase() }
}
```

- [ ] **Step 2:** Modify `AndroidGitConfigFfi`: REMOVE `pub https_token: Option<String>` and `pub ssh_passphrase: Option<String>` (the secrets now come from the provider, not the config struct). Keep `enable_git`, `workspace_root`, `ca_cert_dir`, `ssh_private_key_path`, `ssh_public_key_path`, `ssh_known_hosts_sha256_hex`.

- [ ] **Step 3:** `build_android_engine`: add a parameter `git_credential_provider: Option<Box<dyn AndroidGitCredentialProvider>>` (place it adjacent to the `git: Option<AndroidGitConfigFfi>` param). In the git mapping (`cfg.android_git_secret = Some(AndroidGitSecret { ... })`):
  - build the provider: `let credential_provider: Option<std::sync::Arc<dyn tool_api::GitCredentialProvider>> = git_credential_provider.map(|p| std::sync::Arc::new(AndroidGitCredentialProviderBridge { inner: p }) as std::sync::Arc<dyn tool_api::GitCredentialProvider>);`
  - set `credential_provider: credential_provider.clone()` (or move) into `AndroidGitSecret`; REMOVE the `token:`/`ssh_passphrase:` fields from the struct literal (they no longer exist).
  - `has_token`: where `AndroidGitToolCtx { ... has_token: ... }` is built, set `has_token: credential_provider.is_some()` (provider configured) instead of `c.https_token.is_some()`.
  - The `#[cfg(not(target_os="android"))]` arm of `build_android_engine` must add `git_credential_provider` to its `let _ = (...)` ignore tuple.

- [ ] **Step 4:** Fix the P2 acceptance probe `android_git_probe` (it builds an `AndroidGitSecret { token: None, ca_dir: ... }`): change to `AndroidGitSecret { credential_provider: None, ca_dir: ..., ..Default::default() }` (no token field). Fix any other `AndroidGitSecret`/`AndroidGitConfigFfi` construction site the compiler flags (tests, etc.) to the new shapes.

- [ ] **Step 5:** Build + checks:
```bash
cd lingxi-code
cargo test -p android-aar 2>&1 | tail -15
cargo check --workspace 2>&1 | tail -15
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cargo ndk -t arm64-v8a check -p android-aar 2>&1 | tail -10
```
All clean (the UniFFI scaffolding compiles the new callback_interface for the android target).

- [ ] **Step 6: Commit** `git add -A && git commit -m "feat(android-aar): AndroidGitCredentialProvider UniFFI callback + bridge; per-op provider wiring (P3)"`

---

# Phase P4 — gate

### Task 4: workspace gate + PENDING-DEVICE note

**Files:** Modify `docs/superpowers/plans/2026-06-14-android-git-credential-provider.md` (append the runbook note).

- [ ] **Step 1: Workspace test.** `cargo test --workspace 2>&1 | tail -40`. Known non-regressions: `tool-shell` powershell/cwd_persistence flakes; `platform-posix mcp_stdio` needs `cargo build -p mock_stdio_mcp` then re-run. Triage: a real `tool-api`/`tool-git-mobile`/`android-aar` failure = BLOCKED; known flake = note. Report counts.

- [ ] **Step 2: Clippy.** `cargo clippy --workspace --all-targets -- -D warnings` + `export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973 && cargo ndk -t arm64-v8a clippy -p tool-api -p tool-git-mobile -p android-aar -- -D warnings`. Both clean (the pre-existing `git2-rs autolib` manifest warning is the only allowed one).

- [ ] **Step 3: Both-ABI build.** `cargo ndk -t arm64-v8a build -p android-aar && cargo ndk -t x86_64 build -p android-aar`. Both link (no native change — pure Rust/UniFFI, so this should be quick relative to a crypto rebuild).

- [ ] **Step 4: PENDING-DEVICE note.** Append `## Device acceptance (PENDING-DEVICE)` to this plan: extend P4/G7's `android_git_probe` with a credential-provider case — a Kotlin `AndroidGitCredentialProvider` backed by the Keystore supplies the token per op; an HTTPS clone/fetch authenticates via the per-op `https_token()` (assert the provider's method is invoked and the authed op succeeds), and an SSH op uses `ssh_passphrase()`. Note the host-side evidence: `select_credential` unit tests prove the per-op fetch invokes the right provider method; the live Keystore round-trip is device-only.

- [ ] **Step 5: Commit** `git add -A && git commit -m "chore: per-op credential provider — workspace gate + PENDING-DEVICE runbook (P4)"`

---

## Self-review / spec coverage

- CP1 secrets-only via provider; non-secrets resident: Task 1 (`credential_provider` field, ca_dir/paths/hashes kept) + Task 2 (`select_credential` fetches token/passphrase from provider; SshConfig keeps non-secret fields). ✓
- CP2 replace resident token/passphrase, no fallback: Task 1 removes the fields; Task 3 removes the FFI inputs; no fallback path. ✓
- CP3 zeroization deferred: no `zeroize` dep added anywhere. ✓
- CP4 sync provider: trait methods are sync; called inside the sync libgit2 closure (Task 2/3). ✓
- Host-key verification (G7) unchanged: Task 2 leaves the `certificate_check` block untouched. ✓
- has_token → provider-present: Task 1 (doc) + Task 3 (`has_token: credential_provider.is_some()`). ✓
- Host-testable via mock + select_credential: Task 1/2 mock-driven tests. ✓
- deny/forbid(unsafe_code): no new unsafe (UniFFI bridge is safe). ✓

## Risks

- **Downstream construction sites:** removing `AndroidGitSecret.token`/`ssh_passphrase` + `AndroidGitConfigFfi.https_token`/`ssh_passphrase` breaks every construction site until P3 — `cargo check --workspace` may be red between P1 and P3 (expected; P3 closes it). Each task's own crate compiles+tests; the workspace is green by P4.
- **`Option<Arc<dyn T>>::as_deref()`** → `Option<&dyn T>`: relies on `Arc<dyn T>: Deref<Target=dyn T>` (it is). If a borrow/lifetime issue arises in the closure capture, capture the `&'a dyn T` from `NetCallbacks` (already `'a`-bound) — same pattern as the prior borrowed token.
- **UniFFI sync callback_interface:** the other callbacks are async (tokio); this one is sync. Confirm UniFFI renders a sync `callback_interface` method (it does). The git op runs on `spawn_blocking`, so a sync host call is fine.
- **Live Keystore fetch device-only:** host covers `select_credential` + bridge via the mock; the real per-op Keystore round-trip is PENDING-DEVICE.
