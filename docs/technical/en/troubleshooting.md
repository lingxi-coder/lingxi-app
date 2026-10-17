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

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/apps/bridge-server/src/server.rs) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b/packages/bridge-client/src/client.ts) — Committed source (c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/session/src/jsonl/recovery.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/d2e3aed63ef232526967a8bf176a06d6a25ae8fa/crates/runtime/src/api.rs) — Committed source (d2e3aed63ef232526967a8bf176a06d6a25ae8fa)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/client/executor.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)

- lingxi-app: HEAD c9942f3a4a58eb6e3db6f831f3b77ece7d7ff69b; Referenced input fingerprint sha256:9ac79cec993505292f531b296b57eb329900eb70aaaa7e34660a0857d25913b6
- harness-runtime: HEAD d2e3aed63ef232526967a8bf176a06d6a25ae8fa; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin d2e3aed63ef232526967a8bf176a06d6a25ae8fa
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e
- mobile-linux-runtime: HEAD 6b033d41a2b6b736fda1e8f996f55a4b20bd885d; Referenced input fingerprint sha256:435a888b1a310f357f18311df5f96418fa72b1a900d78e987af282bb3e91d7ca; Product pin 6b033d41a2b6b736fda1e8f996f55a4b20bd885d

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
