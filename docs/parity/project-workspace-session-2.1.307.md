# Desktop directory projects and sessions: Claude Code CLI 2.1.307

Validated on macOS arm64, 2026-10-22. The product pins Harness SDK
`5a2de5b14e6c0d8d2e5bac9155c9c39accda86c7` in all 39 workspace dependency
declarations. The lockfile uses the same Git revision for all 78 SDK packages.
No local Cargo path overlay is required.

## Implemented behavior

The reference is the installed Claude Code CLI 2.1.307 artifact, rather than an
invented ProjectId-to-WorkspaceId registry. Projects use directory buckets and
one-hop directory aliases; sessions keep their UUID when their current directory
and transcript location change.

- `/add-dir` shares the SDK validator, permission grants, prompt-root lifetime,
  and one-way alias behavior with CLI callers.
- `/cd` and private `set_cwd` keep logical cwd, the live Bash cwd, trust decisions,
  transcript ownership, and fresh-session notices consistent. The initial root
  remains available for used/resumed sessions. A fresh empty session does not
  gain a transcript merely by moving directories.
- Current, Repository, and All catalogs retain the selected physical transcript
  path. Ordinary CLI UUID resume and the interactive repository picker use their
  distinct reference resolution rules. Exact title lookup retains the full
  title independently from the displayed summary.
- Desktop cold resume captures the selected source before startup hooks, then
  completes deferred tools after the authenticated permission transport connects.
  Its private acknowledgement follows the restored-history proof and durable
  completion. It does not synthesize ordinary chat turn events.
- Approval, cancellation, disconnect, stale connection events, delayed directory
  dialogs, and competing navigation retain the originating session owner.
  Composer drafts remain editable while sends wait for resume completion.
- Remount restores the actual transcript path, original root, additional grants,
  and registered prompt roots. Workspace environment overlays merge while the
  configuration/storage owner remains fixed.

## Native evidence boundary

SDK documentation and fixtures record 353 controlled cases executing 79 unchanged
native source ranges from artifact SHA-256
`5b113a7752119470f1da419cb0e608af3361cae25b614216b13022093958fb2a`.
Fixture regeneration is byte-identical. This establishes the captured helpers'
branch/text behavior with declared host and filesystem stubs; it is not proof of
byte equality for the entire CLI, real Git enumeration, every host integration,
or every live provider conversation.

No extracted proprietary source is checked in. The SDK contains the extractor,
range digests, controlled fixtures, implementation tests, and detailed limits in
`docs/parity/workspace-session-2.1.307.md`.

## Initial integration validation

| Check | Result |
| --- | --- |
| Native controlled replay and fixture comparison | 353 cases, 79 source ranges |
| SDK session unit and integration tests | 411 + 49 passed |
| SDK workspace identity / command builtins / runtime prompt roots / disconnect ordering | 9 + 829 + 5 + 7 + 1 passed |
| Bridge workspace and real WebSocket resume regressions | 22 passed |
| Bridge-client tests | 104 passed |
| Scoped Electron TypeScript suite on latest main | 1,752 passed, 2 skipped, 0 failed |
| Electron node/web type checks and Vite build | Passed |
| Real Electron picker, IME, focus, navigation fence and resume approval test | Passed |
| Normal-feature bridge binary built against published SDK Git source | Passed |
| Isolated real-process cold start, directory move, exit, and restart | Passed |

The real-process check used empty isolated configuration and HOME directories,
the normal binary, and the built bridge-client. It restored an exact selected
transcript, added a directory, moved cwd, verified transcript relocation and
Current-catalog lookup, checked merged environment settings, then restarted from
the moved transcript with the original root and grants. Both processes exited
gracefully and removed their bridge lockfiles. No real user credentials or model
turn were used. This check is not a signed Electron package or live-provider
acceptance claim.

The Auto regression uses the real runtime, Adapter permission gate and classifier
with an unavailable loopback endpoint. It verifies fail-closed denial, no tool
side effect, the persisted error result, resume proof and bounded shutdown on
the default test-thread stack. The stack fix reuses synchronous future factories;
it does not enlarge thread stacks, replace the classifier, or change Auto policy.

A separate pre-existing Settings GUI fixture fails on clean baseline `f5ee37f67` with
`missing editor provider-credentials`. It is outside this change; the complete
GUI suite is not represented as passing. The running-tool render assertion left
behind by main's tool-row update was corrected to verify the accessible Running
status instead of a retired animation class.

Publication uses scoped worktrees for both repositories. The product retains the
independent LocalApp and tool-row commits `608f8c3d5` and `d7b855721`. Unrelated
tracked/untracked work and the original indexes were excluded from these commits.

## Signed cold-start and restart validation

