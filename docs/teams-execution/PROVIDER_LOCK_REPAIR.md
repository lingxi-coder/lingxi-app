# Provider account lock repair — source verification

The signed-package smoke log `/tmp/teams-m1-desktop-package-maintenance-direct-root.log` completed two custom-provider turns, app restart and cold-session restoration before the third request failed. Its retained diagnostics reported `LOCK_OWNER_UNPUBLISHED`, then `lock_failed`. The isolated home was subsequently removed. No claim is made about the exact process or operation that left the former lock incomplete.

The old lock published a directory before writing its owner. An interruption or write failure in that gap left an ownerless directory outside the cleanup scope. Model-change hydration was asynchronous, and app shutdown did not drain account loading.

The replacement writes and fsyncs a unique private owner file before an exclusive hard link publishes the strict version-2 claim. A crash before publication leaves only an inert candidate. Dead-owner reclamation is serialized by a separate complete recovery claim and rechecks the original inode/token before unlinking. Normal owner handoff during a read is retried. An unknown record, legacy directory, reused/live PID, or orphaned recovery gate is never deleted based on age. No legacy decoder or data conversion is provided.

These are ephemeral process-exclusion locks, not a credential journal. The parent directory is not fsynced: lock-name persistence across power loss is not promised. Durable credential mutation and refresh intents retain their existing independent commit protocol. Crash-left candidate files do not block account admission. A crashed recovery authority remains explicit recovery work; automatic recursive recovery would need a safe compare-and-swap primitive.

Shutdown now stops new account work and token timers, cancels interactive sign-in, waits for admitted account loading/mutations, then drains token operations. Already accepted credential commits finish before process exit. All shutdown phases settle before `app.quit`, even when another phase fails; failure diagnostics remain recorded. The regression executes the actual entrypoint aggregation and proved premature quit before this fix.

Verification is source-only: process fixtures use fresh temporary directories and synthetic metadata. They cover concurrent exclusion, SIGKILL before publication, SIGKILL after acquisition with competing reclaimers, write/link failure cleanup, owner handoff, malformed/legacy preservation, recovery-gate preservation, and shutdown admission/draining. No GUI, Credential Broker, real credential, model provider, package build or existing account storage was used. A newly signed package must still pass the normal smoke gate.

Test receipts and exact commands: `/tmp/teams-provider-lock-receipt.json`.
