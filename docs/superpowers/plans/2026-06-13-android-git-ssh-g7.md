# Android Git SSH Transport (G7) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add SSH transport to the in-process libgit2 Git tool so `git@…`/`ssh://…` URLs work for all network ops (clone/fetch/pull/push), authenticated by a host-supplied SSH key file + in-memory passphrase, with strict host-key verification.

**Architecture:** Vendor `libssh2-sys` + its bundled libssh2 C into `third_party/` and build libgit2 with the `ssh` feature (libssh2 over the already-vendored OpenSSL). SSH is a transport, so it's enabled in the shared `auth.rs` network-callbacks builder (an SSH-key credentials branch + a `certificate_check` host-key callback); `reject_ssh_url` is narrowed rather than duplicated per op.

**Tech Stack:** Rust workspace at `lingxi-code/`. Vendored `git2 0.21.0` / `libgit2-sys 0.18.5+1.9.4` (bundling libgit2 1.9.4) at `third_party/git2-rs/`; that libgit2-sys optionally depends on `libssh2-sys 0.3.0`. Android NDK 27.0.12077973 + `cargo-ndk` (`export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`). Crate `tool-git-mobile` is `#![deny(unsafe_code)]` (one audited carve-out in `auth.rs`).

**Spec:** `docs/superpowers/specs/2026-06-13-android-git-ssh-g7-design.md` (GS1-GS6).

**Predecessor:** P4 (Git tool, HTTPS) + G2 (push) on `main` (`fbc29d45`); this branch (`android-git-ssh-g7`) is cut from there. Git tool at `lingxi-code/tools/git-mobile/src/{lib.rs,ops.rs,auth.rs}`.

**Invariants:**
- `tool-git-mobile` stays `#![deny(unsafe_code)]` — SSH adds NO new unsafe (the lone `set_ssl_cert_dir` carve-out is the only one).
- Secrets in-memory only (passphrase, pinned host keys); never logged. The private key is an app-private FILE (GS2) whose path is validated to stay in the sandbox and is never used to read+log the bytes.
- Strict host-key verification (GS1): unknown/mismatched host → named error, never silently trusted.
- All third-party libs vendored into `third_party/` as committed dirs (G4) — including the new libssh2-sys.

**Host-key wire format (pinned here):** pinned host keys are **lowercase-hex SHA-256** of the server host key. The `certificate_check` callback reads `cert.as_hostkey().hash_sha256` (32 raw bytes), hex-encodes them, and checks membership in the host-supplied set. (Host derives a pin via e.g. `ssh-keyscan -t ed25519 host` → take the key blob → `sha256` → hex. Revisitable to base64/`SHA256:` form later; the comparison helper is the unit-tested seam.)

---

## File structure

```text
third_party/
└── libssh2-sys/   CREATE (G7a): vendored libssh2-sys 0.3.0 crate + bundled libssh2 C
                    (committed real dir, like git2-rs). + LINGXI-PATCHES note.

lingxi-code/
├── tools/git-mobile/
│   ├── Cargo.toml   MODIFY (G7a): add "ssh" to the git2 feature list.
│   ├── auth.rs      MODIFY (G7b): SshConfig; ssh-key credentials branch;
│   │                 certificate_check host-key callback; hostkey-hex helper;
│   │                 a shared make_network_callbacks builder.
│   └── ops.rs       MODIFY (G7c): GitNetConfig.ssh; narrow reject_ssh_url;
│                     thread ssh into clone/fetch/pull/push option builders.
├── tool-api/src/builtin_context.rs  MODIFY (G7c): AndroidGitSecret ssh fields +
│                     redacting Debug (mask passphrase).
└── apps/android-aar/
    ├── Cargo.toml   MODIFY (G7a): confirm the android-aar→tool-git-mobile path
    │                 carries the ssh feature so the cdylib links libssh2.
    └── src/lib.rs   MODIFY (G7c): thread ssh secret fields into GitNetConfig;
                      gate SSH ops on ssh-config presence.
```

---

# Phase G7a — vendor libssh2 + enable ssh + NDK cross-build proof (THE GATE)

