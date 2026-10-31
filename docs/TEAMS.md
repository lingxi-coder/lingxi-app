# Teams implementation contract

Status: the SDK message-cache revision below is a local source change pending release.
Deploy its trusted backend invalidation writes and default-database rules/indexes
before updating clients. Its four-client evidence is recorded separately in
[SDK cache acceptance](../acceptance/teams-firestore-cache-20261030/report.md).

Published baseline: T1/T2 and T3 implementation and scoped acceptance pass. The production
API, message trigger, default rules/index and Teams Web overlay were published
on 2026-10-30. T3's real-account checks pass: an explicit member Bot request gets
a real GPT-6 Luna reply, tool approval goes to its owner, only the original member
answers its question, and host offline-to-online processing executes once.
The official Flare-signed Desktop package, full iOS Simulator product and full
Android Direct Debug product were built and checked. Mobile evidence uses an
owned simulator/AVD; it does not establish physical-device or Store acceptance.
The [current acceptance report](../acceptance/teams-t3-provider-20261030/report.md)
separates source, package, native UI, live provider and publication evidence.
That baseline delivery pushed the two authorized feature branches without
merging or pushing main. The SDK cache revision does not publish either branch. Historical prerequisite reports describe their own snapshots.
This contract refines the approved 2026-10-29 Team plan; each phase requires its
stated four-client checks.

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
| `POST /v1/teams/session` | Empty object; custom token for the authenticated UID, requested by Desktop main for its private SDK sessions |
| `POST /v1/teams` | Strict action object, as below |

Actions: `create {teamId,name}`, `delete {teamId}`, `invite
{teamId,requestId,role,maxUses,ttlSeconds}`, `revokeInvite {teamId,inviteId}`, `join
{code,requestId}`, `leave {teamId,generation}`, `remove {teamId,uid,generation}`,
`role {teamId,uid,generation,role}`. Conversation creation uses the T2 message
endpoint below; the unused, generation-less T1 constructor action was removed.
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

Desktop/Web share `packages/teams/TeamsPanel.tsx`. Membership administration uses
authenticated HTTP, with SDK listeners invalidating its views. Ordinary message
views consume SDK snapshots directly. Web and native clients use their existing
Firebase account. Desktop main owns the account lifecycle, HTTP credentials and
metadata SDK; a bundled, sandboxed Chromium data page owns the persistent message
SDK cache in a project/UID-private partition. Only scoped DTOs cross its private
MessagePort and UI IPC. Provider credentials never enter that page or the UI.

Cached snapshots do not grant fresh access. Account changes and listener
permission failures clear stale views; UID, membership generation and lifecycle
checks reject late responses. T1 member administration requires a network
connection. T2 can display recently loaded SDK-cached messages offline.

Dedicated real Firebase owner/member test users were explicitly authorized and
created on 2026-10-29 without admin claims or verification emails. After separate
explicit authorization, isolated `lingxios.net` browser sessions were saved and
restored without login. Credentials, refresh tokens and IndexedDB storage states
are only in the mode-0700, Git-ignored `output/teams-t1-private/` directory, with
mode-0600 files. `node apps/firebase/scripts/teams-test-accounts.mjs` refreshes and
reuses them; `--create` is only for explicitly authorized initial creation. Preserve
this private directory separately before removing/archiving the worktree. Never
copy its contents into reports, Git, screenshots or logs.

## T2 member conversations and clients

`POST /v1/teams/messages` authenticates the same Firebase account as T1 and
accepts a strict action object. Every action includes `{teamId,generation}`;
conversation actions also include `conversationId`. The current membership and
expected generation are read in the same trusted transaction. RC is unchanged.

| Action | Additional fields / result |
| --- | --- |
| `list` (Bot context) | Optional `after` conversation ID; returns up to 100 authorized conversations, read watermarks and `next` |
| `create` | Stable `conversationId`, human `participantUids`, `kind: dm|group|channel`, nonempty `title`; caller must participate and every participant must be a current member |
| `sync` (Bot context) | `after` change sequence; returns up to 100 contiguous immutable changes, the latest referenced messages, current conversation/read watermarks and `hasMore` in one consistent transaction |
| `mutate` | Stable `messageId`, `changeId`, `operation: create|edit|recall`, `text`, optional create-only `replyTo`; returns the acknowledged change identity |
| `read` | `lastReadSeq`; returns the monotonic confirmed watermark |

