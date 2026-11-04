# urlencoding 2.1.3: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed full source review**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `daf8dba3b7eb870caf1ddeed7bc9d2a049f3cfdfae7cb521b087cc33ae4c49da`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `49f3c7ae9b91b70ca5fc2f6496bd5de684531d0f`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"3cc277fc4e577a0bef99294d7ab9bc85232f0730"},"path_in_vcs":""} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `(none)`. Direct reverse edges: llm-runtime 0.1.0 (normal, all targets), mcp 0.12.0 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

## Independently examined implementation

encode_into copies only the explicit ASCII unreserved set and percent-encodes each other byte from two nibbles; the unchecked str conversions therefore receive ASCII (src/enc.rs:78-130). It propagates writer errors. decode_binary preserves malformed/incomplete % sequences and literal +; decode rejects non-UTF8 output (src/dec.rs:19-81). NeverRealloc silently drops a write if capacity is insufficient, but each decoding step emits no more bytes than it consumes and the output allocation is the original input length. This is a byte percent codec, not a form decoder, URL parser or canonicalization policy. No local FFI, network or subprocess; callers handle allocation limits and security-sensitive parsing separately.

Target files examined (SHA-256):

- `Cargo.toml`: `7336fe00c121d144202b7e1787fbed8fed38898b160b94bba3e82ab90d31e2a9`
- `src/lib.rs`: `8be9baf406ede16922deb1947601a6e5405579521fa74fea41d1d3095dfe13de`
- `src/enc.rs`: `50242a4f6b63deb98e016b6a6ebc122f519c4bead7c2c36ccce24d2268541ec8`
- `src/dec.rs`: `27f7a50d426fd295e88e440358e084bb1b2d54283c6cbbbc99ddce7383bb37a5`

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
