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

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/fc6f9ca01c2bdfc32759a2ca0e4156f464ddb3e7/apps/bridge-server/src/server.rs) — Committed source (fc6f9ca01c2bdfc32759a2ca0e4156f464ddb3e7)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/7fecdf10d044c94cbebbcbb4511ebede0c3c97ed/packages/bridge-client/src/client.ts) — Committed source (7fecdf10d044c94cbebbcbb4511ebede0c3c97ed)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/16cded0d03f9cd83b2e69436ee2367200268e088/crates/session/src/jsonl/recovery.rs) — Committed source (16cded0d03f9cd83b2e69436ee2367200268e088)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/16cded0d03f9cd83b2e69436ee2367200268e088/crates/runtime/src/api.rs) — Committed source (16cded0d03f9cd83b2e69436ee2367200268e088)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/client/executor.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/5dd627e1049ba85e0563b7f351f79b00267c58c3/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (5dd627e1049ba85e0563b7f351f79b00267c58c3)

- lingxi-app: HEAD e74379a16bb1946787a7edf327cd8367128c4357; Referenced input fingerprint sha256:b2d7aabb4ca87d35400a12fe665e0a43040123560edaadd23961a53bf191fc66
- harness-runtime: HEAD 16cded0d03f9cd83b2e69436ee2367200268e088; Referenced input fingerprint sha256:af27a44ee002fcbf278f333efb7f9daebd6c6fd92a6e21f86383c0deb5fea242; Product pin 16cded0d03f9cd83b2e69436ee2367200268e088
- llm-client: HEAD 63c8f7663b7cba7466538213f86c6df4a30442a9; Referenced input fingerprint sha256:2d8d308d351d3f94a029886f11db1bf931d3b28dd76d93e0f64e50665160e636; Product pin 63c8f7663b7cba7466538213f86c6df4a30442a9
- mobile-linux-runtime: HEAD 5dd627e1049ba85e0563b7f351f79b00267c58c3; Referenced input fingerprint sha256:5dd456cd771483f341bb99d25aff43140c96f893f6de5541c381c2083f162064; Product pin 224cc6b272e1e8ee4385a03efd34bd79b18fd8ce

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
