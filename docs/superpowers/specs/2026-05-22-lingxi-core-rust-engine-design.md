# LingXi Core — Rust Engine Design Spec

**Date**: 2026-05-22
**Status**: Draft (v2 — full subsystem scope)
**Author**: luolingfeng + Claude Opus 4.6 / Opus 4.7

---

## 1. Overview

### 1.1 What

LingXi Core is a **platform-agnostic Rust library** that implements the complete conversation engine for an AI coding assistant (claude-code equivalent). It includes the full engine stack:

- **Conversation State Machine** (event-sourced, pure-function reducer)
- **API Client** (Anthropic / OpenAI-compatible, streaming SSE)
- **Memory System** (4-tier: project / user / session / team, LLM-driven selection, prefetch pipeline)
- **MCP Lifecycle** (7 transports, OAuth, agent-scoped servers, dynamic tool surface)
- **Tools System** (30+ method Tool trait, concurrency partition, streaming exec, result storage)
- **Hooks System** (28 event types, 4 executor kinds, output protocol, async registry, SSRF guard)
- **Agent/Subagent** (effect-delegated state machine pool, multi-dispatch, mailbox, worktree, memory snapshot, color manager)
- **Task Manager** (7 task types, polymorphic state, disk-persisted output, notification injection, cron)
- **Coordinator/Team** (coordinator mode, internal tools, teammate mailbox, swarm backend, team memory sync)
- **Compaction Engine** (5 layers + reactive + PTL retry, circuit breaker, cached microcompact, session memory dual extraction)
- **Permission Policy Engine** (5 external + 2 internal modes, 3 classifiers, 8 rule sources, denial tracking, shadow detection, pending classifier checks, bypass killswitch)
- **Plugin System** (manifest/lifecycle/marketplace/blocklist, Claude Code component surface, output styles/LSP/channels/user config, strict-plugin-only policy)
- **Secret & Credential Management** (SecureStorage trait, `Secret<T>` newtype, 7 backend variants, 30+ gitleaks rules, 5 redaction boundaries)
- **API Cost & Budget Tracking** (provider/model pricing catalog, token-class rates, prompt cache savings, per-model usage, budget halt/ask/warn policies)
- **Skills System** (bundled/user/project/plugin/MCP-derived skills, trigger-based discovery prefetch, SkillTool dispatch)
- **Slash Commands** (80+ builtin handlers, markdown command loader, argument substitution, plugin/MCP-sourced commands)
- **Side Query & Forked Agent infrastructure** (CacheSafeParams byte-exact cache sharing, used by Memory selector / Compaction / classifier explainer)
- **Output Styles** (registry, prompt addendum injection, plugin-provided styles)
- **Session Storage & Recovery** (append-only JSONL transcripts, crash-safe reader, SessionResumer integrating all subsystems)
- **File State Cache** (LRU + size-limited, Read↔Edit coordination, partial-view detection, merge/clone for fork)
- **Sandbox** (Linux namespaces / sandbox-exec / Job Object, NetworkPolicy, ResourceLimits, should_use_sandbox decision)
- **LSP Integration** (per-server state machine, language→server routing, LspTool dispatch)
- **Telemetry & Analytics** (AnalyticsSink + AnalyticsBus + GrowthBook feature flags + PII markers + killswitch)
- **Message Queue Manager** (unified priority queue: user input / task notification / orphan permission / SendMessage / hook / cron)
- **Cron Scheduler** (tick loop + cross-process lock + jitter + integration with §11 TaskRegistry)
- **IDE Bridge** (BridgeTransport, 9 message variants, JWT-paired trusted devices)
- **Anthropic OAuth** (login.claude.ai flow, multi-source auth resolver, subscription type detection, ClaudeAiLimitsTracker)
- **Configuration Loading & Merge Precedence**

It does **NOT** include:

- TUI / terminal rendering
- Platform-specific tool implementations (Bash, file I/O, sandbox)
- Production MCP transport implementations
- Any direct OS dependencies

### 1.2 Why

The existing `claw-code` Rust port (92K LOC, 9 crates) proved the concept but was built as a monolithic CLI. To support **5 target platforms** (Linux, macOS, Windows, Android, iOS), the engine must be a pure-logic library with zero OS coupling, consumed by platform-specific crates that inject I/O implementations.

The original `claude-code` TypeScript codebase (~519K LOC) revealed that an "agent engine" is far more than a conversation loop. The subsystems and cross-cutting engines above represent the **minimum** for behavioral fidelity with claude-code.

### 1.3 Relationship to claw-code

This is a **clean-room new codebase**. `claw-code` serves as a reference implementation and proof-of-concept, but code is not directly reused. The architectural lessons learned (especially from the 9-lane parity push) inform this design.

### 1.4 Scope Decision

**Full fidelity core, explicit platform split.** Every subsystem is designed at the same depth as claude-code. M1 is not a "minimal working version" of the engine; it is behaviorally complete under mocks and a demo-only `posix-minimal` host. Production platform implementations and UI layers remain post-M1.

### 1.5 Reference

- Original TypeScript claude-code: ~519K TS LOC
- claw-code Rust port: ~92K Rust LOC, 476+ tests, 40 tool specs, 12 mock parity scenarios
- Estimated M1 Rust LOC: ~60K engine/bridge/demo host + ~28K tests (~88K total)

---

## 2. Decisions Log

| # | Decision | Choice | Alternatives Considered |
|---|---|---|---|
| D1 | Milestone scope | Core engine with complete functionality, no TUI | M1-Interactive, M1-Ecosystem, M1-Parity |
| D2 | Platform/core split | **Option B**: Pure logic vs I/O — core has zero OS dependencies, all I/O through trait injection | Option A: OS-syscall boundary, Option C: Only Bash+Sandbox extracted |
| D3 | Artifact form | Rust library crate (workspace members), platforms consume via Cargo dependency | C FFI shared library, Both |
| D4 | Codebase | Completely new (claw-code as reference only) | Refactor existing claw-code, Gradual radiation |
| D5 | Verification | Contract tests + property tests | Mock parity harness expansion, Both |
| D6 | Target platforms | Linux + macOS + Windows + Android + iOS (5 platforms) | POSIX only, POSIX + Windows, POSIX + WASM |
| D7 | Mobile FFI | UniFFI (auto-generated Kotlin + Swift bindings) | Hand-written C FFI + cbindgen, Defer to M2 |
| D8 | Architecture pattern | **A+C Hybrid**: Trait-First boundaries + Event-Sourced State Machine internally | Trait-First bottom-up, Vertical Slice, Pure Event-Sourced SM |
| D9 | Subsystem scope | **Full fidelity subsystem scope in M1**: Memory, MCP, Tools, Hooks, Agent, Tasks, Coordinator, Compaction, plus cross-cutting permission/token/config engines | Degraded scope with subsystems pushed to M2/M3 |
| D10 | Agent execution model | **Effect-delegated state machine pool** (host maintains pool of SM instances) | True recursive nested SM instances |
| D11 | Tool concurrency model | Partition by `is_concurrency_safe`: read-only parallel, write serial, max 10 concurrent | All serial, all parallel, custom per-tool scheduler |
| D12 | Compaction layering | 5 explicit layers (Snip / Micro / CachedMicro / Collapse / Auto) + Reactive | Single autocompact, simple truncation |
| D13 | Hook protocol | Stdout JSON `HookResponse` can block / modify input / inject messages | Fire-and-forget only |
| D14 | Memory retrieval | LLM-driven selector (Sonnet side-query) with prefetch pipeline | Static injection only, keyword matching only |
| D15 | MCP server scoping | Per-agent scope (agent definitions can declare additional MCP servers, cleaned up on agent exit) | Single global registry |
| D16 | Shared protocol boundary | `lingxi-protocol` owns shared IDs, DTOs, effect envelopes, and effect results used by `core` and `traits` | Put effects in `core` and make `traits` depend on `core` (cycle), duplicate DTOs in each crate |
| D17 | Runtime boundary | Engine crates depend on an injected `RuntimeSpawner`, not directly on Tokio | Allow `tokio` in engine crates, make all background work host-owned only |
| D18 | Permission as dedicated crate | `lingxi-permission` is its own crate consumed by `tools`/`agent`/etc., not embedded in `core` | Embed permission engine in `core`, or split per-tool with no central policy |
| D19 | Secret containment | `Secret<T>` newtype + SecureStorage trait; engine touches secrets only via these. All 5 low-trust redaction boundaries enforced before egress | Plain `String` secrets, optional redaction, scattered keychain access |
| D20 | Cost as ground truth | Cost computed deterministically from normalized token usage × provider/model pricing catalog; persisted per-session; budget enforced pre-API | Trust server-reported cost, no client-side budget enforcement |
| D21 | Plugin materialization model | Plugins materialize Claude Code components into existing registries (Commands/Agents/Skills/Hooks/OutputStyles/MCP/LSP/config channels) on load and unload symmetrically | Invent an arbitrary tool ABI, or only declarative manifests with no materialized components |

---

## 3. Crate Topology

```
lingxi-core/                          ← workspace root
├── Cargo.toml                        ← workspace manifest
│
├── crates/
│   ├── protocol/                     ← ⭐ Shared DTOs/effects/IDs (zero OS deps)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── ids.rs                ← AgentId, SessionId, ToolUseId, RequestId, etc.
│   │   │   ├── effects.rs            ← Effect + EffectResult + EffectError
│   │   │   ├── messages.rs           ← ConversationMessage + API-neutral content blocks
│   │   │   ├── transport.rs          ← HTTP/MCP/process request/response DTOs
│   │   │   ├── secret.rs             ← Secret<T>, SecureStorageData, RedactableContent
│   │   │   └── capabilities.rs       ← PlatformCapabilities + capability flags
│   │   └── Cargo.toml                ← external deps: serde, serde_json, thiserror ONLY
│   │
│   ├── core/                         ← ⭐ State machine + reducer + session model (zero OS deps)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── state_machine.rs      ← conversation state machine
│   │   │   ├── events.rs             ← reducer input events
│   │   │   ├── effects.rs            ← core-to-protocol effect builders
│   │   │   ├── reducer.rs            ← pure state transitions
│   │   │   ├── prompt.rs             ← prompt assembly
│   │   │   ├── token.rs              ← token accounting primitives (pricing/budget lives in cost/)
│   │   │   ├── session.rs            ← session state model
│   │   │   ├── config.rs             ← config model & merge precedence
│   │   │   └── model.rs              ← model aliases + context-window metadata (pricing lives in cost/)
│   │   └── Cargo.toml                ← deps: protocol + serde, serde_json, thiserror, tracing
│   │
│   ├── traits/                       ← ⭐ Platform abstraction traits
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── filesystem.rs         ← FileSystem trait (+ watch)
│   │   │   ├── process.rs            ← ProcessRunner trait
│   │   │   ├── http.rs               ← HttpTransport trait
│   │   │   ├── mcp.rs                ← McpTransport trait (7 transport kinds)
│   │   │   ├── worktree.rs           ← WorktreeManager trait
│   │   │   ├── swarm.rs              ← SwarmBackend trait (tmux etc.)
│   │   │   ├── secure_storage.rs     ← SecureStorage trait (Keychain/libsecret/etc.)
│   │   │   ├── clock.rs              ← Clock trait
│   │   │   ├── notification.rs       ← NotificationSink trait (OS notifs)
│   │   │   ├── effect_handler.rs     ← EffectHandler trait
│   │   │   ├── hook_broadcaster.rs   ← HookEventBroadcaster trait
│   │   │   └── runtime.rs            ← RuntimeSpawner trait for background tasks/timers
│   │   └── Cargo.toml                ← deps: protocol, async-trait, serde, futures-core
│   │
│   ├── api-client/                   ← Anthropic/OpenAI-compat API client
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── anthropic.rs          ← Anthropic provider
│   │   │   ├── openai_compat.rs      ← OpenAI-compatible provider
│   │   │   ├── types.rs              ← MessageRequest, MessageResponse, StreamEvent
│   │   │   ├── sse.rs                ← SSE parser (pure logic)
│   │   │   ├── prompt_cache.rs       ← prompt caching
│   │   │   └── error.rs              ← API error types (PTL etc.)
│   │   └── Cargo.toml                ← deps: protocol, core, traits (HttpTransport)
│   │
│   ├── permission/                   ← ⭐ Permission Policy Engine (§14)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── mode.rs               ← 5 external + 2 internal PermissionMode
│   │   │   ├── rule.rs               ← PermissionRule + 8 RuleSource
│   │   │   ├── result.rs             ← PermissionResult + Decision reasons
│   │   │   ├── policy.rs             ← PermissionPolicy (central engine)
│   │   │   ├── classifier.rs         ← Yolo/Bash/Transcript classifier traits
│   │   │   ├── dangerous_patterns.rs ← Static bash danger table
│   │   │   ├── denial_tracking.rs    ← DenialTrackingState
│   │   │   ├── shadow.rs             ← ShadowedRuleDetector
│   │   │   └── update.rs             ← PermissionUpdate
│   │   └── Cargo.toml                ← deps: protocol, core, traits, tools
│   │
│   ├── secret/                       ← ⭐ Secret & Credential Management (§16)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── credential.rs         ← CredentialManager
│   │   │   ├── keychain_prefetch.rs  ← KeychainPrefetch
│   │   │   ├── scanner.rs            ← SecretScanner + 30+ gitleaks rules
│   │   │   ├── redaction.rs          ← RedactionPolicy + 5 boundaries
│   │   │   └── kinds.rs              ← SecretKind enum
│   │   └── Cargo.toml                ← deps: protocol, traits, regex
│   │
│   ├── cost/                         ← ⭐ Cost & Budget (§17)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── pricing.rs            ← provider/model pricing catalog
│   │   │   ├── usage.rs              ← normalized token usage counter
│   │   │   ├── calculator.rs         ← CostCalculator + token-class rates + cache savings
│   │   │   ├── tracker.rs            ← CostTracker + persistence
│   │   │   └── budget.rs             ← BudgetEnforcer + halt/ask/warn policies
│   │   └── Cargo.toml                ← deps: protocol, core, traits
│   │
│   ├── memory/                       ← ⭐ Memory System (§6)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── tier.rs               ← 4-tier memory model
│   │   │   ├── file.rs               ← MemoryFile + frontmatter parsing
│   │   │   ├── selector.rs           ← LLM-driven selector
│   │   │   ├── prefetch.rs           ← parallel prefetch pipeline
│   │   │   ├── session_memory.rs     ← session memory extractor
│   │   │   ├── team_memory.rs        ← team memory watcher + secret scanner
│   │   │   └── snapshot.rs           ← agent memory snapshot
│   │   └── Cargo.toml                ← deps: protocol, core, traits, api-client
│   │
│   ├── mcp/                          ← ⭐ MCP Lifecycle (§7)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── registry.rs           ← McpRegistry
│   │   │   ├── connection.rs         ← McpConnectionState (per-conn SM)
│   │   │   ├── capabilities.rs       ← ServerCapabilities, McpTool
│   │   │   ├── oauth.rs              ← OAuth flow
│   │   │   ├── approval.rs           ← ApprovalPolicy
│   │   │   ├── transport_spec.rs     ← 7 transport configs
│   │   │   └── agent_scope.rs        ← agent-scoped connections
│   │   └── Cargo.toml                ← deps: protocol, core, traits
│   │
│   ├── tools/                        ← ⭐ Tools System (§8)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── tool_trait.rs         ← Tool trait (30+ methods)
│   │   │   ├── context.rs            ← ToolUseContext
│   │   │   ├── registry.rs           ← ToolRegistry
│   │   │   ├── dispatcher.rs         ← ToolDispatcher + partition
│   │   │   ├── streaming_exec.rs     ← StreamingToolExecutor
│   │   │   ├── result_storage.rs     ← ToolResultStorage
│   │   │   ├── content_replacement.rs ← ContentReplacementState
│   │   │   ├── progress.rs           ← progress channel
│   │   │   └── permissions.rs        ← per-tool permission integration
│   │   └── Cargo.toml                ← deps: protocol, core, traits
│   │
│   ├── hooks/                        ← ⭐ Hooks System (§9)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── events.rs             ← 28 HookEvent variants
│   │   │   ├── definition.rs         ← HookDefinition + 4 executor kinds
│   │   │   ├── registry.rs           ← HookRegistry (multi-source)
│   │   │   ├── executor.rs           ← HookExecutor
│   │   │   ├── async_registry.rs     ← AsyncHookRegistry
│   │   │   ├── ssrf_guard.rs         ← SSRF protection
│   │   │   ├── builtin/              ← Builtin hook handlers
│   │   │   │   ├── mod.rs
│   │   │   │   ├── skill_improvement.rs
│   │   │   │   └── compact_warning.rs
│   │   │   └── response.rs           ← HookResponse + HookDecision
│   │   └── Cargo.toml                ← deps: protocol, core, traits
│   │
│   ├── agent/                        ← ⭐ Agent/Subagent (§10)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── definition.rs         ← AgentDefinition
│   │   │   ├── context.rs            ← SubagentContext
│   │   │   ├── pool.rs               ← StateMachinePool (sibling slots)
│   │   │   ├── runner.rs             ← SubagentRunner (effect delegation)
│   │   │   ├── multi_dispatch.rs     ← MultiAgentDispatcher
│   │   │   ├── tool_resolver.rs      ← AgentToolResolver
│   │   │   ├── permission_mode.rs    ← bubble/isolated/auto/plan
│   │   │   ├── color_manager.rs      ← AgentColorManager
│   │   │   ├── display.rs            ← AgentDisplay
│   │   │   ├── fork.rs               ← ForkSpawner (parent context inherit)
│   │   │   ├── transcript.rs         ← sidechain JSONL writer / resumer
│   │   │   └── worktree_policy.rs    ← WorktreeRequirement + degradation
│   │   └── Cargo.toml                ← deps: protocol, core, traits, tools, mcp, memory, hooks
│   │
│   ├── tasks/                        ← ⭐ Task Manager (§11)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── task_trait.rs         ← Task trait
│   │   │   ├── state.rs              ← TaskState (7 variants)
│   │   │   ├── registry.rs           ← TaskRegistry
│   │   │   ├── output_manager.rs     ← TaskOutputManager
│   │   │   ├── notification.rs       ← TaskNotificationBuilder
│   │   │   ├── cron.rs               ← CronTaskRegistry
│   │   │   ├── id.rs                 ← task ID generation
│   │   │   └── handlers/             ← Per-type handlers
│   │   │       ├── local_bash.rs
│   │   │       ├── local_agent.rs
│   │   │       ├── remote_agent.rs
│   │   │       ├── in_process_teammate.rs
│   │   │       ├── local_workflow.rs
│   │   │       ├── monitor_mcp.rs
│   │   │       └── dream.rs
│   │   └── Cargo.toml                ← deps: protocol, core, traits, agent
│   │
│   ├── coordinator/                  ← ⭐ Coordinator/Team (§12)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── mode.rs               ← CoordinatorMode
│   │   │   ├── internal_tools.rs     ← TeamCreate/Delete/SendMessage/SyntheticOutput
│   │   │   ├── team_registry.rs      ← TeamRegistry
│   │   │   ├── mailbox.rs            ← TeammateMailbox + Router
│   │   │   └── swarm.rs              ← SwarmBackend integration
│   │   └── Cargo.toml                ← deps: protocol, core, traits, agent, tasks, tools, memory
│   │
│   ├── plugin/                       ← ⭐ Plugin System (§15)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── manifest.rs           ← PluginManifest + Source + TrustLevel
│   │   │   ├── lifecycle.rs          ← PluginState (7 variants)
│   │   │   ├── manager.rs            ← PluginManager + load/unload
│   │   │   ├── marketplace.rs        ← MarketplaceManager + reconciler
│   │   │   ├── blocklist.rs          ← PluginBlocklist (static + remote)
│   │   │   ├── strict_policy.rs      ← StrictPluginOnlyPolicy
│   │   │   ├── loaders/              ← per-component loaders
│   │   │   │   ├── commands.rs
│   │   │   │   ├── agents.rs
│   │   │   │   ├── skills.rs
│   │   │   │   ├── hooks.rs
│   │   │   │   ├── output_styles.rs
│   │   │   │   ├── mcp_servers.rs
│   │   │   │   ├── lsp_servers.rs
│   │   │   │   └── channels.rs
│   │   │   └── mcpb.rs               ← MCP Bundle (.mcpb zip) parser
│   │   └── Cargo.toml                ← deps: protocol, core, traits, hooks, mcp, agent
│   │
│   ├── compaction/                   ← ⭐ Compaction Engine (§13)
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── orchestrator.rs       ← CompactionOrchestrator (5-layer scheduler)
│   │   │   ├── snip.rs               ← SnipCompactor
│   │   │   ├── microcompact.rs       ← Microcompactor
│   │   │   ├── cached_microcompact.rs ← CachedMicrocompactor (cache edits)
│   │   │   ├── context_collapse.rs   ← ContextCollapsor
│   │   │   ├── autocompact.rs        ← Autocompactor
│   │   │   ├── reactive.rs           ← ReactiveCompactor
│   │   │   ├── session_memory.rs     ← SessionMemoryCompactor (dual extraction)
│   │   │   ├── post_compact.rs       ← PostCompactBuilder
│   │   │   ├── grouping.rs           ← group_messages_by_api_round
│   │   │   ├── ptl_retry.rs          ← truncate_head_for_ptl_retry
│   │   │   └── thresholds.rs         ← all constants
│   │   └── Cargo.toml                ← deps: protocol, core, traits, api-client, hooks
│   │
│   ├── test-harness/                 ← Contract tests + property tests + mocks
│   │   ├── src/
│   │   │   ├── lib.rs
│   │   │   ├── mocks/                ← Mock impls of all traits
│   │   │   ├── contracts/            ← Contract test suites
│   │   │   └── properties/           ← Property tests (proptest)
│   │   └── Cargo.toml                ← deps: all crates, proptest, tokio (test only)
│   │
│   └── uniffi-bridge/                ← narrow UniFFI facade over engine handles
│       ├── src/lib.rs
│       ├── src/lingxi_core.udl       ← DTO/handle-only UniFFI interface
│       └── Cargo.toml                ← deps: protocol + selected subsystem crates, uniffi
│
├── platforms/                        ← Platform crates (separate downstream workspaces)
│   ├── posix-minimal/                ← M1 demo host: mockable FS/process/http/MCP subset
│   ├── posix/                        ← M2 production Linux + macOS
│   ├── windows/                      ← M2 Windows
│   ├── android/                      ← Android (JNI via UniFFI)
│   └── ios/                          ← iOS (Swift via UniFFI)
│
└── examples/
    └── cli-demo/                     ← Minimal M1 CLI over posix-minimal + mocks
```

### Key Principles

1. **`protocol` owns shared boundary types**: IDs, request/response DTOs, `Effect`, `EffectResult`, and `EffectError` live here so `core` and `traits` never form a dependency cycle.
2. **`core` crate Cargo.toml PROHIBITS OS dependencies**: internal dep `protocol` plus external `serde`, `serde_json`, `thiserror`, `tracing` only.
3. **`core` and `traits` are siblings**: both depend on `protocol`; neither depends on the other. Host/run-loop code maps protocol effects to trait calls.
4. **Each subsystem is a separate crate**: clear dependency graph, parallel development, independent versioning.
5. **`api-client` depends on `traits::HttpTransport`**: does not use reqwest directly.
6. **`platforms/posix-minimal` is M1 demo-only**: production platform crates stay out of the core workspace and land in M2.

### Crate Dependency Graph

```
                          ┌──────────┐
                          │ protocol │  (shared DTOs/effects/IDs)
                          └────┬─────┘
                               │
                ┌──────────────┴──────────────┐
                ▼                             ▼
          ┌─────────┐                   ┌─────────┐
          │  core   │                   │ traits  │
          └────┬────┘                   └────┬────┘
               └──────────────┬──────────────┘
       ┌───────────────┬──────┴──────┬───────────────┐
       │               │             │               │
  ┌────▼────┐   ┌──────▼───┐   ┌─────▼─────┐  ┌──────▼─────┐
  │api-client│   │  secret  │   │   cost    │  │ permission │
  └────┬────┘   └──────┬───┘   └─────┬─────┘  └──────┬─────┘
       │               │             │               │
       │          ┌────▼────┐   ┌────▼─────┐         │
       │          │ memory  │   │   mcp    │         │
       │          └────┬────┘   └────┬─────┘         │
       │               │             │               │
       │               │        ┌────▼─────┐         │
       │               └───────►│  tools   │◄────────┘
       │                        └────┬─────┘
       │                             │
       │                        ┌────▼────┐
       │                        │  hooks  │
       │                        └────┬────┘
       │                             │
       │                        ┌────▼────┐
       └───────────────────────►│  agent  │
                                └────┬────┘
                    ┌────────────────┼────────────────┐
                ┌───▼────┐    ┌──────▼──────┐
                │ tasks  │    │ coordinator │
                └───┬────┘    └──────┬──────┘
                    │                │
                    └────────┬───────┘
                       ┌─────▼──────┐
                       │ compaction │
                       └─────┬──────┘
                             │
                       ┌─────▼──────┐
                       │   plugin   │ (materializes Claude Code components)
                       └─────┬──────┘
                             │
                       ┌─────▼───────┐
                       │test-harness │
                       └─────┬───────┘
                             │
                       ┌─────▼───────┐
                       │uniffi-bridge│  (facade only)
                       └─────────────┘
```

### UniFFI Boundary Rule

`lingxi-uniffi-bridge` is a facade crate, not a binding dump of every Rust API. It may expose:

- Plain DTOs from `lingxi-protocol`
- Opaque handles such as `EngineHandle`, `SessionHandle`, `TaskHandle`, and `StreamHandle`
- Async facade methods returning DTOs or handles

It must not expose Rust-only internals: `dyn Trait`, `Arc<dyn Tool>`, `Stream<Item = ...>`, closures, generic functions, raw channels, or subsystem registry types. Those stay behind facade methods that translate to protocol DTOs.

---

## 4. Trait System

All traits live in `crates/traits/` and import shared DTOs from `lingxi-protocol`. Core emits protocol effects and does not consume traits directly; subsystem orchestrators and host run loops consume traits via generics or `dyn Trait`. **Core never touches OS directly and `traits` never depends on `core`.**

### 4.1 FileSystem (extended)

```rust
#[async_trait]
pub trait FileSystem: Send + Sync {
    async fn read_file(&self, path: &str, offset: Option<u64>, limit: Option<u64>) -> Result<FileContent, FsError>;
    async fn write_file(&self, path: &str, content: &str) -> Result<(), FsError>;
    async fn append_file(&self, path: &str, content: &str) -> Result<(), FsError>;
    async fn edit_file(&self, path: &str, edits: &[EditOperation]) -> Result<EditResult, FsError>;
    async fn delete_file(&self, path: &str) -> Result<(), FsError>;
    async fn file_size(&self, path: &str) -> Result<u64, FsError>;
    async fn glob(&self, pattern: &str, cwd: &str) -> Result<Vec<String>, FsError>;
    async fn grep(&self, pattern: &str, paths: &[String], context_lines: u32) -> Result<Vec<GrepMatch>, FsError>;
    fn is_within_workspace(&self, path: &str) -> bool;
    async fn is_binary(&self, path: &str) -> Result<bool, FsError>;
    /// Watch directory for changes (team memory sync)
    async fn watch(&self, dir: &str) -> Result<Box<dyn Stream<Item = FileEvent> + Send + Unpin>, FsError>;
    /// Symlink (for task output streaming)
    async fn symlink(&self, target: &str, link: &str) -> Result<(), FsError>;
    fn capabilities(&self) -> FileSystemCapabilities;
}

pub struct FileSystemCapabilities {
    pub max_read_size: u64,
    pub max_write_size: u64,
    pub supports_symlinks: bool,
    pub supports_watch: bool,
    pub sandbox_root: Option<String>,
}
```

### 4.2 ProcessRunner

```rust
#[async_trait]
pub trait ProcessRunner: Send + Sync {
    async fn run(&self, cmd: &ProcessCommand) -> Result<ProcessOutput, ProcessError>;
    fn is_available(&self) -> bool;
    async fn spawn_background(&self, cmd: &ProcessCommand) -> Result<ProcessHandle, ProcessError>;
    async fn kill(&self, handle: &ProcessHandle) -> Result<(), ProcessError>;
}
```

### 4.3 HttpTransport

```rust
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
    async fn stream_sse(&self, req: HttpRequest) -> Result<Box<dyn Stream<Item = Result<SseEvent, HttpError>> + Send + Unpin>, HttpError>;
}
```

### 4.4 McpTransport (full)

