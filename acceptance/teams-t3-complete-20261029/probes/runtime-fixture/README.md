# Actual Electron main / Auth / Firestore / Rust worker fixture

This is an explicit demo-only source fixture, not a recorded acceptance result.
It uses the actual `AccountService` (its injected REST fetch maps only the two
Firebase credential service origins to the Auth emulator), `TeamsService`,
`BotStore`, `BotContent`, `TeamPrivateStore`, `TeamConnector`, `runTeamHost`, real
`createTeamsHandler`, real Firebase Auth/Firestore and both rebuilt Rust binaries.
The selected model catalog/key and Anthropic SSE server are synthetic loopback
ports. Only the injected `TeamConnectorOptions.run` wrapper replaces the canonical
backend/model URLs. No production endpoint override was added.

The parent must build the current companion binaries first. This script never
builds, deploys, reads a saved account, opens a browser, starts emulators itself,
or deletes any cache/source/data. It refuses non-macOS hosts, non-demo emulator
addresses, missing binaries or disk space below 2 GiB. Fixture output is retained
under ignored `output/teams-t3-runtime/<run-id>` with private permissions.

Run from the product root, with normal demo emulator orchestration:

```sh
TEAM_SPIKE_COMMAND="node --import '$PWD/packages/bridge-client/node_modules/tsx/dist/loader.mjs' '$PWD/apps/electron/test/teams-host-runtime.integration.ts'" \
  node apps/firebase/scripts/test-teams.mjs
```

The parent serializes this command with every other build/emulator/native gate.
No invocation was performed when preparing this fixture.

After the parent has built and verified the signed Flare app, the same core may
run against its actual shipped binaries. Set the complete app path below and
leave `TEAM_T3_NATIVE` unset, so the fixture exits after core validation:

```sh
env -u TEAM_T3_NATIVE \
  TEAM_T3_PACKAGED_APP='/Users/luolingfeng/lingxi/.worktrees/teams/output/teams-t3-macos/<package-directory>/LingXi Code.app' \
  TEAM_SPIKE_COMMAND="node --import '$PWD/packages/bridge-client/node_modules/tsx/dist/loader.mjs' '$PWD/apps/electron/test/teams-host-runtime.integration.ts'" \
  node apps/firebase/scripts/test-teams.mjs
```

The app's real path must remain strictly beneath that owned macOS output root
and end in `LingXi Code.app`. Only the fixed
`Contents/Resources/bin/harness-team-host` and `harness-team-worker` paths are
accepted; symlink escapes/aliases are rejected. Each binary must pass strict
codesign verification and have Flare Team Identifier `AZ4AX7J833` before launch.
`result.json` records `binaryMode: "signedpackage"` and both `binarySha256`
values, checked again after the core run. Without this variable, the existing
companion `target/debug` paths remain selected and the report says `"debug"`.
Backend, AccountService transport and model fixture boundaries are unchanged.

The initial integration asserts a real three-turn model sequence: one owner-only
approved Bash writes a private workspace file, `AskUserQuestion` is answered only
by the original authenticated human, and one final Bot reply settles. The HTTP
fixture commits the first `finishBot` then returns a synthetic 503, closes the
Node connector, and requires result-only retry from the durable index after
restart even though the backend cursor advanced. That retry must not request a
model credential, execute Bash/model again or duplicate the reply. A second
trigger is sent while the connector is stopped, observed as queued, then consumed
once after restart with the same private host identity.

For parent-controlled native/browser acceptance, retain the same fixture process:

```sh
TEAM_T3_NATIVE=1 \
TEAM_SPIKE_COMMAND="node --import '$PWD/packages/bridge-client/node_modules/tsx/dist/loader.mjs' '$PWD/apps/electron/test/teams-host-runtime.integration.ts'" \
  node apps/firebase/scripts/test-teams.mjs
```

After the initial integration succeeds, the fixture announces readiness and writes
`output/teams-t3-runtime/ready.json`. It listens on `http://127.0.0.1:18783` until
the parent sends SIGTERM/SIGINT; cleanup closes connector processes, SDK sessions,
HTTP servers and Admin SDK without deleting output. Do not expose this synthetic
bootstrap service to a public interface.

The bootstrap protocol is:

```text
GET /_acceptance/bootstrap?uid=ios-human|android-human|web-human|web-observer
-> {customToken,detail,projectId,baseURL,localMessagesURL,conversationId,triggerId,botKey,offlineBotKey}
```

Tokens are demo custom tokens and are never written to reports/logs. `detail` is
the actual backend `TeamDetail`, including the requested UID/current generation.
The complete normal `/v1/teams`, `/v1/teams/:id`, `/v1/teams/session` and
`/v1/teams/messages` APIs use actual emulator ID-token verification. The accepted
browser origin is the prepared Vite harness `http://127.0.0.1:18784`.

The first bootstrap for each human lazily sends its live trigger and waits for a
real worker question, so the five-minute deadline starts after native builds.
The actual question is `Which acceptance result should I use?` with displayed
choices `Current` / `Archived`; exact nonempty answers are preserved. iOS and
Android get private Bot DMs titled `iOS live Bot question` and
`Android live Bot question`. Web gets `Web live Bot question`, an actual group
containing `web-human` and `web-observer`; the observer can read the question but
cannot answer it. Each run uses a new team ID to avoid resetting a native durable
projection namespace.

`T3 acceptance Bot` executes live requests. `T3 offline acceptance Bot` is an
enabled/permitted card on a deliberately unstarted synthetic host, allowing real
queued-state UI checks without a timing race. Sending plain `@Bot` text to a
group does not create a trigger; checked targets use the actual trusted backend.

```text
GET /_acceptance/state?uid=<synthetic-uid>
-> {teamId,botKey,offlineBotKey,conversationId,triggerCount,modelRequestCount,
    ownerApprovalCount,noProcessArguments,
    conversations:[{id,title,triggerCount,messageCount}]}
```

Trigger counts are scoped to the UID's currently readable conversations. State
responses contain no credentials, answers, tool arguments or provider bodies.
The fixture does not make physical-device, signed-product or real-provider claims.
