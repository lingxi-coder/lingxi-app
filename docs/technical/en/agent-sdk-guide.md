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

This is a workflow map, not an API compatibility claim. Claude SDK option objects, TypeScript imports, Python clients and authentication are not interchangeable with LingXi’s Rust API.

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
- [Claude Agent SDK topic reference](https://code.claude.com/docs/en/agent-sdk/overview)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/api.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/lib.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/desktop/sdk.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/runtime/src/mobile/host.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/lib.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/f975ab5804bb3af0e72beb3974e5c911ece44d53/crates/client/src/protocol/commands.rs) — Committed source (f975ab5804bb3af0e72beb3974e5c911ece44d53)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/src/lib.rs) — Committed source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/8409379b84c6616332e51978cfbc31c5ba712d78/packages/bridge-client/src/client.ts) — Committed source (8409379b84c6616332e51978cfbc31c5ba712d78)
- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/8409379b84c6616332e51978cfbc31c5ba712d78/apps/bridge-server/src/server.rs) — Committed source (8409379b84c6616332e51978cfbc31c5ba712d78)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/8409379b84c6616332e51978cfbc31c5ba712d78/apps/bridge-server/src/router.rs) — Committed source (8409379b84c6616332e51978cfbc31c5ba712d78)

- lingxi-app: HEAD 8409379b84c6616332e51978cfbc31c5ba712d78; Referenced input fingerprint sha256:40c003672f89d603760e13d5912a3db44dce330055f42839b55ed26f18dc28e5
- harness-runtime: HEAD f975ab5804bb3af0e72beb3974e5c911ece44d53; Referenced input fingerprint sha256:92c2108dc63215bc69ea9c11ea081795da883fdcb1c9c28b060aa9221a1b95a2; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188
- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