```rust
#[async_trait]
pub trait McpTransport: Send + Sync {
    async fn connect(&self, spec: &McpTransportSpec) -> Result<McpRawConnection, McpError>;
    async fn initialize(&self, conn: &McpRawConnection) -> Result<ServerCapabilities, McpError>;
    async fn list_tools(&self, conn: &McpRawConnection) -> Result<Vec<McpTool>, McpError>;
    async fn list_resources(&self, conn: &McpRawConnection) -> Result<Vec<McpResource>, McpError>;
    async fn list_prompts(&self, conn: &McpRawConnection) -> Result<Vec<McpPrompt>, McpError>;
    async fn call_tool(&self, conn: &McpRawConnection, tool: &str, input: Value) -> Result<McpToolResult, McpError>;
    async fn read_resource(&self, conn: &McpRawConnection, uri: &str) -> Result<McpResourceContent, McpError>;
    async fn ping(&self, conn_id: McpConnectionId) -> Result<(), McpError>;
    async fn notifications(&self, conn: &McpRawConnection) -> Result<Box<dyn Stream<Item = McpNotification> + Send + Unpin>, McpError>;
    async fn handle_elicitation(&self, conn: &McpRawConnection, request: ElicitRequest) -> Result<ElicitResult, McpError>;
    async fn disconnect(&self, conn_id: McpConnectionId) -> Result<(), McpError>;
    fn supported_transports(&self) -> Vec<McpTransportKind>;
}
```

### 4.5 WorktreeManager

```rust
#[async_trait]
pub trait WorktreeManager: Send + Sync {
    async fn create_worktree(&self, slug: &str, base_branch: Option<&str>, copy_includes: &[PathBuf]) -> Result<WorktreeHandle, WorktreeError>;
    async fn remove_worktree(&self, handle: &WorktreeHandle) -> Result<(), WorktreeError>;
    async fn list_worktrees(&self) -> Result<Vec<WorktreeInfo>, WorktreeError>;
    async fn cleanup_stale(&self, max_age: Duration) -> Result<Vec<PathBuf>, WorktreeError>;
    fn is_supported(&self) -> bool;
}
```

### 4.6 SwarmBackend

```rust
#[async_trait]
pub trait SwarmBackend: Send + Sync {
    async fn start_swarm(&self, layout: SwarmLayout) -> Result<SwarmHandle, SwarmError>;
    async fn create_teammate_pane(&self, agent_id: &AgentId, position: PanePosition) -> Result<PaneId, SwarmError>;
    async fn create_teammate_pane_with_leader(&self, leader: &AgentId, followers: &[AgentId]) -> Result<Vec<PaneId>, SwarmError>;
    async fn toggle_teammate_visibility(&self, agent_id: &AgentId) -> Result<(), SwarmError>;
    async fn destroy_swarm(&self, handle: SwarmHandle) -> Result<(), SwarmError>;
    fn is_available(&self) -> bool;
}
```

### 4.7 EffectHandler

```rust
use lingxi_protocol::{Effect, EffectError, EffectResult};

#[async_trait]
pub trait EffectHandler: Send + Sync {
    async fn handle(&self, effect: Effect) -> Result<EffectResult, EffectError>;
}
```

### 4.8 RuntimeSpawner

Engine crates must not call a concrete runtime such as `tokio::spawn`. Background work, timers, and cancellation are delegated through this trait, with Tokio only used by platform/test hosts.

```rust
#[async_trait]
pub trait RuntimeSpawner: Send + Sync {
    async fn spawn(&self, name: &str, task: Pin<Box<dyn Future<Output = ()> + Send + 'static>>) -> Result<BackgroundTaskHandle, RuntimeError>;
    async fn sleep(&self, duration: Duration);
    async fn cancel(&self, handle: &BackgroundTaskHandle) -> Result<(), RuntimeError>;
}
```

### 4.9 SecureStorage

Used by §16 Secret & Credential Management. `SecureStorageData` and `SecureStorageBackend`
are protocol DTOs; platform impls vary (Keychain / libsecret / CredVault / mobile keystores /
encrypted-file / plain-text fallback).

```rust
#[async_trait]
pub trait SecureStorage: Send + Sync {
    async fn store(&self, service: &str, account: &str, data: SecureStorageData) -> Result<(), SecureStorageError>;
    async fn retrieve(&self, service: &str, account: &str) -> Result<Option<SecureStorageData>, SecureStorageError>;
    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError>;
    async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError>;
    fn is_encrypted(&self) -> bool;
    fn backend(&self) -> SecureStorageBackend;
}
```

### 4.10 Clock, NotificationSink, HookEventBroadcaster

```rust
pub trait Clock: Send + Sync {
    fn now(&self) -> SystemTime;
    fn elapsed_since(&self, earlier: SystemTime) -> Duration;
}

#[async_trait]
pub trait NotificationSink: Send + Sync {
    async fn notify(&self, notif: OsNotification) -> Result<(), NotificationError>;
}

#[async_trait]
pub trait HookEventBroadcaster: Send + Sync {
    async fn emit(&self, event: HookExecutionEvent);
    fn subscribe(&self) -> Box<dyn Stream<Item = HookExecutionEvent> + Send + Unpin>;
}
```

### 4.11 Capability System

```rust
#[derive(Debug, Clone)]
pub struct PlatformCapabilities {
    pub filesystem: FileSystemCapabilities,
    pub process: bool,
    pub http: bool,
    pub mcp: bool,
    pub supported_mcp_transports: Vec<McpTransportKind>,
    pub worktree: bool,
    pub swarm: bool,
    pub os_notifications: bool,
}

impl PlatformCapabilities {
    pub fn filter_tools(&self, all_tools: &[ToolSpec]) -> Vec<ToolSpec> {
        all_tools.iter()
            .filter(|tool| tool.required_capabilities.is_satisfied_by(self))
            .cloned()
            .collect()
    }
}
```

---

## 5. State Machine

The heart of `core`. All conversation logic = **deterministic state machine**. Side effects via `Effect` enum, handled by platform `EffectHandler`.

### 5.1 State Definitions (extended)

```rust
#[derive(Debug, Clone)]
pub enum ConversationState {
    // Core conversation states
    Idle { session: SessionState },
    AssemblingPrompt { session: SessionState, user_message: String },
    AwaitingApiResponse { session: SessionState, request_id: RequestId },
    StreamingResponse { session: SessionState, request_id: RequestId, partial_response: PartialResponse },

    // Tool execution
    ToolUseReceived {
        session: SessionState,
        tool_calls: Vec<ToolCall>,
        pending_results: Vec<Option<ToolResult>>,
    },
    AwaitingPermission {
        session: SessionState,
        tool_call: ToolCall,
        remaining_calls: Vec<ToolCall>,
    },
    AwaitingToolResult {
        session: SessionState,
        tool_call: ToolCall,
        remaining_calls: Vec<ToolCall>,
        completed_results: Vec<ToolResult>,
    },
    PreparingToolResponse {
        session: SessionState,
        tool_results: Vec<ToolResult>,
    },

    // Subagent (§10) — Effect-delegated, not nested SM
    AwaitingSubagent {
        session: SessionState,
        agent_id: AgentId,
        is_async: bool,
        progress: AgentProgress,
        remaining_tool_calls: Vec<ToolCall>,
        completed_results: Vec<ToolResult>,
    },

    // Compaction (§13) — full layering
    Compacting {
        session: SessionState,
        layers_running: Vec<CompactionLayer>,
        reason: CompactionReason,
    },

    // Memory (§6) prefetch awaiting
    MemoryPrefetchInProgress {
        session: SessionState,
        prefetch_id: PrefetchId,
        next_state: Box<ConversationState>,
    },

    // Hook blocking
    HookBlocked {
        session: SessionState,
        hook_id: HookId,
        blocked_at: HookEventType,
        reason: String,
    },

    Terminated {
        session: SessionState,
        reason: TerminationReason,
    },
}
```

### 5.2 Event Definitions (canonical full list)

Events are partitioned by source:

```rust
#[derive(Debug, Clone)]
pub enum Event {
    // === User input ===
    UserMessage { message_id: MessageId, request_id: RequestId, content: String },
    UserInterrupt,
    UserExit,

    // === API responses ===
    ApiStreamStart { request_id: RequestId },
    ApiStreamDelta { request_id: RequestId, delta: ContentDelta },
    ApiStreamEnd { request_id: RequestId, response: MessageResponse, usage: Usage },
    ApiError { request_id: RequestId, error: ApiError },

    // === Cost/Budget (§17) ===
    CostRecorded { model_ref: ModelRef, usage: Usage, cost_nano_usd: u64 },
    BudgetThresholdReached { pct: u32, current: u64, limit: u64 },
    BudgetExceeded { current: u64, limit: u64 },
    UnpricedModelDetected { model_ref: ModelRef },
    CostStateRestored { state: CostState },

    // === Tool execution (§8 + §9) ===
    ToolStarted { tool_use_id: ToolUseId, tool_name: String },
    ToolProgress { tool_use_id: ToolUseId, progress: ToolProgress },
    ToolExecutionComplete { call_id: ToolUseId, result: ToolResult },
    ToolExecutionError { call_id: ToolUseId, error: ToolError },
    ToolValidationFailed { tool_use_id: ToolUseId, error: ValidationError },
    ToolNotFound { tool_use_id: ToolUseId, name: String },

    // === Permission (§14) ===
    PermissionGranted { call_id: ToolUseId, scope: PermissionScope },
    PermissionDenied { call_id: ToolUseId },
    PermissionRuleAdded { rule: PermissionRule, source: PermissionRuleSource },
    PermissionRuleRemoved { rule: PermissionRule, source: PermissionRuleSource },
    PermissionModeSwitched { from: PermissionMode, to: PermissionMode },
    PendingClassifierStarted { tool_use_id: ToolUseId, kind: ClassifierKind },
    ClassifierScoreReceived { tool_use_id: ToolUseId, kind: ClassifierKind, score: ClassifierScore },
    BypassKillswitchActivated,
    DenialLimitReached { tool: String, count: u32 },

    // === Hooks (§9) ===
    HookCompleted { hook_id: HookId, result: HookResult },
    HookBlockedTool { tool_use_id: ToolUseId, reason: String },
    HookModifiedInput { tool_use_id: ToolUseId, new_input: Value },
    HookInjectedMessage { message: ConversationMessage },
    HookRequestedStop { reason: String },
    HookPreventedStop { reason: String },

    // === Compaction (§13) ===
    MicrocompactApplied { tokens_freed: u64, cleared_count: usize },
    SnipApplied { tokens_freed: u64, removed_count: usize },
    ContextCollapseApplied { tokens_freed: u64 },
    AutocompactStarted { reason: CompactionReason },
    AutocompactCompleted { result: CompactionResult },
    AutocompactFailed { error: String, consecutive_failures: u32 },
    ReactiveCompactTriggered { ptl_error: String },
    SessionMemoryExtracted { content: String },
    PostCompactCleanupComplete { files_restored: usize, skills_restored: usize },

    // === Memory (§6) ===
    MemoryPrefetchComplete { paths: Vec<PathBuf> },
    MemoryPrefetchTimeout,
    TeamMemoryUpdated { path: PathBuf, content: String },

    // === Secret/Credential (§16) ===
    SecretDetected { boundary: RedactionBoundary, rule_id: String, redacted: bool },
    CredentialStored { kind: SecretKind },
    CredentialRefreshed { kind: SecretKind },
    CredentialDeleted { kind: SecretKind },
    OAuthTokenExpired { service: String },
    KeychainPrefetchComplete,

    // === MCP (§7) ===
    McpConnected { name: String, tools: Vec<McpTool> },
    McpDisconnected { name: String, error: Option<String> },
    McpHealthCheckFailed { name: String, attempt: u32 },
    McpReconnected { name: String },
    McpToolUpdated { name: String, new_tools: Vec<McpTool> },
    McpOAuthCallbackReceived { name: String, code: String, state: String },
    McpApprovalDecision { name: String, approved: bool },

    // === Plugin (§15) ===
    PluginInstalled { id: PluginId, source: PluginSource },
    PluginUninstalled { id: PluginId },
    PluginEnabled { id: PluginId },
    PluginDisabled { id: PluginId },
    PluginUpdated { id: PluginId, from_version: String, to_version: String },
    PluginLoadFailed { id: PluginId, error: String },
    MarketplaceSynced { name: String, entries: usize },
    PluginBlocked { id: PluginId, reason: String },

    // === Agent/Subagent (§10) ===
    SubagentSlotAllocated { agent_id: AgentId },
    SubagentSlotDeallocated { agent_id: AgentId },
    SubagentProgress { agent_id: AgentId, progress: AgentProgress },
    SubagentCompleted { agent_id: AgentId, result: AgentResult },
    SubagentFailed { agent_id: AgentId, error: String },
    SubagentKilled { agent_id: AgentId },
    SubagentMessageDrained { agent_id: AgentId, messages: Vec<TeammateMessage> },
    WorktreeReadyForAgent { agent_id: AgentId, handle: Option<WorktreeHandle> },
    AgentMemorySnapshotBuilt { agent_id: AgentId, snapshot_id: SnapshotId },

    // === Tasks (§11) ===
    TaskCreated { task_id: String, task_type: TaskType },
    TaskStatusChanged { task_id: String, old: TaskStatus, new: TaskStatus },
    TaskOutputAppended { task_id: String, chunk: String },
    TaskProgressUpdated { task_id: String, progress: TaskProgress },
    TaskCompleted { task_id: String, status: TaskStatus, result: Option<Value> },
    PendingNotificationReady { notification: PendingNotification },
    TaskMessageDrained { task_id: String, messages: Vec<String> },

    // === Coordinator/Team (§12) ===
    CoordinatorModeEntered,
    CoordinatorModeExited,
    WorkerSpawned { agent_id: AgentId, agent_type: String, name: String },
    WorkerDeleted { agent_id: AgentId },
    WorkerStatusChanged { agent_id: AgentId, status: WorkerStatus },
    TeammateMessageReceived { from: AgentId, to: AgentId, message: TeammateMessage },
    TeammateIdle { agent_id: AgentId },
    SyntheticOutputInjected { agent_id: AgentId, content: String },

    // === System ===
    SessionLoaded(SessionState),
    ConfigReloaded(Config),
    TokenBudgetExceeded,
}
```

### 5.3 Effect Definitions (full list)

`Effect`, `EffectResult`, and `EffectError` are defined in `lingxi-protocol` because both `core` and `traits::EffectHandler` need them. They are documented here because the reducer emits these effects.

```rust
#[derive(Debug, Clone)]
pub enum Effect {
    // === API ===
    SendApiRequest { request_id: RequestId, request: MessageRequest },
    SendSideQuery { request_id: RequestId, request: MessageRequest, purpose: SideQueryPurpose },

    // === Cost/Budget (§17) ===
    PersistCostState { state: CostState },
    LoadCostState { session_id: SessionId },
    DisplayCostUpdate { snapshot: CostState },
    EnforceBudget { estimated_cost_nano_usd: u64 },
    FireBudgetWarning { pct: u32, current: u64, limit: u64 },
    HaltOnBudget,

    // === Tool execution (§8) ===
    ExecuteTool { call_id: ToolUseId, tool_name: String, tool_input: Value },
    RequestPermission { call_id: ToolUseId, tool_name: String, action_description: String, risk_level: RiskLevel },
    EvaluatePermission { tool_use_id: ToolUseId, tool_name: String, input: Value },
    PersistPermissionUpdate { update: PermissionUpdate },
    RunClassifier { kind: ClassifierKind, tool_use_id: ToolUseId, tool_name: String, input: Value },
    DetectShadowedRules { rule: PermissionRule },

    // === Render ===
    RenderStreamDelta { text: String },
    RenderToolStart { tool_name: String, call_id: ToolUseId },
    RenderToolResult { call_id: ToolUseId, result: ToolResult },
    RenderError { error: String },
    RenderCostUpdate { usage: CumulativeUsage },

    // === Persistence ===
    PersistSession { session: SessionState },
    PersistConfig { config: Config },

    // === Memory (§6) ===
    StartMemoryPrefetch { query: String, memory_dir: PathBuf },
    LoadStaticMemory { tier: MemoryTier },
    ExtractSessionMemory { up_to_message: MessageId },
    PersistSessionMemory { content: String },
    StartTeamMemoryWatch { team_dir: PathBuf },

    // === Secret/Credential (§16) ===
    StoreCredential { kind: SecretKind, data: SecureStorageData },
    RetrieveCredential { kind: SecretKind },
    DeleteCredential { kind: SecretKind },
    RefreshOAuthToken { service: String },
    ScanForSecrets { boundary: RedactionBoundary, content: RedactableContent },
    RedactContent { content: RedactableContent },

    // === MCP (§7) ===
    ConnectMcpServer { name: String, config: McpServerConfig },
    DisconnectMcpServer { name: String },
    CallMcpTool { connection_id: McpConnectionId, tool: String, input: Value },
    StartOAuthFlow { name: String, oauth_config: McpOAuthConfig },
    StartMcpHealthCheck { interval: Duration },
    RequestMcpApproval { name: String, config: McpServerConfig },
    ConnectMcpForAgent { agent_id: AgentId, servers: Vec<McpServerConfig> },
    CleanupMcpForAgent { agent_id: AgentId },

    // === Plugin (§15) ===
    InstallPlugin { source: PluginSource },
    UninstallPlugin { id: PluginId },
    EnablePlugin { id: PluginId },
    DisablePlugin { id: PluginId },
    UpdatePlugin { id: PluginId },
    SyncMarketplace { name: String },
    FetchPluginBlocklist,

    // === Agent/Subagent (§10) ===
    AllocateStateMachineSlot { context: SubagentContext },
    DeallocateStateMachineSlot { agent_id: AgentId },
    SpawnMultiAgents { specs: Vec<MultiAgentSpawnSpec>, coordinator_id: AgentId },
    SendMessageToSubagent { agent_id: AgentId, message: String },
    CreateWorktreeForAgent { agent_id: AgentId, requirement: WorktreeRequirement, slug: String },
    RemoveWorktreeForAgent { agent_id: AgentId, handle: WorktreeHandle },
    AssignAgentColor { agent_id: AgentId },
    ReleaseAgentColor { agent_id: AgentId },
    BuildAgentMemorySnapshot { agent_id: AgentId, filter: Option<AgentMemoryFilter> },
    KillSubagent { agent_id: AgentId },

    // === Tasks (§11) ===
    SpawnTask { task_type: TaskType, input: TaskSpawnInput, description: String },
    KillTask { task_id: String },
    UpdateTask { task_id: String, update: TaskUpdate },
    QueryTask { task_id: String },
    ListTasks { filter: TaskFilter },
    ReadTaskOutput { task_id: String, opts: OutputOptions },
    SendMessageToTask { task_id: String, message: String },
    InjectTaskNotification { notification: PendingNotification },
    EvictTask { task_id: String },
    RegisterCron { def: CronTaskDef },
    UnregisterCron { id: String },

    // === Coordinator/Team (§12) ===
    EnterCoordinatorMode,
    ExitCoordinatorMode,
    SpawnWorker { agent_type: String, name: String, initial_prompt: String },
    DeleteWorker { agent_id: AgentId },
    SendTeammateMessage { from: MessageSender, to: AgentId, content: String },
    InjectSyntheticOutput { agent_id: AgentId, content: String },
    StartSwarmView { layout: SwarmLayout },
    ToggleTeammateVisibility { agent_id: AgentId },
    StartTeamMemorySync { team_dir: PathBuf },

    // === Hooks (§9) ===
    ExecuteHook { event: HookEvent },
    RegisterFrontmatterHooks { agent_id: AgentId, hooks: Vec<HookDefinition> },
    ClearSessionHooks { agent_id: AgentId },
    DrainAsyncHooks { timeout: Duration },

    // === Compaction (§13) ===
    RunSnipCompact { messages: Vec<ConversationMessage>, budget: u64 },
    RunMicrocompact { messages: Vec<ConversationMessage>, now: SystemTime },
    RunContextCollapse { messages: Vec<ConversationMessage> },
    RunAutocompact { messages: Vec<ConversationMessage>, ctx: CompactionContext },
    RunReactiveCompact { error: PromptTooLongError, messages: Vec<ConversationMessage> },
    BuildPostCompactMessages { summary: String, original: Vec<ConversationMessage> },
    RunPostCompactCleanup { compaction_result: CompactionResult },
    FireCompactionWarning { tokens_remaining: u64 },

    // === Lifecycle ===
    LoadSession { session_id: SessionId },
    RecordUnexpectedEvent { state_name: String, event_name: String },
    Terminate { reason: TerminationReason },
}
```

### 5.4 Reducer (Pure Function)

```rust
/// Core reducer — pure function, no side effects.
/// (current_state, event) → (new_state, effects_to_emit)
pub fn reduce(state: ConversationState, event: Event) -> (ConversationState, Vec<Effect>) {
    match (state, event) {
        // Idle → AssemblingPrompt → AwaitingApi
        (ConversationState::Idle { session }, Event::UserMessage { message_id, request_id, content }) => {
            // Synchronously: assemble prompt, send API request
            let request = assemble_request(&session, &content);
            let mut session = session;
            session.history.push(ConversationMessage::user(message_id, content.clone()));
            (
                ConversationState::AwaitingApiResponse { session, request_id },
                vec![
                    Effect::SendApiRequest { request_id, request },
                    Effect::StartMemoryPrefetch {
                        query: content,
                        memory_dir: session.memory_dir.clone(),
                    },
                ],
            )
        }

        // StreamingResponse → ToolUseReceived
        (
            ConversationState::StreamingResponse { mut session, .. },
            Event::ApiStreamEnd { response, usage, .. },
        ) if response.has_tool_use() => {
            session.usage.add(&usage);
            let tool_calls = response.extract_tool_calls();
            let pending = vec![None; tool_calls.len()];
            (
                ConversationState::ToolUseReceived {
                    session: session.clone(),
                    tool_calls,
                    pending_results: pending,
                },
                vec![Effect::RenderCostUpdate { usage: session.usage.clone() }],
            )
        }

        // ToolUseReceived → AwaitingPermission (per tool with permission requirement)
        // ToolUseReceived → AwaitingToolResult (per tool without)
        // ... (~30 more transitions in production)

        // Subagent spawn
        (
            ConversationState::ToolUseReceived { mut session, tool_calls, .. },
            // Internal trigger after subagent context built
            Event::SubagentSlotAllocated { agent_id },
        ) => {
            let (this_call, remaining) = tool_calls.split_first().unwrap();
            (
                ConversationState::AwaitingSubagent {
                    session: session.clone(),
                    agent_id,
                    is_async: false,
                    progress: AgentProgress::default(),
                    remaining_tool_calls: remaining.to_vec(),
                    completed_results: vec![],
                },
                vec![],
            )
        }

        // Subagent progress (real-time)
        (
            ConversationState::AwaitingSubagent { session, agent_id: a, mut progress, remaining_tool_calls, completed_results, is_async },
            Event::SubagentProgress { agent_id, progress: new_progress },
        ) if a == agent_id => {
            progress = new_progress;
            (
                ConversationState::AwaitingSubagent { session, agent_id: a, progress, remaining_tool_calls, completed_results, is_async },
                vec![],
            )
        }

        // ... 60+ more transitions covering all subsystem interactions

        // Terminated is final
        (terminated @ ConversationState::Terminated { .. }, _) => (terminated, vec![]),

        // Catch-all
        (state, event) => {
            (state, vec![Effect::RenderError {
                error: "Unexpected event in current state".to_string(),
            }, Effect::RecordUnexpectedEvent {
                state_name: state.kind().to_string(),
                event_name: event.kind().to_string(),
            }])
        }
    }
}
```

The full reducer has approximately **100 transition arms** to cover all subsystem interactions. The pure-function property is preserved throughout: all IDs, timestamps, randomness, logging, and I/O are provided by input events or emitted as effects, never generated inside `reduce`.

### 5.5 Run Loop (Platform Layer)

Identical to v1 design — the platform layer drives the SM by receiving events and dispatching effects.

---

## 6. Memory System

4-tier memory model with LLM-driven retrieval and parallel prefetch.

### 6.1 Four-Tier Model

```rust
#[derive(Debug, Clone)]
pub enum MemoryTier {
    /// Project: CLAUDE.md at repo root (git tracked, static injection)
    Project { repo_root: PathBuf },
    /// User: ~/.claude/MEMORY.md (index) + ~/.claude/memory/*.md (content)
    User { user_memory_dir: PathBuf },
    /// Session: auto-extracted during conversation by forked agent
    Session { session_id: SessionId },
    /// Team: shared across multi-agent, filesystem-watched
    Team { team_dir: PathBuf, watcher_enabled: bool },
}
```

### 6.2 Memory File & Frontmatter

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub frontmatter: MemoryFrontmatter,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryFrontmatter {
    pub memory_type: String,         // "tool_usage" / "warning" / "reference" / ...
    pub description: String,         // Selector input
    pub tags: Vec<String>,
    pub created_at: Option<SystemTime>,
    pub related_tools: Vec<String>,
}

pub const MAX_ENTRYPOINT_LINES: usize = 200;
pub const MAX_ENTRYPOINT_BYTES: usize = 25_000;
```

### 6.3 LLM-Driven Selector (Core Innovation)

```rust
/// Uses Sonnet as side-query to pick relevant memories (max 5)
pub struct MemorySelector {
    selector_model: ModelAlias,
    max_selected: usize,
}

impl MemorySelector {
    pub async fn select_relevant(
        &self,
        query: &str,
        available_memories: &[MemoryFile],
        recent_tools: &[String],
        already_surfaced: &HashSet<PathBuf>,
    ) -> Result<Vec<PathBuf>, MemoryError> {
        let candidates: Vec<&MemoryFile> = available_memories.iter()
            .filter(|m| !already_surfaced.contains(&m.path))
            .collect();
        if candidates.is_empty() { return Ok(vec![]); }
        let prompt = build_memory_selector_prompt(query, &candidates, recent_tools);
        // Side-query — emitted via Effect::SendSideQuery
        let selection = side_query_for_selection(prompt).await?;
        Ok(parse_filenames(&selection, &candidates))
    }
}
```

### 6.4 Parallel Prefetch Pipeline

```rust
/// Runs in parallel with main API call
pub struct MemoryPrefetch {
    selector: Arc<MemorySelector>,
    runtime: Arc<dyn RuntimeSpawner>,
    settled_at: AtomicU64,
}

impl MemoryPrefetch {
    pub async fn start(&self, query: String, memory_dir: PathBuf) -> Result<PendingMemoryPrefetch, RuntimeError> {
        let selector = self.selector.clone();
        let (result_tx, result_rx) = oneshot::channel();
        let task = self.runtime.spawn("memory-prefetch", Box::pin(async move {
            let result = async {
                let memories = scan_memory_files(&memory_dir).await?;
                selector.select_relevant(&query, &memories, &[], &Default::default()).await
            }.await;
            let _ = result_tx.send(result);
        })).await?;
        Ok(PendingMemoryPrefetch { task, result_rx, settled_at: self.settled_at.clone() })
    }
    /// Non-blocking consume with timeout
    pub fn consume(self, max_wait: Duration) -> Vec<PathBuf> { ... }
}
```

### 6.5 Session Memory Extractor

```rust
pub struct SessionMemoryExtractor {
    config: SessionMemoryConfig,
    last_extracted_message_id: Option<MessageId>,
}

#[derive(Debug, Clone)]
pub struct SessionMemoryConfig {
    pub initialization_threshold: u32,
    pub update_threshold: u32,
    pub extraction_model: ModelAlias,
}

impl SessionMemoryExtractor {
    pub fn should_extract(&self, conversation: &SessionState) -> bool {
        let tool_calls_since = self.count_tool_calls_since(&conversation.history, self.last_extracted_message_id.as_ref());
        if !self.is_initialized() { tool_calls_since >= self.config.initialization_threshold }
        else { tool_calls_since >= self.config.update_threshold }
    }
    pub async fn extract(&mut self, conversation: &SessionState) -> Result<String, MemoryError> {
        // Effect::ForkAgent — extract via forked agent
        ...
    }
}
```

### 6.6 Team Memory Sync

```rust
pub struct TeamMemoryWatcher {
    team_dir: PathBuf,
    secret_scanner: SecretScanner,
}

impl TeamMemoryWatcher {
    pub async fn start(&self, fs: &dyn FileSystem) -> Result<TeamMemoryHandle, MemoryError> {
        let stream = fs.watch(&self.team_dir).await?;
        // Listen for write events → secret scan → notify main conversation
        ...
    }
}

pub struct SecretScanner {
    patterns: Vec<regex::Regex>,  // API keys, tokens, etc.
}
```

---

## 7. MCP Lifecycle

### 7.1 Transport Spec (7 kinds)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum McpTransportSpec {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    Sse { url: String, headers: HashMap<String, String>, headers_helper: Option<String>, oauth: Option<McpOAuthConfig> },
    Http { url: String, headers: HashMap<String, String>, oauth: Option<McpOAuthConfig> },
    WebSocket { url: String, headers: HashMap<String, String> },
    InProcess { registry_key: String },
    SseIde { url: String, ide_name: String, ide_running_in_windows: bool },
    SdkControl { control_channel_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigScope {
    Local, User, Project, Dynamic, Enterprise, ClaudeAi, Managed,
}
```