T2 writes human text messages. Bot DM execution and authenticated Bot triggers
are covered by T3. Only the author may edit/recall a message. Replies reference an existing
message in the same conversation. Recall permanently clears the server body;
receipts keep the digest rather than the plaintext. The trusted API is the shared
mutation implementation for all four clients, with the same three-document
transaction invariants exercised by the client SDK rules. The message endpoint
accepts at most 64 KiB of JSON; membership endpoints retain their 16 KiB limit.

All four clients listen to authorized conversation documents, the caller's read
marks, and the selected conversation's messages ordered by `seq`. Snapshot
changes update an ephemeral UI projection; metadata-only events reuse existing
rows. Old edits and recalls arrive through the same SDK query, including after
reconnection. Selecting another conversation releases its message listener and
retains only that conversation's in-memory rows.

The Firestore SDK owns durable message caching. Apple and Android use its native
persistent cache. Web explicitly enables `persistentLocalCache` with multi-tab
coordination; Desktop uses the browser SDK in its private Chromium partition.
Cache availability covers previously loaded documents and remains subject to SDK
eviction. `fromCache` is observed with metadata changes. An empty cache does not
prove the server has no messages.

`CommandStorage` (IndexedDB on Web/Desktop) and the native atomic command files
persist only pending HTTP command identities, payloads, claim/attempt metadata
and acknowledgement receipts. Known obsolete history fields are removed from
existing records while exact pending command IDs are retained. No ordinary
message body, conversation replica or message cursor is written there. Bot
execution journals, immutable snapshots and result receipts remain separate.
Unread is `max(0,lastSeq-lastReadSeq)`; marking read is explicit.

The local queue saves a stable intent before transport, including conversation
creation. A response lost after server commit retries the same identity. A
permanent rejection is displayed on the pending intent and can be discarded;
a rejected creation cannot block synchronization of existing conversations.
Before each submission, the client claims the latest durable intent. A 30-second
lease coordinates concurrent windows; a separate durable `attempted` flag records
that submission may have begun. Lease expiry permits an idempotent retry, but
never proves that the earlier request was cancelled. Once attempted, an intent
cannot be discarded until acknowledged or definitively rejected. Unattempted
queued work can be discarded. Submission locks prevent rapid repeated taps from
creating multiple intents; completing a save does not erase a newly typed draft.

There is no application-level fixed ordinary-message polling or HTTP list/sync
read path. The SDK owns reconnect/retry and cache reconciliation. Pending HTTP
commands retain bounded backoff and durable claims, with online/focus or
command-journal changes waking recovery. Bot metadata uses sanitized SDK
invalidation stamps and the next known presence/claim expiry; raw Bot run and
question documents remain denied to clients. Control-plane lease renewal and
execution observation are still required for a running Bot.

Revocation/account changes immediately clear the visible projection and stop
sending, even if command-storage cleanup fails. A fresh rejoin uses a new
generation and a separate command namespace. Native page exit removes listeners
and tasks. Desktop main serializes data-page open/close operations, retains
UID-matched SDK authentication for offline app reopen, and signs it out on an
account reset. UI IPC remains bound to the expected account UID.

`onTeamMessage` is the default-database create trigger
using the existing push identity and installation helpers. It excludes the author,
rechecks team/membership/conversation/recalled state and sender generation, and
claims one durable attempt per message/recipient before FCM. Push is a best-effort
notification; ambiguous attempts are not blindly repeated. Reliable message views come
from Firestore SDK listeners/cache; immutable `changes` receipts preserve mutation
idempotency and Bot execution context. Payloads contain the sender/Bot name and generic new
message text, with no message plaintext or credentials. After the member check,
the final awaited read rechecks the installation UID/revision before sending.
Notifications open `team/inbox` on native clients through the existing guarded
inbox, without requiring a local engine or automatically joining a team.

Local evidence is in `acceptance/teams-t2-20261029/`. The browser's actual offline
period was over ten minutes; message ordering/deduplication, an old edit, an old
recall, queue replay, IndexedDB reload, cross-tab reads and removal/rejoin passed.
This does not establish full native packages, physical-device offline runs or
production FCM delivery. Those historical reports predate the published T3
baseline and the SDK-cache acceptance report linked above.

## Message identity and mutation catch-up (T0/T2)

Conversations have two independent cursors, both initialized to zero:

| Field | Advances for | Purpose |
| --- | --- | --- |
| `lastSeq` | New message | Stable message order and read watermarks |
| `lastChangeSeq` | Create, edit, recall | Complete reconnect catch-up |

