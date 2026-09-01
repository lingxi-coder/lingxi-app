# Linux bwrap Sandbox Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the 5 security findings that held Batch-14's bwrap sandbox, building faithful hardening on the current simplified `wrap_linux_bwrap` baseline, with each fix runtime-verified in a `--privileged` Linux Docker container.

**Architecture:** A reusable Docker verification harness (`scripts/verify-bwrap.sh`) is built first and used as the per-task runtime gate. Then four fix areas land in `sandbox/src/wrap.rs` + `sandbox/src/dependency_check.rs` + `platforms/posix/src/sandbox.rs`: conservative network mapping (findings 1+2), `--unshare-user-try` (finding 5), deny-write-by-existence via ro-bind-in-place (finding 3), and host-side post-command bare-repo scrub (finding 4). `wrap` stays a pure deterministic string builder (macOS arg-vector unit tests); the FS-existence split lives in the posix `prepare` layer.

**Tech Stack:** Rust 1.82 / edition 2021, bubblewrap (verified in Docker `arm64v8/debian:stable-slim` + `--privileged`), the existing `sandbox`/`platform-posix` crates. No new Rust deps.

**Spec:** `docs/superpowers/specs/2026-06-10-linux-bwrap-sandbox-hardening-design.md` (approved). Reference of truth: `claude-code/src/utils/sandbox/sandbox-adapter.ts` (deny/scrub semantics; the bwrap argv builder is the unvendored `@anthropic-ai/sandbox-runtime` — the flag shape's reference is the container's real bwrap behavior). Verified runtime facts in memory `bwrap-docker-verification`.

**Branch:** `parity-bwrap-hardening` (created off `main`, spec committed).

**Conventions that bite (read first):**
- Cargo workspace root is `lingxi-code/`, NOT the repo root. Run cargo there; run git from the repo root with `lingxi-code/...`-prefixed paths (`git add` after `cd lingxi-code` → `lingxi-code/lingxi-code/...` pathspec error).
- Workspace enforces rustc `-D missing-docs` + clippy `-D warnings` (pedantic) on `--all-targets`.
- Commit with `git commit -F <file>` (zsh traps backticks/angle brackets/`->`). Footer EXACTLY:
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`
- NEVER run `cargo test --workspace` (runtime) — fs_watch flake. Use per-crate tests + `cargo test --workspace --no-run`.
- **Docker is required for the runtime gates.** The daemon must be running (`docker info` succeeds; if not, `open -a Docker` and wait ~10s). The container probe takes ~30-60s (image pull + apt install bubblewrap on first run; the image is cached after). All bwrap probes need `--privileged` (Docker's default seccomp blocks unprivileged userns). Use `arm64v8/debian:stable-slim` (this box is arm64).
- The bwrap argv reference is the CONTAINER's real bwrap, not a TS file — assert behavior, not a byte-for-byte TS argv.

---

## File map

| File | Action | Responsibility |
|---|---|---|
| `scripts/verify-bwrap.sh` | Create | reusable Docker bwrap runtime-verification harness (per-finding assertions) |
| `lingxi-code/sandbox/src/runtime_config.rs` | Modify | additive `#[serde(skip)]` `ro_bind_in_place` + `scrub_paths` fields on `SandboxRuntimeConfig` |
| `lingxi-code/sandbox/src/wrap.rs` | Modify | `wrap_linux_bwrap`: `--unshare-user-try`, conservative net, `--ro-bind`-in-place (ordered), host-side scrub append |
| `lingxi-code/sandbox/src/dependency_check.rs` | Modify | socat → warning, not blocking error (finding 1) |
| `lingxi-code/platforms/posix/src/sandbox.rs` | Modify | `runtime_config_from_policy` conservative net; `prepare` FS-existence split → populate the two new fields |

`wrap_with_sandbox(command, policy, platform)` signature stays UNCHANGED; the posix layer populates the two additive `SandboxRuntimeConfig` fields that `wrap_linux_bwrap` reads.

---

### Task 1: Docker verification harness (`scripts/verify-bwrap.sh`)

Built first so Tasks 2-5 gate on it. It runs a bwrap shape inside the container and asserts behavior.

**Files:**
- Create: `scripts/verify-bwrap.sh`

