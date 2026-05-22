# Changelog

## [0.1.0] — M1 Foundation Release

### Crates shipped
- protocol, core, traits, api-client (Plan 01)
- permission, secret, cost (Plan 02)
- tools, hooks (Plan 03)
- memory, mcp (Plan 04)
- compaction (Plan 05)
- agent (Plan 06)
- tasks, coordinator (Plan 07)
- sidequery (Plan 08)
- skills, commands, outputstyles (Plan 09)
- session, filestate, msgqueue (Plan 10)
- cron (Plan 11)
- sandbox, lsp (Plan 12)
- telemetry, anthropic-oauth (Plan 13)
- bridge (Plan 14)
- plugin (Plan 15)
- uniffi-bridge, platforms/posix-minimal, examples/cli-demo (Plan 16)
- test-harness (Plan 17)

### Platform support
- Linux/macOS/Windows: runnable via cli-demo + posix-minimal
- Android/iOS: cross-compile gate only; production platform crates land in M3

### Tests + verification (Plan 17)
- 104 tests pass under `cargo test --workspace`
- `cargo clippy --workspace --all-targets -- -D warnings` clean
- `cargo fmt --all --check` clean
- Filesystem trait contract suite + posix-minimal driver
- `10k-iterations` Cargo feature flag plumbed on test-harness (CI wiring in M2)

### Deferred to M2
- Contract suites for the remaining 12 traits (process, http, mcp, worktree,
  swarm, secure_storage, sandbox, lsp, bridge, runtime, clock,
  hook_broadcaster) — pattern seeded in Plan 17, replication across traits
  is mechanical.
- Property tests at 10K iterations across all 11 property domains — feature
  flag is in place, individual suites read it during the M2 expansion.
- Parity fixture recordings (12 scenarios from claude-code reference) and
  per-scenario driver tests — namespace scaffold lands here.
- Contract coverage CLI + CI gate (`ratio ≤ 0.05`) — written into the
  M2 plan; trait method registry is small enough to maintain by hand
  until then.

### Tag
- `v0.1.0` — M1 v0.1.0, desktop-runnable, mobile cross-compile only.