Messages retain `seq` for life and carry the latest `changeSeq` and `changeId`.
`changes/{changeId}` is an immutable receipt containing
`{seq, messageId, operation: create|edit|recall, actorUid, generation, at, textDigest}`.
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

Ordinary clients listen directly to the full selected message query. The SDK
maintains its own persistent cache and resume state; there is no application
message cursor or separate body replica. Backend `list`/`sync` endpoints and
immutable change receipts remain available to the trusted Bot execution/context
protocol and its idempotent replay. Receipts are not pruned by this change.

A retry of an acknowledged mutation reads its existing receipt and verifies
message ID, operation, author UID and the intended text digest. For creation it
also compares the message's immutable author (including Bot identity), kind,
mentions and reply target against the intended payload. Later edits/recalls leave
these fields unchanged. A duplicate-ID error alone is not proof of a successful
send with the intended content. Reusing an ID for different content must produce
a conflict. Four-client implementations must test edits/recalls to
messages older than the reconnect cursor.

## Bot isolation and authenticated triggers (T3)

### Current implementation and supported host

Desktop, Web, iOS and Android implement Bot cards, presence/owner/privacy and use
permissions, permitted Bot DM, explicit group/channel Bot targets, run status,
original-human questions and owner manual recovery. Web and Mobile direct owners
to Desktop to bind a local Bot. Current T3 source, package and live acceptance
are recorded below; native distribution remains separate from these checks.

The trusted macOS Desktop main process binds an existing local Bot and discovers
its backend queues. Binding requires trusted host/worker binaries and one
selectable native model route with an available static Credential Broker key.
Teams reuses the existing SDK registry and protocol codecs, including OpenAI
Responses for GPT-6 Luna. The native catalog supplies the resolved protocol,
endpoint, sanitized singleton provider profile and exact credential slot; the
main process checks that configuration is unchanged after asynchronous lookup.
Unsupported platforms or credential routes fail explicitly. Persona/Skill text
and the complete selected route are saved as an immutable snapshot. Old bindings
without that route require an explicit owner rebind; pending saved results can
still retry delivery without model credentials or execution. Owner folders,
MCP connections, plugins, startup helpers,
ordinary Bot memory and ambient credentials are not inherited.

Each invocation uses a fresh `harness-team-host`/`harness-team-worker` process path
with private storage and the original human's memory scope described below.
Firebase credentials stay in the trusted host; only the selected model key crosses
the separate worker pipe. Account UID/epoch checks surround asynchronous work;
logout, account changes, binding changes and shutdown cancel captured host work.

### Binding, audience and public client commands

- A card identifies `(ownerUid, hostId, botId)` and captures the owner's current
  membership generation and binding revision. `use: all|listed` controls who may
  request it; the Bot owner may use their own enabled Bot. Presence is a
  60-second heartbeat hint, not an execution guarantee.
- Binding and policy updates compare an expected revision. An editable audience
  draft keeps the revision with which it was loaded; a later card refresh cannot
  upgrade a stale draft and regrant removed users. Unbinding increments the
  revision and retains a tombstone. Only the owner receives inactive cards and
  their revisions for rebind; disabled cards cannot be selected for new work.
  There are at most 200 card identities per team, including tombstones.
- Trusted conversation creation accepts `bot_dm` and up to eight `botKeys`.
  A Bot DM has one human and one Bot. Group/channel membership includes explicit
  Bot principals. The backend validates current use permission and derives the
  read ACL, including each participating Bot's owner. The platform and Bot-owner
  visibility disclosures remain part of every client.
- Member-facing `/v1/teams/messages` commands add `bots`, `unbindBot`, `botState`,
  `botAnswer` and `recoverBot`. Desktop exposes binding through a separate trusted
  local-Bot operation. `botState` is available only to current conversation readers
  and exposes public status, owner/Bot identity, original-human provenance,
  lease/question identity and an answered flag; it exposes no answers or tool
  traces. An optional `questionId` observes a historical question without changing
  the active question. Claim/start/finish/poll/wait and tool approval are not
  member-facing capabilities.

### Authenticated triggers and one-shot execution

Human message creation accepts up to eight explicit `targets`. A Bot DM derives
its target from its immutable conversation; a group/channel requires a selected,
participating, currently permitted Bot. Plain `@name`, SDK `mentions`, replies by
Bots/status messages and later edits/recalls never create work by themselves.

The human message/change, immutable trigger and Admin-only `executionIntent`
receipt are committed together. A trigger binds the source create, exact text,
human UID/generation, team/conversation and Bot revision. SDK rules cannot create
that authority or read/write execution state, and an HTTP retry cannot upgrade an
ordinary SDK message into an invocation.