> Build-systems work; proof-gates, NOT strict TDD. **If the NDK cross-build of libssh2 fails after a bounded effort (≤~6 build-fix iterations across host+NDK), STOP and report BLOCKED with the exact errors — do NOT try alternate libssh2 versions / cmake / wolfSSL (GS3).** G7b-d are blocked until G7a is green.

### Task 1: vendor `libssh2-sys` + repoint the libgit2-sys dependency

**Files:** Create `third_party/libssh2-sys/`; Modify `third_party/git2-rs/libgit2-sys/Cargo.toml`, `third_party/git2-rs/LINGXI-PATCHES.md`.

- [ ] **Step 1: Preflight.** `export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973`. READ `third_party/git2-rs/LINGXI-PATCHES.md` (the re-vendor discipline + the rustc-1.82 `str::from_utf8` patch class) and `third_party/git2-rs/libgit2-sys/Cargo.toml` (the `[dependencies.libssh2-sys] version = "0.3.0", optional = true` line and the `ssh`/`vendored`/`vendored-openssl` features). Confirm `libssh2-sys` is NOT in `lingxi-code/Cargo.lock` yet (`grep -c libssh2-sys lingxi-code/Cargo.lock` → 0).

- [ ] **Step 2: Fetch the exact crate source.** Obtain `libssh2-sys 0.3.0` source (it bundles the libssh2 C library under `libssh2/`). Use cargo's vendoring/registry cache — e.g. `cargo download`-style: the source is at `~/.cargo/registry/src/*/libssh2-sys-0.3.0/` after a `cargo fetch` that pulls it. To force the fetch, temporarily add `libssh2-sys = "0.3.0"` to a scratch crate or run a build with the ssh feature enabled (Step 4 will do this) — but for vendoring, copy the registry source dir to `third_party/libssh2-sys/` and remove any `.cargo-ok`/checksum files. Confirm `third_party/libssh2-sys/libssh2/` (the bundled C) and `third_party/libssh2-sys/build.rs` exist.

- [ ] **Step 3: Repoint the dependency to the vendored path.** In `third_party/git2-rs/libgit2-sys/Cargo.toml`, change the libssh2-sys dependency from the registry version to the vendored path:
```toml
[dependencies.libssh2-sys]
path = "../../libssh2-sys"
optional = true
```
(Relative to `third_party/git2-rs/libgit2-sys/` → `third_party/libssh2-sys/`.) Keep `optional = true`. Add a note to `third_party/git2-rs/LINGXI-PATCHES.md` under a new "## libssh2-sys (G7a)" heading: vendored `libssh2-sys 0.3.0` into `third_party/libssh2-sys`, repointed the libgit2-sys dep to that path; re-apply on any re-vendor; libssh2 built with the openssl crypto backend (shares the `vendored-openssl` OpenSSL).

- [ ] **Step 4: Commit the vendored crate.**
```bash
git add third_party/libssh2-sys third_party/git2-rs/libgit2-sys/Cargo.toml third_party/git2-rs/LINGXI-PATCHES.md
git commit -m "vendor(libssh2-sys): bundle libssh2-sys 0.3.0 + libssh2 C; repoint libgit2-sys dep (G7a)"
```

### Task 2: enable the `ssh` feature + HOST build proof

**Files:** Modify `lingxi-code/tools/git-mobile/Cargo.toml`; verify `lingxi-code/apps/android-aar/Cargo.toml`.

