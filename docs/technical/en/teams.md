# Teams, chats and messages

Create a team, start a conversation, send a message or work with a shared Bot.

<a id="create-team"></a>

## Create a team

1. Sign in and open Teams. From the team switcher choose Create a team.
2. Enter a name and confirm. The new team opens under your account.

- [Open Teams](/teams)

<a id="invite"></a>

## Invite a member or join a team

An owner or admin opens the team details, creates an invitation and shares its link. The recipient can open the link or paste the code under Join with an invite, sign in and confirm. An expired or revoked invitation needs replacing.

<a id="chat-type"></a>

## Choose a chat

To chat privately with the LingXi assistant, use New Chat in the main workspace. Open a team to start a DM with a member, a group chat with several participants, a named channel, or a direct chat with a shared Bot. A conversation in Friends reaches the friend you select.

<a id="group-chat"></a>

## Start a group chat or channel

1. Select the team. For a group, use the + beside Direct messages, choose New message → Group and select the people and any participating Bots.
2. Confirm to open the group. A group with other people can use a generated name. If you selected only Bots, enter a name. To create a named channel, use the + beside Channels, enter the channel name, choose its participants and confirm.

<a id="direct-message"></a>

## Start a direct message (DM)

In the team, choose Direct messages → New message → Direct message and select one current member. An existing DM with that person opens again. For a friend outside the team, open the conversation in Friends. To start a personal conversation with the LingXi assistant, use New Chat.

- [Open Friends](/friends)

<a id="send-message"></a>

## Send a message or attachment

Open the intended DM, group or channel. Type a message or attach a file, wait for any uploads and choose Send. On Web and Desktop, Enter sends and Shift+Enter inserts a new line. For a reply to a particular message use Reply on that message.

If the message is pending, reconnect and check the original entry before retrying. A delivered message and a completed Bot task have separate statuses.

<a id="bot-chat"></a>

## Message a Bot directly or address it in a group

Use the + beside Bot chats, select an available shared Bot and open the chat. A Bot DM already addresses the Bot. In a group or channel, type @ and pick a participating Bot from the suggestions to request work from it explicitly.

- [Let a Bot respond to ordinary messages](teams.md#auto-reply)

<a id="auto-reply"></a>

## Let a Bot decide whether to reply

1. In this shared Bot’s publication settings on its Mac, enable “Let the Bot decide when to answer”, set its daily limit and confirm publication. This option is off by default for each Bot. If unavailable, configure Settings → Models and services → Decision on the publishing Mac first.
2. In the group, channel or Bot DM, a participant opens “Bots that answer on their own”, selects the participating Bot and saves. This setting applies to that conversation.
3. Send a normal group or channel message without an @. An enabled, available Bot can answer or stay silent. With either switch off, an ordinary unaddressed group message does not start a task for it. A Bot DM already makes a direct request. Enabling this option there lets it leave an acknowledgement unanswered.

<a id="share-bot"></a>

## Share your Bot with a team

1. On the Mac hosting the Bot, open the team details and its Bots. Choose Publish a Desktop Bot and select your local Bot.
2. Set its role, members allowed to use it, files and limits. Review the audience and materials, then confirm. A permitted member can now open its Bot chat while the publishing host is available.

- [Other ways to share a Bot](bots.md#audience)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/packages/teams/TeamSidebar.tsx](https://github.com/lingxi-coder/lingxi-app/blob/59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c/packages/teams/TeamSidebar.tsx) — Committed source (59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c)
- [lingxi-app/packages/teams/NewConversationDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c/packages/teams/NewConversationDialog.tsx) — Committed source (59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c)
- [lingxi-app/packages/teams/Composer.tsx](https://github.com/lingxi-coder/lingxi-app/blob/59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c/packages/teams/Composer.tsx) — Committed source (59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c)
- [lingxi-app/packages/teams/BotDirectory.tsx](https://github.com/lingxi-coder/lingxi-app/blob/59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c/packages/teams/BotDirectory.tsx) — Committed source (59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c)
- [lingxi-app/packages/teams/AutoReplyDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c/packages/teams/AutoReplyDialog.tsx) — Committed source (59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c)
- [lingxi-app/apps/firebase/src/shared-bots.mjs](https://github.com/lingxi-coder/lingxi-app/blob/59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c/apps/firebase/src/shared-bots.mjs) — Committed source (59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c)
- [lingxi-app/apps/electron/src/main/teams/replyTriage.ts](https://github.com/lingxi-coder/lingxi-app/blob/59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c/apps/electron/src/main/teams/replyTriage.ts) — Committed source (59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c)

- lingxi-app: HEAD 59ccc4ee7d5fa3350268d7c05f5c1ce4da16670c; Referenced input fingerprint sha256:bd99061fca730ba5fef162e15fa304f6074d4919ebd2ed565e7276696ef00cea

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
