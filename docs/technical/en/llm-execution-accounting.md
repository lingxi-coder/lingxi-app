# Routing, failures & attempt accounting

Choose convenience failover or host-controlled one-shot calls and preserve actual usage before output interpretation.

<a id="resolution"></a>

## Resolution and fallback credentials

A provider identity, exact profile, connection group, protocol, wire model and billing mode are distinct. resolve()/complete() resolve model aliases and ordered connections; complete_in selects a starting profile or configured group. PreparedCall::prepare_on requires an exact connection. Region filters, hidden connections, wire model identity and billing compatibility constrain fallback; ambiguous model rows are not guessed.

RequestOptions.protocol overrides the profile default through explicitly registered protocol_profiles. Each alternate profile owns endpoint/auth/capability policy. credential and request-local authenticator belong only to the selected primary; named fallback profiles must receive their own fallback_credentials. Never reuse the primary secret by inferring that two endpoints are the same account.

| Execution path | Replay policy | Use |
| --- | --- | --- |
| chat().complete/stream | Configured ordered fallback only when trigger and execution safety admit it; not a same-profile retry loop | Convenience application request |
| snapshot.prepare_on → dispatch_once | One exact connection, one generation dispatch; no automatic retry/fallback | Host admission and physical-attempt ledger |
| Audio / images / embeddings / native mutations | Operation-specific single-dispatch and unknown-outcome contracts | Independent service lifecycle |

<a id="failure-classes"></a>

## Errors and host retry decisions

LlmError::kind classifies typed errors and ProviderResponse preserves HTTP status, request ID, native body, classification and retry_after where available. FailoverTriggers independently covers rate limits/quota, overload, provider internal/timeout, network/transport timeout and authentication/permission. A standalone default profile has no implied fallback chain.

- InvalidRequest, UnsupportedCapability, ContextOverflow, RequestTooLarge and TLS certificate errors do not become generic endpoint-switch triggers. Fix the input/configuration, compact in the host, or repair transport trust as appropriate.
- A returned stream does not fail over after read failure. A continuation pins the original account/profile/endpoint/model, and side-effecting hosted requests can suppress replay before dispatch.
- A Transport must not silently retry or follow credentialed redirects. The host records an uncertain dispatched attempt before deciding whether another call is acceptable; an error alone does not prove zero execution or zero cost.

<a id="attempt-example"></a>

## Capture observations before decoding

Preparation freezes route and prices, resolves/uploads admitted attachments, encodes final bytes and authenticates; it does not dispatch generation. PreparedCall is consumed once. RequestDraft allows a final synchronous host transformation before seal; dispatch_once_with marks host admission immediately before send. Preparation can still perform attachment I/O or WebSocket connection work.

Collect usage and inference before decode because failed statuses or malformed tool payloads may carry accounting facts. finish consumes the collected response and performs attachment cleanup. Preserve the ledger on every path, including dispatch uncertainty, partial stream usage, content validation failure and cancellation.

### One prepared physical attempt with frozen pricing and cleanup

```rust
use lingxi_llm_client::{LlmClient, RequestOptions};
use lingxi_llm_client::codecs::RequestMode;
use lingxi_llm_client::protocol::{ChatRequest, Submission};

pub async fn one_attempt(
    client: &LlmClient,
    request: &ChatRequest,
    options: &RequestOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = client.snapshot();
    let call = snapshot.prepare_on("openai", request, options, RequestMode::Complete).await?;
    let prices = call.pricing_snapshot();
    let received = call.dispatch_once().await?;
    let collected = received.collect().await?;
    let usage = collected.usage_report().clone();
    let inference = collected.inference_report().clone();
    // Persist these observations in the host before interpreting application content.
    let estimate = prices.estimate(&usage, &inference, Submission::Interactive);
    let response = collected.decode();
    collected.finish().await;
    println!("Usage: {usage:?}; estimate: {estimate:?}");
    println!("{}", response?.message.text());
    Ok(())
}
```

Verification method: Compile check (llm-attempt-accounting)

<a id="pricing-and-accounts"></a>

## Frozen prices and independent account reports

UsageReport distinguishes Missing, Partial, Complete and Invalid observations; absent fields are not zero. FrozenPricing estimates only when usage and execution facts support a valid price, retaining currency and selected model/connection. Requested Fast alone is insufficient when the executed tier is unknown. Anthropic server fallback-summary cost has separate component/completeness handling; partial components must not become a complete total.

client.account_usage(profile, query) does not generate model output. AccountQuery separates API/auth-user identity, ordinary versus management/service credentials, exact selector scopes and UTC time range. AccountMetric distinguishes available, unsupported, credential-required, not-reported and failed states for balance, token history, cost history, quota windows and subscription; an account aggregate is not an attempt ledger or final invoice.

### Query the exact account separately from generation

```rust
use lingxi_llm_client::{AccountIdentity, AccountQuery, AccountSnapshot, LlmClient};
use lingxi_llm_client::protocol::Secret;

pub async fn account_snapshot(
    client: &LlmClient,
    credential: Secret<String>,
) -> Result<AccountSnapshot, Box<dyn std::error::Error>> {
    let mut query = AccountQuery::new(AccountIdentity::ApiKey);
    query.credential = Some(credential);
    // Supply management_credential/selectors separately if this source requires them.
    Ok(client.account_usage("openai", &query).await?)
}
```

Verification method: Compile check (llm-account-query)

execution_contracts, failover and pricing tests validate local behavior with mocked observations. This handbook does not report a live charge, provider invoice, account balance or a full device acceptance result.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/llm-client)

- llm-client/src/client/executor.rs — Uncommitted source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/src/client/resolve.rs — Uncommitted source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/src/client/prepared.rs — Uncommitted source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/client/route.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/route.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/src/client/options.rs — Uncommitted source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/client/frozen_pricing.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/client/frozen_pricing.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/protocol/usage_report.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/protocol/usage_report.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- llm-client/src/protocol/provider.rs — Uncommitted source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/protocol/llm.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/protocol/llm.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/account/service.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/account/service.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/account/types.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/account/types.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/src/account/execution.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/account/execution.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/execution_contracts.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/execution_contracts.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/failover.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/failover.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [llm-client/tests/server_fallback_pricing.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/tests/server_fallback_pricing.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)

- llm-client: HEAD ce32fd07402c13fbfcfa185942162a87907ea0a1; Referenced input fingerprint sha256:a78ca8575ddb70589a8cc04ff4f043825bd52db40d751181584ea10118bfe2b4; Product pin 87db6e234e32cbc251a128d6b370cdda36c4d0c0

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
