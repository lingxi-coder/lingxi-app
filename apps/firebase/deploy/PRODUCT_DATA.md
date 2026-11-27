# Personal datasets in Desktop and Chat

`POST /v1/teams/data` routes to the Product Data adapter. The adapter verifies the caller's Firebase ID token and uses the existing default Firestore database for access bindings and records. Each Cloud Run instance runs the pinned Data Core on a private TLS listener and projects authorized actors into that instance only. Both processes use the attached workload identity for Firestore.

## Build and deploy

From the product checkout, prepare an isolated build context. The script archives the exact Core revision pinned by `src/lingxi-data.mjs` from the separate Data repository.

```sh
sh apps/firebase/deploy/prepare-product-data-build.sh /path/to/lingxi-data /path/to/new-context
gcloud builds submit /path/to/new-context --project "$project" --region us-central1 --tag "$image" --timeout=35m
```

Deploy the verified image digest to a private Cloud Run service in the Firebase project's region. Supply `LINGXI_DATA_PROJECT` and `LINGXI_DATA_FIREBASE_API_KEY` from that project's Firebase app configuration. Assign the runtime identity Firestore read/write access scoped to the default database, plus Firebase Authentication Viewer for checking revoked and disabled accounts. Set Firestore TTL on `lingxiDataReceipts.expiresAt` and verify that the default database rules deny direct client access to the adapter's collections. Grant Cloud Run Invoker on the service to the Teams API runtime identity.

Set `LINGXI_DATA_PRODUCT_ORIGIN` to the service's default HTTPS origin and `LINGXI_DATA_PRODUCT_PRIVATE_RUN=1` in the Teams API deployment environment. Deploy only `teamsApi`. Send an authenticated `{ "scope": { "kind": "personal" }, "command": { "action": "datasets" } }` request through the public Teams endpoint to verify the connection. Use the isolated staging project to verify creation, record access and account isolation before directing production traffic to a new adapter image.

Roll back the Teams API traffic to the prior revision if the connection needs to be disabled. To roll back an adapter update, move traffic to its prior Cloud Run revision. Keep the Firestore data and bindings available during either rollback.
