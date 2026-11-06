# Shared UI / Web / retired-data cleanup deslop

Mode: standard, `ai-slop-cleaner`. Scope is this agent's session-owned changes only. Preserve concurrent edits, existing tests, contracts and behavior; add no dependencies.

## Bounded plan (recorded before code changes)

1. Lock behavior with the existing 135 shared/Web focused regressions, 15 retired-data tests, and the sidebar fixture's Package & Market / dirty-guard regression.
2. Dead code: remove the publication ID stored in `BotDirectory`'s draft but never read (the real command ID is created when review opens), and the unreferenced `taskTerminal` export after a repository reference check.
3. Stale comments: remove retired claim/question terminology from the changed workspace-presence and Firestore-signal comments. Remove only provably redundant non-null/optional access in changed cleanup/publication code if typechecking proves the existing guards sufficient.
4. Duplication/noisy abstractions: inspect changed boundaries, retain separate lifecycle/CAS checks where they enforce different safety points. Do not extract a new utility merely to reduce lines. No architecture, formatting, UI wording or behavior rewrite.
5. Re-run affected focused tests and Web/Electron node typechecks; check the scoped diff. Do not repeat the broad Web suite or emulator/device/package acceptance for these limited changes.

Inspected scope: session-owned files under `packages/teams` (including new execution/member-home modules, excluding root-owned `chat-source.ts` and `contracts/`); `apps/web/src/teams/{TeamsWorkspaceView.tsx,api.ts,market-link.ts}`; the modified shared/Web tests and support files; `apps/electron/src/main/teams/retiredData.ts`, `apps/electron/test/teams-retired-data.test.ts`, and `apps/electron/test/fixtures/bots-sidebar-electron.mjs`. Root-owned signup/invite-intent work and all native/host/market-agent files are excluded.

## Results

- Behavior locked before edits: shared/Web **135/135**, retired-data **15/15**, real Electron sidebar **1/1**; zero skips. The sidebar run includes Package & Market and the existing dirty guard.
- Pass 1 — dead code: removed unused draft `publicationId` state/UUID generation in `BotDirectory.tsx` and the unreferenced `taskTerminal` export in `execution.ts`. The real publication command ID remains created at review. Intermediate Bot tests: **17/17** passed.
- Pass 2 — duplication/abstractions: no edits. Publication review comparisons and repeated retired-data CAS checks protect distinct lifecycle/mutation points; extracting wrappers would reduce clarity or weaken the boundary.
- Pass 3 — comments/guarded access: corrected stale claim/question comments in `workspace.ts` and `firestore.ts`; removed redundant guarded access/assertions in `BotDirectory.tsx` and `retiredData.ts`. No policy, payload, timing, cleanup scope or mutation changes.
- Pass 4 — tests: retained all existing tests; no additions/deletions needed for these behavior-preserving removals.

Final gates: shared/Web **135/135**, retired-data **15/15**, Web typecheck, Electron node typecheck and scoped `git diff --check` all pass. No lint script is defined in Web/Electron; no dependency or lint setup changes were introduced. No broad Web suite, build, emulator, device, production signup or real user-data cleanup was rerun for this pass.

Changed in this pass: `packages/teams/{BotDirectory.tsx,execution.ts,workspace.ts,firestore.ts}`, `apps/electron/src/main/teams/retiredData.ts`, and this report. Other inspected files, including the sidebar fixture/tests, remain unchanged by this pass.

Evidence: `/tmp/deslop_shared_ui_baseline.log`, `/tmp/deslop_retired_baseline.log`, `/tmp/deslop_sidebar_baseline.log`, `/tmp/deslop_shared_ui_deadcode.log`, `/tmp/deslop_shared_ui_final.log`, `/tmp/deslop_retired_final.log`, `/tmp/deslop_shared_ui_typecheck.log`, `/tmp/deslop_retired_typecheck.log`.

Remaining risks: no new ones identified. Actual retired-data apply still requires review and stopped writers; package/device/model acceptance remains a separate integration gate.

## A4 persistence follow-up (standard mode)

Bounded plan was recorded before source edits in `.omx/plans/evidence/2026-11-05-m0-m1/web-a4/PERSISTENCE-FIX-PLAN.md`. Scope: `apps/web/src/pages/TeamsPage.tsx`, `apps/web/src/teams/invite-intent.ts`, `apps/web/test/teams-invite-intent.test.ts` and the new `teams-invite-persistence-ui.test.ts`, plus identical candidate copies. This follow-up explicitly supersedes the earlier exclusion of root-owned invitation work for these authorized lines only.

Behavior lock: actual closed-tab browser RED and a failing rendered-page regression preceded the storage-port fix. GREEN: focused helper/page/view tests **11/11**, Web typecheck, real browser **13/13**. Existing signup/account flows and all concurrent changes were preserved.

Smell review: no dead code, duplicate lifecycle branch or unnecessary abstraction was introduced. The existing lazy storage port is retained so denied browser storage is caught by the bounded helper; no compatibility reader, migration, test-only product hook or dependency was added. The helper comment now accurately describes closed-tab persistence. The shared tab fixture only models browser lifetime/storage and is reused by three regressions. No additional cleanup edit was justified after the minimal fix. No tests were removed.

Scoped whitespace/static checks and canonical/candidate byte parity pass. No Web lint script exists. No broad Web/emulator/Desktop rerun was justified by the three-call storage-port change. Final dist rebuild and live delivery remain root-owned; fresh OAuth and successful-join ACK-loss acceptance remain uncovered.

### Cross-tab review follow-up

Plan added before edits to the same A4 persistence plan. Scope remains the same four Web source/test files and identical candidate copies. Behavior was locked by four intended RED unit/rendered-page failures plus real-browser RED lost-intent checks. Implementation keeps the original page intent in one stable ref and requires a matching code before storage cleanup; no new storage schema, compatibility path, event listener or API was added.

Standard smell pass: retained the existing storage port/helper boundary; no duplicate clear path or speculative queue/abstraction. Removed a redundant pre-effect mocked sign-in call from the new test setup. The tests share the existing tab fixture and parameterize only the two distinct resolution outcomes. No tests/dependencies removed or added beyond the requested regressions.

Final gates: focused tests **15/15**, Web typecheck, scoped diff check and four-file canonical/candidate parity pass. Actual final-source browser cross-tab checks **5/5**, including unchanged full saved record/deadline, fresh-tab restoration and no join requests; outsider remains nonmember. No additional broad Web build/test, emulator or Desktop acceptance was run. Successful production join preservation is deliberately covered at the rendered-page boundary only; no QA membership mutation was needed.

Final integration follow-up: the full Web non-Emulator suite passed403/403 with zero skips using Python3.13; the prior system-Python3.9 tomllib environment failure is retained in `/tmp/teams-web-final-non-emulator-suite.log`. The three actual SDK Emulator cases retain their separate earlier passing evidence.
