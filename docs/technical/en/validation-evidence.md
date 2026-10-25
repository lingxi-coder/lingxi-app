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

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/.github/workflows/ci.yml) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/scripts/check-all.sh) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/.github/workflows/documentation.yml](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/.github/workflows/documentation.yml) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/web/scripts/sync-docs.mjs](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/web/scripts/sync-docs.mjs) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/web/scripts/generate-handbook.ts) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/web/scripts/generate-doc-baseline.py](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/web/scripts/generate-doc-baseline.py) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/web/scripts/check-doc-examples.py) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/apps/web/examples/sdk_tools.py) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/tests/sdk_surface.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- llm-client/tests/downstream-docs/src/lib.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/scripts/check-all.sh) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- lingxi-app: HEAD abad4d9603ce6a0c032ddf089f69f88e927b76c4; Referenced input fingerprint sha256:bbb3c48b6b5761f33d3d66eb68ccaf089f9fd5ffc104138cbe084fb24e3b4f5b
- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin a39da75f0985d61d90fffa255f7b4d90901b0e27
- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8
- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; Referenced input fingerprint sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; Product pin d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
