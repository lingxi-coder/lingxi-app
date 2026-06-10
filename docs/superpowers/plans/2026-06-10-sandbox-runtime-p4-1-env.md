# sandbox-runtime P4-1 — Proxy env vars + command encode (sandbox-utils.js, part 1)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port `generateProxyEnvVars` + `CA_TRUST_VARS` + `encodeSandboxedCommand`/`decodeSandboxedCommand` from `sandbox-utils.js` into `sandbox-runtime`. Pure, deterministic, byte-exact-testable. These produce the exact `--setenv` pairs the bwrap child gets (P4-2 consumes them).

**Reference:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/sandbox-utils.js:261-389` (`CA_TRUST_VARS`, `generateProxyEnvVars`, `encodeSandboxedCommand`, `decodeSandboxedCommand`).

**Branch:** `parity-sandbox-runtime-p4-1`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`** (untracked codex/liter-llm/opencode dirs). `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: `env.rs`

**Files:** Create `lingxi-code/sandbox-runtime/src/env.rs`; Modify `src/lib.rs` (+`pub mod env;`). `base64` is already a crate dep (used by parent_proxy).

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn has(v: &[(String, String)], k: &str, val: &str) -> bool {
        v.iter().any(|(kk, vv)| kk == k && vv == val)
    }

    #[test]
    fn no_ports_returns_minimal_plus_tmpdir() {
        let e = generate_proxy_env_vars(None, None, None, Platform::Linux, "/tmp/claude");
        assert!(has(&e, "SANDBOX_RUNTIME", "1"));
        assert!(has(&e, "TMPDIR", "/tmp/claude"));
        assert!(!e.iter().any(|(k, _)| k == "HTTP_PROXY")); // no proxy ports → no proxy vars
    }

    #[test]
    fn http_port_sets_proxy_and_no_proxy() {
        let e = generate_proxy_env_vars(Some(3128), None, None, Platform::Linux, "/tmp/claude");
        assert!(has(&e, "HTTP_PROXY", "http://localhost:3128"));
        assert!(has(&e, "HTTPS_PROXY", "http://localhost:3128"));
        assert!(has(&e, "http_proxy", "http://localhost:3128"));
        assert!(has(&e, "https_proxy", "http://localhost:3128"));
        assert!(has(&e, "NO_PROXY", "localhost,127.0.0.1,::1,*.local,.local,169.254.0.0/16,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"));
        assert!(has(&e, "no_proxy", "localhost,127.0.0.1,::1,*.local,.local,169.254.0.0/16,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"));
    }

    #[test]
    fn socks_port_sets_all_proxy_git_ssh_ftp_grpc() {
        // linux + http port → socat GIT_SSH_COMMAND
        let e = generate_proxy_env_vars(Some(3128), Some(1080), None, Platform::Linux, "/tmp/claude");
        assert!(has(&e, "ALL_PROXY", "socks5h://localhost:1080"));
        assert!(has(&e, "all_proxy", "socks5h://localhost:1080"));
        assert!(has(&e, "FTP_PROXY", "socks5h://localhost:1080"));
        assert!(has(&e, "RSYNC_PROXY", "localhost:1080"));
        assert!(has(&e, "GRPC_PROXY", "socks5h://localhost:1080"));
        assert!(has(&e, "DOCKER_HTTP_PROXY", "http://localhost:3128"));
        assert!(has(&e, "CLOUDSDK_PROXY_TYPE", "http"));
        assert!(has(&e, "CLOUDSDK_PROXY_PORT", "3128"));
        assert!(e.iter().any(|(k, v)| k == "GIT_SSH_COMMAND"
            && v == "ssh -o ControlMaster=no -o ControlPath=none -o ProxyCommand='socat - PROXY:localhost:%h:%p,proxyport=3128'"));
    }

    #[test]
    fn macos_socks_uses_nc_git_ssh() {
        let e = generate_proxy_env_vars(None, Some(1080), None, Platform::Macos, "/tmp/claude");
        assert!(e.iter().any(|(k, v)| k == "GIT_SSH_COMMAND"
            && v == "ssh -o ControlMaster=no -o ControlPath=none -o ProxyCommand='nc -X 5 -x localhost:1080 %h %p'"));
    }

    #[test]
    fn ca_cert_path_sets_all_trust_vars() {
        let e = generate_proxy_env_vars(Some(3128), None, Some("/tmp/ca.pem"), Platform::Linux, "/tmp/claude");
        for v in CA_TRUST_VARS {
            assert!(has(&e, v, "/tmp/ca.pem"), "missing {v}");
        }
    }

    #[test]
    fn encode_decode_roundtrip_and_truncation() {
        assert_eq!(decode_sandboxed_command(&encode_sandboxed_command("hello")), "hello");
        let long = "x".repeat(200);
        let dec = decode_sandboxed_command(&encode_sandboxed_command(&long));
        assert_eq!(dec.len(), 100); // truncated to 100 chars before base64
    }
}
```

- [ ] **Step 2: Verify fail**, then **implement** `env.rs`:

