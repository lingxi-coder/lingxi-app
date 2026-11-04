# Include the actual main graph's third-party Git forks

Codex AI-assisted, 2026-11-03.

The current main graph includes ten additional third-party Git package identities that cargo-vet previously treated as first-party: icu_collator; lingxi-hyper, lingxi-hyper-rustls, lingxi-hyper-util, lingxi-reqwest; mbedtls-sys; the Git nix 0.31.3; platform-pty; syntect; and webrtc. The newly-included-forks.json evidence lists their exact Git-qualified versions. Main xcap 0.5.2 already comes from crates.io and is not added as a Git fork policy.

Explicit audit-as-crates-io policies require these forks to undergo third-party review. This does not assert registry equivalence: the actual frozen check reports all ten Git-qualified identities as missing. No registry record, trust, exemption, or new local audit is added. The existing five Git policies and these ten produce fifteen Git-qualified failures. For mbedtls-sys, review must cover the embedded native source and build boundary, not just its owned Rust wrapper.

With the same main Cargo files, imports and sixteen local records, frozen checking changes **813 to 823 missing rows**. The difference is exactly the ten newly included Git identities; no earlier gap closes or disappears in this classification change. The previous two full audits and nine public-audit paths still close eleven genuine gaps during this round; the strict current total also includes these ten previously uncounted forks. Overall vetting remains a failure.

The Git nix policy names its exact Git-qualified version because main also has three registry nix versions. A future Harness pin update must update this policy and recheck the complete graph. The source identity here is the exact locally cached Git object; the currently unavailable remote Harness/LLM pins remain a separate publication issue. This is classification and graph verification, not completion of those source audits, remote availability, or runtime acceptance.

Frozen commands and independent metadata/cross-review evidence are retained in validation/gates-fix-20261030/integration/main-fork-classification-20261103 and integration/vet-curated-formal-20261103. The curated snapshot validator and sixteen rejection tests still pass with the strengthened policies.
