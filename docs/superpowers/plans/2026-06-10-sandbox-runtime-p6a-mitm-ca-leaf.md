# sandbox-runtime P6a — MITM CA + leaf minting (mitm-ca.js + mitm-leaf.js)

> REQUIRED SUB-SKILL: superpowers:subagent-driven-development.

**Goal:** Port the TLS-MITM certificate authority + per-host leaf minter into `sandbox-runtime/src/mitm_ca.rs` + `mitm_leaf.rs`. Generate/load an RSA-2048 SHA-256 CA, mint per-host RSA-2048 leaf certs (SAN=DNS/IP, validity clamped to min(CA notAfter, notBefore+99d), signed by the CA), and build cached rustls `ServerConfig`s per host. SECURITY-CRITICAL (these certs are what let the proxy decrypt the sandboxed client's HTTPS — they must chain correctly and ONLY be trusted via the sandbox's injected CA-trust env vars).

**Reference of truth:** `docs/superpowers/references/sandbox-runtime-0.0.54/dist/sandbox/{mitm-ca.js, mitm-leaf.js}`. The TS uses `node-forge`; the faithful Rust equivalent is `rcgen` (cert building/signing) + the `rsa` crate (RSA-2048 keygen — `ring` can sign RSA but not generate it). rustls 0.22 in lock uses the **ring** backend — keep rcgen on ring for one consistent crypto backend (avoid pulling `aws-lc-rs`'s C build).

**Deps to add (NOT in lock — add to sandbox-runtime/Cargo.toml):** `rcgen` (with the ring/`pem` features, NOT `aws_lc_rs`), `rsa` (RSA-2048 keygen), `pkcs8` if needed for DER encoding, `time` (in lock — rcgen uses it for validity), `rustls` (in lock). `getrandom` (added in P4-2c) for serials.

**Branch:** `parity-sandbox-runtime-p6a`. **Conventions:** Cargo root `lingxi-code/`; git from repo root, `lingxi-code/...` paths, **NEVER `git add -A`**. `-D missing-docs` + clippy pedantic. `#![forbid(unsafe_code)]`. Footer `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`.

---

### Task 1: `mitm_ca.rs` — CA load/generate + dispose

**Files:** Create `lingxi-code/sandbox-runtime/src/mitm_ca.rs`; Modify `src/lib.rs` + `Cargo.toml`.

- [ ] **`MitmCa`** struct: `cert_pem: String`, `key_pem: String`, the rcgen signing material (CA `rcgen::Certificate` or `CertificateParams` + `KeyPair`), `cert_path: PathBuf`, `key_path: PathBuf`, `ephemeral: bool`, and interior-mutable caches `leaf_certs: Mutex<HashMap<String, Leaf>>` + `server_configs: Mutex<HashMap<String, Arc<rustls::ServerConfig>>>` (the TS uses plain Maps; Rust needs `Mutex`/`RwLock` for the shared-across-tasks proxy — document).
- [ ] **`create_mitm_ca(opts: MitmCaOptions) -> Result<MitmCa>`** (`opts {ca_cert_path: Option<PathBuf>, ca_key_path: Option<PathBuf>}`):
  - both paths set → `load_ca` (read both PEMs; error if either missing/unreadable/not-PEM/not-parseable; the TS requires an RSA key — enforce, error otherwise);
  - exactly one set → error `"tlsTerminate: caCertPath and caKeyPath must be provided together"`;
  - neither → `generate_ephemeral_ca`: RSA-2048 (`rsa::RsaPrivateKey::new(&mut rng, 2048)`), self-signed CA, CN `"sandbox-runtime ephemeral CA"` + O `"sandbox-runtime"`, validity `[now-1d, now+825d]`, extensions: basicConstraints cA:true critical, keyUsage critical {keyCertSign,cRLSign,digitalSignature}, subjectKeyIdentifier; SHA-256. Write to a fresh `mkdtemp`-style 0700 dir (use a CSPRNG-named temp dir under `std::env::temp_dir()`, like P4-2c's nonce) — `ca.crt` (0644) + `ca.key` (0600); set `ephemeral=true`.
- [ ] **`dispose_mitm_ca(ca)`** — if ephemeral, `rm -rf` the temp dir (the `dirname(cert_path)`); no-op for user CAs.
- [ ] **Tests:** ephemeral CA generates + writes both files with the right modes; the CA cert parses, is a v3 CA (basicConstraints cA), CN matches; one-path-only → the exact error; load a generated CA back from its PEMs round-trips; dispose removes the ephemeral dir. Commit (`feat(sandbox-runtime): MITM CA load/generate (RSA-2048 SHA-256) + dispose (P6a)`).

### Task 2: `mitm_leaf.rs` — per-host leaf minting + ServerConfig

- [ ] **`mint_leaf_cert(ca: &MitmCa, hostname: &str) -> Leaf`** (`Leaf { cert_pem, key_pem }` where `cert_pem` = leaf PEM **concatenated with the CA PEM** so the chain is sent): cached on `ca.leaf_certs`. RSA-2048 leaf; serial = 16 random bytes high-bit-cleared; validity `notBefore = now-1d`, `notAfter = clamp_validity(ca, notBefore)` = `min(ca.notAfter, notBefore+99d)`; subject CN=hostname; issuer = CA subject DN; extensions: basicConstraints cA:false critical, keyUsage critical {digitalSignature,keyEncipherment}, extKeyUsage serverAuth, subjectAltName = `san_for(hostname)` (IP literal → `IpAddress`, else → `DnsName`); **NO authorityKeyIdentifier** (the TS comment: AKI from a hex-string SKI breaks chain verification; issuer/subject DN match suffices — omit it); signed by the CA key with SHA-256.
- [ ] **`server_config_for(ca: &MitmCa, hostname: &str) -> Arc<rustls::ServerConfig>`** — mint-or-cache (on `ca.server_configs`): build a `rustls::ServerConfig` with the leaf chain (leaf + CA certs) + leaf key, `alpn_protocols = ["http/1.1"]`. (This is the SNICallback target in P6b's terminating proxy.)
- [ ] **`clamp_validity`** + **`san_for`** helpers (faithful to mitm-leaf.js:88-101).
- [ ] **Tests:** mint a leaf for `example.com` → parses, CN=example.com, SAN=DNS:example.com, basicConstraints cA:false, EKU serverAuth, validity ≤ now+99d AND ≤ CA notAfter; mint for an IP `1.2.3.4` → SAN=IP:1.2.3.4; **chain verification**: build a rustls/webpki verifier trusting ONLY the CA cert and verify the minted leaf validates for the hostname (the core security property); caching returns the same leaf on second call; `server_config_for` builds + caches a ServerConfig with http/1.1 ALPN. Commit (`feat(sandbox-runtime): per-host MITM leaf minting + rustls ServerConfig (P6a)`).

### Task 3: gates
`cargo test -p sandbox-runtime` + clippy `-D warnings` + `cargo test --workspace --no-run` + `cargo tree -p engine-mobile -e normal | grep -c sandbox-runtime` (0) + frozen diff empty. Stage ONLY explicit sandbox-runtime + Cargo paths. **Confirm the new crypto deps (rcgen/rsa) do NOT leak into engine-mobile** (they're on the leaf crate only).

## Final verification
1. CA: RSA-2048 SHA-256, generate-ephemeral + load-user (RSA-enforced), the exact one-path error, 0700 dir / 0644 cert / 0600 key, dispose.
2. Leaf: RSA-2048, SAN DNS/IP, validity min(CA,+99d), no-AKI, chain=leaf+CA, signed by CA — and a webpki chain-verification test proves a CA-trusting client accepts the leaf for the host (the security property). ServerConfig cached with http/1.1 ALPN.
3. engine-mobile 0-dep (incl. rcgen/rsa); frozen empty.
