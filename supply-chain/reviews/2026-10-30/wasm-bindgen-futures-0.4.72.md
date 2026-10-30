# Source review: wasm-bindgen-futures 0.4.72

Reviewer: Codex AI-assisted (GPT-6.1 Terra source review, independently checked by Codex).
Criteria: safe-to-deploy. Scope: complete published package implementation and package manifests.

Registry archive SHA-256: `9473dbd2991ae90b6291c3c32c30c6187ac49aa32f9905d1cce280ec1e110b0f`, independently equal to the registry index and the frozen product Cargo.lock checksum.

The complete executable source only re-exports js_sys::futures items, plus a feature-gated stream re-export. It has no own unsafe code, build script, process, network, file or environment access. Its two exact-version dependencies and std/stream feature forwarding were reviewed; those dependency implementations require their own cargo-vet paths.

All executable source and package manifests were read; there is no additional binary payload or build script. Release metadata and documentation were distinguished from executable package behavior. This package-level review does not certify dependency implementations or claim full-platform execution.

Reviewed source/manifests:

- `Cargo.toml`: `5b5baa40e0c9e68e717c8e6ed2cc11ca5c92f8ba377308d363d029a1ddd110b5`
- `Cargo.toml.orig`: `ea9ed93eefe00df9ab2963034e3fc5f2539c4fccecc27c91dbb9ef718b2b3de6`
- `src/lib.rs`: `62b00d9c804cf54247a96d21c9ac8a172345d22faab7d7706838944b107139cd`

Original detailed review SHA-256: `35a47f38446643d770fec89775a08510a2e6097ecbd40c51a82c7022b93f5d34`.
Original actual-read manifest SHA-256: `2863afde3e8b93cac0c5fbea5efae251c7622b8aa919bfc64fd1b6f1518b61ac`.
