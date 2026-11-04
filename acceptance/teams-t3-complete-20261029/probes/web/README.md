# Shared Web UI acceptance harness

This harness mounts the Web page's own workspace view,
`apps/web/src/teams/TeamsWorkspaceView.tsx` (what `TeamsPage` renders after sign-in),
and through it the shared `packages/teams` workspace: `useTeamsWorkspace`,
`TeamSidebar` (team switcher and the Channels / Direct messages / Bot DMs lists with
their New channel, New message and Message a Bot dialogs), `TeamConversationView`
with its `Composer`, `TeamDetailsDrawer` (Members, Invites and Bots tabs, with
`BotDirectory`), `BotRunCards`, `TeamMessaging`, IndexedDB storage and CSS. It uses the
production `apps/web/src/teams/api.ts` adapter, including its real authenticated HTTP
requests and Firestore SDK listeners. It contains no copied Teams UI, mock command
replies, synthetic client messages, approval API or executor capability.

The page uses synthetic emulator accounts with Firebase Auth in-memory persistence.
Product message projections still use their real IndexedDB storage. The harness
never loads the production Web Firebase configuration or a real account.

## Start

From the repository root, the `lingxi-app` checkout this directory is in:

```sh
sh acceptance/teams-t3-complete-20261029/probes/web/start.sh
```

It reuses the existing Web Vite configuration and installed dependencies, serves
only on `http://127.0.0.1:18784`, and fails if that port is occupied. It does not
build product packages or install dependencies. `/v1/teams` and `/_acceptance` are
proxied to the parent acceptance fixture on `http://127.0.0.1:18783`.

The parent fixture and emulators must already be running:

- Firebase project `demo-lingxi-teams-t0`.
- Auth `127.0.0.1:9098`; Firestore `(default)` `127.0.0.1:8085` with real Team rules.
- Persistent fixture HTTP server `127.0.0.1:18783`, allowing Origin
  `http://127.0.0.1:18784`.
- `GET /_acceptance/bootstrap?uid=web-human` returns `{customToken,detail,conversationId}`.
  `web-observer` returns the same seeded group conversation with its own verified
  membership and custom token. The fixture controls the current team ID/generation.
- Complete real `/v1/teams` list/detail/session/messages routes.
- Real live Bot card `T3 acceptance Bot` plus enabled/permitted
  `T3 offline acceptance Bot` on a deliberately unstarted host.
- Seeded group `Web live Bot question` includes both Web humans and the live Bot.
  The worker asks `Which acceptance result should I use?`; the journey answers
  `Current` only through the original human's actual UI.
- `GET /_acceptance/state?uid=web-human` returns only synthetic read-scoped public
  counts/status: `{teamId,botKey,conversationId,triggerCount,modelRequestCount,
  ownerApprovalCount,conversations:[{id,title,triggerCount,messageCount,...}]}`.

Bootstrapping lazily starts the seeded live question with a five-minute deadline.
Start the journey soon after opening a probe URL; do not spend the deadline waiting
on package builds. Use a fresh parent fixture run if a previous seeded question
already expired or was answered. The harness does not delete or reseed data itself.

Manual entry points:

