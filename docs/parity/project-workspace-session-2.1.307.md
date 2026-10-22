# Desktop directory projects and sessions: Claude Code CLI 2.1.307

Validated on macOS arm64, 2026-10-22. The product pins Harness SDK
`2f8bd587dae04b1c062c0ff58b32d2e6b9b37d45` in all 39 workspace dependency
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

## Validation

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
