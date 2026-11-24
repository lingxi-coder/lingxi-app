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
- [lingxi-app/apps/web/scripts/sync-docs.mjs](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/apps/web/scripts/sync-docs.mjs) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/07ad69d8d219502e63f918d84c3434c97edce8e6/apps/web/scripts/generate-handbook.ts) — Committed source (07ad69d8d219502e63f918d84c3434c97edce8e6)
- [lingxi-app/apps/web/scripts/generate-doc-baseline.py](https://github.com/lingxi-coder/lingxi-app/blob/a0cbef33938b241b04f04ddaa126850bf029e8ae/apps/web/scripts/generate-doc-baseline.py) — Committed source (a0cbef33938b241b04f04ddaa126850bf029e8ae)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/07ad69d8d219502e63f918d84c3434c97edce8e6/apps/web/scripts/check-doc-examples.py) — Committed source (07ad69d8d219502e63f918d84c3434c97edce8e6)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/b1f5855157fba3bd10e6bb8453fe1176b8377cb7/apps/web/examples/sdk_tools.py) — Committed source (b1f5855157fba3bd10e6bb8453fe1176b8377cb7)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/16cded0d03f9cd83b2e69436ee2367200268e088/crates/runtime/tests/sdk_surface.rs) — Committed source (16cded0d03f9cd83b2e69436ee2367200268e088)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/tests/downstream-docs/src/lib.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/5dd627e1049ba85e0563b7f351f79b00267c58c3/scripts/check-all.sh) — Committed source (5dd627e1049ba85e0563b7f351f79b00267c58c3)

- lingxi-app: HEAD e74379a16bb1946787a7edf327cd8367128c4357; Referenced input fingerprint sha256:b2d7aabb4ca87d35400a12fe665e0a43040123560edaadd23961a53bf191fc66
- harness-runtime: HEAD 16cded0d03f9cd83b2e69436ee2367200268e088; Referenced input fingerprint sha256:af27a44ee002fcbf278f333efb7f9daebd6c6fd92a6e21f86383c0deb5fea242; Product pin 16cded0d03f9cd83b2e69436ee2367200268e088
- llm-client: HEAD 63c8f7663b7cba7466538213f86c6df4a30442a9; Referenced input fingerprint sha256:2d8d308d351d3f94a029886f11db1bf931d3b28dd76d93e0f64e50665160e636; Product pin 63c8f7663b7cba7466538213f86c6df4a30442a9
- mobile-linux-runtime: HEAD 5dd627e1049ba85e0563b7f351f79b00267c58c3; Referenced input fingerprint sha256:5dd456cd771483f341bb99d25aff43140c96f893f6de5541c381c2083f162064; Product pin 224cc6b272e1e8ee4385a03efd34bd79b18fd8ce

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