`botQueue`/`claimBot` use a server checkpoint and the earliest unresolved trigger
per Bot/conversation. An ahead-of-server cursor is rejected. A 30-second claim can
be reclaimed only before execution starts. Only the transaction changing
`claimed` to `running` returns `startBot execute: true` and the authenticated
execution snapshot. Repeated starts provide no second execution permit. A lost
start response or ambiguous worker interruption requires observation/owner
recovery, never a new worker identity, quota or blind model/tool replay.

The host validates the one-shot snapshot against its exact target/attempt and
checks current owner/human generations, binding permission, unchanged/unrecalled
source and trusted receipt evidence. Invalid work is revoked. The original
five-minute running deadline includes model work, approvals, questions and
shutdown; polling or waiting cannot renew it. Requests older than 24 hours receive
one missed reply instead of executing.

`finishBot` atomically writes a deterministic Bot reply, immutable change,
terminal receipt and checkpoint. Trusted output IDs occupy the reserved `bot_`
namespace; human HTTP and SDK writes cannot preoccupy them. A cursor alone never
asserts exactly-once external effects.

### Owner tool approval and original-human questions

The broker-enabled runtime registers only the scoped Bash tool and the dedicated
original-human question tool. Every Bash invocation, including reads, requires
an explicit decision from that Bot's owner. The same single-use request may be
resolved by the owner's Desktop dialog or the owner's independently approved
Remote Control device when that Bot's RC switch is enabled. Both paths allow only
**Allow once** or **Deny** for the unchanged original input. They cannot create
Always/Auto rules, edit the command, start a chat or answer a human question.
Current owner/account epoch, invocation, binding and RC connection/device grants
are checked again around the wait; a Team role or membership is not approval
authority. The default unbrokered worker still denies tools.

`botQuestion` binds a question identity and exact question-text keys to the
run/attempt and original human UID/membership generation. Other readers can see
the question, but neither the Bot owner nor another member can answer on that
human's behalf. `botAnswer` persists an immutable answer ID and exact answer map;
new answers revalidate the current run, source, binding and recipient. An exact
receipt retry may acknowledge an earlier answered question after the worker has
moved to its next question. An observed `answered: true` is not that receipt:
another device may have supplied a different answer ID/content. Clients retry
the saved answer unchanged and require the backend's exact ID/digest check.
Historical observations do not overwrite active-question UI state. The valid
owner attempt receives answers through the host-only question broker, under the
same original lease and cancellation authority.

### Offline persistence, result delivery and owner recovery

All four clients retain conversation Bot participants, explicit message targets,
question metadata and exact answer/recovery intents across offline operation and
restart. Intents are durably saved before mutating requests; ACK loss does not
mint a new message, answer ID or settlement payload. A new owner recovery first
checks current eligibility; retrying an already saved settlement preserves that
same payload instead of creating a new request from current UI text.

Desktop durably records queue discovery before launch, attempt intent before
claim, a start marker before dispatch, and the verified permit/original deadline.
Its stable journal key cannot change merely because a binding/generation changes.
Saved permits or ambiguous start markers never authorize a restarted worker.
A stale prelaunch index with no start/result can be cleared only after the host's
observational `Unstarted` proof, which performs no model/tool execution.

The exact terminal result is synced before `finishBot`. Offline/429/5xx or lost
finish ACK leaves a result-only retry; that path needs no model key and cannot
claim, start or run tools again. Desktop distinguishes `result_pending` from
recovery and preserves the saved result for delivery. Expired claimed/running or
ambiguous runs can instead be explicitly settled by their owner with
`recoverBot`, without tools; a renewed live lease rejects new recovery. Recovery
is never invoked automatically. Private journal corruption or identity mismatch
fails closed rather than replacing or deleting the invocation record.

### Process containment and bounded authority

The private macOS sandbox grants the selected private scope and required system
readables, denies network/owner resources and excludes inherited native file tools,
owner MCP/plugins/hooks,
settings watchers, startup work and shell expansion. Approved Bash uses fixed
shell/environment settings and cannot disable sandboxing or expand grants via
Git metadata. Explicit and automatic background handoff are disabled.

A private per-command shell guardian holds the shell group under a worker-owned
stdin lease. Worker death/EOF, cancellation, deadline and normal command return
trigger group cleanup. The guardian pins the group with an unreaped leader,
kills/verifies no live members remain, and only then permits a successful tool
result. The sandbox denies admitted detach mechanisms (`setsid`, `setpgid` and
`posix_spawn`), while ordinary fork/exec remain available. Cleanup and reaping
are bounded; OS/scheduler/filesystem stalls prevent a hard real-time guarantee.

