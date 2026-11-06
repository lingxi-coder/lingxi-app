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
