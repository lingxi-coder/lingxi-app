# Local Teams Hosting overlay preparation

This tool prepares a reviewable local plan. It performs no cloud requests, remote
writes, full Hosting deployment, Git merge/push or IAM changes.

It requires the captured finalized LIVE version and its complete ACTIVE file list
in `output/teams-t3-private-preflight-r2/{hosting-version,hosting-files}.json`.
The expected baseline is `sites/lingxi-5e4b2/versions/b628323e196483e0` with 19,265
files. Its existing root entry, newer Admin/catalog code, docs and other bytes are
inherited by hash; the current checkout's full `dist` tree must never replace it.

From the product checkout:

```sh
node --test acceptance/teams-t3-complete-20261029/probes/publication/prepare-hosting.test.mjs
node acceptance/teams-t3-complete-20261029/probes/publication/prepare-hosting.mjs
```

The default output is the fresh directory `output/teams-t3-publication-plan`.
Existing output is refused, never cleaned or overwritten. Optional paired flags:
`--baseline-dir`, `--dist`, `--firebase-config`, `--output`, `--expected-version`.
The expected version flag takes the complete `sites/.../versions/...` name and
must only change after reviewing a freshly captured LIVE baseline.

The plan retains every existing file/hash and config key/value/order. It adds
only hashed regular files from `apps/web/dist/assets`, plus the current
`dist/index.html` at reserved `/teams/index.html`. A different hash at any existing
non-Team path fails preparation. Same-hash assets are reused and not copied.
Symlink inputs/components and nonregular or unhashed assets are rejected.
Hosting `hash` values cover the level-9 gzip upload stream, matching Firebase
CLI's Hosting uploader. `new-files.json` also records `rawSha256` for the original
staged bytes. Collision checks compare the Hosting hashes; future uploads must
gzip staged bytes at level 9 and verify that exact stream hash before uploading.

Exactly five REST rewrites are prepended: `/v1/teams` and `/v1/teams/**` to
`run: {serviceId: teamsapi, region: us-central1}`, followed by `/teams`,
`/teams/**` and `/open/team/join` to `path: /teams/index.html`. Only the three Teams
route header rules from `firebase.json` are appended, translated to REST
`glob`/header-object form. Existing rewrites and headers keep their order.
The existing security-zone router already hard-reloads between Teams and the
ordinary site; no React changes or replacement root entry are needed.

Outputs:

- `baseline-files.json`, `baseline-config.json`: captured public path/hash and
  config baseline, with no creator account metadata.
- `candidate-config.json`, `union-files.json`: proposed REST config and complete
  inherited-plus-overlay file hash union.
- `new-files.json`, `stage/`: only new asset bytes and the reserved Teams entry.
- `plan.json`: source, canonical config/manifest and artifact byte hashes, counts,
  preserved root hash, and mandatory unresolved publication gates.

Canonical hashes recursively sort object keys but preserve array order; output
config JSON preserves the baseline's existing object key and rule order.
Artifact hashes separately cover the exact generated JSON bytes.

## Required before any future publication

**Confirm the deployed function's actual Cloud Run service ID.** Read
`teamsApi.serviceConfig.service` and verify its final resource segment is
`teamsapi` in `us-central1`; the candidate target is provisional until confirmed.
For example, the delivery owner may use the authorized read-only command:

```sh
gcloud functions describe teamsApi --gen2 --region us-central1 \
  --project lingxi-5e4b2 --format='value(serviceConfig.service)'
```

REST reads in this environment also require the authorized quota-project header
`x-goog-user-project: lingxi-5e4b2`. This local tool neither authenticates nor sends
those requests.

**Refresh LIVE version, config and all file pages immediately before publishing.**
Any drift invalidates this plan and requires fresh preparation/review; a matching
file count alone cannot establish current LIVE identity. The future publisher
must clone the confirmed finalized baseline and preserve all inherited files,
then apply only the sealed overlay/config. This directory is preparation evidence,
not authorization or a command to deploy its sparse stage as a full site.

Explicit publication approval and the parent's reviewed/sealed public manifest
remain required. Functional/native/signed-package acceptance is recorded by the
parent separately and is not inferred from this plan.