The subsequent signed acceptance used product code `b9c86c5bc` with published
SDK `5a2de5b14e6c0d8d2e5bac9155c9c39accda86c7`. The documentation update after
that build does not change packaged code. All 39 workspace declarations and
78 lockfile packages use that SDK Git revision, without a local Cargo overlay.

Real process testing and regression review found and repaired these issues:

- Terminal ownership rejected a committed cross-directory session move. Existing
  PTYs now retain their running process and cwd while their owner follows the
  session; newly opened terminals use its new directory.
- Moving an empty draft saved a UUID with no durable transcript. Directory
  selection is now durable independently of a volatile draft, preserving any
  previously remembered durable conversation.
- Disposing a replaced runtime marked its still-live UUID as removed in the
  renderer. An explicit replacement reason retains the owner while ordinary
  disposal, archive, failure and stale-generation fences remain effective.
- A remount reset live permission, reasoning, Fast and Proactivity controls to
  defaults. An authenticated, bounded private restore carries same-session idle
  controls through remount and selected resume. It does not write global
  preferences or invent transcript metadata; target capability gates are
  recalculated while Proactivity's original session baseline is retained.
- The renderer displayed the old permission mode after successful restoration
  and reported intentional replacement as an action error. It now renders the
  effective permission from controls and suppresses only the exact intentional
  replacement disconnect error.

| Final check | Result |
| --- | --- |
| SDK orchestrator library | 2,143 passed |
| Native workspace/private restore/WebSocket regressions | 22 unit + 2 integration passed |
| Real PTY ownership and process lifecycle | 2 passed |
| Focused Desktop remount/runtime suite after test ownership isolation | 71 passed |
| Final renderer controls and connection lifecycle | 29 passed |
| Bridge-client | 105 passed |
| Electron node/web type checks and packaged Vite build | Passed |
| Mandatory Flare package signing, portable native verification and full cold smoke | Passed |
| Signed CUA directory/session acceptance, three launches and normal exits | Passed |
| Final signed launch with original user profile | Passed; left open |

The mandatory `package:mac:flare` wrapper used Team `AZ4AX7J833`, timestamped
hardened signing and a cold Credential Broker with no warm-up. It passed static
verification and the actual packaged credential, terminal, Git, scheduling,
permission and security checks, authenticated localhost model request, LocalApp
tool creation, delivery card, library, independent window, restart and cleanup.
The final ZIP SHA-256 is
`1da668a7f4e87cd88a24e5601031389413d2828c6acf380bc096c112a8af7841`.

Additional CUA acceptance used an isolated HOME/config/userData, Git directory A,
its linked worktree B, additional directory C and unrelated project D. All new
interactive input used CUA; fixture probes only read state/catalogs/events and
staged the disposable provider environment. Four authenticated fixture turns
invoked the real Bash dispatcher in A, B, D and B after a process restart.

An empty A-to-B move retained its UUID without creating a transcript; its next
cold launch opened a fresh B draft. A used session retained its exact UUID,
history, model, Ask permission and opted-in Proactivity when moved from A to B,
and its transcript physically moved to B's directory bucket. A new chat used a
different UUID. The native picker searched the original UUID and restored its
physical transcript; from A, Current/Repository/All returned 0/1/2 conversations,
respectively, and Repository selected B while excluding D. Invalid-directory
failure and native additional-directory picker cancellation retained the owner.

Three actual packaged processes exited normally with code zero. The final cold
restart restored the exact selected B UUID and A/B history, then executed a fresh
Bash command in B. D's transcript remained byte-identical. No renderer errors
were captured. The final normal-profile launch restored the user's original
project, session, model and history; active project/session, project list and
pinned sessions matched their pre-test values. The original 11 dirty product
files retained their hashes, both original indexes were unchanged, and the SDK's
untracked directories remained present.

Evidence is saved under the checkout parent's
`acceptance/desktop-workspace-session-signed-782713ad5-20261022/`, including
`acceptance-result.json`, `reviewed-build-inputs.json`,
`build-reviewed-sign-verify.log`, `gui-verified/`, `preservation-final.json` and
`user-profile-after.json`. `validate-evidence.py` rechecks saved proofs, artifact
digest, file/index preservation and user-profile fields. This signed acceptance
supersedes the earlier real-process-only boundary for the scenarios above.

The full Desktop run before the final renderer additions had 1,870 passes,
3 skips and 5 failures. All five reproduce on untouched main `782713ad5` with
the same assertions: activity text, session hover/drag, the Settings
`provider-credentials` editor, Stage scroll padding, and thought-preference tool
text. Those unrelated UI fixtures remain failing; the whole Desktop suite is
not represented as green. The localhost fixture does not establish production
provider quality, this GUI model only supports automatic reasoning, and a
different Mac's Gatekeeper transfer and Developer ID notarization/stapling were
not tested.
