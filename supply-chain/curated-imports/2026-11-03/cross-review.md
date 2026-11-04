# Independent cross-review: additional cargo-vet peer attestations

Reviewer: Codex (GPT-6.1 Terra; AI-assisted review), 2026-11-03. Scope is provenance, exact attestation fields, criteria, version chains, and a frozen-graph solver check. This is **not** an independent full-source audit of the third-party crates or a cryptographic correctness review.

## Disposition

I support the **12 specific version/delta attestations** in the frozen [strict candidate](../../../integration/vet-additional-peers-20261103/README.md) as importable third-party `safe-to-deploy` evidence, conditional on the project accepting the named auditors and the identified organizations as sources. The 13th selected record is an upstream `epaint <0.20.0` **violation**, not an audit; preserve it unchanged. I found no other basis to remove a strict candidate record. This judgment trusts the authors' express attestations at their exact stated criteria; it does not establish that their unpublished review process was thorough, confer trust on future versions or publishers, or attest crypto correctness.

Exclude Ariel's `litrs 1.0.0`: its `version` field encodes a full audit, while its notes discuss only new unsafe functions and changes. More specific evidence is the [original Ariel commit](https://github.com/ariel-os/ariel-os/commit/e3d6ce155c77fe5503fa1d914813dccb7462c119), which adds that full entry while removing Mozilla's `litrs 0.4.1` full audit from the imports lock; its [PR](https://github.com/ariel-os/ariel-os/pull/1667) was a dependency update moving to `litrs 1.0.0`. This supports, but does not prove, that the new review was a delta from the older baseline. There is insufficient evidence to assert a full review or to rewrite its entry into an independently established delta; leave it out unless scope is clarified or a substantive review supplies the missing assurance.

## Source and criterion checks

The official cargo-vet 0.10.3 [registry at fixed commit](https://github.com/mozilla/cargo-vet/blob/cef4dd43a0b9d2d37febf0e5ad42abc6d9ea5472/registry.toml) identifies the three candidate organizations' published audit sources; registry membership is a discovery and provenance signal, not a guarantee that any organization or individual did a correct audit. I independently retrieved the fixed-commit source files through GitHub and computed Git blob hashes of the received contents:

| Source | Fixed revision | Git blob |
| --- | --- | --- |
| [Embark Studios](https://github.com/EmbarkStudios/rust-ecosystem/blob/bc9f6c527108a8b79b74d44121aed574e7b5e526/audits.toml) | `bc9f6c527108a8b79b74d44121aed574e7b5e526` | `bbfef9f9fd75a9e1c0955863b4ba327468e28371` |
| [ISRG / Divvi Up](https://github.com/divviup/libprio-rs/blob/2e6282207930d439bf54bfaa79d841ef24c0d212/supply-chain/audits.toml) | `2e6282207930d439bf54bfaa79d841ef24c0d212` | `6bf08be3f87ed3a6c8342f0f0522a216341592f2` |
| [Zcash aggregate](https://github.com/zcash/rust-ecosystem/blob/ce132d7abdd52f2b3b0d55ed5dc9f152331535c2/supply-chain/audits.toml) | `ce132d7abdd52f2b3b0d55ed5dc9f152331535c2` | `d94538e9957149b53b261a0c62af4c607ef1b021` |

GitHub content at the frozen Ariel and Fermyon/Spin revisions likewise matched the candidate's stated blob hashes. The [Divvi Up project](https://divviup.org/about/) identifies itself as an ISRG project and points to its open-source Prio implementation. GitHub reports the Embark repository as archived, so these exact attestations remain historical evidence rather than an active update promise. GitHub reports both `zcash/wallet` and `zcash/zallet` as repository ID `863610221`; likewise the registry's Fermyon/Spin repository resolves to the same repository ID as `spinframework/spin`. Only Embark, ISRG and Zcash are used by the strict set.

I parsed every strict record, checked it against the identified fixed source and its cited line, and independently checked that each of the four Zcash aggregate records has identical `who`, criterion, version/delta and notes (where present) in the referenced upstream [librustzcash](https://github.com/zcash/librustzcash/blob/a551fac8a233f54799ca1920ef78ef098cfda929/supply-chain/audits.toml), [zcash](https://github.com/zcash/zcash/blob/558f686599586f55def3db86955d74d3be44605e/qa/supply-chain/audits.toml), or [zallet](https://github.com/zcash/zallet/blob/b0034a0925fc0c12eb4cd619114c3287b160093f/supply-chain/audits.toml) file. The aggregation-only `aggregated-from` field is preserved. Full fields, exact line links and source SHA-256 values are in [strict-evidence-records.json](../../../integration/vet-additional-peers-20261103/strict-evidence-records.json).

Each selected record declares exactly the built-in `safe-to-deploy` criterion and one of `version`, `delta` or `violation`; none is `importable = false`. I checked all three upstream audit files for violations: only Embark's `epaint <0.20.0` appears and it is retained. No `epaint` package is present in the frozen App lock. The proposed imported sections contain only `audits`, with no wildcard or trusted-publisher sections. There is no custom-criterion mapping or use of a `safe-to-run` record to satisfy deployment.

Some ISRG attestations have no explanatory notes. That is an evidence limitation, not proof of absence of review or, without contrary evidence, grounds to recast a `version` entry as a delta. I traced `hmac 0.12.1` to [commit 704c083](https://github.com/divviup/libprio-rs/commit/704c083f2cd94931cfc83baa0ca2cfc47f29a1da), `rand_chacha 0.3.1` to [commit 5d14959](https://github.com/divviup/libprio-rs/commit/5d14959e18fa7cec88f8a2e5f752705bd93bb511), and `rayon-core 1.12.1` to [commit c1fa40c](https://github.com/divviup/libprio-rs/commit/c1fa40cc10afc50c621c0cc52b31e7c4ee6dd062); all explicitly introduce the full-version fields and the named reviewer. The `rand_chacha 0.3.1 -> 0.9.0` and `inout 0.1.4 -> 0.2.2` entries were added in [rand upgrade](https://github.com/divviup/libprio-rs/commit/e6983b596513a122342fdd56fe41b3ab22d58d70) and [RustCrypto upgrade](https://github.com/divviup/libprio-rs/commit/7549f3c7dffe96d3a6e6fb214007911cdec3e8e6) commits that explicitly describe recording those audits; a [rand PR comment](https://github.com/divviup/libprio-rs/pull/1238) also discusses vetting remaining crate diffs. `rayon-core 1.12.1 -> 1.13.0` is introduced by [commit 222ca64](https://github.com/divviup/libprio-rs/commit/222ca643783ee233f6d1bf9c6270f2f307b377e9). These histories support provenance and recorded intent, not independent verification of each source-code line or an additional cryptography-specific criterion. The [cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html) define the scope of `safe-to-deploy`; any separate crypto-correctness criterion remains separate.

## Closure and independent execution

I independently ran `cargo vet check` with the strict research store, `--locked --frozen --no-registry-suggestions --no-minimize-exemptions`, offline Cargo configuration and a fresh temporary cache. It exited `255` with `fail (vetting)` and **749** unresolved entries, versus the separately recorded baseline of **758**; the failure set exactly matched the saved strict output, with nine removed `safe-to-deploy` failures and no new ones. I also independently ran nine `explain-audit` calls; each exited zero and produced the following exact paths:

| Frozen App target | Exact audit chain, all `safe-to-deploy` |
| --- | --- |
| `dunce 1.0.5` | Zcash full `1.0.5` |
| `hmac 0.12.1` | ISRG full `0.12.1` |
| `inout 0.1.4` | Zcash full `0.1.3` → Zcash delta `0.1.3 -> 0.1.4` |
| `inout 0.2.2` | previous chain → ISRG delta `0.1.4 -> 0.2.2` |
| `rand_chacha 0.3.1` | ISRG full `0.3.1` |
| `rand_chacha 0.9.0` | ISRG full `0.3.1` → ISRG delta `0.3.1 -> 0.9.0` |
| `rayon-core 1.13.0` | ISRG full `1.12.1` → ISRG delta `1.12.1 -> 1.13.0` |
| `tap 1.0.1` | Embark full `1.0.1` |
| `valuable 0.1.1` | Embark full `0.1.0` → Zcash delta `0.1.0 -> 0.1.1` |

The nine target lock entries are crates.io registry identities; I checked the target checksums against the local crates.io index. The three non-target anchors also appear in that index: `inout 0.1.3` checksum `a0c10553d664a4d0bcff9f4215d0aac67a639cc68ef660840afe309b807bc9f5`, `rayon-core 1.12.1` `1465873a3dfdaa8ae7cb14b4383657caab0b3e8a0aa9ae8e04b044854c8dfce2`, and `valuable 0.1.0` `830b7e5d4d90034032940e4ace0d9a9a057e7a45cd94e6c007832e39edb82f6d`. These identity checks do not add new audits or prove source soundness.

The five App inputs matched the strict run's frozen hashes before and after both independent executions, as did the three strict-store files. The App `Cargo.lock` SHA-256 was `221db470e0c34f2842b439ce0d4b2530f2093e5292871de6dde1d2f1af528b52`; the other hashes and exact command are in [strict-run.json](../../../integration/vet-additional-peers-20261103/strict-run.json). I verified all 29 files listed in the candidate's `MANIFEST.sha256` without mismatch. This is a solver result for that frozen graph, not a current-main or overall CI pass.

## Controlled import requirements

The [three strict peer snapshots](../../../integration/vet-additional-peers-20261103/strict-peers/) contain the exact approved-source fields, including the violation, and are review artifacts only. For a durable import, a project-controlled immutable HTTPS snapshot of each reviewed peer can be used with the normal `cargo vet import <name> <URL>` / `cargo vet regenerate imports` workflow, then checked against the real current graph. Those commands must actually run in a properly authorized isolated candidate first; snapshot availability, provenance, lock contents and the nine explanations must be verified before adopting them. The current synthetic research `imports.lock` only demonstrates solver behavior and is not proof that normal regeneration will preserve every needed row.

Pin snapshot URLs to immutable content, retain human-reviewable source/line provenance, and enforce a CI allowlist or equivalent check that the imported records are exactly the reviewed records, with no wildcard audits, publisher/trusted entries, `importable = false` entries, exemptions or custom criterion mappings. Future record updates require review. Directly pointing at the original broad Embark/ISRG files while relying on `imports.exclude` is not equivalent: cargo-vet 0.10.3 [`storage.rs`](../../../vet-research/fork-classification-current/official-source/cargo-vet-0.10.3/src/storage.rs) removes excluded names only from `audit_file.audits`, while wildcard entries are processed separately. Resolve snapshot hosting and acceptance of the named third-party reviewers before actual import; do not mistake a filename, source hash or registry listing for review approval.

No canonical file was edited in this cross-review. The isolated check and explanation commands used temporary caches and left both App and strict research store hashes unchanged.
