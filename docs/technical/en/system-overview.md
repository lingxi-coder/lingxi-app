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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/abad4d9603ce6a0c032ddf089f69f88e927b76c4/Cargo.toml) — Committed source (abad4d9603ce6a0c032ddf089f69f88e927b76c4)
- [harness-runtime/crates/runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/Cargo.toml) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/a39da75f0985d61d90fffa255f7b4d90901b0e27/crates/runtime/src/api.rs) — Committed source (a39da75f0985d61d90fffa255f7b4d90901b0e27)
- llm-client/src/lib.rs — Unpublished source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [mobile-linux-runtime/Cargo.toml](https://github.com/lingxi-coder/mobile-linux-runtime/blob/d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9/Cargo.toml) — Committed source (d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9)

- lingxi-app: HEAD abad4d9603ce6a0c032ddf089f69f88e927b76c4; Referenced input fingerprint sha256:bbb3c48b6b5761f33d3d66eb68ccaf089f9fd5ffc104138cbe084fb24e3b4f5b
- harness-runtime: HEAD a39da75f0985d61d90fffa255f7b4d90901b0e27; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin a39da75f0985d61d90fffa255f7b4d90901b0e27
- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8
- mobile-linux-runtime: HEAD d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9; Referenced input fingerprint sha256:c85076a3035f8a036929b8e4a66cfae06a1e6eac952ef5315ba1cba3470d2139; Product pin d96c0b9b40a7d0efbf1e14e333e06b6d641ac6d9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
