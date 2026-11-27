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

<a id="roles"></a>

## Roles, ownership and leaving a team

- Owner: everything an admin can do, plus delete the team, make or remove admins and Make owner. Handing over ownership keeps the old owner as an admin. The single owner cannot leave.
- Admin: create and revoke invites, invite friends and remove ordinary members. Member: use permitted conversations and shared Bots, and Leave team.
- An invite link allows 1–50 uses and expires after 1 hour to 7 days. Leaving or removal ends access to the team’s conversations and shared Bots; returning needs a new invite. Delete team removes access for everyone and cannot be undone. A team has at most 200 members and a person joins at most 20 teams.

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
- Text a Bot writes never asks another Bot and never opens a round, and an @ in it triggers nothing. A Bot asks another Bot only through the delegation described next.

<a id="bot-delegation"></a>

## Let a Bot ask another Bot for help

In a group or channel, a Bot that is working on your request can hand part of it to another Bot that takes part in the same chat. It does this as a step of its own work, not because its text contains an @. It either waits for the other Bot’s answer and goes on with it, or lets the other Bot answer in the chat itself.

Both Bots have to agree. The Bot that is asked publishes whom it takes requests from: no Bot (the default), any Bot of the team, or only the Bots its maintainer lists. The Bot that asks publishes the Bots it may ask, with a daily limit. This is set only on the publishing Mac, as described under Personal and shared Bots. Everyone else reads it, without changing it, in the Bot directory and in a Bot’s details as “Accepts delegation from” and “May delegate to”.

Your own rights apply. The other Bot works for you, the person who sent the message: you must be allowed to use it, exactly as if you had written to it yourself, and it uses only what has been shared with it, never what the first Bot may use.

- How far it goes: at most two hops from your message (you → Planner → Travel → Chef, and Chef cannot ask anyone), and never a Bot that is already on the chain.
- Waiting: the first Bot’s task card says “Waiting for Travel” and nobody has to answer anything. The other Bot’s task card shows the chain, “Delegated: Planner → Travel”. The first Bot waits up to ten minutes. After that it goes on without the answer and says so, and the other Bot’s reply is still posted in the chat when it comes.
- Replying directly: when the first Bot does not need the answer, it only sends the request and does not wait. The other Bot posts its reply in the chat itself.
- If the other Bot’s Desktop is off, its request is kept for up to a day and its task card says that Bot “is offline and will reply when it’s back online”. The first Bot still stops waiting after ten minutes.
- Cancel: you, the maintainer of the Bot that was asked and the maintainer of the Bot that asked can cancel the other Bot’s task from its task card. Cancelling also stops what was started under it, and a Bot that was waiting for it goes on, told that the request was cancelled. Cancelling the first Bot’s waiting task cancels its request to the other Bot as well.
- The auto-reply log of a Bot marks a request that came from another Bot with “Delegated by Planner”.

- [Publish Bots that may ask each other](bots.md#delegation)

<a id="share-bot"></a>

## Share your Bot with a team

1. On the Mac hosting the Bot, open the team details and its Bots. Choose Publish a Desktop Bot and select your local Bot.
2. Set its role, members allowed to use it, files and limits. Review the audience and materials, then confirm. A permitted member can now open its Bot chat while the publishing host is available.

- [Other ways to share a Bot](bots.md#audience)

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/packages/teams/TeamSidebar.tsx](https://github.com/lingxi-coder/lingxi-app/blob/7fecdf10d044c94cbebbcbb4511ebede0c3c97ed/packages/teams/TeamSidebar.tsx) — Committed source (7fecdf10d044c94cbebbcbb4511ebede0c3c97ed)
- [lingxi-app/packages/teams/NewConversationDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/a3ecab5a900bae02175bfc9c2bc4507d8f2e6659/packages/teams/NewConversationDialog.tsx) — Committed source (a3ecab5a900bae02175bfc9c2bc4507d8f2e6659)
- [lingxi-app/packages/teams/Composer.tsx](https://github.com/lingxi-coder/lingxi-app/blob/b9841c6515f9716c0149ec13c487b3779bf25167/packages/teams/Composer.tsx) — Committed source (b9841c6515f9716c0149ec13c487b3779bf25167)
- [lingxi-app/packages/teams/BotDirectory.tsx](https://github.com/lingxi-coder/lingxi-app/blob/c0eeb5af8aad9e218c510597a301176cf8db53e2/packages/teams/BotDirectory.tsx) — Committed source (c0eeb5af8aad9e218c510597a301176cf8db53e2)
- [lingxi-app/packages/teams/AutoReplyDialog.tsx](https://github.com/lingxi-coder/lingxi-app/blob/c00607ed4841d7d032a571105471e9c8b7c0b6ce/packages/teams/AutoReplyDialog.tsx) — Committed source (c00607ed4841d7d032a571105471e9c8b7c0b6ce)
- [lingxi-app/apps/firebase/src/shared-bots.mjs](https://github.com/lingxi-coder/lingxi-app/blob/b9841c6515f9716c0149ec13c487b3779bf25167/apps/firebase/src/shared-bots.mjs) — Committed source (b9841c6515f9716c0149ec13c487b3779bf25167)
- [lingxi-app/apps/electron/src/main/teams/replyTriage.ts](https://github.com/lingxi-coder/lingxi-app/blob/c00607ed4841d7d032a571105471e9c8b7c0b6ce/apps/electron/src/main/teams/replyTriage.ts) — Committed source (c00607ed4841d7d032a571105471e9c8b7c0b6ce)

- lingxi-app: HEAD e74379a16bb1946787a7edf327cd8367128c4357; Referenced input fingerprint sha256:b2d7aabb4ca87d35400a12fe665e0a43040123560edaadd23961a53bf191fc66

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
