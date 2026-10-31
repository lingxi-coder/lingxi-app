# Teams publication approval plan

This plan was approved and executed on 2026-10-30. The
[publication record](../teams-t3-live-20261030/report.md) contains the deployed
revisions, verification and remaining gates. The handoff
requires approval: **“No push and no Firebase deploy without the user's
approval.”** The [local acceptance report](report.md) and `results.json` identify
the reviewed source and tested artifacts.

## Scope

- Push only product branch `claude/teams` and companion branch
  `codex/teams-runtime`, using the `github.com-lingxi-coder` SSH identity.
  No main-branch merge or push is included.
- Project/site: `lingxi-5e4b2`; function region: `us-central1`.
  The actual default Firestore database location is **`nam5`**, so the Firestore
  event trigger uses `nam5` rather than the function region.
- Deploy only Teams API, message-created trigger, default-database Team
  rules/indexes, and the scoped Hosting overlay described below.
- Reuse the already authorized test accounts and private saved sessions for
  live acceptance. Do not reset them or remove business data.

## Minimum IAM and platform prerequisites

Application identity: `push-api@lingxi-5e4b2.iam.gserviceaccount.com`.

| Principal | Role | Scope |
| --- | --- | --- |
| Application identity | `roles/datastore.user` | Project binding conditioned on `resource.name == 'projects/lingxi-5e4b2/databases/(default)'`; preserve the existing push-database binding. |
| Application identity | `roles/iam.serviceAccountTokenCreator` | Its own service-account resource only, for custom-token signing. |
| Application identity | `roles/eventarc.eventReceiver` | Project `lingxi-5e4b2`. |
| Application identity | `roles/run.invoker` | Only the Cloud Run service returned by the new `onTeamMessage` function. |
| `allUsers` | `roles/run.invoker` | Only the HTTP service returned by `teamsApi`, which authenticates every data command itself. |

