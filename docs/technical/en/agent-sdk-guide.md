# Choose an Agent integration

Map familiar Agent SDK concepts to LingXi’s actual Rust runtime, model client and product protocol, then choose the smallest useful integration.

<a id="pick-the-layer"></a>

## Start with the operation you need

A direct model request does not execute tool calls on your filesystem. Harness adds the agent loop through its host composition. Its public facade gives access to an existing session; it is not a process launcher or a hosted service.

| Your application needs | Use | What you provide |
| --- | --- | --- |
| Call a model or consume model deltas | LLM Client | Provider configuration, request, credential and response handling. |
| Run a tool-using agent in a Rust host | Harness Runtime | An assembled runtime, output destination, permission gate and lifecycle owner. |
| Build a UI for an existing LingXi host | ClientCommand / ClientEvent and product Bridge | Authenticated transport, command dispatch, event rendering and interactive replies. |
| Run commands or a PTY on Android/iOS | Mobile Linux Runtime | Verified resources, platform paths and process ownership. |

<a id="concept-map"></a>

## Translate concepts before translating code

| Familiar concept | LingXi integration point | Migration decision |
| --- | --- | --- |
| Run an agent query | SessionHandle::run(RunInput, CancellationToken) | Supply an assembled session; keep the terminal Result separate from streamed output. |
| Iterate assistant/tool messages | OutputStream or AdapterOutputStream → ClientEventSink | Install output during composition, before the first run. |
| Resume/fork a conversation | ResumeSession; ForkSession on MobileHost | Resume confirms a mounted session. MobileHost fork confirms branch creation; mount the returned branch before sending work to it. Desktop Bridge does not currently route ForkSession. |
| Approve a tool call | PermissionGate; AdapterPermissionGate for protocol hosts | Route the real user decision to its request owner using request_id. |
| Tools, skills, hooks and MCP | Host-owned registries and their lifecycle | Verify activation and dependencies in the composition you ship. |

This is a workflow map for LingXi’s Rust API. Option objects, imports, and authentication from another language binding do not replace these Rust types.

<a id="host-contract"></a>

## Write down the host contract first

1. Choose desktop, mobile or injected SessionService/LifecycleService implementations; confirm required features and native resources.
2. Define where text, tools, questions, permissions and terminal outcomes go. Related adapters should share the same connection/session owner.
3. Set working directory, tool scope, provider/model and credential ownership before admitting work.
4. Cancel a turn and await cleanup separately from draining the host when the application exits.

- [Harness consumer manifest](harness.md#installation)
- [First turn recipe](agent-first-turn.md)
- [Model-only quickstart](llm-client.md#first-request)

<a id="migrate-one-slice"></a>

## Migrate one complete interaction

- Start with a text-only turn including normal completion and failure. Then add streaming presentation and one approval-gated tool.
- Port option intent, not names. Model choice, tool allowlists and session continuation must reach their owning host services rather than invented RunInput fields.
- Verify images, tool results, cancelled approvals, follow-up context and shutdown on the target host before expanding the migration.

<a id="use-the-reference"></a>

## Read recipes and signatures together

Recipes describe a complete interaction; Harness chapters describe API signatures; the declaration index locates implementation. Compiler-generated Rustdoc resolves methods, feature gates and trait bounds for the built profile. A raw declaration search does not resolve those conditions.

- [Harness API chapters](harness.md)
- [API declarations and Rustdoc](api-reference.md)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/runtime/src/api.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [harness-runtime/crates/runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/runtime/src/lib.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/runtime/src/desktop/sdk.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/runtime/src/mobile/host.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [harness-runtime/crates/client/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/client/src/lib.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/f50814ec8381f10756f66eadf457474d90bcc69a/crates/client/src/protocol/commands.rs) — Committed source (f50814ec8381f10756f66eadf457474d90bcc69a)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/63c8f7663b7cba7466538213f86c6df4a30442a9/src/lib.rs) — Committed source (63c8f7663b7cba7466538213f86c6df4a30442a9)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/packages/bridge-client/src/client.ts) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/apps/bridge-server/src/server.rs) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/apps/bridge-server/src/router.rs) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)

- lingxi-app: HEAD 90a626b807a57c06a984bcd117bb03235aa6e0e5; Referenced input fingerprint sha256:0414ded61011432be634176b81ab5e5d501536ae723505b952a4b31c1b32bd78
- harness-runtime: HEAD f50814ec8381f10756f66eadf457474d90bcc69a; Referenced input fingerprint sha256:6fe1d28e55bd5b9d72e3fe6ee54f892c7efe64b640840a407ccee12dcef85567; Product pin f50814ec8381f10756f66eadf457474d90bcc69a
- llm-client: HEAD 63c8f7663b7cba7466538213f86c6df4a30442a9; Referenced input fingerprint sha256:c333c57f017ceb83b75e77ac252dc2cf3850ac8a74a5199f5d9154b7e698100c; Product pin 63c8f7663b7cba7466538213f86c6df4a30442a9

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
