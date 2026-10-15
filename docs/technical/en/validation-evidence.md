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

- [lingxi-app/.github/workflows/ci.yml](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/.github/workflows/ci.yml) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/scripts/check-all.sh](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/scripts/check-all.sh) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/.github/workflows/documentation.yml](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/.github/workflows/documentation.yml) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/web/scripts/sync-docs.mjs](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/web/scripts/sync-docs.mjs) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/web/scripts/generate-handbook.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/web/scripts/generate-handbook.ts) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/web/scripts/generate-doc-baseline.py](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/web/scripts/generate-doc-baseline.py) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/web/scripts/check-doc-examples.py](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/web/scripts/check-doc-examples.py) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/apps/web/examples/sdk_tools.py](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/web/examples/sdk_tools.py) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [harness-runtime/crates/runtime/tests/sdk_surface.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/tests/sdk_surface.rs) — Committed source (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [llm-client/tests/downstream-docs/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/tests/downstream-docs/src/lib.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [mobile-linux-runtime/scripts/check-all.sh](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/scripts/check-all.sh) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)

- lingxi-app: HEAD ed180d70d6dc36d7bed12c654620d0bccb231883; Referenced input fingerprint sha256:ef51a25cc818084d97b831957cb840fd84e1a4666089524025c7544ee886acda
- harness-runtime: HEAD 1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d; Referenced input fingerprint sha256:d77506e15375f8653d0ae9198567f3dbeba4e931dce8226b58971350a635ae8a; Product pin 1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d
- llm-client: HEAD 179d336607b2367cf0709a78c760c9333ec25bb3; Referenced input fingerprint sha256:64423ae6bcb4985bab0883b693378509bb7fe060e59bf4facafedcb4d2f32293; Product pin 450d52b12b6d88a377ab3332c3b8a6a1820e2b7c
- mobile-linux-runtime: HEAD a632e878dc597dc41f6a3712c5a5cdebb794c99c; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
