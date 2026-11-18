# Inbox cloud operations

Production: Firebase project `lingxi-5e4b2`, default Firestore database,
Cloud Functions/Cloud Tasks in `us-central1`. All Inbox runtime functions use
`push-api@lingxi-5e4b2.iam.gserviceaccount.com`.

## Deployment scope

Deploy the Inbox API, task worker, fourteen `onInbox*` source triggers, and the
updated `teamsApi` and `onTeamMessage`. Do not deploy unrelated functions.
Deploy rules/indexes with `--only 'firestore:(default)'` so the catalog and push
databases keep their own configuration. The checked-in project dotenv sets
`INBOX_NOTIFY_ROLLOUT_PERCENT=0`: summaries and reads are enabled while existing
Team notification delivery is retained. Notification rollout is a separate step.

The `inboxWork` task queue uses concurrency 16 and 20 attempts with 2–120 second
backoff. The runtime service account needs Enqueuer on that queue and actAs on
itself. The task worker's Cloud Run invoker is the same service account.
Eventarc also needs that service account to have `roles/run.invoker` on each
individual Inbox event service. Keep all these bindings scoped to their resources;
never make the worker/event services public or grant project-wide Enqueuer.
The Firebase CLI does not automatically add the custom Eventarc identity's Cloud
Run invocation binding to newly created event services. Check the deployed policy
and request any required operator approval before changing IAM.

The public `/v1/inbox{,/**}` Hosting rewrite targets Cloud Run `inboxapi` in
`us-central1`. When only API routing changes, preserve the currently live Hosting
files instead of deploying a possibly unrelated web build. Record the old and new
Hosting versions, their file hashes and the previous configuration for rollback.

## Acceptance

From `apps/firebase`, run the script with the repository's tsx loader:

```sh
INBOX_LIVE_REPORT=/absolute/path/to/live-report.json \
  node --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs \
  scripts/verify-inbox-live.mjs
```

The operator needs a current gcloud login with default-database fixture cleanup
access. The script creates disposable password accounts with random in-memory
passwords; it neither uses real contacts nor registers push tokens. Two independent
Firebase clients sign in as the reader, another as sender, and an outsider tests
ACLs. The shared Inbox subscriptions must observe real source-trigger/task fanout,
retry deduplication, no self unread, canonical watermark writes, cross-client
read/reminder propagation, stale-read protection and legacy SDK read reconciliation.
Only exact created accounts/documents are cleaned up, including friend/invite
lookup records. A passing SDK test is not physical Android/iOS/Desktop UI acceptance.

Use the isolated `npm run test:teams` emulator suite for backend and SDK rules
coverage. Failed production runs keep diagnostic reports and still clean up their
fixtures. Check task queue attempts and Cloud Run WARNING/ERROR request logs;
missing invocation IAM produces 403 warnings before handler execution.

## Rollback

Releasing the saved previous Hosting version removes the new route without changing
website files. Restore only the selected function sources/configuration if needed.
Do not delete Inbox rows, canonical reads, manual reminders or personal settings:
rolling back the UI/API must preserve confirmed user state. Keep the dedicated
queue and event access in place until pending legitimate events have drained, and
remove only bindings/resources introduced by this deployment if retiring the service.
