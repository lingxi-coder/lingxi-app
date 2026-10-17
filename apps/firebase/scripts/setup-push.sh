#!/usr/bin/env bash
set -euo pipefail

# Fixed project/account boundaries; never creates a service-account key.
project_id=lingxi-5e4b2
service_account="push-api@${project_id}.iam.gserviceaccount.com"
gcloud services enable fcm.googleapis.com fcmregistrations.googleapis.com firestore.googleapis.com cloudscheduler.googleapis.com --project="$project_id"
if ! gcloud firestore databases describe --database=push-notifications --project="$project_id" >/dev/null 2>&1; then
  gcloud firestore databases create --database=push-notifications --location=us-central1 --type=firestore-native --delete-protection --project="$project_id"
fi
if ! gcloud iam service-accounts describe "$service_account" --project="$project_id" >/dev/null 2>&1; then
  gcloud iam service-accounts create push-api --display-name='LingXi push notifications' --project="$project_id"
fi
gcloud projects add-iam-policy-binding "$project_id" --member="serviceAccount:$service_account" --role=roles/datastore.user \
  --condition="expression=resource.name == 'projects/$project_id/databases/push-notifications',title=push-database-only" --quiet >/dev/null
for role in roles/firebaseauth.viewer roles/firebasecloudmessaging.admin; do
  gcloud projects add-iam-policy-binding "$project_id" --member="serviceAccount:$service_account" --role="$role" --condition=None --quiet >/dev/null
done

printf '%s\n' 'Push database and runtime identity are ready. Deploy only pushApi, prunePushRegistrations and firestore:push-notifications.'
