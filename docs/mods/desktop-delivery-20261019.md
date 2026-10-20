# Desktop and frozen 2.1.295 delivery index

## Frozen alignment baseline

The final R413 record closes the agreed 42-row matrix with 0 open rows. These
are mixed byte, constraint, source-semantic and approved-difference evidence
classes; this is not a claim of whole-output byte identity for every behavior.

| Repository | Delivered baseline |
| --- | --- |
| llm-client | `e986d0841e5cc6b5151bdaeabca1c6dd156771df` |
| Harness | `d3ed8723fa7dd21eed45397537592a555d030e94` |
| Product | `77f4cdabe5d8fd890ac65dbc3f797a73e0e6fe46` |
| Mobile Linux | `6b033d41a2b6b736fda1e8f996f55a4b20bd885d` |

Authoritative workspace receipts are under
`.worktrees/claude-alignment-integration/acceptance/matrix-closure-r379/verification/`:
`final-delivery-status-r413.md`, `final-delivery-index-r413.json`, and
`checklist.json`. The tracked Harness matrix is
`docs/parity/current-alignment/current-matrix.md` in the Harness repository.
This summary does not repeat old intermediate pending-work checkpoints as open
requirements after their final receipts closed them.

R406 macOS development signing/static/smoke and actual packaged CUA input,
append, Submit and keyboard reorder passed. Pointer-drag reorder was not
established. Android PlayDebug APK and both native ABI publication checks passed;
iOS StoreDebug simulator-arm64 app linking and build-for-testing passed.
Android/iOS apps were not installed or launched in that final acceptance; iOS
build-for-testing did not execute its test bundles. Notarization and transferred
Gatekeeper acceptance were not established. Remote and the other explicitly
excluded/approved-difference domains remain outside this frozen baseline.

## Subsequent Desktop repairs

| Change | Scoped source commit | Integration |
| --- | --- | --- |
| Desktop Crashlytics | `0e144f86ab611f78208d22873c9bb0ae2da24864` | Delivered to main |
| Bounded Mod UI IPC backlog and Replay invalidation/session routing | `bbf24ed0dbb12b0a3677b8c97aac41fcbed2baff` | main merge `2c8991d83c4a6eb62af6b899541b00052b9dadad` |
| AbovePrompt Bridge-ready startup gate and preserved Weather/Replay patch | `1295b00ce` | main merge `22dcf9b83da5e7c2c69374b515391ab43b4499e7` |

The later Desktop package consumes Harness
`b9c2860ea3f0094b83b3bd9f36b2f5fd9920d03d`; the R413 baseline pins above are
historical, not an assertion that later Product main still uses the R413 Harness
revision. Source changes after the frozen alignment baseline have their own
scoped validation and do not inherit all historical platform results.

The final Weather/Replay signed arm64 ZIP SHA-256 is
`835721442a9a06fb93ff0893253ae47775973e137114b92df63d3cfd11afcabf`.
Static verification and standard packaged smoke passed. With normal user data
and DeepSeek Flash, cold resume showed Weather and two real Write calls showed
Weather plus Replay without resizing. Open, Next, Prev and Close retained
Weather. Eleven relevant Desktop tests and node/web TypeScript checks passed.
These were bounded observations, not a guarantee against every future crash.

Historical Crashlytics showed a V8 out-of-memory failure during Electron IPC
object deserialization. The report does not identify the channel/body, so the
repaired Mod UI backlog cannot be declared its unique historical cause.

## Maintained plugin source

[Desktop Mods 0.1.1](../../plugins/desktop-mods/README.md) replaces cache-only
maintenance with a version-controlled local marketplace. The executable module
bodies are unchanged from the signed GUI acceptance; distribution adds distinct
plugin identities, a version bump, license/maintenance notices, tests and hashes.
Production Bridge install and reinstall validation is separate from the earlier
real-model GUI acceptance. This distribution does not imply another full
mobile build or a new Desktop binary build was performed.

Detailed local receipts and rollback backups are under
`.buildcache/maintained-desktop-mods-20261019/` in the multi-repository workspace.
Private user settings backups are not committed. Earlier evidence remains under
`.buildcache/ipc-memory-20261019/`, `.buildcache/replay-live-20261019/`, and
`.buildcache/mod-band-coexist-20261019/`.

## Maintained distribution acceptance follow-up

The live Desktop plugin catalog showed both maintained plugins after the source
switch. Inspecting a plugin with no user-config schema exposed a separate
Product white screen: the valid protocol value `config_schema_json: "null"`
was parsed as null and the details page dereferenced `fields`. Product now
normalizes that absence to an empty schema. The regression reproduced the exact
TypeError before the fix, then both no-schema and declared-field component
renders passed; the focused Desktop batch passed 33 tests and node/web TypeScript
checks passed. Five maintained-Mod tests and production Bridge install/reinstall
and render checks passed separately. No Harness or SDK API change was needed.

Local source registration points to the active delivery checkout at
`.worktrees/mod-band-coexist-delivery/plugins/desktop-mods`. Keep this worktree
while it is the registered directory marketplace, or re-register the source to
another durable checkout before retiring it. Upstream plugin records are retained
but disabled. Unrelated user settings and original installation records were
compared with private pre-migration backups and are unchanged.

Final follow-up Flare package static and standard smoke checks passed. Actual GUI
opened both maintained plugin detail pages (`0.1.1`, Enabled) without a white
screen; cold resume and returning from Settings displayed Weather at 21%,
208.4k / 1M. No new model turn was sent in this follow-up; replay interactions
are covered by both-order regressions and the prior identical-module GUI run.
Follow-up signed ZIP SHA-256: `71fff7ce4425174c10bed36a71fe8a181da207f4e71e728879a542bf541ad5a2`.
Large existing transcript Mod UI sites produced serialized-character-limit
rejections during observation; this receipt does not claim those oversized sites
passed or raise their Host limits. Normal transcript fallback remained visible.

The oversized transcript-site follow-up is now closed by
[the input-bounds repair](mod-ui-input-bounds-20261019.md): bounded preflight skips
Host-ineligible Mod rendering while retaining native content. The new signed
package passed static/smoke checks and the original long-session GUI check with
Weather visible and no new serialized-limit failures during observation.