### 7.2 Connection State Machine (per server)

```rust
#[derive(Debug, Clone)]
pub enum McpConnectionState {
    Disconnected { config: McpServerConfig, last_error: Option<String> },
    Connecting { config: McpServerConfig, started_at: SystemTime },
    AwaitingOAuth { config: McpServerConfig, oauth_state: OAuthState, callback_port: u16 },
    Connected {
        config: McpServerConfig,
        connection_id: McpConnectionId,
        capabilities: ServerCapabilities,
        tools: Vec<McpTool>,
        resources: Vec<McpResource>,
        prompts: Vec<McpPrompt>,
        connected_at: SystemTime,
    },
    HealthChecking { /* ... */ },
    Reconnecting { config: McpServerConfig, retry_count: u32, next_retry_at: SystemTime },
    Failed { config: McpServerConfig, error: String, attempts: u32 },
    Stopped { config: McpServerConfig },
}
```

### 7.3 ServerCapabilities & McpTool

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub tools: Option<ToolCapabilities>,
    pub resources: Option<ResourceCapabilities>,
    pub prompts: Option<PromptCapabilities>,
    pub logging: Option<LoggingCapabilities>,
    pub experimental: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub server_name: String,
    pub tool_name: String,
    pub description: String,
    pub input_schema: Value,
    pub full_name: String,  // "mcp__<server>__<tool>"
}
```

### 7.4 McpRegistry (Central Manager)

```rust
pub struct McpRegistry {
    connections: HashMap<String, McpConnectionState>,
    agent_scoped: HashMap<AgentId, HashMap<String, McpConnectionId>>,
    transport: Arc<dyn McpTransport>,
    oauth_handler: Arc<OAuthHandler>,
    health_check_interval: Duration,
    max_retry_count: u32,
}

impl McpRegistry {
    pub async fn connect(&mut self, name: &str, config: &McpServerConfig) -> Result<McpConnectionId, McpError> {
        // 1. Reuse if Connected
        // 2. State: Disconnected → Connecting
        // 3. Transport connect
        // 4. OAuth flow if needed
        // 5. MCP handshake: initialize → list_tools/resources/prompts
        // 6. State: Connecting → Connected
        ...
    }
    pub async fn call_tool(&self, connection_id: McpConnectionId, tool_name: &str, input: Value) -> Result<McpToolResult, McpError> { ... }
    pub async fn health_check_all(&mut self) { ... }
    pub async fn connect_for_agent(&mut self, agent_id: AgentId, servers: Vec<McpServerConfig>) -> Result<Vec<McpConnectionId>, McpError> { ... }
    pub async fn cleanup_for_agent(&mut self, agent_id: AgentId) { ... }
}
```

### 7.5 OAuth Flow

```rust
#[derive(Debug, Clone)]
pub enum OAuthState {
    Initiated { callback_port: u16, code_verifier: String, state_token: String },
    AwaitingCallback { callback_port: u16, code_verifier: String, state_token: String, auth_url: String },
    ExchangingCode { code: String },
    Authenticated { access_token: String, refresh_token: Option<String>, expires_at: SystemTime },
    Refreshing { refresh_token: String },
}
```

### 7.6 Approval Policy

```rust
pub struct McpApprovalPolicy {
    pub project_servers_require_approval: bool,
    pub approved: HashSet<String>,
    pub rejected: HashSet<String>,
    pub enterprise_restrictions: Option<EnterprisePolicy>,
}

impl McpApprovalPolicy {
    pub fn is_approved(&self, name: &str, scope: ConfigScope) -> ApprovalStatus {
        if self.rejected.contains(name) { return ApprovalStatus::Rejected; }
        if self.approved.contains(name) { return ApprovalStatus::Approved; }
        match scope {
            ConfigScope::Local | ConfigScope::User | ConfigScope::Enterprise | ConfigScope::Managed => ApprovalStatus::Approved,
            ConfigScope::Project => ApprovalStatus::PendingApproval,
            _ => ApprovalStatus::PendingApproval,
        }
    }
}
```

---

## 8. Tools System

### 8.1 Tool Trait (30+ methods)

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn aliases(&self) -> &[&str] { &[] }
    fn search_hint(&self) -> Option<&str> { None }

    // Schema
    fn input_schema(&self) -> &Value;
    fn output_schema(&self) -> Option<&Value> { None }

    // Static properties
    fn is_enabled(&self, ctx: &ToolStaticContext) -> bool;
    fn is_mcp(&self) -> bool { false }
    fn is_lsp(&self) -> bool { false }
    fn requires_user_interaction(&self) -> bool { false }
    fn should_defer(&self) -> bool { false }
    fn always_load(&self) -> bool { false }
    fn strict(&self) -> bool { false }
    fn max_result_size_chars(&self) -> usize;

    // Dynamic (input-dependent) properties
    fn is_concurrency_safe(&self, input: &Value) -> bool;
    fn is_read_only(&self, input: &Value) -> bool;
    fn is_destructive(&self, input: &Value) -> bool { false }
    fn is_open_world(&self, input: &Value) -> bool { false }
    fn is_search_or_read(&self, input: &Value) -> Option<SearchReadInfo> { None }
    fn interrupt_behavior(&self, input: &Value) -> InterruptBehavior { InterruptBehavior::Block }

    // Input handling
    fn backfill_observable_input(&self, input: &mut Value) {}
    async fn validate_input(&self, input: &Value, ctx: &ToolUseContext) -> Result<(), ValidationError> { Ok(()) }
    async fn prepare_permission_matcher(&self, input: &Value) -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> { None }

    // Permission
    async fn check_permissions(&self, input: &Value, ctx: &ToolUseContext) -> PermissionResult;
    fn get_path(&self, input: &Value) -> Option<PathBuf> { None }

    // Prompt generation
    async fn description(&self, input: &Value, opts: &DescriptionOptions) -> String;
    async fn prompt(&self, opts: &PromptOptions) -> String;

    // Core execution
    async fn call(&self, input: Value, ctx: ToolUseContext, progress_tx: ToolProgressSender)
        -> Result<ToolCallResult, ToolError>;

    fn get_activity_description(&self, input: &Value) -> Option<String> { None }
}
```

### 8.2 ToolUseContext

```rust
#[derive(Clone)]
pub struct ToolUseContext {
    pub options: ToolUseOptions,
    pub abort_signal: AbortSignal,
    pub file_state_cache: Arc<FileStateCache>,
    pub messages: Vec<ConversationMessage>,
    pub tool_use_id: Option<String>,
    pub agent_id: Option<AgentId>,
    pub agent_type: Option<String>,
    pub query_tracking: Option<QueryChainTracking>,
    pub file_reading_limits: Option<FileReadingLimits>,
    pub glob_limits: Option<GlobLimits>,
    pub tool_decisions: Arc<Mutex<HashMap<String, ToolDecision>>>,
    pub rendered_system_prompt: Option<Arc<str>>,
    pub content_replacement_state: Option<Arc<Mutex<ContentReplacementState>>>,
    pub effect_tx: EffectSender,
    // Callbacks
    pub set_tool_jsx: Option<Box<dyn Fn(Option<ToolJsx>) + Send + Sync>>,
    pub send_os_notification: Option<Box<dyn Fn(OsNotification) + Send + Sync>>,
    pub handle_elicitation: Option<Box<dyn Fn(ElicitRequest) -> ElicitResult + Send + Sync>>,
}

#[derive(Clone)]
pub struct ToolUseOptions {
    pub debug: bool,
    pub verbose: bool,
    pub main_loop_model: String,
    pub tools: Vec<Arc<dyn Tool>>,
    pub thinking_config: ThinkingConfig,
    pub mcp_clients: Vec<McpConnectionId>,
    pub is_non_interactive_session: bool,
    pub agent_definitions: Vec<AgentDefinition>,
    pub custom_system_prompt: Option<String>,
    pub append_system_prompt: Option<String>,
    pub max_budget_nano_usd: Option<u64>,
    pub refresh_tools: Option<Arc<dyn Fn() -> Vec<Arc<dyn Tool>> + Send + Sync>>,
}
```

### 8.3 ToolCallResult & InterruptBehavior

```rust
pub struct ToolCallResult {
    pub data: Value,
    pub new_messages: Vec<ConversationMessage>,
    pub context_modifier: Option<Box<dyn FnOnce(ToolUseContext) -> ToolUseContext + Send>>,
    pub mcp_meta: Option<McpMeta>,
}

#[derive(Debug, Clone)]
pub enum InterruptBehavior { Cancel, Block }
```

### 8.4 Tool Registry

```rust
pub struct ToolRegistry {
    builtin: Vec<Arc<dyn Tool>>,
    mcp_tools: HashMap<McpConnectionId, Vec<Arc<dyn Tool>>>,
    lsp_tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn available_tools(&self, ctx: &ToolStaticContext) -> Vec<Arc<dyn Tool>> { ... }
    pub fn find_by_name(&self, name: &str) -> Option<Arc<dyn Tool>> { ... }
    pub fn register_mcp_tools(&mut self, conn_id: McpConnectionId, tools: Vec<Arc<dyn Tool>>) { ... }
    pub fn unregister_mcp_tools(&mut self, conn_id: McpConnectionId) { ... }
}
```

### 8.5 Tool Dispatcher (Concurrency Partition)

```rust
pub struct ToolDispatcher {
    registry: Arc<ToolRegistry>,
    max_concurrency: usize,  // default 10 (CLAUDE_CODE_MAX_TOOL_USE_CONCURRENCY)
    permission_enforcer: Arc<PermissionEnforcer>,
    hook_executor: Arc<HookExecutor>,
}

impl ToolDispatcher {
    /// Read-only/concurrency-safe → parallel; write → serial
    pub async fn run_tools(
        &self,
        tool_calls: Vec<ToolCall>,
        assistant_messages: Vec<AssistantMessage>,
        mut ctx: ToolUseContext,
    ) -> impl Stream<Item = ToolDispatchEvent> {
        let partitions = self.partition_tool_calls(&tool_calls, &ctx);
        async_stream::stream! {
            for partition in partitions {
                if partition.is_concurrency_safe {
                    // Parallel execution with FuturesUnordered
                    ...
                } else {
                    // Serial execution
                    ...
                }
            }
        }
    }
}
```

### 8.6 Single-Tool Execution Lifecycle

Per-tool execution flow:
1. `backfill_observable_input`
2. `validate_input`
3. **PreToolUse hook** → may block/modify input/auto-approve
4. `check_permissions` (tool + global PermissionEnforcer)
5. Permission gate (request UI / auto-deny / approved)
6. Execute via `tool.call()` with progress channel
7. **PostToolUse** or **PostToolUseFailure hook**

### 8.7 StreamingToolExecutor

```rust
/// Tools begin executing on `content_block_start: tool_use` in the SSE stream,
/// before the full assistant message is received.
pub struct StreamingToolExecutor {
    dispatcher: Arc<ToolDispatcher>,
    pending_calls: Mutex<HashMap<ToolUseId, PendingToolCall>>,
}

impl StreamingToolExecutor {
    pub async fn on_tool_use_start(&self, tool_use_id: ToolUseId, tool_name: String) { ... }
    pub async fn on_tool_use_complete(&self, tool_use_id: ToolUseId, input: Value) { ... }
}
```

### 8.8 Result Storage & Content Replacement

```rust
pub struct ToolResultStorage {
    storage_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
}

impl ToolResultStorage {
    pub async fn store(&self, tool_use_id: &str, full_result: &str) -> Result<StoredResultRef, Error> { ... }
}

#[derive(Debug, Clone, Default)]
pub struct ContentReplacementState {
    pub replacements: HashMap<ToolUseId, ReplacementRecord>,
    pub total_budget_chars: usize,
    pub used_chars: usize,
}

#[derive(Debug, Clone)]
pub struct ReplacementRecord {
    pub original_size: usize,
    pub replaced_at_turn: u32,
    pub placeholder: String,  // "[Old tool result content cleared]"
}
```

---

## 9. Hooks System

### 9.1 28 Hook Events

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HookEvent {
    // Tool lifecycle
    PreToolUse { tool_name: String, tool_input: Value, tool_use_id: ToolUseId },
    PostToolUse { tool_name: String, tool_input: Value, tool_output: Value, tool_use_id: ToolUseId },
    PostToolUseFailure { tool_name: String, error: String, tool_use_id: ToolUseId },

    // Session lifecycle
    SessionStart { session_id: SessionId, source: SessionStartSource },
    SessionEnd { session_id: SessionId, reason: SessionEndReason },
    Setup,

    // User input
    UserPromptSubmit { prompt: String },

    // Stop hooks
    Stop { reason: StopReason },
    StopFailure { error: String },

    // Subagent lifecycle
    SubagentStart { agent_id: AgentId, agent_type: String, parent_agent_id: Option<AgentId> },
    SubagentStop { agent_id: AgentId, status: SubagentStatus },

    // Compaction lifecycle
    PreCompact { reason: CompactionReason },
    PostCompact { summary: String, tokens_freed: u64 },

    // Permission
    PermissionRequest { tool_name: String, tool_input: Value, reason: String },
    PermissionDenied { tool_name: String, reason: String },

    // Teammate / Task
    TeammateIdle { agent_id: AgentId },
    TaskCreated { task_id: String, task_type: String, description: String },
    TaskCompleted { task_id: String, status: TaskStatus },

    // MCP
    Elicitation { server_name: String, params: ElicitParams },
    ElicitationResult { server_name: String, result: ElicitResult },

    // Config / Workspace
    ConfigChange { changes: Vec<ConfigDiff> },
    WorktreeCreate { path: PathBuf, branch: String },
    WorktreeRemove { path: PathBuf },
    InstructionsLoaded { paths: Vec<PathBuf> },
    CwdChanged { old: PathBuf, new: PathBuf },
    FileChanged { path: PathBuf, kind: FileEventKind },

    // User notification
    Notification { message: String, kind: NotificationKind },
}
```

### 9.2 HookDefinition (4 Executor Kinds)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookDefinition {
    pub id: HookId,
    pub name: String,
    pub events: Vec<HookEventType>,
    pub if_condition: Option<HookCondition>,
    pub executor: HookExecutor,
    pub source: HookSource,
    pub blocking: bool,
    pub timeout: Option<Duration>,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HookExecutor {
    Command { command: String, args: Vec<String>, env: HashMap<String, String>, cwd: Option<PathBuf> },
    Http { url: String, method: HttpMethod, headers: HashMap<String, String>, timeout: Duration },
    Agent { agent_type: String, prompt: String },
    Builtin { handler_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookCondition {
    pub pattern: String,           // "Bash(git *)" style
    pub match_tool_name: bool,
    pub match_input: bool,         // via tool.prepare_permission_matcher
}
```

### 9.3 Hook Output Protocol (Key Innovation)

```rust
/// Hook's stdout JSON parsed as HookResponse → can affect main flow
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResult {
    pub outcome: HookOutcome,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub response: Option<HookResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookOutcome { Success, Error, Cancelled, Timeout }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookResponse {
    pub decision: Option<HookDecision>,
    pub reason: Option<String>,
    pub updated_input: Option<Value>,        // PreToolUse can rewrite tool input
    pub system_message: Option<String>,      // Inject into conversation
    pub attachments: Vec<HookAttachment>,
    pub suppress_output: bool,
    pub structured_content: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum HookDecision { Allow, Approve, Block, Continue }
```

### 9.4 Multi-Source Hook Registry

```rust
pub struct HookRegistry {
    user: Vec<HookDefinition>,           // ~/.claude/settings.json
    project: Vec<HookDefinition>,        // .claude/settings.json
    managed: Vec<HookDefinition>,        // Enterprise policy
    plugin: HashMap<PluginId, Vec<HookDefinition>>,
    frontmatter: HashMap<AgentId, Vec<HookDefinition>>,  // agent .md files
    session: Vec<HookDefinition>,        // runtime-registered
    skill: HashMap<String, Vec<HookDefinition>>,
}

impl HookRegistry {
    pub fn match_event(&self, event: &HookEvent, ctx: &HookContext) -> Vec<&HookDefinition> {
        // Match by event type, condition pattern, then sort by priority
        ...
    }
}
```

### 9.5 HookExecutor

```rust
pub struct HookExecutor {
    registry: Arc<RwLock<HookRegistry>>,
    process: Arc<dyn ProcessRunner>,
    http: Arc<dyn HttpTransport>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinHookHandler>>,
    async_registry: AsyncHookRegistry,
    event_handler: Option<Arc<dyn HookEventHandler>>,
    ssrf_guard: SsrfGuard,
}

impl HookExecutor {
    pub async fn execute(&self, event: HookEvent) -> AggregateHookResult {
        // Blocking hooks: serial execution, can short-circuit on Block
        // Non-blocking hooks: spawn to async_registry
        ...
    }
}

#[derive(Debug, Clone, Default)]
pub struct AggregateHookResult {
    pub decision: Option<HookDecision>,
    pub reason: Option<String>,
    pub modified_input: Option<Value>,
    pub system_messages: Vec<String>,
    pub attachments: Vec<HookAttachment>,
    pub all_results: Vec<(HookId, HookResult)>,
}
```

### 9.6 AsyncHookRegistry

```rust
pub struct AsyncHookRegistry {
    runtime: Arc<dyn RuntimeSpawner>,
    in_flight: Arc<Mutex<HashMap<HookId, BackgroundTaskHandle>>>,
    completion_tx: mpsc::Sender<(HookId, HookResult)>,
}

impl AsyncHookRegistry {
    pub async fn spawn(&self, hook: HookDefinition, event: HookEvent, ctx: HookContext) -> Result<(), RuntimeError> {
        let completion_tx = self.completion_tx.clone();
        let hook_id = hook.id;
        let handle = self.runtime.spawn("hook-async", Box::pin(async move {
            let result = run_hook(hook.clone(), event, ctx).await;
            let _ = completion_tx.send((hook_id, result)).await;
        })).await?;
        self.in_flight.lock().insert(hook_id, handle);
        Ok(())
    }
    pub async fn drain(&self, timeout: Duration) { ... }
}
```

### 9.7 SSRF Guard

```rust
pub struct SsrfGuard {
    allowed_schemes: HashSet<String>,
    blocked_cidrs: Vec<IpRange>,
    allowed_hosts: Option<HashSet<String>>,
}

impl SsrfGuard {
    pub fn check(&self, url: &str) -> Result<(), SsrfError> {
        // 1. Scheme check
        // 2. DNS resolve + IP CIDR check
        // 3. Optional host allowlist
        ...
    }
}
```

### 9.8 Builtin Hook Handlers

```rust
#[async_trait]
pub trait BuiltinHookHandler: Send + Sync {
    async fn handle(&self, event: &HookEvent, ctx: &HookContext) -> HookResult;
    fn id(&self) -> &str;
}

// Examples:
// - SkillImprovementHandler: analyze PostToolUse for skill creation opportunities
// - CompactWarningHandler: warn near token limit
// - SessionActivityHandler: track session usage
```

---

## 10. Agent / Subagent

### 10.1 Cross-System Integration Table

| Subsystem | Parent → Child Strategy | Agent Can Override | Isolation/Sharing |
|---|---|---|---|
| **Tools (§8)** | Inherit parent's tool pool (`use_exact_tools` byte-exact for prompt cache) | ✅ `tools: [..]` whitelist/blacklist/all | byte-exact share or rebuild |
| **MCP (§7)** | Share parent MCP connections (memoized) | ✅ `mcpServers` field — additive | shared `ByName`; per-agent `Inline` cleanup on exit |
| **ToolUseContext (§8.2)** | Derived via `create_subagent_context()` | ✅ All fields | `setAppState` no-op for async; `setAppStateForTasks` always reaches root |
| **Hooks (§9)** | Inherit user/project/managed/plugin hooks | ✅ frontmatter `hooks:` adds session-scoped | `clearSessionHooks(agentId)` on exit |
| **Memory (§6)** | Inherit static memory + selector | ✅ `memory_snapshot` filter | `AgentMemorySnapshot` independent view; team memory still shared |
| **Compaction (§13)** | Subagent runs own compaction loop | ✅ permission_mode/max_turns affect frequency | Independent `AutoCompactTrackingState` |
| **Permission (§8.5)** | Inherit rules; `allowed_tools` **replaces** (not merges) | ✅ `permission_mode: bubble/isolated/auto/plan` | Per-agent enforcement |
| **FileStateCache** | `cloneFileStateCache(parent)` — copy | ✅ N/A | Copy isolated |
| **Worktree** | Not inherited (each isolated agent → own worktree) | ✅ `worktree: required/optional/none` | See §10.7 degradation |
| **Telemetry** | Inherit chainId, depth+1 | N/A | Parent-child link via QueryChainTracking |
| **transcripts** | Sidechain JSONL (isolated) | N/A | Parent doesn't read directly |
| **API client** | Share HttpTransport, independent prompt cache | ✅ `model` field including `inherit` | forked agent `model: inherit` for cache hit |

Plugin-provided agents are a narrower trust surface than user/project agents:
their frontmatter `permission_mode`, `hooks`, and `mcpServers` fields are ignored.
Those privileges must be granted through the plugin manifest at install/enable time,
then materialized by PluginManager (§15), so a third-party agent file cannot silently
expand permissions after review.

