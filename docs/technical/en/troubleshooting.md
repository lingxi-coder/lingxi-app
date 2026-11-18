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
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/fc6f9ca01c2bdfc32759a2ca0e4156f464ddb3e7/packages/bridge-client/src/client.ts) — Committed source (fc6f9ca01c2bdfc32759a2ca0e4156f464ddb3e7)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/session/src/jsonl/recovery.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/runtime/src/api.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/client/executor.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/5dd627e1049ba85e0563b7f351f79b00267c58c3/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (5dd627e1049ba85e0563b7f351f79b00267c58c3)

- lingxi-app: HEAD 90a626b807a57c06a984bcd117bb03235aa6e0e5; Referenced input fingerprint sha256:0414ded61011432be634176b81ab5e5d501536ae723505b952a4b31c1b32bd78
- harness-runtime: HEAD f50814ec8381f10756f66eadf457474d90bcc69a; Referenced input fingerprint sha256:6fe1d28e55bd5b9d72e3fe6ee54f892c7efe64b640840a407ccee12dcef85567; Product pin f50814ec8381f10756f66eadf457474d90bcc69a
- llm-client: HEAD 63c8f7663b7cba7466538213f86c6df4a30442a9; Referenced input fingerprint sha256:c333c57f017ceb83b75e77ac252dc2cf3850ac8a74a5199f5d9154b7e698100c; Product pin 63c8f7663b7cba7466538213f86c6df4a30442a9
- mobile-linux-runtime: HEAD 5dd627e1049ba85e0563b7f351f79b00267c58c3; Referenced input fingerprint sha256:5dd456cd771483f341bb99d25aff43140c96f893f6de5541c381c2083f162064; Product pin 5dd627e1049ba85e0563b7f351f79b00267c58c3

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
