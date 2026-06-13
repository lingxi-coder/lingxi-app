# Android Git Tool — SSH Transport (G7) Design

Date: 2026-06-13
Status: Approved design (brainstormed + decisions locked)
Part of: the Android Git tool effort (spec `2026-06-13-android-git-design.md`,
§G7 = "SSH"). P4 shipped HTTPS-only and **rejected** `git@…`/`ssh://…` URLs
(`reject_ssh_url`); G2 added push. This spec lifts the **no-SSH** deferral (G7)
by building libgit2 with libssh2 and adding SSH-key auth + strict host-key
verification to the in-process Git tool.

## Summary

SSH is a new **transport** under the existing structured libgit2-in-process Git
tool. Because the transport is orthogonal to the operation set, enabling it
makes `git@…`/`ssh://…` URLs work for **all** network ops (clone/fetch/pull/push)
with no per-op changes — the credentials callback gains an SSH-key branch and a
host-key (`certificate_check`) callback is added; `reject_ssh_url` is narrowed to
"reject SSH only when no SSH config is present."

The **make-or-break** part is the build (G7a): libgit2 must be compiled with
libssh2, which is **not** currently linked (P4 was HTTPS-only). G7a vendors
`libssh2-sys` + its bundled libssh2 C into `third_party/` (G4), enables the
`git2`/`libgit2-sys` `ssh` feature for the Android build, and cross-compiles
**libgit2 + libssh2 + openssl** under the NDK. If libssh2 cannot cross-compile
under the NDK after a bounded effort, **G7 is BLOCKED** and reported — G7b–d
depend entirely on G7a.

## Decision log (this G7 brainstorm, 2026-06-13)

