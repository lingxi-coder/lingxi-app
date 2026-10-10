# Current audio configuration verification

Verification date: 2026-10-10. This record covers the current schema v4 shared
configuration and Electron host/UI integration. Older configurations are not
migrated: absent or unsupported versions use fresh v4 defaults. String voices,
display-name aliases, old configuration fields, recovery values, and migration
completion markers are not read. Stable system identifiers and exact provider
profile/model voice scopes remain explicit.

## Checks

- Shared JS/TS normalization and route fixtures: six tests passed, including
  rejected old versions, independent cloud bindings, no local fallback for cloud
  routes, and ready native endpoints with nullable model IDs.
- Generated TS/Swift/Kotlin configuration and shared offline catalog drift
  checks passed.
- Electron audio/voice/settings regression suite: 171 tests passed. Both
  Electron TypeScript projects passed.
- Bridge transport: four tests passed, covering frame bounds and bounded
  realtime input backpressure. The bridge-client production build passed.
- Native Swift helper: 36 tests passed, including current configuration and
  stable voice identifier regression checks. Evidence is `/tmp/lingxi-desktop-swift-audio-currentonly.log`.

Evidence: `/tmp/lingxi-desktop-audio-currentonly-tests.log` and
`/tmp/lingxi-desktop-audio-currentonly-typecheck.log`.

Tests cover immutable configuration revisions, unsaved preview isolation,
capability/readiness gates, exact current-session routes, operation and device
identity fences, cancellation, bounded media queues, provider-reported usage,
and ordinary persistence after an unsupported stored version. Obsolete owner
commands are rejected before helper dispatch; UI usage uses the same host audio
service as Agent tools.

Live provider requests, physical microphone/speaker operation, and acoustic echo
cancellation have not been verified. Desktop realtime offers turn-based
interaction. These focused checks do not establish signed-package or physical
iOS/Android device acceptance.