### 10.2 AgentDefinition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub agent_type: String,
    pub when_to_use: String,
    pub tools: AgentToolPolicy,
    pub max_turns: u32,
    pub model: AgentModel,
    pub permission_mode: AgentPermissionMode,
    pub source: AgentSource,
    pub base_dir: PathBuf,
    pub system_prompt: Option<String>,
    pub mcp_servers: Vec<AgentMcpServerSpec>,
    pub frontmatter_hooks: Vec<HookSpec>,
    pub memory_filter: Option<AgentMemoryFilter>,
    pub worktree_requirement: Option<WorktreeRequirement>,
    pub icon: Option<String>,
    pub allowed_tools: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentToolPolicy {
    All { use_exact_tools: bool },
    Explicit(Vec<String>),
    Except(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentModel { Inherit, Alias(ModelAlias), Explicit(String) }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentPermissionMode { Bubble, Isolated, Auto, Plan }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentSource { BuiltIn, UserDefined, Project, Plugin, PolicySettings }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentMcpServerSpec {
    ByName(String),
    Inline { name: String, config: McpServerConfig },
}
```

### 10.3 SubagentContext

```rust
#[derive(Debug, Clone)]
pub struct SubagentContext {
    pub agent_id: AgentId,
    pub parent_agent_id: Option<AgentId>,
    pub agent_definition: AgentDefinition,
    pub prompt_messages: Vec<ConversationMessage>,
    pub fork_context_messages: Option<Vec<ConversationMessage>>,
    pub available_tools: Vec<Arc<dyn Tool>>,
    pub allowed_tools: Vec<String>,
    pub worktree_path: Option<PathBuf>,
    pub is_async: bool,
    pub can_show_permission_prompts: bool,
    pub abort_signal: AbortSignal,
    pub file_state_cache: FileStateCache,
    pub rendered_system_prompt: Option<Arc<str>>,
    pub mcp_clients: Vec<McpConnectionId>,
    pub transcript_subdir: PathBuf,
    pub permission_context: PermissionContext,
    pub query_tracking: Option<QueryChainTracking>,
    pub content_replacement_state: Option<Arc<Mutex<ContentReplacementState>>>,
    pub agent_memory: Option<AgentMemorySnapshot>,
    pub display: AgentDisplay,
    pub mailbox: Arc<TeammateMailbox>,  // §12 integration
}
```

### 10.4 Cross-System Subagent Context Builder

```rust
/// Single function that performs all subsystem inheritance/isolation decisions
pub async fn create_subagent_context(
    parent_ctx: &ToolUseContext,
    agent_def: &AgentDefinition,
    spawn_input: &AgentSpawnInput,
    runtime: &SubagentRuntime,
) -> Result<SubagentContext, AgentError> {
    let agent_id = AgentId::new();
    let parent_id = parent_ctx.agent_id.clone();

    // (1) Tools - subject to AgentToolPolicy + permission_mode
    let agent_mcp_tools = runtime.connect_agent_mcp_servers(&agent_def.mcp_servers, &agent_id).await?;
    let resolved_tools = AgentToolResolver::resolve(agent_def, &parent_ctx.options.tools, &agent_mcp_tools.tools, runtime.coordinator_mode.is_enabled());

    // (2) MCP - parent shared + agent inline
    let mcp_clients = [parent_ctx.options.mcp_clients.clone(), agent_mcp_tools.connections].concat();

    // (3) Hooks - register frontmatter hooks to agent scope
    runtime.hook_registry.register_frontmatter(agent_id.clone(), agent_def.frontmatter_hooks.clone());

    // (4) Memory snapshot
    let agent_memory = if let Some(filter) = &agent_def.memory_filter {
        Some(AgentMemorySnapshot::build_for_agent(agent_def, &runtime.memory_dir, &*runtime.fs).await?)
    } else { None };

    // (5) Worktree - platform degradation
    let worktree = runtime.worktree.create_worktree_or_degrade(
        agent_def.worktree_requirement(),
        &agent_id.to_slug(),
    ).await?;

    // (6) File state cache - clone
    let file_state_cache = clone_file_state_cache(&parent_ctx.file_state_cache, READ_FILE_STATE_CACHE_SIZE);

    // (7) Permission context
    let permission_ctx = build_permission_context_for_agent(
        agent_def.permission_mode, &agent_def.allowed_tools, &parent_ctx.tool_permission_context,
    );

    // (8) Telemetry chain
    let query_tracking = QueryChainTracking {
        chain_id: parent_ctx.query_tracking.as_ref().map(|t| t.chain_id.clone()).unwrap_or_else(uuid),
        depth: parent_ctx.query_tracking.as_ref().map(|t| t.depth + 1).unwrap_or(0),
    };

    // (9) System prompt - fork keeps byte-exact for cache hit
    let rendered_system_prompt = if agent_def.is_fork() {
        parent_ctx.rendered_system_prompt.clone()
    } else {
        Some(render_system_prompt_for_agent(agent_def, runtime).await?)
    };

    // (10) Content replacement - fork shares, else new
    let content_replacement = if agent_def.is_fork() {
        parent_ctx.content_replacement_state.clone()
    } else {
        Some(Arc::new(Mutex::new(ContentReplacementState::default())))
    };

    // (11) Display
    let display = AgentDisplay {
        color: runtime.color_manager.assign_color(&agent_id),
        spinner_mode: SpinnerMode::AgentRunning,
        icon: agent_def.icon.clone(),
    };

    Ok(SubagentContext {
        agent_id: agent_id.clone(),
        parent_agent_id: parent_id,
        agent_definition: agent_def.clone(),
        prompt_messages: spawn_input.prompt_messages.clone(),
        fork_context_messages: spawn_input.fork_context_messages.clone(),
        available_tools: resolved_tools,
        allowed_tools: agent_def.allowed_tools.clone(),
        worktree_path: worktree.as_ref().map(|w| w.path.clone()),
        is_async: spawn_input.is_async,
        can_show_permission_prompts: spawn_input.can_show_permission_prompts.unwrap_or(!spawn_input.is_async),
        abort_signal: spawn_input.abort_controller.clone().unwrap_or_else(AbortSignal::new),
        file_state_cache,
        rendered_system_prompt,
        mcp_clients,
        transcript_subdir: runtime.session.allocate_transcript_subdir(&agent_id),
        permission_context: permission_ctx,
        query_tracking: Some(query_tracking),
        content_replacement_state: content_replacement,
        agent_memory,
        display,
        mailbox: Arc::new(TeammateMailbox::new(agent_id.clone())),
    })
}
```

### 10.5 State Machine Pool (Effect Delegation)

**Key architectural decision**: subagents are NOT nested SM instances. They are sibling slots in a host-managed pool.

```rust
pub struct StateMachinePool {
    slots: Arc<RwLock<HashMap<AgentId, StateMachineSlot>>>,
    runtime: Arc<dyn RuntimeSpawner>,
    max_concurrent: usize,
}

pub struct StateMachineSlot {
    pub agent_id: AgentId,
    pub state: ConversationState,
    pub event_tx: EventSender,
    pub event_rx: EventReceiver,
    pub task: BackgroundTaskHandle,
}

impl StateMachinePool {
    pub async fn allocate(&self, ctx: SubagentContext) -> Result<(AgentId, EventReceiver), Error> {
        let mut slots = self.slots.write().await;
        if slots.len() >= self.max_concurrent { return Err(Error::TooManyAgents); }
        let agent_id = ctx.agent_id.clone();
        let (event_tx, event_rx) = mpsc::channel(100);
        let (caller_tx, caller_rx) = mpsc::channel(100);
        let task = self.runtime.spawn(
            "subagent-state-machine",
            Box::pin(run_conversation_state_machine(ctx, event_tx, caller_tx)),
        ).await?;
        slots.insert(agent_id.clone(), StateMachineSlot {
            agent_id: agent_id.clone(),
            state: ConversationState::initial(),
            event_tx: event_tx.clone(),
            event_rx,
            task,
        });
        Ok((agent_id, caller_rx))
    }
    pub async fn deallocate(&self, agent_id: &AgentId) { ... }
}
```

**Rationale for effect delegation over nested SM**:
- Stack space: 5-deep fork chain × SM size avoided
- Concurrency: multi-dispatch creates siblings in same scheduler
- Cancellation: parent abort directly deallocates all child slots
- Observability: unified metrics collection

### 10.6 Multi-Agent Dispatch

```rust
pub struct MultiAgentDispatcher {
    pool: Arc<StateMachinePool>,
    team_registry: Arc<TeamRegistry>,
    mailbox_router: Arc<MailboxRouter>,
    runtime: Arc<dyn RuntimeSpawner>,
}

impl MultiAgentDispatcher {
    pub async fn spawn_multi(
        &self,
        specs: Vec<MultiAgentSpawnSpec>,
        coordinator_id: AgentId,
    ) -> Result<Vec<AgentId>, AgentError> {
        let mut agent_ids = Vec::new();
        for spec in specs {
            let agent_id = self.team_registry.spawn_worker(/* ... */).await?;
            let ctx = create_subagent_context_for_worker(spec, &agent_id).await?;
            let (_, event_rx) = self.pool.allocate(ctx).await?;
            // Route worker events to coordinator's mailbox
            let coordinator_id_clone = coordinator_id.clone();
            let router = self.mailbox_router.clone();
            self.runtime.spawn("coordinator-mailbox-route", Box::pin(async move {
                pin_mut!(event_rx);
                while let Some(event) = event_rx.next().await {
                    if let Some(msg) = event_to_teammate_message(&agent_id, &event) {
                        let _ = router.route(&coordinator_id_clone, msg).await;
                    }
                }
            })).await?;
            agent_ids.push(agent_id);
        }
        Ok(agent_ids)
    }
}

#[derive(Debug, Clone)]
pub struct MultiAgentSpawnSpec {
    pub agent_type: String,
    pub name: String,
    pub initial_prompt: String,
    pub agent_def_override: Option<AgentDefinition>,
}
```

### 10.7 Worktree Multi-Platform Degradation

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorktreeRequirement {
    Required,    // platform must support, else error
    Optional,    // degrade to in-place if unsupported
    None,        // never create
}

impl WorktreeManager {
    pub async fn create_worktree_or_degrade(
        &self,
        requirement: WorktreeRequirement,
        slug: &str,
    ) -> Result<Option<WorktreeHandle>, WorktreeError> {
        match (requirement, self.is_supported()) {
            (WorktreeRequirement::Required, false) => Err(WorktreeError::Unsupported),
            (WorktreeRequirement::Required, true) => Ok(Some(self.create_worktree(slug, None, &[]).await?)),
            (WorktreeRequirement::Optional, true) => {
                self.create_worktree(slug, None, &[]).await.ok().map(Some).ok_or(WorktreeError::Degraded)
                    .or(Ok(None))  // graceful fall-through
            }
            (WorktreeRequirement::Optional, false) | (WorktreeRequirement::None, _) => Ok(None),
        }
    }
}
```

### 10.8 Agent Tool Resolver

```rust
pub struct AgentToolResolver;

impl AgentToolResolver {
    pub fn resolve(
        agent_def: &AgentDefinition,
        parent_tools: &[Arc<dyn Tool>],
        agent_mcp_tools: &[Arc<dyn Tool>],
        coordinator_mode: bool,
    ) -> Vec<Arc<dyn Tool>> {
        let mut tools = match &agent_def.tools {
            AgentToolPolicy::All { use_exact_tools } => {
                if *use_exact_tools { parent_tools.to_vec() }
                else { rebuild_tools_for_mode(parent_tools, agent_def.permission_mode) }
            }
            AgentToolPolicy::Explicit(names) => parent_tools.iter().filter(|t| names.contains(&t.name().to_string())).cloned().collect(),
            AgentToolPolicy::Except(names) => parent_tools.iter().filter(|t| !names.contains(&t.name().to_string())).cloned().collect(),
        };
        tools.extend(agent_mcp_tools.iter().cloned());
        if coordinator_mode { tools.extend(coordinator_internal_tools()); }
        // Plan mode filter
        if agent_def.permission_mode == AgentPermissionMode::Plan {
            tools.retain(|t| matches!(t.name(), "Read" | "Grep" | "Glob" | "WebSearch" | "WebFetch"));
            tools.push(Arc::new(ExitPlanModeTool::new()));
        }
        tools
    }
}
```

### 10.9 Agent Memory Snapshot

```rust
pub struct AgentMemorySnapshot {
    pub snapshot_id: SnapshotId,
    pub agent_type: String,
    pub included_files: Vec<MemoryFile>,
    pub created_at: SystemTime,
    pub session_id: Option<SessionId>,
    pub frozen: bool,
}

impl AgentMemorySnapshot {
    pub async fn build_for_agent(
        agent_def: &AgentDefinition,
        memory_dir: &Path,
        fs: &dyn FileSystem,
    ) -> Result<Self, MemoryError> {
        let included = if let Some(filter) = &agent_def.memory_filter {
            let all = scan_memory_files(memory_dir, fs).await?;
            all.into_iter().filter(|m| filter.matches(&m.frontmatter)).collect()
        } else { vec![] };
        Ok(AgentMemorySnapshot { snapshot_id: SnapshotId::new(), agent_type: agent_def.agent_type.clone(), included_files: included, created_at: SystemTime::now(), session_id: None, frozen: false })
    }
    pub fn to_prompt_section(&self) -> String { ... }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMemoryFilter {
    pub include_tags: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub memory_types: Vec<String>,
}
```

### 10.10 Agent Color Manager

```rust
pub struct AgentColorManager {
    available_colors: Vec<AgentColor>,
    assigned: Arc<RwLock<HashMap<AgentId, AgentColor>>>,
    recent: Arc<Mutex<VecDeque<AgentColor>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentColor {
    Cyan, Magenta, Yellow, Green, Blue, Red, Orange, Purple, Pink, Teal,
}

impl AgentColorManager {
    pub fn assign_color(&self, agent_id: &AgentId) -> AgentColor {
        // Avoid recent neighbors
        ...
    }
    pub fn release(&self, agent_id: &AgentId) { ... }
}
```

### 10.11 Fork Subagent

```rust
pub struct ForkSpawner {
    parent_rendered_system_prompt: String,
    parent_messages: Vec<ConversationMessage>,
    parent_tool_pool: Vec<ToolSpec>,
    fork_directive: String,
}

impl ForkSpawner {
    pub fn is_in_fork_child(messages: &[ConversationMessage]) -> bool {
        messages.iter().any(|m| m.content_includes(FORK_BOILERPLATE_TAG))
    }
    pub fn spawn(self) -> SubagentContext {
        // Use FORK_AGENT definition: model: Inherit, tools: "*", permission_mode: Bubble
        // rendered_system_prompt: parent bytes (byte-exact cache hit)
        ...
    }
}

pub const FORK_BOILERPLATE_TAG: &str = "<fork-boilerplate>";
pub const FORK_DIRECTIVE_PREFIX: &str = "<fork-directive>";
```

### 10.12 Transcript Writer

```rust
pub struct AgentTranscriptWriter {
    transcript_path: PathBuf,
    agent_id: AgentId,
}

impl AgentTranscriptWriter {
    pub async fn record(&self, message: &ConversationMessage, fs: &dyn FileSystem) -> Result<(), Error> {
        let json_line = serde_json::to_string(&TranscriptEntry {
            agent_id: self.agent_id,
            timestamp: SystemTime::now(),
            message: message.clone(),
        })?;
        fs.append_file(&self.transcript_path, &format!("{}\n", json_line)).await
    }
}

pub struct AgentResumer;
impl AgentResumer {
    pub async fn load_sidechain(path: &Path, fs: &dyn FileSystem) -> Result<Vec<ConversationMessage>, Error> { ... }
}
```

---

## 11. Task Manager

### 11.1 TaskType (7 variants)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TaskType {
    LocalBash, LocalAgent, RemoteAgent, InProcessTeammate,
    LocalWorkflow, MonitorMcp, Dream,
}

impl TaskType {
    pub fn id_prefix(self) -> char {
        match self {
            TaskType::LocalBash => 'b', TaskType::LocalAgent => 'a',
            TaskType::RemoteAgent => 'r', TaskType::InProcessTeammate => 't',
            TaskType::LocalWorkflow => 'w', TaskType::MonitorMcp => 'm',
            TaskType::Dream => 'd',
        }
    }
}

pub fn generate_task_id(task_type: TaskType) -> String {
    let prefix = task_type.id_prefix();
    let bytes = rand::random::<[u8; 8]>();
    let suffix: String = bytes.iter().map(|b| TASK_ID_ALPHABET[(*b as usize) % 36] as char).collect();
    format!("{}{}", prefix, suffix)
}

const TASK_ID_ALPHABET: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
// 36^8 ≈ 2.8 trillion combinations
```

### 11.2 TaskStatus & TaskState

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus { Pending, Running, Completed, Failed, Killed }

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Killed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStateBase {
    pub id: String,
    pub task_type: TaskType,
    pub status: TaskStatus,
    pub description: String,
    pub tool_use_id: Option<String>,
    pub start_time: SystemTime,
    pub end_time: Option<SystemTime>,
    pub total_paused_ms: u64,
    pub output_file: PathBuf,
    pub output_offset: u64,
    pub notified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TaskState {
    LocalBash(LocalBashTaskState),
    LocalAgent(LocalAgentTaskState),
    RemoteAgent(RemoteAgentTaskState),
    InProcessTeammate(InProcessTeammateTaskState),
    LocalWorkflow(LocalWorkflowTaskState),
    MonitorMcp(MonitorMcpTaskState),
    Dream(DreamTaskState),
}

// LocalAgent example
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAgentTaskState {
    #[serde(flatten)]
    pub base: TaskStateBase,
    pub agent_id: AgentId,
    pub prompt: String,
    pub selected_agent: Option<AgentDefinition>,
    pub agent_type: String,
    pub model: Option<String>,
    pub error: Option<String>,
    pub result: Option<AgentToolResult>,
    pub progress: Option<AgentProgress>,
    pub retrieved: bool,
    pub messages: Vec<ConversationMessage>,
    pub last_reported_tool_count: u32,
    pub last_reported_token_count: u64,
    pub is_backgrounded: bool,
    pub pending_messages: Vec<String>,  // SendMessage queue
    pub retain: bool,
    pub disk_loaded: bool,
    pub evict_after: Option<SystemTime>,
}

// Other 6 variants follow similar pattern
```

### 11.3 Task Trait

```rust
#[async_trait]
pub trait Task: Send + Sync {
    fn name(&self) -> &str;
    fn task_type(&self) -> TaskType;
    async fn spawn(&self, input: TaskSpawnInput, ctx: TaskContext) -> Result<TaskHandle, TaskError>;
    async fn kill(&self, task_id: &str, ctx: TaskContext) -> Result<(), TaskError>;
    async fn poll_progress(&self, task_id: &str, ctx: TaskContext) -> Option<TaskProgress> { None }
    fn supports_messages(&self) -> bool { false }
    async fn send_message(&self, task_id: &str, message: String, ctx: TaskContext) -> Result<(), TaskError> {
        Err(TaskError::Unsupported)
    }
}

#[derive(Clone)]
pub struct TaskContext {
    pub abort_signal: AbortSignal,
    pub effect_tx: EffectSender,
    pub fs: Arc<dyn FileSystem>,
    pub process: Arc<dyn ProcessRunner>,
    pub http: Arc<dyn HttpTransport>,
    pub mcp: Arc<McpRegistry>,
    pub worktree: Arc<dyn WorktreeManager>,
    pub hook_executor: Arc<HookExecutor>,
    /// Per-task handlers (e.g. LocalBashTask draining stdout to disk) MUST spawn
    /// background work via this trait — never via tokio::spawn directly. Keeps the
    /// RuntimeSpawner discipline of D17 consistent across all subsystems.
    pub runtime: Arc<dyn RuntimeSpawner>,
}
```

### 11.4 TaskRegistry

```rust
pub struct TaskRegistry {
    tasks: Arc<RwLock<HashMap<String, TaskState>>>,
    handlers: HashMap<TaskType, Arc<dyn Task>>,
    handles: Arc<Mutex<HashMap<String, BackgroundTaskHandle>>>,
    runtime: Arc<dyn RuntimeSpawner>,
    output_manager: Arc<TaskOutputManager>,
    notification_tx: mpsc::Sender<PendingNotification>,
    cleanup_registry: Arc<CleanupRegistry>,
}

impl TaskRegistry {
    pub async fn create(&self, task_type: TaskType, input: TaskSpawnInput, description: String) -> Result<String, TaskError> { ... }
    pub async fn kill(&self, task_id: &str) -> Result<(), TaskError> { ... }
    pub async fn get(&self, task_id: &str) -> Option<TaskState> { ... }
    pub async fn list(&self, filter: TaskFilter) -> Vec<TaskState> { ... }
    pub async fn update(&self, task_id: &str, update: TaskUpdate) -> Result<(), TaskError> { ... }
    pub async fn output(&self, task_id: &str, opts: OutputOptions) -> Result<TaskOutput, TaskError> { ... }
    pub async fn send_message(&self, task_id: &str, message: String) -> Result<(), TaskError> {
        let state = self.get_state(task_id).await?;
        if state.base().status.is_terminal() { return Err(TaskError::TerminatedTask); }
        // Queue message for drain at tool round boundary
        ...
    }
}
```

### 11.5 TaskOutputManager (Disk Persistence)

```rust
pub struct TaskOutputManager {
    output_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    max_file_size: u64,
    total_budget: u64,
    used: AtomicU64,
}

impl TaskOutputManager {
    pub async fn allocate(&self, task_id: &str) -> Result<PathBuf, TaskError> { ... }
    pub async fn init_as_symlink(&self, task_id: &str, target: &Path) -> Result<(), TaskError> { ... }
    pub async fn read(&self, output_file: &Path, opts: OutputOptions) -> Result<TaskOutput, TaskError> { ... }
    pub async fn append(&self, output_file: &Path, chunk: &str) -> Result<(), TaskError> { ... }
    pub async fn evict(&self, output_file: &Path) -> Result<(), TaskError> { ... }
}
```

### 11.6 Task Notification Injection

```rust
pub struct TaskNotificationBuilder;

impl TaskNotificationBuilder {
    pub fn build(
        task_id: &str,
        tool_use_id: Option<&str>,
        output_path: &Path,
        status: TaskStatus,
        summary: &str,
        final_message: Option<&str>,
        usage: Option<&TaskUsage>,
        worktree: Option<&WorktreeInfo>,
    ) -> String {
        // Builds XML-tagged notification for injection into next user message
        // <task-notification>
        //   <task-id>...</task-id>
        //   <output-file>...</output-file>
        //   <status>...</status>
        //   <summary>...</summary>
        //   ...
        // </task-notification>
        ...
    }
}

pub struct PendingNotification {
    pub value: String,
    pub mode: NotificationMode,
}

pub enum NotificationMode { Normal, TaskNotification }
```

### 11.7 Cron Tasks

```rust
pub struct CronTaskRegistry {
    tasks: Arc<RwLock<HashMap<String, CronTaskDef>>>,
    lock: Arc<dyn FileSystem>,  // File-based lock against duplicate triggers
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronTaskDef {
    pub id: String,
    pub schedule: String,
    pub prompt: String,
    pub agent_type: Option<String>,
    pub last_run: Option<SystemTime>,
    pub enabled: bool,
}
```

---

## 12. Coordinator / Team

### 12.1 CoordinatorMode

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorMode {
    pub enabled: bool,
    pub session_started_as_coordinator: bool,
    pub workflow_mode: bool,
    pub fork_mode: bool,
}

impl CoordinatorMode {
    pub fn match_session_mode(current: bool, session_stored: Option<bool>) -> Option<ModeSwitchResult> { ... }
}
```

### 12.2 Coordinator Internal Tools

```rust
/// Only available when coordinator mode is enabled
pub fn coordinator_internal_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(TeamCreateTool::new()),
        Arc::new(TeamDeleteTool::new()),
        Arc::new(SendMessageTool::new()),
        Arc::new(SyntheticOutputTool::new()),
    ]
}
```

### 12.3 TeamRegistry

```rust
pub struct TeamRegistry {
    workers: Arc<RwLock<HashMap<AgentId, WorkerAgent>>>,
    task_to_worker: HashMap<String, AgentId>,
    mailbox_router: Arc<MailboxRouter>,
    coordinator_id: AgentId,
}

