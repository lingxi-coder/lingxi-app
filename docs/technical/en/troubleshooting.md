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

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/94c394356844efcf56b7f53221a57f190de17004/apps/bridge-server/src/server.rs) — Committed source (94c394356844efcf56b7f53221a57f190de17004)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/94c394356844efcf56b7f53221a57f190de17004/packages/bridge-client/src/client.ts) — Committed source (94c394356844efcf56b7f53221a57f190de17004)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/e03def6e13ff326fe0d507372ea8c50f58b44100/crates/session/src/jsonl/recovery.rs) — Committed source (e03def6e13ff326fe0d507372ea8c50f58b44100)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/e03def6e13ff326fe0d507372ea8c50f58b44100/crates/runtime/src/api.rs) — Committed source (e03def6e13ff326fe0d507372ea8c50f58b44100)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/client/executor.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)

- lingxi-app: HEAD 94c394356844efcf56b7f53221a57f190de17004; Referenced input fingerprint sha256:bbc5366e564a2ce7f2645abf263a30963cab2fd4e24639928ed3837a3e6ff313
- harness-runtime: HEAD e03def6e13ff326fe0d507372ea8c50f58b44100; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin e03def6e13ff326fe0d507372ea8c50f58b44100
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e
- mobile-linux-runtime: HEAD 6b033d41a2b6b736fda1e8f996f55a4b20bd885d; Referenced input fingerprint sha256:435a888b1a310f357f18311df5f96418fa72b1a900d78e987af282bb3e91d7ca; Product pin 6b033d41a2b6b736fda1e8f996f55a4b20bd885d

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
