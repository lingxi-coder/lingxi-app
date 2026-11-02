# Desktop skill/runtime acceptance, 2026-11-01

Final Desktop acceptance passed on 2026-11-02 UTC (2026-11-01 local): two
complete signed-package runs, actual Astra / Max session replay, and the
packaged native brand-path probe. No checks were disabled or deadlines
increased, and no legacy compatibility was introduced. This is local working-tree
acceptance with explicitly declared local Harness development patches, rather
than an immutable-pin release. Packaging uses `package:mac:flare`, the Flare
Apple Development identity, native path remapping, private Team executable
provenance, inside-out signing, and the complete static/runtime verification.

## Fixes and evidence

### Apps library timeout

The failure snapshot contained the published app in the Host catalog while the
renderer remained in its newly opened chat. Host bootstrap had published the
new session ID before renderer admission completed. The smoke clicked Apps
while navigation was disabled, so that click was intentionally discarded.

Both Apps transitions now wait for an editable composer and enabled renderer
navigation. The 20-second deadline and publication, independent window,
durable-data, Modify, authoring-session and independent-draft checks remain.
The terminal also waits for session admission instead of querying a temporary
draft scope after the Host has created the actual session.

Evidence: `apps-diagnosis/packaged-apps-library-failure.json`,
`apps-race-fixed.log`, and the final packaged smoke logs.

### Automatic extra answers

The original trace showed a visible answer and normal end, followed by a new
query carrying a completed-task notification whose report had already arrived
through SubagentHandback. Empty-output recovery forced that passive query to
answer again. Allowing a notification to finish silently removed the forced
nudge, but real Astra / Max replay still produced a redundant confirmation
(`r7-extra-confirmation-events.json` and `r7-extra-confirmation.png`).

The task registry now retires the redundant completion notice using the current
run's admitted receipt and recipient. This applies both before idle readiness
checks and before direct in-turn notification drains. It retains the report,
resumable task and UI delivery state. Failures, turn limits, worktree artifacts,
unreported results, unconsumed owned work and a changed fallback recipient keep
their notifications. No report-text matching or new persisted format is used.

Ordinary user requests retain empty-output recovery. A new non-meta user input
accepted during a notification wake restores the visible-answer requirement.

The duplicate-completion regression failed before the registry fix. The full
Tasks library now passes 560 tests, with no failures or ignored tests.

### Cold-start credential path discovery

A fresh packaged smoke exposed a credential launch failure at the lock stage.
The Host retained one eager directory-discovery Promise for its lifetime, so
an initial discovery rejection would poison every later credential operation.
Independent Host services also launched competing native path-discovery
processes. A regression demonstrated 12 native launches for 12 concurrent
consumers before the fix.

Each catalog now shares its native discovery and caches only success; a failed
discovery remains an error and is released for a later operation to try again.
Credential locks obtain that result on demand. Returned paths are copied to
prevent caller mutation. Paths still come from the brand's native discovery;
there is no hard-coded replacement home or legacy-path fallback. Lock-stage
diagnostics classify known failures without logging credential contents.

Both final r9 packaged smoke runs passed, including authenticated requests
before and after restart. The final package includes the report-completion fix.

### Runtime dependency packaging

The current dependency graph includes MCP SDK 1.32.2 and Express 5.3.0, whose
installed scopes require different `content-type` major versions. The former
copier flattened all packages by name and rejected that valid graph. Runtime
copying now preserves installed Node scopes, including linked workspaces and
cycles. Static verification now resolves the required `ws` payload from the
actual bridge-client import scope, and still checks its stream implementation,
manifest and actual signed-Electron import. A collision at the same destination still fails. No dependency was
forced to another version and no unrelated manifest/lockfile change was
reverted. The regression failed under the old copier and now verifies actual
Node `require()` resolution for both versions. All 35 packaging tests pass.
Evidence: `runtime-dependency-graph.json`, `runtime-dependencies-before-fix.log`
and `runtime-dependencies-final.log`.

## Focused validation

| Check | Result |
| --- | --- |
| Tasks library, including report ownership, deduplication, errors, rest and workflow lifecycle | 560 passed |
| Query/notification regressions, including user empty output and new user input during a wake | 7 passed |
| Existing thinking-only recovery tests | 21 passed |
| Visible-text tests | 7 passed |
| Desktop packaging, Local App authoring, terminal and session bridge tests | 82 passed |
| Native path discovery and provider accounts | 50 passed |
| Runtime packaging, including nested Node scopes and workspace cycles | 35 passed |
| Desktop Node and renderer typechecking | Passed |
| Final native package static/runtime verification | Passed twice, full suite |
| Final real Astra / Max replay without an extra answer | Passed, 112-second delayed observation |

