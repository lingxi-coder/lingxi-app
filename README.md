# LingXi Core

Platform-agnostic Rust engine for an AI coding assistant. M1 desktop-runnable
(Linux/macOS/Windows); Android/iOS land in M3.

## Quickstart

```bash
cd lingxi-core && cargo build --workspace --release
ANTHROPIC_API_KEY=sk-ant-... cargo run --bin lingxi-demo -- --model claude-opus-4-6
```

## Architecture

See `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md` for the
full design (~7000 lines) and `docs/ARCHITECTURE.md` for a navigation aid.

## License

MIT OR Apache-2.0.
