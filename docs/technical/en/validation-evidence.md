# Validation and evidence boundaries

Understand what documentation checks, consumer compilation, mock tests, provider calls and device acceptance actually prove.

<a id="levels"></a>

## Evidence levels

| Check | Proves | Requires separate acceptance |
| --- | --- | --- |
| Content/type/export checks | Bilingual coverage, routes, anchors, deterministic outputs and valid renderer contracts | Truth of every runtime capability |
| Compiler consumer examples | Declared dependencies and actual public type/method access | Model credentials, provider response or device backend |
| Mock runtime tests | Specific lifecycle, error and cancellation contracts without a real service | Native input, real service and production behavior |
| Live provider/device/package test | The tested target, identity and scenario | Untested platforms, models, parity or broad release readiness |

<a id="docs"></a>

## Handbook and source freshness

docs:check validates referenced working-tree sources, the API catalog and both language exports. docs:check:content validates saved content/baselines without requiring sibling SDK checkouts, suitable for documentation CI. Checks do not rewrite tracked files.

### Deterministic source/content checks

```sh
cd lingxi-app/apps/web
npm run docs:sync
npm run docs:check
npm run docs:check:content
npm run docs:examples -- --workspace ../../..
```

Verification method: Environment validation

<a id="consumers"></a>

## Verify the copied consumer

The example validator extracts marked Rust blocks from the handbook, tests supported standalone examples with their complete manifests and verifies SDK source identities. Isolated local overlays are explicit. A fixed Git installation example is verified against the fixed commit separately; compilation against a newer local SDK is not evidence for that Git revision.

compile and mock tags describe validation methods, not a claim that every environment has passed them. Read the validator report for actual pass, failure and prerequisite information.

<a id="parity"></a>

## Parity and dated reports

Native compatibility reports belong to specific versions, distributions, settings and fixtures. Byte-level protocol claims require raw native/runtime comparisons. Old design milestones, aggregate test counts and a green website build do not establish current platform or parity acceptance.

<a id="report"></a>

## Recording outcomes

1. Record the source revision and dirty-input fingerprint, target and feature set.
2. Separate compilation, mock execution, service calls, packaging and device evidence.
3. Retain failures and environment prerequisites; do not turn missing tooling into a passing claim.
4. Re-run the focused checks affected by subsequent source changes.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/3b61640f7128f33abf0dab433741befba0a80a48/.github/workflows/ci.yml) — Committed source (3b61640f7128f33abf0dab433741befba0a80a48)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/011ca150ec195d3d8324f10955f91ede556a03d7/scripts/check-all.sh) — Committed source (011ca150ec195d3d8324f10955f91ede556a03d7)
- [lingxi-app/.github/workflows/documentation.yml](https://github.com/lingxi-coder/lingxi-app/blob/138ce38bdd60e1b9984adf6e895e9228be7df6ec/.github/workflows/documentation.yml) — Committed source (138ce38bdd60e1b9984adf6e895e9228be7df6ec)
- lingxi-app/apps/web/scripts/sync-docs.mjs — Unpublished source (a0cbef33938b241b04f04ddaa126850bf029e8ae)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/07ad69d8d219502e63f918d84c3434c97edce8e6/apps/web/scripts/generate-handbook.ts) — Committed source (07ad69d8d219502e63f918d84c3434c97edce8e6)
- lingxi-app/apps/web/scripts/generate-doc-baseline.py — Unpublished source (a0cbef33938b241b04f04ddaa126850bf029e8ae)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/07ad69d8d219502e63f918d84c3434c97edce8e6/apps/web/scripts/check-doc-examples.py) — Committed source (07ad69d8d219502e63f918d84c3434c97edce8e6)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/b1f5855157fba3bd10e6bb8453fe1176b8377cb7/apps/web/examples/sdk_tools.py) — Committed source (b1f5855157fba3bd10e6bb8453fe1176b8377cb7)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/c551d1b4ac161cf9024dfcbd88ccd4c22f55de69/crates/runtime/tests/sdk_surface.rs) — Committed source (c551d1b4ac161cf9024dfcbd88ccd4c22f55de69)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/tests/downstream-docs/src/lib.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/5dd627e1049ba85e0563b7f351f79b00267c58c3/scripts/check-all.sh) — Committed source (5dd627e1049ba85e0563b7f351f79b00267c58c3)

- lingxi-app: HEAD a0cbef33938b241b04f04ddaa126850bf029e8ae; Referenced input fingerprint sha256:d24e5f9709f5f11d278f815a85503d35ceae930f5c575ea490d8bc78fe7266f5
- harness-runtime: HEAD c551d1b4ac161cf9024dfcbd88ccd4c22f55de69; Referenced input fingerprint sha256:6e9c6b19e59dad38ee1a5c755e041b15b7d37e041d16fb9ada609f303de3ea64; Product pin c551d1b4ac161cf9024dfcbd88ccd4c22f55de69
- llm-client: HEAD 63c8f7663b7cba7466538213f86c6df4a30442a9; Referenced input fingerprint sha256:c333c57f017ceb83b75e77ac252dc2cf3850ac8a74a5199f5d9154b7e698100c; Product pin 63c8f7663b7cba7466538213f86c6df4a30442a9
- mobile-linux-runtime: HEAD 5dd627e1049ba85e0563b7f351f79b00267c58c3; Referenced input fingerprint sha256:5dd456cd771483f341bb99d25aff43140c96f893f6de5541c381c2083f162064; Product pin 5dd627e1049ba85e0563b7f351f79b00267c58c3

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
