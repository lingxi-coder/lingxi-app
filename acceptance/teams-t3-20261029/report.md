# Teams T3 trusted-backend checkpoint — 2026-10-29

**In progress. T3 is not implemented or accepted.** The parent is local commit
`b26556209` on `claude/teams`. No push, merge, Firebase/rules/index deployment or
IAM change. Desktop/Web/iOS/Android have no T3 UI or enabled host execution yet.
This checkpoint preserves the trusted protocol work needed before that rollout.

## Implemented source

- Trusted card owner/generation/revision and allowlist, tombstoned unbind,
  heartbeat hint, Bot conversation ACL construction.
- Human create transaction writes authenticated trigger + execution evidence;
  ordinary SDK messages/mentions cannot execute or be upgraded by an HTTP retry.
- Earliest-trigger transaction slot per Bot/conversation, claim fencing,
  one-shot start permit and persisted checkpoint.
- Five-minute **total** running deadline includes permission/question waits.
  Polling does not renew it. An interrupted/expired running invocation needs
  explicit owner recovery; it cannot be automatically rerun. The future host
  must enforce termination independently of the server lease.
- Deterministic, reserved-namespace Bot replies and atomic terminal receipts;
  source generation, policy, recall/edit and owner generation are rechecked.
- Original-human-only question/answer identity and immutable retry evidence.
- Trigger composite index is mapped only to `(default)`; existing named-database
  rules/index configurations are retained. No new cloud resource is created.

See `docs/TEAMS.md` for the API actions, limits and next runtime requirements.

## Validation and its exact limit

`npm --prefix apps/firebase run test:teams` passed **48 rules/backend cases and
7 real Desktop/Web SDK cases**, no failures/skips. The 48 include 13 new T3
protocol cases and all 35 previous T0/T1/T2 backend/rules cases. Tests use only
`demo-lingxi-teams-t0` on local Auth/Firestore emulators, with synthetic users.
They do not execute a real Bot/model, send real push or access production accounts.

The final passing snapshot covers concurrent human request deduplication, SDK execution
forgery/upgrade denial, explicit group targeting, one execution permit, host
fencing, earliest-trigger blocking, crash before start, ambiguous execution,
idempotent reply/recovery, atomic checkpoint, removal/rejoin, binding revocation,
recalled source, original-human questions and 24-hour missed work.

Static review found predictable output IDs could be preoccupied by a member.
Final source reserves `bot_` IDs for trusted outputs and rejects them in HTTP/SDK
writes; the new otherwise-valid coupled-write regression passes. Question
uniqueness, current-generation SDK Bot authorship and the default-only composite
index mapping also pass in that final rerun. All hardening edits are included.

The first attempt had one test-fixture failure (the recovery test supplied an
intentionally unsupported `attemptId`); the corrected second attempt passed 54
cases. The final hardening rerun passed all **55**. The canonical final log is
`logs/emulator-final.log.gz`; the earlier snapshot is kept separately.
`npm --prefix apps/firebase test` also passes **24** normal unit cases with **55
environment skips**; the separate Team emulator run covers the relevant Team
skips, not unrelated database integration gates. JavaScript syntax checks and
`git diff --check` pass. `results.json` records the exact validation limits.

## Runtime preflight: work still required

Independent source review found the ordinary owner Bot launch path unsuitable:

1. It shares the owner Bot's configuration home, bound folders and local router
   token. Team execution must use a clean scope and must not inherit owner MCP,
   plugin secrets, computer/browser authority or owner Bot delegation.
2. A configuration home isolates automatic/session memory, but is not a native
   file-tool boundary. Pinned Harness supports Bash parent deny/own-scope read
   exceptions. Native file/agent/host capabilities need an unbypassable scope
   restriction or an explicit restricted-tool profile that excludes them.
3. Harness `token_budget` is an auto-continuation target, not a usage ceiling.
   Host usage-event cancellation is insufficient as a strict cap or proof of
   process termination. Add the trusted budget/termination capability before
   enabling execution; existing turn/cost limits also need bridge wiring.
4. The general owner IPC/RC question-answer path lacks an original Team-human
   principal. Team questions need a distinct trusted route and generic paths
   must not answer them. Tool approval remains with the owner.

Scope identity must include owner UID, Bot, team, conversation, **human UID** and
that human's generation (different users can both have generation 1). Copy only
immutable persona/skill inputs, protect them and configuration from Bash writes,
and preserve separate transcripts/automatic/session memory. Validate overlapping
listeners and crashes before claim/after execution/after reply/before checkpoint
with a real host, then deliver/test all four clients together.

The pinned Harness already has `session_tool_grant_lease` / `SessionToolPolicy`:
a Bash/AskUserQuestion-only allowlist with no MCP grants can forbid dynamic
MCP/LSP/plugin tools and recheck after an awaited approval. A minimal restricted
DesktopConfig can receive a host-constructed in-memory sandbox policy through
`flag_settings`; it must not load writable scope/project settings. The existing
App invocation profile clears those flag settings, so it cannot simply be reused
unchanged. This is an implementation route, not evidence of an implemented or
tested Team executor. Hard token admission/accounting still needs runtime work.

## Disk and cleanup

The user explicitly approved deleting only this worktree's
`output/teams-t0-ios/SourcePackages` checkout (about 1.3 GiB logical size). It was
deleted. Source, native probe projects/logs, Firebase accounts and private browser
sessions remain. No other cache or worktree was deleted.

After an earlier successful emulator run, available space fell below the handoff's
**2 GiB floor**: observed `2,134,016,000` bytes (**1.987 GiB**). Builds/tests were
stopped. A subsequent check recovered to 4.425 GiB without further cleanup; only
then was the final source regression resumed and completed. The later check was
5.722 GiB. Space is volatile; recheck it before subsequent runtime/native builds.
No emulator was left listening. The real test credentials/session directory
remains Git-ignored and must be preserved when moving/removing this checkout.

## Resume

1. Recheck free space before building, preserving the 2 GiB floor.
2. Complete the isolated, budgeted, terminable host runtime and question/approval
   routing before connecting the queue to an engine. Do not use the ordinary
   owner Bot launch as a shortcut.
3. Implement the corresponding four-client Bot UI, run real host/offline/crash
   and native acceptance. Keep T3 unchecked until all gates pass.
4. Push/deployment/IAM still require the user's separate authorization.
