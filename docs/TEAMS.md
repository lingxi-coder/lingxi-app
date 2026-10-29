# Teams implementation contract

Status: T0 validated; T1 implemented locally, with deployment and full native
product/device acceptance still pending. This document refines the approved 2026-10-29 Team plan
after review. It is the repository contract for subsequent phases; a phase is
complete only when its stated checks pass on all four clients.

## Product boundaries

- Team member messaging and shared Bot communication use Firestore `(default)`.
  The UI discloses that the platform can read messages and that the Bot owner can
  read conversations with their Bot.
- Remote Control continues to authenticate the same account and approved device.
  Team membership does not authorize RC access.
- Team Local Apps execute on the owner's Desktop through the authenticated HTTPS
  gateway. The app owner approves access, expiry and Agent scope. Mobile uses the
  resolved system browser by default and retains the existing opening choices.
- No billing or always-on host is added.

## Trusted conversation membership (T1/T2)

`teamsApi` verifies a Firebase ID token, reads current membership in the same
transaction, and creates conversations. Clients cannot create conversations or
change their participants, `memberUids`, kind or Bot identity directly.

The backend accepts participant principals, never a caller-supplied read ACL:

- A `dm` has exactly two distinct human members. `memberUids` equals those UIDs.
- A `bot_dm` has exactly one human member and one registered Bot. Its immutable
  `botKey` identifies that Bot. `memberUids` is exactly the human and the Bot
  owner (deduplicated). The human must pass the Bot's current `use: all|listed`
  permission. A Bot owner can always use their own Bot.
- Group/channel principals are unique, current members or registered Bots. The
  backend derives the read ACL, including the owners of participating Bots.
  Membership additions/removals use a trusted transaction, not a message update.
- Conversation identity/idempotency includes the participant set. A retry cannot
  change the participants of an existing request.

Rules separately enforce current team membership, conversation read membership,
Bot authorship and `bot_dm` send permission. Revoking Bot use prevents new turns;
it does not erase already-shared conversation history. Removing the member or
deleting the team denies subsequent server reads and writes. Cached data cannot
be remotely erased and must not be represented as a new authorized read.

The only member-document client update is the caller's own `lastActiveAt`, using
a server timestamp. Role, generation and all other membership fields are trusted
backend writes.

## T1 membership API and clients

`teamsApi` uses the existing Functions codebase and existing `push-api` service
identity. Hosting routes `/v1/teams` and `/v1/teams/**` before the catalog rewrite.
Every request verifies the caller's ID token with revocation checking. The API
is defined locally; deploying it and its default-database IAM/signing permissions
is a separate gate. There are no Remote Control authorization changes.

| Request | Body / result |
| --- | --- |
| `GET /v1/teams` | Current teams from the caller's private index, revalidated against team state and membership |
| `GET /v1/teams/{teamId}` | Current team, members and (for managers) invite metadata |
| `POST /v1/teams/session` | Empty object; custom token for the authenticated UID, Desktop main only |
| `POST /v1/teams` | Strict action object, as below |

Actions: `create {teamId,name}`, `delete {teamId}`, `invite
{teamId,requestId,role,maxUses,ttlSeconds}`, `revokeInvite {teamId,inviteId}`, `join
{code,requestId}`, `leave {teamId,generation}`, `remove {teamId,uid,generation}`,
`role {teamId,uid,generation,role}` and the trusted `conversation` constructor.
IDs/request IDs use 8–64 URL-safe characters (UUIDs work). Invites have 192 bits
of random entropy, 1–50 uses and 1 hour–7 days validity. Their global lookup is a
SHA-256 digest; raw links are returned only to their creator and retained in an
Admin-only idempotency record. Members cannot enumerate invitations through rules.

Only the owner can delete a team or assign/remove administrators. An administrator
may issue member invitations and remove ordinary members; a member can leave.
The single owner cannot leave or be removed/demoted. Ownership transfer is outside
T1. Limits are 200 members/team and 20 teams/user, keeping deletion/index updates
within one transaction. Creation, joining, role changes, removal and deletion
maintain the `users/{uid}/teams/{teamId}` index atomically.

Deletion marks the team `state: deleted` and deletes active memberships/indexes
in the same transaction. Rules and trusted conversation creation require
`state: active`. Epochs, request receipts and the team tombstone remain so an old
request or invitation cannot recreate deleted state. Deletion is access revocation,
not a promise to erase every record or another device's already-cached data.

