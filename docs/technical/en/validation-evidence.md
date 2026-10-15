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

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/.github/workflows/ci.yml) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/scripts/check-all.sh) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [lingxi-app/.github/workflows/documentation.yml](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/.github/workflows/documentation.yml) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [lingxi-app/apps/web/scripts/sync-docs.mjs](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/apps/web/scripts/sync-docs.mjs) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/apps/web/scripts/generate-handbook.ts) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [lingxi-app/apps/web/scripts/generate-doc-baseline.py](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/apps/web/scripts/generate-doc-baseline.py) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/apps/web/scripts/check-doc-examples.py) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/135250505a5277d520f03aa2d1a411f03ff6949b/apps/web/examples/sdk_tools.py) — Committed source (135250505a5277d520f03aa2d1a411f03ff6949b)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/d5e4990edbfbae00753f32f259a7a49658eecaf2/crates/runtime/tests/sdk_surface.rs) — Committed source (d5e4990edbfbae00753f32f259a7a49658eecaf2)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/tests/downstream-docs/src/lib.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/scripts/check-all.sh) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)

- lingxi-app: HEAD 135250505a5277d520f03aa2d1a411f03ff6949b; Referenced input fingerprint sha256:ac73bd146078949f2a1c8f7801b199c918f6beef6e2ce215544e06d08ef33fb3
- harness-runtime: HEAD d5e4990edbfbae00753f32f259a7a49658eecaf2; Referenced input fingerprint sha256:87cee15ec392d0bc954fbf98a046813bbe7d38d0c257d335f4a74e606d22bd92; Product pin d5e4990edbfbae00753f32f259a7a49658eecaf2
- llm-client: HEAD 7333dbc3795b6bad893cb7287088d80931538630; Referenced input fingerprint sha256:51afdece5bf1e2d135f676f447d6ff43c84aaa464328f96ba19fa1e5010928af; Product pin 7333dbc3795b6bad893cb7287088d80931538630
- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