The database condition and signing permissions follow the official
[database IAM](https://firebase.google.com/docs/firestore/manage-databases) and
[custom-token](https://firebase.google.com/docs/auth/admin/create-custom-tokens)
documentation. Eventarc separates project Event Receiver from service-level Run
Invoker. [Eventarc permissions](https://docs.cloud.google.com/eventarc/docs/roles-permissions)

Read-only preflight found the normal deployment APIs enabled except
`eventarcpublishing.googleapis.com`. Enable that API and provision only missing
standard Pub/Sub/Eventarc service identities and their matching service-agent
roles. If token-mint permission is proven missing for authenticated delivery,
grant the Pub/Sub service agent Token Creator on the `push-api` service-account
resource only. [Pub/Sub authentication](https://docs.cloud.google.com/pubsub/docs/authenticate-push-subscriptions)

Installed Firebase CLI 15.34.0 bootstraps unrelated project-wide default Compute
and Pub/Sub grants on a first Firestore-trigger deployment. The prepared path
below uses supported gcloud deployment to avoid those additions; it does not
patch or disable CLI security checks.

## Selective function and database deployment

After the IAM prerequisites, deploy the Firestore function with the actual
database location and explicit runtime/trigger identity:

```sh
gcloud functions deploy onTeamMessage \
  --project=lingxi-5e4b2 --gen2 --region=us-central1 --runtime=nodejs22 \
  --source=/Users/luolingfeng/lingxi/.worktrees/teams/lingxi-app/output/teams-t3-cloud-deploy/functions-source \
  --entry-point=onTeamMessage \
  --service-account=push-api@lingxi-5e4b2.iam.gserviceaccount.com \
  --trigger-service-account=push-api@lingxi-5e4b2.iam.gserviceaccount.com \
  --trigger-location=nam5 \
  --trigger-event-filters='type=google.cloud.firestore.document.v1.created,database=(default),namespace=(default)' \
  --trigger-event-filters-path-pattern='document=teams/{teamId}/conversations/{conversationId}/messages/{messageId}' \
  --memory=256MiB --cpu=1 --concurrency=80 --timeout=60s \
  --min-instances=0 --max-instances=2 --no-allow-unauthenticated \
  --update-env-vars=FUNCTION_SIGNATURE_TYPE=cloudevent \
  --update-labels=firebase-functions-codebase=backend
```

These are supported [GA flags](https://docs.cloud.google.com/sdk/gcloud/reference/functions/deploy).
The source directory is the private, hash-checked staging copy of the 25 tracked
backend files. Prepare and validate that copy before reuse; do not upload account
state, local dependencies, environment files or unrelated task output.
Read the resulting `serviceConfig.service` before granting its scoped invoker
binding. Verify trigger filters, identity, location and actual event delivery.
The Firebase `onDocumentCreated` wrapper requires the `cloudevent` signature.
The gcloud build defaulted to legacy `event` during the first deployment: requests
returned HTTP 200 without entering the handler's database reads. An explicit
runtime signature override fixed delivery; three fresh events then produced
their exact Admin-only receipts. Keep this flag on subsequent gcloud deployments.
Then use the original Teams config with
`--only functions:backend:teamsApi --project lingxi-5e4b2` for the HTTP function.
The HTTP-only filtered deployment avoids the first-event-service IAM bootstrap.

Use an isolated Firebase config containing only the default database's absolute
rules/index paths for the rules/index deployment. Named databases, catalog,
market, push functions, Storage and Realtime Database are excluded.

## Hosting overlay

Remote main is `215e6b84fb47bb04f618cd1d89a97632a9f8d7d1`, with unrelated changes
outside this task. A full Hosting deploy from the task checkout would replace
other pages. Instead, the prepared overlay preserves the current live version's
**19,265 existing file paths and hashes**, including `/index.html` and all docs.
The observed baseline is `sites/lingxi-5e4b2/versions/b628323e196483e0`.

Clone the latest live version without finalizing, add only current hashed Teams
build assets and the HTML entry at `/teams/index.html`, and prepend these routes:

| Paths | Destination |
| --- | --- |
| `/v1/teams`, `/v1/teams/**` | The verified `teamsApi` Cloud Run service in `us-central1`. |
| `/teams`, `/teams/**`, `/open/team/join` | `/teams/index.html`. |

The prepared snapshot adds 11 new hashed assets plus the Teams HTML entry, reuses
five identical existing assets and retains all 19,265 baseline file hashes. Its
union contains 19,277 paths. The candidate configuration hash is
`00291004def62b35700e8d7c5b418c10bb413776053aff0031e50e2fbe003845`;
the canonical union-file-map hash is
`a366991ed149d48fe569bd7a568f0f04001588aa483a5223db71902fa7805d1f`.
The preparer's five preservation/failure tests pass. These facts establish a
reviewable candidate, not permission to release it.

Add only the reviewed Team route headers from `firebase.json`; retain every
other header, route and configuration key. Existing assets with the same path
must retain the same Hosting hash. Hosting hashes describe gzip upload bytes;
the manifest also records raw content hashes. Confirm the returned API service
identity before binding the Hosting target.

The existing router loads a new document when leaving the Teams security zone,
and the Teams page contains its own sign-in form. No existing home, login,
administration or documentation entry is replaced. The preparer performs local
file/config work only. [Clone API](https://firebase.google.com/docs/reference/hosting/rest/v1beta1/sites.versions/clone),
[config patch API](https://firebase.google.com/docs/reference/hosting/rest/v1beta1/sites.versions/patch)

Refresh live version/config/file evidence immediately before execution. Rebuild
the overlay if the baseline changed and verify all unrelated file hashes remain
identical before finalization. Abort publication if another release appears
during preparation; do not publish a stale snapshot or blindly roll back an old
whole-site version.

## Full Desktop smoke permission

At plan preparation, the official Flare wrapper had produced the signed app/ZIP
and static verification passed. Its final application smoke refused because
`gui/501/com.lingxi.code.credential-broker.development` is already registered.
The [guard](../../apps/electron/scripts/packaged-app-smoke.mjs#L823) protects an
active user's service; it was not bypassed or unloaded at that checkpoint. The separate actual
signed Teams host/worker integration passed without using that service.

The approved full application smoke temporarily closed the current LingXi
after work was saved, unregistered its development Broker, ran the isolated
smoke, then restored the snapshotted application/service and URL handler.
Verification passed with unchanged original hashes and preserved settings,
credentials, workspaces and business data. A later interruption still requires
applicable session authorization; deployment permission alone is insufficient.

## After approval

Verify unauthenticated rejection, token signing, SDK membership/read ACLs,
two-account join/message/Bot behavior, original-human answers, owner approval,
one-shot execution, offline/restart recovery and production event delivery.
Use only the isolated test team. Real-provider and full-product mobile evidence
remain distinct from the already passing synthetic/native feature harnesses.
Record every deployed revision, service identity, changed binding and release;
preserve unrelated live resources and data.
