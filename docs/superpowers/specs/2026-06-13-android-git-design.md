# Android Git Tool Design (P4)

Date: 2026-06-13
Status: Approved design (brainstormed + section-approved)
Part of: the Android sandbox + shell effort (spec r3
`2026-06-12-android-sandbox-shell-design.md`). This is the P4 phase — the
structured Git tool — designed in its own spec because the implementation
diverged materially from the r3 sketch (libgit2 in-process, not a bundled
git binary under minijail).

## Summary

Android gets git through a **structured, in-process `Git` tool backed by
libgit2** (the `git2` crate), NOT through the deny-net `Shell` tool and NOT
through a bundled git executable. libgit2 links into the engine `.so` as a
**library** — exempt from Android 10 W^X exec restrictions, requiring no
package-manager exec path and no P0b packaging proof — and runs in the engine
process, so it never touches minijail and is fully decoupled from the sandbox.

The tool exposes a structured operation enum (clone/fetch/pull/status/diff/log/
show/branch_list/checkout/add/commit/branch_create/merge) mapped to
deterministic `git2` calls — the model cannot inject arbitrary argv. v1 covers
**read + local-write, no push**. Networked operations (clone/fetch/pull) get a
per-operation permission prompt and an HTTPS token supplied in-memory by the
Kotlin host (never disk, never env). All third-party libraries — libgit2,
git2/libgit2-sys, and (now) minijail — are vendored into `third_party/`.

## Decision log (this P4 brainstorm, 2026-06-13)

| # | Decision | Choice |
|---|---|---|
| G1 | Implementation path | libgit2 (`git2` crate) in-process — NOT pure gitoxide, NOT a bundled git binary |
| G2 | Operation set (v1) | read + local-write incl. commit; **no push** (deferred) |
| G3 | Auth | Kotlin host supplies HTTPS token via FFI, in-process to libgit2's credential callback, per-operation; never disk/env; under the D11 Keystore gate |
| G4 | Vendoring convention | **all third-party libs vendored into `third_party/`**; libgit2 + git2-rs added there; **minijail symlink → properly vendored** (debt repaid) as part of P4 |
| G5 | Registration gate | independent of the minijail sandbox (libgit2 is in-process); gate = `enable_git && workspace ready && CA store reachable`; **token NOT in the gate** (missing token disables only network ops, not the tool) |
| G6 | TLS / CA | libgit2 TLS backend = mbedtls; CA store = Android system `/system/etc/security/cacerts` (no bundle), fallback bundled Mozilla CA PEM |
| G7 | SSH | out of v1 (HTTPS + token only); `git@…` URLs return a named error |
| G8 | merge/pull | fast-forward only in v1; non-ff returns a named error (no auto conflict resolution, no leftover conflict markers) |

## Context

- Predecessor: P0a+P1+P2+P3 merged to main (`006a1b17`). The minijail sandbox
  runtime executes jailed deny-net shell commands (device-verified); the mobile
  `Shell` tool is registration-gated. Spec r3 §"Git as the first structured
  network tool (P4)" named two paths — bundled NDK git binary OR gitoxide —
  with a decision rule. This brandstorm chose a **third** concrete path
  (libgit2/`git2`), which keeps the in-process / no-exec / no-W^X benefits the
  rule favored while using the most battle-tested local-write engine.
- The `Shell` tool stays deny-net (D10). Networked git is deliberately a
  separate structured tool: approving `git fetch` never grants arbitrary
  `sh -c` network access.
- `BuiltinToolContext` already carries per-platform optional fields (camera,
  voice, `android_shell` from P3). The Git tool adds `android_git` the same way.
- Permission allow/ask is the engine's `AdapterPermissionGate` (already wired on
  mobile via `PermissionRequestSink` → Kotlin). The desktop `BashTool` and the
  P3 mobile `Shell` tool both stub `check_permissions`; the Git tool mirrors
  that — the engine gate is the real allow/ask.

## Goals

1. A model-facing structured `Git` tool for read + local-write git, in-process
   via libgit2, with no exec/W^X/packaging exposure.
2. Vendor all third-party libs (libgit2, git2-rs, minijail) into `third_party/`
   so a clean checkout builds with no external fetch — repaying the minijail
   symlink debt that blocks CI.
3. Per-operation network permission + in-process token auth (HTTPS), never
   disk/env, under the D11 Keystore gate.