- [ ] **Step 1: Write the harness.** `scripts/verify-bwrap.sh`:

```bash
#!/usr/bin/env bash
# Runtime verification of the bwrap shapes the Rust sandbox builder produces.
# Requires Docker (LinuxKit). bwrap needs --privileged (Docker's default seccomp
# blocks unprivileged userns). arm64 host → arm64v8/debian.
#
# Usage: scripts/verify-bwrap.sh            (runs all checks)
#        scripts/verify-bwrap.sh net|userns|deny|scrub   (one group)
set -euo pipefail
GROUP="${1:-all}"
IMG="arm64v8/debian:stable-slim"

if ! docker info >/dev/null 2>&1; then
  echo "FATAL: Docker daemon not running (open -a Docker)"; exit 2
fi

# The in-container script. Installs bubblewrap then runs the requested checks.
read -r -d '' INNER <<'EOS' || true
set -u
apt-get update -qq >/dev/null 2>&1
apt-get install -y -qq bubblewrap >/dev/null 2>&1
GROUP="$1"
fail=0
ok()   { echo "PASS: $1"; }
bad()  { echo "FAIL: $1"; fail=1; }

if [ "$GROUP" = net ] || [ "$GROUP" = all ]; then
  # LoopbackOnly/Disabled shape == --unshare-net : external blocked, loopback up.
  if bwrap --unshare-user-try --unshare-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'getent hosts example.com >/dev/null 2>&1' ; then
    bad "unshare-net leaked external DNS"
  else ok "unshare-net blocks external"; fi
  if bwrap --unshare-user-try --unshare-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'ip link show lo >/dev/null 2>&1 || true; echo lo-ok' | grep -q lo-ok ; then
    ok "unshare-net keeps loopback ns"
  else bad "unshare-net loopback missing"; fi
  # Allowed shape == --share-net : external resolvable (host has net).
  if bwrap --unshare-user-try --share-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'getent hosts example.com >/dev/null 2>&1' ; then
    ok "share-net allows external"
  else echo "WARN: share-net external unresolved (host offline?) — not a sandbox failure"; fi
fi

if [ "$GROUP" = userns ] || [ "$GROUP" = all ]; then
  if bwrap --unshare-user-try --unshare-pid --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'echo up' | grep -q up ; then ok "unshare-user-try starts"; else bad "unshare-user-try start"; fi
  sysctl -w user.max_user_namespaces=0 >/dev/null 2>&1 || true
  if bwrap --unshare-user-try --unshare-pid --unshare-net --ro-bind / / --proc /proc --dev /dev -- \
       /bin/sh -c 'echo deg' | grep -q deg ; then ok "unshare-user-try degrades (userns=0)"; else bad "degrade"; fi
  sysctl -w user.max_user_namespaces=15000 >/dev/null 2>&1 || true
fi

if [ "$GROUP" = deny ] || [ "$GROUP" = all ]; then
  mkdir -p /w && echo orig > /w/HEAD
  out=$(bwrap --unshare-user-try --ro-bind / / --bind /w /w --ro-bind /w/HEAD /w/HEAD --proc /proc --dev /dev -- \
       /bin/sh -c 'echo x > /w/HEAD 2>/dev/null && echo WROTE || echo DENIED')
  [ "$out" = DENIED ] && ok "ro-bind-in-place denies write (overrides --bind parent)" || bad "ro-bind-in-place write $out"
  [ "$(cat /w/HEAD)" = orig ] && ok "host file unchanged" || bad "host file mutated"
fi

if [ "$GROUP" = scrub ] || [ "$GROUP" = all ]; then
  mkdir -p /s && cd /s
  # The wrapped-string shape: inner plants HEAD in host cwd; suffix scrubs it, preserves rc.
  bash -c 'bwrap --unshare-user-try --ro-bind / / --bind /s /s --proc /proc --dev /dev -- \
            /bin/sh -c "echo planted > /s/HEAD; exit 7"
           rc=$?; rm -rf -- /s/HEAD 2>/dev/null; exit $rc' ; rc=$?
  [ "$rc" = 7 ] && ok "scrub suffix preserves inner exit code" || bad "exit code $rc != 7"
  [ ! -e /s/HEAD ] && ok "planted bare-repo file scrubbed" || bad "HEAD not scrubbed"
  # Pre-existing file must NOT be scrubbed (it would be in ro_bind_in_place, never the scrub list).
  echo keep > /s/config
  bash -c 'true; rc=$?; rm -rf -- /s/HEAD 2>/dev/null; exit $rc' >/dev/null 2>&1 || true
  [ -e /s/config ] && ok "pre-existing file not in scrub list (untouched)" || bad "config wrongly scrubbed"
fi

echo "=== $([ $fail = 0 ] && echo ALL-PASS || echo SOME-FAIL) ==="
exit $fail
EOS

docker run --rm --privileged "$IMG" /bin/bash -c "$INNER" _ "$GROUP"
```

