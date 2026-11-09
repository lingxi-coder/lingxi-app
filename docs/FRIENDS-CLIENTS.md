# Friends clients (F1 + F2)

Status: the client contract on top of the F0 backend ([FRIENDS.md](FRIENDS.md)). Nothing here is deployed. One
decision-numbered spec (D1...D20) for four clients: **Web and Desktop** (one React implementation in
`packages/teams`), **iOS** (SwiftUI) and **Android** (Compose). Section 3 lists what each platform adds. If a client
and this document disagree, fix the client; if the document is wrong, change it and the vectors together.

It relies on two F0 follow-ups that [FRIENDS.md](FRIENDS.md) documents (source only, like the rest of F0): the chat
summary on friend cards (`lastSeq`, `lastMessage`, `lastChangeSeq`, `lastReadSeq`; D13) and the person's name on block
entries (`displayName`, `handle?`; D12). Section 6 lists the consequences.

In scope: friends list, requests, Add friend (link, @username, email), profile and discoverability settings, blocked
list, 1:1 chat. Out of scope: sharing Bots with friends (F3), QR codes (no client has a QR generator; none is
added), account-deletion cleanup, rename fan-out.

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
- Live data is two owner-only Firestore listeners: `users/{uid}/friends` (all three states, `FriendCard`) and
  `users/{uid}/blocks` (`BlockEntry`). The cards listener alone feeds the whole list: names, last message, last-activity
  order and unread all come from the cards (D13); no listener runs per friend. One-shot `get` of `profiles/{uid}` for a
  friend (D10).
- Everything else is HTTP with the Firebase ID token: `GET /v1/friends/profile` and `POST /v1/friends` through a
  `friendsTransport` shaped like `teamsTransport` (same error shape, same 20 s timeout).
- Chat data is the Teams message layer, unchanged (D9).
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
2. Requests, only when there is at least one (D4).
3. Filter field `friends_filter_placeholder` (D5). Always visible on Desktop and Web; on mobile it is the list's search
   affordance. No match: `friends_filter_empty`.
4. Friends, one row per `friends` card in D5 order. Row: avatar (the platform's existing `avatarColor` / `initials`),
   name (one line, bold while unread), one preview line, time of the newest message (`lastMessage.at`; none when there
   is no message), unread badge (`badgeText` of the D13 unread). Tap opens the chat (D9). Preview is the Teams
   `conversationPreview` rule applied to the card's `lastMessage` ("You: " prefix, recalled and non-text labels
   included); a card without `lastMessage` shows `teams_preview_none`. Every friend gets a preview and a badge: they all
   come from the card (D13).

States: loading `friends_loading` until the first cards snapshot; listener error `friends_load_error` with
`common_retry`; no friends and no requests: `friends_empty_title`, `friends_empty_body` and a primary `friends_add`.
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
  no team-detail call. Never select the pair team in the Teams tab's workspace.
- Hidden compared with a Team: conversation list or sidebar, new conversation (group, channel, Bot DM), team
  details drawer or page, members, invites, Bot picker and Bot cards (F3), team badge. Kept: send, edit, recall,
  reply, read marks (D13), offline queue, new-messages divider, jump to latest.
- Header: avatar and the friend's name (D10 override if any, else the card), `@handle` as subtitle when known, a back
  affordance on mobile, and the menu of D10.
- Ended friendship (the card disappears, or a send fails with `MEMBERSHIP_REQUIRED`, `STALE_MEMBERSHIP` or
  `CONVERSATION_REQUIRED`): replace the body and composer with `friends_chat_unavailable` and a Back action. The Teams
  layer already drops the stale generation's outbox.

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

**D14 Badge** (vectors `badge`). `incoming request count + sum of the D13 unread over the friends cards`, rendered by
the client's existing tab-badge rule (nothing at 0, the number up to 99, `99+` from 100). Outgoing requests add
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
- `Add` runs D8. `Requests` selects Friends and scrolls to Requests. `Chat(uid)` selects Friends and opens that
  friend's chat once the first cards snapshot has arrived; a uid that is not a current friend opens the root with
  `friends_chat_unavailable`.
