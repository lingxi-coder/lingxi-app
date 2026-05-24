# LingXi Core

Platform-agnostic Rust engine for an AI coding assistant with 1:1 behavioral
parity to claude-code (2026-03-31 TypeScript reference) on desktop OSes.
v0.4.0 (M3) completes the engine surface — Settings, Memory, real API
client, OAuth refresh, cost events, 143 telemetry events — over an 8-10
week single-developer delivery on top of v0.3.0 (M2 desktop platforms).
Android/iOS land in M4.

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