- [Original human](http://127.0.0.1:18784/?uid=web-human)
- [Observer](http://127.0.0.1:18784/?uid=web-observer)

## Run the prepared Playwright journey

The runner reuses the already bundled Codex Node Playwright installation, without
adding an npm dependency. Python Playwright is not required. Source-only readiness:

```sh
node acceptance/teams-t3-complete-20261029/probes/web/run-journey.mjs --check
```

Actual Chromium run, authorized/executed by the parent acceptance lane:

```sh
TEAMS_T3_OUTPUT_DIR=/private/tmp/teams-t3-web-acceptance/output/playwright \
  node acceptance/teams-t3-complete-20261029/probes/web/run-journey.mjs
```

Add `--headed` for visual inspection. If the bundled browser is unavailable, use
the installed browser explicitly rather than downloading one during another build:

```sh
CHROME_EXECUTABLE='/Applications/Google Chrome.app/Contents/MacOS/Google Chrome' \
TEAMS_T3_OUTPUT_DIR=/private/tmp/teams-t3-web-acceptance/output/playwright \
  node acceptance/teams-t3-complete-20261029/probes/web/run-journey.mjs --headed
```

`TEAMS_PLAYWRIGHT_MODULE` may point to another existing Playwright installation.
The default is `$HOME/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright`.

Alternatively the Playwright MCP `browser_run_code_unsafe` tool can load the
self-contained `journey.js` by its absolute filename. It receives `page` and
creates a second isolated observer browser context. The file is an async function
expression, intentionally usable by both the existing runtime and that tool.
Direct `journey.js` execution covers the eight UI gates below. Use
`run-journey.mjs` for the additional backend final-reply gate; an answer receipt
alone does not establish successful model completion.

The journey drives the workspace through its roles, labels and `data-*` attributes
(`nav` "Teams", the `tw-switcher` menu of radio items, `li.tw-row[data-kind]` rows
under the three section headings, `dialog` "Message a Bot" / "New message", the
"Message" box, "Send" and "Ask a Bot (@)" controls, the "Team details" drawer and
its tabs) and the `.team-bot-question` / `.team-bot-run` cards. A change to those
names in `packages/teams` is a change to this journey.

## Journey and evidence

The journey fails on missing gates; it never substitutes mocked UI state:

1. Observer picks the team in the switcher, opens the seeded conversation, reads
   the real Bot question and has no answer control.
2. Human sees the three conversation lists, and the team's details drawer: Members
   (the account is marked), an Invites tab only for an owner or admin, and the Bots
   tab with Bot cards, owner disclosure and Web's Desktop binding guidance.
3. Original human opens the seeded conversation, answers through the shared UI and
   sees a confirmed receipt.
4. Human starts a chat from the sidebar's Message a Bot dialog with the permitted live
   Bot; it is listed once under Bot DMs, and a UI send creates exactly one real trigger.
5. Human creates a group from the New message dialog with an explicit participating
   Bot; it is listed under Direct messages. Plain `@Bot` text leaves that group's
   backend trigger count at zero.
6. Human picks the Bot with the composer's Ask a Bot (@) control, which shows it as a
   target chip, and sends; the command preserves its Bot key, the real trigger count
   becomes one, and the live question can be answered.
7. Human uses Message Bot on the offline Bot's card in the Bots tab. Real `botStates`
   (the workspace's batch check of the open conversation's runs) reports `queued` and
   the production UI renders `Bot · Queued`.
8. Recorded member requests contain no claim/start/finish/poll/wait/question host
   command, and browser runtime errors fail the run.
9. The runner observes the three successful UI answers (seeded original-human
   question, live Bot DM and explicit group target), then waits up to 60 seconds
   for real `botState: completed` for each trigger. Authenticated `sync` reads must
   contain exactly one unrecalled final Bot `kind: text` message for that trigger,
   with the same Bot key and exact immutable fixture text:
   `Current acceptance result completed once.` Failed/revoked/recovery states,
   duplicate replies, missing replies or changed text fail the run. The offline
   Bot's deliberately queued request remains a separate gate and is not expected
   to complete.

Artifacts are screenshots `01-...png` through `07-...png` plus `result.json`; a
failure writes `failure.png` and a failed `result.json`. The JSON records only
command action/target identifiers, HTTP status and public run status, never ID
tokens, answer content, tool arguments or execution traces. The probe does not
write browser authentication storage exports or traces.
The runner retains the actual UI request's Authorization header only in memory
for the final read commands. `finalReplies` records public trigger/reply identities,
completed state, reply count and the synthetic final message text. It never writes
the token or answer map. If the added completion gate fails after the UI journey
passed, the runner overwrites `result.json` with `passed: false` and the completion
error; the earlier eight checks remain visible and do not become a full pass.

This establishes emulator-backed shared Web UI behavior only after the parent
actually runs and inspects the journey. Desktop owner tool approval/Remote Control,
signed native packages, real provider credentials, public network and production
Firebase deployment remain separate gates.
