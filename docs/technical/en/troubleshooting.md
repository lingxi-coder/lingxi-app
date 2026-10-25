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

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/77b17c505508c8de002408ca6dddf4c8cf5d2187/apps/bridge-server/src/server.rs) — Committed source (77b17c505508c8de002408ca6dddf4c8cf5d2187)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/77b17c505508c8de002408ca6dddf4c8cf5d2187/packages/bridge-client/src/client.ts) — Committed source (77b17c505508c8de002408ca6dddf4c8cf5d2187)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/session/src/jsonl/recovery.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/crates/runtime/src/api.rs) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/client/executor.rs) — Committed source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- lingxi-app: HEAD 77b17c505508c8de002408ca6dddf4c8cf5d2187; Referenced input fingerprint sha256:542c75ebbb66242ca76885685c120d7913ec9ad0b9b72f6af8865d05c76a6cd0
- harness-runtime: HEAD 0e28910fd624638e6c5d1f92eca4899a7938d188; Referenced input fingerprint sha256:b6b36966a8f4c48b8d0813ea03f73cd617387ddaf2531d8254e4a5ac81f48255; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188
- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8
- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; Referenced input fingerprint sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; Product pin d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
