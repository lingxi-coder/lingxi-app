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

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/.github/workflows/ci.yml) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/scripts/check-all.sh) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/.github/workflows/documentation.yml](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/.github/workflows/documentation.yml) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/apps/web/scripts/sync-docs.mjs](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/apps/web/scripts/sync-docs.mjs) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/apps/web/scripts/generate-handbook.ts) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/apps/web/scripts/generate-doc-baseline.py](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/apps/web/scripts/generate-doc-baseline.py) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/apps/web/scripts/check-doc-examples.py) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/apps/web/examples/sdk_tools.py) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/runtime/tests/sdk_surface.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/tests/downstream-docs/src/lib.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/25cf8a273bcd572b9be48e18fc9807e01dcdfce1/scripts/check-all.sh) — Committed source (25cf8a273bcd572b9be48e18fc9807e01dcdfce1)

- lingxi-app: HEAD fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64; Referenced input fingerprint sha256:a5bfcf25a49c613b97f21fc4507c7f97a198e934f9d09398cb8794e7d86f49e3
- harness-runtime: HEAD bb7a016c305609a0b4ca7b0c1a91faead18c4db0; Referenced input fingerprint sha256:700726d3f012384673f588a15ab3a32ecaf4f3fc499f0e4e86c0b8e6bc321559; Product pin ae5a65b1f4816cd26a06d462573ebc9841f14f96
- llm-client: HEAD 66ce76a2ecc679f0dba551e26ef6aaceb40d2f49; Referenced input fingerprint sha256:89fef6f87fb7803bec69d768fa11e408a54f2123d196e2c859d8faba79092d96; Product pin 3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3
- mobile-linux-runtime: HEAD 25cf8a273bcd572b9be48e18fc9807e01dcdfce1; Referenced input fingerprint sha256:3f654be245e2810979441677e37d079538d03bf98bfb68f98d054e4cc79d84df; Product pin 43382a20161e371a0a31526412164b3d35545ab5

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
