# LingXi Core

Platform-agnostic Rust engine for an AI coding assistant with 1:1 behavioral
parity to claude-code (2026-03-31 TypeScript reference) on desktop OSes.
v0.6.0 (M5) completes the Execution Engine 全集: `ConversationOrchestrator`
batched turn loop, streaming SSE, interactive permission gate, Hooks 4-arm
runtime, byte-equivalent session JSONL, `--resume` session loading, 18
implemented slash commands, `lingxi-cli` binary, and stdio REPL mode.
v0.5.0 (M4) completed the Tools 全集 surface: **40 tools** (File ×5,
Search ×1, Shell ×4, Web ×2, Workflow ×5, Agent+Task ×8, Team ×2,
MCP+LSP ×5, System ×8) all wired with byte-aligned schemas, telemetry
events (`tengu_tool_*_{started,completed,failed}`), and permission gating.
The v0.4.0 (M3) engine surface — Settings/Memory/API client/OAuth/cost
events/telemetry, 238 events — remains intact underneath.

## Quickstart

```bash
cargo build --workspace --release

# Demo against the production posix platform (real HTTP/SSE, real MCP,
# real sandbox, real Keychain on macOS).
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin lingxi-demo -- \
    --model claude-opus-4-7 \
    --platform posix
```

## Platform support

| OS | Status |
|---|---|
| macOS 13+ | Full support (Keychain, sandbox-exec, tmux/iTerm) |
| Linux | Full support (bubblewrap + socat sandbox; plaintext SecureStorage) |
| WSL2 | Full support (same as Linux) |
| Windows 10 22H2+ | Limited (no sandbox, no tmux; LSP/MCP/worktree work) |
| WSL1 | Sandbox refused at init |
| Android / iOS | M4 (not v0.4.0) |

All three Tier-1 platforms (macOS / Linux / WSL2) run the M3 engine
subsystems (Settings, Memory, API client, OAuth refresh, cost events,
telemetry schema) identically. See `docs/PLATFORMS.md` for the per-OS
setup notes + the "M3 engine subsystems" section.

## Subsystem status (v0.6.0)

| Subsystem | Status | Milestone |
|---|---|---|
| Settings / Memory / API client / OAuth | Complete | M3 / v0.4.0 |
| Tools (40 builtins, 9 categories) | Complete | M4 / v0.5.0 |
| ConversationOrchestrator (turn loop + streaming SSE) | Complete | M5 / v0.6.0 |
| Permission gate UX | Complete | M5 / v0.6.0 |
| Hooks 4-arm runtime (Builtin/Http/Command/Agent) | Complete (Command stub) | M5 / v0.6.0 |
| Session JSONL byte-equivalent | Complete | M5 / v0.6.0 |
| `--resume` session loading | Complete | M5 / v0.6.0 |
| Slash commands (99 registered, 18 implemented) | Complete | M5 / v0.6.0 |
| `lingxi-cli` binary + stdio REPL | Complete | M5 / v0.6.0 |
| Plugin marketplace + MCP server | M6 | M6 |
| UI / Terminal rendering | Out of scope | — |
| Mobile real-device binding | Out of scope (compile-only gates remain) | — |

## Architecture

Full design lives in three docs:
- `docs/superpowers/specs/2026-05-23-m3-engine-completion-design.md` (M3
  engine completion, v0.4.0)
- `docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md` (M2
  desktop parity, v0.3.0)
- `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md` (M1
  engine design, v0.2.0)

Navigation aid: `docs/ARCHITECTURE.md`. Security model: `docs/SECURITY.md`.
Behavioral parity guarantees with claude-code: see
`docs/ARCHITECTURE.md#claude-code-parity-guarantees-locked-in-v030`
(M2 v0.3.0 additions) and the
`claude-code parity guarantees (v0.4.0 additions)` subsection beneath it
(M3 v0.4.0 additions).

## License

MIT OR Apache-2.0.
