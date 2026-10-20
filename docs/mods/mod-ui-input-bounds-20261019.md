# Long-session Mod UI input bounds

## Cause and change

The Host accepts parent-render `props` only within depth 32, 20,000 JSON values
and 100,000 serialized UTF-16 units. Desktop sent full tool results and grouped
calls even when they could not pass that contract. Subsequent site invalidation
and resize repeated the rejected request. The observed prior signed package
recorded 215 serialized-character-limit refusals after its launch.

Read-only size diagnosis of the affected persisted conversation found 74 tool
results, including 17 Read results above 100,000 units; the largest was about
256k units. Only sizes and tool names were saved, not user input/output bodies.
Those diagnostic counts use Python compact JSON; exact boundary tests use the
JavaScript encoder and are kept separate from byte-comparison claims.

Desktop now checks parent props before mounting the Mod render runtime. A site
outside the existing Host bounds renders its complete native children instead.
No fields, arrays or strings are truncated or re-encoded for transport. Eligible
props are passed by the same object reference. Becoming oversized unmounts the
previous runtime and releases its exact owned render revision; becoming eligible
again mounts a fresh runtime. Independent sites such as Weather remain active.
Tool execution and Mod `tool.call` recording are unchanged.

The shared plain-data validator also replaces the equivalent response-props
validator. It counts JSON escapes, surrogate pairs and lone surrogate units,
keys, punctuation and finite-number encoding without building another oversized
serialized string. It rejects malformed/cyclic data without calling getters or
custom `toJSON` functions. The Host, preload queue, and main queue limits remain
unchanged; this does not expand the set of payloads accepted by the Host.

## Verification

- Exact 100,000-unit boundary and one-unit overflow, including escaped controls,
  property keys, valid/lone surrogates, Unicode, numeric forms and punctuation.
- Depth/value boundaries, shared subtrees, sparse/cyclic and non-JSON data.
- Complete native fallback and eligibility for normal-size sites.
- Mounted Electron regression: valid -> oversized -> valid; no oversized IPC or
  invalidation/resize retries; exact old revision released; all native text
  retained; original eligible props preserved; Weather sibling stays visible.
- The mounted regression fails against the original main component and passes
  against the fixed candidate. Initial sandbox startup aborts and a test-driver
  top-level-readiness timeout were diagnosed separately; neither is reported as
  a production renderer regression.
- All 29 relevant Mod UI tests passed, including the existing Electron-backed
  Client input/revision test; the additional mounted-bounds Electron test passed.
- Five maintained Weather/Replay tests and node/web TypeScript checks passed.

Detailed source, diagnosis and test receipts are under
`.buildcache/mod-ui-input-bounds-20261019/` in the multi-repository workspace.
No new Native CLI collection, provider requests, tokenizer sweep or mobile build
was required for this renderer-only change. The alignment target stays 2.1.295.

## Signed package

Flare signing and static verification passed for the package under
`.buildcache/mod-ui-input-bounds-20261019/package/`. ZIP SHA-256:
`dfa2faf380d971cb126d8c29da5635e01d6481ceccff3968311fb7de78fbb697`.
The unchanged signed binary passed the standard smoke checks: renderer/security,
bundled sidecar, terminal, Git, scheduled-task restart persistence, encrypted
custom-provider credential restart, authenticated localhost request and cleanup.

The bootstrap check had incorrectly required an in-progress placeholder even
when hydration had already completed. Its regression now accepts the idle
placeholder only with a connected Bridge and trusted workspace. All eight smoke
contract tests passed. Earlier cold-start and credential/CDP timing failures are
retained in the evidence directory; only the completed run is recorded as passing.

## Real desktop acceptance

The signed app resumed the existing AGAI 18-message session and expanded its
32-tool group, showing native tool rows and Read image containers. Weather stayed
visible at 21%, 208.4k / 1M; the composer returned to its idle state. From launch
04:04:25.440Z through 04:07:03.609Z on 2026-10-20, diagnostics recorded zero Mod UI
control failures, zero serialized-limit rejections and zero crash events. This
is bounded observation, not a long-duration crash guarantee. Full large-string
retention and resize/invalidation behavior are proved by the mounted regression;
no new model turn or user project write was used for this GUI check.
