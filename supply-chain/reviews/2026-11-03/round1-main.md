# Main delivery round 1: fourteen complete source audits

This batch adds fourteen AI-assisted exact-version full source audits and their independent cross-review reports. It adds no delta record, exemption, wildcard/publisher trust, criteria mapping, source-classification policy or dependency update. Every report SHA referenced by each record was checked against the committed bytes. The package-local findings, consumer obligations and bounded verification limits remain in those reports.

The frozen cargo-vet check on the current main dependency graph reports **837 missing rows before and 824 after**, both exit 255 (`fail (vetting)`), with no metadata/store-format error. `utf8_iter 1.0.4` already has a path in the existing imported records, so fourteen added audits close thirteen currently missing rows. The overall source audit is not passing. The separate larger integration candidate has a different graph and stricter fork classification; its 758 count is not a main result.

Manifest SHA-256: `07c8e8f3342a7404e121a456eaa25ede832fdcce5b70b6ffcaf1df717959a5c0`.
Cargo.lock SHA-256: `0fce51526b391fbf2567b13590257cda989e5722bb2eb08c3b150d1712e97ea6`.
Unchanged config SHA-256: `70834ad5dcc08532d8eed1145e6c788a660f4910eb05afca493b232498129e3b`.
Unchanged imports.lock SHA-256: `45a7d0159870eb74066fe4d684f14536f2b21b1d3291e08b2fdfaacbdfec0f6a`.

The main lock currently pins LLM Git `3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3`, which could not be fetched from the owner SSH remote during this check. Its exact existing local Git object was loaded into the task Cargo cache for metadata resolution without changing manifest/lock; this confirms the stated graph only, not remote availability or a fresh product build. No unrelated LLM source was published. Remote pin availability remains a separate issue to resolve.

Local reproducible commands/results are retained at `validation/gates-fix-20261030/integration/source-audit-round1-20261103`. This source-audit batch does not alter runtime behavior or claim device/CI/product acceptance.
