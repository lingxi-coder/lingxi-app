# LingXi technical handbook

Bilingual system and SDK documentation for the current working tree. Generated from website documentation data; edit apps/web/src/data/docs.

[Chinese contents](../zh/README.md)

## Get started

- [SDKs & libraries](overview.md) — Build with LingXi. Connect models, orchestrate agents, and bring a Linux runtime to mobile — with a toolkit that fits your application.
- [API reference](api-reference.md) — Find the API for your task, follow a complete integration guide, and search the underlying public declarations.

## System internals

- [Agent execution and recovery](agent-execution.md) — Follow one admitted turn from host input to model requests, streamed tool execution and a durable terminal state.
- [Configuration, credentials and isolation](configuration-security.md) — Trace settings provenance and keep model routing, authorization, credential storage and OS isolation as explicit host responsibilities.
- [Session identity and durable storage](session-storage.md) — Understand JSONL graphs, stable UUIDs, cold restoration, forks and the difference between context compaction and transcript reclamation.
- [Tools, skills, hooks, MCP and LSP](extensions-lifecycle.md) — Discover extensions through trusted registries and retain authority across activation, refresh, invocation and teardown.
- [Subagents and background tasks](agents-background.md) — Separate agent-pool admission, task ownership, mailbox delivery and terminal publication across foreground and detached work.
- [Cron, Workflow and Fusion](automation-workflows-fusion.md) — Understand the live execution paths for schedules, scripted agent orchestration and evidence-based multi-model work.
- [Attachments, audio and realtime hosts](multimodal-host.md) — Route multimodal input and audio through explicit provider capability and native resource owners.
- [Inline visualization integration](runtime-visualization.md) — Publish immutable visualization revisions and mount them through one shared authorized service on Electron, Android and iOS.
- [System architecture & ownership](system-overview.md) — Follow the product hosts, shared Agent runtime, model SDK and local execution layer, with explicit ownership and implementation boundaries.

## Harness Runtime

- [Harness Runtime](harness.md) — Embed the Rust agent runtime in a desktop, mobile, or custom host.
- [Sessions & turns](harness-sessions.md) — Run prompts, observe output, cancel work, and read session state.
- [Host lifecycle & persistence](harness-lifecycle.md) — Compose host services and shut down through the runtime’s durability barriers.
- [Tools & permissions](harness-tools.md) — Implement tools using the shared contract and register their capabilities.
- [Plugins](harness-plugins.md) — Load plugin components, inspect their lifecycle, and handle unload failures.
- [Client protocol & adapters](harness-client.md) — Share commands, events, presentation, and runtime adapters across clients.
- [Choose an Agent integration](agent-sdk-guide.md) — Map familiar Agent SDK concepts to LingXi’s actual Rust runtime, model client and product protocol, then choose the smallest useful integration.
- [Run your first Agent turn](agent-first-turn.md) — Go from an assembled Harness to a prompt, a terminal result, follow-up context and a clean shutdown.
- [Connect streamed output to a UI](agent-ui-streaming.md) — Wire one event destination and render text, tool activity and interactive requests without confusing message completion with turn completion.
- [Continue, resume, fork and cancel](agent-session-workflows.md) — Choose the right session operation and distinguish cancellation, saved history and active execution ownership.
- [Add a tool and handle approval](agent-tool-workflows.md) — Take a tool from schema and host registration through a real permission round-trip, result presentation and teardown.
- [Compose skills, hooks and MCP](agent-extension-recipes.md) — Choose the right extension, configure a scoped lifecycle check and verify an external capability is connected and callable.
- [Observe parallel agents and tasks](agent-parallel-tasks.md) — Give independent work clear ownership and build progress/cancellation controls around authoritative agent and task state.

## LLM Client

- [LLM Client](llm-client.md) — A standalone Rust client for provider protocols, typed requests, streaming, images, embeddings, and model catalogs.
- [Requests & execution](llm-requests.md) — Compose typed model input and control each physical attempt with draft, prepare, dispatch, and collect.
- [Streaming & cancellation](llm-streaming.md) — Read normalized events or per-chunk observations while preserving tools, native content, usage, and terminal errors.
- [Image API](llm-images.md) — Generate, edit, and query native image tasks through a service independent of Chat.
- [Providers, catalogs & pricing](llm-providers.md) — Bind profiles, filter by region, manage catalogs, access native resources, and estimate cost from observed facts.
- [Attachments & provider files](llm-files.md) — Keep durable host attachments separate from scoped provider uploads, model input and generated artifacts.
- [Cloud transcription & speech](llm-audio.md) — Use exact-profile audio independently of chat, with explicit formats, bounded streams and dispatch-aware errors.
- [Realtime sessions & backpressure](llm-realtime.md) — Run bounded bidirectional provider sessions on the host executor and preserve tool, audio and interruption lifecycles.
- [Provider resources & hosted tools](llm-native-services.md) — Bind native resources explicitly and separate provider-side execution from Harness tools and background work.
- [Output, inference & cache controls](llm-controls.md) — Apply validated request-local contracts without weakening schemas or confusing reasoning budgets with monetary limits.
- [Routing, failures & attempt accounting](llm-execution-accounting.md) — Choose convenience failover or host-controlled one-shot calls and preserve actual usage before output interpretation.

## Mobile Linux

- [Mobile Linux Runtime](mobile-linux.md) — Embed a Linux userspace in Android and iOS apps, with commands, streaming, PTY, raw stdio and verified rootfs lifecycle.
- [Rust API](mobile-rust.md) — Shared contracts for commands, task lifecycle, PTY, raw stdio, mounts and rootfs verification.
- [Android · Kotlin](mobile-android.md) — Install the Maven bundle, stage a verified rootfs, and operate a persistent Kotlin runtime.
- [iOS · Swift](mobile-ios.md) — Consume SwiftPM and XCFramework products, provide iSH fakefs resources, and control lifecycle through Swift.
- [Permissions and integrity](mobile-authorization.md) — Host-owned mount policy, network and resource enforcement, rootfs trust and explicit platform limits.

## Product integrations

- [TypeScript Bridge Client](bridge-client.md) — Connect a Node.js host to the local LingXi Bridge service, submit commands, consume events, and resolve permission requests.
- [Native host integration](platform-integration.md) — Understand the product’s iOS and Android UniFFI wrappers, generated bindings, native callbacks, and shared mobile session host.
- [Desktop · Electron host](desktop-host.md) — Session-owned Bridge sidecars, renderer IPC, event replay, native services and signed packaging.
- [CLI, TUI and headless hosts](cli-tui.md) — Use the product command line, distinguish terminal and print modes, and integrate deterministic output without bypassing runtime admission.
- [Web · SDK site and UI prototype](web-product.md) — The current browser application publishes SDK documentation and demonstrates account/console UI without a connected runtime backend.
- [Native builds, bindings and release acceptance](platform-builds.md) — Build matched Rust/native artifacts, select distribution variants and separate compile, simulator and physical-device acceptance.

## Development & operations

- [Development environment & dependency identity](development-workflow.md) — Set up the four repositories, build incrementally, and make SDK changes without silently mixing local and pinned Rust types.
- [Build, signing & release procedures](build-release.md) — Build each product entrypoint and distinguish dependency checks, signed packaging, static artifacts and published releases.
- [Validation and evidence boundaries](validation-evidence.md) — Understand what documentation checks, consumer compilation, mock tests, provider calls and device acceptance actually prove.
- [Diagnostics and operational recovery](troubleshooting.md) — Trace faults to the owning layer, recover sessions and runtime resources, and collect useful evidence without leaking credentials.
