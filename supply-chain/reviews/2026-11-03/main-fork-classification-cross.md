# Delivery-main Git fork classification: independent cross-review

Reviewer: Codex (GPT-6.1 Terra; AI-assisted), 2026-11-03. **Supported:** the proposed ten `audit-as-crates-io = true` policies correctly make the listed third-party source-bearing packages subject to cargo-vet review on the delivery graph. This is a classification and provenance review, not a review or certification of the packages' full source.

The actual delivery `Cargo.lock` SHA-256 is `0fce51526b391fbf2567b13590257cda989e5722bb2eb08c3b150d1712e97ea6`. I ran `cargo metadata --locked --offline --all-features` from the delivery App root, checked every listed Git package and revision against that lock, and verified each checked-out package manifest byte-for-byte against the Git object at the **complete locked commit**. The metadata SHA-256 and exact package IDs, manifest paths, upstream repository metadata and parent edges are in [main-fork-metadata-review.json](main-fork-metadata-review.json). The table abbreviations below are:

- **H**: `git+https://github.com/lingxi-coder/harness-runtime.git` at `e59c7ed9720edd2cbae866c193c8757948aec490`.
- **L**: `git+https://github.com/lingxi-coder/llm-client.git` at `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3`.
- **M**: `git+https://github.com/lingxi-coder/mobile-linux-runtime.git` at `43382a20161e371a0a31526412164b3d35545ab5`.

| Newly classified Git package | Locked version/source | Source provenance inspected |
| --- | --- | --- |
| `icu_collator` | `2.3.1`, H | `third_party/icu_collator`; manifest identifies ICU4X. |
| `lingxi-hyper` | `1.9.0`, L | `vendor/lingxi-hyper`; manifest identifies hyperium/hyper. |
| `lingxi-hyper-rustls` | `0.27.5`, L | `vendor/lingxi-hyper-rustls`; manifest identifies rustls/hyper-rustls. |
| `lingxi-hyper-util` | `0.1.21`, L | `vendor/lingxi-hyper-util`; manifest identifies hyperium/hyper-util. |
| `lingxi-reqwest` | `0.12.29`, L | `vendor/lingxi-reqwest`; manifest identifies seanmonstar/reqwest. |
| `mbedtls-sys` | `3.6.2`, H | A **LingXi-local build seam**, not the unrelated same-named registry crate. Its manifest says `publish = false`; `build.rs` dynamically collects C files under sibling `third_party/mbedtls/library` and includes sibling `include`. The locked C headers identify Mbed TLS 3.6.2. Audit its actual native inputs and build configuration too. |
| `nix` | `0.31.3`, H | `packages/webrtc/vendor/nix`; manifest identifies nix-rust/nix. Its policy is correctly qualified to `nix:0.31.3@git:e59c7ed9720edd2cbae866c193c8757948aec490`; three other `nix` versions in this graph are registry packages. |
| `platform-pty` | `0.12.0`, M | The locked crate's `NOTICE` identifies adaptation from OpenAI Codex at `b8c2d29cc23b41fa7c7f5f5483e92fc71099635a`, and its four Windows ConPTY source headers say they originate from WezTerm. This is a local implementation containing substantial third-party source, so including its source in the required review is justified. |
| `syntect` | `5.3.0-harness.1`, H | `third_party/syntect`; manifest identifies trishume/syntect. |
| `webrtc` | `0.20.3`, H | `packages/webrtc/vendor/webrtc`; manifest identifies webrtc-rs/webrtc. |

I also inspected the commit-matched `mbedtls-sys` manifest and build script and the `platform-pty` NOTICE and Windows source headers; tracked-file checks showed those files match their locked checkout commits. The source origins above are provenance, not proof of unchanged upstream code. Unlike the historical integrated graph, **delivery main's `xcap 0.5.2` is a crates.io registry package**, so no new Git policy for it is warranted here.

## Exact policy and frozen results

The baseline already had five policies (`askama_derive`, `askama_parser`, `git2`, `libgit2-sys`, `libssh2-sys`). I parsed the working candidate against the recorded baseline: it adds exactly the ten policies listed above, all `true`; every existing policy and other config field is unchanged, and `audits.toml` and `imports.lock` are byte-identical. No record, exemption, wildcard, trust or criterion reduction was added by this change.

Independent official `cargo-vet 0.10.3 check` runs used the actual delivery manifest, the two stores, separate temporary caches, default all-feature graph, and `--locked --frozen --no-registry-suggestions --no-minimize-exemptions`. Both exited `255` with `fail (vetting)`: **813 before → 823 after**. The difference is exactly the ten Git-qualified `safe-to-deploy` failures above; no prior failure closed. **All 15 Git-qualified packages, including the original five, remain failures.** Commands, exact qualified versions and before/after file hashes are in [main-fork-check-independent.json](main-fork-check-independent.json); raw output is in the adjacent `fork-independent-*.stdout` files. The candidate config SHA-256 at review was `1563fe1f0cdf33370b6c620dfda3f9250f36aaca86c3f3c32705ce03ccb2e60d`. All checked inputs were unchanged during both runs.

In cargo-vet 0.10.3, `PackageExt::is_third_party` uses the policy to force the package into third-party evaluation, and `vet_version` carries the Git revision. The flag is therefore **not itself** a claim that Git source equals a crates.io release, nor an audit. Current results substantiate that the affected Git versions remain unaudited. Other cargo-vet mechanisms, especially an `unpublished` mapping, can create different equivalence behavior; the current store has no such mapping or exemption for these 15 names, and future changes must not silently add one. Exact-source review and independently verified source/build inputs remain necessary.

No canonical file was edited by this cross-review. Counts are scoped to the recorded delivery lock, candidate config and full graph; they are not an overall CI pass or device/runtime acceptance.