Input/text/frame bounds, finite Agent turns, output-token caps and shared limits
on physical model dispatches bound the admitted route without resetting authority
on retries or forks. They are not a total-input-token, monetary, CPU, memory or disk ceiling.
Other execution-host platforms and unsupported authentication routes remain
separate scopes. The existing admitted static protocol routes have source/codec
coverage; real-network acceptance here covers the selected OpenAI route.

Runtime contracts live in the companion checkout:
[private profile](../../harness-runtime/docs/TEAM_INVOCATION_PROFILE.md),
[worker/guardian process](../../harness-runtime/docs/TEAM_INVOCATION_PROCESS.md)
and [trusted connector](../../harness-runtime/docs/TEAM_CONNECTOR.md). Current
source hashes, resolved product pins, commands/results and acceptance artifacts
are recorded in the [source and feature report](../acceptance/teams-t3-complete-20261029/report.md),
[initial production publication](../acceptance/teams-t3-live-20261030/report.md)
and [current provider/native acceptance](../acceptance/teams-t3-provider-20261030/report.md).
The latter uses product source `ea59210a` and runtime `b09033fa`: the configured
OpenAI GPT-6 Luna route passed a real Bot DM and a separate request queued while
the host was offline. Reopening the same private Desktop profile processed the
new request once while preserving the old final reply and marker. The original
member answered through the full iOS app; the owner used native AllowOnce and
had no question-answer controls. The full Android Direct Debug app passed its
1,276 unit tests, normal member login, restart restoration and live history checks.

The full official signed Desktop smoke passed in a separate guarded run after
its initial active-Broker refusal. Original app, Broker and URL handler were
restored with zero errors; normal UI verified the same cloud identity and
configured OpenAI credential. The test Bot was unbound after completion, so new
requests/answers are disabled while history remains. The two feature branches
are published; this delivery does not merge, push or modify main. Mobile debug
builds and simulator/AVD UI do not establish physical-device, Store or push-delivery
acceptance. The same-user Broker account slot is shared across user-data
folders; changing HOME is not account-session isolation.

The `(botKey ASC, seq ASC)` trigger index is declared only for Firestore
`(default)` in `infra/firebase/firestore.teams.indexes.json`; named databases keep
their existing configuration. Publishing rules/index/API changes requires the
user's explicit authorization.

For a gcloud deployment of the Firebase message trigger, explicitly set
`FUNCTION_SIGNATURE_TYPE=cloudevent`. Legacy `event` unmarshalling loses the
envelope expected by the SDK wrapper and can acknowledge an event before any
database read. Verify actual Admin-only message receipts after deployment; an
HTTP 200 or ACTIVE function alone is insufficient. The published Hosting overlay
preserves all 19,265 prior file hashes and adds only the reviewed Teams assets and
routes. Current deployment identifiers and the minimal IAM audit are in the
initial publication record; the current provider report records the refreshed
Teams entry and asset version.

### Historical prerequisite evidence

These reports preserve the source snapshots, commands, test counts and limits of
the earlier prerequisites. Their statements about disabled entry points,
all-tools-denied workers or missing client/broker integration describe those
snapshots, not the current local implementation above. They do not establish
current T3 completion or publication.

| Historical checkpoint | Recorded evidence |
| --- | --- |
| Trusted backend | [Backend report](../acceptance/teams-t3-20261029/report.md) |
| Shared output quota | [Runtime prerequisite report](../acceptance/teams-t3-runtime-20261029/report.md) |
| Private principal profile | [Profile report](../acceptance/teams-t3-profile-20261029/report.md) |
| Fresh dedicated worker | [Process report](../acceptance/teams-t3-process-20261029/report.md) |
| Authenticated queue connector | [Connector report](../acceptance/teams-t3-connector-20261029/report.md) |

### Required isolated runtime

Separate Harness session IDs do not isolate memory. Both automatic memory and
session-memory retrieval use the configuration home. Each private Bot
conversation therefore has its own home, workspace, transcripts, automatic memory
and session-memory namespace, scoped by `(ownerUid, botKey, teamId,
conversationId, originHumanUid, membership generation)`. Generations are per UID;
Alice generation 1 and Bob generation 1 are not the same principal. Group
conversations also use a conversation-specific, human-generation-scoped namespace.
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