- Friends links are independent of the workspace and session (like `team/*`): never queued behind a session,
  `needsContextRebind` false, tab target `friends`. Signed out: sign in first and keep the intent.
- A push `route` is accepted only as `team/inbox`, `friend/requests`, `friend/chat?uid=<[A-Za-z0-9_-]{1,128}>` (no
  percent-encoding) or the existing `remote/...`; anything else is dropped, and a push never carries `friend/add`.
  Notification taps keep Teams' recipient and expiry checks.

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

**D19 Teardown.** On sign-out or uid change stop the cards and blocks listeners and the open chat's workspace, and
clear cards, profile, name overrides, lookup state and open dialogs. The pending friend-code intent of D8 survives (it
belongs to the device). Nothing about friends is written to disk except that Web intent.

**D20 Strings.** `friends_*` keys in `resources/translations/*.json` (section 4). Placeholders follow the existing
conventions (iOS token in the key, Android `_fmt` sibling with positional tokens).

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
```

`lastMessage` is the Teams conversation pointer (`TeamConversation['lastMessage']`, `MessageAuthor` as in
`packages/teams/messages.ts`); `packages/teams` maps it with `firestoreLastMessage`, which turns the Timestamp into an
ISO string (a friend chat's author is always `kind: 'user'`). Card and block fields are defined in FRIENDS.md.

| Action (`POST /v1/friends`) | Body fields | Result |
| --- | --- | --- |
| `profile` | `displayName?, handle?, findByHandle?, findByEmail?` | `FriendProfile` |
| `lookup` | one of `handle`, `email`, `code`; or `teamId` + `uid` | `LookupResult` |
| `request` | `requestId, note?, via`; `handle` or `email` or `teamId` + `uid` | `{uid, state:'outgoing'}` or `{uid, state:'friends', teamId}` |
| `accept` | `otherUid` | `{state:'friends', teamId}` |
| `decline`, `cancel`, `remove`, `block`, `unblock` | `otherUid` | `{ok:true}` |
| `redeem` | `code, requestId` | `{state:'friends', uid, teamId}` |
| `rotateCode` / `revokeCode` | none | `{code, link}` / `{ok:true}` |

The chat uses the Teams `POST /v1/teams/messages` and Firestore listeners with `{teamId, generation}`; a pair team
rejects every other Teams action with `PAIR_TEAM`.

## 3. Platform implementers

### 3.1 Web and Desktop (shared React in `packages/teams`)

New in `packages/teams`:
- `friends.ts`: types, `friendsTransport`, `FriendsApi` port (`profile`, `execute`, `watchCards`, `watchBlocks`,
  `readProfile`) and the pure rules, one function per vector section: `foldName`, `friendCollator(locale)` (D5),
  `compareFriends`, `compareRequests`, `compareCards`, `matchesFilter`, `unreadOf(card)`, `badgeCount(cards)`,
  `badgeText`, `friendCodeOf`, `classifyQuery`, `lookupBody`, `confirmBody`, `friendsErrorKey`.
- `friends-workspace.ts`: `FriendsWorkspace` (no React, `subscribe` / `getSnapshot` like `TeamsWorkspace`) with the
  snapshot and actions of D3 to D15. It runs the two listeners of D2 and nothing per friend. The cards mapper turns
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
beside `lingxi:teams` (HTTP, cards and blocks listeners in the main-process Firestore session) and its preload
bridge. Desktop has no FCM: the badge is live only while the window is open. Desktop-only string
`desktop_friends_rail_unread` goes in `navigationCatalog.ts` (five locales, `%1$d`); `friends_title`,
`friends_signin` and `friends_loading` come from the canonical JSON through `tr()`.

### 3.2 iOS (`apps/ios/native`)

Done in step 0: `Sources/App/CanonicalDeepLink.swift` (routes, `uid`), `Sources/Account/PushMessagePolicy.swift`
(`link(forRoute:)`), `Sources/Friends/FriendLink.swift`, `Tests/FriendLinkTests.swift`, the vectors in `project.yml`.

To do: `Sources/Friends/`: `FriendsLogic.swift` (Foundation only, the pure rules with the D5 collator; add it to the
macOS harness), `FriendsModels.swift` (cards decode the D13 summary; `lastMessage.at` as whole epoch milliseconds),
`FriendsWorkspace.swift` (`@MainActor @Observable`, API root `https://lingxios.net/v1/friends`, the two listeners of D2
and no per-friend listener), `FriendsTab.swift`, `AddFriendView.swift`, `FriendProfileView.swift`,
`BlockedPeopleView.swift`, `FriendChatView.swift` (injects a pair-pinned `TeamsWorkspace` into
`TeamConversationView`; give `TeamsWorkspace` an initializer or option for it). Wiring: `Tabs/AppTab.swift`
(`.friends`), `Tabs/TabShell.swift`, `Tabs/TabLogic.swift` (`LinkTarget.friends`; `tabForLink("friend/...")` returns
it; test `links.tabForLink`), `App/RootView.swift` (`applyCanonicalLink` case `"friend"` sets a pending `FriendLink`
the tab consumes, and add `"friend"` to the independence list `["remote", "team"]` near line 3624), the member row in
`Teams/TeamDetailsPage.swift`. Until this is wired a friend link reaches the existing "unavailable" path.

