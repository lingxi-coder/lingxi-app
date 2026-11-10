# Friends clients (F1 to F4)

Status: the client contract on top of the F0 backend ([FRIENDS.md](FRIENDS.md)). Nothing here is deployed. One
decision-numbered spec (D1...D30) for four clients: **Web and Desktop** (one React implementation in
`packages/teams`), **iOS** (SwiftUI) and **Android** (Compose). D1 to D20 are F1 and F2 (the Friends tab and the chat),
D21 to D25 are F3 (Bots in a friendship) and D26 to D30 are F4 (inviting a friend to a team). Section 3 lists what each
platform adds. If a client and this document disagree, fix the client; if the document is wrong, change it and the
vectors together.

It relies on two F0 follow-ups that [FRIENDS.md](FRIENDS.md) documents (source only, like the rest of F0): the chat
summary on friend cards (`lastSeq`, `lastMessage`, `lastChangeSeq`, `lastReadSeq`; D13) and the person's name on block
entries (`displayName`, `handle?`; D12). Section 6 lists the consequences.

F3 and F4 rely on the F3/F4 backend ([FRIENDS.md](FRIENDS.md), "Shared Bots in a friendship" and "Team offers"; source
only, like the rest): the SharedBots commands in a pair team, and the actions `offerTeam`, `acceptOffer` and
`declineOffer` with the owner-readable `users/{uid}/teamOffers` documents.

In scope: friends list, requests, Add friend (link, @username, email), profile and discoverability settings, blocked
list, 1:1 chat, and for F3 and F4 the Bots of a friendship (list, chat with one, share and stop sharing from Desktop)
and inviting a friend to a team (the invitation, its answer and the badge). Out of scope: QR codes (no client has a QR
generator; none is added), account-deletion cleanup, rename fan-out, and the maintainer opening a friend's Bot chat
from the Friends screens (open question 12).

Shared test vectors, loaded by every client's unit tests and never adjusted per platform:
[`packages/teams/fixtures/friends-vectors.json`](../packages/teams/fixtures/friends-vectors.json) (section 5).

## 1. Decisions

### Placement and data

**D1 Where Friends lives.**

| Client | Entry |
| --- | --- |
| Desktop | Rail mode `friends` after `teams`, before `apps`. `SidebarMode` and `AppPage` gain `friends`; `pageForMode`, `reconcileSidebarMode` and `restoredSidebarMode` treat it like `teams`. The sidebar is the Friends root (D3); the main area is a chat or one page (Add friend, Profile settings, Blocked people). |
| Web | `/friends` (list and chat) and `/friends/add`. Both belong to the `teams` security zone (`securityZone`, `firebase.json` headers and CSP, exactly as `/teams`). |
| iOS | `AppTab.friends` after `.teams` and before `.settings`, titled "Friends" / "好友". |
| Android | `AppTab.Friends` after `Teams` and before `Settings`, same title. |

**D2 Account and data sources.**
- Friends uses the Teams Firebase account. Signed out, Friends shows the Teams signed-out state with `friends_signin`.
- Live data is owner-only Firestore listeners: `users/{uid}/friends` (all three states, `FriendCard`),
  `users/{uid}/blocks` (`BlockEntry`) and, from F4, `users/{uid}/teamOffers` (`TeamOffer`). The cards listener alone feeds the
  whole friends list: names, last message, last-activity order and unread all come from the cards (D13); no listener runs
  per friend. The offers listener feeds the Team invitations section and the badge (D27, D28) and runs as long as the cards
  listener does. One-shot `get` of `profiles/{uid}` for a friend (D10).
- Everything else is HTTP with the Firebase ID token: `GET /v1/friends/profile` and `POST /v1/friends` through a
  `friendsTransport` shaped like `teamsTransport` (same error shape, same 20 s timeout).
- Chat data is the Teams message layer, unchanged (D9); so are the Bots of a friendship, which are the SharedBots commands
  of that layer (D21 to D25).
- On the first open per signed-in account, and on every open of Add friend or Profile settings, call
  `GET /v1/friends/profile` (it creates the profile and the first friend code on first use). The result lives in
  memory only. A failure shows an inline error with `common_retry`; it never blocks the list.
- There is no other cache; Firestore's own persistence is all the offline support there is.

Wire types are in appendix A.

### Screens

**D3 Friends root.** Header: `friends_title`, an Add friend button (`friends_add`) and a "more" menu with exactly two
items, `friends_profile_title` and `friends_blocked_title` (these two screens are reached only from here, on every
client). Body, top to bottom:
1. Name banner when `profile.displayName` is `Member` (the backend fallback): `friends_name_prompt` with the action
   `friends_name_prompt_action`, which opens Profile settings.
2. Team invitations (F4), only when there is at least one (D27).
3. Requests, only when there is at least one (D4).
4. Filter field `friends_filter_placeholder` (D5). Always visible on Desktop and Web; on mobile it is the list's search
   affordance. No match: `friends_filter_empty`.
5. Friends, one row per `friends` card in D5 order. Row: avatar (the platform's existing `avatarColor` / `initials`),
   name (one line, bold while unread), one preview line, time of the newest message (`lastMessage.at`; none when there
   is no message), unread badge (`badgeText` of the D13 unread). Tap opens the chat (D9). Preview is the Teams
   `conversationPreview` rule applied to the card's `lastMessage` ("You: " prefix, recalled and non-text labels
   included); a card without `lastMessage` shows `teams_preview_none`. Every friend gets a preview and a badge: they all
   come from the card (D13).

States: loading `friends_loading` until the first cards snapshot; listener error `friends_load_error` with
`common_retry`; no friends, no requests and no invitations: `friends_empty_title`, `friends_empty_body` and a primary `friends_add`.
On wide layouts follow whatever the Teams tab does (list pane plus chat pane).

**D4 Requests.** Heading `friends_section_requests`, then `friends_requests_received`, then `friends_requests_sent`,
each in D5 request order and limited to 20 rows with a final row `friends_show_all(n)` that expands it (incoming
requests are not capped by the backend). Row: avatar, name, @handle if any, the sender's note (at most 2 lines, plain
text, never Markdown or links).
- Received: `friends_request_accept` (`accept`) and `friends_request_decline` (`decline`, no confirmation). After
  Accept show `friends_now_friends(name)` once and stay on the list.
- Sent: secondary text `friends_request_sent` and `friends_request_cancel` (`cancel`, no confirmation).
- `REQUEST_UNAVAILABLE` on Accept means the request is gone: show its message; the listener removes the row.
- A row is disabled while one of its actions is in flight.
- Team invitations (D27) are a section of their own above this one, with the same 20-row limit.

**D5 Ordering and filter** (vectors `sort`, `filter`).
- The whole `users/{uid}/friends` collection is one list: incoming requests, then outgoing requests, then friends
  (vector `sort.cards`). The Requests section (D4) and the Friends section (D3) are slices of it.
- Requests: incoming before outgoing, then newest `since` first, then `uid` (vector `sort.requests`).
- Friends, by last activity and then by name (vector `sort.friends`):
  1. friends with a `lastMessage` come before friends without one;
  2. among those with one, the newer `lastMessage.at` first, compared as whole epoch milliseconds (a `lastMessage`
     whose `at` is missing or unreadable counts as none);
  3. then `displayName` by the platform's **locale-aware collation at primary strength** (letters compared ignoring
     case and accents), so Chinese names sort by pinyin under zh-Hans and every other locale gets its own convention;
  4. then `uid`, compared as text by code point (not numerically).

  There is no section index.
- The collator is built once per UI locale (the language the app shows its strings in, not the device region):
  - Web, Desktop: `new Intl.Collator(locale, { sensitivity: 'base' })`; `locale` is the component's `locale` prop, else
    `zh ? 'zh-CN' : 'en-US'` (as `TeamDetailsDrawer` does).
  - iOS: `a.compare(b, options: [.caseInsensitive, .diacriticInsensitive], range: a.startIndex..<a.endIndex, locale: locale)`.
    Not `localizedStandardCompare`: it forces an order on names that differ only in case, so such names would never
    reach the `uid` rule.
  - Android: `java.text.Collator.getInstance(locale)` with `strength = Collator.PRIMARY`.

  Primary strength is what makes "Sam" and "sam" tie, so that `uid` decides, identically on every platform; a
  default-strength collator orders them by case instead. Digits, spaces, punctuation and non-Latin scripts follow each
  platform's own tables, so their relative order may differ between platforms. The vectors therefore hold ASCII-letter
  names only (section 5).
