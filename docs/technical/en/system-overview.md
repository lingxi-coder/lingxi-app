# System architecture & ownership

Follow the product hosts, shared Agent runtime, model SDK and local execution layer, with explicit ownership and implementation boundaries.

<a id="layers"></a>

## Four independent repositories

The sibling directory is a development convenience. Each repository has its own Git history, dependency lock, build gates and release identity. Product Cargo dependencies choose immutable SDK revisions; the sibling working trees do not automatically override those dependencies.

| Layer | Owns | Consumer contract |
| --- | --- | --- |
| lingxi-app | UI, CLI/TUI, Bridge, native wrappers, callbacks, signing and product resources | SDK facade, shared client DTOs, resource contracts |
| harness-runtime | Agent execution, sessions, tools, permission, sandbox, extension and lifecycle composition | Host-injected services and platform traits |
| llm-client | Provider codecs, connections, transport, normalized streams and service APIs | Requests, prepared calls and provider capabilities |
| mobile-linux-runtime | Linux execution, PTY, resource verification and platform bindings | Runtime API and explicit resource/policy inputs |

![Product hosts call Harness. Harness calls LLM Client and local execution services. Mobile Linux remains independent of Agent and model SDKs.](../assets/system-layers.svg)

Arrows show consumption, not deployment or automatic local dependency substitution.

<a id="hosts"></a>

## Hosts and composition roots

Electron runs the local Rust Bridge and consumes the private TypeScript client. CLI/TUI uses desktop composition directly. Android and iOS use platform-local Rust FFI and native callbacks. The current apps/web application is the public website and SDK documentation with retained prototype screens; it does not establish a deployed browser Agent service.

- [Product & platform integration](platform-integration.md)
- [Harness lifecycle](harness-lifecycle.md)

<a id="sources"></a>

## Working tree, HEAD and product pin

This handbook describes referenced working-tree files. HEAD identifies the committed base, while the product pin identifies its declared dependency. These can differ. A new file or file whose bytes differ from HEAD has no committed line link; a fingerprint identifies the input bytes but does not make uncommitted code remotely retrievable.

Dependency identity and integration status must be checked separately. Documenting a newer SDK interface does not prove the product is built against it.

<a id="platforms"></a>

## Capabilities and unavailable services

Platform implementations supply filesystem/process/credential/sandbox/device services; the shared Agent owns the workflow. Unsupported capabilities must remain explicit results. Desktop platform code, mobile native execution and hosted model tools have different permission and cancellation owners.

- [Mobile Linux availability and resources](mobile-linux.md)
- [Permissions and sandbox](harness-tools.md)

<a id="related"></a>

## Related independent systems

Computer Use and Local App Market have their own architecture and implementation evidence. Their presence in the parent directory is not a dependency edge or proof that every feature is integrated into this product. Use their current documentation and status reports when assessing those projects.

- [Computer Use repository](https://github.com/lingxi-coder/computer-use)
- [Local App Market repository](https://github.com/lingxi-coder/local-app-market)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/Cargo.toml) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [harness-runtime/crates/runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/runtime/Cargo.toml) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/crates/runtime/src/api.rs) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/src/lib.rs) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)
- [mobile-linux-runtime/Cargo.toml](https://github.com/lingxi-coder/mobile-linux-runtime/blob/b65f8856d773d27ebf73b0cade907884d3080704/Cargo.toml) — Committed source (b65f8856d773d27ebf73b0cade907884d3080704)

- lingxi-app: HEAD cc3f725e248e4d717ceecec4c029a86a318dec3e; Referenced input fingerprint sha256:af041726ea2dcbfff18678637efb08857c8817c356f2a3bf2e0ddd2bd1205b1e
- harness-runtime: HEAD 6a26289330604ca621d3d1c46e5e71e3369f9955; Referenced input fingerprint sha256:50eb3dc51cb2a5ee927dcf0f48b20b7e956398fe44c92c11c0f1ee8b4b25cc83; Product pin 6a26289330604ca621d3d1c46e5e71e3369f9955
- llm-client: HEAD 7333dbc3795b6bad893cb7287088d80931538630; Referenced input fingerprint sha256:51afdece5bf1e2d135f676f447d6ff43c84aaa464328f96ba19fa1e5010928af; Product pin 7333dbc3795b6bad893cb7287088d80931538630
- mobile-linux-runtime: HEAD b65f8856d773d27ebf73b0cade907884d3080704; Referenced input fingerprint sha256:de6f69b8671150c8682b938faea726c0360978b776acaa4c077b257b13792174; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
