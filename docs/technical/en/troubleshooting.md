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

- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/apps/bridge-server/src/server.rs) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64/packages/bridge-client/src/client.ts) — Committed source (fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64)
- [harness-runtime/crates/session/src/jsonl/recovery.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/session/src/jsonl/recovery.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/bb7a016c305609a0b4ca7b0c1a91faead18c4db0/crates/runtime/src/api.rs) — Committed source (bb7a016c305609a0b4ca7b0c1a91faead18c4db0)
- [llm-client/src/client/executor.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/src/client/executor.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)
- [mobile-linux-runtime/crates/mobile-linux-api/src/mobile_linux.rs](https://github.com/lingxi-coder/mobile-linux-runtime/blob/25cf8a273bcd572b9be48e18fc9807e01dcdfce1/crates/mobile-linux-api/src/mobile_linux.rs) — Committed source (25cf8a273bcd572b9be48e18fc9807e01dcdfce1)

- lingxi-app: HEAD fbc7cc304c20cf4122e53636fd9a3eec7fd9bc64; Referenced input fingerprint sha256:a5bfcf25a49c613b97f21fc4507c7f97a198e934f9d09398cb8794e7d86f49e3
- harness-runtime: HEAD bb7a016c305609a0b4ca7b0c1a91faead18c4db0; Referenced input fingerprint sha256:700726d3f012384673f588a15ab3a32ecaf4f3fc499f0e4e86c0b8e6bc321559; Product pin ae5a65b1f4816cd26a06d462573ebc9841f14f96
- llm-client: HEAD 66ce76a2ecc679f0dba551e26ef6aaceb40d2f49; Referenced input fingerprint sha256:89fef6f87fb7803bec69d768fa11e408a54f2123d196e2c859d8faba79092d96; Product pin 3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3
- mobile-linux-runtime: HEAD 25cf8a273bcd572b9be48e18fc9807e01dcdfce1; Referenced input fingerprint sha256:3f654be245e2810979441677e37d079538d03bf98bfb68f98d054e4cc79d84df; Product pin 43382a20161e371a0a31526412164b3d35545ab5

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
