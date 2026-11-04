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

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/harness-runtime)

- [harness-runtime/crates/runtime/src/api.rs](https://github.com/lingxi-coder/harness-runtime/blob/f07f87e9a179379e6c429d6143007f979500bd81/crates/runtime/src/api.rs) — Committed source (f07f87e9a179379e6c429d6143007f979500bd81)
- [harness-runtime/crates/runtime/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f07f87e9a179379e6c429d6143007f979500bd81/crates/runtime/src/lib.rs) — Committed source (f07f87e9a179379e6c429d6143007f979500bd81)
- [harness-runtime/crates/runtime/src/desktop/sdk.rs](https://github.com/lingxi-coder/harness-runtime/blob/f07f87e9a179379e6c429d6143007f979500bd81/crates/runtime/src/desktop/sdk.rs) — Committed source (f07f87e9a179379e6c429d6143007f979500bd81)
- [harness-runtime/crates/runtime/src/mobile/host.rs](https://github.com/lingxi-coder/harness-runtime/blob/f07f87e9a179379e6c429d6143007f979500bd81/crates/runtime/src/mobile/host.rs) — Committed source (f07f87e9a179379e6c429d6143007f979500bd81)
- [harness-runtime/crates/client/src/lib.rs](https://github.com/lingxi-coder/harness-runtime/blob/f07f87e9a179379e6c429d6143007f979500bd81/crates/client/src/lib.rs) — Committed source (f07f87e9a179379e6c429d6143007f979500bd81)
- [harness-runtime/crates/client/src/protocol/commands.rs](https://github.com/lingxi-coder/harness-runtime/blob/f07f87e9a179379e6c429d6143007f979500bd81/crates/client/src/protocol/commands.rs) — Committed source (f07f87e9a179379e6c429d6143007f979500bd81)
- [llm-client/src/lib.rs](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/src/lib.rs) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)
- [lingxi-app/packages/bridge-client/src/client.ts](https://github.com/lingxi-coder/lingxi-app/blob/4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87/packages/bridge-client/src/client.ts) — Committed source (4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87)
- [lingxi-app/apps/bridge-server/src/server.rs](https://github.com/lingxi-coder/lingxi-app/blob/4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87/apps/bridge-server/src/server.rs) — Committed source (4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87)
- [lingxi-app/apps/bridge-server/src/router.rs](https://github.com/lingxi-coder/lingxi-app/blob/4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87/apps/bridge-server/src/router.rs) — Committed source (4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87)

- lingxi-app: HEAD 4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87; Referenced input fingerprint sha256:40b76956c72557655c95a9de37a4fed9d6afad2dc25a52556454f55b9fc0a60a
- harness-runtime: HEAD f07f87e9a179379e6c429d6143007f979500bd81; Referenced input fingerprint sha256:c3dfae4cc5eb31ce91b7b2090c819842f550bbe7ebb0d564931735752ad375b2; Product pin e54f6d018e334dec3153e11d67f8a8224ff538d2
- llm-client: HEAD 66ce76a2ecc679f0dba551e26ef6aaceb40d2f49; Referenced input fingerprint sha256:89fef6f87fb7803bec69d768fa11e408a54f2123d196e2c859d8faba79092d96; Product pin 3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