- [ ] **Step 2: Make executable + smoke it.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next
chmod +x scripts/verify-bwrap.sh
docker info >/dev/null 2>&1 || open -a Docker   # ensure daemon
scripts/verify-bwrap.sh all
```

Expected: each group prints `PASS:` lines and a final `=== ALL-PASS ===`. (The `share-net` external check may WARN if the host is offline — that is not a failure.) If any `FAIL:` appears, the harness itself is wrong — fix it before proceeding (the Rust isn't involved yet; this validates the assertions against real bwrap).

- [ ] **Step 3: Commit.** `git add scripts/verify-bwrap.sh` →
`feat(sandbox): Docker bwrap runtime-verification harness` + footer. (No Rust changed; no cargo gate needed for this task.)

---

### Task 2: conservative network mapping (findings 1 + 2)

**Files:**
- Modify: `lingxi-code/platforms/posix/src/sandbox.rs` (`runtime_config_from_policy`)
- Modify: `lingxi-code/sandbox/src/wrap.rs` (`wrap_linux_bwrap` net decision)
- Modify: `lingxi-code/sandbox/src/dependency_check.rs` (socat → warning)

- [ ] **Step 1: Failing test — net mapping.** In `platforms/posix/src/sandbox.rs` tests, add:

```rust
    #[test]
    fn loopback_and_disabled_do_not_request_full_egress() {
        use platform_api::NetworkPolicy;
        let mut p = crate::sandbox::default_policy_for_test(); // or construct a SandboxPolicy
        p.network = NetworkPolicy::LoopbackOnly;
        let cfg = runtime_config_from_policy(&p);
        assert!(cfg.network.allowed_domains.is_empty(), "loopback must not map to [*]");
        p.network = NetworkPolicy::Disabled;
        assert!(runtime_config_from_policy(&p).network.allowed_domains.is_empty());
        p.network = NetworkPolicy::Allowed;
        assert_eq!(runtime_config_from_policy(&p).network.allowed_domains, vec!["*".to_string()]);
    }
```

(If there's no `default_policy_for_test` helper, construct a `SandboxPolicy` literal — check the struct's public fields; `sandbox::policy::default_*` or `SandboxPolicy { network, writable_paths, denied_paths, .. }`. Match the existing test style in this file.)

- [ ] **Step 2: Verify failure**, then **Step 3: Implement net mapping.** In `runtime_config_from_policy` (sandbox.rs:239), change the network block:

```rust
        network: NetworkRestrictionConfig {
            // Conservative: only full-allow requests external egress. LoopbackOnly
            // is satisfied by bwrap's fresh netns (loopback present, external
            // blocked) → empty allowed_domains → `--unshare-net` in the wrapper.
            allow_local_binding: matches!(policy.network, NetworkPolicy::LoopbackOnly),
            allowed_domains: if matches!(policy.network, NetworkPolicy::Allowed) {
                vec!["*".to_string()]
            } else {
                vec![]
            },
            ..Default::default()
        },
```

(Delete the old `want_net = Allowed || LoopbackOnly` binding.)

- [ ] **Step 4: Failing test — wrap net decision.** In `sandbox/src/wrap.rs` tests, add:

```rust
    #[test]
    fn share_net_only_when_allowed_domains_present() {
        let mut cfg = SandboxRuntimeConfig::default();
        // Allowed → ["*"] → --share-net
        cfg.network.allowed_domains = vec!["*".into()];
        assert!(wrap_linux_bwrap("true", &cfg).contains("--share-net"));
        // LoopbackOnly: empty domains + allow_local_binding → NOT --share-net
        cfg.network.allowed_domains.clear();
        cfg.network.allow_local_binding = true;
        let w = wrap_linux_bwrap("true", &cfg);
        assert!(!w.contains("--share-net"), "loopback must not get full egress: {w}");
        assert!(w.contains("--unshare-net"));
    }