- `fold(s)`, used by the filter only: NFKC, lowercase (full Unicode mapping, no locale), NFD, remove U+0300-U+036F, NFC.
- Filter: needle = `fold(trim(query))`; if it starts with `@`, drop that character and trim again. Empty matches
  everyone; otherwise a card matches when `fold(displayName)` or `fold(handle)` contains the needle.

**D6 Add friend screen**, top to bottom.
1. *My friend link* (`friends_link_title`): `profile.friendCode.link`, selectable, one line, middle-truncated.
   `friends_link_copy` copies it and answers `friends_link_copied`. `friends_link_share` opens the native share sheet
   with `friends_link_share_message` + space + link (mobile; Web only when `navigator.share` exists; Desktop has none).
   `friends_link_reset` (`rotateCode`) and `friends_link_off` (`revokeCode`) each ask first
   (`friends_link_reset_confirm`, `friends_link_off_confirm`). With `friendCode == null` show `friends_link_is_off`
   and `friends_link_create` (`rotateCode`, no confirmation). `friends_link_disclosure` always sits under the card.
2. *Find someone* (`friends_search_title`): one text field (`friends_search_placeholder`; no autocapitalisation or
   autocorrect; Return looks up) and `friends_search_action`. The text is classified by D7; `invalid` shows
   `friends_search_hint` and keeps the button disabled. Never look up while typing (lookups are rate limited).
3. *Result card* after a lookup: avatar, name, @handle if any, one action by relation.

| relation | action |
| --- | --- |
| `none` | `friends_add`, with an optional note field `friends_note_placeholder` (at most 200 characters; not for code lookups) |
| `incoming` | `friends_request_accept` |
| `outgoing` | disabled `friends_request_sent` |
| `friends` | `friends_relation_friends` and `friends_open_chat` |

Lookup errors appear under the field (D18); a new lookup replaces the card; editing the field clears it.

**D7 One field, three kinds** (vectors `query`). Classify the trimmed text, first match wins:
1. empty: invalid;
2. **code**: 32 characters of `[A-Za-z0-9_-]`, or a link `lingxi://friend/add?code=...`,
   `https://lingxios.net/open/friend/add?code=...` or `https://lingxios.net/friends/add?code=...` with exactly the one
   `code` parameter and no userinfo, port or fragment;
3. contains `@` after position 0: **email**, lowercased, at most 254 characters, `^[^\s@]+@[^\s@]+\.[^\s@]+$`,
   otherwise invalid;
4. an optional leading `@` and then `^[A-Za-z0-9_]{3,24}$`: **handle** (without the `@`); otherwise invalid.

The first call is `lookup` with that one proof (`{handle}`, `{email}` or `{code}`); its result only fills the card.
The confirming tap sends handle: `request {via:'handle', handle, requestId, note?}`; email:
`request {via:'email', email, requestId, note?}`; code: `redeem {code, requestId}`. On a handle or email lookup whose
relation is `incoming`, send `accept {otherUid}` instead; a code always redeems (the link is the owner's standing
consent and replaces a pending request). A bare uid is never a proof. A `request` that answers `friends` (a mutual
request) counts as accepted. Success: `request` shows `friends_request_sent_to(name)`; `redeem`, `accept` and a mutual
request open the chat and show `friends_now_friends(name)`.

**D8 Opening a friend link** (`friend/add?code=`). Never redeem on open. Signed out: sign in first and keep the code
as a pending intent (Web: localStorage `lingxi.friends.addIntent.1`, 7 days, same helper shape as the Teams
invitation intent; Desktop and native: memory only, never on disk). Signed in: open Friends and run `lookup {code}` in
a dialog or sheet:
- relation other than `friends`: title `friends_confirm_title(name)`, body `friends_confirm_link_body`, buttons
  `friends_add` and `common_cancel`; confirm sends `redeem {code, requestId}` (D7 success);
- relation `friends`: `friends_relation_friends` with `friends_open_chat`;
- `NOT_FOUND` (revoked, rotated, blocked, your own link): `friends_link_invalid`.

Drop the code after redeem, cancel or failure. Never log it or put it in analytics or crash breadcrumbs.

### Chat

