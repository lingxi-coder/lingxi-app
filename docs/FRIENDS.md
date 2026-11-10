# Friends implementation contract

Status: phase F0 (backend core), the friend-list summaries and phases F3 (Bots in a friendship) and F4 (team offers) are
source only. Nothing is deployed: the API, rules, indexes, Hosting routes, scheduled prune and the two summary triggers
below need the user's explicit approval (see [Deployment gates](#deployment-gates)). Client behaviour (phases F1 to F4)
is specified in [FRIENDS-CLIENTS.md](FRIENDS-CLIENTS.md).

A friendship is a hidden two-person **pair team** in the Teams stack described in [TEAMS.md](TEAMS.md).
Friends therefore reuse its membership rules, message mutation protocol, offline caches, push trigger and
SharedBots; this document only adds what is new: profiles, discovery, requests, the pair team lifecycle,
the places where the Teams contract treats `kind: 'pair'` differently, and offers of an ordinary team to a friend.

## Data

All documents are in Firestore `(default)` and are written only by the trusted backend (`apps/firebase/src/friends.mjs`).
Rules grant no write anywhere below.

| Path | Content | Client read |
| --- | --- | --- |
| `profiles/{uid}` | `displayName`, optional `handle` (display case) and `handleLower`, `findByHandle`, `findByEmail` (both default true), `friendCodeId`, `updatedAt` | `get` by the owner and by a current friend; no `list` |
| `handles/{handleLower}` | `{uid}` uniqueness lock, swapped with the profile in one transaction | none |
| `friendCodes/{sha256(code)}` | `{uid, code, createdAt, revoked}`; the raw `code` field is index-exempt | none |
| `friendships/{pairKey}` | `{uids, state: pending\|friends\|ended, requester, epoch, teamId?, source, note?, createdAt, updatedAt, endedAt?}`; `pairKey = sha256(a\|b)` with `a < b` | none |
| `users/{uid}/friends/{other}` | the owner's index: `state: friends\|incoming\|outgoing`, `displayName`, `handle?`, `note?`, and for `friends` also `teamId`, `generation: 1`, `since`, and the chat summary `lastSeq`, `lastMessage`, `lastChangeSeq`, `lastReadSeq` (absent until the first message or read mark; see [Friend list summaries](#friend-list-summaries)); deleted when the relation ends | owner `get`/`list` |
| `users/{uid}/blocks/{other}` | `{at, displayName, handle?}`: the blocked person's name when the block was made | owner `get`/`list` |
| `users/{uid}/teamOffers/{teamId}` | a friend's offer of a team to the owner: `{teamId, teamName, fromUid, fromName, role: 'member', expiresAt}` (names are snapshots); the document id is the team id, so there is one offer per (team, friend); see [Team offers](#team-offers-f4) | owner `get`/`list` |
| `users/{uid}/friendRequests/{requestId}` | idempotency receipt `{digest, otherUid, at}` of `request`, `redeem` and `offerTeam` (an offer's receipt also holds `offerId` and `expiresAt`) | none |
| `users/{uid}/teamJoinRequests/{requestId}` | the Teams receipt of a join: `{teamId, inviteId, generation}` for an invite link, `{teamId, offerId, generation}` for `acceptOffer` | none |
| `friendLimits/{sha256(bucket:uid)}` | sliding window `{sentAt[], expiresAt}`; pruned by the daily scheduled function | none |

`friendships/{pairKey}` serializes every transition between two users, and the `users/{uid}/friends` cards, the
pair team and the friendship document change in **one transaction**. The cards are projections that never disagree
with the friendship. Rules deny every client write, so a forged `state: 'friends'` card cannot exist.

Names are snapshots. Cards, block entries, pair team member documents and the conversation record carry the name the
person had when the relation (or the block) was made; `profiles/{uid}` is the current one (readable by friends). There
is no rename fan-out.

## Pair team

`createPairTeam` (in `teams.mjs`) writes, inside the friendship transaction:

- `teams/{teamId}`: `{name: 'Friends', kind: 'pair', createdBy, state: 'active', memberCount: 2}`;
- two members (`role: 'member'`, `generation: 1`) and their `membershipEpochs`;
- one `dm` conversation `friend_chat` with participants `user:{a}`, `user:{b}`, built by the same
  `conversationRecord` as trusted `createTeamConversation`.

`teamId = 'f_' + first 40 hex of sha256(a|b|epoch)` (`a < b`). `epoch` counts the pair's friendships and is stored in
`friendships/{pairKey}`: every new friendship gets a new team, so a deleted pair team and its messages are never reused.
No `users/{uid}/teams` index entry exists for a pair team.

Ending a friendship (`remove`, `block`) is Team deletion: the team becomes `state: deleted`, `memberCount: 0`, both
members are deleted, and the team, epochs, conversation and messages stay as an unreachable tombstone. Rules and
every Teams API require `state: active` and a current member, so both users lose reads and writes at once.

### How the Teams contract treats pair teams

| Surface | Behaviour |
| --- | --- |
| `GET /v1/teams` | never lists them |
| `USER_TEAM_LIMIT` (20) / `TEAM_MEMBER_LIMIT` (200) | pair teams do not count |
| `GET /v1/teams/{teamId}` | works for the two members; `team.kind: 'pair'`, `team.name: 'Friends'`, `invites: []` |
| every `POST /v1/teams` action on a pair team | 400 `PAIR_TEAM` after the membership check (an outsider still gets 403 `MEMBERSHIP_REQUIRED`) |
| `create` with a team ID starting `f_` | 400 `PAIR_TEAM`: the ID space is reserved so nobody can occupy a future pair team's ID |
| `join` with an invite that points at a pair team | 400 `PAIR_TEAM` |
| `POST /v1/teams/messages` `create` conversation | 400 `PAIR_TEAM`, even for `friend_chat`; the backend writes the only conversation |
| message `mutate`, `sync`, `read`, `list` | unchanged |
| SharedBots commands | unchanged, except that `createConversation` accepts only `kind: 'bot_dm'` (400 `PAIR_TEAM` for `group` and `channel`); see [Shared Bots in a friendship](#shared-bots-in-a-friendship-f3) |
| `offerTeam` naming a pair team | 400 `PAIR_TEAM` after the membership check |

## Shared Bots in a friendship (F3)

A friendship is a Team of two, so the SharedBots execution contract ([TEAMS.md](TEAMS.md)) applies unchanged and F3 adds no
endpoint and no data. `friends.emulator.test.mjs` ("shared Bots in a friendship") drives the whole path in the emulator.
What it means in a pair team:

| Step | Behaviour |
| --- | --- |
| publish | `publishSharedBot` by either friend (the maintainer), normally with `usePolicy: {mode: 'all'}`: the audience is the two members at that moment. `listed` with the other friend works the same. Quotas apply as in a Team (`team`, `member`, `bot` requests per UTC day). |
| list | `listSharedBots` by either member returns the Bots they may use; the maintainer also sees their own (with `instanceId`, and with `state: 'paused'` after a pause). `permitted` and `ready` are as in a Team. A non-member gets 403 `MEMBERSHIP_REQUIRED`. |
| conversations | `createConversation` accepts only `kind: 'bot_dm'` in a pair team; `group` and `channel` are 400 `PAIR_TEAM` (a friendship holds its friend chat and one Bot DM per Bot). The readers of a Bot DM are the requesting friend **and the Bot's maintainer**: the maintainer can read what the friend asks their Bot, the same disclosure as in a Team, which the clients show in the Bots panel and in the publication review ([FRIENDS-CLIENTS.md](FRIENDS-CLIENTS.md) D21, D23). |
| submit and host | `submit`, `listHostTasks`, `startTask`, `completeTask` and `readArtifact` are unchanged. The maintainer's Desktop is the host and answers in the Bot DM as the Bot. |
| stop sharing | `pauseSharedBot` is the client's "stop sharing": the Bot leaves the friend's list, `submit` is 403 `BOT_USE_REQUIRED`, the history stays readable to both, and publishing again (the next revision) restores it. |
| unfriend | `remove` and `block` delete the pair team, so every command above answers 403 `MEMBERSHIP_REQUIRED` for both friends (the host's included) and the rules close the Bot DMs together with the friend chat. A new friendship is a new team: no Bot and no conversation come back. |

**Push route for a Bot DM (decided).** A message in a Bot DM of a pair team (the friend's question, delivered to the
maintainer, or the Bot's answer, delivered to the friend) uses the existing pair route `friend/chat?uid=<sender>`, kind
`friend_message` and the generic copy; for the Bot's answer the sender is the Bot's maintainer and the title is the Bot's
name. No route and no parameter are added: the deep-link grammar, `fixtures.json` and the TypeScript, Rust, Android and iOS
parsers are unchanged. A tap opens the friend chat with that friend, where the Bots entry
([FRIENDS-CLIENTS.md](FRIENDS-CLIENTS.md) D21) is one tap from the Bot DM. A `conversation` parameter would loosen a strict
grammar in five parsers to save that tap.

**Summaries are unaffected.** `onFriendChatUpdated` and `onFriendChatRead` fire on the literal `friend_chat` path, and
their handlers ignore any other conversation of a pair team, so a Bot DM never changes a friend card, its preview, its
unread count or the Friends badge. A Bot DM has no unread count in v1 (`friend-summary.test.mjs` and the "non-pair data is
ignored" test of `friend-summary.emulator.test.mjs` cover a `bot_chat_01` event).

**Desktop host discovery (contract for the Desktop client).** `TeamConnector.discover()`
(`apps/electron/src/main/teams/connector.ts`) runs a binding only if `TeamsService.list(uid)` (`GET /v1/teams`) contains
its team with the binding's generation. That list never contains a pair team, so a Bot bound to a friendship would be
skipped on every pass. A binding whose `teamId` starts with `f_` (`PAIR_PREFIX`, a reserved id space, so no ordinary team
can match it) therefore skips that check, with generation 1, and the server decides: every host command already checks the
current team and member. When the friendship has ended, the next command is a definitive 403 `MEMBERSHIP_REQUIRED`; the
connector stops the binding as it does for any definitive 4xx (aborts its runs, drops its heartbeat) and also keeps it in an
in-memory `ended` set until `revoke()` (sign-out or account change), so the five-second discovery loop does not call a dead
team again. Nothing is written to the private store. A new friendship has a new team id and needs a new publication.

## Friend list summaries

A client lists friends with one listener on `users/{uid}/friends`; it needs no listener per friend. For a `friends`
card the backend copies the pair chat's newest-message pointer and the owner's own read mark onto the card:

| Card field | Value | From |
| --- | --- | --- |
| `lastSeq` | sequence of the newest message | `conversation.lastSeq` |
| `lastMessage` | the pointer whole, exactly as stored: `{id, seq, at, author, preview, recalled?}`; a recalled newest message has `preview: ''` and `recalled: true` | `conversation.lastMessage` |
| `lastChangeSeq` | the conversation change counter this card has taken (the ordering guard below) | `conversation.lastChangeSeq` |
| `lastReadSeq` | the owner's read mark | `reads/{owner}.lastReadSeq` |

All four are absent on a new friendship (the card is written whole at each friendship, so none survives into the
next): absent means no message and nothing read. **Unread is `max(0, lastSeq - lastReadSeq)`**, computed by the client
from the card. The sender's `lastReadSeq` moves only when that client calls the Teams `read` action (a chat that is open
with the newest message visible does), so the backend never counts a user's own message as read on their behalf. The
preview is text the owner can already read in the chat, on a card only the owner can read.

Two Firestore triggers in `(default)` do the copying (`apps/firebase/src/friend-summary.mjs`, wired in `index.mjs` next
to `onTeamMessage` with the same region `us-central1`, service account `push-api`, 256 MiB and at most 2 instances):

| Function | Fires on | Writes |
| --- | --- | --- |
| `onFriendChatUpdated` | update of `teams/{teamId}/conversations/friend_chat` | `lastSeq`, `lastMessage`, `lastChangeSeq` on both members' cards |
| `onFriendChatRead` | any write of `teams/{teamId}/conversations/friend_chat/reads/{uid}` | `lastReadSeq` on `{uid}`'s own card only |

The conversation id is literal in the trigger path, so a conversation of an ordinary Team never starts a function
(path patterns cannot match on the `f_` team-id prefix, so a `{conversationId}` wildcard would start one invocation per
message of every Team). The handlers still require a `teamId` starting `f_`, a team of `kind: 'pair'` and a well-formed
event (two distinct member uids, whole-number counters), and return before any read otherwise. A message costs one
invocation (three reads, at most two writes); a read mark costs one (three reads, at most one write).

Events are at-least-once and unordered, and neither function sets `retry`. Each write is a transaction that reads the card
first, which keeps it correct:

- It writes only while the card is still this friendship: `state: 'friends'` with `teamId` equal to the event's team. After
  `remove`, `block`, or a re-friend (a new epoch and team id) a late event writes nothing, creates no card, and never
  reaches the new friendship's cards.
- A conversation event applies only if its `lastChangeSeq` is greater than the card's, so duplicates and older events are
  ignored, and `lastSeq` is kept at its maximum besides. Every event carries the whole pointer, so a lost event is healed by
  the next one.
- A read mark applies only if it is greater than the card's `lastReadSeq` and its membership `generation` is the card's.

The triggers change nothing else on a card.

## API

`GET /v1/friends/profile` returns the caller's profile. `POST /v1/friends` takes one strict action object (unknown
fields are `INVALID_INPUT`; 16 KiB cap). Both are mounted in the Teams handler, with its authentication
(revocation-checked ID token), CORS and error shape. Hosting rewrites `/v1/friends` and `/v1/friends/**` to `teamsApi`
before the catalog rewrite.

| Action | Body | Result |
| --- | --- | --- |
| `profile` | `{displayName?, handle?, findByHandle?, findByEmail?}`; empty reads (and on first use creates) | `{uid, displayName, handle?, findByHandle, findByEmail, friendCode: {code, link} \| null}` |
| `lookup` | exactly one of `handle`, `email`, `code`, or `teamId` with `uid` | `{uid, displayName, handle?, relation: none\|friends\|incoming\|outgoing}` |
| `request` | `{requestId, note?, via: handle\|email\|team}` plus the proof for `via`: `handle`, `email`, or `teamId` with `uid` | `{uid, state: 'outgoing'}` or `{uid, state: 'friends', teamId}` (`uid` is who the proof resolved to) |
| `accept` | `{otherUid}` | `{state: 'friends', teamId}` |
| `decline`, `cancel` | `{otherUid}` | `{ok: true}` |
| `redeem` | `{code, requestId}` | `{state: 'friends', uid, teamId}` |
| `remove`, `block`, `unblock` | `{otherUid}` | `{ok: true}` |
| `rotateCode`, `revokeCode` | `{}` | `{code, link}`, `{ok: true}` |
| `offerTeam` | `{teamId, friendUid, requestId}` | `{offerId, expiresAt}` (ISO time) |
| `acceptOffer` | `{offerId, requestId}` | `{teamId, generation, alreadyMember}` |
| `declineOffer` | `{offerId}` | `{ok: true}` |

Errors: the Teams handler's own (401 `SIGN_IN_REQUIRED`/`INVALID_SESSION`, 403 `ORIGIN_NOT_ALLOWED`, 413) plus 400
`INVALID_INPUT`, 404 `NOT_FOUND` (uniform, see below), 404 `REQUEST_UNAVAILABLE` (`accept` with nothing to accept), 409
`HANDLE_TAKEN`, `REQUEST_ID_CONFLICT`, `STALE_REQUEST`, `FRIEND_LIMIT`, `REQUEST_LIMIT`, 429 `RATE_LIMITED`, and 503
`SERVICE_UNAVAILABLE` (email lookup without an Auth client). The offer actions add the codes of
[Team offers](#team-offers-f4).

## States and idempotency

```
none ──request──▶ pending(requester) ──accept (receiver)──▶ friends ──remove / block──▶ ended
                     │  ▲                                      ▲
   decline / cancel ─┘  └── mutual request, or redeem ─────────┘
```

- A request to a user who already asked the caller completes the friendship, exactly like `accept`, but only through a
  proof that resolves to that user as `lookup` would show them. A friend link (`redeem`) is the code owner's standing
  consent: it makes the friendship at once and replaces a pending request in either direction. Both keep who asked
  first; a mutual request keeps the original `source`, `redeem` records `code`.
- `accept`, `decline`, `cancel`, `remove`, `block`, `unblock` and `revokeCode` are naturally idempotent. `decline` and
  `cancel` on a request that is gone, or in the other direction, change nothing. `remove` on a non-friend is a no-op.
- `request` and `redeem` carry a client `requestId` (8-64 URL-safe characters). The receipt binds it to the payload
  digest (the proof included): the same id and payload replays the result for the user the first call resolved, while
  the relation still matches (`outgoing` or `friends`; a replay is not a new discovery), otherwise `STALE_REQUEST`; the
  same id with another payload is `REQUEST_ID_CONFLICT`. An ended friendship therefore cannot be recreated by a delayed
  retry; asking again uses a new id.
- `accept`, a mutual `request` and `redeem` create the pair team (`epoch + 1`), both cards and the friendship in one
  transaction. A repeated accept returns the existing `teamId`.
- `block` writes `users/{uid}/blocks/{other}` as `{at, displayName, handle?}` (once; a repeat keeps the first entry), ends
  the friendship or pending request in both directions and deletes the pair team in the same transaction. The name is the
  person's profile, else the name already on the caller's card for them, else `Member`, as every transition reads it, so
  a Blocked list can show names on any device. `unblock` only deletes the block; it restores nothing and either side may
  ask again.

## Team offers (F4)

A manager of an ordinary team invites a friend without a link. `offerTeam` writes an **offer** into the friend's own
`users/{friendUid}/teamOffers/{teamId}`; the friend sees it in Friends and accepts or declines. An offer holds no secret
and grants nothing: until `acceptOffer`, the friend is not a member, cannot read the team, and the team does not know of
them. The document is `{teamId, teamName, fromUid, fromName, role: 'member', expiresAt}`: the team's and the sender's
names as they were when the offer was made, and the role the friend will join with (always `member`; promoting stays a
Team action).

| Action | Rules |
| --- | --- |
| `offerTeam {teamId, friendUid, requestId}` | The caller is a current **owner or admin** of the team, which is ordinary (not a pair team) and active, and has room (`memberCount` below `TEAM_MEMBER_LIMIT`). `friendUid` is a current friend (never blocked: a block ends the friendship) and not yet a member. The friend holds fewer than 20 live offers, unless this one refreshes a live offer. It writes the offer, replacing an earlier one for the same team and friend, and the receipt. Result `{offerId, expiresAt}` (`offerId` is the document id, today the team id; clients treat it as opaque). |
| `acceptOffer {offerId, requestId}` | The caller's own offer exists and has not expired; its team is active and ordinary; its sender (`fromUid`) is still a manager of it and still the caller's friend. Then the friend joins through the same core as an invite link (`admitMember` in `teams.mjs`: the new membership generation, `users/{uid}/teams`, the member count, the membership signal, `TEAM_MEMBER_LIMIT`, `USER_TEAM_LIMIT`), and the offer is deleted. Result `{teamId, generation, alreadyMember}`, as `join`. |
| `declineOffer {offerId}` | Deletes the caller's own offer. A missing offer is a harmless repeat (`{ok: true}`). It also dismisses an expired offer. |

Life cycle:

- **One live offer per (team, friend).** A repeat refreshes the same document: a new 7-day expiry, and `fromUid` and
  `fromName` become the new sender's (any manager of the team may offer). A repeat while the offer is live is silent;
  only a new offer or one that had expired pushes (see [Push](#push-and-deep-links)), so repeating cannot be used to nag.
- **TTL.** An offer lives 7 days (`OFFER_TTL_MS`, as long as the longest invite link). It then stays listed as expired for
  7 more days (`OFFER_GRACE_MS`), so the friend can see it lapsed and dismiss it; the daily scheduled function deletes it
  after that (a collection-group query on `expiresAt`, which needs the index override below). An expired offer cannot be
  accepted.
- **Cap.** At most 20 live offers per recipient (`OFFER_LIMIT`; expired offers do not count). Expired and live offers
  together stay below about 40 documents per friend.
- **Gone with the friendship.** `remove` and `block` delete, in the friendship's transaction, every offer the two friends
  sent each other (offers whose `fromUid` is one and whose owner is the other). An offer another manager refreshed belongs to
  that manager's friendship and stays. Because `offerTeam` reads the friendship document, an offer and an unfriend
  serialize.
- **Gone with the join.** Whichever way a friend joins a team (`acceptOffer` or an invite link), `admitMember` deletes
  their offer of it.
- **Stale offers.** Deleting or restructuring a team does not scan offers. An offer whose team was deleted, or whose
  sender was demoted, removed or left, stays listed until the friend accepts (the attempt deletes it and answers 404
  `OFFER_UNAVAILABLE`, uniform for every such cause), declines it, or the prune removes it. A refusal the friend can
  fix keeps the offer: 409 `MEMBER_LIMIT` (the team is full), 409 `TEAM_LIMIT` (the friend is in 20 teams).
- **Idempotency.** `offerTeam` stores a receipt in `users/{caller}/friendRequests/{requestId}` bound to the payload
  digest: the same id and payload replays the first result (even if the offer has since been answered), another payload
  is `REQUEST_ID_CONFLICT`. `acceptOffer` uses `users/{caller}/teamJoinRequests/{requestId}` as a link join does: a replay
  answers `alreadyMember: true` while the membership still has that generation, otherwise `STALE_MEMBERSHIP`; the same id
  for another offer or for an invite link is `REQUEST_ID_CONFLICT`. Two simultaneous accepts end in one membership.
- **Rate limit.** `offerTeam` spends the same 30 per 10 minutes as `request` and `redeem`, refused calls included.
- **Privacy.** The sender can neither read the offer nor learn whether it was seen, accepted or declined (the new member
  shows in the team's member list). The friend sees only the two snapshotted names. User-authored text (team name, sender
  name) is never in the push.

Errors of the three actions, besides the Teams handler's own and 400 `INVALID_INPUT` (a malformed or extra field, or an
offer to oneself) and 429 `RATE_LIMITED`:

| Code | Status | When |
| --- | --- | --- |
| `MEMBERSHIP_REQUIRED` | 403 | `offerTeam`: the team does not exist, is deleted, or the caller is not a member |
| `ROLE_REQUIRED` | 403 | `offerTeam`: the caller is a plain member |
| `PAIR_TEAM` | 400 | `offerTeam` naming a pair team |
| `FRIEND_REQUIRED` | 403 | `offerTeam`: the target is not a current friend (a stranger, a pending request, a blocked or unknown uid are one answer) |
| `ALREADY_MEMBER` | 409 | `offerTeam`: the friend is already in the team |
| `MEMBER_LIMIT` | 409 | `offerTeam` and `acceptOffer`: the team has 200 members |
| `OFFER_LIMIT` | 409 | `offerTeam`: the friend holds 20 live offers |
| `OFFER_UNAVAILABLE` | 404 | `acceptOffer`: no such offer, or it can no longer be honoured (expired, team gone, sender no longer a manager or a friend) |
| `TEAM_LIMIT` | 409 | `acceptOffer`: the friend is in 20 teams |
| `REQUEST_ID_CONFLICT`, `STALE_MEMBERSHIP` | 409 | a reused `requestId` for another payload; `acceptOffer` replay after the friend left again |

## Discovery and privacy

- A profile and its one active friend code are created on first use. The default name is the ID-token name, else `Member`.
- **Handles** match `^[A-Za-z0-9_]{3,24}$` and are unique case-insensitively through `handles/{handleLower}`.
- **Lookup** returns only `uid`, `displayName`, `handle` (whenever the profile has one, on every route) and the caller's
  `relation`; an email address is never returned. The routes differ in what they require:
  - `handle` needs a profile with that handle and its `findByHandle`;
  - `email` needs an Auth account whose email is **verified** (Admin Auth `getUserByEmail`, injected as `auth`), so
    someone who registered an address they do not own is not found, and `findByEmail`. It defaults to true, so an
    account that never opened Friends (no profile) is discoverable; its card name is the Auth display name, else
    `Member`;
  - `code`: the code is the consent and the owner's profile exists; the flags are ignored;
  - `teamId` + `uid`: both must be current members of the same active team, with or without a profile; the flags are
    ignored.
- **A request carries the same proof**, never a bare uid: `{via: 'handle', handle}`, `{via: 'email', email}` or
  `{via: 'team', teamId, uid}`, and any other combination (a `targetUid`, a `uid` without a `teamId`, two proofs) is
  `INVALID_INPUT`. The server resolves the owner as `lookup` does, with one shared rule: the handle lock is read inside
  the transaction, `getUserByEmail` runs once before it, team membership is checked inside it. So knowing a uid proves
  nothing; a handle or email that is not the target's finds nobody, or only whoever really owns it. A request is
  refused with the same 404 wherever `lookup` would show nothing, then the relation decides: friends, already asked,
  the other asked first (accept), or a new request.
- **Uniform `NOT_FOUND`**: unknown, hidden by a flag, unverified email, no profile (handle and code routes only), yourself,
  blocked in either direction, a revoked or rotated code, and a team the caller is not in all return the same 404 (for
  `lookup`, `request` and `redeem`, whichever causes apply).
- **Friend code**: 32 URL-safe characters from 24 random bytes. Only its SHA-256 is a document ID; the raw code sits in
  the Admin-only `friendCodes` document and is returned to its owner by `profile`. There is at most one active code:
  `rotateCode` revokes the old one in the same transaction, `revokeCode` leaves none (and reading a profile does not
  make a new one). Link: `https://lingxios.net/open/friend/add?code=<code>`.
- `note` (at most 200 characters) is visible only to the two people, lives on the pending friendship and both cards, is
  removed when the request ends, and is never put in a push.

## Limits

500 friends per user (checked for both sides on accept, mutual request and redeem: `FRIEND_LIMIT`), 50 outgoing
requests per user (`REQUEST_LIMIT`; incoming requests are not capped, the receiver can block), display name 80
characters. `lookup` is limited to 30 per rolling 10 minutes per user; `request` and `redeem` (which push to the other
user, and resolve a proof) share a separate 30 per 10 minutes, so discovery through `request` is limited too. Misses
count, a refused call is not recorded, and a window closes by itself (`friendLimits` documents are also pruned daily).
`offerTeam` shares the `request` window. A friend holds at most 20 live team offers (`OFFER_LIMIT`); an offer lives 7 days
and stays listed as expired for 7 more (`OFFER_TTL_MS`, `OFFER_GRACE_MS`).

## Push and deep links

Push stays generic and the title is the other person's name: no message, note, or invite text.

| Event | Route | `kind` | Body (zh / en) |
| --- | --- | --- | --- |
| message in a pair team | `friend/chat?uid=<sender>` | `friend_message` | 好友有新消息 / New message from a friend |
| incoming request | `friend/requests` | `friend_request` | 想添加你为好友 / wants to add you as a friend |
| request accepted | `friend/requests` | `friend_accepted` | 已接受你的好友请求 / accepted your friend request |
| friend link redeemed | `friend/requests` | `friend_linked` | 已通过你的好友链接成为好友 / is now your friend |
| team offer (new, or after it expired) | `friend/requests` | `friend_offer` | 邀请你加入团队 / invited you to join a team |

Team pushes keep `team/inbox` and `team_message`. Friends pushes are best effort: a provider or notifier failure never fails
the command, and a request or offer push is skipped if the card or offer it announces has already gone. An offer push names
the sender (the title) and never the team. A repeat of a live offer and a replay of a receipt send nothing. Pair-team message pushes reuse
the existing `onTeamMessage` trigger and its membership/generation re-check.

Deep-link grammar (`packages/deep-links`, mirrored in `packages/deep-links-rs`, shared `fixtures.json`):
`friend/add?code=<32 URL-safe>`, `friend/requests` (no parameter), `friend/chat?uid=<1-128 URL-safe>`, each in the
`lingxi://` and `https://lingxios.net/open/` forms, each with exactly its one parameter, and no route other than
`team/join` and `friend/add` accepts `code`. As for Team invites, a client must not redeem on open: it shows the owner
(`lookup {code}`) and asks first. Hosting sends `no-referrer` and `no-store` for `/open/friend/add`. A uid outside
`[A-Za-z0-9_-]` can still be a friend, but its chat push cannot deep-link (Firebase-issued uids always match).

## Rules, indexes, routing

`infra/firebase/firestore.default.rules` adds `get`/`list` for the owner on `users/{uid}/friends`, `users/{uid}/blocks` and
`users/{uid}/teamOffers` (the sender cannot read the offer they made),
and `get` on `profiles/{uid}` for the owner or for a user whose own card for it has `state == 'friends'`. Everything else
in this document, and every client write, has no rule and is denied; collection and collection-group queries on
profiles, friend codes and friendships are denied too. `firestore.teams.indexes.json` exempts `friendCodes.code` and
enables `teamOffers.expiresAt` for collection and collection-group queries (the daily prune of expired offers).
`firebase.json` adds the two Hosting rewrites and the link-page headers.

## Deployment gates

Deploying any of this requires explicit approval; F0 deploys nothing.

1. Order: default-database rules and indexes, then Functions (`teamsApi`, `prunePushRegistrations`, and the two summary
   triggers `onFriendChatUpdated` and `onFriendChatRead`), then Hosting, then clients. The `teamOffers.expiresAt`
   collection-group index must be built before the new `prunePushRegistrations`: without it the offer sweep fails with
   `FAILED_PRECONDITION` (the `friendLimits` sweep before it still runs).
2. IAM preflight: `teamsApi` and the scheduled function run as `push-api`. `getUserByEmail` needs Firebase Authentication
   read access for it (for example `roles/firebaseauth.viewer`); without it email lookup fails with a server error
   instead of `NOT_FOUND`. The daily prune now also deletes expired `friendLimits` and offers expired for 7 days in `(default)`.
   The summary triggers run as the same `push-api` as `onTeamMessage`, on the same `(default)` database and region, and
   write only `users/{uid}/friends` there, as `teamsApi` already does, so they need no IAM beyond it. They are new
   Firestore (Eventarc) triggers: the first deploy creates them, and the cards fill in from the next message or read mark.
   Nothing is backfilled: with F0 undeployed no friendship exists yet, so there is nothing to catch up.
3. Android and iOS learn the three routes (`DeepLink.kt`, `PushRoute.kt`, `CanonicalDeepLink`, `PushMessagePolicy`, with
   the typed `FriendLink` in both) in the client step; the shared fixture corpus tests pass with them. Until the Friends
   screens are wired, a friend link or friend push reaches the existing "unavailable" path.

## Validation

```bash
npm --prefix apps/firebase test                                   # unit tests (emulator tests skip)
JAVA_HOME=/opt/homebrew/opt/openjdk@21 npm --prefix apps/firebase run test:teams   # emulator: rules + stores
node --import ./packages/bridge-client/node_modules/tsx/dist/loader.mjs --test packages/deep-links/index.test.ts
cargo test --locked -p lingxi-deep-links
```

`friends.test.mjs` (commands including the three offer actions, push copy, handler, config, the index override),
`friends.emulator.test.mjs` (profiles, discovery, every transition including races, block names, pair-team contract, shared
Bots in a friendship from publish to unfriend, the offer actions with their refusals, limits, races, cleanup and prune,
push), `friends.rules.emulator.test.mjs` (client access, offers, unfriend closing the chat; the join refactor is covered by
`teams.members.emulator.test.mjs`) and `friend-summary.emulator.test.mjs`
(summary create/edit/recall, duplicate and out-of-order events, read marks, unfriend and re-friend, non-pair data) are
part of the first `test:teams` command. `friend-summary.test.mjs` runs without the emulator: it checks that other team
and conversation ids cost no read and the trigger wiring (paths, region, service account, no retry).

Not built: client UI (see [FRIENDS-CLIENTS.md](FRIENDS-CLIENTS.md)), account-deletion cleanup of friendships and offers,
cleanup of offers when their team is deleted, and rename fan-out.
