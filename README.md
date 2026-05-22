# LingXi-Next

Design workspace for **LingXi Core** — a platform-agnostic Rust engine for an AI coding assistant (claude-code equivalent), targeting Linux, macOS, Windows, Android, and iOS through trait-based platform injection.

## Current state

- **Design doc**: `docs/superpowers/specs/2026-05-22-lingxi-core-rust-engine-design.md`
- **Scope**: Full-fidelity M1 covering 8 core subsystems + 4 cross-cutting engines (permission / plugin / secret / cost)
- **Status**: Design v2 complete; not yet in implementation

## Layout

```
docs/superpowers/
├── specs/                                          ← architecture design docs
│   └── 2026-05-22-lingxi-core-rust-engine-design.md
└── plans/                                          ← (future) implementation plans
```

## Reference projects (not in this repo)

These directories are vendored locally for reference but excluded from this repo via `.gitignore` — each has its own `.git` history.

- `claude-code/` — original TypeScript implementation (~519K LOC) used as behavioral reference.
- `claw-code/` — earlier Rust port (~92K LOC, 9 crates) used as architectural lessons-learned.
- `claude-code-graphify/`, `claw-code-graphify/` — knowledge-graph extractions of the above; regenerable with the `graphify` tool.

## Architecture summary

LingXi Core is a Rust library with **zero OS dependencies**. All I/O is injected via traits implemented by platform crates (`posix`, `windows`, `android`, `ios`). The conversation engine is an event-sourced state machine with a pure-function reducer; side effects are emitted as `Effect` values and handled by the platform via `EffectHandler`.

The engine ships as a Cargo workspace with 17 crates covering:

- `protocol` / `core` / `traits` — boundary types, state machine, trait definitions
- `api-client` — Anthropic / OpenAI-compat with streaming SSE
- `memory` / `mcp` / `tools` / `hooks` — core conversation subsystems
- `agent` / `tasks` / `coordinator` / `compaction` — agentic and orchestration
- `permission` / `plugin` / `secret` / `cost` — cross-cutting engines
- `test-harness` / `uniffi-bridge` — verification and mobile binding

See the design doc for the full architecture, decisions log, M1 deliverables, and milestone schedule.