## Runtime acceptance

The original failed session `9f374e54-8af5-4624-924b-88a2ae0b22b8` is preserved.
The education Local App flow uses QA session
`0129d491-6cb2-4369-bb92-dfe077a03a65`, GPT 6 Astra / Max, `openai-chatgpt`.
The original discussion and follow-up use the real packaged composer and actual
Context7 requests. They are read-only; no education application is generated
in the user's project. The full smoke separately publishes and exercises an
actual Local App in its isolated fixture home.

Final checks cover ToolSearch, an actual MCP query, a fresh guide agent's
SubagentHandback receipt, acceptance of native transcript UUIDs, connected and
editable parent UI, no redundant answer after completion, and branded path
ownership using the packaged executable. The path probe checks explicit Host
ownership against a conflicting ambient directory, the brand default, empty
overrides, and absence of empty Cron locks or a generated upstream namespace.

Evidence directory:
`/Users/luolingfeng/lingxi/.buildcache/skill-runtime-desktop-20261101/evidence/`.
Final r9 replay captured 291 events: Agent, ToolSearch and the actual
Context7 query all returned without error. The fresh guide agent reached
`completed` with `report_delivery: delivered`; all three child message UUIDs
were accepted. There was one root turn start and one normal end. Over a further
112 seconds there were zero text deltas after the end, zero duplicate task
notifications, zero empty-output recovery nudges and zero additional turns.
The parent remained connected with an empty editable composer and enabled
model picker. The previous r7 extra confirmation remains in the preserved
history; it was not generated again by r9.

Final evidence: `package-unlocked.log`, `package-unlocked-repeat.log`,
`package-unlocked-apps/`, `package-unlocked-repeat-apps/`,
`final-replay-assertions.json`, `final-live-events.json`,
`final-replay-complete.png`, `brand-path-result.json`,
`source-manifest-r9.json`, `native-package-r9.json`, and
`normal-launch-final.json`. Original session/file hashes and mtimes were
rechecked unchanged. Only generated Cargo.lock source removals from local
path-patch validation were restored; unrelated work was retained.

The verified app has been left running via ordinary macOS launch, without
remote-debugging arguments; the CDP port is closed. This establishes local
signed Desktop acceptance on this Mac. It does not establish Developer ID
notarization or installation on another Mac.

Package:
`/Users/luolingfeng/lingxi/.buildcache/skill-runtime-desktop-20261101/package/LingXi-Code-0.1.0-mac-arm64.zip`

SHA-256:
`f74ba4407927a8089062976b06c92b66c7ea96f7ffec7ad4b3c3dbbd071de078`.

## Resolved macOS Keychain interruption

The first r9 runtime attempt was blocked by `User interaction is not allowed
(-25308)` for the existing login item, while signed Broker health and absent
item queries succeeded. The console reported unlocked. After the user locked
and unlocked macOS with their password, the same signed Broker could access
the existing item. Both full runtime runs and the real replay then passed
without changing any credentials or weakening the login gate. The failed
attempt remains preserved as evidence, not counted as a passed run:
`keychain-blocker-r9.json`, `package-final.log`, and
`package-final-apps/packaged-runtime-diagnostics.jsonl`.

## Scoped main delivery

The Harness fix is published as `9f2f29e5127fcbe191dc63ae4939fbb43c90988c`. Product Cargo.toml
and Cargo.lock pin that same remote revision, including patched nix/WebRTC
sources. Clean `cargo metadata --locked` resolves all 83 Harness packages
from that remote Git source, with no local Harness path patches. The existing
llm-client pin `a30a3a96907a7e3847af4aa999e7ddf7b4590dcf` is unchanged;
that repository requires no additional fix commit.

The commits were assembled in isolated worktrees. Independent Local App MCP
implementation/SDK dependency changes and image-preview changes are excluded.
Shared translation files, guide resources and Host assembly were separated at
the relevant code blocks. Original working-copy source and staged state were
preserved during assembly. The r9 signed-package evidence above records its
working-tree source manifest; it is not an immutable-pin package claim.

Validation of this isolated delivery: 560 task, 14 notification, 21 thinking
recovery, 1 discovery, 7 guide, 6 brand, 155 Cron, 25 configuration, 1 supplied
control-tool authorization and 5 configuration-guide Rust tests passed.
Product Node/renderer typechecks, 90 focused Desktop tests and 4 Bridge
transcript validation tests also passed. Dependency graph, scope manifests and
logs are under
`/Users/luolingfeng/lingxi/.buildcache/skill-runtime-main-20261101/`.
