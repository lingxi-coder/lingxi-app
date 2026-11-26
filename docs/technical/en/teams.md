# Teams, chats and messages

Create a team, start a conversation, send a message or work with a shared Bot.

<a id="create-team"></a>

## Create a team

1. Sign in and open Teams. From the team switcher choose Create a team.
2. Enter a name and confirm. The new team opens under your account.

- [Open Teams](https://lingxios.net/teams)

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

- [Open Friends](https://lingxios.net/friends)

<a id="send-message"></a>

## Send a message or attachment

Open the intended DM, group or channel. Type a message or attach a file, wait for any uploads and choose Send. On Web and Desktop, Enter sends and Shift+Enter inserts a new line. For a reply to a particular message use Reply on that message.

If a message is pending, reconnect and check the original entry. Web and Desktop retry temporary failures automatically. If the entry shows Retry, use that button. A delivered message and a completed Bot task have separate statuses.

<a id="bot-chat"></a>

## Message a Bot directly or address it in a group

Use the + beside Bot chats, select an available shared Bot and open the chat. A Bot DM already addresses the Bot. In a group or channel, type @ and pick a participating Bot from the suggestions to request work from it explicitly.

- [Let a Bot respond to ordinary messages](teams.md#auto-reply)

<a id="auto-reply"></a>

## Let a Bot decide whether to reply

1. In this shared Bot’s publication settings on its Mac, enable “Let the Bot decide when to answer”, set its daily limit and confirm publication. This option is off by default for each Bot. If unavailable, configure Settings → Models and services → Decision on the publishing Mac first.
2. In the group, channel or Bot DM, a participant opens “Bots that answer on their own”, selects the participating Bot and saves. This setting applies to that conversation. A group or channel has two more settings in the same window: “Most Bots that answer one message on their own” (1 to 5, 1 by default) and “Let Bots also judge messages that @mention only people” (off by default).
3. Send a normal group or channel message without an @. An enabled, available Bot can answer or stay silent. With either switch off, an ordinary unaddressed group message does not start a task for it. A Bot DM already makes a direct request. Enabling this option there lets it leave an acknowledgement unanswered.

- [Who answers when several Bots could](teams.md#reply-rounds)
- [Which messages ask a Bot](teams.md#bot-mentions)

<a id="reply-rounds"></a>

## Who answers when several Bots could

In a group or channel, a message that asks no Bot is not judged by each Bot on its own. The service opens a round of about 15 seconds. Each Bot with auto-reply on in this chat reports how sure it is that a reply is wanted, and the service picks the surest ones, up to “Most Bots that answer one message on their own” (1 by default). The Bots not picked do not run and use no quota. A newer message from a person ends a round that is still open, and a Desktop that does not answer within the round is left out of it.

Bots are ranked by how sure they are. A Bot that was spoken to comes first, then one whose own keyword matched, then one that judged a reply is wanted, then one that is willing to reply but may stay silent. Bots on different Desktops are compared by that level first. Decision probabilities from different maintainers’ services are not compared directly: they only order Bots of the same level, and a fixed order settles any tie left. A Bot whose own judgement was “no” never answers. If a chosen Bot has not started about 20 seconds later, the next-ranked Bot takes its place, once.

<a id="bot-mentions"></a>

## Which messages ask a Bot

- @ a Bot: that Bot answers and no other Bot joins in, whatever its settings. In a group it cannot choose to stay silent.
- @ several Bots: each one answers in its own role, and each is told which others were asked too.
- Replying to a Bot’s message, or calling a Bot by name, without an @: this counts as speaking to that Bot. It answers unless the message plainly needs no answer.
- @ only people: Bots stay quiet, unless the chat turns on “Let Bots also judge messages that @mention only people”. Then the message takes part in a round like any ordinary message.
- No @ and no name: the Bots with auto-reply on in this chat take part in a round.
- Quiet hours and the cooldown set for a Bot apply only when it judges for itself, never to a message that asked it.
- The Bot’s Desktop is offline: in a group or channel the message still posts, and the chat shows that the Bot is offline and will reply when it’s back online. The Bot answers once its Desktop returns, for up to a day, and after that the task card shows it expired. A Bot DM, or a hosted Bot service, does not take a message while its host is offline.
- Editing a message does not notify a Bot, because an edit never starts a Bot task. To ask a Bot, send a new message. Recalling a message ends its open round and cancels the Bot requests still waiting to run.
- A message written by a Bot never asks another Bot and never opens a round.

<a id="share-bot"></a>

## Share your Bot with a team

1. On the Mac hosting the Bot, open the team details and its Bots. Choose Publish a Desktop Bot and select your local Bot.
2. Set its role, members allowed to use it, files and limits. Review the audience and materials, then confirm. A permitted member can now open its Bot chat while the publishing host is available.

- [Other ways to share a Bot](bots.md#audience)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/packages/teams/TeamSidebar.tsx](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/packages/teams/TeamSidebar.tsx) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/packages/teams/NewConversationDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/packages/teams/NewConversationDialog.tsx) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/packages/teams/Composer.tsx](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/packages/teams/Composer.tsx) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/packages/teams/BotDirectory.tsx](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/packages/teams/BotDirectory.tsx) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/packages/teams/AutoReplyDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/packages/teams/AutoReplyDialog.tsx) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/apps/firebase/src/shared-bots.mjs](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/apps/firebase/src/shared-bots.mjs) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)
- [lingxi-app/apps/electron/src/main/teams/replyTriage.ts](https://github.com/lingxi-coder/lingxi-app/blob/90a626b807a57c06a984bcd117bb03235aa6e0e5/apps/electron/src/main/teams/replyTriage.ts) — Committed source (90a626b807a57c06a984bcd117bb03235aa6e0e5)

- lingxi-app: HEAD 90a626b807a57c06a984bcd117bb03235aa6e0e5; Referenced input fingerprint sha256:0414ded61011432be634176b81ab5e5d501536ae723505b952a4b31c1b32bd78

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
