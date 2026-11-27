# Publish and share Local Apps

Release to your Apps library, share an installable copy or use your running desktop app from a paired device.

<a id="publish-local"></a>

## Publish a local release

1. In Code mode open the source project on the host you want to use. Ask LingXi to build or update the Local App and publish it into Apps. The local-app skill guides the release through the available LocalApp tool.
2. Open the released revision from Apps and check that it runs. For an update, open Modify on the existing app so the new release uses the same app ID. If the revision changed elsewhere, review the newer source before publishing.

<a id="share-options"></a>

## Choose how to share a Local App

- For anyone to install a copy, make the market listing public and share its link.
- To give the source to one person, share a source ZIP. The Export source ZIP entry is available only on iOS and Android.
- To use an app running on your own computer, open it from a paired device. You can continue working with records saved by its backend.

- [Publish a market listing](local-apps.md#market)
- [Share a source ZIP privately](local-apps.md#source-zip)
- [Open the running app](local-apps.md#paired-devices)

<a id="market"></a>

## Publish to the market and send the listing

1. Sign in on the device that holds the release and enter Code mode. On Desktop, choose Apps → the app’s ••• menu → Publish to market, fill in the listing and release notes, then choose Review source and publish. On iOS or Android, open the app’s details and choose Publish to market. Review the source files and confirm the upload.
2. A new listing starts hidden. Open the publication manager on the publishing device and check the release status. On Desktop this is Manage publications. After approval, choose Make public. Anyone can now open the listing.
3. Open the public market detail page and send its link in the intended DM or group. The recipient reviews the release and installs a compatible copy with their own permissions and data.

- [Send a team message](teams.md#send-message)

<a id="source-zip"></a>

## Give someone a source ZIP privately

1. The Export source ZIP entry is available only on iOS and Android. On the device that holds the release, open Apps → app details → Export source ZIP and save the file. The archive contains the release source. Runtime databases, browser data and permissions stay with the original installation.
2. Send the ZIP privately using a channel that accepts it, such as AirDrop or email. To share through a LingXi team DM, send a download link restricted to that person. Team attachments accept PNG, JPEG, PDF and plain text.
3. On iOS in Code mode, open Apps → + → Import source folder and select the ZIP. On Android in Code mode, choose Import source in Apps and select the ZIP. Open the imported app from its details.

<a id="paired-devices"></a>

## Open the same running app on your own device

1. On Desktop configure the remote app gateway and enable Remote Control. In Apps → •••, turn on Allow access from paired devices for this app.
2. With the same account, open Remote Control, pair with the computer and select the app under Apps. It uses the same backend and the records that backend saves. To stop access, turn off this app’s remote-access setting on Desktop.
3. Each remote opening gets a new browser origin. Cookies, localStorage and IndexedDB are isolated for that opening. If an app saves a counter only in the browser, reopening it on the same phone shows the initial count.

- [Pair a device](remote-control.md#same-account)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [harness-runtime/crates/local-app/assets/skill/SKILL.md](https://github.com/lingxi-coder/harness-runtime/blob/16cded0d03f9cd83b2e69436ee2367200268e088/crates/local-app/assets/skill/SKILL.md) — Committed source (16cded0d03f9cd83b2e69436ee2367200268e088)
- [harness-runtime/crates/skill/src/builtin/lingxi-api/references/local-app.md](https://github.com/lingxi-coder/harness-runtime/blob/16cded0d03f9cd83b2e69436ee2367200268e088/crates/skill/src/builtin/lingxi-api/references/local-app.md) — Committed source (16cded0d03f9cd83b2e69436ee2367200268e088)
- [lingxi-app/apps/electron/src/renderer/components/LocalApps.tsx](https://github.com/lingxi-coder/lingxi-app/blob/1f3ba253398ece3048e3cea87673884cabb95655/apps/electron/src/renderer/components/LocalApps.tsx) — Committed source (1f3ba253398ece3048e3cea87673884cabb95655)
- [lingxi-app/apps/electron/src/renderer/components/LocalAppPublish.tsx](https://github.com/lingxi-coder/lingxi-app/blob/a3e7d1af2991e7021dc8de0be5ec3e23794c7875/apps/electron/src/renderer/components/LocalAppPublish.tsx) — Committed source (a3e7d1af2991e7021dc8de0be5ec3e23794c7875)
- [lingxi-app/apps/electron/src/main/local-apps/remoteGateway.ts](https://github.com/lingxi-coder/lingxi-app/blob/1f3ba253398ece3048e3cea87673884cabb95655/apps/electron/src/main/local-apps/remoteGateway.ts) — Committed source (1f3ba253398ece3048e3cea87673884cabb95655)
- [lingxi-app/apps/ios/native/Sources/Apps/AppsPages.swift](https://github.com/lingxi-coder/lingxi-app/blob/125f50c44a4d820f02d0e3af6fd2738179b4b7b6/apps/ios/native/Sources/Apps/AppsPages.swift) — Committed source (125f50c44a4d820f02d0e3af6fd2738179b4b7b6)
- [lingxi-app/apps/android/native/app/src/main/java/com/lingxi/code/apps/UnifiedAppsPages.kt](https://github.com/lingxi-coder/lingxi-app/blob/8c3c1f06998e32297aef756212338cc0d021bd31/apps/android/native/app/src/main/java/com/lingxi/code/apps/UnifiedAppsPages.kt) — Committed source (8c3c1f06998e32297aef756212338cc0d021bd31)
- [lingxi-app/packages/teams/attachments.ts](https://github.com/lingxi-coder/lingxi-app/blob/9bc44029fff2b34f963cc1b386ed6a7080cab90b/packages/teams/attachments.ts) — Committed source (9bc44029fff2b34f963cc1b386ed6a7080cab90b)
- [lingxi-app/docs/LOCAL_APP_REMOTE_ACCESS.md](https://github.com/lingxi-coder/lingxi-app/blob/07c2dc030fe6e5e77a435479a302d9522d3965cd/docs/LOCAL_APP_REMOTE_ACCESS.md) — Committed source (07c2dc030fe6e5e77a435479a302d9522d3965cd)

- lingxi-app: HEAD e74379a16bb1946787a7edf327cd8367128c4357; Referenced input fingerprint sha256:b2d7aabb4ca87d35400a12fe665e0a43040123560edaadd23961a53bf191fc66
- harness-runtime: HEAD 16cded0d03f9cd83b2e69436ee2367200268e088; Referenced input fingerprint sha256:af27a44ee002fcbf278f333efb7f9daebd6c6fd92a6e21f86383c0deb5fea242; Product pin 16cded0d03f9cd83b2e69436ee2367200268e088

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