4. Maximize host-testable surface: libgit2 is a library, so nearly all tool
   logic is end-to-end testable on the host against real temp repos (`file://`
   bare remotes for network ops) — the device gate is a thin TLS/cacerts layer.

## Non-goals

1. No push in v1 (G2). No SSH in v1 (G7). No three-way merge / conflict
   resolution in v1 (G8 — fast-forward only).
2. No use of minijail for git (it is in-process; the sandbox is for the Shell
   tool's exec path).
3. No git credential-helper / on-disk credential storage; no `/etc/ssl`
   dependence.
4. No reuse of the engine's LLM-API credential store for git remotes (different
   credential domain).

## Architecture

```text
third_party/
├── libcap/        (P0a, vendored — the precedent)
├── minijail/      ◀ P4: symlink → properly vendored + committed (debt repaid)
├── libgit2/       ◀ P4: vendored libgit2 C sources (NDK cross-compiled)
└── git2-rs/       ◀ P4: vendored git2 + libgit2-sys Rust crate sources

lingxi-code/
├── tools/git-mobile/                 new crate `tool-git-mobile` (forbid-unsafe)
│   └── src/
│       ├── lib.rs    GitTool (Tool impl) + register_all (gated) + schema
│       ├── ops.rs    operation enum → git2 calls (no argv)
│       └── auth.rs   credential callback (host token → libgit2, in-process)
├── tool-api/src/builtin_context.rs   + AndroidGitToolCtx carrier + android_git field
├── apps/engine-mobile/{lib.rs,host.rs}  register GitTool; MobileConfig.android_git
└── apps/android-aar/src/lib.rs       compute the git gate + token into MobileConfig
```

