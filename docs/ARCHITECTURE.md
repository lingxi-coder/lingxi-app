# Architecture

LingXi Core is an event-sourced conversation engine split across 30 crates.
This document is a navigation aid; full design lives in
`docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`.

## Crate map

- `protocol` — shared DTOs, IDs, Effect/Event envelopes
- `core` — state machine, reducer, prompt assembly, session model
- `traits` — 13 platform abstraction traits
- `api-client` — Anthropic/OpenAI-compatible API + SSE
- `permission/secret/cost` — security & cost foundations (Plan 02)
- `tools/hooks` — execution + extension (Plan 03)
- `memory/mcp` — retrieval + tool surface (Plan 04)
- `compaction` — 5-layer compactor (Plan 05)
- `agent` — subagent runtime (Plan 06)
- `tasks/coordinator` — background work + multi-agent (Plan 07)
- `sidequery` — side LLM + forked agent infra (Plan 08)
- `skills/commands/outputstyles` — user-facing surface (Plan 09)
- `session/filestate/msgqueue` — persistence + caching (Plan 10)
- `cron` — scheduled tasks (Plan 11)
- `sandbox/lsp` — execution support (Plan 12)
- `telemetry/anthropic-oauth` — infra + main auth (Plan 13)
- `bridge` — IDE integration (Plan 14)
- `plugin` — manifest + 8-registry materialization (Plan 15)
- `uniffi-bridge` — FFI façade (Plan 16)
- `test-harness` — contracts + properties + parity (Plan 17)
- `platforms/posix-minimal` — M1 desktop demo host
- `examples/cli-demo` — M1 end-to-end demo

## Key flows

### Single-turn conversation
User input → `reduce(Idle, UserMessage)` → `Effect::SendApiRequest` →
HttpTransport → SSE stream → `Event::ApiStream*` → `Effect::RenderStreamDelta`
→ `Event::ApiStreamEnd` → `Idle`.

### Tool dispatch
Assistant tool_use block → `ToolUseReceived` → permission check (rules + classifier)
→ PreToolUse hook → tool.call() → PostToolUse hook → `Effect::ExecuteTool` result
→ tool_result message → next API turn.

### Compaction
Token estimate > threshold → orchestrator → micro/cached-micro/collapse →
autocompact via ForkedAgentRunner → PostCompactBuilder → boundary message in transcript.

### Subagent
AgentTool → SubagentContext built (Tools/MCP/Hooks/Memory/Permission inherited)
→ StateMachinePool::allocate → sibling slot runs its own reducer → SubagentEvent
to parent → tool_result.

## Cross-cutting concerns

- **No tokio::spawn outside runtime trait** — all background work via `RuntimeSpawner`.
- **No tokio::fs/std::fs in engine crates** — all I/O via `FileSystem` trait.
- **Secrets never logged** — `Secret<T>` debug is always `<redacted>`.
- **Sandbox is type-enforced** — `ProcessRunner::run` only accepts `SandboxedCommand`.