`users/{uid}/teamJoinRequests/{requestId}` is an Admin-only durable receipt bound
to the invitation, team and resulting generation. A lost join response can be
retried without consuming another invite use. After removal/leave or a later
rejoin, replaying the original request returns `STALE_MEMBERSHIP`; it cannot create
a fresh membership. An intentional rejoin uses a new request ID and advances the
durable membership epoch. Removal, leaving and role updates also require the
expected target generation so delayed mutations cannot affect a later rejoin.

All four clients expose Teams/member management and review an invite before
joining. Supported links are `lingxi://team/join?code=...` and
`https://lingxios.net/open/team/join?code=...`; the latter offers the Web page
`/teams/join?code=...`. Duplicate/extra parameters, alternate origins and fragments
are rejected. Opening a link never joins automatically. Mobile links preserve
the local conversation and do not need its engine; a pending invite survives the
login page. The Web removes the code from its address bar after capturing it and
uses `no-referrer` on invitation/Teams pages.

Desktop/Web share `packages/teams/TeamsPanel.tsx`; Firestore remains in Desktop
main and renderer IPC is bound to the panel's expected account UID. Browser and
native clients use their existing Firebase account. SDK listeners invalidate the
team/member views, while authenticated HTTP responses populate them. Cached SDK
snapshots do not grant access. Account changes and listener permission failures
clear stale views; request/lifecycle versions discard late responses and prevent
reattachment after a native page closes. Failed listeners do not retry continuously
and can attach for a later membership generation. T1 requires a network connection;
T2's durable offline message projection remains separate.

Dedicated real Firebase owner/member test users were explicitly authorized and
created on 2026-10-29 without admin claims or verification emails. After separate
explicit authorization, isolated `lingxios.net` browser sessions were saved and
restored without login. Credentials, refresh tokens and IndexedDB storage states
are only in the mode-0700, Git-ignored `output/teams-t1-private/` directory, with
mode-0600 files. `node apps/firebase/scripts/teams-test-accounts.mjs` refreshes and
reuses them; `--create` is only for explicitly authorized initial creation. Preserve
this private directory separately before removing/archiving the worktree. Never
copy its contents into reports, Git, screenshots or logs.

## Message identity and mutation catch-up (T0/T2)

Conversations have two independent cursors, both initialized to zero:

| Field | Advances for | Purpose |
| --- | --- | --- |
| `lastSeq` | New message | Stable message order and read watermarks |
| `lastChangeSeq` | Create, edit, recall | Complete reconnect catch-up |

Messages retain `seq` for life and carry the latest `changeSeq` and `changeId`.
`changes/{changeId}` is an immutable receipt containing
`{seq, messageId, operation: create|edit|recall, actorUid, at, textDigest}`.
`textDigest` is lowercase SHA-256 of the resulting UTF-8 body, checked by rules.
It preserves retry evidence without retaining a recalled plaintext body. Message/change IDs
are client-generated 8–64 character URL-safe IDs. Timestamps use server time.
Text is at most 8192 UTF-8 bytes. Recall permanently blanks the body and leaves a
tombstone; it cannot be edited back into a normal message.