### 3.3 Android (`apps/android/native/app/src/main/java/com/lingxi/code`)

Done in step 0: `navigation/DeepLink.kt` (routes, `uid`), `push/PushRoute.kt`, `friends/FriendLink.kt`, and the test
`app/src/test/java/com/lingxi/code/friends/FriendLinkTest.kt`.

To do: `friends/`: `FriendsLogic.kt` (pure rules with the D5 collator; `lastMessage.at` as whole epoch milliseconds),
`FriendsWorkspace.kt` (StateFlow, instantiable like `TeamsWorkspace`; the two listeners of D2 and no per-friend
listener), `FriendsTab.kt`, `AddFriendScreen.kt`, `FriendProfileScreen.kt`, `BlockedPeopleScreen.kt`,
`FriendChatScreen.kt` (`TeamConversationScreen` over a pair-pinned `TeamsWorkspace`). Wiring: `tabs/AppTab.kt`
(`Friends`), `tabs/MainTabShell.kt`, `tabs/TabLogic.kt` (`LinkTarget.Friends("friends")`, `tabForLink` returns it),
`navigation/DeepLinkInbox.kt` (`independent` also for `friend/`), `MainActivity.kt` near line 327 (friend routes call
a `tabState.requestFriends(FriendLink)`), the member row in `teams/TeamDetailsScreens.kt`. The manifest needs no
change (`/open/` is already an app-link prefix). Until this is wired a friend link reaches the existing unhandled path.

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

The sort rows hold only what every platform agrees on: activity order (`lastAtMs`, epoch milliseconds, so a 64-bit
integer), requests before friends, name order for ASCII-letter names in mixed case (the collators of D5 give one order
for them in every locale of the app; checked with Node ICU, the JDK collator and Foundation for zh-Hans, zh-Hant, en, ja
and ko), and ties by `uid`. Chinese, accented, numeric and punctuated names are left to each platform's collation and
have no rows. The case-only pairs (`Alice`/`alice`, `Kim`/`kim`, `Sam`/`sam`) fail a default-strength collator and
`localizedStandardCompare` on purpose; the pair `n9`/`n10` fails a numeric `uid` comparison.

Each client must add a test that loads the file from `packages/teams/fixtures/` (Android and iOS walk up from the test
file as `TeamBotsTest` does) and checks every row: `packages/teams/friends.test.ts`, iOS `FriendsLogicTests`,
Android `FriendsLogicTest`. `links.typed` and `links.push` are already checked by iOS `FriendLinkTests`, Android
`FriendLinkTest` and the TS test in `packages/deep-links/index.test.ts`.

Commands (repository root unless noted):

```bash
node --import ./packages/bridge-client/node_modules/tsx/dist/loader.mjs --test packages/deep-links/index.test.ts
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
10. **i18n gate** was already failing before this work (unpaired keys 94 vs baseline 93, zh-Hant identical 251 vs 235,
   en identical 20 vs 19); Friends adds nothing to those counts.
