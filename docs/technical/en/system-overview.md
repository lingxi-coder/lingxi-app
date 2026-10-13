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

- lingxi-app/Cargo.toml — Uncommitted source (2c1528a557a52d8652f4cee2d9bb3e46f7d569d6)
- harness-runtime/crates/runtime/Cargo.toml — Uncommitted source (e18e31ef3fa0b739147f13db3c2c9e61e815a9f5)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/e18e31ef3fa0b739147f13db3c2c9e61e815a9f5/crates/runtime/src/api.rs) — Committed source (e18e31ef3fa0b739147f13db3c2c9e61e815a9f5)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/ce32fd07402c13fbfcfa185942162a87907ea0a1/src/lib.rs) — Committed source (ce32fd07402c13fbfcfa185942162a87907ea0a1)
- [mobile-linux-runtime/Cargo.toml](https://github.com/lingxi-coder/mobile-linux-runtime/blob/eb82dcd73080bc35d0236abc15209a3bbee9f52c/Cargo.toml) — Committed source (eb82dcd73080bc35d0236abc15209a3bbee9f52c)

- lingxi-app: HEAD 2c1528a557a52d8652f4cee2d9bb3e46f7d569d6; Referenced input fingerprint sha256:3e09a114ea3046bd62d2333be94726f0169efbe46b3cd64422178d54079069f9
- harness-runtime: HEAD e18e31ef3fa0b739147f13db3c2c9e61e815a9f5; Referenced input fingerprint sha256:ee5e12368e31aa0021cee361f5c031d077c0371600228a09970bfd11e0bda8ab; Product pin 05f2e97050bcda8e36df4bc533267acb740b8bd9
- llm-client: HEAD ce32fd07402c13fbfcfa185942162a87907ea0a1; Referenced input fingerprint sha256:a78ca8575ddb70589a8cc04ff4f043825bd52db40d751181584ea10118bfe2b4; Product pin 87db6e234e32cbc251a128d6b370cdda36c4d0c0
- mobile-linux-runtime: HEAD eb82dcd73080bc35d0236abc15209a3bbee9f52c; Referenced input fingerprint sha256:78374cc6ec701b4b4e082679fb8033d488b6379c3345bd5d03cd383177c3daf4; Product pin b65f8856d773d27ebf73b0cade907884d3080704

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