- **Library, not executable.** `git2` (safe Rust wrapper) → `libgit2-sys`
  (built from vendored `third_party/libgit2` via cmake/cc under the NDK, like
  P0a's libcap). libgit2 links into the engine `.so`; it is a library, so W^X
  does not apply, there is no `nativeLibraryDir` exec path, and **no P0b
  packaging proof is needed**.
- **forbid-unsafe.** `git2` is a safe API → `tool-git-mobile` keeps
  `#![forbid(unsafe_code)]`. The only C is libgit2-sys at build time.
- **In-process, sandbox-decoupled.** libgit2 runs in the engine process: no
  fork, no exec, no minijail. The Git tool works even where the minijail
  sandbox is unavailable.
- **License.** libgit2 is "GPLv2 with a linking exception" (permits linking
  into any app); `git2`/`libgit2-sys` are MIT/Apache. OSS notice; no source
  offer burden.
- **TLS / CA (G6).** libgit2 TLS backend = mbedtls (lightest pure-C, good
  cross-compile). CA store = Android system `/system/etc/security/cacerts` via
  `git_libgit2_opts(GIT_OPT_SET_SSL_CERT_LOCATIONS)` (device-provided, no
  bundle); fallback = a bundled Mozilla CA PEM if the system dir is
  unreachable. Budget ~2–3 MB/ABI (well under a bundled git's ~8 MB).

## The Git tool

Model-facing name **`Git`**. Input is a structured operation enum + named
parameters (NOT a shell string); the tool maps each operation to a deterministic
`git2` call, so the model cannot inject arbitrary argv/flags.

```text
Git {
  operation: "clone"|"fetch"|"pull"|"status"|"diff"|"log"|"show"
            |"branch_list"|"checkout"|"add"|"commit"|"branch_create"|"merge",
  repo_url?, remote?(default origin), branch?/refspec?, paths?[],
  message?, rev?/rev_range?, new_branch?
}
```

All repo paths are anchored under the app-private workspace root with path
validation (reject escapes), mirroring the Shell tool's cwd handling.

### Operation → git2 mapping (v1)

| operation | git2 call | network | authorization |
|---|---|---|---|
| `clone` | `build::RepoBuilder` + `FetchOptions` (cred cb) | yes | per-op prompt ("clone `<url>`?") |
| `fetch` | `Remote::fetch` (cred cb) | yes | per-op prompt |
| `pull` | `fetch` + **fast-forward only** merge | yes | per-op prompt; non-ff → named error |
| `status` | `Repository::statuses` | local | engine gate |
| `diff` | `Repository::diff_*` (workdir/index/tree) | local | engine gate |
| `log` | `Revwalk` (+ paging/cap) | local | engine gate |
| `show` | `find_commit/blob` + diff | local | engine gate |
| `branch_list` | `Repository::branches` | local | engine gate |
| `checkout` | `set_head` + `checkout_tree` (safe mode) | local-write | engine gate; dirty worktree → reject (no data loss) |
| `add` | `Index::add_all` + `write` | local-write | engine gate |
| `commit` | `Repository::commit` (signature below) | local-write | engine gate |
| `branch_create` | `Repository::branch` | local-write | engine gate |
| `merge` | `merge_analysis` → **fast-forward only** | local-write | engine gate; non-ff/conflict → named error |

### v1 boundary decisions

- **commit identity (G8-adjacent).** `git2` needs author/committer signatures.
  v1 reads `user.name`/`user.email` from the repo's `.git/config`; if absent,
  uses a fixed default (`LingXi <noreply@lingxi>`). Never reads the
  environment or a global gitconfig (mobile has none).
- **merge/pull fast-forward only (G8).** Non-ff returns a named error so the
  model chooses an explicit path; v1 never auto-resolves conflicts or leaves
  conflict markers.
- **`check_permissions`** mirrors the desktop/Shell stub (Allow + reason); the
  real allow/ask is the engine `AdapterPermissionGate`.

## Authentication (G3)

```text
Kotlin host (Keystore-backed storage)
  │  user enters an HTTPS PAT (GitHub/GitLab token) in settings → Keystore
  ▼  passed via FFI as an in-memory String (UniFFI; never written to disk)
GitTool / auth.rs  — token held only in memory, function scope
  ▼  only for clone/fetch/pull: installed into RemoteCallbacks::credentials
libgit2 cred callback: Cred::userpass_plaintext(user, token)
  │   GitHub/GitLab: user = "x-access-token" or the PAT as username
  ▼   TLS (mbedtls + system cacerts) → remote
returns; token drops with scope. Never disk, never child-process env.
```

- **Not out of process.** libgit2 is in-process → the token never reaches a
  child, argv, or env. The plaintext-exposure surface P3 worried about for the
  Shell tool (`cat` the token) does not exist for the Git tool by construction.
- **Not on disk.** No `.git/credentials`, no credential-helper (those persist).
  The host supplies the token fresh per build.
- **D11-consistent.** The token must come from Keystore-backed storage (the
  same D11 gate as Shell). Missing token → network ops return "git credentials
  not configured", not a 401 with empty creds.
- **Token lifetime.** v1 holds the host-supplied token in
  `BuiltinToolContext.android_git` (resident in the tool context — same UID,
  same tier as other in-memory credentials). A stricter "pull the token from
  the host per operation" (a token-provider FFI callback) is a documented
  later hardening (see Risks).

## Registration gate (G5)

The Git tool's gate is **independent of the minijail sandbox** — libgit2 is
in-process and never touches minijail, so the gate depends on no sandbox
capability probe. The Shell and Git tools toggle independently (a device with
no usable minijail but a configured git token can have Git without Shell).

```rust
fn android_git_gate(cfg: &AndroidGitConfig) -> bool {
    cfg.enable_git                  // build/user config opt-in
    && workspace_root_ready         // app-private dir exists + writable
    && ca_store_reachable           // /system/etc/security/cacerts OR bundled PEM
}
// token is NOT in the gate: a missing token disables only NETWORK operations
// (clone/fetch/pull return "git credentials not configured"); local operations
// (status/diff/log/commit/...) work whenever enable_git holds. No token still
// allows cloning public repos + all local writes.
```

Carrier + registration mirror P3's `AndroidShellToolCtx` pattern:

```rust
// tool-api
pub struct AndroidGitToolCtx {
    pub enabled: bool,        // = android_git_gate(...)
    pub has_token: bool,      // whether network ops are usable (prompt wording)
    pub workspace_root: String,
}
// android-aar build_android_engine computes the gate + token into
//   MobileConfig.android_git → host.rs → BuiltinToolContext.android_git
// tool_git_mobile::register_all registers GitTool only when
//   ctx.android_git.is_some_and(|g| g.enabled) — absent-not-erroring (same as Shell)
```

Runtime authorization: network operations go through the engine
`AdapterPermissionGate` per-operation ("approve fetch from origin?"),
session-only on mobile (no persistent store — confirmed in P3). Local
operations need no extra network authorization.

## Rollout phases

- **P4a — third_party vendoring + build proof.** Vendor libgit2 (C) +
  git2/libgit2-sys (Rust) into `third_party/`; convert `third_party/minijail`
  from symlink to a properly vendored, committed subset (CORE sources + `rust/`
  + pre-generated tables; drop `.git`/tests/graphify-out). Prove
  `libgit2-sys` NDK cross-compiles (arm64 + x86_64) with mbedtls + system
  cacerts. Verify the P0a–P2 minijail cross-compile still passes on a clean
  checkout (no symlink).
- **P4b — local operations (host-testable, no network).** `tool-git-mobile`
  crate + `Git` schema + `ops.rs` for status/diff/log/show/branch_list/
  checkout/add/commit/branch_create/merge(ff). End-to-end tested **on the host**
  against real temp repos — no device needed.
- **P4c — network operations + auth.** clone/fetch/pull + credential callback +
  CA wiring; `auth.rs`; `AndroidGitConfig`/`AndroidGitToolCtx` + gate +
  registration wiring (android-aar). Host-tested against `file://` bare remotes
  (no real network/token/device).
- **P4d — device acceptance + P4 gate.** Real-device clone of a public HTTPS
  repo (proves mbedtls + system cacerts + real TLS) + local-write round trip +
  commit; workspace anchoring. Workspace fmt/clippy/test + both-ABI cross-build.

## Testing strategy

libgit2 is a **library** (unlike minijail's jailed exec, which only runs on
Android), so nearly all of `tool-git-mobile` is end-to-end testable on the
macOS host. The device gate is the thin "real `.so` load + system cacerts +
real HTTPS clone" layer.

- **Host unit (tool-git-mobile):** each operation against a `tempfile` temp repo
  — init→add→commit→branch→checkout→log→diff→status→merge(ff); schema
  validation; path-escape rejection (repo must be under the workspace root);
  non-ff merge/pull → named error; commit-identity fallback.
- **Host network:** clone/fetch/pull against a `file://` local bare repo
  (exercises the fetch engine + ff merge without real network/CA/token);
  credential-callback-invoked assertion.
- **Gate/registration (host):** `enable_git=false` → Git absent; `true` →
  present; `has_token=false` → tool present but network ops return "credentials
  not configured".
- **Device acceptance (API-34 arm64 emulator):** real clone of a public HTTPS
  repo (mbedtls + system cacerts + real TLS); post-clone local add/commit/log
  round trip; workspace anchoring.
- **Build:** `libgit2-sys` per-ABI NDK cross-compile; linked into the
  android-aar cdylib; symbol check.

## Risks

| Risk | Mitigation |
|---|---|
| libgit2-sys NDK cross-compile (cmake / TLS backend) | P4a front-loaded proof; mbedtls (lightest pure-C, cross-compiles cleanly); openssl fallback |
| system cacerts path/format varies by device | gate probes ca_store reachability; unreachable → bundled PEM fallback |
| token resident in ctx memory | same UID, same tier as other in-memory creds; "per-operation pull from host" is a documented later hardening |
| APK size (libgit2 + mbedtls) | budget ~2–3 MB/ABI (« bundled git ~8 MB); per-ABI CI tracking |
| gix-style local-write maturity gaps | N/A — libgit2 chosen precisely because local-write/checkout/merge are battle-tested |
| minijail re-vendoring breaks the P0a–P2 build | P4a verifies the cross-compile on a clean checkout before anything depends on it |

## Open decisions

1. Whether the per-operation token-provider callback (stricter than the
   resident-token v1) is worth the FFI complexity — revisit if a security
   review of mobile credential residency demands it.
2. Whether to add SSH (libssh2 + key management) in a later phase or keep
   HTTPS-only indefinitely.

## References

- Parent: `docs/superpowers/specs/2026-06-12-android-sandbox-shell-design.md`
  (r3, §"Git as the first structured network tool (P4)").
- P3 (the Shell tool + `AndroidShellToolCtx` carrier pattern this mirrors):
  merged at `006a1b17`.
- Codebase anchors: `tool-api/src/builtin_context.rs` (`AndroidShellToolCtx`),
  `tools/shell-mobile/` (the mirror crate), `apps/android-aar/src/lib.rs`
  (`android_shell_gate` + eager probe), `third_party/libcap/` (vendoring
  precedent), `traits/src/sandbox.rs` (`ExecTarget::BundledHelper` — the
  bundled-binary path P4 deliberately does NOT take).
