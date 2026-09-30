# Shared LLM execution migration

Status: implementation and validation complete.

This extends the external-client migration from wire-codec integration to the
shared client's complete request lifecycle. The upstream implementation is published on `codex/lingxi-runtime-integration`
as `9b0323f10f76c5834acafedaa04470c4e96346f2`. The dependency is pinned to this
full Git revision; no local source checkout is required.

## Implemented boundary

- Request execution uses upstream `RequestDraft`, `PreparedCall`,
  `ReceivedCall`, `CollectedResponse` and `ModelStream::next_batch`.
- Application request fields are finalized before authentication/signing.
  The signing bytes, sent bytes and exact UTF-16 representation share one
  serializer. Final effective service-tier fields are retained for pricing.
- Physical dispatch markers remain synchronous and every retry receives a
  separate call and application lease. Usage is retained before schema decoding
  or event delivery, including error envelopes and usage-only stream chunks.
- Upstream `ResponsesSession` owns prewarm, incremental requests, connection
  reuse and cancellation invalidation. A send-stage failure never causes an
  implicit second send. Host budget admission is outside the network watchdog.
- File transfer and explicit readiness polling use upstream `FileService`.
  Gemini upload capability URLs must share the configured origin and do not
  receive a repeated API key on the second upload leg.
- Exact counting, model resolution, SSE/AWS framing, AWS SigV4, usage decoding
  and price arithmetic are shared implementations. Host naming precedence is
  selected explicitly through `RoutingCatalog` rather than duplicated.
- Actual non-stream response estimates use frozen selected-model prices,
  complete usage and observed inference facts. Explicit host price overrides
  are applied before dispatch. Unknown Fast and non-USD values never become
  Standard/USD costs.

The application retains configuration/UI projections, account acquisition and
refresh, platform networking, retry and fallback decisions, history/tool-input
policy, conservative context-fit estimates, budget admission, and durable ledger
settlement. The legacy-shaped transport methods remain adapter seams for
in-memory fixtures and existing platform callers; the production execution path
uses raw bytes and the upstream executor. Codec projections remain for host-type
conversion and protocol fixtures, not as a production execution alternative.

## Validation

- Final shared-client full suite: 644 tests passed; strict all-target/all-feature
  Clippy passed. Later focused tests cover final request signing, dispatch veto,
  native model-ID precedence and final-body Fast inference with lone surrogates.
- Pinned-source runtime/provider-config full suite: 1,251 tests passed. Protocol unit
  coverage moved upstream, so this is not directly comparable to the prior
  in-tree total.
- Final service suite: 146 tests passed, plus a service-entry heartbeat watchdog
  regression. Large preparation futures are heap-allocated; default-stack
  connection-failover tests passed without increasing thread stack size.
- Final Fusion suite: 15 tests passed, including concurrent revocation during
  durable intent admission. HTTP attempts do not hold the WebSocket session
  mutex while waiting for persistence acknowledgements.
- New raw-transport regression verifies that the old execution methods are not
  called and that the AWS signature matches final host headers and sent bytes.
- WebSocket regression verifies cancellation clears continuation and forces a
  fresh connection. The 15 existing/new transport-stream tests passed.
- Final orchestrator streaming-loop suite: 10 tests passed.
- Electron TypeScript checks passed. Final iOS arm64 simulator and Android arm64
  Rust target checks passed. No mobile UI or packaging contract changed.
- The final `npm run package:mac:flare -- --launch` passed Flare signing, static
  package checks and packaged-app smoke checks (renderer/sidecar, terminal, Git,
  Keychain restart persistence, authenticated localhost inference, permissions
  and scheduled tasks). The verified application was launched from
  `apps/electron/dist/LingXi-Code-0.1.0-mac-arm64/LingXi Code.app`.
- Final `cargo check --workspace --all-targets --locked --offline` passed.
  Existing workspace documentation/mobile warnings remain unchanged.

No real-provider billed calls or physical-device acceptance is included. Mobile
compilation is not a newly packaged mobile release. Existing unrelated worktree
changes remain outside this migration.
