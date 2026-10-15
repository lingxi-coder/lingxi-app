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

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/apps/bridge-server/src/server.rs) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/ed180d70d6dc36d7bed12c654620d0bccb231883/packages/bridge-client/src/client.ts) — Committed source (ed180d70d6dc36d7bed12c654620d0bccb231883)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/session/src/jsonl/recovery.rs) — Committed source (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d/crates/runtime/src/api.rs) — Committed source (1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/179d336607b2367cf0709a78c760c9333ec25bb3/src/client/executor.rs) — Committed source (179d336607b2367cf0709a78c760c9333ec25bb3)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/a632e878dc597dc41f6a3712c5a5cdebb794c99c/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (a632e878dc597dc41f6a3712c5a5cdebb794c99c)

- lingxi-app: HEAD ed180d70d6dc36d7bed12c654620d0bccb231883; Referenced input fingerprint sha256:ef51a25cc818084d97b831957cb840fd84e1a4666089524025c7544ee886acda
- harness-runtime: HEAD 1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d; Referenced input fingerprint sha256:d77506e15375f8653d0ae9198567f3dbeba4e931dce8226b58971350a635ae8a; Product pin 1c08d9265d9a4fbf20d59c0776ddbfb96cb03e4d
- llm-client: HEAD 179d336607b2367cf0709a78c760c9333ec25bb3; Referenced input fingerprint sha256:64423ae6bcb4985bab0883b693378509bb7fe060e59bf4facafedcb4d2f32293; Product pin 450d52b12b6d88a377ab3332c3b8a6a1820e2b7c
- mobile-linux-runtime: HEAD a632e878dc597dc41f6a3712c5a5cdebb794c99c; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
