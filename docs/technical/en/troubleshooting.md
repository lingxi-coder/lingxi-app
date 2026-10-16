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

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/bridge-server/src/server.rs) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/packages/bridge-client/src/client.ts) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/session/src/jsonl/recovery.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/runtime/src/api.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/client/executor.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)

- lingxi-app: HEAD cc3f725e248e4d717ceecec4c029a86a318dec3e; Referenced input fingerprint sha256:af041726ea2dcbfff18678637efb08857c8817c356f2a3bf2e0ddd2bd1205b1e
- harness-runtime: HEAD 6a26289330604ca621d3d1c46e5e71e3369f9955; Referenced input fingerprint sha256:50eb3dc51cb2a5ee927dcf0f48b20b7e956398fe44c92c11c0f1ee8b4b25cc83; Product pin 6a26289330604ca621d3d1c46e5e71e3369f9955
- llm-client: HEAD 7333dbc3795b6bad893cb7287088d80931538630; Referenced input fingerprint sha256:51afdece5bf1e2d135f676f447d6ff43c84aaa464328f96ba19fa1e5010928af; Product pin 7333dbc3795b6bad893cb7287088d80931538630
- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
