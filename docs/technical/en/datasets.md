# Datasets

Create a Dataset, share it into a conversation with fixed, latest or editable access, connect a data source and bind data to apps, pages, Bots and tasks.

<a id="personal-data"></a>

## Create a Dataset and add records

In Desktop Chat open Personal data in the top bar and choose Create a dataset from JSON Schema; Use schema from the latest reply takes a schema the assistant just drafted. Select the Dataset to Add record or Select and edit. In Web Chat turn on Dataset beside the composer, or attach one of My datasets; the assistant’s create, write and read requests wait for your approval.

<a id="share-data"></a>

## Share data into a conversation

1. In the team conversation choose Add data and pick a Data location: My datasets, Team datasets or Shared in this conversation.
2. Choose Access: Use fixed version shares one immutable snapshot, See latest shares read-only current records, and Edit together lets the audience read and write. Narrow Visible fields and optional Partitions.
3. Check Share with … and choose Share data. To publish personal records instead, use Personal data → Publish to Team, pick the conversation, Preview publication audience and Confirm publication.

Revoking access or leaving the team affects later reads only. What people already saw or exported stays with them.

<a id="conversation-data"></a>

## Browse a conversation’s data

Open Conversation data in the conversation header, pick a Dataset, switch between Table and its views, read its history and send a record into the chat. If a save reports that the record changed, load the latest revision and save again.

<a id="data-sources"></a>

## Connect a database or file

On Desktop open Data sources, choose the host configuration and Test and preview. Set Private preview permissions for who may request a preview and which fields, then Prepare report to publish a reviewed, versioned report into a conversation. MySQL, PostgreSQL, SQLite, MongoDB, Redis, CSV and JSON are read on the owner’s Mac with read-only credentials from Keychain; raw rows and connection details stay there. Disable and revoke ends later reads.

<a id="bindings"></a>

## Use a Dataset in apps, pages, Bots and tasks

- Local App: the app declares a data binding. On Desktop open Apps → the app’s details panel → Data bindings and enter the Dataset ID, the share grant and, for read access, an optional fixed snapshot.
- Interactive page: from a visualization choose Publish interactive page, pick the conversation and its Dataset share, then review the code, assets and permitted actions before publishing.
- Shared Bot: the maintainer opens Bot Dataset permission in the conversation, enters the Dataset ID and task plan and chooses Review plan. It applies to the current published Bot in that conversation and can be revoked there.
- Scheduled task: open the task in Scheduled, choose Bind Dataset, enter the Dataset ID and task plan and review it.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/packages/teams/PersonalChatData.tsx](https://github.com/lingxi-coder/lingxi-app/blob/1874d9bc1fba41107a616a4c4b8fbdfc7c1554dc/packages/teams/PersonalChatData.tsx) — Committed source (1874d9bc1fba41107a616a4c4b8fbdfc7c1554dc)
- [lingxi-app/packages/teams/DatasetConversation.tsx](https://github.com/lingxi-coder/lingxi-app/blob/f782f22fab0f137e1206ef8314a2b25e3a225a34/packages/teams/DatasetConversation.tsx) — Committed source (f782f22fab0f137e1206ef8314a2b25e3a225a34)
- [lingxi-app/packages/teams/ConversationData.tsx](https://github.com/lingxi-coder/lingxi-app/blob/1874d9bc1fba41107a616a4c4b8fbdfc7c1554dc/packages/teams/ConversationData.tsx) — Committed source (1874d9bc1fba41107a616a4c4b8fbdfc7c1554dc)
- [lingxi-app/packages/teams/DatasetPage.tsx](https://github.com/lingxi-coder/lingxi-app/blob/f782f22fab0f137e1206ef8314a2b25e3a225a34/packages/teams/DatasetPage.tsx) — Committed source (f782f22fab0f137e1206ef8314a2b25e3a225a34)
- [lingxi-app/packages/teams/BotDatasetTaskSettings.tsx](https://github.com/lingxi-coder/lingxi-app/blob/7fecdf10d044c94cbebbcbb4511ebede0c3c97ed/packages/teams/BotDatasetTaskSettings.tsx) — Committed source (7fecdf10d044c94cbebbcbb4511ebede0c3c97ed)
- [lingxi-app/apps/electron/src/renderer/components/DesktopDataSources.tsx](https://github.com/lingxi-coder/lingxi-app/blob/ee82bc7e88681e7446fcf04354ca4c9c39808e45/apps/electron/src/renderer/components/DesktopDataSources.tsx) — Committed source (ee82bc7e88681e7446fcf04354ca4c9c39808e45)
- [lingxi-app/apps/electron/src/renderer/components/LocalAppDataSettings.tsx](https://github.com/lingxi-coder/lingxi-app/blob/7fecdf10d044c94cbebbcbb4511ebede0c3c97ed/apps/electron/src/renderer/components/LocalAppDataSettings.tsx) — Committed source (7fecdf10d044c94cbebbcbb4511ebede0c3c97ed)

- lingxi-app: HEAD e74379a16bb1946787a7edf327cd8367128c4357; Referenced input fingerprint sha256:b2d7aabb4ca87d35400a12fe665e0a43040123560edaadd23961a53bf191fc66

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