#[derive(Debug, Clone)]
pub struct WorkerAgent {
    pub agent_id: AgentId,
    pub agent_type: String,
    pub name: String,
    pub parent_id: Option<AgentId>,
    pub status: WorkerStatus,
    pub task_id: String,
    pub mailbox: Arc<TeammateMailbox>,
    pub spawned_at: SystemTime,
    pub last_active_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerStatus {
    Idle,
    Working { current_activity: String },
    AwaitingMessage,
    Completed { result: Value },
    Failed { error: String },
    Killed,
}
```

### 12.4 TeammateMailbox

```rust
pub struct TeammateMailbox {
    pub agent_id: AgentId,
    inbox: Arc<Mutex<VecDeque<TeammateMessage>>>,
    waker: Arc<Notify>,
    closed: AtomicBool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateMessage {
    pub from: MessageSender,
    pub content: String,
    pub message_id: String,
    pub timestamp: SystemTime,
    pub attachments: Vec<TeammateAttachment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageSender { Coordinator, Teammate(AgentId), User, System }

impl TeammateMailbox {
    pub async fn deliver(&self, msg: TeammateMessage) -> Result<(), MailboxError> { ... }
    pub fn drain(&self) -> Vec<TeammateMessage> { ... }
    pub async fn wait_for_message(&self, timeout: Duration) -> Option<TeammateMessage> { ... }
}

pub struct MailboxRouter {
    mailboxes: RwLock<HashMap<AgentId, Arc<TeammateMailbox>>>,
}

impl MailboxRouter {
    pub fn register(&self, agent_id: AgentId, mailbox: Arc<TeammateMailbox>) { ... }
    pub async fn route(&self, to: &AgentId, msg: TeammateMessage) -> Result<(), MailboxError> { ... }
}
```

### 12.5 SyntheticOutput Tool

```rust
pub struct SyntheticOutputTool;

impl Tool for SyntheticOutputTool {
    async fn call(&self, input: Value, ctx: ToolUseContext, _: ToolProgressSender) -> Result<ToolCallResult, ToolError> {
        // input: { agent_id, content }
        // Find worker, wrap content as ToolResult
        // Inject into coordinator conversation
        ...
    }
    fn is_concurrency_safe(&self, _: &Value) -> bool { true }
    fn is_read_only(&self, _: &Value) -> bool { true }
}
```

### 12.6 Swarm Backend

```rust
// See §4.6 for trait definition. tmux implementation example:
pub struct TmuxSwarmBackend {
    process: Arc<dyn ProcessRunner>,
    session_name: String,
}

#[derive(Debug, Clone)]
pub enum SwarmLayout {
    LeaderFollower,
    Tiled,
    External,
}
```

### 12.7 Team Memory Sync

```rust
pub struct TeamMemorySync {
    team_dir: PathBuf,
    watcher: Box<dyn Stream<Item = FileEvent> + Send + Unpin>,
    secret_scanner: Arc<SecretScanner>,
    loaded: Arc<RwLock<HashMap<PathBuf, MemoryFile>>>,
}

impl TeamMemorySync {
    pub async fn notify_write(&self, path: &Path, content: &str) -> Result<(), MemoryError> {
        if let Some(violations) = self.secret_scanner.scan(content) {
            return Err(MemoryError::SecretLeak(violations));
        }
        ...
    }
    pub fn is_team_memory_write(&self, tool_name: &str, input: &Value) -> bool { ... }
    pub fn is_team_memory_search(&self, tool_name: &str, input: &Value) -> bool { ... }
}
```

---

## 13. Compaction Engine

### 13.1 Five Layers + Reactive

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompactionLayer {
    Snip,               // Layer 0: tail truncation, no LLM
    Microcompact,       // Layer 1: clear old tool results by time
    CachedMicrocompact, // Layer 2: edit prompt cache (preserves structure)
    ContextCollapse,    // Layer 3: collapse Read operations
    Autocompact,        // Layer 4: full summarization via forked agent
    PartialAutocompact, // Layer 5: directional partial summarization
}
```

### 13.2 Thresholds (verified from claude-code)

```rust
pub mod compaction_thresholds {
    pub const AUTOCOMPACT_BUFFER_TOKENS: u64 = 13_000;
    pub const WARNING_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
    pub const ERROR_THRESHOLD_BUFFER_TOKENS: u64 = 20_000;
    pub const MANUAL_COMPACT_BUFFER_TOKENS: u64 = 3_000;
    pub const MAX_OUTPUT_TOKENS_FOR_SUMMARY: u64 = 20_000;
    pub const MAX_CONSECUTIVE_AUTOCOMPACT_FAILURES: u32 = 3;
    pub const POST_COMPACT_MAX_FILES_TO_RESTORE: usize = 5;
    pub const POST_COMPACT_TOKEN_BUDGET: u64 = 50_000;
    pub const POST_COMPACT_MAX_TOKENS_PER_FILE: u64 = 5_000;
    pub const POST_COMPACT_MAX_TOKENS_PER_SKILL: u64 = 5_000;
    pub const POST_COMPACT_SKILLS_TOKEN_BUDGET: u64 = 25_000;
    pub const MAX_PTL_RETRIES: u32 = 3;
    pub const MAX_COMPACT_STREAMING_RETRIES: u32 = 2;
}
```

### 13.3 Autocompact Tracking (Circuit Breaker)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoCompactTrackingState {
    pub compacted: bool,
    pub turn_counter: u32,
    pub turn_id: String,
    pub consecutive_failures: u32,
}
```

### 13.4 Microcompact

```rust
pub fn compactable_tools() -> HashSet<&'static str> {
    HashSet::from(["Read", "Bash", "PowerShell", "Grep", "Glob", "WebSearch", "WebFetch", "Edit", "Write"])
}

pub const TIME_BASED_MC_CLEARED_MESSAGE: &str = "[Old tool result content cleared]";

pub struct Microcompactor {
    config: TimeBasedMCConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeBasedMCConfig {
    pub age_threshold: Duration,
    pub keep_recent_count: usize,
    pub max_per_result_bytes: usize,
    pub image_max_token_size: u64,
}

impl Microcompactor {
    pub fn compact(&self, messages: Vec<ConversationMessage>, now: SystemTime) -> MicrocompactResult { ... }
}
```

### 13.5 Cached Microcompact

```rust
pub struct CachedMicrocompactor {
    state: Arc<Mutex<CachedMCState>>,
    pending_cache_edits: Arc<Mutex<Option<CacheEditsBlock>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedMCState {
    pub deleted_tool_uses: HashSet<ToolUseId>,
    pub cache_deleted_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEditsBlock {
    pub delete_tool_use_ids: Vec<ToolUseId>,
    pub reason: String,
}

impl CachedMicrocompactor {
    pub fn mark_for_deletion(&self, tool_use_ids: Vec<ToolUseId>) { ... }
    pub fn consume_pending(&self) -> Option<CacheEditsBlock> { ... }
    pub fn on_api_response(&self, deleted_tokens: u64) { ... }
}
```

### 13.6 Autocompactor

```rust
pub struct Autocompactor {
    config: AutocompactConfig,
}

impl Autocompactor {
    pub async fn compact(
        &self,
        messages: Vec<ConversationMessage>,
        ctx: CompactionContext,
        api_client: &dyn ApiClient,
    ) -> Result<CompactionResult, CompactionError> {
        let stripped = strip_images_from_messages(messages);
        let stripped = strip_reinjected_attachments(stripped);
        let groups = group_messages_by_api_round(&stripped);
        let request = build_compact_request(&stripped, &self.config);
        let response = self.compact_with_retries(request, &groups, api_client).await?;
        let summary = extract_summary_from_response(&response);
        let session_memory = try_session_memory_compaction(&response, &stripped);
        let post_compact = PostCompactBuilder::build(summary, &stripped, session_memory).await?;
        Ok(CompactionResult {
            pre_compact_token_count: ctx.current_tokens,
            post_compact_token_count: estimate_tokens_for_messages(&post_compact.summary_messages),
            true_post_compact_token_count: response.usage.input_tokens,
            compaction_usage: Some(response.usage),
            summary_messages: post_compact.summary_messages,
            attachments: post_compact.attachments,
            hook_results: post_compact.hook_results,
        })
    }
    async fn compact_with_retries(&self, mut request: MessageRequest, groups: &[ApiRoundGroup], api_client: &dyn ApiClient) -> Result<MessageResponse, CompactionError> {
        for _attempt in 0..compaction_thresholds::MAX_PTL_RETRIES {
            match api_client.send(request.clone()).await {
                Ok(response) => return Ok(response),
                Err(ApiError::PromptTooLong { token_gap, response }) => {
                    request.messages = truncate_head_for_ptl_retry(request.messages, token_gap, groups)?;
                }
                Err(e) => return Err(CompactionError::Api(e)),
            }
        }
        Err(CompactionError::MaxRetriesExceeded)
    }
}
```

### 13.7 ReactiveCompactor

```rust
pub struct ReactiveCompactor {
    autocompactor: Arc<Autocompactor>,
}

impl ReactiveCompactor {
    pub async fn handle_ptl(
        &self,
        error: &PromptTooLongError,
        messages: Vec<ConversationMessage>,
        ctx: CompactionContext,
    ) -> Result<ReactiveCompactResult, CompactionError> {
        let token_gap = error.parse_token_gap().unwrap_or_else(|| estimate_tokens_for_messages(&messages) / 5);
        // Try microcompact first (cheapest)
        let mc_result = self.try_microcompact(&messages, token_gap).await;
        if mc_result.tokens_freed >= token_gap {
            return Ok(ReactiveCompactResult::MicrocompactSufficient { messages: mc_result.messages, tokens_freed: mc_result.tokens_freed });
        }
        // Else full autocompact
        let autocompact = self.autocompactor.compact(messages, ctx, /* api_client */).await?;
        Ok(ReactiveCompactResult::Autocompacted(autocompact))
    }
    pub fn is_withheld_prompt_too_long(msg: &Message) -> bool {
        matches!(msg, Message::Assistant(m) if m.api_error == Some("prompt_too_long"))
    }
}
```

### 13.8 SessionMemoryCompactor (Dual Extraction)

```rust
pub struct SessionMemoryCompactor {
    config: SessionMemoryCompactConfig,
}

impl SessionMemoryCompactor {
    /// Single LLM call: summarize history + extract memory in one response
    pub fn try_extract_memory(&self, compaction_response: &MessageResponse, original_messages: &[ConversationMessage]) -> Option<SessionMemoryExtraction> {
        let text = compaction_response.text_content();
        if let Some(memory_section) = extract_section(&text, "<session_memory>", "</session_memory>") {
            Some(SessionMemoryExtraction { content: memory_section, extracted_at_message_id: original_messages.last().map(|m| m.id().clone()) })
        } else { None }
    }
    pub fn should_use(config: &SessionMemoryConfig) -> bool { config.enabled && config.compact_extracts_memory }
}
```

### 13.9 PostCompactBuilder

```rust
pub struct PostCompactBuilder;

impl PostCompactBuilder {
    pub async fn build(
        summary: String,
        original_messages: &[ConversationMessage],
        session_memory: Option<SessionMemoryExtraction>,
    ) -> Result<PostCompactMessages, CompactionError> {
        let mut summary_messages = vec![create_compact_boundary_message(&summary)];
        let mut attachments = vec![];
        // Restore recent files (up to POST_COMPACT_MAX_FILES_TO_RESTORE)
        let recent_files = extract_recent_file_reads(original_messages, POST_COMPACT_MAX_FILES_TO_RESTORE);
        let mut file_tokens = 0;
        for file in recent_files {
            if file_tokens > POST_COMPACT_TOKEN_BUDGET { break; }
            let content = read_with_token_limit(&file, POST_COMPACT_MAX_TOKENS_PER_FILE).await?;
            attachments.push(file_to_attachment(file, content));
            file_tokens += estimate_tokens(&content);
        }
        // Re-inject active skills
        let active_skills = extract_active_skills(original_messages);
        let mut skill_tokens = 0;
        for skill in active_skills {
            if skill_tokens > POST_COMPACT_SKILLS_TOKEN_BUDGET { break; }
            let truncated = truncate_skill(&skill, POST_COMPACT_MAX_TOKENS_PER_SKILL);
            attachments.push(skill_to_attachment(skill.name, truncated));
            skill_tokens += POST_COMPACT_MAX_TOKENS_PER_SKILL;
        }
        if let Some(memory) = session_memory {
            attachments.push(session_memory_to_attachment(memory));
        }
        let hook_results = execute_post_compact_hooks(&summary, &attachments).await?;
        Ok(PostCompactMessages { summary_messages, attachments, hook_results })
    }
}
```

### 13.10 CompactionOrchestrator

```rust
pub struct CompactionOrchestrator {
    snip: Arc<SnipCompactor>,
    micro: Arc<Microcompactor>,
    cached_micro: Option<Arc<CachedMicrocompactor>>,
    context_collapse: Option<Arc<ContextCollapsor>>,
    auto: Arc<Autocompactor>,
    reactive: Arc<ReactiveCompactor>,
    session_memory: Arc<SessionMemoryCompactor>,
    hook_executor: Arc<HookExecutor>,
}

impl CompactionOrchestrator {
    /// Called every query loop iteration
    pub async fn process_iteration(
        &self,
        messages: Vec<ConversationMessage>,
        ctx: &CompactionContext,
        snip_tokens_freed_already: u64,
    ) -> Result<IterationCompactionResult, CompactionError> {
        let mut current = messages;
        let mut layers_applied = Vec::new();
        let mut total_tokens_freed = snip_tokens_freed_already;
        if snip_tokens_freed_already > 0 { layers_applied.push(CompactionLayer::Snip); }
        // Layer 1: Microcompact
        let mc_result = self.micro.compact(current, ctx.now);
        if mc_result.boundary_message.is_some() { layers_applied.push(CompactionLayer::Microcompact); }
        current = mc_result.messages;
        // Layer 2: Context collapse (feature gate)
        if let Some(cc) = &self.context_collapse {
            let cc_result = cc.apply_if_needed(current, ctx).await?;
            if cc_result.applied { layers_applied.push(CompactionLayer::ContextCollapse); }
            current = cc_result.messages;
        }
        // Layer 4: Autocompact (if still over)
        let current_tokens = estimate_tokens_for_messages(&current);
        if current_tokens > ctx.autocompact_threshold {
            self.hook_executor.execute(HookEvent::PreCompact { reason: CompactionReason::TokenLimit }).await;
            let auto_result = self.auto.compact(current, ctx.clone(), &*ctx.api_client).await?;
            current = auto_result.summary_messages.clone();
            layers_applied.push(CompactionLayer::Autocompact);
            total_tokens_freed += auto_result.pre_compact_token_count.saturating_sub(auto_result.post_compact_token_count);
            self.hook_executor.execute(HookEvent::PostCompact { summary: extract_summary_text(&auto_result), tokens_freed: total_tokens_freed }).await;
        }
        Ok(IterationCompactionResult { messages: current, layers_applied, total_tokens_freed })
    }
    /// Called on PTL API error
    pub async fn handle_api_error(&self, error: ApiError, messages: Vec<ConversationMessage>, ctx: &CompactionContext) -> Result<ReactiveCompactResult, CompactionError> {
        match error {
            ApiError::PromptTooLong(ptl) => self.reactive.handle_ptl(&ptl, messages, ctx.clone()).await,
            _ => Err(CompactionError::NotApplicable),
        }
    }
}
```

---

## 14. Permission Policy Engine

### 14.1 PermissionMode (5 external + 2 internal modes)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    // External/user-addressable modes (settings, CLI, recovery)
    /// Default: rule-driven decision, prompt on no match
    Default,
    /// Plan mode: only Read/Grep/Glob/WebSearch/WebFetch
    Plan,
    /// Accept all edits (file writes auto-approved; Bash still rule-bound)
    AcceptEdits,
    /// Skip all permission checks (YOLO)
    BypassPermissions,
    /// No prompts: matched=allow/deny, no match=deny
    DontAsk,
    // Internal-only modes
    /// Subagent only: bubble permission prompts up to parent terminal.
    /// Not accepted in settings/CLI validation.
    Bubble,
    /// Auto mode (transcript classifier feature-gated; not always runtime-valid)
    Auto,
}
```

### 14.2 PermissionRule

```rust
/// rule = (tool_name, optional content, behavior, source)
/// Examples: ("Bash", Some("git *"), Allow, ProjectSettings)
///           ("FileWrite", Some("/etc/**"), Deny, UserSettings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub value: PermissionRuleValue,
    pub behavior: PermissionBehavior,
    pub source: PermissionRuleSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRuleValue {
    pub tool_name: String,
    pub rule_content: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionBehavior { Allow, Deny, Ask }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionRuleSource {
    UserSettings,    // ~/.claude/settings.json
    ProjectSettings, // .claude/settings.json
    LocalSettings,   // .claude/settings.local.json
    FlagSettings,    // --settings / SDK inline settings
    PolicySettings,  // enterprise managed settings
    CliArg,          // --allow-tools / --deny-tools / --permission-mode
    Command,         // /permissions add ...
    Session,         // runtime "always allow"
}
```

### 14.3 PermissionResult

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior")]
pub enum PermissionResult {
    Allow {
        reason: PermissionDecisionReason,
        updated_input: Option<Value>,
        update_destination: Option<PermissionUpdateDestination>,
        metadata: PermissionMetadata,
    },
    Deny {
        reason: PermissionDecisionReason,
        explanation: Option<String>,
        metadata: PermissionMetadata,
    },
    Ask {
        reason: PermissionDecisionReason,
        prompt: PermissionPrompt,
        /// Bash safety classifiers may run while the user prompt is displayed.
        /// If they approve first, the prompt is resolved without user action.
        pending_classifier_check: Option<PendingClassifierCheck>,
        metadata: PermissionMetadata,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PermissionDecisionReason {
    MatchedRule { rule: PermissionRule },
    PermissionMode { mode: PermissionMode },
    SubcommandResults { reasons: HashMap<String, PermissionResult> },
    PermissionPromptTool { tool_name: String },
    ClassifierApproved { classifier: ClassifierKind, score: f64 },
    ClassifierRejected { classifier: ClassifierKind, score: f64 },
    HookOverride { hook_id: HookId, source: Option<String>, reason: Option<String> },
    AsyncAgent { reason: String },
    SandboxOverride { reason: SandboxOverrideReason },
    WorkingDirectory { reason: String },
    SafetyCheck { reason: String, classifier_approvable: bool },
    Other { reason: String },
    DenialLimitExceeded,
    AutoModeFallback,
    BypassPermissions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingClassifierCheck {
    pub classifier: ClassifierKind,
    pub request_id: RequestId,
    pub started_at: SystemTime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxOverrideReason {
    ExcludedCommand,
    DangerouslyDisableSandbox,
}
```

### 14.4 Classifiers (3 kinds)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassifierKind {
    /// Decides whether an action is destructive (acceptEdits / auto mode)
    Yolo,
    /// Decides if bash command is safe (rm -rf, sudo, curl|sh, ...)
    Bash,
    /// LLM-driven turn-level intent classifier (ant-only)
    Transcript,
}

#[async_trait]
pub trait PermissionClassifier: Send + Sync {
    fn kind(&self) -> ClassifierKind;
    async fn classify(&self, tool: &dyn Tool, input: &Value, ctx: &ClassifierContext) -> Result<ClassifierScore, ClassifierError>;
}

#[derive(Debug, Clone)]
pub struct ClassifierScore {
    pub score: f64,  // 0.0-1.0
    pub explanation: String,
    pub flagged_patterns: Vec<String>,
}

/// Bash dangerous patterns (compile-time static table)
pub mod dangerous_patterns {
    pub const PATTERNS: &[&str] = &[
        r"rm\s+-rf\s+/",
        r"sudo\s+",
        r"curl\s+.*\|\s*(sh|bash)",
        r"chmod\s+777",
        r":\(\)\{.*:.*\|.*&.*\};:",  // fork bomb
        // ...
    ];
}
```

### 14.5 Denial Tracking

```rust
/// Track consecutive denials; fall back to prompt after threshold.
/// Prevents Auto/AcceptEdits modes from looping on rejected actions.
#[derive(Debug, Clone)]
pub struct DenialTrackingState {
    pub per_tool_denials: HashMap<String, DenialRecord>,
    pub total_consecutive: u32,
    pub last_success_at: Option<SystemTime>,
}

#[derive(Debug, Clone)]
pub struct DenialRecord {
    pub consecutive_count: u32,
    pub last_denial_at: SystemTime,
    pub last_reason: PermissionDecisionReason,
}

pub mod denial_limits {
    pub const PER_TOOL_FALLBACK: u32 = 5;
    pub const GLOBAL_FALLBACK: u32 = 10;
    pub const RECORD_TTL: Duration = Duration::from_secs(300);
}
```

### 14.6 PermissionPolicy (Central Engine)

```rust
pub struct PermissionPolicy {
    mode: PermissionMode,
    allow_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    deny_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    ask_rules: HashMap<PermissionRuleSource, Vec<PermissionRule>>,
    additional_working_directories: HashMap<String, AdditionalWorkingDirectory>,
    classifiers: HashMap<ClassifierKind, Arc<dyn PermissionClassifier>>,
    denial_tracking: Arc<Mutex<DenialTrackingState>>,
    bypass_available: bool,
    bypass_killswitch_active: bool,
    shadow_detector: ShadowedRuleDetector,
}

impl PermissionPolicy {
    pub async fn authorize(&self, tool: &dyn Tool, input: &Value, ctx: &ToolUseContext) -> PermissionResult {
        // 1. Tool-level matcher
        // 2. Behavior traversal: Deny → Allow → Ask, with Claude Code source order:
        //    userSettings → projectSettings → localSettings → flagSettings → policySettings → cliArg → command → session.
        //    Policy/flag/command are read-only; only user/project/local/session/cliArg are valid update destinations.
        // 3. Mode-driven fallback (Bypass / Plan / AcceptEdits + Yolo / DontAsk / Auto + Transcript)
        // 4. Bash pendingClassifierCheck can race the user prompt and auto-approve before user response
        // 5. Denial limit fallback to Ask
        ...
    }

    pub fn apply_update(&mut self, update: PermissionUpdate) -> Result<(), PermissionError> { ... }
    pub fn detect_shadowing(&self, new_rule: &PermissionRule) -> Option<PermissionRule> { ... }
}
```

### 14.7 PermissionUpdate (Runtime Rule Modification)

```rust
/// Generated when user selects "Always allow" in permission dialog
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionUpdate {
    pub rule: PermissionRule,
    pub destination: PermissionUpdateDestination,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PermissionUpdateDestination {
    UserSettings,
    ProjectSettings,
    LocalSettings,
    Session,
    CliArg,
}
```

### 14.8 Integration

Canonical `Event`/`Effect` variants for permission are defined in §5.2-§5.3.
Section §14 owns the policy semantics only.

---

## 15. Plugin System

### 15.1 PluginManifest

```rust
/// A plugin materializes the same component surface as Claude Code plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: Option<String>,
    pub homepage: Option<String>,
    pub source: PluginSource,
    pub components: PluginComponents,
    pub trust_level: PluginTrustLevel,
    pub depends_on: Vec<PluginId>,
    /// User-provided values; sensitive entries are stored through SecureStorage
    /// and substituted into MCP/LSP/hooks/commands at load time.
    pub user_config: Option<UserConfigSchema>,
    pub channels: Vec<PluginChannel>,
    pub settings: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginComponents {
    pub commands: Vec<ComponentPath>,
    pub agents: Vec<ComponentPath>,
    pub skills: Vec<ComponentPath>,
    pub output_styles: Vec<ComponentPath>,
    pub hooks: Vec<HookDefinition>,
    pub mcp_servers: HashMap<String, McpServerConfig>,
    pub lsp_servers: HashMap<String, LspServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentPath {
    pub path: PathBuf,
    pub metadata: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfigSchema {
    pub fields: HashMap<String, UserConfigField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserConfigField {
    pub description: String,
    pub sensitive: bool,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginChannel {
    pub name: String,
    pub mcp_server: String,
    pub user_config: Option<UserConfigSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PluginSource {
    BuiltIn,
    OfficialMarketplace { name: String },
    Marketplace { url: String, name: String },
    Git { url: String, ref_: String },
    LocalPath { path: PathBuf },
    /// MCPB = MCP Bundle (zip format with manifest + assets)
    Mcpb { path: PathBuf, hash: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PluginTrustLevel {
    AdminTrusted,   // bundled / enterprise / official marketplace
    UserTrusted,    // user-installed marketplace plugin
    Untrusted,      // untrusted source (random git url)
}
```

### 15.2 Plugin Lifecycle

```rust
#[derive(Debug, Clone)]
pub enum PluginState {
    Declared { source: PluginSource },
    Fetching { source: PluginSource, started_at: SystemTime },
    Fetched { manifest: PluginManifest, install_dir: PathBuf },
    Loaded { manifest: PluginManifest, install_dir: PathBuf, loaded_at: SystemTime },
    Disabled { manifest: PluginManifest, install_dir: PathBuf },
    Failed { source: PluginSource, error: String },
    Blocked { source: PluginSource, reason: String },
}
```

### 15.3 PluginManager (Central Manager)

```rust
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<PluginId, PluginState>>>,
    marketplaces: Arc<RwLock<HashMap<String, Marketplace>>>,
    blocklist: Arc<PluginBlocklist>,
    install_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    http: Arc<dyn HttpTransport>,
    runtime: Arc<dyn RuntimeSpawner>,
    credential_manager: Arc<CredentialManager>,
    /// Cross-subsystem registries to materialize plugin components into
    command_registry: Arc<RwLock<CommandRegistry>>,
    agent_registry: Arc<RwLock<AgentRegistry>>,
    skill_registry: Arc<RwLock<SkillRegistry>>,
    hook_registry: Arc<RwLock<HookRegistry>>,
    output_style_registry: Arc<RwLock<OutputStyleRegistry>>,
    mcp_registry: Arc<RwLock<McpRegistry>>,
    lsp_registry: Arc<RwLock<LspRegistry>>,
    channel_registry: Arc<RwLock<ChannelRegistry>>,
}

impl PluginManager {
    pub async fn install(&self, source: PluginSource) -> Result<PluginId, PluginError> { ... }
    pub async fn uninstall(&self, id: &PluginId) -> Result<(), PluginError> { ... }
    pub async fn enable(&self, id: &PluginId) -> Result<(), PluginError> { ... }
    pub async fn disable(&self, id: &PluginId) -> Result<(), PluginError> { ... }
    pub async fn update(&self, id: &PluginId) -> Result<(), PluginError> { ... }
    pub async fn reload(&self, id: &PluginId) -> Result<(), PluginError> { ... }

    /// Load plugin → materialize all declared Claude Code components.
    async fn load_plugin(&self, manifest: &PluginManifest, install_dir: &Path) -> Result<(), PluginError> {
        let resolved_config = resolve_user_config(manifest, &*self.credential_manager).await?;

        let commands = load_plugin_commands(install_dir, &resolved_config).await?;
        self.command_registry.write().await.register_plugin_commands(manifest.id.clone(), commands);

        let agents = load_plugin_agents(install_dir).await?;
        self.agent_registry.write().await.register_plugin_agents(manifest.id.clone(), agents);

        let skills = load_plugin_skills(install_dir).await?;
        self.skill_registry.write().await.register_plugin_skills(manifest.id.clone(), skills);

        let hooks = load_plugin_hooks(install_dir, &resolved_config).await?;
        self.hook_registry.write().await.register_plugin_hooks(manifest.id.clone(), hooks);

        let output_styles = load_plugin_output_styles(install_dir).await?;
        self.output_style_registry.write().await.register_plugin_output_styles(manifest.id.clone(), output_styles);

        let servers = load_plugin_mcp_servers(install_dir, &resolved_config).await?;
        for server in servers {
            self.mcp_registry.write().await.register_plugin_server(manifest.id.clone(), server).await?;
        }

        let lsp_servers = load_plugin_lsp_servers(install_dir, &resolved_config).await?;
        self.lsp_registry.write().await.register_plugin_servers(manifest.id.clone(), lsp_servers).await?;

        let channels = load_plugin_channels(manifest, &resolved_config).await?;
        self.channel_registry.write().await.register_plugin_channels(manifest.id.clone(), channels);

        Ok(())
    }

    /// Unload plugin → clean up the exact registries touched by load_plugin.
    async fn unload_plugin(&self, id: &PluginId) -> Result<(), PluginError> {
        self.command_registry.write().await.unregister_plugin(id);
        self.agent_registry.write().await.unregister_plugin(id);
        self.skill_registry.write().await.unregister_plugin(id);
        self.hook_registry.write().await.unregister_plugin(id);
        self.output_style_registry.write().await.unregister_plugin(id);
        self.mcp_registry.write().await.unregister_plugin(id).await?;
        self.lsp_registry.write().await.unregister_plugin(id).await?;
        self.channel_registry.write().await.unregister_plugin(id);
        Ok(())
    }
}
```

### 15.4 Marketplace

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub url: String,
    pub trust_level: PluginTrustLevel,
    pub entries: Vec<MarketplaceEntry>,
    pub last_synced: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    pub plugin_id: PluginId,
    pub name: String,
    pub description: String,
    pub latest_version: String,
    pub install_count: Option<u64>,
    pub download_url: String,
    pub signature: Option<String>,
}

pub struct MarketplaceManager {
    official_url: String,
    user_marketplaces: HashMap<String, Marketplace>,
    http: Arc<dyn HttpTransport>,
    fs: Arc<dyn FileSystem>,
    runtime: Arc<dyn RuntimeSpawner>,
}

impl MarketplaceManager {
    pub async fn sync_official(&mut self) -> Result<(), PluginError> { ... }
    pub async fn search(&self, query: &str) -> Result<Vec<MarketplaceEntry>, PluginError> { ... }
    /// Compare declared vs materialized → produce install/update/remove diff
    pub async fn reconcile(&self) -> Result<MarketplaceDiff, PluginError> { ... }
}
```

### 15.5 Plugin Blocklist

```rust
/// Compile-time + remote blocklist for emergency takedown
pub struct PluginBlocklist {
    static_blocklist: HashSet<PluginId>,
    remote_blocklist: Arc<RwLock<HashSet<PluginId>>>,
    fetch_url: String,
}

impl PluginBlocklist {
    pub fn is_blocked(&self, id: &PluginId) -> Option<String> { ... }
    pub async fn refresh_remote(&self, http: &dyn HttpTransport) -> Result<(), PluginError> { ... }
}
```

### 15.6 Strict Plugin-Only Mode

```rust
/// Enterprise policy can lock certain components to plugin-only
/// e.g., strict_plugin_only = ["mcp", "agents"] means user-defined
/// .claude/agents/*.md is ignored; only plugin-provided agents are loaded.
pub struct StrictPluginOnlyPolicy {
    pub locked_components: HashSet<PluginComponent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PluginComponent {
    Commands,
    Agents,
    Skills,
    Hooks,
    OutputStyles,
    McpServers,
    LspServers,
    Channels,
}

impl StrictPluginOnlyPolicy {
    pub fn is_locked(&self, component: PluginComponent) -> bool { ... }
}
```

### 15.7 Integration (Plugin is the most cross-cutting subsystem)

```
PluginManager.load_plugin()
    ├── CommandRegistry.register_plugin_commands()  → core
    ├── AgentRegistry.register_plugin_agents()      → §10
    ├── SkillRegistry.register_plugin_skills()      → core
    ├── HookRegistry.register_plugin_hooks()        → §9
    ├── OutputStyleRegistry.register_plugin_styles()→ core/UI facade
    ├── McpRegistry.register_plugin_server()        → §7
    ├── LspRegistry.register_plugin_servers()       → tools/LSP
    └── ChannelRegistry.register_plugin_channels()  → MCP assistant channels
```

Canonical `Event`/`Effect` variants for plugin lifecycle are defined in §5.2-§5.3.
Section §15 owns manifest/component semantics and registry materialization.

Hook reload is transactional: compute the next plugin hook set, swap it under the
hook registry lock, and prune removed plugin hooks without clearing unrelated
registered hooks. Cache invalidation must not be used as hook unregistration.

---

## 16. Secret & Credential Management

### 16.1 SecureStorage Trait

`SecureStorage` lives in `lingxi-traits`; the shared DTOs below live in `lingxi-protocol`
so platform backends can implement the trait without depending on `lingxi-secret`.

```rust
#[async_trait]
pub trait SecureStorage: Send + Sync {
    async fn store(&self, service: &str, account: &str, data: SecureStorageData) -> Result<(), SecureStorageError>;
    async fn retrieve(&self, service: &str, account: &str) -> Result<Option<SecureStorageData>, SecureStorageError>;
    async fn delete(&self, service: &str, account: &str) -> Result<(), SecureStorageError>;
    async fn list(&self, service: &str) -> Result<Vec<String>, SecureStorageError>;
    fn is_encrypted(&self) -> bool;
    fn backend(&self) -> SecureStorageBackend;
}

#[derive(Clone)]
pub struct SecureStorageData {
    /// Actual secret bytes — MUST NEVER appear in log/transcript.
    /// Kept private so call sites must use `expose_secret()` explicitly.
    bytes: Secret<Vec<u8>>,
    pub metadata: SecureStorageMetadata,
}

impl std::fmt::Debug for SecureStorageData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecureStorageData")
            .field("bytes", &"<redacted>")
            .field("metadata", &self.metadata)
            .finish()
    }
}

impl SecureStorageData {
    pub fn new(bytes: Vec<u8>, metadata: SecureStorageMetadata) -> Self {
        Self { bytes: Secret::new(bytes), metadata }
    }
    pub fn expose_secret_bytes(&self) -> &[u8] { self.bytes.expose_secret() }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecureStorageMetadata {
    pub created_at: SystemTime,
    pub last_accessed: Option<SystemTime>,
    pub kind: SecretKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SecretKind {
    AnthropicApiKey,
    AnthropicOAuthAccessToken,
    AnthropicOAuthRefreshToken,
    AwsCredentials,
    McpOAuthAccessToken { server: String },
    McpOAuthRefreshToken { server: String },
    GenericApiKey { provider: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureStorageBackend {
    MacOsKeychain,
    LinuxLibsecret,      // GNOME Keyring / KWallet
    WindowsCredVault,
    AndroidKeystore,
    IosKeychain,
    EncryptedFile,       // fallback
    PlainText,           // last resort, warning
}
```

### 16.2 Secret<T> Newtype (Anti-Leak, defined in lingxi-protocol)

```rust
/// Newtype prevents accidental Debug/Display leaks in logs, error messages,
/// or transcripts. Only `expose_secret()` exposes the raw value, making
/// secret access grep-able during code review.
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<redacted>")
    }
}

impl<T> std::fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<redacted>")
    }
}

impl<T> Secret<T> {
    pub fn new(value: T) -> Self { Self(value) }
    /// The only way to get the raw value — code review must flag every call site
    pub fn expose_secret(&self) -> &T { &self.0 }
}
```

### 16.3 CredentialManager

```rust
pub struct CredentialManager {
    storage: Arc<dyn SecureStorage>,
    api_key_cache: Arc<RwLock<Option<CachedApiKey>>>,
    oauth_cache: Arc<RwLock<HashMap<String, CachedOAuthToken>>>,
    refresh_lock: Arc<Mutex<()>>,  // prevent concurrent refresh races
    http: Arc<dyn HttpTransport>,
    clock: Arc<dyn Clock>,
}

#[derive(Debug, Clone)]
struct CachedApiKey {
    key: Arc<Secret<String>>,
    cached_at: SystemTime,
    ttl: Duration,
}

#[derive(Debug, Clone)]
struct CachedOAuthToken {
    access_token: Arc<Secret<String>>,
    refresh_token: Option<Arc<Secret<String>>>,
    expires_at: SystemTime,
}

impl CredentialManager {
    /// Priority: env > keychain > config
    pub async fn get_anthropic_api_key(&self) -> Result<Option<Secret<String>>, CredentialError> { ... }

    /// Get OAuth access token, auto-refresh on near-expiry
    pub async fn get_oauth_access_token(&self) -> Result<Option<Secret<String>>, CredentialError> {
        let cached = self.oauth_cache.read().await.clone();
        if let Some(cached) = cached {
            if cached.expires_at > self.clock.now() + Duration::from_secs(60) {
                return Ok(Some((*cached.access_token).clone()));
            }
            // Near expiry — refresh under lock
            return self.refresh_oauth_token(cached.refresh_token).await;
        }
        // Load from storage
        ...
    }

    async fn refresh_oauth_token(&self, refresh: Option<Arc<Secret<String>>>) -> Result<Option<Secret<String>>, CredentialError> {
        let _guard = self.refresh_lock.lock().await;
        // double-check after acquiring lock
        ...
    }

    pub async fn store_oauth_token(&self, tokens: OAuthTokens) -> Result<(), CredentialError> { ... }
    pub async fn clear_all(&self) -> Result<(), CredentialError> { ... }
}
```

### 16.4 Keychain Prefetch (Startup Optimization)

```rust
/// macOS keychain first access shows an authorization dialog. Trigger it
/// at startup parallel with other init so the prompt appears once, early.
pub struct KeychainPrefetch {
    result_rx: Mutex<Option<oneshot::Receiver<Result<Option<SecureStorageData>, SecureStorageError>>>>,
    handle: BackgroundTaskHandle,
}

impl KeychainPrefetch {
    pub async fn start_prefetch(storage: Arc<dyn SecureStorage>, runtime: &dyn RuntimeSpawner) -> Result<Self, RuntimeError> {
        let (tx, rx) = oneshot::channel();
        let storage_clone = storage.clone();
        let handle = runtime.spawn("keychain-prefetch", Box::pin(async move {
            let result = storage_clone.retrieve("lingxi", "anthropic-credentials").await;
            let _ = tx.send(result);
        })).await?;
        Ok(Self { result_rx: Mutex::new(Some(rx)), handle })
    }
    pub async fn consume(&self) -> Option<Result<Option<SecureStorageData>, SecureStorageError>> { ... }
}
```

### 16.5 Secret Scanner (Leak Detection)

```rust
/// Scans data before it crosses low-trust boundaries (log / transcript /
/// team memory upload / conversation injection / telemetry).
/// Rules sourced from gitleaks (high-confidence subset with distinctive prefixes).
pub struct SecretScanner {
    rules: Vec<CompiledSecretRule>,
}

#[derive(Debug, Clone)]
pub struct CompiledSecretRule {
    pub id: String,
    pub label: String,
    pub pattern: regex::Regex,
}

/// 30+ built-in rules (gitleaks subset)
pub fn builtin_rules() -> Vec<SecretRuleSpec> {
    vec![
        // Cloud providers
        SecretRuleSpec { id: "aws-access-token", source: r"\b((?:A3T[A-Z0-9]|AKIA|ASIA|ABIA|ACCA)[A-Z2-7]{16})\b" },
        SecretRuleSpec { id: "gcp-api-key", source: r"\b(AIza[\w-]{35})..." },
        SecretRuleSpec { id: "azure-ad-client-secret", source: "..." },
        SecretRuleSpec { id: "digitalocean-pat", source: r"\b(dop_v1_[a-f0-9]{64})..." },
        // AI APIs
        // Build distinctive Anthropic prefixes from fragments at runtime so the bundled scanner
        // does not itself contain a complete credential-looking token prefix.
        SecretRuleSpec::anthropic_api_key_runtime_built_prefix(),
        SecretRuleSpec { id: "anthropic-admin-api-key", source: "..." },
        SecretRuleSpec { id: "openai-api-key", source: "..." },
        SecretRuleSpec { id: "huggingface-access-token", source: "..." },
        // Version control
        SecretRuleSpec { id: "github-pat", source: r"ghp_[0-9a-zA-Z]{36}" },
        SecretRuleSpec { id: "github-fine-grained-pat", source: r"github_pat_\w{82}" },
        SecretRuleSpec { id: "gitlab-pat", source: "..." },
        // ... 20+ more
    ]
}

impl SecretScanner {
    /// Public detection results intentionally omit values and byte ranges.
    /// Redaction can use internal match spans, but scan results are safe for telemetry/logging.
    pub fn scan(&self, content: &str) -> Vec<SecretDetection> { ... }
    pub fn redact(&self, content: &str) -> String { ... }
}

#[derive(Debug, Clone)]
pub struct SecretDetection {
    pub rule_id: String,
    pub label: String,
}

/// Protocol DTO for effect payloads that may contain sensitive user content.
#[derive(Clone)]
pub struct RedactableContent(String);

impl std::fmt::Debug for RedactableContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "<redactable-content>")
    }
}

impl RedactableContent {
    pub fn new(content: String) -> Self { Self(content) }
    pub fn expose_for_scan(&self) -> &str { &self.0 }
}
```

### 16.6 Redaction Boundaries

```rust
pub enum RedactionBoundary {
    TranscriptWrite,
    LogOutput,
    TeamMemoryUpload,
    ConversationInjection,
    Telemetry,
}

pub struct RedactionPolicy {
    scanner: Arc<SecretScanner>,
    boundary_policies: HashMap<RedactionBoundary, BoundaryPolicy>,
}

#[derive(Debug, Clone)]
pub enum BoundaryPolicy {
    AlwaysRedact,          // replace matches with [REDACTED:<rule-id>]
    RejectOnDetection,     // refuse to transmit on any match
    WarnOnly,              // dev-only
}
```

### 16.7 Integration

Canonical `Event`/`Effect` variants for secret and credential flows are defined in §5.2-§5.3.
Effect payloads that may contain user content use `RedactableContent`, whose `Debug`
implementation never prints the body.

---

## 17. API Cost & Budget Tracking

### 17.1 Provider/Model Pricing Catalog

```rust
/// Cost keys include provider because the same model string can exist behind
/// different gateways, and future providers (OpenAI/ChatGPT-family, Gemini,
/// OpenAI-compatible vendors) can have different token categories and rates.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModelRef {
    pub provider: ProviderId,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderId {
    Anthropic,
    OpenAI,
    GoogleGemini,
    OpenAICompatible { name: String },
    Custom { name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TokenClass {
    Input,
    Output,
    CacheWrite,
    CacheRead,
    ReasoningOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NonTokenBillableUnit {
    WebSearchRequest,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MoneyPerToken {
    /// Store rates as nano-USD per token to keep calculation deterministic.
    pub nano_usd_per_token: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub model_ref: ModelRef,
    pub token_rates: HashMap<TokenClass, MoneyPerToken>,
    pub non_token_rates_nano_usd: HashMap<NonTokenBillableUnit, u64>,
    pub effective_from: Option<SystemTime>,
    pub source: PricingSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PricingSource {
    BuiltInReference { provider: ProviderId },
    HostOverride { path: PathBuf },
    RemoteManagedSettings,
}

pub struct PricingCatalog {
    entries: HashMap<ModelRef, ModelPricing>,
    provider_defaults: HashMap<ProviderId, ModelPricing>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PricingResolution {
    ExactModel { model_ref: ModelRef },
    ProviderDefault { requested: ModelRef },
    UnpricedModel { requested: ModelRef },
}

impl PricingCatalog {
    /// Resolve by provider + model. Anthropic/Claude Code parity entries are the first
    /// built-in reference set; OpenAI (ChatGPT-family) and Gemini entries can be added without touching CostTracker.
    pub fn resolve(&self, model_ref: &ModelRef, usage: &Usage) -> Result<(ModelPricing, PricingResolution), CostError> { ... }
}
```

### 17.2 Usage Counter

```rust
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub tokens: TokenUsage,
    pub server_tool_use: Option<ServerToolUsage>,
    /// Provider-reported speed/variant can change model pricing while the model
    /// string stays the same (e.g. fast tier). Catalog resolution may inspect it.
    pub speed: Option<ApiSpeed>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
    pub reasoning_output: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ServerToolUsage {
    pub web_search_requests: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiSpeed { Standard, Fast }

impl Usage {
    pub fn add(&mut self, other: &Usage) { ... }
    pub fn tokens_for(&self, class: TokenClass) -> u64 { ... }
    pub fn total_tokens(&self) -> u64 { ... }
}
```

Provider adapters normalize native API usage into this shape before emitting
`ApiStreamEnd`. Anthropic cache fields map to `cache_write/cache_read`; OpenAI or
Gemini-specific fields map to the closest `TokenClass`, and unsupported classes
stay at zero. This keeps calculation model/token-based even when providers differ.

### 17.3 CostCalculator

```rust
pub struct CostCalculator;

impl CostCalculator {
    pub fn calculate_nano_usd(usage: &Usage, pricing: &ModelPricing) -> u64 {
        let token_cost = pricing.token_rates.iter().map(|(class, rate)| {
            usage.tokens_for(*class) * rate.nano_usd_per_token
        }).sum::<u64>();

        let web_search = usage.server_tool_use
            .map(|s| {
                s.web_search_requests as u64
                    * pricing.non_token_rates_nano_usd
                        .get(&NonTokenBillableUnit::WebSearchRequest)
                        .copied()
                        .unwrap_or(0)
            })
            .unwrap_or(0);

        token_cost + web_search
    }

    /// How much prompt cache saved (for display)
    pub fn cache_savings_nano_usd(usage: &Usage, pricing: &ModelPricing) -> u64 {
        let Some(input_rate) = pricing.token_rates.get(&TokenClass::Input) else { return 0 };
        let Some(cache_read_rate) = pricing.token_rates.get(&TokenClass::CacheRead) else { return 0 };
        let full = usage.tokens.cache_read * input_rate.nano_usd_per_token;
        let discounted = usage.tokens.cache_read * cache_read_rate.nano_usd_per_token;
        full.saturating_sub(discounted)
    }
}
```

### 17.4 CostTracker

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostState {
    pub session_id: SessionId,
    pub total_nano_usd: u64,
    pub per_model_usage: HashMap<ModelRef, ModelUsage>,
    pub total_api_duration_ms: u64,
    pub total_api_duration_without_retries_ms: u64,
    pub total_tool_duration_ms: u64,
    pub total_lines_added: u64,
    pub total_lines_removed: u64,
    pub unpriced_models: HashSet<ModelRef>,
    pub total_web_search_requests: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsage {
    pub model_ref: ModelRef,
    pub usage: Usage,
    pub cost_nano_usd: u64,
    pub context_window: u64,
    pub max_output_tokens: u64,
}

pub struct CostTracker {
    state: Arc<RwLock<CostState>>,
    catalog: Arc<PricingCatalog>,
    /// Persistence to .claude/project-config.json
    persist_fn: Arc<dyn Fn(&CostState) + Send + Sync>,
}

impl CostTracker {
    pub async fn record_api_response(&self, model_ref: ModelRef, usage: Usage, duration_ms: u64, retries: u32) -> Result<(), CostError> {
        let (pricing, pricing_resolution) = self.catalog.resolve(&model_ref, &usage)?;
        let cost_nano_usd = CostCalculator::calculate_nano_usd(&usage, &pricing);
        let mut state = self.state.write().await;
        state.total_nano_usd += cost_nano_usd;
        state.total_api_duration_ms += duration_ms;
        if retries == 0 {
            state.total_api_duration_without_retries_ms += duration_ms;
        }
        let model_usage = state.per_model_usage
            .entry(model_ref.clone())
            .or_insert_with(|| ModelUsage {
                model_ref: model_ref.clone(),
                usage: Usage::default(),
                cost_nano_usd: 0,
                context_window: 0,
                max_output_tokens: 0,
            });
        model_usage.usage.add(&usage);
        model_usage.cost_nano_usd += cost_nano_usd;
        if let PricingResolution::UnpricedModel { requested } = pricing_resolution {
            state.unpriced_models.insert(requested);
        }
        if let Some(s) = usage.server_tool_use {
            state.total_web_search_requests += s.web_search_requests;
        }
        drop(state);
        (self.persist_fn)(&self.snapshot().await);
        Ok(())
    }

    pub async fn total_nano_usd(&self) -> u64 { self.state.read().await.total_nano_usd }
    pub async fn snapshot(&self) -> CostState { self.state.read().await.clone() }
    pub async fn reset(&self) { *self.state.write().await = CostState::default(); }
}
```

### 17.5 Budget Enforcement

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub max_session_nano_usd: Option<u64>,
    pub max_turn_nano_usd: Option<u64>,
    pub max_turn_tokens: Option<u64>,
    pub warning_thresholds: Vec<f64>,  // [0.5, 0.8, 0.95]
    pub on_exceed: BudgetExceedPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BudgetExceedPolicy { Halt, AskUser, WarnOnly }

pub struct BudgetEnforcer {
    config: BudgetConfig,
    cost_tracker: Arc<CostTracker>,
    warnings_fired: Arc<RwLock<HashSet<u32>>>,
}

impl BudgetEnforcer {
    pub async fn check_pre_api_call(&self, estimated_cost_nano_usd: u64) -> BudgetCheckResult {
        let current = self.cost_tracker.total_nano_usd().await;
        let after = current + estimated_cost_nano_usd;
        if let Some(max) = self.config.max_session_nano_usd {
            if after > max {
                return match self.config.on_exceed {
                    BudgetExceedPolicy::Halt => BudgetCheckResult::Halt { current, limit: max },
                    BudgetExceedPolicy::AskUser => BudgetCheckResult::AskUser { current, limit: max },
                    BudgetExceedPolicy::WarnOnly => BudgetCheckResult::Warn { current, limit: max },
                };
            }
            for &threshold in &self.config.warning_thresholds {
                let ratio = after / max;
                if ratio >= threshold {
                    let pct = (threshold * 100.0) as u32;
                    if self.warnings_fired.write().await.insert(pct) {
                        return BudgetCheckResult::ThresholdWarning { pct, current, limit: max };
                    }
                }
            }
        }
        BudgetCheckResult::Ok
    }
}

#[derive(Debug, Clone)]
pub enum BudgetCheckResult {
    Ok,
    ThresholdWarning { pct: u32, current: u64, limit: u64 },
    Warn { current: u64, limit: u64 },
    AskUser { current: u64, limit: u64 },
    Halt { current: u64, limit: u64 },
}
```

### 17.6 Integration

Canonical `Event`/`Effect` variants for cost and budget flows are defined in §5.2-§5.3.
Budget policies are the exceed actions (`Halt`, `AskUser`, `WarnOnly`); threshold
warnings are separate budget events, not a fourth exceed policy.

Adding a provider such as OpenAI or Gemini is a catalog/adapter change:
1. add provider-specific `ModelPricing` entries keyed by `ModelRef`;
2. normalize that provider's API usage response into `Usage`;
3. add parity fixtures for representative input/output/cache/reasoning token counts.
No CostTracker or BudgetEnforcer logic should become provider-specific.

---

## 18. Skills System

### 18.1 Skill Definition

```rust
/// Skill = markdown file with frontmatter that the model discovers and invokes via SkillTool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub frontmatter: SkillFrontmatter,
    pub content: String,
    pub source: SkillSource,
    pub loaded_from: LoadedFrom,
    pub plugin_id: Option<PluginId>,
    pub file_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub tools_allowed: Option<Vec<String>>,
    pub auto_search: bool,
    pub triggers: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SkillSource {
    Bundled, User, Project, Plugin, Managed,
    Mcp { server_name: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadedFrom {
    Bundled, Skills, Plugin, Managed, Mcp, CommandsDeprecated,
}
```

### 18.2 SkillRegistry

```rust
pub struct SkillRegistry {
    skills: HashMap<String, Skill>,
    trigger_index: HashMap<String, Vec<String>>,
    mcp_skills: HashMap<McpConnectionId, Vec<String>>,
    plugin_skills: HashMap<PluginId, Vec<String>>,
}

impl SkillRegistry {
    pub fn discover(&self, query: &str) -> Vec<&Skill>;
    pub fn get(&self, name: &str) -> Option<&Skill>;
    pub fn register_plugin_skills(&mut self, plugin_id: PluginId, skills: Vec<Skill>);
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId);
    pub fn register_mcp_skills(&mut self, conn: McpConnectionId, skills: Vec<Skill>);
}
```

### 18.3 SkillTool & Discovery Prefetch

```rust
/// Meta-tool: input = skill_name + arguments; execution = forked agent runs the skill content
pub struct SkillTool {
    registry: Arc<RwLock<SkillRegistry>>,
}

impl Tool for SkillTool {
    fn name(&self) -> &str { "Skill" }
    async fn call(&self, input: Value, ctx: ToolUseContext, ...) -> Result<ToolCallResult, ToolError> {
        let skill_name = input["name"].as_str().ok_or(ToolError::InvalidInput)?;
        let skill = self.registry.read().await.get(skill_name).cloned();
        // Execute via §24 ForkedAgentRunner with skill content as initial prompt
        ...
    }
}

/// Parallel skill discovery (analogous to §6.4 MemoryPrefetch)
pub struct SkillDiscoveryPrefetch {
    registry: Arc<RwLock<SkillRegistry>>,
    runtime: Arc<dyn RuntimeSpawner>,
}
```

### 18.4 Integration

- §15 Plugin → `register_plugin_skills`
- §7 MCP → mcp_skill_builders derive skills from MCP server tool descriptions
- §13 Compaction → `POST_COMPACT_SKILLS_TOKEN_BUDGET` re-injects active skills
- §9 Hooks → `skill_improvement` builtin handler proposes new skills from PostToolUse
- §24 Side Query / Forked Agent → skill execution runs through ForkedAgent

---

## 19. Slash Commands

### 19.1 SlashCommand Definition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub source: CommandSource,
    pub args_schema: Option<ArgsSchema>,
    pub kind: SlashCommandKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SlashCommandKind {
    /// 80+ built-in commands: /help /init /memory /compact /resume /login /plugin /agents …
    Builtin { handler_id: String },
    /// Markdown files in ~/.claude/commands/ or .claude/commands/
    Markdown {
        file_path: PathBuf,
        frontmatter: CommandFrontmatter,
        prompt_template: String,
    },
    /// Plugin-provided (markdown + plugin attribution)
    Plugin {
        plugin_id: PluginId,
        file_path: PathBuf,
        frontmatter: CommandFrontmatter,
        prompt_template: String,
    },
    /// MCP prompt promoted to slash command
    Mcp { connection_id: McpConnectionId, prompt_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandFrontmatter {
    pub description: String,
    pub tools_allowed: Option<Vec<String>>,
    pub model: Option<ModelAlias>,
    pub argument_hints: Vec<ArgumentHint>,
    pub allowed_tools: Option<Vec<String>>,
    pub thinking: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandSource {
    Builtin, User, Project, Local, Plugin, Managed, Mcp,
}
```

### 19.2 Parsing & Argument Substitution

```rust
pub fn parse_slash_command(input: &str) -> Option<ParsedSlashCommand>;

#[derive(Debug, Clone)]
pub struct ParsedSlashCommand {
    pub name: String,
    pub raw_args: String,
    pub positional_args: Vec<String>,
}

/// Substitutes $1, $2, $ARGUMENTS, $@ in prompt templates
pub fn substitute_arguments(template: &str, args: &ParsedSlashCommand) -> String;
```

### 19.3 CommandRegistry

```rust
pub struct CommandRegistry {
    commands: HashMap<String, SlashCommand>,
    aliases: HashMap<String, String>,
    builtin_handlers: HashMap<String, Arc<dyn BuiltinCommandHandler>>,
    plugin_commands: HashMap<PluginId, Vec<String>>,
}

#[async_trait]
pub trait BuiltinCommandHandler: Send + Sync {
    async fn handle(&self, args: &ParsedSlashCommand, ctx: &CommandContext) -> CommandResult;
    fn name(&self) -> &str;
    fn description(&self) -> &str;
}

#[derive(Debug, Clone)]
pub enum CommandResult {
    Done { display: Option<String> },
    InjectMessage { content: String },
    EmitEffects { effects: Vec<Effect>, display: Option<String> },
    SwitchMode { new_state: ConversationState },
    RequestConfirmation { prompt: String, on_confirm: Vec<Effect> },
}
```

### 19.4 Integration

Slash commands touch nearly every subsystem:

| Command | Subsystem |
|---|---|
| `/memory add` | §6 Memory |
| `/compact` | §13 Compaction |
| `/permissions add` | §14 Permission |
| `/plugin install` | §15 Plugin |
| `/login` `/logout` | §16 Secret + §34 AnthropicOAuth |
| `/cost` | §17 Cost |
| `/resume` | §25 SessionStorage |
| `/output-style` | §29 OutputStyles |
| `/mcp` `/agents` `/hooks` `/skills` | per-subsystem registries |
| `/tasks` | §11 Tasks |
| `/cron` | §32 CronScheduler |
| `/ide` | §33 IDE Bridge |

---

## 20. Side Query & Forked Agent

Shared infrastructure used by §6 (Memory selector), §13 (Compaction), §14 (Classifier explainer), Session memory extraction, and any subsystem needing a side LLM call.

### 20.1 SideQuery — stateless LLM call

```rust
/// One-shot LLM call outside the main conversation loop.
/// Used by: memory selector, permission explainer, session search, classifiers.
#[derive(Debug, Clone)]
pub struct SideQueryRequest {
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<ConversationMessage>,
    pub tools: Vec<ToolDefinition>,
    pub tool_choice: Option<ToolChoice>,
    pub output_format: Option<JsonOutputFormat>,
    pub max_tokens: u32,
    pub max_retries: u32,
    pub temperature: Option<f32>,
    pub thinking: Option<ThinkingConfig>,
    pub stop_sequences: Vec<String>,
    pub query_source: QuerySource,
    pub skip_system_prompt_prefix: bool,
}

#[derive(Debug, Clone)]
pub struct SideQueryResponse {
    pub text: Option<String>,
    pub structured: Option<Value>,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Usage,
    pub stop_reason: StopReason,
}

#[async_trait]
pub trait SideQueryClient: Send + Sync {
    async fn query(&self, request: SideQueryRequest) -> Result<SideQueryResponse, SideQueryError>;
}
```

### 20.2 ForkedAgent — full agent loop replica

```rust
/// Runs a complete query loop sharing the parent's prompt cache but isolated state.
/// Used by: compaction summarization, session memory extraction, supervisor, post-turn summary.
pub struct ForkedAgentRunner {
    pool: Arc<StateMachinePool>,
    cache_safe_slot: Arc<CacheSafeParamsSlot>,
}

/// Byte-exact params for prompt cache hit (Anthropic cache key components)
#[derive(Debug, Clone)]
pub struct CacheSafeParams {
    pub system_prompt: SystemPrompt,
    pub user_context: HashMap<String, String>,
    pub system_context: HashMap<String, String>,
    pub tool_use_context: ToolUseContext,
    pub fork_context_messages: Vec<ConversationMessage>,
}

/// Updated after each turn so post-turn forks can inherit the main loop's cache
pub struct CacheSafeParamsSlot {
    last: Arc<RwLock<Option<CacheSafeParams>>>,
}

impl CacheSafeParamsSlot {
    pub async fn save(&self, params: CacheSafeParams);
    pub async fn get_last(&self) -> Option<CacheSafeParams>;
}

#[derive(Debug, Clone)]
pub struct ForkedAgentRequest {
    pub prompt_messages: Vec<ConversationMessage>,
    pub cache_safe_params: CacheSafeParams,
    pub fork_label: String,
    pub query_source: QuerySource,
    pub overrides: SubagentContextOverrides,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub enum ForkPurpose {
    Compaction,
    SessionMemoryExtraction,
    Supervisor,
    PromptSuggestion,
    PostTurnSummary,
    ClassifierExplainer,
    SkillExecution,
    Custom(String),
}
```

### 20.3 ForkedAgent vs §10 Subagent

| Dimension | §10 Subagent (AgentTool) | §20 ForkedAgent (infra) |
|---|---|---|
| Triggered by | Model calls AgentTool | System internal trigger |
| User visible | ✅ shows as tool call | ❌ invisible to model |
| Lifecycle | Multi-turn, interruptible | One-shot, completes then disposed |
| Result destination | Returned as parent tool_result | Returned to calling code |
| Permission | bubble / isolated / auto / plan | Inherits parent context, typically read-only |
| Worktree | Optional | Never (no isolation needed) |
| Cache sharing | Fork mode = byte-exact | Always byte-exact |

### 20.4 Cross-subsystem fixes (C1, C2)

`§6.3 MemorySelector` and `§13.6 Autocompactor` now explicitly call `SideQueryClient` / `ForkedAgentRunner`:

```rust
// §6.3 MemorySelector
let request = SideQueryRequest {
    model: self.selector_model.clone(),
    system_prompt: Some(MEMORY_SELECTOR_PROMPT.into()),
    messages: vec![create_user_message(prompt)],
    output_format: Some(json_output_format_for_filename_list()),
    max_tokens: 1024,
    query_source: QuerySource::MemorySelector,
    ...
};
let response = self.side_query_client.query(request).await?;

// §13.6 Autocompactor
let cache_safe = self.cache_safe_params_slot.get_last().await
    .ok_or(CompactionError::NoCacheSafeParams)?;
let request = ForkedAgentRequest {
    prompt_messages: vec![create_user_message(compact_prompt)],
    cache_safe_params: cache_safe,
    fork_label: "compaction".into(),
    query_source: QuerySource::Compaction,
    ...
};
let response = self.forked_runner.run(request).await?;
```

---

## 21. Output Styles

### 21.1 OutputStyle Definition

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputStyle {
    pub name: String,
    pub description: String,
    pub source: OutputStyleSource,
    pub frontmatter: OutputStyleFrontmatter,
    pub system_prompt_addendum: String,
    pub source_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputStyleFrontmatter {
    pub name: String,
    pub description: String,
    pub default: bool,
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputFormat { Markdown, Plain, JsonStream, Concise, Explanatory }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputStyleSource { Builtin, User, Project, Plugin, Managed }
```

### 21.2 OutputStyleRegistry

```rust
pub struct OutputStyleRegistry {
    styles: HashMap<String, OutputStyle>,
    current_active: Arc<RwLock<String>>,
    plugin_styles: HashMap<PluginId, Vec<String>>,
}

impl OutputStyleRegistry {
    pub fn active(&self) -> OutputStyle;
    pub async fn switch(&self, name: &str) -> Result<(), OutputStyleError>;
    pub fn register_plugin_styles(&mut self, plugin_id: PluginId, styles: Vec<OutputStyle>);
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId);
}
```

### 21.3 Integration

- §5 Prompt assembly injects `current_active.system_prompt_addendum`
- §15 Plugin registers plugin styles
- §19 SlashCommand `/output-style` switches active

---

## 22. Session Storage & Recovery

### 22.1 File Layout

```
~/.claude/sessions/<session_id>/
├── metadata.json
├── transcript.jsonl
├── content_replacements.jsonl
├── queue_operations.jsonl
├── agents/<agent_id>/{metadata.json, sidechain.jsonl}
├── tasks/<task_id>.txt
├── session_memory.md
└── plan.md

~/.claude/projects/<project_hash>/
├── current_session_id
└── last_session_id_for_resume
```

### 22.2 Persistence Model

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: SessionId,
    pub parent_session_id: Option<SessionId>,
    pub created_at: SystemTime,
    pub project_dir: PathBuf,
    pub cwd: PathBuf,
    pub agent_type: Option<String>,
    pub model: String,
    pub permission_mode: PermissionMode,
    pub coordinator_mode: bool,
    pub enabled_plugins: Vec<PluginId>,
    pub mcp_servers_enabled: Vec<String>,
    pub working_directories: Vec<PathBuf>,
    pub current_output_style: String,
    pub claude_md_paths: Vec<PathBuf>,
    pub last_modified: SystemTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TranscriptEntry {
    Message { uuid: MessageId, timestamp: SystemTime, message: ConversationMessage },
    StreamEvent { request_id: RequestId, event: StreamEvent },
    ToolUseSummary { tool_use_id: ToolUseId, summary: String },
    CompactBoundary { boundary: CompactBoundaryMessage },
    Tombstone { replaced_uuid: MessageId, reason: TombstoneReason },
    HookResult { hook_id: HookId, result: HookResult },
    QueueOperation { op: QueueOperation },
    ContentReplacement { replacement: ReplacementRecord },
    SessionResumed { previous_session_id: SessionId, resumed_at: SystemTime },
}
```

### 22.3 SessionStorage Service

```rust
pub struct SessionStorage {
    base_dir: PathBuf,
    fs: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    runtime: Arc<dyn RuntimeSpawner>,
    pending_writes: Arc<Mutex<Vec<PendingWrite>>>,
    transcript_writer: Arc<Mutex<Option<TranscriptWriter>>>,
}

impl SessionStorage {
    pub async fn create(&self, metadata: SessionMetadata) -> Result<SessionHandle, StorageError>;
    pub async fn append(&self, session_id: &SessionId, entry: TranscriptEntry) -> Result<(), StorageError>;
    pub async fn save_metadata(&self, session_id: &SessionId, metadata: &SessionMetadata) -> Result<(), StorageError>;
    pub async fn list(&self, filter: SessionListFilter) -> Result<Vec<SessionMetadata>, StorageError>;
    pub async fn load(&self, session_id: &SessionId) -> Result<LoadedSession, StorageError>;
    pub async fn list_for_project(&self, project_dir: &Path, limit: usize) -> Result<Vec<SessionMetadata>, StorageError>;
    pub async fn record_sidechain(&self, parent_session: &SessionId, agent_id: &AgentId, entry: TranscriptEntry) -> Result<(), StorageError>;
    pub async fn close(&self, session_id: &SessionId) -> Result<(), StorageError>;
}

#[derive(Debug)]
pub struct LoadedSession {
    pub metadata: SessionMetadata,
    pub messages: Vec<ConversationMessage>,
    pub compact_boundaries: Vec<CompactBoundaryMessage>,
    pub content_replacements: ContentReplacementState,
    pub queue_operations: Vec<QueueOperation>,
    pub session_memory: Option<String>,
    pub plan: Option<String>,
}
```

### 22.4 Crash-Safe JSONL

```rust
/// Tolerant reader: parses line-by-line, truncates on corruption (last write interrupted)
pub struct CrashSafeJsonlReader {
    fs: Arc<dyn FileSystem>,
}

impl CrashSafeJsonlReader {
    pub async fn read_recover(&self, path: &Path) -> Result<RecoveryResult, StorageError>;
}

pub struct RecoveryResult {
    pub entries: Vec<TranscriptEntry>,
    pub truncated_at: u64,
}
```

### 22.5 SessionResumer (fix C7)

```rust
pub struct SessionResumer {
    storage: Arc<SessionStorage>,
    plugin_manager: Arc<PluginManager>,
    mcp_registry: Arc<RwLock<McpRegistry>>,
    permission_policy: Arc<RwLock<PermissionPolicy>>,
    cost_tracker: Arc<CostTracker>,
    output_style_registry: Arc<RwLock<OutputStyleRegistry>>,
    fs: Arc<dyn FileSystem>,
}

impl SessionResumer {
    pub async fn resume(&self, session_id: &SessionId) -> Result<ResumedSession, ResumeError> {
        // 1. Load metadata + transcript
        let loaded = self.storage.load(session_id).await?;
        // 2. Re-enable plugins
        for plugin_id in &loaded.metadata.enabled_plugins {
            self.plugin_manager.enable(plugin_id).await?;
        }
        // 3. Reconnect MCP servers
        for server_name in &loaded.metadata.mcp_servers_enabled {
            self.mcp_registry.write().await.reconnect_by_name(server_name).await?;
        }
        // 4. Restore permission mode
        self.permission_policy.write().await.set_mode(loaded.metadata.permission_mode);
        // 5. Restore cost state
        self.cost_tracker.restore(&loaded.metadata.session_id).await?;
        // 6. Restore output style
        self.output_style_registry.write().await.switch(&loaded.metadata.current_output_style).await?;
        // 7. Rebuild FileStateCache from historical ReadTool calls
        let file_state_cache = rebuild_file_state_cache(&loaded.messages, &*self.fs).await?;
        // 8. Apply content replacements
        let content_replacement = loaded.content_replacements;
        // 9. Filter to post-compact-boundary messages
        let messages = get_messages_after_compact_boundary(&loaded.messages);
        Ok(ResumedSession { metadata: loaded.metadata, messages, file_state_cache, content_replacement, session_memory: loaded.session_memory })
    }
}
```

---

## 23. File State Cache

### 23.1 FileStateCache

```rust
pub struct FileStateCache {
    cache: LruCache<NormalizedPath, FileState>,
    max_entries: usize,
    max_size_bytes: u64,
    current_size_bytes: AtomicU64,
}

#[derive(Debug, Clone)]
pub struct FileState {
    pub content: String,
    pub timestamp: SystemTime,
    pub offset: Option<u64>,
    pub limit: Option<u64>,
    /// True when entry was auto-injected (e.g. CLAUDE.md) and stripped/truncated
    /// before reaching the model. content holds RAW disk bytes; Edit must require explicit Read first.
    pub is_partial_view: bool,
}

pub const READ_FILE_STATE_CACHE_MAX_ENTRIES: usize = 100;
pub const READ_FILE_STATE_CACHE_MAX_BYTES: u64 = 25 * 1024 * 1024;
```

### 23.2 Verification (fix C5)

```rust
pub fn verify_file_state(
    cache: &FileStateCache,
    path: &str,
    current_disk_mtime: SystemTime,
    current_disk_hash: Option<[u8; 32]>,
) -> FileStateVerification {
    let Some(cached) = cache.get(path) else { return FileStateVerification::NotInCache; };
    if cached.is_partial_view { return FileStateVerification::PartialView; }
    if current_disk_mtime > cached.timestamp {
        return FileStateVerification::ModifiedSinceRead {
            cached_at: cached.timestamp,
            disk_mtime: current_disk_mtime,
        };
    }
    if let Some(disk_hash) = current_disk_hash {
        let cached_hash = sha256(cached.content.as_bytes());
        if disk_hash != cached_hash { return FileStateVerification::ContentMismatch; }
    }
    FileStateVerification::Valid
}

#[derive(Debug, Clone)]
pub enum FileStateVerification {
    Valid, NotInCache, PartialView,
    ModifiedSinceRead { cached_at: SystemTime, disk_mtime: SystemTime },
    ContentMismatch,
}
```

### 23.3 Edit Tool Integration

```rust
impl Tool for FileEditTool {
    async fn call(&self, input: Value, ctx: ToolUseContext, ...) -> Result<ToolCallResult, ToolError> {
        let path = input["path"].as_str()?;
        let disk_mtime = ctx.fs.file_mtime(path).await?;
        match verify_file_state(&ctx.file_state_cache, path, disk_mtime, None) {
            FileStateVerification::Valid => {}
            FileStateVerification::NotInCache => return Err(ToolError::EditWithoutRead { path: path.into() }),
            FileStateVerification::PartialView => return Err(ToolError::PartialViewMustReread { path: path.into() }),
            FileStateVerification::ModifiedSinceRead { .. } => return Err(ToolError::FileModifiedExternally { path: path.into() }),
            FileStateVerification::ContentMismatch => return Err(ToolError::FileContentMismatch { path: path.into() }),
        }
        // ... perform edit ...
        ctx.file_state_cache.set(path, FileState { content: new_content, timestamp: SystemTime::now(), ... });
        Ok(...)
    }
}
```

### 23.4 Merge & Clone

```rust
impl FileStateCache {
    pub fn clone_cache(&self) -> Self;
    /// More recent timestamp wins; used to merge subagent reads back to parent.
    pub fn merge(&mut self, other: &Self);
    /// Serialize for session persistence (§22)
    pub fn dump(&self) -> Vec<(String, FileState)>;
    pub fn load(&mut self, entries: Vec<(String, FileState)>);
}
```

---

## 24. Sandbox

### 24.1 Sandbox Trait

```rust
/// Hardening layer for ProcessRunner. Linux: unshare+seccomp; macOS: sandbox-exec;
/// Windows: Job Object + restricted token; Mobile: unsupported.
#[async_trait]
pub trait Sandbox: Send + Sync {
    fn is_available(&self) -> bool;
    fn backend(&self) -> SandboxBackend;
    fn wrap_command(&self, cmd: ProcessCommand, policy: &SandboxPolicy) -> Result<ProcessCommand, SandboxError>;
    async fn probe_capability(&self) -> SandboxCapability;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxBackend {
    LinuxNamespaces, LinuxFirejail, MacOsSandboxExec, WindowsJobObject, None,
}

#[derive(Debug, Clone)]
pub struct SandboxCapability {
    pub available: bool,
    pub reason: Option<String>,
    pub features_supported: SandboxFeatures,
}

#[derive(Debug, Clone, Default)]
pub struct SandboxFeatures {
    pub network_isolation: bool,
    pub fs_readonly: bool,
    pub fs_readwrite_paths: bool,
    pub process_limit: bool,
    pub no_new_privileges: bool,
}
```

### 24.2 SandboxPolicy

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxPolicy {
    pub network: NetworkPolicy,
    pub writable_paths: Vec<PathBuf>,
    pub denied_paths: Vec<PathBuf>,
    pub allow_subprocess: bool,
    pub limits: ResourceLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkPolicy { Disabled, LoopbackOnly, Allowed }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceLimits {
    pub max_cpu_seconds: Option<u32>,
    pub max_memory_mb: Option<u32>,
    pub max_processes: Option<u32>,
    pub max_open_files: Option<u32>,
}
```

### 24.3 should_use_sandbox Decision

```rust
pub fn should_use_sandbox(
    cmd: &str,
    permission_mode: PermissionMode,
    project_trust: ProjectTrustLevel,
    classifier_result: Option<&ClassifierScore>,
) -> SandboxDecision { ... }

#[derive(Debug, Clone)]
pub enum SandboxDecision {
    NoSandbox,
    Sandbox { policy: SandboxPolicy },
    RefuseBecauseSandboxUnavailable { reason: String },
}
```

---

## 25. LSP Integration

### 25.1 LSP Server Config

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub trigger_languages: Vec<String>,
    pub root_dir_markers: Vec<String>,
    pub initialization_options: Option<Value>,
}
```

### 25.2 LSP Connection State

```rust
#[derive(Debug, Clone)]
pub enum LspConnectionState {
    Disconnected { config: LspServerConfig },
    Starting { config: LspServerConfig, started_at: SystemTime, pid: u32 },
    Initialized {
        config: LspServerConfig,
        connection_id: LspConnectionId,
        server_capabilities: LspServerCapabilities,
        pid: u32,
    },
    Failed { config: LspServerConfig, error: String },
    Stopped { config: LspServerConfig },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspServerCapabilities {
    pub text_document_sync: TextDocumentSyncKind,
    pub completion: bool,
    pub hover: bool,
    pub definition: bool,
    pub references: bool,
    pub diagnostics: bool,
    pub symbols: bool,
    pub formatting: bool,
    pub rename: bool,
    pub code_action: bool,
}
```

### 25.3 LspTransport & Registry

```rust
#[async_trait]
pub trait LspTransport: Send + Sync {
    async fn start_server(&self, config: &LspServerConfig) -> Result<LspRawConnection, LspError>;
    async fn initialize(&self, conn: &LspRawConnection, root_uri: &str) -> Result<LspServerCapabilities, LspError>;
    async fn request(&self, conn: &LspRawConnection, method: &str, params: Value) -> Result<Value, LspError>;
    async fn notify(&self, conn: &LspRawConnection, method: &str, params: Value) -> Result<(), LspError>;
    async fn shutdown(&self, conn_id: LspConnectionId) -> Result<(), LspError>;
    fn is_available(&self) -> bool;
}

pub struct LspRegistry {
    servers: HashMap<String, LspConnectionState>,
    file_route_cache: Arc<RwLock<HashMap<PathBuf, String>>>,
    transport: Arc<dyn LspTransport>,
    plugin_servers: HashMap<PluginId, Vec<String>>,
}

impl LspRegistry {
    pub async fn ensure_server_for_file(&self, path: &Path) -> Result<LspConnectionId, LspError>;
    pub async fn dispatch(&self, action: LspAction, path: &Path, ...) -> Result<LspResponse, LspError>;
    pub fn register_plugin_servers(&mut self, plugin_id: PluginId, configs: Vec<LspServerConfig>);
    pub fn unregister_plugin(&mut self, plugin_id: &PluginId) -> Vec<LspConnectionId>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LspAction {
    Hover { line: u32, character: u32 },
    Definition { line: u32, character: u32 },
    References { line: u32, character: u32 },
    Diagnostics,
    Symbols { query: Option<String> },
    Completion { line: u32, character: u32 },
    Formatting,
    Rename { line: u32, character: u32, new_name: String },
}
```

### 25.4 LspTool

```rust
pub struct LspTool {
    registry: Arc<RwLock<LspRegistry>>,
}

impl Tool for LspTool {
    fn name(&self) -> &str { "LSP" }
    fn is_lsp(&self) -> bool { true }
    async fn call(&self, input: Value, ctx: ToolUseContext, ...) -> Result<ToolCallResult, ToolError> {
        let action = parse_action(&input)?;
        let path = input["path"].as_str()?;
        let response = self.registry.read().await.dispatch(action, Path::new(path), ...).await?;
        Ok(ToolCallResult { data: serde_json::to_value(&response)?, ... })
    }
}
```

---

## 26. Telemetry & Analytics

### 26.1 AnalyticsSink Trait

```rust
#[async_trait]
pub trait AnalyticsSink: Send + Sync {
    async fn log_event(&self, name: &str, metadata: LogEventMetadata);
    async fn log_event_async(&self, name: &str, metadata: LogEventMetadata);
    fn name(&self) -> &str;
}

pub type LogEventMetadata = HashMap<String, AnalyticsValue>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AnalyticsValue {
    Bool(bool), Int(i64), Float(f64), String(String), None,
}
```

### 26.2 PII Marker Types

```rust
/// Compile-time marker requiring explicit verification of non-sensitive content
pub type AnalyticsMetadata_Verified = String;
pub type AnalyticsMetadata_PiiTagged = String;

/// _PROTO_* prefix routes to privileged BQ proto columns; stripped from
/// general-access sinks (Datadog etc.).
pub fn strip_proto_fields(metadata: &mut LogEventMetadata) {
    metadata.retain(|k, _| !k.starts_with("_PROTO_"));
}
```

### 26.3 AnalyticsBus

```rust
pub struct AnalyticsBus {
    sink: Arc<RwLock<Option<Arc<dyn AnalyticsSink>>>>,
    pending: Arc<Mutex<VecDeque<QueuedEvent>>>,
    max_pending: usize,
    killswitch_active: Arc<AtomicBool>,
}

impl AnalyticsBus {
    pub fn log_event(&self, name: &str, metadata: LogEventMetadata);
    pub async fn attach_sink(&self, sink: Arc<dyn AnalyticsSink>);
    pub async fn activate_killswitch(&self);
}
```

### 26.4 GrowthBook Feature Flags

```rust
pub struct FeatureFlagsClient {
    cache: Arc<RwLock<HashMap<String, FeatureValue>>>,
    cache_ttl: Duration,
    fetcher: Arc<dyn FeatureFlagsFetcher>,
    runtime: Arc<dyn RuntimeSpawner>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FeatureValue {
    Bool(bool), Number(f64), String(String), Json(Value),
}

#[async_trait]
pub trait FeatureFlagsFetcher: Send + Sync {
    async fn fetch(&self) -> Result<HashMap<String, FeatureValue>, FeatureFlagsError>;
}

impl FeatureFlagsClient {
    /// Cached-may-be-stale getter, analogous to claude-code's *_CACHED_MAY_BE_STALE
    pub fn get_value(&self, key: &str, default: FeatureValue) -> FeatureValue;
    pub fn get_bool(&self, key: &str, default: bool) -> bool;
    pub fn get_json<T: DeserializeOwned>(&self, key: &str, default: T) -> T;
    pub async fn start_refresh_loop(&self) -> Result<(), FeatureFlagsError>;
}
```

---

## 27. Message Queue Manager

### 27.1 Unified Command Queue

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedCommand {
    pub uuid: String,
    pub content: QueuedCommandContent,
    pub priority: QueuePriority,
    pub queued_at: SystemTime,
    pub source: QueueSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueuedCommandContent {
    UserInput { text: String, attachments: Vec<Attachment> },
    SlashCommand { parsed: ParsedSlashCommand },
    TaskNotification { value: String, mode: NotificationMode },
    TeammateMessage { from: AgentId, message: TeammateMessage },
    OrphanedPermission { tool_use_id: ToolUseId, request: PermissionRequest },
    HookInjected { content: String, hook_id: HookId },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum QueuePriority { Now, Next, Later }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueSource {
    PromptInput, TaskCompletion, AgentSendMessage, Hook, Orphan, Cron,
}
```

### 27.2 MessageQueueManager

```rust
pub struct MessageQueueManager {
    queue: Arc<RwLock<VecDeque<QueuedCommand>>>,
    snapshot: Arc<RwLock<Arc<Vec<QueuedCommand>>>>,
    notify: Arc<Notify>,
    operations_log: Arc<dyn QueueOperationsLog>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueueOperation {
    Enqueue { uuid: String, priority: QueuePriority, source: QueueSource },
    Dequeue { uuid: String },
    Remove { uuid: String, reason: String },
    Clear { count: usize },
}

impl MessageQueueManager {
    pub async fn enqueue(&self, cmd: QueuedCommand);
    pub async fn dequeue(&self) -> Option<QueuedCommand>;
    pub async fn drain_now_priority(&self) -> Vec<QueuedCommand>;
    pub async fn remove(&self, uuid: &str, reason: &str);
    pub async fn snapshot(&self) -> Arc<Vec<QueuedCommand>>;
    pub async fn wait_for_message(&self, timeout: Duration) -> Option<QueuedCommand>;
}
```

---

## 28. Cron Scheduler

### 28.1 Scheduler

```rust
pub struct CronScheduler {
    registry: Arc<RwLock<CronTaskRegistry>>,
    task_registry: Arc<TaskRegistry>,
    fs: Arc<dyn FileSystem>,
    clock: Arc<dyn Clock>,
    runtime: Arc<dyn RuntimeSpawner>,
    lock_dir: PathBuf,
    jitter_seconds: u32,
    tick_task: Mutex<Option<BackgroundTaskHandle>>,
}

impl CronScheduler {
    pub async fn start(&self) -> Result<(), CronError>;
    pub async fn stop(&self) -> Result<(), CronError>;
}
```

### 28.2 Cross-Process Lock (fix C6)

```rust
pub struct CronTasksLock {
    lock_path: PathBuf,
    fs: Arc<dyn FileSystem>,
}

impl CronTasksLock {
    /// Stale-lock detection: if existing lock mtime > 60s old, override.
    pub async fn acquire(path: &Path, fs: &dyn FileSystem, now: SystemTime) -> Result<Self, CronError>;
}
```

### 28.3 Tick Loop & Dispatch

Cron tick runs every minute (`next_minute_boundary`). For each due task:
1. Apply jitter sleep (up to `jitter_seconds`)
2. Acquire cross-process lock (`CronTasksLock::acquire`)
3. If acquired, call `task_registry.create(cron_task.target_task_type(), ...)` to spawn the actual task via §11 TaskManager
4. Mark `last_run` in registry
5. On lock failure, skip (another claude process took it)

---

## 29. IDE Bridge

### 29.1 Bridge Protocol

```rust
pub struct IdeBridge {
    transport: Arc<dyn BridgeTransport>,
    pairing: Arc<BridgePairing>,
    state: Arc<RwLock<BridgeState>>,
    runtime: Arc<dyn RuntimeSpawner>,
}

#[async_trait]
pub trait BridgeTransport: Send + Sync {
    async fn connect(&self, config: &BridgeConfig) -> Result<BridgeConnection, BridgeError>;
    async fn send(&self, conn: &BridgeConnection, message: BridgeMessage) -> Result<(), BridgeError>;
    async fn receive(&self, conn: &BridgeConnection) -> Result<Box<dyn Stream<Item = BridgeMessage> + Send + Unpin>, BridgeError>;
    async fn disconnect(&self, conn: BridgeConnection) -> Result<(), BridgeError>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeConfig {
    pub bridge_url: String,
    pub jwt_token: Secret<String>,
    pub poll_interval_ms: u32,
    pub trusted_device_id: String,
}
```

### 29.2 Bridge Messages

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BridgeMessage {
    // IDE → CLI
    UserPrompt { text: String, attachments: Vec<Attachment> },
    OpenFileRequest { path: PathBuf, line: Option<u32> },
    GetCurrentFileResponse { path: PathBuf, content: String },
    PermissionDecision { tool_use_id: ToolUseId, decision: PermissionUserDecision },
    AbortRequest,
    // CLI → IDE
    ShowDiff { path: PathBuf, old_content: String, new_content: String },
    ShowPermissionPrompt { tool_use_id: ToolUseId, action: String, risk: RiskLevel },
    UpdateStatus { status: BridgeStatus },
    AssistantMessage { text: String, role: MessageRole },
    ToolExecution { tool_use_id: ToolUseId, tool_name: String, status: ToolExecStatus },
    SessionEvent { event: SessionEvent },
    // Bidirectional
    Heartbeat { timestamp: SystemTime },
}
```

### 29.3 Trusted Device Pairing

```rust
pub struct BridgePairing {
    storage: Arc<dyn SecureStorage>,
    jwt_verifier: Arc<JwtVerifier>,
}

impl BridgePairing {
    /// Out-of-band pairing: CLI shows 6-digit code, user enters in IDE, exchange JWTs.
    pub async fn pair_device(&self) -> Result<TrustedDevice, BridgeError>;
    pub async fn list_devices(&self) -> Result<Vec<TrustedDevice>, BridgeError>;
    pub async fn revoke_device(&self, device_id: &str) -> Result<(), BridgeError>;
    pub fn verify_jwt(&self, token: &str) -> Result<JwtClaims, BridgeError>;
}

#[derive(Debug, Clone)]
pub struct TrustedDevice {
    pub device_id: String,
    pub name: String,
    pub paired_at: SystemTime,
    pub last_seen: SystemTime,
    pub jwt: Secret<String>,
}
```

---

## 30. Anthropic OAuth (extends §16 Secret)

§16 covered the generic Secret/Credential framework. §30 specializes for the Anthropic OAuth flow (login.claude.ai) and the multi-source authentication resolver.

### 30.1 Claude.ai OAuth Client

```rust
pub struct ClaudeAiOAuthClient {
    config: ClaudeAiOAuthConfig,
    http: Arc<dyn HttpTransport>,
    credential_manager: Arc<CredentialManager>,
    clock: Arc<dyn Clock>,
}

#[derive(Debug, Clone)]
pub struct ClaudeAiOAuthConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub revocation_endpoint: String,
    pub profile_endpoint: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokens {
    pub access_token: Secret<String>,
    pub refresh_token: Secret<String>,
    pub expires_at: SystemTime,
    pub subscription_type: SubscriptionType,
    pub account_info: AccountInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SubscriptionType { Free, Pro, Max, Team, Enterprise, Unknown }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountInfo {
    pub user_id: String,
    pub email: String,
    pub org_id: Option<String>,
    pub display_name: Option<String>,
}
```

### 30.2 Multi-Source Resolver

```rust
pub struct AnthropicAuthResolver {
    credential_manager: Arc<CredentialManager>,
    config_loader: Arc<ConfigLoader>,
    env_reader: Arc<dyn EnvReader>,
}

#[derive(Debug, Clone)]
pub enum AuthSource {
    EnvApiKey,
    EnvAuthToken,
    FileDescriptor,
    OAuthClaudeAi,
    StoredApiKey,
    SettingsApiKey,
    ApiKeyHelper { script_path: PathBuf },
    AwsBedrock,
    None,
}

impl AnthropicAuthResolver {
    /// Priority resolution; managed contexts (CCR/Claude Desktop) force OAuth.
    pub async fn resolve(&self) -> Result<AuthSource, AuthError>;
    pub async fn get_auth_for_request(&self) -> Result<RequestAuth, AuthError>;
}

#[derive(Debug, Clone)]
pub enum RequestAuth {
    ApiKey(Secret<String>),
    BearerToken(Secret<String>),
    AwsSigv4 { credentials: Secret<AwsCredentials>, region: String },
    None,
}
```

### 30.3 Token Refresh & Lifecycle

```rust
impl ClaudeAiOAuthClient {
    pub async fn start_refresh_loop(&self) -> Result<(), OAuthError>;
    pub async fn refresh_now(&self) -> Result<OAuthTokens, OAuthError>;
    pub async fn login_interactive(&self, port: u16) -> Result<OAuthTokens, OAuthError>;
    pub async fn logout(&self) -> Result<(), OAuthError>;
    pub async fn get_profile(&self) -> Result<AccountInfo, OAuthError>;
}
```

### 30.4 ClaudeAiLimitsTracker (complements §17 Cost)

Claude.ai subscriptions enforce a 5-hour rolling message-count window; this is server-side rate limit data parsed from `x-claudeai-*` response headers, separate from client-side cost budget.

```rust
pub struct ClaudeAiLimitsTracker {
    state: Arc<RwLock<ClaudeAiLimitsState>>,
    parser: Arc<ClaudeAiLimitsHeaderParser>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClaudeAiLimitsState {
    pub subscription_type: SubscriptionType,
    pub message_count_window: u32,
    pub message_limit_window: u32,
    pub window_resets_at: Option<SystemTime>,
    pub extra_usage_dollars: f64,
    pub last_updated: SystemTime,
}

impl ClaudeAiLimitsTracker {
    pub async fn update_from_response_headers(&self, headers: &HashMap<String, String>);
    pub async fn check_pre_api_call(&self) -> ClaudeAiLimitCheckResult;
}
```

---

## 31. Cross-System Event & Effect Additions

Sections 18-30 introduce additional canonical Event/Effect variants. They live in §5.2 / §5.3 (single source of truth); summarized here for navigation:

**Events**: `SlashCommandParsed`, `SlashCommandCompleted`, `SlashCommandRejected`, `CommandRegistryReloaded`, `OutputStyleSwitched`, `SessionCreated`, `SessionResumed`, `SessionClosed`, `TranscriptCorruptionRecovered`, `SidechainRecorded`, `CommandQueued`, `CommandDequeued`, `QueueDrained`, `LspServerStarted`, `LspServerFailed`, `BridgeConnected`, `BridgeDisconnected`, `BridgeMessageReceived`, `ClaudeAiLimitsUpdated`, `OAuthLoginCompleted`, `OAuthLogout`, `FeatureFlagsRefreshed`, `AnalyticsKillswitchActivated`.

**Effects**: `ExecuteSlashCommand`, `ReloadCommands`, `SwitchOutputStyle`, `AppendTranscript`, `SaveSessionMetadata`, `ResumeSession`, `ListSessions`, `CloseSession`, `RecordSidechain`, `EnqueueCommand`, `DrainQueueAtBoundary`, `RunSideQuery`, `RunForkedAgent`, `EnsureLspServerForFile`, `DispatchLsp`, `WrapSandbox`, `LogAnalyticsEvent`, `FetchFeatureFlags`, `ActivateAnalyticsKillswitch`, `ConnectBridge`, `DisconnectBridge`, `SendBridgeMessage`, `PairDevice`, `LoginClaudeAi`, `LogoutClaudeAi`, `RefreshClaudeAiToken`, `UpdateClaudeAiLimits`.

---

## 32. Verification Strategy

Three-layer verification: **Contract Tests** (trait contracts), **Property Tests** (invariants), and **Parity/Integration Tests** (behavioral fidelity).

### 32.1 Contract Tests (all traits)

Each trait has a companion contract test suite that **any platform implementation must pass**:

```rust
// crates/test-harness/src/contracts/

pub fn filesystem_contract_tests<F: FileSystem>(fs: &F) { /* basic R/W, edit, glob, grep, boundary, watch */ }
pub fn process_runner_contract_tests<P: ProcessRunner>(runner: &P) { /* exec, timeout, bg, kill */ }
pub fn http_contract_tests<H: HttpTransport>(http: &H) { /* req, sse, errors */ }
pub fn mcp_contract_tests<M: McpTransport>(mcp: &M) { /* connect, init, list, call, ping, disconnect */ }
pub fn worktree_contract_tests<W: WorktreeManager>(wt: &W) { /* create, list, cleanup, supported */ }
pub fn swarm_contract_tests<S: SwarmBackend>(sw: &S) { /* layout, panes, visibility */ }
pub fn runtime_contract_tests<R: RuntimeSpawner>(rt: &R) { /* spawn, sleep, cancel, no leaked tasks */ }
pub fn secure_storage_contract_tests<S: SecureStorage>(storage: &S) { /* store, retrieve, delete, list, roundtrip; is_encrypted reflects reality */ }
```

### 32.2 Property Tests (state machine + subsystem invariants)

```rust
// crates/test-harness/src/properties/

// State machine invariants
proptest! { #[test] fn history_length_monotonic(events in arb_event_sequence(1..100)) { ... } }
proptest! { #[test] fn token_usage_monotonic(events in arb_event_sequence(1..50)) { ... } }
proptest! { #[test] fn permission_before_execution(events in arb_event_sequence(1..100)) { ... } }
proptest! { #[test] fn terminated_is_final(reason in arb_termination_reason(), event in arb_event()) { ... } }
proptest! { #[test] fn session_state_roundtrip(state in arb_session_state()) { ... } }

// Compaction invariants
proptest! { #[test] fn compaction_reduces_or_equal_tokens(messages in arb_messages(50..500)) { ... } }
proptest! { #[test] fn post_compact_preserves_recent_assistant(messages in arb_messages(50..500)) { ... } }
proptest! { #[test] fn autocompact_circuit_breaker_terminates(failures in 1..10u32) { ... } }

// MCP invariants
proptest! { #[test] fn mcp_connection_state_transitions_valid(seq in arb_mcp_event_seq()) { ... } }
proptest! { #[test] fn agent_scoped_cleanup_removes_only_inline(setup in arb_mcp_setup()) { ... } }

// Tools invariants
proptest! { #[test] fn concurrency_safe_partition_preserves_order(calls in arb_tool_calls(1..20)) { ... } }
proptest! { #[test] fn tool_result_storage_recovers_full(content in arb_string(1..100_000)) { ... } }

// Hooks invariants
proptest! { #[test] fn hook_aggregate_priority_ordered(hooks in arb_hooks(1..10)) { ... } }
proptest! { #[test] fn ssrf_blocks_private_ips(ip in arb_private_ip()) { ... } }

// Agent invariants
proptest! { #[test] fn agent_pool_capacity_enforced(spawns in 1..20) { ... } }
proptest! { #[test] fn fork_subagent_preserves_system_prompt_bytes(parent in arb_subagent_ctx()) { ... } }

// Task invariants
proptest! { #[test] fn task_id_uniqueness(n in 1..1000usize) { ... } }
proptest! { #[test] fn terminal_task_rejects_messages(task in arb_terminal_task()) { ... } }

// Permission invariants
proptest! { #[test] fn deny_rule_wins_over_allow(rules in arb_conflicting_rules()) { ... } }
proptest! { #[test] fn higher_source_priority_wins(rules in arb_multi_source_rules()) { ... } }
proptest! { #[test] fn plan_mode_blocks_all_writes(write_call in arb_write_tool_call()) { ... } }
proptest! { #[test] fn denial_tracking_eventually_falls_back(seq in 1..20u32) { ... } }
proptest! { #[test] fn bypass_killswitch_overrides_bypass_mode(mode in arb_permission_mode()) { ... } }

// Secret invariants
proptest! { #[test] fn secret_debug_never_leaks(value in any::<String>()) {
    let s = Secret::new(value.clone());
    prop_assert!(!format!("{:?}", s).contains(&value));
    prop_assert!(!format!("{}", s).contains(&value));
}}
proptest! { #[test] fn redact_removes_all_matches(content in arb_content_with_secrets()) { ... } }
proptest! { #[test] fn oauth_refresh_under_lock_is_idempotent(concurrent in 2..10usize) { ... } }
proptest! { #[test] fn keychain_roundtrip(kind in arb_secret_kind(), bytes in any::<Vec<u8>>()) { ... } }

// Cost invariants
proptest! { #[test] fn cost_is_monotonic(events in arb_usage_sequence(1..100)) { ... } }
proptest! { #[test] fn cost_matches_pricing_catalog(usage in arb_usage(), model_ref in arb_model_ref()) { ... } }
proptest! { #[test] fn provider_usage_normalization_preserves_billable_tokens(native in arb_provider_usage()) { ... } }
proptest! { #[test] fn cache_savings_non_negative(usage in arb_usage(), pricing in arb_model_pricing()) { ... } }
proptest! { #[test] fn budget_halt_stops_subsequent_calls(budget in arb_budget()) { ... } }

// Plugin invariants
proptest! { #[test] fn install_uninstall_roundtrip(manifest in arb_plugin_manifest()) { ... } }
proptest! { #[test] fn components_cleanup_on_uninstall(plugin in arb_loaded_plugin()) { ... } }
proptest! { #[test] fn blocklist_prevents_load(blocked_id in arb_plugin_id()) { ... } }
proptest! { #[test] fn strict_plugin_only_blocks_user_defined(component in arb_plugin_component()) { ... } }
```

### 32.3 Subsystem Integration Tests

Per-subsystem integration scenarios using mock implementations:

| Subsystem | Integration scenarios |
|---|---|
| Memory | static injection, selector with mock LLM, prefetch timeout, team sync watcher |
| MCP | full lifecycle (connect → handshake → list → call → disconnect), OAuth flow, reconnection, agent-scoped cleanup |
| Tools | parallel partition, streaming exec, content replacement, ToolUseContext propagation |
| Hooks | 4 executor kinds, blocking/non-blocking, output protocol decisions, SSRF guard |
| Agent | spawn / multi-dispatch / mailbox / fork / worktree degradation |
| Tasks | 7 types lifecycle, disk output streaming, notification injection, cron triggers |
| Coordinator | TeamCreate → SendMessage → SyntheticOutput → completion |
| Compaction | 5 layers stacked, PTL retry, session memory dual extraction, post-compact restore |
| Permission | 5 external + 2 internal modes × representative rules, classifier paths, pending classifier race, denial fallback, shadow detection, bypass killswitch |
| Secret | keychain roundtrip, Secret<T> Debug/Display, 5 redaction boundaries, OAuth refresh under concurrency, prefetch consume |
| Cost | provider/model pricing catalog fixtures, provider usage normalization, cache savings, budget halt/ask/warn paths, persistence roundtrip |
| Plugin | install (git + marketplace + mcpb) → load → component materialization → disable → uninstall → cleanup; blocklist enforcement; strict policy |
| FFI | session facade, event/effect DTO roundtrip, Kotlin/Swift handle lifecycle |
| Parity | claw-code mock scenarios for tools, hooks, subagents, tasks, and compaction |

### 32.4 CI Pipeline (extended)

```yaml
jobs:
  # Layer 1: Zero-dependency compile check
  compile-check:
    - cargo check -p lingxi-protocol --no-default-features
    - cargo check -p lingxi-core --no-default-features
    - cargo check -p lingxi-traits

  # Layer 2: Unit tests
  unit-tests:
    - cargo test -p lingxi-protocol
    - cargo test -p lingxi-core
    - cargo test -p lingxi-permission
    - cargo test -p lingxi-secret
    - cargo test -p lingxi-cost
    - cargo test -p lingxi-memory
    - cargo test -p lingxi-mcp
    - cargo test -p lingxi-tools
    - cargo test -p lingxi-hooks
    - cargo test -p lingxi-agent
    - cargo test -p lingxi-tasks
    - cargo test -p lingxi-coordinator
    - cargo test -p lingxi-compaction
    - cargo test -p lingxi-plugin

  # Layer 3: Property tests at scale
  property-tests:
    - cargo test -p lingxi-test-harness --features 10k-iterations

  # Layer 4: Contract tests (mock implementations)
  contract-tests:
    - cargo test -p lingxi-test-harness --test contract_*

  # Layer 5: Cross-compile verification (5 targets)
  cross-compile:
    matrix:
      target: [aarch64-linux-android, aarch64-apple-ios, x86_64-pc-windows-msvc, x86_64-unknown-linux-gnu, aarch64-apple-darwin]
    steps:
      - cross build --target ${{ matrix.target }} -p lingxi-protocol
      - cross build --target ${{ matrix.target }} -p lingxi-core
      - cross build --target ${{ matrix.target }} -p lingxi-traits
      - cross build --target ${{ matrix.target }} -p lingxi-api-client
      - cross build --target ${{ matrix.target }} -p lingxi-uniffi-bridge

  # Layer 6: Lint
  lint:
    - cargo clippy --workspace --all-targets -- -D warnings
    - cargo fmt --all -- --check

  # Layer 7: Contract coverage + parity gates
  coverage-and-parity:
    - cargo run --bin contract-coverage-checker -- --max-unexercised-ratio 0.05
    - cargo test -p lingxi-test-harness --test parity_*
```

### 32.5 Contract Coverage Metric

```rust
pub fn compute_unexercised_trait_method_ratio() -> f64 {
    let total_methods = count_trait_methods();      // all trait methods across §4 + subsystems
    let covered_methods = count_covered_methods();  // methods exercised by contract tests
    1.0 - (covered_methods as f64 / total_methods as f64)
}
```

CI gate: `unexercised_trait_method_ratio <= 0.05`. This is a coverage guard, not a fidelity proof; parity fixtures and subsystem integration tests are the fidelity gates.

---

## 33. M1 Deliverables (Updated for Full Scope)

| # | Crate / Deliverable | Definition | Completion Criteria |
|---|---|---|---|
| D1 | `lingxi-protocol` | shared IDs, DTOs, effect envelopes, effect results | `cargo check --no-default-features` passes; no dependency on `core`, `traits`, runtime, or OS APIs |
| D2 | `lingxi-core` | state machine + reducer + session model | `cargo check --no-default-features` passes; no OS deps; reducer replay is deterministic |
| D3 | `lingxi-traits` | 13 traits (incl. SecureStorage) + capability system + runtime spawner | Full rustdoc; all traits async-trait compliant; no dependency on `core` |
| D4 | `lingxi-api-client` | Anthropic + OpenAI-compat + SSE | Stream parser passes proptest; depends only on `HttpTransport` for network I/O |
| D5 | `lingxi-memory` | 4-tier model + selector + prefetch + session/team | Selector + prefetch with mock LLM works end-to-end; no direct runtime spawn |
| D6 | `lingxi-mcp` | 7 transport specs + lifecycle + OAuth + approval | All 7 transport state machines validated with mocked `McpTransport` |
| D7 | `lingxi-tools` | Tool trait + registry + dispatcher + streaming + storage | Concurrency partition correctness; streaming exec works |
| D8 | `lingxi-hooks` | 28 events + 4 executors + async registry + SSRF | All 4 executor kinds verified; SSRF guard blocks private CIDRs |
| D9 | `lingxi-agent` | Context builder + pool + multi-dispatch + fork + memory snapshot + color | All §10 cross-system integrations verified through `RuntimeSpawner` |
| D10 | `lingxi-tasks` | 7 task types + registry + output manager + notifications + cron | Each task type passes lifecycle tests; cron uses injected clock/runtime |
| D11 | `lingxi-coordinator` | Mode + internal tools + team registry + mailbox + swarm + team memory | Coordinator → TeamCreate → SendMessage → completion verified |
| D12 | `lingxi-compaction` | 5 layers + reactive + PTL retry + session memory + post-compact | Property tests for token monotonicity + circuit breaker |
| D13 | `lingxi-permission` | 5 external + 2 internal modes + 3 classifiers + 8 rule sources + pending classifier checks + denial tracking + shadow detection | Authorize path tested under all runtime-valid modes; classifier mock returns deterministic; pending classifier can resolve prompt; denial-fallback triggers correctly |
| D14 | `lingxi-plugin` | manifest/component model + 7 lifecycle states + component registry materialization + marketplace + blocklist + strict policy | install/enable/disable/uninstall round-trip; commands/agents/skills/hooks/output-styles/MCP/LSP/channels reach correct registries; blocklist prevents load |
| D15 | `lingxi-secret` | SecureStorage backends + CredentialManager + keychain prefetch + 30+ gitleaks rules + redaction boundaries over protocol `Secret<T>` DTOs | All 5 redaction boundaries verified; `Secret<T>`/`SecureStorageData` debug output redacts; OAuth refresh races resolved under lock |
| D16 | `lingxi-cost` | provider/model pricing catalog + normalized token Usage + cache savings calc + per-session CostTracker + BudgetEnforcer | Cost reconciles within 0.1% against catalog fixtures for fixed provider/model/token usage; provider usage normalization and budget halt/ask/warn trigger correctly |
| D17 | `lingxi-skills` | Skill model + registry + discovery prefetch + SkillTool + mcp_skill_builders | Bundled+user+project+plugin+mcp skills load; trigger-keyword discovery works; SkillTool dispatches via ForkedAgentRunner |
| D18 | `lingxi-commands` | SlashCommand model + 80+ builtin handlers + markdown loader + arg substitution + plugin/MCP sources | All 80+ builtins parse + handle; markdown commands with frontmatter work; arg substitution covers $1/$ARGUMENTS/$@ |
| D19 | `lingxi-sidequery` | SideQueryClient + ForkedAgentRunner + CacheSafeParams slot | Memory selector, classifier explainer, and compaction summarization all route through these; cache hit verified |
| D20 | `lingxi-outputstyles` | OutputStyle model + registry + prompt addendum injection | `/output-style` switches active; addendum appears in next system prompt assembly |
| D21 | `lingxi-session` | SessionStorage + transcript JSONL + crash-safe reader + SessionResumer | Append/load roundtrip + corruption recovery + resume restores plugins/MCP/permission/cost/file-state |
| D22 | `lingxi-filestate` | FileStateCache + verify_file_state + Edit-Read coordination + merge/clone | All 5 FileStateVerification variants tested; Edit refuses with correct error on each |
| D23 | `lingxi-sandbox` | Sandbox trait + 4 backend stubs + SandboxPolicy + should_use_sandbox | Linux namespaces probe + sandbox-exec profile + Windows Job Object stubs compile; refuse-when-unavailable path tested |
| D24 | `lingxi-lsp` | LspTransport trait + LspRegistry + LspTool + per-conn state machine | mock LSP transport drives all LspAction variants; auto-route by file extension works |
| D25 | `lingxi-telemetry` | AnalyticsSink trait + AnalyticsBus + FeatureFlagsClient + PII markers + _PROTO_ strip | Pending events drain to attached sink; killswitch halts cleanly; FeatureFlagsClient cache + background refresh works |
| D26 | `lingxi-msgqueue` | MessageQueueManager + 6 QueuedCommandContent variants + 3-tier priority + ops log | Priority + FIFO drained correctly; tool-round boundary drain works; queue ops persist for crash recovery |
| D27 | `lingxi-cron` | CronScheduler + CronTasksLock + tick loop + jitter | Cron fires at minute boundary; cross-process lock prevents duplicate runs; jitter spreads thundering herd |
| D28 | `lingxi-bridge` | BridgeTransport trait + IdeBridge + JWT pairing + 9 BridgeMessage variants | Mock transport drives full pair/connect/send/receive/disconnect; JWT verify rejects tampered tokens |
| D29 | `lingxi-anthropic-oauth` | ClaudeAiOAuthClient + AnthropicAuthResolver + 9 AuthSource variants + ClaudeAiLimitsTracker | login_interactive completes via mock IdP; resolver picks correct source under priority rules; limits parsed from response headers |
| D30 | `lingxi-test-harness` | mocks + contract suites + property tests + parity fixtures | Contract coverage ≤ 0.05 unexercised trait-method ratio; property tests at 10K iterations |
| D31 | `lingxi-uniffi-bridge` | FFI-safe facade, DTOs, and opaque engine handles | Generated Kotlin + Swift compile; no raw `dyn Trait`, stream, closure, or `Arc<dyn Tool>` exposed |
| D32 | Cross-compile CI | 5 targets green | `protocol`, `core`, `traits`, `api-client`, and `uniffi-bridge` compile on supported target matrix |
| D33 | `platforms/posix-minimal` + `examples/cli-demo` | M1 demo host with minimal FS/process/http/MCP mocks + plain-text SecureStorage + stub IDE bridge | Multi-turn conversation + mock tool use + mock subagent + compact + permission prompt + cost display + slash command + skill discovery + file state verify works; production POSIX remains M2 |

---

## 34. M1 Milestone Schedule (Full Scope)

Timeline reflects full subsystem implementation. Single-developer estimate; multi-developer (autonomous claws) can compress significantly.

```
PHASE A: FOUNDATION (Weeks 1-4)
M1.1 (W1-2): Traits + Core Scaffold
├── lingxi-protocol complete (shared IDs, DTOs, effect envelopes)
├── lingxi-traits complete (13 traits + capabilities + RuntimeSpawner + SecureStorage)
├── lingxi-core SM skeleton (Idle / AssemblingPrompt / AwaitingApiResponse)
├── CI: compile-check + lint + cross-compile (5 targets)
└── Gate: cargo check all green, cross-compile passes

M1.2 (W3-4): API + Streaming
├── lingxi-api-client (Anthropic + OpenAI-compat + SSE)
├── SM expansion: StreamingResponse + ApiStreamEnd handling
├── Mock HttpTransport + contract tests
├── Property tests: token monotonic, session roundtrip
└── Gate: mock-mode single-turn conversation reducer passes

PHASE B: SECURITY + COST FOUNDATIONS (Weeks 5-8)
M1.3 (W5-6): Permission Policy Engine
├── 5 external + 2 internal PermissionMode + 8 RuleSource + PermissionRule/Result/Update
├── PermissionPolicy + 3 classifiers (Yolo / Bash / Transcript) with mocks
├── Pending classifier checks + denial tracking + shadow detection + bypass killswitch
└── Gate: authorize() returns correct result under all runtime-valid modes; pending classifier and denial fallback trigger

M1.4 (W7): Secret & Credential Management
├── SecureStorage trait + 7 backend variants (5 M1 stubs: Keychain / Libsecret / CredVault / EncryptedFile / PlainText; mobile implementations M2)
├── protocol Secret<T> newtype + CredentialManager + OAuth refresh-lock
├── KeychainPrefetch + SecretScanner with 30+ gitleaks rules + RedactionPolicy
└── Gate: Secret<T>::Debug returns "<redacted>"; all 5 redaction boundaries verified; OAuth refresh race-safe

M1.5 (W8): Cost & Budget
├── provider/model pricing catalog + normalized token Usage + CostCalculator (incl. cache savings)
├── CostTracker (per-provider/model, per-session, persisted) + BudgetEnforcer (halt/ask/warn)
├── Integration with reducer for token usage event flow
└── Gate: cost reconciles within 0.1% vs catalog fixtures; provider usage normalization and budget halt/ask/warn trigger correctly

PHASE C: CORE SUBSYSTEMS (Weeks 9-22)
M1.6 (W9-11): Tools System
├── Tool trait (30+ methods) + ToolUseContext
├── ToolRegistry + ToolDispatcher + concurrency partition
├── StreamingToolExecutor + ToolResultStorage + ContentReplacement
├── Integration with PermissionPolicy + per-tool permission matchers
└── Gate: multi-tool agentic loop reducer passes; concurrency partition correctness

M1.7 (W12-13): Hooks System
├── 28 HookEvent variants + HookDefinition + 4 executors
├── HookRegistry (multi-source) + HookExecutor + AsyncHookRegistry
├── SSRF Guard + 3 builtin handlers
└── Gate: Pre/PostToolUse hook integration; output decisions affect dispatcher

M1.8 (W14-16): Memory System
├── 4-tier MemoryTier + MemorySelector (LLM side-query) + MemoryPrefetch
├── SessionMemoryExtractor + TeamMemoryWatcher + SecretScanner integration
├── FileSystem trait extended with watch
└── Gate: prefetch parallel with API call; team memory watcher detects writes; secrets blocked from team upload

M1.9 (W17-19): MCP Lifecycle
├── 7 McpTransportSpec variants + per-conn state machine
├── McpRegistry + OAuth flow (CredentialManager integration) + ApprovalPolicy
├── Agent-scoped connections + cleanup
└── Gate: full lifecycle works; OAuth tokens stored via SecureStorage

M1.10 (W20-22): Compaction Engine
├── 5 layers + ReactiveCompactor + PTL retry
├── SessionMemoryCompactor dual extraction + PostCompactBuilder
└── Gate: token monotonicity + circuit breaker; PTL retry recovery

PHASE D: AGENT + TASKS + COORDINATOR (Weeks 23-32)
M1.11 (W23-26): Agent / Subagent
├── AgentDefinition + SubagentContext + StateMachinePool (effect delegation)
├── MultiAgentDispatcher + ForkSpawner + AgentMemorySnapshot + ColorManager
├── Worktree multi-platform degradation
├── Cross-system integration (Tools + MCP + Hooks + Memory + Permission)
└── Gate: subagent spawn through sibling SM slots; multi-agent dispatch routes to coordinator mailbox

M1.12 (W27-29): Task Manager
├── 7 TaskType + polymorphic TaskState + TaskRegistry + handlers
├── TaskOutputManager + disk persistence + TaskNotificationBuilder
├── CronTaskRegistry
└── Gate: each task type passes lifecycle test; notification injection works

M1.13 (W30-32): Coordinator / Team
├── CoordinatorMode + 4 internal tools (TeamCreate/Delete/SendMessage/SyntheticOutput)
├── TeamRegistry + TeammateMailbox + MailboxRouter
├── SwarmBackend trait + tmux reference impl + TeamMemorySync
└── Gate: Coordinator → TeamCreate → SendMessage → SyntheticOutput → completion verified

PHASE E: USER-FACING + EXECUTION SUPPORT (Weeks 33-42)
M1.14 (W33-34): Side Query & Forked Agent
├── SideQueryClient + ForkedAgentRunner + CacheSafeParams slot
├── Refactor §6 MemorySelector and §13 Autocompactor to use these
├── Mock LLM impl for testing
└── Gate: byte-exact cache hit verified; concurrent forked agents isolated

M1.15 (W35-37): Skills + Slash Commands + Output Styles
├── lingxi-skills: registry + discovery prefetch + SkillTool + mcp_skill_builders
├── lingxi-commands: 80+ builtin handlers + markdown loader + arg substitution
├── lingxi-outputstyles: registry + prompt addendum
└── Gate: /skills /agents /memory /compact /resume work in cli-demo; SkillTool dispatches via ForkedAgent

M1.16 (W38-40): Session Storage + File State Cache + Message Queue
├── lingxi-session: append-only JSONL + metadata + crash-safe reader
├── SessionResumer integrating Plugin/MCP/Permission/Cost/FileStateCache
├── lingxi-filestate: verify_file_state + Edit integration + merge/clone
├── lingxi-msgqueue: priority queue + tool-round drain + ops log
└── Gate: /resume restores all subsystems; Edit refuses on stale state; queue priority correct

M1.17 (W41-42): Cron Scheduler
├── CronScheduler tick + cross-process lock + jitter
├── Integration with §11 TaskRegistry
└── Gate: scheduled tasks fire at minute boundary; lock prevents duplicates

PHASE F: INFRA + EXTERNAL (Weeks 43-50)
M1.18 (W43-44): Sandbox + LSP
├── Sandbox trait + Linux namespaces / sandbox-exec / Job Object stubs
├── SandboxPolicy + should_use_sandbox decision
├── LspTransport trait + LspRegistry + LspTool
└── Gate: Bash routes through Sandbox.wrap_command when appropriate; mock LSP drives all actions

M1.19 (W45-46): Telemetry + Anthropic OAuth
├── AnalyticsBus + AnalyticsSink + FeatureFlagsClient + killswitch
├── ClaudeAiOAuthClient + AnthropicAuthResolver + ClaudeAiLimitsTracker
└── Gate: events queue→drain when sink attaches; OAuth login_interactive works against mock IdP

M1.20 (W47-48): IDE Bridge
├── BridgeTransport trait + IdeBridge + JWT pairing
├── 9 BridgeMessage variants + heartbeat
└── Gate: pair→connect→send→receive→disconnect over mock transport; tampered JWT rejected

PHASE G: PLUGINS + INTEGRATION + POLISH (Weeks 49-60)
M1.21 (W49-52): Plugin System
├── PluginManifest + PluginSource (6 kinds) + PluginState (7 states)
├── PluginManager + component registry materialization
├── MarketplaceManager (official + 3rd-party) + reconciler
├── PluginBlocklist (static + remote) + StrictPluginOnlyPolicy
├── Integration with Command/Agent/Skill/Hook/OutputStyle/MCP/LSP/Channel registries
└── Gate: install/enable/disable/uninstall round-trip; components reach correct registries; blocklist enforced

M1.22 (W53-55): UniFFI + Cross-Crate Integration
├── lingxi-uniffi-bridge with narrow facade APIs
├── Kotlin + Swift binding generation + compilation
├── End-to-end test through bridge
├── platforms/posix-minimal for demo-only FS/process/http/MCP mocks + plain-text SecureStorage + stub IDE bridge
├── examples/cli-demo full implementation (incl. /resume, /skills, /output-style, sandboxed bash)
└── Gate: cli-demo end-to-end works incl. permission prompts + cost display + slash commands + skill discovery; UniFFI bindings compile on mobile targets

M1.23 (W56-58): Test Coverage + Parity
├── Contract tests for all 13 traits (incl. SecureStorage, Sandbox, LspTransport, BridgeTransport)
├── Property tests for all subsystems at 10K iterations
├── Subsystem integration tests incl. permission/secret/cost/plugin/skill/cmd/session/bridge/cron scenarios
├── Contract coverage metric + parity fixtures from claude-code reference
└── Gate: unexercised trait-method ratio ≤ 0.05; all property and parity tests pass

M1.24 (W59-60): Documentation + Release
├── Complete rustdoc for all public APIs
├── ARCHITECTURE.md (subsystem overview)
├── SECURITY.md (Secret<T>, redaction, SSRF, plugin trust model, sandbox model, JWT pairing)
├── CONTRIBUTING.md + CHANGELOG.md + README.md
└── Gate: docs render correctly; M1 v0.1.0 tag
```

**Total: ~60 weeks (single developer), 14-20 weeks with 3-5 autonomous claws (claw-code precedent)**.

---

## 35. Post-M1 Roadmap (Preview)

```
M2: Production Platform Crates (Weeks 37-50, 14 weeks)
├── lingxi-platform-posix (Linux + macOS) — all 12 traits implemented, replacing posix-minimal
├── lingxi-platform-windows — all 12 traits implemented
├── Bash execution + Linux namespaces / macOS sandbox-exec
├── MCP stdio transport + LSP process orchestration
├── Git worktree manager
└── tmux SwarmBackend implementation

M3: Mobile (Weeks 51-64, 14 weeks)
├── lingxi-platform-android (JNI via UniFFI Kotlin bindings)
├── lingxi-platform-ios (Swift via UniFFI bindings)
├── WebSocket McpTransport for mobile
├── Mobile-appropriate FileSystem (sandboxed)
├── Mobile capability constraints (ProcessRunner unavailable, Bash hidden)
└── React Native / Flutter bridge (optional)

M4: UI Layer (independent projects, parallel)
├── Terminal UI (crossterm/ratatui) for desktop
├── Native iOS UI (SwiftUI)
├── Native Android UI (Compose)
├── Web UI (WebAssembly + React)
└── Full feature parity with claude-code
```

---

## Appendix A: Platform Capability Matrix (extended)

| Capability | Linux | macOS | Windows | Android | iOS |
|---|---|---|---|---|---|
| FileSystem | std::fs | std::fs | std::fs | App sandbox | App sandbox |
| FileSystem.watch | inotify | FSEvents | ReadDirectoryChangesW | FileObserver | DispatchSource |
| ProcessRunner | fork/exec | fork/exec | CreateProcess | ❌ | ❌ |
| HttpTransport | reqwest | reqwest | reqwest | OkHttp/reqwest-android | URLSession/reqwest |
| McpTransport.stdio | ✅ | ✅ | ✅ | ❌ | ❌ |
| McpTransport.sse | ✅ | ✅ | ✅ | ✅ | ✅ |
| McpTransport.http | ✅ | ✅ | ✅ | ✅ | ✅ |
| McpTransport.websocket | ✅ | ✅ | ✅ | ✅ | ✅ |
| McpTransport.in_process | ✅ | ✅ | ✅ | ✅ | ✅ |
| WorktreeManager | ✅ git | ✅ git | ✅ git | ❌ | ❌ |
| SwarmBackend.tmux | ✅ | ✅ | ❌ (wezterm/wt fallback) | ❌ | ❌ |
| Sandbox | Linux namespaces | sandbox-exec | Job objects | OS app sandbox | OS app sandbox |
| Bash tool | ✅ | ✅ | PowerShell adapter | ❌ | ❌ |
| Read/Write/Edit | ✅ | ✅ | ✅ | ✅ (within sandbox) | ✅ (within sandbox) |
| Glob/Grep | ✅ | ✅ | ✅ | ✅ (within sandbox) | ✅ (within sandbox) |
| LSP tool | ✅ | ✅ | ✅ | ❌ | ❌ |
| OS notifications | notify-rust | NSUserNotification | toast | NotificationManager | UNUserNotification |
| SecureStorage | libsecret (or EncryptedFile) | Keychain (Security.framework) | Credential Vault | Keystore | Keychain |

---

## Appendix B: Dependency Budget

### lingxi-protocol (STRICT — zero OS deps)
```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
```

### lingxi-core (STRICT — zero OS deps)
```toml
[dependencies]
lingxi-protocol = { path = "../protocol" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
# NOTHING ELSE external.
```

### lingxi-traits
```toml
[dependencies]
lingxi-protocol = { path = "../protocol" }
async-trait = "0.1"
serde = { version = "1", features = ["derive"] }
futures-core = "0.3"
```

### lingxi-api-client / memory / mcp / tools / hooks / agent / tasks / coordinator / compaction / permission / secret / cost / plugin
```toml
[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-core = { path = "../core" }
lingxi-traits = { path = "../traits" }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tracing = "0.1"
async-trait = "0.1"
futures = "0.3"
# Subsystem-specific:
# memory: regex (frontmatter parse)
# mcp: url (URL parsing for transports)
# hooks: regex (pattern matching), url (SSRF check)
# tools: rand (id generation)
# tasks: rand (task id generation)
# compaction: (none additional)
# permission: regex (rule matching), aho-corasick (multi-pattern bash check)
# secret: regex (30+ gitleaks rules), zeroize (secure-erase Secret<T> on drop)
# cost: (none additional — pure arithmetic)
# plugin: regex (manifest validation), zip (mcpb bundle parsing), semver (dependency resolution)
# NO direct tokio, reqwest, std::fs, std::process usage.
# Background work uses RuntimeSpawner; network/files/processes use traits.
```

### lingxi-test-harness
```toml
[dev-dependencies]
proptest = "1"
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
# Mock impls use tokio for test runtime
```

### lingxi-uniffi-bridge
```toml
[dependencies]
lingxi-protocol = { path = "../protocol" }
lingxi-core = { path = "../core" }
# + selected facade-backed subsystem crates
uniffi = "0.28"

[build-dependencies]
uniffi = { version = "0.28", features = ["build"] }
```

---

## Appendix C: M1 LOC Estimate

| Crate | Estimated LOC | Test LOC |
|---|---|---|
| lingxi-protocol | 1,000 | 300 |
| lingxi-core | 4,000 | 1,500 |
| lingxi-traits | 1,800 | 400 |
| lingxi-api-client | 2,500 | 800 |
| lingxi-permission | 2,500 | 1,200 |
| lingxi-secret | 2,000 | 1,000 |
| lingxi-cost | 1,200 | 600 |
| lingxi-memory | 2,000 | 800 |
| lingxi-mcp | 3,000 | 1,200 |
| lingxi-tools | 2,500 | 1,000 |
| lingxi-hooks | 2,000 | 800 |
| lingxi-agent | 3,000 | 1,200 |
| lingxi-tasks | 2,500 | 1,000 |
| lingxi-coordinator | 1,500 | 600 |
| lingxi-compaction | 2,500 | 1,000 |
| lingxi-plugin | 2,800 | 1,200 |
| lingxi-skills | 1,500 | 700 |
| lingxi-commands | 4,000 | 1,500 |
| lingxi-sidequery | 1,200 | 600 |
| lingxi-outputstyles | 600 | 300 |
| lingxi-session | 3,000 | 1,500 |
| lingxi-filestate | 600 | 400 |
| lingxi-sandbox | 1,500 | 700 |
| lingxi-lsp | 2,000 | 900 |
| lingxi-telemetry | 1,500 | 600 |
| lingxi-msgqueue | 1,000 | 500 |
| lingxi-cron | 800 | 400 |
| lingxi-bridge | 2,000 | 900 |
| lingxi-anthropic-oauth | 1,500 | 700 |
| lingxi-test-harness | 1,800 | 7,000 |
| lingxi-uniffi-bridge | 1,200 | 300 |
| platforms/posix-minimal | 1,500 | 500 |
| examples/cli-demo | 800 | 0 |
| **Total** | **~59,500** | **~28,300** |

Grand total: **~88K Rust LOC** for full-fidelity M1.

(Comparison: claw-code current `main` = 92K LOC, but includes substantial duplication, dead code, and tests; the fresh implementation should be more compact.)

### Why ~88K LOC

The full parity audit (against claude-code's ~519K TS LOC) identified 30 subsystems in M1. Major LOC drivers beyond the original §6–§13 core:
- **lingxi-commands** (4K): 80+ builtin handlers + markdown loader + frontmatter parse + argument substitution + alias resolution
- **lingxi-session** (3K): append-only JSONL writer + crash-safe reader + SessionResumer integrating all subsystems + per-agent sidechain
- **lingxi-permission/plugin/agent** (2.5-3K each): full claude-code parity (modes/classifiers/component materialization/effect delegation)
- **lingxi-bridge / lingxi-lsp / lingxi-anthropic-oauth / lingxi-sandbox** (1.5-2K each): protocol + transport + per-conn state machines
- **test-harness** (1.8K + 7K tests): contract suites for all 13 traits + property tests + parity fixtures at 10K iterations
- Smaller cross-cutting crates (skills/outputstyles/sidequery/filestate/msgqueue/cron/telemetry/cost): 600-1500 LOC each

This estimate stays roughly half of claude-code's TS LOC despite full feature parity, because Rust's type system absorbs many TS validation/runtime checks at compile time.
