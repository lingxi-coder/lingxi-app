# Desktop Git Review

Git management is available from the branch indicator in the chat header. Changes opens the shared Review inspector; its view selector opens commits, history, stashes and remote synchronization. The inspector can be resized or expanded while the bottom terminal remains available.

## Implementation

- `apps/electron/src/main/git.ts` and `git/command.ts`: system Git service, repository/worktree discovery, serialized mutations, bounded subprocesses, file watchers and snapshot validation. No additional dependencies or model connection required.
- `apps/electron/src/shared/git.ts`, host and preload: structured desktop-only API, trusted main-frame and session checks. Background agent tracking prevents worktree-changing operations during active work.
- `apps/electron/src/renderer/components/GitReview.tsx`, `GitReview.css` and `gitDiff.ts`: branch search/create/switch, grouped and nested file navigation, unified/split diffs, complete-hunk staging, staged commits, history, stash and conflict resolution. Review integrates into App/BetaDesktop/RuntimeCenter.
- Destructive confirmations retain the reviewed snapshot. Rename unstaging includes both paths. Deleted conflict sides remain distinct from empty files. Merge ownership is tied to MERGE_HEAD. Session request generations prevent stale A-to-B-to-A responses from replacing current state.

## Verification

- TypeScript main/preload and renderer checks pass.
- 39 targeted Git, host, activity, diff and packaging tests pass, including 18 real temporary-repository workflows.
- Electron UI tests verify draft retention, branch search/create, occupied worktrees, split diff, file filtering, stash confirmation, conflict resolution, stale confirmation tokens, session isolation and themes.
- Full Desktop suite: 1024 passed, 3 existing failures, 2 opt-in native terminal tests skipped. Existing failures are the conversation loop folded-ID expectation, ProviderEditorFields tooltip guard, and settings page count (17 versus expected 18).
- Signed packaged integration completed init, branch creation, diff, staging, commit, push/fetch/fast-forward pull against a temporary local bare remote, and automatic Review refresh. No real remote or working repository was modified by integration tests.
- UI fixture screenshots: `/tmp/lingxi-git-ui/light.png` and `/tmp/lingxi-git-ui/dark.png`; visual verdict 91/pass. Packaged screenshot: macOS temporary directory `lingxi-git-packaged.png`.

Logs: `/tmp/lingxi-git-targeted.log`, `/tmp/lingxi-git-full-tests-final.log`, `/tmp/git-ui-final-all.log`.

## Limits

Native runtime validation is on macOS ARM64. Windows/Linux branches need validation on those platforms. Existing credential helpers, SSH agent, hooks and signing are retained; interactive authentication/signing may require the terminal. Worktree creation/migration, rebase, cherry-pick, force push and remote editing are outside this delivery. No commit or push of this implementation was requested.

Final package: `package:mac:flare -- --launch` passed with the latest UI generation guards and launched Desktop. Both Git and terminal smoke flows passed, as did credential persistence and restart checks. One preceding attempt failed the credential metadata snapshot assertion; rerunning the unchanged checks passed, so the transient failure is recorded rather than suppressed. Final log: `/tmp/lingxi-git-package-retry.log`. Packaged visual verdict: 93/pass.

Environment correction: GitEnvironment is shared by the pinned summary and topbar. Local expands project path; the anchored branch selector shows current branch first, its uncommitted file count and upstream, plus other branches. Current worktree is no longer mislabeled as occupied by another checkout. Two Electron interaction tests and web typecheck passed. Signed wrapper ultimately passed all static/runtime checks and launched the corrected app (`/tmp/lingxi-git-environment-package3.log`); preceding attempts reported sealed app.asar mismatch. No packaging code changes retained. Packaged screenshot `lingxi-git-environment-packaged.png` scored 91/pass.

Selected-project correction: actual app inspection showed AGAI's conversation stayed open after selecting LingXi-Next. Git scope previously preferred that old session, yielding No Git repository for AGAI's non-repository parent. Git now follows settings.activeProject, using the session ID only when it belongs to that project. Added selectedGitScope regression. 29 backend tests, UI interaction and scope regression, and typecheck passed; signed package runtime Git/environment smoke passed (`/tmp/git-project-follow-package.log`). Non-repository detection now preserves unexpected Git errors instead of silently reporting no repository.

Draft-scope IPC fix: reproduced `use the active session for this workspace` with a project Git request while a session was already active. Git now permits project-scoped draft requests after trusted sender and registered-project validation; terminal requests retain strict session validation. Regression failed before the fix, then Git host/scope and terminal host tests passed (3 tests), as did typecheck and signed package smoke. Evidence: `/tmp/git-scope-error-tests.log`, `/tmp/git-scope-error-package.log`.