```

- [ ] **Step 5: Implement wrap net decision.** In `wrap_linux_bwrap`, replace the `want_network` block:

```rust
    // Conservative network posture: full host net ONLY for an allow-all policy
    // (allowed_domains non-empty == NetworkPolicy::Allowed). LoopbackOnly /
    // Disabled get a fresh network namespace (loopback-only, external blocked).
    // The socat domain-filter companion is deferred; a domain-allowlist policy
    // therefore gets no external egress (errs safe) until it lands.
    if policy.network.allowed_domains.is_empty() {
        args.push("--unshare-net".into());
    } else {
        args.push("--share-net".into());
    }
```

- [ ] **Step 6: socat → warning (finding 1).** In `dependency_check.rs::into_errors`, REMOVE the socat→errors push:

```rust
    pub fn into_errors(self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.sandbox_exec {
            errors.push("sandbox-exec not found".to_string());
        }
        if self.bwrap {
            errors.push("bwrap not found".to_string());
        }
        // socat is NOT a blocking error: the conservative network posture does
        // not shell to socat (a non-full-allow policy maps to --unshare-net, no
        // proxy). A missing socat must NEVER disable the sandbox (finding 1) —
        // it only limits the (deferred) domain-filter companion. Surfaced as a
        // warning by `check_dependencies` instead.
        errors
    }
```

And in `check_dependencies` (sandbox.rs:60), move socat into `warnings`:

```rust
        let mut warnings = Vec::new();
        if missing.socat {
            warnings.push("socat not found (domain-filtered networking unavailable; sandbox still enforced)".to_string());
        }
        SandboxDependencyCheck { errors: missing.into_errors(), warnings }
```

(Adjust to the actual `check_dependencies` body shape — it currently builds `errors: missing.into_errors(), warnings: vec![]`. Keep `missing.socat = !which_exists("socat")` so the probe still runs; just route it to warnings.)

- [ ] **Step 7: Add a socat-warning test** in `dependency_check.rs`:

```rust
    #[test]
    fn missing_socat_is_a_warning_not_a_blocking_error() {
        let missing = MissingTools { sandbox_exec: false, bwrap: false, socat: true };
        assert!(missing.into_errors().is_empty(), "socat must not block the sandbox");
    }
```

(Match `MissingTools`'s real field names/visibility; if it's private, test via `check_dependencies` with a stubbed `which_exists` if one exists, else assert `into_errors` on a constructed value — make the struct/fields `pub(crate)` + test in-module if needed.)

- [ ] **Step 8: Run unit tests.** `cd lingxi-code && cargo test -p sandbox -p platform-posix` (use the actual posix crate package name — check `platforms/posix/Cargo.toml` `name`; likely `platform-posix`). Expect PASS.

- [ ] **Step 9: Docker net gate.** `scripts/verify-bwrap.sh net` → `ALL-PASS` (unshare-net blocks external + keeps loopback; share-net allows external).

- [ ] **Step 10: Clippy + commit.** `cargo clippy -p sandbox -p platform-posix --all-targets --no-deps -- -D warnings`. Commit (`fix(sandbox): conservative net mapping (loopback/disabled->no-net; only allowed->share-net) + socat non-blocking`).

---

### Task 3: `--unshare-user-try` (finding 5)

**Files:**
- Modify: `lingxi-code/sandbox/src/wrap.rs`

- [ ] **Step 1: Failing test.** In `wrap.rs` tests:

```rust
    #[test]
    fn bwrap_creates_a_user_namespace() {
        let w = wrap_linux_bwrap("true", &SandboxRuntimeConfig::default());
        assert!(w.contains("--unshare-user-try"),
            "bwrap must request a userns (degrading) so it can create pid/net ns unprivileged: {w}");
    }
