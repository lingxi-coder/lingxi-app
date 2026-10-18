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

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/.github/workflows/ci.yml) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/scripts/check-all.sh) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [lingxi-app/.github/workflows/documentation.yml](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/.github/workflows/documentation.yml) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [lingxi-app/apps/web/scripts/sync-docs.mjs](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/apps/web/scripts/sync-docs.mjs) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/apps/web/scripts/generate-handbook.ts) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [lingxi-app/apps/web/scripts/generate-doc-baseline.py](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/apps/web/scripts/generate-doc-baseline.py) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/apps/web/scripts/check-doc-examples.py) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/550ec4bc9f03342673dc498729ab2a8c2a745462/apps/web/examples/sdk_tools.py) — Committed source (550ec4bc9f03342673dc498729ab2a8c2a745462)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/44f2eef5baf8ae16254934dbca451cbfd528d955/crates/runtime/tests/sdk_surface.rs) — Committed source (44f2eef5baf8ae16254934dbca451cbfd528d955)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/tests/downstream-docs/src/lib.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/scripts/check-all.sh) — Committed source (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)

- lingxi-app: HEAD 550ec4bc9f03342673dc498729ab2a8c2a745462; Referenced input fingerprint sha256:5ed2e4d8ce8b971fe311f75369fd5ba6caff744a02eebe282446b32085baf11a
- harness-runtime: HEAD 44f2eef5baf8ae16254934dbca451cbfd528d955; Referenced input fingerprint sha256:25269e99665402275451ca5949e4177e613143620f1cc8cabdbf422dcd7ab8a2; Product pin 44f2eef5baf8ae16254934dbca451cbfd528d955
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e
- mobile-linux-runtime: HEAD 6b033d41a2b6b736fda1e8f996f55a4b20bd885d; Referenced input fingerprint sha256:435a888b1a310f357f18311df5f96418fa72b1a900d78e987af282bb3e91d7ca; Product pin 6b033d41a2b6b736fda1e8f996f55a4b20bd885d

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
