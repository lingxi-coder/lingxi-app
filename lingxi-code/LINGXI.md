# LINGXI.md

This file provides guidance to LingXi (claude.ai/code) when working with code in this repository.

## Build & Test Commands

```bash
# Build the CLI binary (default workspace members)
cargo build

# Build a specific crate
cargo build -p llm-client
cargo build -p orchestrator

# Full project build including apps
cargo build --workspace

# Run all tests for a crate
cargo test -p llm-client

# Run a single test by name substring
cargo test -p llm-client -- vision_image

# Run a specific integration test file
cargo test -p llm-client --test client_route_test

# Run tests with output (for println/eperln debugging)
cargo test -p tui -- files::tests -- --nocapture

# Check compilation only (faster than build)
cargo check -p orchestrator

# Lint
cargo clippy --workspace

# Dependency architecture check (composition-root rules)
scripts/check-deps.sh
```

**Rust toolchain**: pinned to 1.82.0 (`rust-toolchain.toml`). Workspace edition 2021.

## Architecture Overview

LingXi is a multi-provider AI coding assistant with a Ratatui TUI, CLI, and desktop bridge server. It is a Rust port of claude-code with byte-parity fidelity.

### Crate Dependency Layers (strict composition-root architecture)

The crates form an inverted pyramid — libraries make no shipping choices, only the composition roots do. `scripts/check-deps.sh` enforces this at CI time.

1. **Foundation** — `protocol`, `branding`, `features` (pure data types, zero deps)
2. **Abstraction** — `traits`, `tool-api`, `skill-api`, `command-api` (interfaces, no impl deps)
3. **Engine libraries** — `orchestrator`, `llm-client`, `compaction`, `agent`, `session`, `memory`, `hooks`, `permission`, `cost`, `telemetry`, `mcp`, `sidequery`, `coordinator`, `workflow`
4. **Tool implementations** — `tools/file`, `tools/shell`, `tools/web`, `tools/agent`, `tools/lsp`, `tools/mcp`, `tools/skill`, `tools/worktree`, `tools/plan`, `tools/task`, `tools/meta`, `tools/cron`, `tools/ui`, `tools/team`, `tools/workflow`, `tools/mobile`, `tools/computer-use`, `tools/android-use`, `tools/ios-use`
5. **Platform adapters** — `platforms/posix`, `platforms/windows`, `platforms/ios`, `platforms/android`
6. **Composition roots (leaves)** — `apps/cli`, `apps/engine-desktop`, `apps/engine-mobile`, `apps/bridge-server`, `tui`, `tui-core`

Key invariants: tools must not depend on each other; platforms must not depend on tools/commands/apps; API crates (`tool-api`, `skill-api`, `command-api`) must not depend on any impl crate.

### Core Data Types

- `protocol::ConversationMessage` — User/Assistant/System messages with `ContentBlock` variants (Text, ToolUse, ToolResult, Thinking, Image, Document, RedactedThinking, ServerToolUse, ConnectorText, AdvisorToolResult)
- `protocol::ContentBlock::Image` carries an `ImageSource` (Base64 or Url). Images persist in `session.history` and must be stripped before sending to non-vision models.
- `llm_client::LlmRequest` / `LlmEvent` / `LlmResponse` — provider-neutral request/response types. `LlmRequest` is distinct from `protocol::ConversationMessage`; conversion happens in `llm-client/src/convert.rs`.
- `protocol` (shared DTOs) vs `llm-client` (provider-facing types): these are parallel hierarchies. `to_llm_messages()` bridges them.

### Main Loop Flow

The orchestrator's turn loop (`orchestrator/src/conversation.rs`) drives every conversation turn:

1. **Entry**: `run_turn_streaming()` → `try_run_turn_streaming()` (line 5545) for TUI; `run_turn_with_cancel()` for CLI/REPL
2. **Preflight**: assemble system prompt → append user message to `session.history` → fire `UserPromptSubmit` hook → build wire tools → inject per-turn reminders (date, output-style, plan-mode, skill-listing, conditional-rules, diagnostics, agent-listing, todo-reminder, memories)
3. **Model call**: `ProviderApiAdapter::stream()` → `ApiService::stream()` → `build_request()` → `drive_stream()` — retry loop with exponential backoff, AWS auth refresh, rate-limit header parsing
4. **SSE pump**: `pump_stream_inner()` (`streaming_loop.rs:381`) decodes `LlmEvent` frames via `dispatch_event()` (`sse/event_router.rs:77`). Tool uses start executing **mid-stream** via `StreamingToolExecutor::add_tool()` immediately on `ContentBlockStop(tool_use)`.
5. **Tool drain**: post-stream, drive all in-flight tools to completion, persist each tool_result as JSONL lines with UUID parentage, collect `ContextModifier`s.
6. **Loop disposition**: based on `stop_reason` — `end_turn` → Stop hooks → break; `tool_uses` → continue; `max_tokens` → recovery retry (3 attempts); malformed tool_use → nudge once.

