# LingXi Code

Platform-agnostic Rust engine for an AI coding assistant with 1:1 behavioral
parity to claude-code (2026-03-31 TypeScript reference) on desktop OSes.
v0.8.0 (M7) ships the **TUI Surface**: the full single-user terminal UI on top
of the v0.7.0 foundation — full ANSI/markdown/syntect rendering + StructuredDiff,
~22 message renderers, a windowed `VirtualMessageList` scrollback, an advanced
multi-line `PromptInput` (vim Normal/Insert/Visual + command palette +
`@`-completion + history search + image paste), four full-page screens
(Doctor / Resume / Settings / Memory), a message search/jump/export selector,
and a 6-theme picker. `--no-tui` and non-TTY fall back byte-for-byte to the
v0.6.0 stdio REPL.
v0.7.0 (M6) shipped the **TUI Foundation**: the first iocraft-based terminal
UI — a 3-zone layout (StatusLine / Scrollback / PromptInput) with streaming,
4 message renderers, 3 permission dialogs, and real cost + MCP/Hooks/Agents
listings + `/compact` wired into the surface.
v0.6.0 (M5) completed the Execution Engine 全集: `ConversationOrchestrator`
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

# Run the CLI. Tool/skill/command assembly is owned by the `engine-desktop`
# composition root (M8); the `cli` crate just hands it the platform + config.
ANTHROPIC_API_KEY=sk-ant-... cargo run -p cli -- \
    --model claude-opus-4-7

# The minimal end-to-end demo (effect-only path, no live API call):
cargo run -p cli-demo
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

## Subsystem status (v0.8.0)

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
| TUI foundation (iocraft 3-zone, streaming, 4 renderers, 3 dialogs) | Complete | M6 / v0.7.0 |
| TUI engine wiring (real cost / MCP-Hooks-Agents lists / `/compact`) | Complete (summary stub) | M6 / v0.7.0 |
| Plugin marketplace + MCP server | M6 | M6 |
| TUI rendering primitives (ANSI 16/256/truecolor, markdown, syntect, StructuredDiff) | Complete | M7 / v0.8.0 |
| TUI message renderers (~22 system/assistant/user) + VirtualMessageList scrollback | Complete | M7 / v0.8.0 |
| Advanced PromptInput (vim Normal/Insert/Visual, command palette, `@`-completion, history search, image paste) | Complete (vim parity subset) | M7 / v0.8.0 |
| TUI full-page screens (Doctor / Resume / Settings / Memory) + theme picker + message selector | Complete | M7 / v0.8.0 |
| Live assistant text → markdown/syntect (#211) | Plain text (markdown wired only into secondary renderers) | M8 |
| Advanced engine wiring (real `/compact` summary, CostTracker→AnalyticsBus, MCP auto-connect, OAuth PKCE, per-model cost) | Deferred | M8 |
| Team / Coordinator / Swarm renderers, voice, mouse mode, inline image display | Out of scope | M8 |
| Composable engine (`engine-desktop` / `engine-mobile` composition roots, ~73 flat crates, §8.1 dep gate) | Complete | M8 / v0.9.0 |
| Mobile platform + UniFFI callbacks (`platform-ios/android`, `tool-camera/voice/share`, `ios-framework`/`android-aar` + Swift/Kotlin skeletons) | Skeleton only — full bring-up in M9 | M8 / v0.9.0 |

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
