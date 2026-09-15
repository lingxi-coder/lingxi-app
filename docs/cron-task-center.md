# Local scheduled task center

Scheduled tasks belong to a validated workspace scope, independently of the
foreground conversation. Desktop uses an application-managed
`scheduled-workspace` for No project; mobile uses `scheduled/workspace` beneath
its application storage. These directories are not aliases for the current
project or the mobile application's entire data directory.

## Persistence and execution

`CronTask.automation` stores version 2 configuration and run history. Task status
(`active`, `paused`, `completed`) is independent of each run's status. The legacy
`session_id` remains the creation source. Execution uses `runMode` and its fixed
target or persisted task-owned session association.

The shared Rust scheduler claims occurrences under the existing file lock,
records a run before dispatch, and merges terminal results into the latest task
record. A task has at most one running and one coalesced pending occurrence.
Pausing cancels pending work without interrupting an already-started turn.
Completed and expired tasks remain available with their history.

Desktop service controller sessions own scheduling; ordinary foreground Bridge
sessions do not. `ScheduledTaskService` leases background runtimes without
activating them. A target session is bound immediately before its automatic turn
starts. Mobile restores real persisted sessions through its existing engine
path, including the session's Chat/Code mode. Saved model and reasoning settings
are scoped to the automatic turn and do not replace human chat defaults.
Automatic assistant transcript rows carry `perTurnSettings` and retain their
actual model/provider/reasoning. Session replay keeps these messages in history
but excludes their settings when recovering human defaults. If no such defaults
were persisted, replay preserves the application's bootstrap settings.

History records include the actual target session and model. Terminal retention
is bounded to 20 records per task and 500 total; unfinished records are retained.
Notification policies are all runs, failures only, and off. Hosts reserve
terminal notification delivery durably to avoid duplicates after restart.

## Compatibility

Desktop protocol version 14 carries automation configuration and the scheduled
execution handshake. Mobile UniFFI DTOs carry the same configuration as JSON.
Deploy matching Bridge/native binaries and client bindings together.

Legacy tasks retain their IDs and schedules. Migration captures application
model defaults and selects a new conversation per run; tasks without a usable
model are paused. Mobile global-store migration uses an atomic recovery marker
at `.lingxi/cron-v2-migration.json` to suppress the old source before publishing
the destination. CLI and cron tools preserve the new fields and use the shared
session-aware execution path.

## Review corrections

Reactivating a v2 one-shot task uses its latest activation anchor while retaining
its original creation time and history. Resuming therefore schedules a future
occurrence instead of replaying an expired one. Completed tasks do not consume
the cron tool's schedulable-task quota.

Busy retries retain the original run ID and merge later pending occurrences.
Mobile callers receive an explicit retryable result for that durable queue entry;
iOS restart reconciliation preserves entries that have not started. Cancellation
settles any in-progress disk claim and releases execution before writing the same
run's terminal state, even after the foreign caller's runtime is destroyed.
Borrowed foreground engines are released on a blocking thread because their
owned Tokio runtimes cannot safely be destroyed on an async worker.

The CLI acquires the target session's writer lease before replaying its transcript
and keeps that lease through runtime construction and execution.

Each occurrence now also persists a `claimGeneration`. Binding and completion
check that generation and its process owner, so a shutting-down scheduler cannot
overwrite a queued retry or a newer claimant. Binding rechecks expiry, and a
configuration failure from an old execution snapshot cannot pause newly repaired
settings. The CLI's current-session execution holds the turn gate across target
validation, binding, execution and result capture.

Mobile manual calls use a persisted host occurrence identity. The optional
`manualOccurrenceAt` field lets a legacy queued run adopt that identity without
changing its original run ID or schedule timestamp. iOS reserves notification
delivery with an exclusive durable marker. Android parks busy retries in a
separate delayed worker and releases the global execution chain only after the
wake has been durably submitted.

The third review corrections separate durable completion from an execution's
return: persistence errors retain the captured result and timestamp for retry,
without re-executing the model. Old executions cannot complete newly edited
one-shot plans. Native CLI executions have an independent cleanup owner; stopping
the scheduler waits for that owner, and incomplete cleanup remains retryable.

Desktop archive/project removal use the pause lifecycle operation, preserving
current settings and attaching the dependency reason. Mobile conversation reuse
synchronizes transcript, permission and Agent/Workflow identities. Android
preflight failures park an independent wake; an already-started run remains in
recovery and cannot become executable again. iOS snapshots each run's notification
policy and recovers unreserved terminal notifications, including completed tasks
that no longer have another due occurrence. Legacy terminal history without a
policy snapshot is not retroactively notified.

The fourth review corrections recheck scheduled preparation cancellation after
credential loading, model lookup and session binding. Android recovers retained
terminal notifications independently of current project/task availability, using
the run's saved policy and durable claim. iOS serializes history mutations and
notification reservations with a shared file lock, then removes reservations
whose history was durably pruned; stale store instances reread the current file.

## Runtime limits

Tasks execute locally on their device. Desktop execution requires the application
to remain running. Mobile background execution remains subject to platform wake,
frequency, timeout, and permission limits. This implementation does not add
cross-device synchronization. A matching rebuilt application must be restarted
to load the updated native runtime.

## Verification entry points

The Rust regression groups are `cron` library tests,
`engine-desktop::cron_management`, `engine-desktop::cron_native`, Bridge library
tests, and `engine-mobile::host::cron_automation_tests` with the `uniffi` feature
enabled. The rooted-filesystem direct-root test covers the mobile sandbox opening
path. Protocol snapshot/version tests must pass without `BLESS` after bindings
have been regenerated.

Electron's scheduled service, settings, draft and interaction tests cover the
global task center. The packaged smoke test also exercises project and No project
task CRUD across an application restart, in an isolated profile. Run macOS
packaging through `npm run package:mac:flare`; its startup check refuses to run
alongside an existing LingXi instance.

iOS Cron repository/UI tests and Android Cron JVM/instrumentation tests exercise
the native clients. Rebuild Android Direct and Play JNI libraries separately;
their capability features differ. Native tests must use binaries built from the
same source as their generated bindings.