```rust
//! Proxy environment variables + command encoding (`sandbox-utils.js:261-389`).
//! [`generate_proxy_env_vars`] produces the exact `--setenv` pairs injected into
//! the bwrap child so its HTTP/SOCKS clients route through the host proxy bridge.

use base64::Engine;

/// Per-tool CA-trust env vars set when TLS-MITM is configured so HTTPS clients
/// trust the proxy-minted leaf certs (`CA_TRUST_VARS`, sandbox-utils.js:261-270).
pub const CA_TRUST_VARS: [&str; 9] = [
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "CURL_CA_BUNDLE",
    "REQUESTS_CA_BUNDLE",
    "PIP_CERT",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "CARGO_HTTP_CAINFO",
    "DENO_CERT",
];

/// Target platform for the `GIT_SSH_COMMAND` branch (`getPlatform`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS — `nc -X 5 -x` SOCKS5 `ProxyCommand`.
    Macos,
    /// Linux — `socat ... PROXY:` `ProxyCommand` (requires `http_port`).
    Linux,
}

const NO_PROXY_ADDRESSES: &str =
    "localhost,127.0.0.1,::1,*.local,.local,169.254.0.0/16,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16";

/// `generateProxyEnvVars` (sandbox-utils.js:272-377). `tmpdir` is the resolved
/// `CLAUDE_CODE_TMPDIR || CLAUDE_TMPDIR || /tmp/claude` (resolved by the caller
/// to keep this pure). Returns ordered `(key, value)` pairs.
#[must_use]
pub fn generate_proxy_env_vars(
    http_proxy_port: Option<u16>,
    socks_proxy_port: Option<u16>,
    ca_cert_path: Option<&str>,
    platform: Platform,
    tmpdir: &str,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = vec![
        ("SANDBOX_RUNTIME".into(), "1".into()),
        ("TMPDIR".into(), tmpdir.into()),
    ];
    if let Some(ca) = ca_cert_path {
        for v in CA_TRUST_VARS {
            env.push((v.into(), ca.into()));
        }
    }
    if http_proxy_port.is_none() && socks_proxy_port.is_none() {
        return env;
    }
    env.push(("NO_PROXY".into(), NO_PROXY_ADDRESSES.into()));
    env.push(("no_proxy".into(), NO_PROXY_ADDRESSES.into()));
    if let Some(p) = http_proxy_port {
        let u = format!("http://localhost:{p}");
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            env.push((k.into(), u.clone()));
        }
    }
    if let Some(s) = socks_proxy_port {
        env.push(("ALL_PROXY".into(), format!("socks5h://localhost:{s}")));
        env.push(("all_proxy".into(), format!("socks5h://localhost:{s}")));
        let ssh_mux = "-o ControlMaster=no -o ControlPath=none";
        match platform {
            Platform::Macos => env.push((
                "GIT_SSH_COMMAND".into(),
                format!("ssh {ssh_mux} -o ProxyCommand='nc -X 5 -x localhost:{s} %h %p'"),
            )),
            Platform::Linux => {
                if let Some(h) = http_proxy_port {
                    env.push((
                        "GIT_SSH_COMMAND".into(),
                        format!("ssh {ssh_mux} -o ProxyCommand='socat - PROXY:localhost:%h:%p,proxyport={h}'"),
                    ));
                }
            }
        }
        env.push(("FTP_PROXY".into(), format!("socks5h://localhost:{s}")));
        env.push(("ftp_proxy".into(), format!("socks5h://localhost:{s}")));
        env.push(("RSYNC_PROXY".into(), format!("localhost:{s}")));
        let docker = format!("http://localhost:{}", http_proxy_port.unwrap_or(s));
        env.push(("DOCKER_HTTP_PROXY".into(), docker.clone()));
        env.push(("DOCKER_HTTPS_PROXY".into(), docker));
        if let Some(h) = http_proxy_port {
            env.push(("CLOUDSDK_PROXY_TYPE".into(), "http".into()));
            env.push(("CLOUDSDK_PROXY_ADDRESS".into(), "localhost".into()));
            env.push(("CLOUDSDK_PROXY_PORT".into(), format!("{h}")));
        }
        env.push(("GRPC_PROXY".into(), format!("socks5h://localhost:{s}")));
        env.push(("grpc_proxy".into(), format!("socks5h://localhost:{s}")));
    }
    env
}

/// `encodeSandboxedCommand` (sandbox-utils.js:378-381): truncate to 100 chars,
/// base64. (Char-truncation matches the TS `.slice(0,100)`; for non-ASCII a
/// byte-vs-char nuance exists — TS slices UTF-16 code units; we slice by
/// `char` boundary, which is the closest faithful behavior. Documented.)
#[must_use]
pub fn encode_sandboxed_command(command: &str) -> String {
    let truncated: String = command.chars().take(100).collect();
    base64::engine::general_purpose::STANDARD.encode(truncated.as_bytes())
}

/// `decodeSandboxedCommand` (sandbox-utils.js:385-389): base64-decode to UTF-8
/// (lossy on invalid UTF-8, matching Node's `toString('utf8')`).
#[must_use]
pub fn decode_sandboxed_command(encoded: &str) -> String {
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default()
}
```

- [ ] **Step 3: Run `cargo test -p sandbox-runtime` → PASS. lib.rs `pub mod env;`. Gate + commit** (`feat(sandbox-runtime): generate_proxy_env_vars + CA_TRUST_VARS + command encode (P4-1)`).

---

### Task 2: gates
```bash
cd /Users/luolingfeng/Projects/LingXi-Next/lingxi-code
cargo test -p sandbox-runtime && cargo clippy -p sandbox-runtime --all-targets --no-deps -- -D warnings
cargo test --workspace --no-run && cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime  # 0
```
Frozen `git diff main -- lingxi-code/traits lingxi-code/protocol` empty. Stage ONLY explicit sandbox-runtime + Cargo paths.

## Final verification
1. env var list byte-exact vs `generateProxyEnvVars` (the NO_PROXY string, the GIT_SSH socat/nc forms, CLOUDSDK trio, DOCKER pair, the socks5h URLs).
2. engine-mobile 0-dep; frozen empty.
