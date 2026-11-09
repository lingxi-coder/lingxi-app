# Friends implementation contract

Status: phase F0 (backend core) and the friend-list summaries are source only. Nothing is deployed: the API, rules,
indexes, Hosting routes, scheduled prune and the two summary triggers below need the user's explicit approval (see
[Deployment gates](#deployment-gates)). Client behaviour (phases F1+F2) is specified in
[FRIENDS-CLIENTS.md](FRIENDS-CLIENTS.md).

A friendship is a hidden two-person **pair team** in the Teams stack described in [TEAMS.md](TEAMS.md).
Friends therefore reuse its membership rules, message mutation protocol, offline caches, push trigger and
SharedBots; this document only adds what is new: profiles, discovery, requests, the pair team lifecycle
and the places where the Teams contract treats `kind: 'pair'` differently.

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
| `users/{uid}/friendRequests/{requestId}` | idempotency receipt `{digest, otherUid, at}` | none |
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
| SharedBots commands | unchanged: a Bot published into a pair team with `usePolicy` `all` is usable by the friend (the F3 precondition) |

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

Errors: the Teams handler's own (401 `SIGN_IN_REQUIRED`/`INVALID_SESSION`, 403 `ORIGIN_NOT_ALLOWED`, 413) plus 400
`INVALID_INPUT`, 404 `NOT_FOUND` (uniform, see below), 404 `REQUEST_UNAVAILABLE` (`accept` with nothing to accept), 409
`HANDLE_TAKEN`, `REQUEST_ID_CONFLICT`, `STALE_REQUEST`, `FRIEND_LIMIT`, `REQUEST_LIMIT`, 429 `RATE_LIMITED`, and 503
`SERVICE_UNAVAILABLE` (email lookup without an Auth client).

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

## Push and deep links

Push stays generic and the title is the other person's name: no message, note, or invite text.

| Event | Route | `kind` | Body (zh / en) |
| --- | --- | --- | --- |
| message in a pair team | `friend/chat?uid=<sender>` | `friend_message` | 好友有新消息 / New message from a friend |
| incoming request | `friend/requests` | `friend_request` | 想添加你为好友 / wants to add you as a friend |
| request accepted | `friend/requests` | `friend_accepted` | 已接受你的好友请求 / accepted your friend request |
| friend link redeemed | `friend/requests` | `friend_linked` | 已通过你的好友链接成为好友 / is now your friend |

Team pushes keep `team/inbox` and `team_message`. Friends pushes are best effort: a provider or notifier failure never fails
the command, and a request push is skipped if the card it announces has already gone. Pair-team message pushes reuse
the existing `onTeamMessage` trigger and its membership/generation re-check.

Deep-link grammar (`packages/deep-links`, mirrored in `packages/deep-links-rs`, shared `fixtures.json`):
`friend/add?code=<32 URL-safe>`, `friend/requests` (no parameter), `friend/chat?uid=<1-128 URL-safe>`, each in the
`lingxi://` and `https://lingxios.net/open/` forms, each with exactly its one parameter, and no route other than
`team/join` and `friend/add` accepts `code`. As for Team invites, a client must not redeem on open: it shows the owner
(`lookup {code}`) and asks first. Hosting sends `no-referrer` and `no-store` for `/open/friend/add`. A uid outside
`[A-Za-z0-9_-]` can still be a friend, but its chat push cannot deep-link (Firebase-issued uids always match).

## Rules, indexes, routing

`infra/firebase/firestore.default.rules` adds `get`/`list` for the owner on `users/{uid}/friends` and `users/{uid}/blocks`,
and `get` on `profiles/{uid}` for the owner or for a user whose own card for it has `state == 'friends'`. Everything else
in this document, and every client write, has no rule and is denied; collection and collection-group queries on
profiles, friend codes and friendships are denied too. `firestore.teams.indexes.json` exempts `friendCodes.code`.
`firebase.json` adds the two Hosting rewrites and the link-page headers.

## Deployment gates

Deploying any of this requires explicit approval; F0 deploys nothing.

1. Order: default-database rules and indexes, then Functions (`teamsApi`, `prunePushRegistrations`, and the two summary
   triggers `onFriendChatUpdated` and `onFriendChatRead`), then Hosting, then clients.
2. IAM preflight: `teamsApi` and the scheduled function run as `push-api`. `getUserByEmail` needs Firebase Authentication
   read access for it (for example `roles/firebaseauth.viewer`); without it email lookup fails with a server error
   instead of `NOT_FOUND`. The daily prune now also deletes expired `friendLimits` in `(default)`.
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

`friends.test.mjs` (commands, push copy, handler, config), `friends.emulator.test.mjs` (profiles, discovery, every
transition including races, block names, pair-team contract, SharedBots in a pair team, push),
`friends.rules.emulator.test.mjs` (client access, unfriend closing the chat) and `friend-summary.emulator.test.mjs`
(summary create/edit/recall, duplicate and out-of-order events, read marks, unfriend and re-friend, non-pair data) are
part of the first `test:teams` command. `friend-summary.test.mjs` runs without the emulator: it checks that other team
and conversation ids cost no read and the trigger wiring (paths, region, service account, no retry).

Not in F0: client UI (see [FRIENDS-CLIENTS.md](FRIENDS-CLIENTS.md)), sharing Bots with friends, account-deletion
cleanup of friendships, and rename fan-out.