Rules use Firebase's [SHA-256 primitive](https://firebase.google.com/docs/reference/rules/rules.hashing#sha256).
The [hexadecimal conversion](https://firebase.google.com/docs/reference/rules/rules.Bytes#toHexString)
is normalized to lowercase to match the clients' canonical encoding.

Each mutation is one transaction with exactly three coupled documents:

1. Update the conversation's `lastChangeSeq` by one and set
   `lastChange: {id: changeId, seq: lastChangeSeq}`. For creation only, also
   increment `lastSeq` and set `lastMessage: {id: messageId, seq: lastSeq, at}`.
2. Create/update the message and its latest change identity.
3. Create the immutable change receipt. Its ID must not already exist.

Rules use `getAfter` to require the other writes. Neither a cursor nor a receipt
can be committed without its matching message mutation. Edits/recalls preserve
`lastSeq`, so they do not increase unread counts. `lastMessage` is an identity
pointer, not a cached plaintext preview; clients resolve the current message.

Reconnect listens to and pages through `changes where seq > lastAppliedChangeSeq
orderBy seq`, not just new messages. A client fetches the latest referenced
message, upserts by message ID using the newest `changeSeq`, and commits the local
projection and cursor together. Advance only through contiguous change sequences.
Overlapping pages/listener deliveries are harmless. A crash before this local
commit replays the same receipt. Keep receipts without pruning until a separately
validated snapshot/retention protocol exists. Persisting message text while only
persisting an in-memory cursor is not sufficient.

A retry of an acknowledged mutation reads its existing receipt and verifies
message ID, operation, author UID and the intended text digest. For creation it
also compares the message's immutable author (including Bot identity), kind,
mentions and reply target against the intended payload. Later edits/recalls leave
these fields unchanged. A duplicate-ID error alone is not proof of a successful
send with the intended content. Reusing an ID for different content must produce
a conflict. Four-client implementations must test edits/recalls to
messages older than the reconnect cursor.

## Bot isolation and authenticated triggers (T3)

Separate Harness session IDs do not isolate memory. Both automatic memory and
session-memory retrieval use the configuration home. Each private Bot
conversation therefore has its own home, workspace, transcripts, automatic memory
and session-memory namespace, scoped by `(teamId, conversationId, membership
generation)`. Group conversations also use a conversation-specific namespace.
No team-wide writable home is shared between private conversations.

Bot persona/skills are copied as immutable inputs into the selected scope. Any
explicit shared team knowledge is a separate, owner-controlled source whose
visibility is disclosed; private conversation memory is never promoted to it
automatically. A scope cannot read sibling/owner homes, including through Bash.
Force sandboxing, disable bypass, and keep owner MCP connections disabled until
explicitly granted for that scope. Apply turn/token/duration limits per scope.

`mentions` is display metadata and cannot execute a Bot. T3 adds an immutable
authenticated trigger for each target Bot, bound to the original human message,
its author UID, membership generation, team/conversation and target Bot. The
creation rule/backend validates the authenticated human against that message and
the target Bot's allowlist. Bot replies, status messages, edits and owner-visible
observations do not automatically become new human triggers.

Before execution the host rechecks that the trigger remains valid, the human is
still in the same membership generation, the message is not recalled, and the
target Bot currently permits the human. Route tool approval to the owner and
questions to the original human. Never let another Bot approve a tool.

Use durable trigger receipts with states/lease, deterministic invocation and reply
IDs, and a persisted per-conversation cursor. A cursor alone cannot provide
exactly-once effects across a crash between execution and checkpointing. Tools
must support idempotency before automatic replay of effects; otherwise an
ambiguous interrupted execution requires owner recovery instead of blind rerun.
Validate crashes before claim, after execution, after reply and before cursor
commit, including host restart and overlapping listeners.

## Human provenance, rejoin and delegation (T1/T4)

Keep an Admin-only durable `teams/{tid}/membershipEpochs/{uid}` counter after an
active `members/{uid}` document is deleted. Rejoin increments that counter and
creates a member with the new generation; never reset the generation to one.

Tasks carry an immutable root trigger reference and
`origin: {kind: user, uid, generation}`. A requesting Bot owner's authenticated
UID proves that Bot's authorship, not the claimed human's identity. The trusted
task creation/validation path must follow the root trigger to its authenticated
human message and validate the root, conversation and target. Caller-supplied
`origin.uid` or an arbitrary reference to an unrelated allowed user's message is
not authorization. Bind the trigger's requested target and delegated request to
the task's canonical identity; preserve that proof through the complete chain.

The receiver rechecks the current generation, root authorization, allowed Bot use,
chain length/cycles, rate limits and claim lease transactionally. An old-generation
queued task cannot become valid merely because the same UID rejoins. Same-owner
delegation preserves the same human origin and effective audience. Results and
inputs remain text-only.

## Local App grants (T5)

Grant identity includes team, app, human UID, membership generation, grant revision
and expiry. The host validates all of them before ticket creation and again after
awaited operations. Its authorization mirror must fail closed while stale.

Browser sessions belong to the stable `(team, app, uid, generation)` principal;
request IDs identify individual opens, not the revocation principal. Grant expiry,
revocation, member removal or app unbinding closes every associated stream and
WebSocket, not just the most recently issued ticket. Owner-only RC authorization
is independently retained. Tickets stay single-use and valid for 60 seconds; the
UI discloses the Firestore ticket visibility to the platform.

## Phase gates and validation

T0 covers isolated emulator rule checks, a real custom-token SDK listener in
Electron main, a browser SDK check, and native Firestore dependency/build size
probes using the pinned iOS SDK/Android BoM. A minimal native probe is dependency
feasibility evidence, not a signed full-product or physical-device acceptance.

T1 implements the trusted APIs, conversation ACL construction, durable membership
epochs, invite concurrency and four-client member/deep-link UI. T2 implements
the mutation protocol and offline projection on all clients. T3–T5 implement the
contracts above with their original four-client and real-account acceptance gates.

Only `(default)` references `firestore.default.rules` in `firebase.json`.
`model-catalog` and `push-notifications` retain deny-all rules. Runtime resources,
signed application validation and real second-account acceptance are tracked
separately. Push and Firebase deployment require the user's explicit approval.