- [ ] **Step 1: Add `ssh` to the git2 features.** In `lingxi-code/tools/git-mobile/Cargo.toml`, the git2 dep currently lists `["vendored-libgit2", "vendored-openssl", "https"]`. Add `"ssh"`:
```toml
git2 = { path = "../../../third_party/git2-rs/git2", default-features = false, features = [
    "vendored-libgit2",
    "vendored-openssl",
    "https",
    "ssh",
] }
```
Update the adjacent comment to note `ssh` enables the libssh2 transport (built against the vendored OpenSSL). Confirm `apps/android-aar/Cargo.toml`'s dependency on `tool-git-mobile` doesn't disable default/SSH (it depends on the crate, which now always carries ssh — no per-consumer feature gating needed; verify there's no `default-features = false` stripping it).

- [ ] **Step 2: HOST build proof (catch libssh2 C build errors before cross-compiling).**
```bash
cd lingxi-code
cargo build -p tool-git-mobile 2>&1 | tail -30
```
Expected: libssh2-sys compiles its bundled C (against the vendored OpenSSL), libgit2-sys recompiles with `GIT_SSH`, the crate builds. If the libssh2 `cc` build fails on the host (missing OpenSSL include wiring, etc.), fix the dependency/feature wiring here first (host failures are cheaper to debug than NDK). Record what was needed.

- [ ] **Step 3: Verify `GIT_SSH` is compiled in (host).** Add a temporary check or inspect the build: confirm `libgit2-sys` saw `CARGO_FEATURE_SSH` (its build.rs emits `#define GIT_SSH 1` / `GIT_SSH_LIBSSH2 1`). A simple runtime proof: a unit test that an `ssh://` URL no longer fails with "unsupported URL protocol" but with a connection/auth error (deferred to G7c — for now the build linking is the proof).

- [ ] **Step 4: Commit.**
```bash
git add lingxi-code/tools/git-mobile/Cargo.toml lingxi-code/Cargo.lock
git commit -m "build(tool-git-mobile): enable git2 ssh feature (libssh2 over vendored OpenSSL) — host build (G7a)"
```

### Task 3: NDK cross-build proof — THE make-or-break gate

**Files:** none (build verification + a commit of any lockfile/wiring fix).

- [ ] **Step 1: Cross-build both ABIs.**
```bash
export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973
cd lingxi-code
cargo ndk -t arm64-v8a build -p android-aar 2>&1 | tail -40
cargo ndk -t x86_64 build -p android-aar 2>&1 | tail -40
```
Expected: libssh2-sys's bundled C cross-compiles under the NDK clang (against the cross-built vendored OpenSSL), libgit2 links libssh2, and `android-aar` produces its cdylib for both ABIs. **This is the gate.**

- [ ] **Step 2: If the NDK build fails** — apply the SAME class of fixes the P4a/P5a cross-builds needed (the known traps): NDK clang target/API-level (cargo-ndk may default the API below libssh2/bionic needs — the P0a lesson was to pin API 29+; if libssh2's `cc` invocation picks a low `--target=<triple><api>`, set it to 29), OpenSSL include dir wiring (`DEP_OPENSSL_INCLUDE` must reach libssh2-sys's build — it's normally exported by openssl-sys; confirm the vendored-openssl path exports it), and BSD-vs-GNU build-script tool issues (the libcap/toybox trap — libssh2-sys uses `cc`, not make/sed, so this is unlikely). Budget ≤~6 iterations total across Tasks 2-3. **If still failing, STOP: report BLOCKED with the exact compiler/linker errors and which fixes were tried — do NOT switch libssh2 versions, cmake, or crypto backends (GS3).**

- [ ] **Step 3: Verify the link (not just compile).** Confirm libssh2 symbols are in the cdylib (e.g. `llvm-nm`/`llvm-readelf` on `target/aarch64-linux-android/debug/liblingxi_android.so` — adjust to the actual cdylib name — greps a libssh2 symbol like `libssh2_session_init`, OR confirm the build emitted the libssh2 build dir). Record the cdylib size delta vs the pre-ssh baseline (libssh2 is small relative to libgit2/openssl).

- [ ] **Step 4: Commit (any wiring/lockfile fix; else a no-op marker commit is unnecessary).**
```bash
git add -A
git commit -m "build(android-aar): libgit2+libssh2 cross-compiles both ABIs — G7a gate GREEN" || echo "nothing to commit (already green from Task 2)"
```
**If BLOCKED:** do not commit a false-green; report and stop.

---

# Phase G7b — auth.rs SSH credentials + host-key verification (host TDD)

### Task 4: `SshConfig`, SSH-key credentials branch, host-key `certificate_check`

**Files:** Modify `lingxi-code/tools/git-mobile/src/auth.rs`.

**API notes (git2 0.21 — verify the exact signatures against the vendored `third_party/git2-rs/git2/src/` and adapt if they differ, as G2 did for `Reference::shorthand`):**
- `git2::Cred::ssh_key(username: &str, publickey: Option<&Path>, privatekey: &Path, passphrase: Option<&str>) -> Result<Cred, Error>`.
- `git2::Cred::ssh_key_from_agent` / `username` exist; the credentials callback's `allowed_types: CredentialType` may include `SSH_KEY` and (separately) `USERNAME`.
- `RemoteCallbacks::certificate_check(cb)` where `cb: FnMut(&git2::Cert<'_>, &str) -> Result<git2::CertificateCheckStatus, git2::Error>`; `CertificateCheckStatus::{CertificateOk, CertificatePassthrough}`.
- `git2::Cert::as_hostkey() -> Option<&git2::CertHostkey<'_>>`; `CertHostkey::hash_sha256() -> Option<&[u8]>` (or a field — verify).

- [ ] **Step 1: Failing tests** in auth.rs's `#[cfg(test)] mod tests`:
```rust
    #[test]
    fn hostkey_hex_matches_pinned() {
        // 32-byte sha256 -> lowercase hex; membership check against pinned set.
        let raw = [0xABu8; 32];
        let hex = hostkey_sha256_hex(&raw);
        assert_eq!(hex.len(), 64);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        let pinned = vec![hex.clone()];
        assert!(host_key_is_pinned(&raw, &pinned), "exact hex match accepted");
        let other = [0x00u8; 32];
        assert!(!host_key_is_pinned(&other, &pinned), "unknown key rejected");
        // case-insensitive on the pinned side (host may supply uppercase):
        let pinned_upper = vec![hex.to_uppercase()];
        assert!(host_key_is_pinned(&raw, &pinned_upper), "pinned hex compared case-insensitively");
    }

    #[test]
    fn validate_ssh_key_path_rejects_outside_sandbox() {
        let sandbox = tempfile::tempdir().unwrap();
        let key = sandbox.path().join("id_ed25519");
        std::fs::write(&key, b"-----BEGIN OPENSSH PRIVATE KEY-----\n").unwrap();
        // inside the sandbox + exists -> ok
        validate_ssh_key_path(key.to_str().unwrap(), sandbox.path())
            .expect("in-sandbox key path accepted");
        // nonexistent -> error
        let missing = sandbox.path().join("nope");
        assert!(validate_ssh_key_path(missing.to_str().unwrap(), sandbox.path()).is_err());
        // traversal outside the sandbox -> error
        let outside = sandbox.path().join("../escape");
        assert!(validate_ssh_key_path(outside.to_str().unwrap(), sandbox.path()).is_err());
    }

    #[test]
    fn make_network_callbacks_assembles_for_https_and_ssh() {
        // HTTPS-only: token present, no ssh config -> assembles without panic.
        let _cb = make_network_callbacks(&NetCallbacks { token: Some("tok"), ssh: None });
        // SSH: ssh config present -> assembles without panic (cred + host-key
        // callbacks installed). We cannot invoke libgit2's private dispatch in
        // isolation, so this only asserts the assembly path does not panic.
        let ssh = SshConfig {
            private_key_path: "/sandbox/id".into(),
            known_hosts_sha256_hex: vec!["abc".into()],
            ..Default::default()
        };
        let _cb2 = make_network_callbacks(&NetCallbacks { token: None, ssh: Some(&ssh) });
    }
```
(The load-bearing tests are `hostkey_hex_matches_pinned` and `validate_ssh_key_path_rejects_outside_sandbox`, which test pure helpers with no live SSH; the assembly test just guards the builder against panics.)

- [ ] **Step 2: Run → FAIL** (`cargo test -p tool-git-mobile auth::tests` — unresolved helpers).

- [ ] **Step 3: Implement** in auth.rs:
```rust
use sha2::{Digest, Sha256};

/// SSH transport config: a host-supplied private-key FILE path (GS2), an
/// optional in-memory passphrase, and the pinned server host keys (lowercase-hex
/// SHA-256) for strict verification (GS1).
#[derive(Clone, Default)]
pub struct SshConfig {
    pub private_key_path: String,
    pub public_key_path: Option<String>,
    pub passphrase: Option<String>,
    pub known_hosts_sha256_hex: Vec<String>,
}

/// Lowercase-hex SHA-256 of a host key's raw sha256 hash bytes.
#[must_use]
pub fn hostkey_sha256_hex(sha256: &[u8]) -> String {
    sha256.iter().map(|b| format!("{b:02x}")).collect()
}

/// True if `sha256` (raw host-key sha256 bytes) hex-matches any pinned entry
/// (compared case-insensitively, so the host may supply upper/lowercase hex).
#[must_use]
pub fn host_key_is_pinned(sha256: &[u8], pinned_hex: &[String]) -> bool {
    let hex = hostkey_sha256_hex(sha256);
    pinned_hex.iter().any(|p| p.eq_ignore_ascii_case(&hex))
}

/// Validate the SSH private-key path: it must exist and canonicalize to inside
/// the app sandbox root (no `..`/symlink escape). Returns the validated path.
pub fn validate_ssh_key_path(path: &str, sandbox_root: &std::path::Path) -> Result<std::path::PathBuf, GitOpError> {
    let canonical_root = sandbox_root.canonicalize().map_err(|e| {
        GitOpError::NotFound(format!("sandbox root {}: {e}", sandbox_root.display()))
    })?;
    let p = std::path::Path::new(path).canonicalize().map_err(|e| {
        GitOpError::InvalidInput(format!("ssh key path {path}: {e}"))
    })?;
    if !p.starts_with(&canonical_root) {
        return Err(GitOpError::InvalidInput(format!(
            "ssh key path escapes the app sandbox: {path}"
        )));
    }
    Ok(p)
}
```
Then add the network-callbacks builder that installs BOTH the HTTPS token branch AND (when an `SshConfig` with a key is present) the SSH-key credentials branch + the host-key `certificate_check`. Concrete shape (adapt `CredentialType`/`Cert` API to the vendored git2 0.21):
```rust
/// Network parameters passed to the shared callbacks builder.
pub struct NetCallbacks<'a> {
    pub token: Option<&'a str>,
    pub ssh: Option<&'a SshConfig>,
}

/// Build the RemoteCallbacks shared by all network ops, with the HTTPS-token
/// credentials branch and, when an SshConfig is present, an SSH-key credentials
/// branch + strict host-key verification.
pub fn make_network_callbacks<'a>(p: &'a NetCallbacks<'a>) -> git2::RemoteCallbacks<'a> {
    let mut callbacks = git2::RemoteCallbacks::new();
    let token = p.token;
    let ssh = p.ssh;
    callbacks.credentials(move |_url, username_from_url, allowed| {
        if allowed.contains(git2::CredentialType::USERNAME) {
            // libgit2 first asks for the username on some SSH servers.
            let user = username_from_url.unwrap_or("git");
            return git2::Cred::username(user);
        }
        if allowed.contains(git2::CredentialType::SSH_KEY) {
            if let Some(s) = ssh {
                let user = username_from_url.unwrap_or("git");
                return git2::Cred::ssh_key(
                    user,
                    s.public_key_path.as_deref().map(std::path::Path::new),
                    std::path::Path::new(&s.private_key_path),
                    s.passphrase.as_deref(),
                );
            }
        }
        if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            if let Some(t) = token {
                return git2::Cred::userpass_plaintext("x-access-token", t);
            }
        }
        Err(git2::Error::from_str("no usable git credentials configured"))
    });
    if let Some(s) = ssh {
        let pinned = s.known_hosts_sha256_hex.clone();
        callbacks.certificate_check(move |cert, _host| {
            if let Some(hk) = cert.as_hostkey() {
                if let Some(sha) = hk.hash_sha256() {
                    if host_key_is_pinned(sha, &pinned) {
                        return Ok(git2::CertificateCheckStatus::CertificateOk);
                    }
                }
                return Err(git2::Error::from_str(
                    "unknown or mismatched SSH host key (not in pinned known_hosts)",
                ));
            }
            // Non-hostkey cert on an SSH transport: defer to libgit2's default.
            Ok(git2::CertificateCheckStatus::CertificatePassthrough)
        });
    }
    callbacks
}
```
Keep `install_token_credentials` for the HTTPS-only fetch path OR refactor `make_fetch_options` to delegate to `make_network_callbacks` (preferred DRY — but if lifetimes fight, keep both and have `make_network_callbacks` be the SSH-capable one). Add `sha2` + the test's `tempfile` dev-dep if not present (sha2 is used elsewhere in the workspace; check `tool-git-mobile/Cargo.toml`).

- [ ] **Step 4: Run → PASS** (`cargo test -p tool-git-mobile auth::tests`) + `cargo clippy -p tool-git-mobile --all-targets -- -D warnings`. **Commit** `feat(tool-git-mobile): SSH-key credentials + strict host-key verification helpers (G7b)`.

---

# Phase G7c — ops/lib/android-aar threading + gate (host TDD)

### Task 5: thread SshConfig through GitNetConfig + narrow reject_ssh_url + secret seam + gate

**Files:** Modify `lingxi-code/tools/git-mobile/src/ops.rs`, `lingxi-code/tool-api/src/builtin_context.rs`, `lingxi-code/apps/android-aar/src/lib.rs`.

- [ ] **Step 1: Failing tests.**
  - In `ops.rs`: `reject_ssh_url` is narrowed — add a helper `ssh_allowed(url, ssh: Option<&SshConfig>) -> Result<(), GitOpError>` that returns Ok for an SSH URL when `ssh.is_some()`, and the existing SSH-rejection error when `ssh.is_none()`; non-SSH URLs always Ok. Test both branches.
  ```rust
      #[test]
      fn ssh_url_allowed_only_with_ssh_config() {
          let ssh = SshConfig { private_key_path: "/x/id".into(), ..Default::default() };
          assert!(ssh_allowed("git@github.com:o/r.git", Some(&ssh)).is_ok());
          let err = ssh_allowed("git@github.com:o/r.git", None).unwrap_err();
          assert!(matches!(err, GitOpError::InvalidInput(ref m) if m.to_lowercase().contains("ssh")));
          assert!(ssh_allowed("https://github.com/o/r.git", None).is_ok(), "https unaffected");
      }
  ```
  - In `builtin_context.rs`: `AndroidGitSecret` gains `ssh_private_key_path: Option<String>`, `ssh_public_key_path: Option<String>`, `ssh_passphrase: Option<String>`, `ssh_known_hosts_sha256_hex: Vec<String>`; the redacting `Debug` masks `ssh_passphrase` (like `token`). Test the Debug masks the passphrase.
  ```rust
      #[test]
      fn android_git_secret_debug_redacts_ssh_passphrase() {
          let s = AndroidGitSecret { ssh_passphrase: Some("hunter2".into()), ..Default::default() };
          let dbg = format!("{s:?}");
          assert!(!dbg.contains("hunter2"), "passphrase must be redacted: {dbg}");
      }
  ```

- [ ] **Step 2: Run → FAIL.**

- [ ] **Step 3: Implement.**
  - `ops.rs`: add `pub ssh: Option<SshConfig>` to `GitNetConfig` (re-export `SshConfig` from `auth`). Replace each `reject_ssh_url(url)?` call site in clone/fetch/push (and the remote-URL check) with `ssh_allowed(url, net.ssh.as_ref())?`. Build the network options via `auth::make_network_callbacks(&auth::NetCallbacks { token: net.token.as_deref(), ssh: net.ssh.as_ref() })` wrapped in `FetchOptions`/`PushOptions` (replace the token-only `make_fetch_options`/inline push callbacks so SSH flows everywhere). For the key-path validation: when `net.ssh` is some, validate `private_key_path` against the workspace/app-sandbox root before the network call (use the repo/workspace root already available in each op) → `InvalidInput` on escape.
  - `builtin_context.rs`: add the 4 ssh fields to `AndroidGitSecret`; extend the manual `Debug` to mask `ssh_passphrase` (`.field("ssh_passphrase", &self.ssh_passphrase.as_ref().map(|_| "<redacted>"))`) and show the rest.
  - `lib.rs` (`git_net_config`): populate `GitNetConfig.ssh` from the secret seam — `Some(SshConfig { ... })` when `ssh_private_key_path` is present, else `None`. The network gate: an SSH-URL op with no ssh config surfaces the narrowed `ssh_allowed` error; HTTPS still gated on token. (No registration-gate change — missing SSH creds disable only SSH ops, mirroring missing-token.)
  - `apps/android-aar/src/lib.rs`: extend `AndroidGitConfigFfi`/the secret mapping with the ssh fields (key path, public key path, passphrase, known-hosts list) from the Kotlin host; thread them into `AndroidGitSecret`. Default to None/empty so non-SSH setups are unaffected (update all construction sites).

- [ ] **Step 4: Run → PASS** + `cargo test -p tool-git-mobile -p tool-api -p android-aar` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo check --workspace`. **Commit** `feat(tool-git-mobile,android-aar): thread SshConfig + narrow SSH rejection + secret seam (G7c)`.

---

# Phase G7d — device acceptance + final gate

### Task 6: final gate + PENDING-DEVICE runbook

**Files:** Modify `docs/superpowers/plans/2026-06-13-android-git-ssh-g7.md` (append runbook).

- [ ] **Step 1: Workspace test.** `cargo test --workspace 2>&1 | tail -40`. Known non-regressions (NOT failures): `tool-shell` `powershell`/`cwd_persistence` parallel-isolation flakes; `platform-posix mcp_stdio` needs `cargo build -p mock_stdio_mcp` then re-run. A real `tool-git-mobile`/`android-aar` failure = BLOCKED.

- [ ] **Step 2: Clippy.** `cargo clippy --workspace --all-targets -- -D warnings` and `export ANDROID_NDK_HOME=~/Library/Android/sdk/ndk/27.0.12077973 && cargo ndk -t arm64-v8a clippy -p tool-git-mobile -p android-aar -- -D warnings`. Both clean (the pre-existing `git2-rs autolib` manifest warning is the only allowed one).

- [ ] **Step 3: Both-ABI build + size delta.** `cargo ndk -t arm64-v8a build -p android-aar` and `-t x86_64`. Record the cdylib size delta from adding libssh2 (per ABI).

- [ ] **Step 4: Record the PENDING-DEVICE runbook** — append `## Device acceptance (PENDING-DEVICE)` to this plan capturing: extend P4's env-gated `android_git_probe` (in `apps/android-aar/src/lib.rs`) with an SSH case — `clone git@…`/`ssh://…` with a host-supplied key file + a correct pinned host key (sha256 hex) → succeeds; a WRONG/absent pinned host key → fails closed with the named host-key error; an SSH push round-trip. Gate behind a test-only key+host-key env, exactly as P4's clone probe. Additive; NOT required for the host-merge gate. Also note the residual: the live SSH handshake + host-key callback firing is exercised only on-device (host tests cover the pure helpers + assembly + gate).

- [ ] **Step 5: Commit.** `git add -A && git commit -m "chore(tool-git-mobile): G7 SSH gate — workspace + android-target clean; PENDING-DEVICE runbook"`.

---

## Self-review / spec coverage

- GS1 strict host-key verify: Task 4 `host_key_is_pinned` + `certificate_check` callback → named error on mismatch; tests `hostkey_hex_matches_pinned`. ✓
- GS2 key as file path + in-memory passphrase + path validation: Task 4 `validate_ssh_key_path` + `Cred::ssh_key`; Task 5 validates at the op; passphrase in-memory via the secret seam. ✓
- GS3 build gate BLOCKED-on-fail: G7a Task 3 Step 2 (bounded budget, no rat-holing). ✓
- GS4 host tests + PENDING-DEVICE: Tasks 4-5 host-test the pure helpers/assembly/gate; Task 6 records the device runbook. ✓
- GS5 SSH covers all network ops, reject_ssh_url narrowed: Task 5 `ssh_allowed` replaces per-op rejection; `make_network_callbacks` used by clone/fetch/pull/push. ✓
- GS6 in-memory secrets + redacting Debug + deny(unsafe): Task 5 `AndroidGitSecret` ssh fields + masked Debug; no new unsafe in any task. ✓
- G4 vendoring: G7a Task 1 vendors libssh2-sys into third_party/. ✓
- Host-key wire format pinned (lowercase-hex SHA-256): stated in the header + Task 4. ✓

## Risks

- **libssh2 NDK cross-compile (G7a, the gate):** front-loaded; openssl already cross-built (P4a); host build proof (Task 2) catches C errors before the NDK. Bounded budget, then BLOCKED (GS3).
- **git2 0.21 SSH API drift:** `Cred::ssh_key` / `certificate_check` / `Cert::as_hostkey` / `CertificateCheckStatus` signatures verified against the vendored source; adapt as G2 did for `shorthand()`. The pure helpers (`host_key_is_pinned`, `validate_ssh_key_path`) are signature-independent and fully host-tested.
- **Private key on disk (GS2):** mitigated by sandbox-path validation + host-owned at-rest protection + in-memory passphrase; documented departure.
- **Live SSH not host-testable:** host covers pure helpers + callback assembly + gate; the handshake/host-key-callback is PENDING-DEVICE (GS4).

---

## Device acceptance (PENDING-DEVICE)

The G7 host-merge gate (workspace tests + workspace clippy + android-target clippy for
`tool-git-mobile`/`android-aar` + both-ABI `android-aar` link) is GREEN. The live SSH
handshake and the `certificate_check` host-key callback firing against a real server can
only be exercised on a device/emulator with network egress, so they are deferred. The
steps below are **additive** and are **NOT required** for the host-merge gate.

### (a) SSH device runbook

Extend the existing P4 env-gated `android_git_probe` UniFFI path in
`apps/android-aar/src/lib.rs` (the same shape as P4's clone probe — test-only, gated
behind env so it never fires in a normal build) with an SSH case:

1. **Setup (host-supplied, app-private storage).**
   - Place the test private key file in app-private storage (e.g.
     `<filesDir>/ssh/id_ed25519`, mode 0600). Expose its path via a test-only env, e.g.
     `LINGXI_GIT_SSH_KEY_PATH`, exactly as P4's clone probe gates on a test-only env.
   - Supply the expected server host key as a **lowercase-hex SHA-256** fingerprint via
     a test-only env, e.g. `LINGXI_GIT_SSH_HOSTKEY_SHA256` (the wire format pinned in the
     header and validated by `host_key_is_pinned`).
   - Optionally an in-memory passphrase via the `AndroidGitSecret` ssh seam (never on disk).

2. **Clone — correct pinned host key (ACCEPT).** Drive `android_git_probe` to
   `clone git@<host>:<repo>` (or `ssh://git@<host>/<repo>`) with the key path + the
   **correct** pinned host-key SHA-256. Expected: clone succeeds; the `certificate_check`
   callback observes the server host key, `host_key_is_pinned` returns true, and
   `CertificateCheckStatus::CertificateOk` is returned.

3. **Clone — wrong OR absent pinned host key (REJECT, fail-closed).** Repeat with a
   **wrong** SHA-256, and again with an **empty/absent** pin list. Expected: clone fails
   closed with the named host-key error (mismatch / not-pinned) — never a silent
   trust-on-first-use accept. The empty-list case must fail closed (covered by the
   `host_key_is_pinned` empty-list host test, but here verified end-to-end through the
   live callback).

4. **Push round-trip.** With the correct pinned host key, `push` a commit to the SSH
   remote and confirm the remote ref advances (then reset). Exercises the shared
   `make_network_callbacks` credentials + host-key path on a write op (GS5).

All SSH cases stay behind the test-only key-path + host-key env, mirroring P4's clone
probe; with the env unset the probe is inert.

### (b) Residual-risk note

The host test suite covers, with no live network:

- the pure helpers — `host_key_is_pinned` (including the **empty-list fail-closed** case)
  and `validate_ssh_key_path`;
- the credentials + host-key **callback assembly** in `make_network_callbacks`
  (construction only — no live dispatch); and
- the `ssh_allowed` gate (the narrowed replacement for per-op `reject_ssh_url`).

What is exercised **only on-device** is the live SSH handshake and the
`certificate_check` callback **actually firing** against a real server host key.
Therefore the device run **MUST** include **both**:

- the **correct-host-key** case (ACCEPT), and
- the **wrong/absent-host-key** case (REJECT, fail-closed with the named host-key error).

Until that on-device run is completed and recorded, strict host-key verification is
proven by construction + unit tests but not by a live handshake.