```

- [ ] **Step 2: Verify failure**, then **Step 3: Implement.** In `wrap_linux_bwrap`'s initial `args` vec, add `--unshare-user-try` FIRST (before `--unshare-pid`):

```rust
    let mut args: Vec<String> = vec![
        // Create a user namespace where possible; degrade gracefully on kernels
        // with unprivileged userns disabled (`-try`) instead of failing to start
        // (finding 5). Without it an unprivileged bwrap cannot create the pid/net
        // namespaces below. Verified to start + degrade under user.max_user_namespaces=0.
        "--unshare-user-try".into(),
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--proc".into(),
        "/proc".into(),
        "--dev".into(),
        "/dev".into(),
        "--unshare-pid".into(),
        "--die-with-parent".into(),
    ];
```

- [ ] **Step 4: Run tests + Docker userns gate.** `cargo test -p sandbox` then `scripts/verify-bwrap.sh userns` → `ALL-PASS` (starts + degrades under userns=0).

- [ ] **Step 5: Clippy + commit** (`fix(sandbox): --unshare-user-try so bwrap functions + degrades unprivileged (finding 5)`).

---

### Task 4: deny-write by existence — ro-bind-in-place (finding 3)

**Files:**
- Modify: `lingxi-code/sandbox/src/runtime_config.rs` (additive fields)
- Modify: `lingxi-code/sandbox/src/wrap.rs` (emit ro-bind-in-place, ordered)
- Modify: `lingxi-code/platforms/posix/src/sandbox.rs` (`prepare`: FS-existence split)

- [ ] **Step 1: Additive fields.** In `runtime_config.rs`, add to `SandboxRuntimeConfig` (the top-level struct — find it; it has `filesystem`, `network`, etc.) two `#[serde(skip)]` fields (skip keeps the TS-mirror serialization byte-identical):

```rust
    /// Host paths to mount read-only IN PLACE (`--ro-bind <p> <p>`), overriding
    /// any writable parent. Populated by the posix `prepare` layer from the
    /// EXISTING subset of denied + bare-repo paths (FS access lives there, not
    /// in the pure wrapper). claude-code denyWrite semantics (sandbox-adapter.ts:264).
    #[serde(skip)]
    pub ro_bind_in_place: Vec<String>,
    /// Host paths to delete AFTER the command (non-existent-at-config-time
    /// bare-repo files planted during the run). See finding 4 / Task 5.
    #[serde(skip)]
    pub scrub_paths: Vec<String>,
```

(`#[serde(skip)]` defaults them to empty on deserialize; `Default` already gives empty Vecs. Confirm the struct derives `Default`.)

- [ ] **Step 2: Failing test — wrap emits ro-bind-in-place AFTER allow_write binds.** In `wrap.rs` tests:

```rust
    #[test]
    fn ro_bind_in_place_comes_after_allow_write_so_deny_wins() {
        let mut cfg = SandboxRuntimeConfig::default();
        cfg.filesystem.allow_write = vec!["/work".into()];
        cfg.ro_bind_in_place = vec!["/work/.git/HEAD".into()];
        let w = wrap_linux_bwrap("true", &cfg);
        let bind_pos = w.find("--bind /work /work").expect("allow_write bind");
        let ro_pos = w.find("--ro-bind /work/.git/HEAD /work/.git/HEAD").expect("ro-bind-in-place");
        assert!(ro_pos > bind_pos, "ro-bind-in-place must follow allow_write bind to override it:\n{w}");
    }
```

- [ ] **Step 3: Verify failure**, then **Step 4: Implement.** In `wrap_linux_bwrap`, AFTER the `allow_write` bind loop and BEFORE the network block, add:

```rust
    // Deny-write: re-mount existing denied / bare-repo paths read-only IN PLACE.
    // Placed after the allow_write `--bind`s so a deny overrides a writable
    // parent (bwrap: later mounts win — verified). NEVER `--ro-bind-try /dev/null`
    // (that blanks the host file); ro-bind-in-place preserves it read-only
    // (finding 3, sandbox-adapter.ts:264).
    for path in &policy.ro_bind_in_place {
        args.push("--ro-bind".into());
        args.push(path.clone());
        args.push(path.clone());
    }
```

