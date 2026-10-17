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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/d1840b78301dae46c8cf076eddc0b3eef1610279/Cargo.toml) — Committed source (d1840b78301dae46c8cf076eddc0b3eef1610279)
- [harness-runtime/crates/runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/dc6ce1217143b693d97ab6278ff66568beea20d3/crates/runtime/Cargo.toml) — Committed source (dc6ce1217143b693d97ab6278ff66568beea20d3)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/dc6ce1217143b693d97ab6278ff66568beea20d3/crates/runtime/src/api.rs) — Committed source (dc6ce1217143b693d97ab6278ff66568beea20d3)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/src/lib.rs) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)
- [mobile-linux-runtime/Cargo.toml](https://github.com/lingxi-coder/mobile-linux-runtime/blob/6b033d41a2b6b736fda1e8f996f55a4b20bd885d/Cargo.toml) — Committed source (6b033d41a2b6b736fda1e8f996f55a4b20bd885d)

- lingxi-app: HEAD d1840b78301dae46c8cf076eddc0b3eef1610279; Referenced input fingerprint sha256:302434a272a31c7fc1d4f0c764107332cbbef9d6977bde52add1d448b51dc5ae
- harness-runtime: HEAD dc6ce1217143b693d97ab6278ff66568beea20d3; Referenced input fingerprint sha256:a83d44fb79099d3a923f8fd33bfd18789edd02c2b1d5c683a16c589fffaf2578; Product pin dc6ce1217143b693d97ab6278ff66568beea20d3
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e
- mobile-linux-runtime: HEAD 6b033d41a2b6b736fda1e8f996f55a4b20bd885d; Referenced input fingerprint sha256:435a888b1a310f357f18311df5f96418fa72b1a900d78e987af282bb3e91d7ca; Product pin 6b033d41a2b6b736fda1e8f996f55a4b20bd885d

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
