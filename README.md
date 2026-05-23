# LingXi Core

Platform-agnostic Rust engine for an AI coding assistant with 1:1 behavioral
parity to claude-code (2026-03-31 TypeScript reference) on desktop OSes.
v0.3.0 ships the M2 desktop production stack. Android/iOS land in M3.

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
| Android / iOS | M3 (not v0.3.0) |

Per-OS setup notes: see `docs/PLATFORMS.md`.

## Architecture

Full design lives in
`docs/superpowers/specs/2026-05-23-m2-claude-code-parity-design.md` (M2 parity
design) and `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`
(M1 engine design). Navigation aid: `docs/ARCHITECTURE.md`. Security model:
`docs/SECURITY.md`. Behavioral parity guarantees with claude-code:
`docs/ARCHITECTURE.md#claude-code-parity-guarantees-locked-in-v030`.

## License

MIT OR Apache-2.0.
