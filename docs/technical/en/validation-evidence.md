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

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/2c1528a557a52d8652f4cee2d9bb3e46f7d569d6/.github/workflows/ci.yml) — Committed source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/2c1528a557a52d8652f4cee2d9bb3e46f7d569d6/scripts/check-all.sh) — Committed source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- lingxi-app/.github/workflows/documentation.yml — Uncommitted source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- lingxi-app/apps/web/scripts/sync-docs.mjs — Uncommitted source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- lingxi-app/apps/web/scripts/generate-handbook.ts — Uncommitted source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- lingxi-app/apps/web/scripts/generate-doc-baseline.py — Uncommitted source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- lingxi-app/apps/web/scripts/check-doc-examples.py — Uncommitted source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- lingxi-app/apps/web/examples/sdk_tools.py — Uncommitted source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/e18e31ef3fa0b739147f13db3c2c9e61e815a9f5/crates/runtime/tests/sdk_surface.rs) — Committed source (e18e31ef3fa0b739147f13db3c2c9e61e815a9f5)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/downstream-docs/src/lib.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/scripts/check-all.sh) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)

- lingxi-app: HEAD 2c1528a557a52d8652f4cee2d9bb3e46f7d569d6; Referenced input fingerprint sha256:3e09a114ea3046bd62d2333be94726f0169efbe46b3cd64422178d54079069f9
- harness-runtime: HEAD e18e31ef3fa0b739147f13db3c2c9e61e815a9f5; Referenced input fingerprint sha256:ee5e12368e31aa0021cee361f5c031d077c0371600228a09970bfd11e0bda8ab; Product pin 05f2e97050bcda8e36df4bc533267acb740b8bd9
- llm-client: HEAD ce32fd07402c13fbfcfa185942162a87907ea0a1; Referenced input fingerprint sha256:a78ca8575ddb70589a8cc04ff4f043825bd52db40d751181584ea10118bfe2b4; Product pin 87db6e234e32cbc251a128d6b370cdda36c4d0c0
- mobile-linux-runtime: HEAD eb82dcd73080bc35d0236abc15209a3bbee9f52c; Referenced input fingerprint sha256:78374cc6ec701b4b4e082679fb8033d488b6379c3345bd5d03cd383177c3daf4; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