### Provider / Model Resolution

`llm-client` owns the provider routing layer:
- `DefaultLlmClient::prepare()` resolves model aliases via `ModelRegistry`, validates capabilities, encodes via provider codec (AnthropicMessages/OpenAiChat/Gemini/OpenAiResponses/AzureOpenAi/VertexClaude/BedrockClaude).
- `validate_capabilities()` in `llm-client/src/protocol.rs` checks streaming/tools/reasoning/structured_output/vision/documents against the resolved route's `Capabilities`.
- Model definitions are in `llm-client/data/models-dev/` (JSON catalog with modalities, pricing, context limits).
- `prepare_at()` (in `client.rs`) soft-degrades reasoning and vision for non-capable models by stripping incompatible blocks before validation.

### Compaction (Context Compression)

`compaction/` is a standalone crate, wired into the orchestrator via `CompactionOrchestrator`:
- **Three-layer pipeline**: snip (drop oldest messages) → microcompact (clear stale large tool results) → autocompact (LLM-driven summarization via forked agent).
- Autocompact threshold: `effective_context_window − 13,000` buffer (hardcoded to 150k in production composition roots).
- `force_compact_with_cancel()` (`conversation.rs:2334`) handles manual `/compact`. Emits `emit_compaction_started()` / `emit_compaction_completed()` on the output stream.
- Boundary marker: `"Conversation compacted"` system message with `compactMetadata` (trigger, preTokens, preservedSegment).
- Post-compact file restoration: re-reads up to 5 most-recently-read files from disk (5000 tokens/file cap, 50000 total budget).

### TUI Architecture

- `tui-core` holds the backend-neutral state, render model, and message types.
- `tui` (`tui/src/`) is the Ratatui renderer: `app.rs` is the top-level event loop, `chat_widget.rs` is the main chat area, `composer.rs` is the input widget, `bottom_pane/` contains the completion popup (slash commands and @file).
- `@file` completion (`tui/src/files.rs`): when the fragment has no `/`, recursively walks the project tree (max depth 8) skipping `.git`/`target`/`node_modules`/`.lingxi`.

### Session Persistence

`session/` manages append-only JSONL transcripts (`~/.lingxi/projects/<slug>/<uuid>.jsonl`). Each message line has a UUID for chain-based parentage. Compaction boundaries are `type: "system"`, `subtype: "compact_boundary"` lines with `compactMetadata`.

### Key Crate Purposes

| Crate | Purpose |
|---|---|
| `llm-client` | Provider abstraction: routing, auth, codec, retry, rate-limit |
| `orchestrator` | Turn loop, tool dispatch, hooks, streaming, PTL recovery |
| `compaction` | Context compression (snip + micro + auto) |
| `agent` | Subagent spawner (`ForkedAgentRunner` for compaction/recap) |
| `session` | JSONL transcript persistence + crash-safe reader |
| `hooks` | Lifecycle hook registry (PreToolUse, PostToolUse, Stop, PreCompact, etc.) |
| `permission` | Tool permission gate with modes (default/acceptEdits/bypassPermissions/plan) |
| `protocol` | Shared DTOs: `ConversationMessage`, `ContentBlock`, IDs |
| `traits` | Public interfaces: `OutputStream`, `OrchestratorHandle`, `McpTransport` |
| `tool-api` | Tool registration, `ToolContext`, `BuiltinToolContext` |
| `mcp` | MCP client (Streamable HTTP + stdio transports) |
| `sidequery` | Forked-agent runner for compaction/recap (shares parent prompt cache) |
| `telemetry` | Tengu analytics events (structured, non-PII) |
| `cost` | Token usage tracking, pricing, `CostTracker` |

### Module Organization Conventions

- `#![forbid(unsafe_code)]` on every crate (workspace default is `deny`; individual crates harden to `forbid`).
- Tests live in `#[cfg(test)] mod tests` at the bottom of source files OR as `tests/*.rs` integration test files. Integration tests use crate-internal APIs re-exported through `test_support` modules.
- `test_support.rs` modules export mocks (`MockApiClient`, `MockOutputStream`, `NoOpPermissionGate`, `StaticMemoryProvider`) used across crate boundaries.
- Provider-specific code lives in `llm-client/src/providers/` (Anthropic, OpenAI, Gemini, OpenAI Responses, Azure, Vertex, Bedrock).