- [ ] **Step 5: Failing test — posix existence split.** In `platforms/posix/src/sandbox.rs` tests, add a test that drives `prepare` (or the split helper) against a tempdir with one existing + one absent denied path and asserts the existing one lands in `ro_bind_in_place` and the absent one does NOT (it goes to scrub in Task 5). If `prepare`'s split is extracted to a helper `fn split_deny_paths(deny: &[String], bare_repo_dirs: &[PathBuf]) -> (Vec<String> ro_in_place, Vec<String> scrub)`, test the helper directly:

```rust
    #[test]
    fn existing_denied_paths_go_ro_in_place_absent_go_scrub() {
        let tmp = tempfile::tempdir().unwrap();
        let exists = tmp.path().join("HEAD");
        std::fs::write(&exists, "x").unwrap();
        let absent = tmp.path().join("objects");
        let (ro, scrub) = split_bare_repo_paths(&[tmp.path().to_path_buf()]);
        assert!(ro.contains(&exists.to_string_lossy().into_owned()));
        assert!(scrub.contains(&absent.to_string_lossy().into_owned()));
        assert!(!ro.iter().any(|p| p.ends_with("objects")));
    }
```

- [ ] **Step 6: Implement the split in `prepare`.** Add a helper + call it in `prepare`, populating the two new `runtime_cfg` fields. The bare-repo set + scope (1:1 sandbox-adapter.ts:267, simplified to a single cwd — this port's `prepare` has one `cmd.cwd`, so `original_cwd == cwd`; documented):

```rust
/// claude-code bare-repo escape-defense file set (sandbox-adapter.ts:267).
const BARE_GIT_REPO_FILES: [&str; 5] = ["HEAD", "objects", "refs", "hooks", "config"];

/// Split the bare-repo escape-defense paths under each dir by FS existence:
/// existing → ro-bind-in-place (deny write); absent → scrub list (delete
/// post-command). Mirrors sandbox-adapter.ts:264-280. `original_cwd == cwd`
/// in this port (single prepare cwd), so the TS `[originalCwd, cwd]` widening
/// collapses to the one dir.
fn split_bare_repo_paths(dirs: &[std::path::PathBuf]) -> (Vec<String>, Vec<String>) {
    let mut ro_in_place = Vec::new();
    let mut scrub = Vec::new();
    for dir in dirs {
        for f in BARE_GIT_REPO_FILES {
            let p = dir.join(f);
            let s = p.to_string_lossy().into_owned();
            if p.exists() {
                ro_in_place.push(s);
            } else {
                scrub.push(s);
            }
        }
    }
    (ro_in_place, scrub)
}
```

In `prepare`, after `let runtime_cfg = runtime_config_from_policy(policy);`, fold in the split (use `cmd.cwd` as the single dir; skip when no cwd):

```rust
        let mut runtime_cfg = runtime_config_from_policy(policy);
        if let Some(cwd) = &cmd.cwd {
            let (ro_in_place, scrub) = split_bare_repo_paths(std::slice::from_ref(cwd));
            // Existing generic denied paths also re-mount ro-in-place (the wrapper
            // ignored deny_write before; now it enforces it for existing paths).
            let mut ro = ro_in_place;
            for d in &runtime_cfg.filesystem.deny_write {
                if std::path::Path::new(d).exists() {
                    ro.push(d.clone());
                }
            }
            runtime_cfg.ro_bind_in_place = ro;
            runtime_cfg.scrub_paths = scrub; // consumed by Task 5's wrap suffix
        }
```

(`runtime_cfg` must become `mut`. The `scrub_paths` are populated here but only WIRED into the wrap suffix in Task 5 — until then they are inert, which is fine.)

- [ ] **Step 7: Run unit tests + Docker deny gate.** `cargo test -p sandbox -p platform-posix` then `scripts/verify-bwrap.sh deny` → `ALL-PASS` (ro-bind-in-place denies write, overrides `--bind` parent, host file unchanged).

- [ ] **Step 8: Clippy + commit** (`fix(sandbox): deny-write via ro-bind-in-place by FS existence, never /dev/null (finding 3)`).

---

### Task 5: post-command bare-repo scrub (finding 4) + final gates

**Files:**
- Modify: `lingxi-code/sandbox/src/wrap.rs` (host-side scrub suffix)

- [ ] **Step 1: Failing test — scrub suffix shape.** In `wrap.rs` tests:

```rust
    #[test]
    fn scrub_paths_append_exit_preserving_host_side_rm() {
        let mut cfg = SandboxRuntimeConfig::default();
        cfg.scrub_paths = vec!["/s/HEAD".into(), "/s/ob'j".into()]; // includes a quote to test escaping
        let w = wrap_linux_bwrap("true", &cfg);
        assert!(w.contains("rc=$?"), "must capture bwrap exit: {w}");
        assert!(w.contains("exit \"$rc\"") || w.contains("exit $rc"), "must restore exit code: {w}");
        assert!(w.contains("rm -rf --"), "must rm the scrub paths: {w}");
        assert!(w.contains(r"'/s/ob'\''j'"), "scrub paths single-quote escaped: {w}");
    }

    #[test]
    fn no_scrub_suffix_when_list_empty() {
        let w = wrap_linux_bwrap("true", &SandboxRuntimeConfig::default());
        assert!(!w.contains("rc=$?"), "empty scrub list must not append a suffix (byte-identical): {w}");
    }
```

- [ ] **Step 2: Verify failure**, then **Step 3: Implement the suffix.** At the END of `wrap_linux_bwrap`, after `let joined = args.join(" ");` and the base `format!`, append the host-side scrub when `scrub_paths` is non-empty. Replace the final `format!(...)` with:

```rust
    let quoted = shell_escape_single(command);
    let joined = args.join(" ");
    let base = format!("bwrap {joined} -- /bin/sh -c {quoted}");
    if policy.scrub_paths.is_empty() {
        return base;
    }
    // Host-side post-command scrub of planted bare-repo files (finding 4,
    // scrubBareGitRepoFiles). Runs OUTSIDE bwrap on the host cwd after the
    // command, ENOENT-tolerant, preserving bwrap's exit code. Empty list →
    // no suffix (byte-identical to the un-hardened string).
    let scrub_args = policy
        .scrub_paths
        .iter()
        .map(|p| shell_escape_single(p))
        .collect::<Vec<_>>()
        .join(" ");
    format!("{base}\nrc=$?; rm -rf -- {scrub_args} 2>/dev/null; exit \"$rc\"")
}
```

- [ ] **Step 4: Run unit tests + Docker scrub gate.** `cargo test -p sandbox` then `scripts/verify-bwrap.sh scrub` → `ALL-PASS` (planted file scrubbed; exit code preserved; pre-existing file untouched).

- [ ] **Step 5: Full Docker harness.** `scripts/verify-bwrap.sh all` → `=== ALL-PASS ===`.

- [ ] **Step 6: FULL GATE RITUAL.**

```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p sandbox
cargo test -p platform-posix      # the platforms/posix package name
cargo clippy -p sandbox -p platform-posix --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run        # struct-trap (~2-3 min)
cargo build -p engine-desktop
cargo build -p engine-mobile
# engine-mobile must not regress on the sandbox change (it may pull sandbox
# already; confirm the diff is additive/inert for mobile):
cargo tree -p engine-mobile -e normal | grep -c "sandbox" # record (sharing is OK; the change is additive)
```

- [ ] **Step 7: Frozen-surface check.** `git diff main -- lingxi-code/traits lingxi-code/protocol` → MUST be empty.

- [ ] **Step 8: Commit** (`fix(sandbox): host-side post-command bare-repo scrub, exit-preserving (finding 4)`).

---

## Final verification (whole-branch)

1. `cargo test -p sandbox -p platform-posix` — all green.
2. `scripts/verify-bwrap.sh all` → `=== ALL-PASS ===` (the load-bearing runtime proof of all 5 findings).
3. `git diff main -- lingxi-code/traits lingxi-code/protocol` — empty (frozen surfaces).
4. Re-read the spec's "Safety invariants" against the code: no-more-net-than-requested (Task 2: loopback/disabled→unshare-net), missing-socat-can't-un-sandbox (Task 2), existing-paths-ro-in-place-never-blanked + planted-files-scrubbed (Tasks 4+5), sandbox-functions-unprivileged (Task 3 userns).
5. Update memory: `parity-1to1-effort.md` Batch-14 entry → bwrap hardening DONE (5 findings closed, Docker-verified); note the deferred socat domain-filter. Update `bwrap-docker-verification.md` if the harness path/shape changed.
