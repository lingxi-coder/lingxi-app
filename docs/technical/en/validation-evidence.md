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

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/.github/workflows/ci.yml) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/scripts/check-all.sh) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/.github/workflows/documentation.yml](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/.github/workflows/documentation.yml) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/apps/web/scripts/sync-docs.mjs](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/apps/web/scripts/sync-docs.mjs) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/apps/web/scripts/generate-handbook.ts) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/apps/web/scripts/generate-doc-baseline.py](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/apps/web/scripts/generate-doc-baseline.py) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/apps/web/scripts/check-doc-examples.py) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/apps/web/examples/sdk_tools.py) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/98fc362da3fabfe36aa5610b9e93818923c1ab31/crates/runtime/tests/sdk_surface.rs) — Committed source (98fc362da3fabfe36aa5610b9e93818923c1ab31)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/tests/downstream-docs/src/lib.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/scripts/check-all.sh) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)

- lingxi-app: HEAD ec6ff963545c8d02e0c9c220a22f07874ba62959; Referenced input fingerprint sha256:b657a9b5e03a337d3dce34527025cc7cd87afb3439754fe074426c39589fd716
- harness-runtime: HEAD 98fc362da3fabfe36aa5610b9e93818923c1ab31; Referenced input fingerprint sha256:8ac34e7055ee1007cfccf0eafbeaa5e2437c2f20dc623f1069d255177d27dd90; Product pin 98fc362da3fabfe36aa5610b9e93818923c1ab31
- llm-client: HEAD 46240a65df198a654d7d6de1c0bd99c23fcbb979; Referenced input fingerprint sha256:548db437ccaaeec3ce6ad3beb931ef868b263f6ad9b3c47309a3e00c514326dd; Product pin 46240a65df198a654d7d6de1c0bd99c23fcbb979
- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