**D9 Friend chat.** Reuse each client's Team conversation view and message layer with `teamId = card.teamId`,
`generation = card.generation` (read from the card, never hard-coded) and the single conversation `friend_chat`,
opened directly (no list, no selection).
- **Pair-pinned workspace.** The chat gets its own `TeamsWorkspace` instance pinned to the pair: a synthesized
  `TeamSummary` (`id` = teamId, `name` = friend's name, `memberCount` 2, `role` member, `generation`) and a two-person
  directory built from the cards (me: `profile.displayName`; friend: the card). It makes no `GET /v1/teams` call and
  no team-detail call. Never select the pair team in the Teams tab's workspace. From F3 it also loads the friendship's Bot
  cards (`listSharedBots`, D21).
- Hidden compared with a Team: conversation list or sidebar, new conversation (group, channel, Bot DM), team
  details drawer or page, members, invites, team badge. F3 adds back one thing, the Bots entry (D21), through which a Bot
  DM opens (D22). Kept: send, edit, recall, reply, read marks (D13), offline queue, new-messages divider, jump to latest.
- Header: avatar and the friend's name (D10 override if any, else the card), `@handle` as subtitle when known, a back
  affordance on mobile, and the menu of D10.
- Ended friendship (the card disappears, or a send fails with `MEMBERSHIP_REQUIRED`, `STALE_MEMBERSHIP` or
  `CONVERSATION_REQUIRED`): replace the body and composer with `friends_chat_unavailable` and a Back action. The Teams
  layer already drops the stale generation's outbox. A Bot command of the pair workspace that fails with one of the same
  codes ends it the same way (D25).

**D10 Friend menu** (in the chat header).
- **View profile**: read `profiles/{uid}` once (ignore failure). If `displayName` or `handle` differ from the card, use
  the profile's values for this friend for the rest of the session (in-memory map; cards are never written). Shows
  name, @handle and `friends_since(date)` from the card's `since`.
- **Remove friend**: confirmation `friends_remove_title(name)` + `friends_remove_body`, destructive button
  `friends_menu_remove`, then `remove {otherUid}`; on success go back to the list.
- **Block**: confirmation `friends_block_title(name)` + `friends_block_body`, destructive button `friends_menu_block`,
  then `block {otherUid}`; on success go back to the list. The backend keeps the person's name on the block entry (D12).

**D11 Profile settings.** A form: Display name (`friends_profile_name`; trimmed, 1-80 UTF-16 units, required);
Username (`friends_profile_handle`; `^[A-Za-z0-9_]{3,24}$`, hint `friends_profile_handle_hint`; once set it can be
changed but not removed, so an empty field means "leave as is"); two switches `friends_profile_find_handle` and
`friends_profile_find_email` (default on) with footer `friends_profile_find_footer`. One Save button (`common_save`),
enabled only when the form changed and is valid; it sends one `profile` action with only the changed fields.
`HANDLE_TAKEN` appears under the Username field; every other error in a form-level line (D18).

**D12 Blocked people.** Rows from `users/{uid}/blocks`, newest `at` first: the name, `@handle` when the entry has one,
and `friends_unblock` (`unblock`, no confirmation). Footer `friends_unblock_footer`; empty `friends_blocked_empty`. The
name is the entry's `displayName`, which the backend writes when the block is made (a snapshot, like card names); if it
is missing or empty show `Member` (the existing `teams_member` string). Nothing is remembered on the device.

**D13 Previews, unread and read marks** (vectors `unread`).
- Everything the list knows about a chat comes from its `friends` card, so the one cards listener (D2) is enough. The
  card carries `lastSeq`, `lastMessage` (the Teams pointer `{id, seq, at, author, preview, recalled?}`, whole),
  `lastChangeSeq` and `lastReadSeq` (FRIENDS.md, "Friend list summaries"). `lastMessage` is the preview (D3) and the
  activity time (D5). **Unread = `max(0, lastSeq - lastReadSeq)`.** A missing counter is 0, and a missing or `null`
  `lastMessage` means no message: a new friendship's card has none of the four fields until the first message or read
  mark. `lastChangeSeq` is the backend's ordering token and clients ignore it. The backend copies only a read mark made
  under the card's own membership generation, so the list compares no generation.
- No per-friend conversation listener, no watcher, no cap: all friends (at most 500) get preview, order and unread from
  the one listener. A card whose `lastMessage` is not the whole pointer counts as having none; one bad card never
  fails the list.
- **The open chat** (D9) keeps using the Team conversation listener and message layer of its pair-pinned workspace, so
  it sees its own `lastSeq` and `reads/{uid}` live. Marking read is the existing Teams `read` command (chat open and the
  newest message visible); it writes `reads/{uid}` as today, and the backend copies the mark onto the card's
  `lastReadSeq` a moment later. The list's unread and the tab badge therefore clear shortly after the chat marks itself
  read, not instantly; a lost copy is healed by the next read mark.
- While a friend's chat is open and visible the client may treat it as read: show 0 unread for that friend in the list
  and in the badge (it also hides the lag after a message arrives or after you send one), and use the card's value again
  when the chat closes. This is a display choice; the vectors cover the card arithmetic only.

**D14 Badge** (vectors `badge`, `offers.badge`). `incoming request count + sum of the D13 unread over the friends cards +
the number of live team offers (D28)`, rendered by the client's existing tab-badge rule (nothing at 0, the number up to 99, `99+` from 100). Outgoing requests add
nothing. Shown on the Desktop rail button (accessibility label `desktop_friends_rail_unread`, in Desktop's
`navigationCatalog.ts`, parallel to `desktop_teams_rail_unread`), on the iOS and Android Friends tab (accessibility
`tabs_badge_unread`) and, on Web, in the page header and document title. Counted only while signed in.

**D15 Add friend from a Team member.** Each human member except me gets an Add friend action (`friends_add`) in the
Team member list (Desktop and Web drawer members tab, iOS `TeamDetailsPage`, Android `TeamDetailsScreens`); Bots have
none. It runs `lookup {teamId, uid}` into the D6 result card (note field included); confirm sends
`request {via:'team', teamId, uid, requestId, note?}` (relation `incoming`: `accept`). A member who is already a
friend shows `friends_relation_friends`.

### Links, push, errors

**D16 Links and push** (vectors `links.typed`, `links.push`, `links.tabForLink`). Grammar as in FRIENDS.md. Typed
destinations: `Add(code)`, `Requests`, `Chat(uid)`, produced only from an already parsed canonical link
(iOS `FriendLink(CanonicalDeepLink)`, Android `FriendLink.from(DeepLink)`, both implemented).
- `Add` runs D8. `Requests` selects Friends and scrolls to the top of the requests area: Team invitations when there are
  any (D27), else Friend requests (D4). `Chat(uid)` selects Friends and opens that
  friend's chat once the first cards snapshot has arrived; a uid that is not a current friend opens the root with
  `friends_chat_unavailable`.
- Friends links are independent of the workspace and session (like `team/*`): never queued behind a session,
  `needsContextRebind` false, tab target `friends`. Signed out: sign in first and keep the intent.
- A push `route` is accepted only as `team/inbox`, `friend/requests`, `friend/chat?uid=<[A-Za-z0-9_-]{1,128}>` (no
  percent-encoding) or the existing `remote/...`; anything else is dropped, and a push never carries `friend/add`.
  Notification taps keep Teams' recipient and expiry checks. F3 and F4 add no route (D29).

**D17 Web open page** (`OpenLinkPage`). `viewer` paths: `friend/add?code=C` becomes `/friends/add?code=C`,
`friend/requests` becomes `/friends`, `friend/chat?uid=U` becomes `/friends?chat=U` (vectors `links.webViewer`).
`/friends/add` stores the intent (D8), then removes the code from the address bar with `history.replaceState`.
`OpenLinkPage`'s own text stays inline (`zh ? ... : ...`).

**D18 Errors and request ids** (vectors `errors`, `requestId`). `error.code` maps to a string key; the message shows
under the control that failed, never as a blocking alert (destructive confirmations are the only alerts).

| code | key |
| --- | --- |
| `NOT_FOUND` | `friends_error_not_found` |
| `RATE_LIMITED` | `friends_error_rate_limited` |
| `FRIEND_LIMIT` | `friends_error_friend_limit` |
| `REQUEST_LIMIT` | `friends_error_request_limit` |
| `HANDLE_TAKEN` | `friends_error_handle_taken` |
| `REQUEST_UNAVAILABLE` | `friends_error_request_unavailable` |
| `STALE_REQUEST` | `friends_error_stale_request` |
| `SIGN_IN_REQUIRED`, `INVALID_SESSION` | `friends_error_session` |
| `SERVICE_UNAVAILABLE`, transport failure, timeout | `friends_error_network` |
| `INVALID_INPUT`, `INVALID_JSON` | `friends_error_invalid_input` |
| `MEMBERSHIP_REQUIRED`, `STALE_MEMBERSHIP`, `CONVERSATION_REQUIRED` (chat only) | `friends_chat_unavailable` |
| anything else (`REQUEST_ID_CONFLICT`, `PAIR_TEAM`, unknown) | `friends_error_generic` |

`request` and `redeem` carry a `requestId` (`^[A-Za-z0-9_-]{8,64}$`; a UUID is fine). Mint it when the person taps the
confirming button; reuse it only to retry that same confirmation after an **unknown outcome** (network failure,
timeout, `SERVICE_UNAVAILABLE`; the vectors' `definitive: false`); drop it after any other result and whenever the proof
or note changes. Never persist it.

**D19 Teardown.** On sign-out or uid change stop the cards, blocks and offers listeners and the open chat's workspace (its
Bot cards and any Bot DM with it), and clear cards, offers, profile, name overrides, lookup state, the invite picker's
session marks (D26) and open dialogs. The pending friend-code intent of D8 survives (it belongs to the device). Nothing
about friends is written to disk except that Web intent.

**D20 Strings.** `friends_*` keys in `resources/translations/*.json` (section 4; F3 and F4 add 34 entries). Placeholders
follow the existing conventions (iOS token in the key, Android `_fmt` sibling with positional tokens).

### Bots in a friendship (F3)

A friendship is a Team of two, so its Bots are the Teams SharedBots of [TEAMS.md](TEAMS.md) on the pair team: the
backend adds nothing but the restriction that a pair team's `createConversation` accepts only `bot_dm`
([FRIENDS.md](FRIENDS.md), "Shared Bots in a friendship"). What the client adds is a way in (D21), the chat (D22), sharing
and stopping (D23), the Desktop host (D24) and the end of the friendship (D25).

**D21 Bots entry in the friend chat.**
- The friend chat header (D9) gets a **Bots** action (accessibility label and page title `friends_bots_title`; the
  platform's existing Bot glyph). Web and Desktop: a button beside the friend menu that opens a panel over the chat;
  iOS: a toolbar button that pushes `FriendBotsView`; Android: an action that opens `FriendBotsScreen`. It is always
  shown, also with no Bots, so that a friend can find it (and a Desktop user can share, D23).
- The panel lists the Bot cards of this friendship: the `bots` of `listSharedBots` run on the pair team
  (`{teamId: card.teamId, generation: card.generation}`) by the chat's pair-pinned workspace. The mobile messaging models
  already refresh them for any team; Web and Desktop must stop skipping `BotCards` when `pair` is set (`refresh` and
  `detail` stay skipped). Nothing is filtered: the server returns the Bots the viewer may use (the friend's, published for
  `all` or listing the viewer) and the viewer's own (with `state: 'paused'` after stopping). A paused Bot of the friend is
  not returned.
- Each row is the Teams Bot card unchanged (name, description, presence, the Message action, `teams_bot_owner(name)` with
  the maintainer's name from the pair directory, `teams_bot_not_permitted`, the offline and disabled labels) except that
  the audience and quota lines are not shown: a friendship has no audience to choose. Loading `teams_bots_loading`; a
  failed read `teams_bots_unverified`; empty `friends_bots_none`.
- Under the list, always: `friends_bots_note` (a Bot runs on its owner's Desktop and the owner can read every conversation
  with it, which is the backend's reader rule). On Web, iOS and Android, which cannot publish, also
  `friends_bots_share_hint`; Desktop shows the share action instead (D23).
- Message on a usable Bot opens its Bot DM (D22). The panel offers no other Bot management than D23.

**D22 Bot DM.**
- Message calls the pair workspace's Bot DM start: reuse the viewer's own `bot_dm` with that Bot if the conversation list
  has one (`findBotDm`), else `createConversation {kind: 'bot_dm', participantUids: [me], sharedBotIds: [sharedBotId],
  title: bot.name}` with a fresh `conversationId`, queued and retried by the same offline machinery as any Teams command
  (Web `TeamsWorkspace.startBotDm`, iOS `TeamsWorkspace.openBotDM`, Android the Bot DM start of `TeamsTab`). A client never
  sends `group` or `channel` to a pair team (400 `PAIR_TEAM`); there is no new-conversation dialog (D9).
- The Bot DM is shown with the **existing Teams conversation view** on the same pair-pinned workspace: Bot name and
  presence in the header, the composer, Bot run cards, usage and artifacts, as in a Team. The friend header and menu of D9
  belong to `friend_chat` only (Web and Desktop: pass `friend` to `TeamConversationView` only for that conversation, and
  keep Bot runs enabled for a `bot_dm`). A Back affordance returns to the friend chat: the pair workspace selects
  `friend_chat` again (system back on mobile).
- A Bot DM never moves the friend list or the badge: the card summary mirrors `friend_chat` only (D13, FRIENDS.md).
  Inside the open workspace a Bot DM marks itself read like any Teams conversation. v1 shows no unread marker on the Bots
  entry (open question 11).
- A message in a Bot DM pushes `friend/chat?uid=<the other friend>` (D29); the tap opens the friend chat, from which the
  Bots entry is one tap away.

**D23 Sharing from Desktop, and stopping.**
- **Share** (Desktop only, where `TeamsApi` has `localBots`, `selectSharedFiles` and `publishLocalBot`, the existing
  `native` test of `BotDirectory`). In the Bots panel a button `friends_share_bot_with(name)` (name: the friend's name, D10)
  opens the Teams publication wizard (choose a local Bot, role, read-only files, resource bindings, quotas, review and
  confirm, then `publishLocalBot`) with these differences: `teamId = card.teamId`, `generation = card.generation` (1); the
  audience is fixed to `{mode: 'all'}`, which in a friendship is the friend, so the audience choice and member checkboxes
  are not shown and the review line names the friend (inline text in `packages/teams`, D20); the review also shows
  `friends_bots_note`. Quotas keep the wizard's defaults (100 / 20 / 100 per UTC day for team / member / Bot, editable).
  Choosing the same local Bot again prefills its publication and publishes the next revision, which also restores a Bot
  that was stopped.
- **Stop sharing** (every client, on the viewer's own `published` Bot): `friends_share_bot_stop`, with a confirmation
  (title `friends_share_bot_stop`, message `teams_bot_unbind_detail`, destructive `friends_share_bot_stop`,
  `common_cancel`), then the Teams `pauseSharedBot {sharedBotId, expectedRevision: bot.revision}` (iOS
  `TeamMessagingModel.pause`, Android `TeamMessagingModel.pause`; Desktop's service already stops the binding). It is a
  pause: the friend's list drops the Bot and a send is 403 `BOT_USE_REQUIRED` (shown as the Teams text), the history stays
  readable to both, and the row shows the Teams paused state for its maintainer. Refresh the Bot cards afterwards.

**D24 Desktop host discovery of pair teams** (contract for `apps/electron/src/main/teams/connector.ts`).
`discover()` runs a binding only if `TeamsService.list(uid)` contains its team with the binding's generation, and a pair
team is never in that list (FRIENDS.md), so a Bot bound to a friendship would be skipped on every pass. Therefore:
1. A binding whose `teamId` starts with `f_` (`PAIR_PREFIX`, a reserved id space: no ordinary team can have such an id)
   skips that membership check and uses its stored generation (always 1). Every other check and command of `discover()`
   is unchanged: the server decides, and every host command already checks the team and the member
   (`listSharedBots`, `activateDeployment`, `heartbeatDeployment`, `listHostTasks`, `startTask`, `completeTask`).
2. `TeamsService.list` must keep excluding pair teams (the Teams UI must never show them); the bypass is the connector's.
3. After the friendship ends the next host command answers 403 `MEMBERSHIP_REQUIRED`, which the connector's existing
   `definitive(error)` classifies as final: it stops the binding as it does today and also records the team id in an
   in-memory `ended` set, so the five-second discovery loop does not call a dead team forever: no loop of `discover()`
   (bindings, the replay of a pending publication, delivery retries) sends that team another call, and its pending
   deliveries are dropped instead of retried. `revoke()` (sign-out, account change) clears the set. Nothing is written
   to the private store; the binding and its snapshot stay until the person removes the Bot, as for a team the person left.
4. A new friendship with the same person has a new team id (FRIENDS.md): the old binding never matches it and nothing
   resumes by itself; the person shares again (D23).
5. Tests (Desktop): a binding `f_...` missing from `list()` still activates, heartbeats and lists host tasks; after a
   `MEMBERSHIP_REQUIRED` it is stopped, not called again, and called again after `revoke()`; an ordinary binding missing
   from `list()` is still skipped.

**D25 When the friendship ends.**
- Every Bot command of the pair workspace that fails with `MEMBERSHIP_REQUIRED`, `STALE_MEMBERSHIP` or
  `CONVERSATION_REQUIRED` (listing Bots, creating or sending in a Bot DM, pausing) counts as the end of the chat (`chatEnded`,
  D9): the whole friend area, including an open Bots panel or Bot DM, is replaced by `friends_chat_unavailable` and Back.
  The card disappearing ends it too.
- Nothing is cleaned up by the client: the Bots, their chats and the host's access end with the pair team (FRIENDS.md);
  Desktop's connector stops its binding (D24). A new friendship starts with no Bots and no Bot chats.

### Team offers (F4)

A manager of an ordinary team invites a friend without a link ([FRIENDS.md](FRIENDS.md), "Team offers"): an offer
document appears in the friend's own `users/{uid}/teamOffers`, the friend joins or declines, and the sender is never
told which.

**D26 Invite friends** (vectors `offers.entry`, `offers.picker`).
- **Entry.** In Team details, in the Invites area above "Create invite link": a button `friends_invite_action` (Web and
  Desktop: the Invites tab of `TeamDetailsDrawer`, which `drawerTabs(role)` already gives to owners and admins only; iOS: the
  invites section of `TeamDetailsPage`; Android: the invites section of `TeamDetailsScreens`). Only an owner or an admin sees
  it, whatever the member count: a full team still shows it and the server answers `MEMBER_LIMIT` (D30).
- **Picker.** A dialog (Web and Desktop), sheet (iOS) or bottom sheet (Android) titled `friends_invite_title(teamName)`:
  the filter field (`friends_filter_placeholder`, D5 filter), then the list, then `friends_invite_disclosure` under it.
  The list is the friend cards (`state == friends`) whose uid is **not** a member of the team (the member list the Teams
  view already holds; a person who joined meanwhile is refused with `ALREADY_MEMBER`), in the Friends order of D5, narrowed
  by the filter. Requests are never listed. Empty list with an empty filter: `friends_invite_empty`; a filter with no
  match: `friends_filter_empty`. Row: avatar, name, @handle if any and the button `friends_invite_send`.
- **Friend data.** The picker needs the cards: Web and Desktop run a Friends workspace for as long as the picker is open
  (the `MemberFriend` pattern of D15, so the Teams page runs no Friends listener otherwise); iOS and Android hand the
  app-wide Friends workspace to the Teams tab. No listener per friend.
- **Send.** `friends_invite_send` runs `offerTeam {teamId, friendUid, requestId}` (the id minted at the tap and reused only
  after an unknown outcome, D18). The row is disabled while it runs. On success the row shows `friends_invite_sent`
  (disabled) until the picker closes and a one-shot `friends_invite_sent_to(name)` appears; nothing is stored, because the
  sender cannot read offers. Opening the picker again lists the person again: inviting again refreshes the same offer
  (a new 7-day expiry, no second push while it is live).
- The invitation is always for role `member` (promote in Team details afterwards). The person cannot attach a message.
  Errors: D30, under the row. A friend who stops being a friend leaves the list through the cards listener.

**D27 Team invitations** (vectors `offers.expiry`, `offers.sort`).
- **Data.** The third owner-only listener of D2, `users/{uid}/teamOffers`, as `TeamOffer` values (appendix A): `expiresAt`
  in epoch milliseconds, the document id is the team id. A document without string `teamId`, `teamName`, `fromUid`,
  `fromName` or a readable `expiresAt` is skipped, never failing the list. Order: `offers.sort` (live before expired,
  the newest first, then the team id). A failed offers listener ends that watch, shows nothing and counts nothing; the
  retry of the cards listener (D3) restarts all of them.
- **Section.** `friends_section_offers` at the top of the Friends root (D3), only when there is at least one offer, limited
  to 20 rows with `friends_show_all(n)` (the D4 rule). The `friend/requests` link and the offer push land here (D16).
- **Row.** The team's avatar (the platform's team initials and colour from `teamName`), `teamName` (one line, bold),
  `friends_offer_invited_by(fromName)`, and the expiry line from `offers.expiry`: a live offer shows
  `friends_offer_expires(x)`, where `x` is the platform's relative-time text in the future for the row's (value, unit), such
  as "in 3 days" (Web: `Intl.RelativeTimeFormat(locale, {numeric: 'always'})`; iOS: `RelativeDateTimeFormatter`; Android:
  `android.icu.text.RelativeDateTimeFormatter` or its equivalent); an expired offer shows `friends_offer_expired`. Plain
  text: the names are the sender's and the team's at offer time (snapshots), never Markdown or links.
- **Actions.** Live: `friends_offer_join` (`acceptOffer {offerId: teamId, requestId}`) and `friends_request_decline`
  (`declineOffer {offerId}`, no confirmation). Expired: `friends_offer_dismiss` (`declineOffer`) only, since an expired
  offer cannot be accepted and the server keeps it 7 days more. A row is disabled while one of its actions is in flight (D4).
- **Join.** On success show `friends_offer_joined(teamName)` once, stay in Friends, and refresh the Teams list as a link
  join does (the team appears there); the row goes through the listener. `alreadyMember: true` (a replay) is a success.
  On failure show D30's text under the row: `MEMBER_LIMIT` and `TEAM_LIMIT` keep the offer, so Join stays available;
  `OFFER_UNAVAILABLE` means the server deleted it and the listener removes the row.
- **Clock.** The expiry display uses the device clock; the server decides. An offer that looks live may still answer
  `OFFER_UNAVAILABLE`; one that looks expired is never sent.

**D28 Badge and the offer push** (vector `offers.badge`).
- The badge of D14 also counts every **live** offer (`expiresAt` after now); expired offers add nothing. Because that is a
  function of the clock, recompute at the earliest live expiry (one timer) and whenever the Friends view is shown.
- The offer push has route `friend/requests`, kind `friend_offer`, the sender's name as the title and a generic body
  (FRIENDS.md, "Push and deep links"); the tap runs D16 `Requests`: Friends, scrolled to Team invitations. If the offer was
  answered or expired meanwhile the root simply shows what is there.

**D29 Routes.** F3 and F4 add no route and no parameter: an offer push opens `friend/requests`, and a Bot DM push uses the
friend chat route `friend/chat?uid=<other friend>` (the Bot's answer is titled with the Bot's name). The deep-link grammar,
`fixtures.json` and the TypeScript, Rust, Android and iOS parsers are unchanged. A `conversation` parameter was weighed and
rejected: it loosens a strict grammar in five parsers to save one tap (D22).

**D30 Offer errors and request ids** (vector `offers.errors`). As D18, the message shows under the control that failed.
Do **not** use `friendsErrorKey` for these three actions: it maps `MEMBERSHIP_REQUIRED` and `STALE_MEMBERSHIP` to
`friends_chat_unavailable`, which would be wrong here. The key is the action's own row, else the common row, else
`friends_error_generic`:

| code | action | key |
| --- | --- | --- |
| `FRIEND_REQUIRED` | `offerTeam` | `friends_error_friend_required` |
| `ALREADY_MEMBER` | `offerTeam` | `friends_error_already_member` |
| `OFFER_LIMIT` | `offerTeam` | `friends_error_offer_limit` |
| `MEMBER_LIMIT` | `offerTeam`, `acceptOffer` | `friends_error_member_limit` |
| `ROLE_REQUIRED` | `offerTeam` | `friends_error_offer_role` |
| `MEMBERSHIP_REQUIRED` | `offerTeam` | `friends_error_offer_team_gone` |
| `OFFER_UNAVAILABLE`, `STALE_MEMBERSHIP` | `acceptOffer` | `friends_error_offer_unavailable` |
| `TEAM_LIMIT` | `acceptOffer` | `friends_error_team_limit` |
| `SIGN_IN_REQUIRED`, `INVALID_SESSION` | all | `friends_error_session` |
| `SERVICE_UNAVAILABLE`, transport failure, timeout | all | `friends_error_network` (not definitive: retry with the same id) |
| `INVALID_INPUT`, `INVALID_JSON` | all | `friends_error_invalid_input` |
| `RATE_LIMITED` | `offerTeam` | `friends_error_rate_limited` |
| anything else (`PAIR_TEAM`, `REQUEST_ID_CONFLICT`, a code of another action, unknown) | all | `friends_error_generic` |

`offerTeam` and `acceptOffer` carry a `requestId` (the D18 rules: minted at the tap, reused only after an unknown outcome,
dropped otherwise, never persisted); `declineOffer` has none and is safe to repeat. After `OFFER_UNAVAILABLE` the offers
listener drops the section row; after `FRIEND_REQUIRED` the cards listener drops the person from the picker; after
`ALREADY_MEMBER` drop the picker row yourself and refresh the member list.

## 2. Appendix A: wire types

```ts
type Relation = 'none' | 'friends' | 'incoming' | 'outgoing';
interface FriendCard { uid: string /* doc id */; state: 'friends' | 'incoming' | 'outgoing'; displayName: string;
  handle?: string; note?: string; since: Timestamp;
  teamId?: string; generation?: number; /* both only when 'friends' */
  /* The chat summary, only when 'friends' (D13). Each part is absent until the first message or read mark. */
  lastSeq?: number; lastReadSeq?: number; lastChangeSeq?: number;
  lastMessage?: { id: string; seq: number; at: Timestamp; author: MessageAuthor; preview: string; recalled?: true } | null }
interface FriendProfile { uid: string; displayName: string; handle?: string; findByHandle: boolean; findByEmail: boolean;
  friendCode: { code: string; link: string } | null }
interface LookupResult { uid: string; displayName: string; handle?: string; relation: Relation }
interface BlockEntry { uid: string /* doc id */; at: Timestamp; displayName: string; handle?: string }
/** `users/{uid}/teamOffers/{teamId}` (F4, D27): a friend's offer of an ordinary team to the owner. One per (team, friend). */
interface TeamOffer { teamId: string /* doc id */; teamName: string; fromUid: string; fromName: string;
  role: 'member'; expiresAt: Timestamp }
```

`lastMessage` is the Teams conversation pointer (`TeamConversation['lastMessage']`, `MessageAuthor` as in
`packages/teams/messages.ts`); `packages/teams` maps it with `firestoreLastMessage`, which turns the Timestamp into an
ISO string (a friend chat's author is always `kind: 'user'`). Card, block and offer fields are defined in FRIENDS.md. An
offer's `teamName` and `fromName` are snapshots; `role` is always `member`; `expiresAt` is 7 days after the offer was made or
last refreshed. `packages/teams` maps an offer with `firestoreTeamOffer` (Timestamp to epoch milliseconds; a document without
the four strings or a readable `expiresAt` is skipped).

| Action (`POST /v1/friends`) | Body fields | Result |
| --- | --- | --- |
| `profile` | `displayName?, handle?, findByHandle?, findByEmail?` | `FriendProfile` |
| `lookup` | one of `handle`, `email`, `code`; or `teamId` + `uid` | `LookupResult` |
| `request` | `requestId, note?, via`; `handle` or `email` or `teamId` + `uid` | `{uid, state:'outgoing'}` or `{uid, state:'friends', teamId}` |
| `accept` | `otherUid` | `{state:'friends', teamId}` |
| `decline`, `cancel`, `remove`, `block`, `unblock` | `otherUid` | `{ok:true}` |
| `redeem` | `code, requestId` | `{state:'friends', uid, teamId}` |
| `rotateCode` / `revokeCode` | none | `{code, link}` / `{ok:true}` |
| `offerTeam` | `teamId, friendUid, requestId` | `{offerId, expiresAt}` (`expiresAt` an ISO time; `offerId` is the team id, opaque to clients) |
| `acceptOffer` | `offerId, requestId` | `{teamId, generation, alreadyMember}` |
| `declineOffer` | `offerId` | `{ok:true}` |

The chat uses the Teams `POST /v1/teams/messages` and Firestore listeners with `{teamId, generation}`. So do a friendship's
Bots (F3): `listSharedBots`, `createConversation` (only `kind: 'bot_dm'`), `submit`, `pauseSharedBot`, `publishSharedBot` and
the host commands, all with `{teamId: card.teamId, generation: card.generation}`. A pair team rejects every `POST /v1/teams`
action, and a `createConversation` of any other kind, with `PAIR_TEAM`.

```ts
// Additions to FriendsCommand and FriendsResults (packages/teams/friends.ts)
| { action: 'offerTeam'; teamId: string; friendUid: string; requestId: string }
| { action: 'acceptOffer'; offerId: string; requestId: string }
| { action: 'declineOffer'; offerId: string }
// offerTeam: { offerId: string; expiresAt: string }   acceptOffer: { teamId: string; generation: number; alreadyMember: boolean }
// declineOffer: { ok: true }
```

## 3. Platform implementers

### 3.1 Web and Desktop (shared React in `packages/teams`)

New in `packages/teams`:
- `friends.ts`: types, `friendsTransport`, `FriendsApi` port (`profile`, `execute`, `watchCards`, `watchBlocks`,
  `readProfile`) and the pure rules, one function per vector section: `foldName`, `friendCollator(locale)` (D5),
  `compareFriends`, `compareRequests`, `compareCards`, `matchesFilter`, `unreadOf(card)`, `badgeCount(cards)`,
  `badgeText`, `friendCodeOf`, `classifyQuery`, `lookupBody`, `confirmBody`, `friendsErrorKey`.
- `friends-workspace.ts`: `FriendsWorkspace` (no React, `subscribe` / `getSnapshot` like `TeamsWorkspace`) with the
  snapshot and actions of D3 to D15. It runs the listeners of D2 and nothing per friend. The cards mapper turns
  `since` and `lastMessage` into plain values with the Teams helpers (`firestoreLastMessage`; a pointer that fails to
  parse counts as absent). The D13 optimistic read comes from the open chat's pair workspace.
- UI: `FriendsSidebar`, `AddFriendView`, `FriendProfileSettings`, `BlockedList`, `FriendChatView` (wraps
  `TeamConversationView`), `useFriendsWorkspace`; texts inline through the components' local `const t = (en, cn) => zh ? cn : en`.
- Changes: `TeamsWorkspaceOptions.pair` (D9: skips `list` and `detail`, builds summary and directory);
  `TeamConversationView` gains a `friend` variant (no new-conversation, no details); `TeamDetailsDrawer` members get the
  D15 action; `conversationPreview` exports its message part as `pointerPreview(lastMessage, uid, zh)` (behaviour
  unchanged) so a Friends row prints the same text as a Teams row.
- Test `packages/teams/friends.test.ts` runs every vector section through `friends.ts`.

Web (`apps/web/src`): `friends/api.ts` (reuse `teams/firestore.ts`), `friends/add-intent.ts`, `pages/FriendsPage.tsx`;
`app/router.ts` (`/friends`, `/friends/add` in `appRoutes`, `securityZone` returns `teams` for them), `App.tsx`,
`OpenLinkPage.tsx` (D17); `firebase.json` header blocks for `/friends` and `/friends/**` (no-store, no-referrer, the
`/teams` CSP). Web has no push.

Desktop (`apps/electron/src`): `shared/settings.ts` (`SidebarMode`), `renderer/bridge/sidebarMode.ts` (`AppPage` and
the three mode functions), `renderer/components/BetaDesktop.tsx` (rail button after Teams with the D14 badge, page
sidebar), `renderer/App.tsx` (friend links select the mode, like team links), main process: a `lingxi:friends` IPC
beside `lingxi:teams` (HTTP, cards, blocks and offers listeners in the main-process Firestore session) and its preload
bridge. Desktop has no FCM: the badge is live only while the window is open. Desktop-only string
`desktop_friends_rail_unread` goes in `navigationCatalog.ts` (five locales, `%1$d`); `friends_title`,
`friends_signin` and `friends_loading` come from the canonical JSON through `tr()`.

**F3 and F4 additions (D21 to D30).**
- `friends.ts`: `TeamOffer`; `offerTeam`, `acceptOffer` and `declineOffer` in `FriendsCommand` and `FriendsResults`;
  `FriendsApi.watchOffers` (and the `DesktopFriendsApi` twin); one pure function per new vector section: `offerExpiry(expiresAtMs,
  nowMs)` (which also gives the state), `compareOffers(nowMs)`, `badgeCount(cards, offers, nowMs)`, `invitePicker(cards, memberUids,
  query, collator)`, `canInviteFriends(role)` and `offerErrorKey(action, code)` (not `friendsErrorKey`, D30). `friends.test.ts` runs the
  `offers.*` sections. `firestore.ts`: `firestoreTeamOffer(id, data)`.
- `friends-workspace.ts`: the third listener, `FriendsSnapshot.offers`, `badge` with live offers (a timer at the earliest live
  expiry, D28) and the actions `offerTeam`, `acceptOffer` and `declineOffer`, which resolve an error code like the others;
  `retry` restarts all three listeners. `workspace.ts`: build `BotCards` for a `pair` workspace too (D21); `refresh` and `detail`
  stay skipped.
- UI: `FriendChatView` gets the Bots action and a `FriendBotsPanel` (rows are the Teams Bot cards; Stop sharing; on Desktop the
  share button that opens the `BotDirectory` wizard in a `pair` mode: audience fixed to all, review naming the friend, D23).
  `TeamConversationView` passes `friend` only for `friend_chat`, keeps Bot runs on for a `bot_dm` and shows a back button there
  (D22). `FriendsSidebar` gets the Team invitations section (D27). `friend-dialogs.tsx` gets `InviteFriendsDialog` and
  `TeamDetailsDrawer` an optional `onInviteFriends?()` prop, shown in the Invites tab, which only managers have (D26).
- Web (`apps/web/src`): `friends/api.ts` `watchOffers`; `friends/InviteFriends.tsx`, shaped like `MemberFriend.tsx` (a Friends
  workspace that lives as long as the dialog), wired from `teams/TeamsWorkspaceView.tsx`. Extend
  `apps/web/test/friends-firestore.emulator.test.ts` with an offer read through the mapper.
- Desktop (`apps/electron/src`): `main/teams/friends.ts`: `ACTIONS` gains the three actions and `FriendsCollection` gains
  `'teamOffers'` (with the collection check of `friendsIpc.ts` and the preload bridge); `main/teams/connector.ts`: D24;
  `renderer/components/TeamsMain.tsx`: the invite dialog, like `MemberFriend`. The rail badge reads `FriendsSnapshot.badge`, so it
  counts offers once the workspace does. Tests: `friends-service.test.ts` (the actions), `friends-ipc.test.ts` (the collection),
  `teams-host.test.ts` (D24 item 5) and `friends-firestore.emulator.test.ts` (an offer read).

### 3.2 iOS (`apps/ios/native`)

Done in step 0: `Sources/App/CanonicalDeepLink.swift` (routes, `uid`), `Sources/Account/PushMessagePolicy.swift`
(`link(forRoute:)`), `Sources/Friends/FriendLink.swift`, `Tests/FriendLinkTests.swift`, the vectors in `project.yml`.

To do: `Sources/Friends/`: `FriendsLogic.swift` (Foundation only, the pure rules with the D5 collator; add it to the
macOS harness), `FriendsModels.swift` (cards decode the D13 summary; `lastMessage.at` as whole epoch milliseconds),
`FriendsWorkspace.swift` (`@MainActor @Observable`, API root `https://lingxios.net/v1/friends`, the listeners of D2
and no per-friend listener), `FriendsTab.swift`, `AddFriendView.swift`, `FriendProfileView.swift`,
`BlockedPeopleView.swift`, `FriendChatView.swift` (injects a pair-pinned `TeamsWorkspace` into
`TeamConversationView`; give `TeamsWorkspace` an initializer or option for it). Wiring: `Tabs/AppTab.swift`
(`.friends`), `Tabs/TabShell.swift`, `Tabs/TabLogic.swift` (`LinkTarget.friends`; `tabForLink("friend/...")` returns
it; test `links.tabForLink`), `App/RootView.swift` (`applyCanonicalLink` case `"friend"` sets a pending `FriendLink`
the tab consumes, and add `"friend"` to the independence list `["remote", "team"]` near line 3624), the member row in
`Teams/TeamDetailsPage.swift`. Until this is wired a friend link reaches the existing "unavailable" path.

**F3 and F4 additions (D21 to D30).**
- `Sources/Friends/FriendsLogic.swift`: the pure rules of the `offers.*` sections (`offerExpiry`, `compareOffers`, the badge with
  live offers, `invitePicker`, `canInviteFriends`, `offerErrorKey`); `Tests/FriendsLogicTests.swift` runs them through
  `FriendsVectors.rows("offers", ...)` and `FriendsVectors.section("offers", ...)` (the loader and the `card` adapter already
  fit the row shapes). Add `FriendsLogic.swift` to the macOS harness as before.
- `FriendsModels.swift`: `TeamOffer` read from `users/{uid}/teamOffers` (Timestamp to epoch milliseconds; skip a document
  without the four strings or a readable `expiresAt`). `FriendsWorkspace.swift`: the third listener beside cards and blocks,
  `offers`, the badge with live offers (a timer at the earliest live expiry, D28) and `offerTeam`, `acceptOffer`, `declineOffer`
  through the existing `/v1/friends` call.
- `FriendsListView.swift`: the Team invitations section (D27); `FriendsTab.swift`: the one-shot `friends_offer_joined(teamName)`
  and a refresh of the Teams workspace after a join.
- `FriendChatView.swift`: the Bots toolbar button and `FriendBotsView.swift` (new): rows from the pair workspace's
  `messaging.bots`, Message through `TeamsWorkspace.openBotDM`, Stop sharing through `TeamMessagingModel.pause`, then
  `friends_bots_note` and `friends_bots_share_hint`. The Bot DM is `TeamConversationView` on the same pair workspace, with a back
  action that selects `friend_chat` again. A Bot command that fails with a `chatEnded` code ends the chat (D25).
- `Teams/TeamDetailsPage.swift`: in `inviteSections` (shown when `TeamRoles.canInvite(role:)`) the button
  `friends_invite_action`, presenting `Sources/Friends/InviteFriendsSheet.swift` (new) with the app's friend cards and the
  team's members (D26).
- iOS cannot publish a Bot: no share action, only `friends_bots_share_hint`.

### 3.3 Android (`apps/android/native/app/src/main/java/com/lingxi/code`)

Done in step 0: `navigation/DeepLink.kt` (routes, `uid`), `push/PushRoute.kt`, `friends/FriendLink.kt`, and the test
`app/src/test/java/com/lingxi/code/friends/FriendLinkTest.kt`.

To do: `friends/`: `FriendsLogic.kt` (pure rules with the D5 collator; `lastMessage.at` as whole epoch milliseconds),
`FriendsWorkspace.kt` (StateFlow, instantiable like `TeamsWorkspace`; the listeners of D2 and no per-friend
listener), `FriendsTab.kt`, `AddFriendScreen.kt`, `FriendProfileScreen.kt`, `BlockedPeopleScreen.kt`,
`FriendChatScreen.kt` (`TeamConversationScreen` over a pair-pinned `TeamsWorkspace`). Wiring: `tabs/AppTab.kt`
(`Friends`), `tabs/MainTabShell.kt`, `tabs/TabLogic.kt` (`LinkTarget.Friends("friends")`, `tabForLink` returns it),
`navigation/DeepLinkInbox.kt` (`independent` also for `friend/`), `MainActivity.kt` near line 327 (friend routes call
a `tabState.requestFriends(FriendLink)`), the member row in `teams/TeamDetailsScreens.kt`. The manifest needs no
change (`/open/` is already an app-link prefix). Until this is wired a friend link reaches the existing unhandled path.

**F3 and F4 additions (D21 to D30).**
- `friends/FriendsLogic.kt`: `TeamOffer` and the pure rules of the `offers.*` sections (`offerExpiry`, `compareOffers`, the badge
  with live offers beside `badgeCount`, `invitePicker`, `canInviteFriends`, `offerErrorKey`); `FriendsLogicTest` runs them through
  `FriendsVectors.rows("offers", ...)` and `FriendsVectors.obj("offers", ...)` (the loader and the `card` adapter already fit the row
  shapes). A model test reads an offer document (skip an invalid one).
- `friends/FriendsWorkspace.kt`: the third listener on `users/{uid}/teamOffers`, an `offers` state, the badge with live offers (a
  timer at the earliest live expiry, D28; `FriendsBadge.kt`) and `offerTeam`, `acceptOffer`, `declineOffer`. `FriendsListScreen.kt`:
  the Team invitations section (D27); `FriendsTab.kt`: the one-shot `friends_offer_joined(teamName)` and a refresh of the Teams
  workspace after a join.
- `friends/FriendChatScreen.kt`: the Bots action and `friends/FriendBotsScreen.kt` (new): rows from the pinned workspace's
  `messaging.bots`, Message through the Bot DM start of `TeamsTab`, Stop sharing through `TeamMessagingModel.pause`, then
  `friends_bots_note` and `friends_bots_share_hint`. The Bot DM is `TeamConversationScreen` on the same pinned workspace, with a back
  action that selects `friend_chat` again. A Bot command that fails with a `chatEnded` code ends the chat (D25).
- `teams/TeamDetailsScreens.kt`: in `invitesSection` (the Invites segment exists for `privileged` roles) the button
  `friends_invite_action`, opening `friends/InviteFriendsSheet.kt` (new) with the app's friend cards and the team's members (D26).
- Android cannot publish a Bot: no share action, only `friends_bots_share_hint`.

## 4. Strings

Added to all five `resources/translations/*.json` (real translations; generated outputs regenerated with
`generate.py`, never hand-edited): **81 keys** = 67 plain + 7 placeholder pairs.
- Placeholder pairs (iOS key / Android `_fmt` sibling): `friends_show_all %lld`, `friends_now_friends %@`,
  `friends_request_sent_to %@`, `friends_confirm_title %@`, `friends_remove_title %@`, `friends_block_title %@`,
  `friends_since %@`.
- Reused existing keys: `common_cancel`, `common_retry`, `common_save`, `teams_preview_none`, `teams_empty_thread`,
  `teams_member` (the name of a block entry that has none, D12), `tabs_badge_unread %lld`.
- zh-Hant equals zh-Hans for three keys (`friends_title`, `friends_request_accept`, `friends_relation_friends`); they
  are listed in `INTENTIONAL_IDENTICAL_TO_BASE` of `scripts/checks/check_i18n_pairing.py`.

| Client | Mechanism |
| --- | --- |
| iOS | `String(localized:)` with the keys above; `Localizable.xcstrings` is generated. |
| Android | `R.string.friends_*`; placeholder strings use the `_fmt` sibling (`%1$s`, `%1$d`). |
| Desktop | `tr('friends_...')` resolves canonical JSON keys (merged with the catalogs); Desktop-only copy with `%1$d` goes in `renderer/i18n/navigationCatalog.ts` (five locales). |
| `packages/teams` (Web and shared Desktop components) | Inline: each component declares `const t = (en, cn) => zh ? cn : en` and takes a `zh` prop; it cannot call `tr()`. Use the en and zh-Hans values of the JSON verbatim so all clients read the same. |

Key groups: root and requests (`friends_title` ... `friends_request_sent`), Add friend (`friends_link_*`,
`friends_search_*`, `friends_note_placeholder`, `friends_relation_friends`, `friends_open_chat`,
`friends_confirm_link_body`, `friends_link_invalid`), chat and menu (`friends_chat_unavailable`, `friends_menu_*`,
`friends_remove_body`, `friends_block_body`), profile (`friends_profile_*`, `friends_name_prompt*`), blocked
(`friends_blocked_*`, `friends_unblock*`), errors (`friends_error_*`).

**F3 and F4 add 34 entries** after `friends_since_fmt` in each JSON file: 22 plain keys and 6 placeholder pairs (iOS key /
Android `_fmt` sibling), so the Friends keys are 115 entries in all.
- Team invitations (D27): `friends_section_offers`, `friends_offer_expired`, `friends_offer_join`, `friends_offer_dismiss`;
  pairs `friends_offer_invited_by %@` (the sender's name), `friends_offer_expires %@` (the relative expiry text of D27, for
  example "in 3 days"), `friends_offer_joined %@` (the team's name).
- Invite friends (D26): `friends_invite_action`, `friends_invite_send`, `friends_invite_sent`, `friends_invite_empty`,
  `friends_invite_disclosure`; pairs `friends_invite_title %@` (the team's name), `friends_invite_sent_to %@` (the friend's name).
- Offer errors (D30): `friends_error_friend_required`, `friends_error_already_member`, `friends_error_offer_limit`,
  `friends_error_member_limit`, `friends_error_team_limit`, `friends_error_offer_role`, `friends_error_offer_team_gone`,
  `friends_error_offer_unavailable`. They carry the limits 7 days, 20 invitations, 200 members and 20 teams, which are in the
  vectors' `limits`; change both together.
- Bots in a friendship (D21, D23): `friends_bots_title`, `friends_bots_none`, `friends_bots_note`, `friends_bots_share_hint`,
  `friends_share_bot_stop`; pair `friends_share_bot_with %@` (the friend's name).
- Reused without change: `friends_filter_placeholder`, `friends_filter_empty`, `friends_show_all %lld`, `friends_request_decline`,
  the general `friends_error_*` (session, network, invalid input, rate limit, generic), `common_cancel`, and from Teams
  `teams_bot_unbind_detail`, `teams_bots_loading`, `teams_bots_unverified`, `teams_bot_owner %@`, `teams_bot_message`,
  `teams_bot_not_permitted`.
- zh-Hant equals zh-Hans for five more keys (`friends_offer_join`, `friends_offer_dismiss`, `friends_bots_title`,
  `friends_bots_none`, `friends_share_bot_stop`) and for the pair `friends_offer_expires`; all seven entries are in
  `INTENTIONAL_IDENTICAL_TO_BASE` too.

## 5. Tests and vectors

`friends-vectors.json` sections and the decision each one tests (the file's `_meta.encoding` explains the row
shapes):

| Section | Decision |
| --- | --- |
| `limits`, `requestId`, `handle`, `lengthRule` | D11, D18 |
| `sort.friends`, `sort.cards`, `sort.requests` | D5 |
| `filter` | D5 |
| `unread` | D13 |
| `badge` | D14 |
| `relationAction` | D6 |
| `query` | D7 |
| `errors` | D18 |
| `links.typed`, `links.push` | D16 |
| `links.tabForLink` | D16 (wiring) |
| `links.webViewer` | D17 |
| `limits` (offer fields), `offers.expiry`, `offers.sort` | D27 |
| `offers.picker`, `offers.entry` | D26 |
| `offers.badge` | D28 (and D14) |
| `offers.errors` | D30 |

The sort rows hold only what every platform agrees on: activity order (`lastAtMs`, epoch milliseconds, so a 64-bit
integer), requests before friends, name order for ASCII-letter names in mixed case (the collators of D5 give one order
for them in every locale of the app; checked with Node ICU, the JDK collator and Foundation for zh-Hans, zh-Hant, en, ja
and ko), and ties by `uid`. Chinese, accented, numeric and punctuated names are left to each platform's collation and
have no rows. The case-only pairs (`Alice`/`alice`, `Kim`/`kim`, `Sam`/`sam`) fail a default-strength collator and
`localizedStandardCompare` on purpose; the pair `n9`/`n10` fails a numeric `uid` comparison.

The `offers` rows (F4) all read one clock, `offers.nowMs`, so they never age; times are epoch milliseconds. `offers.expiry`
(18 rows) pins the display rule of D27: strictly after now is live, `ceil` to hours up to and including 24 hours and to days
above (2 to 7), 1 ms and 1 minute left read 1 hour, and an offer expired for longer than the 7-day grace is still `expired`
until the server's prune deletes it. The unit and value go to each platform's relative-time formatter; no text is in the
vectors. `offers.sort` (8 rows, shuffled) has live and expired offers and ties by team id (`team_10` before `team_9`).
`offers.badge` (10 rows) is `badge` plus offers: only live offers count, one that expires exactly now does not, and the 99/100
edge is crossed by an offer. `offers.picker` (13 cases over one card set) uses the ASCII-name cards of `sort.friends` (also
checked with Node ICU for all five locales, including the `Alice`/`alice` tie): members, incoming and outgoing requests are
never listed, the filter is the `filter` rule, and an empty result is the `friends_invite_empty` case. `offers.entry` (5 rows)
is the managers-only rule. `offers.errors` (33 rows) is per action, because `MEMBERSHIP_REQUIRED` and `STALE_MEMBERSHIP` mean
"the chat ended" in `errors` and something else here. The rows were produced by a Python reference of D26 to D30 and checked
by an independent Node implementation written from the same text (and every key against `en.json`); `packages/teams/friends.test.ts`
keeps reading the older sections and ignores `offers` until the Web client adds its test.

Each client must add a test that loads the file from `packages/teams/fixtures/` (Android and iOS walk up from the test
file as `TeamBotsTest` does) and checks every row: `packages/teams/friends.test.ts`, iOS `FriendsLogicTests`,
Android `FriendsLogicTest`; since F4 that includes every `offers.*` section (iOS and Android read nested sections with the
loaders' `rows(...)` and `section(...)`). `links.typed` and `links.push` are already checked by iOS `FriendLinkTests`, Android
`FriendLinkTest` and the TS test in `packages/deep-links/index.test.ts`.

Commands (repository root unless noted):

```bash
node --import ./packages/bridge-client/node_modules/tsx/dist/loader.mjs --test packages/deep-links/index.test.ts
node --import ./packages/bridge-client/node_modules/tsx/dist/loader.mjs --test packages/teams/friends.test.ts
npm --prefix apps/firebase test                                                    # backend unit tests (F3, F4 included)
JAVA_HOME=/opt/homebrew/opt/openjdk@21 npm --prefix apps/firebase run test:teams   # emulator suite; needs 2 GiB free
python3 resources/translations/generate.py --check && python3 scripts/checks/check_i18n_pairing.py
cd apps/android/native && ./gradlew --offline :app:testPlayDebugUnitTest --tests 'com.lingxi.code.friends.*' --tests 'com.lingxi.code.navigation.*'
#   (a worktree without app/build/mobileLinuxNative needs  -x verifyPlayMobileLinuxNative)
# iOS: ios-typecheck-diff.sh <worktree>/apps/ios/native FriendLinkTests.swift   (type-check, no new diagnostics)
#      macOS runner with CanonicalDeepLink, FriendLink, PushMessagePolicy joined and a LingxiAppAction stub
```

## 6. Open questions

1. **Backend dependency.** The card summary (D13) and the names on block entries (D12) are F0 follow-ups, documented in
   FRIENDS.md and present as source only (`apps/firebase/src/friend-summary.mjs`, `friends.mjs`); they ship under the
   same approval gates as the rest of F0. Until the two summary triggers run, cards carry no summary, which the rules
   already read as "no message, nothing read": every row shows `teams_preview_none`, no unread, and the order is by name.
2. **"5th tab".** Read as five tabs with Friends fourth and Settings last (Chats, Bots, Teams, Friends, Settings).
3. **Badge** counts messages (as the Teams badge does), not conversations; switching is a one-line change in
   `badgeCount`.
4. **Name order is locale collation** (D5). zh-Hans sorts Chinese names by pinyin. Under zh-Hant the ICU collators
   (Intl and Foundation, both run here) give stroke order, the CLDR default, not pinyin. Han, kana and Hangul names are
   ordered by each platform's own tables, which differ from one platform to the next; no vector asserts them. Android's
   `java.text.Collator` was not run here.
5. **Ordering is last activity first, then name.** An earlier draft ordered friends A-Z. The review asked for
   last-activity order from the cards and locale-aware names, and D5 does both: friends with a message by newest
   `lastMessage.at`, then friends without, each tie by collated name, then `uid`. To get pure A-Z instead, drop rules 1
   and 2 of the friends order in D5 and the `lastAtMs` values of `sort.friends` and `sort.cards`; nothing else changes.
6. **Snapshot names.** Cards hold the name at friendship time and there is no rename fan-out; D10 refreshes a friend's
   name only when a chat or profile is opened.
7. **Web and Desktop get no push**, so requests and messages show only while the app is open.
8. **Existing mismatch, untouched:** Android `PushRoute` accepts any `remote/` prefix while iOS validates the Remote
   Control route strictly.
9. **QR** is omitted because no client has a generator. Android's `libs.versions.toml` declares `com.google.zxing:core`
   but nothing uses it, so an Android QR on the link card would be a small follow-up.
10. **i18n gate** was already failing before this work (unpaired keys 94 vs baseline 93, zh-Hant identical 252 vs 235,
   en identical 20 vs 19); Friends, F3 and F4 add nothing to those counts. The unpaired key is `teams_bots_count %lld`, from
   Teams.
11. **Bot DM unread.** A Bot DM moves no friend card, preview, unread or badge (the backend's summary triggers watch only
   `friend_chat`), and v1 shows no unread marker on the Bots entry either. The pair workspace already knows every
   conversation's `lastSeq` and read mark, so a dot on the Bots action needs no backend change, only a label for it.
12. **The maintainer and a friend's Bot chats.** The backend lets the maintainer read the friend's chats with their Bot, and
   the UI says so (`friends_bots_note`), but v1 offers no way to open them from the Friends screens: a Bot row lists only
   the viewer's own chat. A read-only list under the maintainer's Bot row is a UI addition with one new string.
13. **Sharing needs the Desktop to be on.** A Bot runs on its maintainer's Desktop, so the friend sees it as unavailable
   (Teams' presence labels) when that Desktop is off or offline. There is no message queue beyond the Teams one.
14. **The sender cannot see an offer's fate.** Rules deny the sender any read of an offer, so the picker's "Invited" mark lasts
   one picker session and there is no list of pending invitations for managers; the new member simply appears in the team.
   Inviting a friend again refreshes the offer silently while it is live.
15. **Expired offers linger up to 7 days** (then the daily prune deletes them), so a person sees "Expired" with Dismiss for a
   week; they never count in the badge.
16. **What an offer reveals.** Before joining, the friend sees only the team's name and the sender's name, and
   `friends_invite_disclosure` says so to the sender; joining is the same as joining by link, so the person becomes a member
   whom the whole team sees.