| # | Decision | Choice |
|---|----------|--------|
| GS1 | Host-key (server) verification | **Strict, host-supplied pinned keys.** The Kotlin host supplies the expected server host key(s) in-memory; libgit2's `certificate_check` callback verifies the presented host key against them. Unknown/mismatched host → named error (fail-closed). No CA chain exists for SSH, so this is the only MITM defense. No accept-any, no TOFU (the in-process tool has no persistent known_hosts). |
| GS2 | SSH private-key supply | **File path on device.** The host places the private key in app-private storage and supplies its path; libgit2 `Cred::ssh_key(user, pubkey?, privkey_path, passphrase?)`. The **passphrase** (a secret) is supplied **in-memory**; the key file's at-rest protection is the host's responsibility. The path is validated (exists, inside the app sandbox) and never logged. (Deliberate departure from the HTTPS token's "never to disk" rule — chosen because SSH keys are conventionally managed as app-private files.) |
| GS3 | Build-gate failure handling | **Stop at BLOCKED.** G7a is front-loaded; if libssh2-NDK cross-compile fails after a bounded effort (~the P4a/P5a budget), stop and report findings — do NOT rat-hole on alternate libssh2 versions / cmake / wolfSSL. |
| GS4 | Testing depth | **Host: callback assembly + host-key match/mismatch logic + key-path validation + the network gate (unit-level, no live SSH). The real SSH clone/push round-trip + live host-key verification is PENDING-DEVICE** (same posture as P4's HTTPS-clone). The build link (libgit2+libssh2 cross-compiles, both ABIs) is the host-verifiable G7a gate. |
| GS5 | Operation scope | SSH is a transport, so enabling it covers **all** network ops (clone/fetch/pull/push). No per-op work; `reject_ssh_url` is narrowed, not duplicated per op. |
| GS6 | Secret seam | Passphrase + pinned host keys are **in-memory** via the extended `AndroidGitSecret` (redacting `Debug`); the key path rides the same seam (non-secret but never logged). Crate stays `#![deny(unsafe_code)]`. |

## Goals

1. Build libgit2 with libssh2 (openssl backend) and cross-compile it for Android
   (the G7a gate). Vendor `libssh2-sys` into `third_party/` (G4).
2. Accept `git@…`/`ssh://…` URLs for all network ops, authenticated by a
   host-supplied SSH key (file path + in-memory passphrase).
3. Strictly verify the server host key against host-supplied pinned keys; unknown
   or mismatched host → named error (no MITM).
4. Keep the passphrase + pinned host keys in-memory (never disk/log); validate
   the key path stays inside the app sandbox; crate stays `deny(unsafe_code)`.

## Non-goals

1. No ssh-agent (unavailable on Android). No interactive
   keyboard-password SSH auth (key-based only).
2. No TOFU / no accept-any host key (GS1). No persistent known_hosts file.
3. No in-memory private key in v1 (GS2 chose file-path;
   `ssh_key_from_memory` remains a future option if the disk requirement is
   revisited). No SSH config-file parsing (`~/.ssh/config`).
4. No new operations — SSH only changes the transport for the existing
   clone/fetch/pull/push.
5. No rat-holing on the build (GS3): one bounded libssh2-NDK attempt, then BLOCKED.

## Architecture

```text
third_party/
├── git2-rs/                 (P4 — vendored git2 + libgit2-sys + bundled libgit2)
└── libssh2-sys/   ◀ G7a: vendored libssh2-sys crate + its bundled libssh2 C

lingxi-code/
├── tools/git-mobile/
│   ├── Cargo.toml   MODIFY: add "ssh" to the git2 feature list (pulls libssh2-sys,
│   │                 makes libgit2-sys compile transports/ssh_libssh2.c + GIT_SSH).
│   ├── auth.rs      MODIFY: SSH branch in the credentials callback + a
│   │                 certificate_check host-key callback; + SshConfig.
│   └── ops.rs       MODIFY: GitNetConfig gains `ssh: Option<SshConfig>`; narrow
│                     reject_ssh_url (reject SSH only when no ssh config).
├── tool-api/src/builtin_context.rs  MODIFY: AndroidGitSecret gains ssh fields
│                     (key path, passphrase, pinned host keys) + redacting Debug.
└── apps/android-aar/
    ├── Cargo.toml   MODIFY: ensure the android-aar→tool-git-mobile path carries
    │                 the ssh feature (so the cdylib links libssh2).
    └── src/lib.rs   MODIFY: thread the SSH secret fields from the FFI config into
                      GitNetConfig; the network gate accepts an op when a token
                      (HTTPS) OR an ssh config (SSH URL) is present.
```

- **Build (G7a — the gate).** `git2`'s `ssh` feature → `libgit2-sys/ssh` →
  `libssh2-sys`. `libgit2-sys/build.rs` already gates the libssh2 transport on
  `CARGO_FEATURE_SSH` (emits `GIT_SSH`, `GIT_SSH_LIBSSH2`,
  `GIT_SSH_LIBSSH2_MEMORY_CREDENTIALS`, reads `DEP_SSH2_INCLUDE`). `libssh2-sys`
  bundles libssh2 and builds it with the `cc` crate against the openssl we
  already vendor (`vendored-openssl`) + the existing `libz-sys`. The risk is the
  libssh2 `cc` build under the NDK (cross headers, openssl include wiring) — the
  same class of risk as the P4a libgit2 / P5a mksh cross-builds.
- **Transport is orthogonal.** Every network op already builds its callbacks via
  the shared `auth.rs` builder. SSH support lives entirely in those callbacks +
  the URL no longer being rejected, so clone/fetch/pull/push all gain SSH at once.
- **`deny(unsafe_code)` preserved** — the only unsafe stays the audited
  `set_ssl_cert_dir` carve-out (HTTPS CA path, unchanged).

### Auth + host-key data flow (SSH URL)

1. The op opens the remote with a `git@host:owner/repo.git` / `ssh://…` URL.
2. libgit2 requests credentials of type `SSH_KEY`. The credentials callback
   returns `Cred::ssh_key(username_from_url, pubkey_path?, &privkey_path,
   passphrase.as_deref())` — `username_from_url` is the SSH user (`git`), the
   key path comes from the SshConfig (validated: exists + inside the app
   sandbox), the passphrase is the in-memory secret (or `None`).
3. libgit2 connects and fires `certificate_check` with the server host key. The
   callback compares it against the host-supplied pinned host key(s); a match →
   proceed; unknown/mismatch → return an error → mapped to a named
   `GitOpError` ("unknown or mismatched SSH host key").
4. The op proceeds exactly as for HTTPS (same fetch/push logic, ff-only, etc.).

### Config / gating

- `SshConfig { private_key_path: String, public_key_path: Option<String>,
  passphrase: Option<String>, known_hosts: Vec<String> }` (host keys as
  OpenSSH `known_hosts`-style entries or base64 key blobs — exact wire form
  pinned in the plan). Lives inside `GitNetConfig.ssh: Option<SshConfig>`.
- `AndroidGitSecret` gains the SSH fields; its redacting `Debug` masks the
  passphrase (and omits the key bytes — only the path, which is non-secret, may
  appear, but is still not logged in normal flow).
- **Network gate:** a network op is allowed when EITHER a token is present
  (HTTPS) OR an `ssh` config is present (SSH). An SSH URL with no `ssh` config →
  the existing "git credentials not configured" style named error. The
  registration-level gate (`android_git_gate`) is unchanged (it already gates on
  enable + workspace + CA reachability; SSH adds no new registration conjunct —
  missing SSH creds disable only SSH ops, mirroring how a missing token disables
  only HTTPS network ops).

## Error handling (all named, fail-closed)

- libssh2 not built / SSH URL but transport unavailable → mapped `Libgit2`
  error (only possible if G7a regressed; the build gate prevents shipping it).
- SSH URL with no SSH config → "git SSH credentials not configured".
- Key path missing / outside the app sandbox → `InvalidInput` ("ssh key path
  invalid").
- Host key unknown / mismatched → named host-key-verification error
  (fail-closed; never silently trusted).
- Auth failure (bad key/passphrase) → mapped `Libgit2` auth error.

## Security

- **MITM defense:** strict host-key verification against host-supplied pinned
  keys (GS1). Unknown host fails closed.
- **Secrets:** passphrase + pinned host keys in-memory only (redacting `Debug`,
  never logged). The private key is an app-private file whose at-rest protection
  the host owns; the path is validated to stay inside the app sandbox and is
  never used to read+log the key bytes.
- **No new unsafe;** `deny(unsafe_code)` holds. libgit2/libssh2 are in-process
  (no exec, no argv, no env credential leak).

## Testing

Host tests (deterministic, no live SSH):
- credentials callback returns an `SSH_KEY` cred for an SSH URL and the existing
  `USER_PASS_PLAINTEXT` for HTTPS (assemble-path assertions, mirroring the
  existing `credentials_callback_yields_token` test).
- host-key verification logic: a match accepts, an unknown/mismatched key
  produces the named error (unit-test the comparison helper directly with
  sample known_hosts entries).
- key-path validation: a path outside the sandbox / nonexistent → `InvalidInput`.
- network gate: SSH URL + ssh config → allowed; SSH URL + no ssh config → named
  error; HTTPS still gated on token.
- `reject_ssh_url` narrowing: SSH URL is no longer rejected when an ssh config
  is present.

Build gate (host-verifiable): `cargo ndk -t arm64-v8a / x86_64 build -p
android-aar` links libgit2 **with** libssh2 for both ABIs; symbol/feature check
that `GIT_SSH` is compiled in.

Device acceptance (**PENDING-DEVICE**, like P4 clone): a real `git clone
git@…`/`ssh://…` with a host-supplied key + pinned host key, asserting the clone
succeeds and that a wrong/absent pinned host key fails closed; an SSH push.
Recorded as a turn-key runbook extending P4's `android_git_probe`.

## Phasing

- **G7a — vendor libssh2-sys + enable ssh + NDK cross-build proof (THE GATE).**
  Vendor `libssh2-sys` (+ bundled libssh2) into `third_party/`; add `ssh` to the
  git2 features for tool-git-mobile (and ensure android-aar carries it);
  cross-compile both ABIs; verify libssh2 links and `GIT_SSH` is enabled. BLOCKED
  on failure after a bounded effort (GS3). Host-only proof-gate (no live SSH).
- **G7b — auth.rs SSH credentials + host-key callback + SshConfig (host TDD).**
- **G7c — ops/lib/android-aar threading + gate + narrow reject_ssh_url (host TDD).**
- **G7d — device acceptance + final gate (PENDING-DEVICE runbook).**

## Risks

| Risk | Mitigation |
|------|------------|
| **libssh2 NDK cross-compile (the gate)** | Front-loaded G7a; openssl backend already vendored + cross-built (P4a precedent); libz-sys present. One bounded attempt, then BLOCKED (GS3) — no rat-holing. |
| Host-key wire format (known_hosts vs raw blob) ambiguity | Pin the exact format in the plan; unit-test the comparison helper with concrete sample entries. |
| Private key on disk weakens the credential posture | Accepted (GS2); mitigated by app-sandbox path validation + host-owned at-rest protection + passphrase in-memory; documented as a deliberate departure. |
| Binary size (libssh2 added to the cdylib) | Record the AAR size delta per ABI in the final gate; libssh2 is small relative to libgit2/openssl. |
| SSH round-trip not host-testable | Host covers callbacks/gate/path-validation; the live clone/push + host-key check is PENDING-DEVICE (GS4), same posture as P4. |
