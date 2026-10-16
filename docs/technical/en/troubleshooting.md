# Diagnostics and operational recovery

Trace faults to the owning layer, recover sessions and runtime resources, and collect useful evidence without leaking credentials.

<a id="triage"></a>

## Locate the failing owner

| Symptom | Inspect first | Recovery boundary |
| --- | --- | --- |
| Client cannot connect | Bridge process, discovery/handshake and SDK versions | Do not infer model failure before transport is connected |
| Model request rejected | Actual connection/profile, capability, authentication and typed error | Host refresh/retry policy; preserve attempt metadata |
| Tool remains waiting | Permission/interactive request owner, cancellation and task ID | Resume/cancel the owned request; do not duplicate execution |
| Native guest execution unavailable | Platform/backend, resource integrity and explicit runtime policy | Repair declared resources; simulator does not become device backend |
| Docs freshness check fails | Referenced source changes and product pins | Review content, then regenerate; do not hand-edit generated output |

<a id="sessions"></a>

## Session and shutdown recovery

Stop admitting new work before shutdown; cancel or await owned turns, and preserve the runtime owner while durable writes finish. ShutdownReport.complete determines whether cleanup has finished. Resume from persisted identity and structured history rather than matching rendered message text.

- [Lifecycle and shutdown](harness-lifecycle.md)
- [Sessions and recovery](harness-sessions.md)

<a id="resources"></a>

## Execution resources and bindings

Check rootfs/native assets against their declared manifest/checksum and target ABI before repair. Rebuild native bindings when DTO/signature versions differ. Preserve unrelated user sessions and credentials; a source rebuild is not a reason to erase them.

- [Resource policy and authorization](mobile-authorization.md)

<a id="evidence"></a>

## Useful diagnostic evidence

- Record repository/SDK pins, platform, feature set, session/request identity and the terminal outcome.
- Keep typed error category and sanitized attempt metadata. Never paste provider keys, credential files, signed account tokens or local secrets into reports.
- Describe what was actually run and where the evidence stops. A UI timeout, sandbox restriction or simulator limitation needs a precise reproduction boundary.

<a id="verify"></a>

## After recovery

1. Confirm the intended source and runtime identity are active.
2. Run the smallest relevant transport, lifecycle, model or resource check.
3. Verify persistence and clean terminal completion for the affected session.
4. Record remaining live-provider or device acceptance separately.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/apps/bridge-server/src/server.rs) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/packages/bridge-client/src/client.ts) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/98fc362da3fabfe36aa5610b9e93818923c1ab31/crates/session/src/jsonl/recovery.rs) — Committed source (98fc362da3fabfe36aa5610b9e93818923c1ab31)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/98fc362da3fabfe36aa5610b9e93818923c1ab31/crates/runtime/src/api.rs) — Committed source (98fc362da3fabfe36aa5610b9e93818923c1ab31)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/src/client/executor.rs) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)

- lingxi-app: HEAD ec6ff963545c8d02e0c9c220a22f07874ba62959; Referenced input fingerprint sha256:b657a9b5e03a337d3dce34527025cc7cd87afb3439754fe074426c39589fd716
- harness-runtime: HEAD 98fc362da3fabfe36aa5610b9e93818923c1ab31; Referenced input fingerprint sha256:8ac34e7055ee1007cfccf0eafbeaa5e2437c2f20dc623f1069d255177d27dd90; Product pin 98fc362da3fabfe36aa5610b9e93818923c1ab31
- llm-client: HEAD 46240a65df198a654d7d6de1c0bd99c23fcbb979; Referenced input fingerprint sha256:548db437ccaaeec3ce6ad3beb931ef868b263f6ad9b3c47309a3e00c514326dd; Product pin 46240a65df198a654d7d6de1c0bd99c23fcbb979
- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
