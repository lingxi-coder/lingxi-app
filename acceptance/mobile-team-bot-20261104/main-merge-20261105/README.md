# Main integration validation — 2026-11-05 UTC

Merged mobile Teams/Bots branch `a08afa9f0` into main `97e8ce929` in an isolated worktree at the user's explicit request. The merge preserves current main's session-owner and terminal-directory validation, remote connection fences and admitted-operation drains. Added tab-selection revisions to reject late Local App navigation after leaving Chats, and adapted the session-link test to main's required mode parameter.

- Android product and all JVM test sources compile. Focused tabs, remote, Teams, Settings, cron, Local Apps, session and project suites: **681 tests, 0 failures** (78 suites).
- iOS complete product typecheck with current main generated bindings and external SDK stubs: **0 errors** (151 seconds). Dependency-light macOS model and navigation suites: **101 tests, 0 failures**.
- Canonical translation generation and pairing gates pass. Source translation keys were merged semantically; generated outputs were synchronized from those keys.
- Independent read-only integration review has no outstanding high/medium finding.

See `validation.json` for scope and limitations, compressed logs for execution output, and `source-sha256.json` for merged product inputs. This is source-level integration evidence. The original device acceptance receipts remain unchanged and do not certify this merge's artifacts. Full live-account/release/soak acceptance and the earlier two full-suite audio failures are not declared resolved.
