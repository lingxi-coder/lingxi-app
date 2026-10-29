# Reproduce T2 focused checks

Run from this Teams checkout, keep at least 2 GiB free, and do not delete other
worktrees or user data. These are source probes, not signed products or devices.

The T1 native projects stage the current product Teams sources. The Android probe
also stages the actual guarded push route; only its source include list changed.

```sh
npm --prefix apps/firebase run test:teams
node --import ./packages/bridge-client/node_modules/tsx/dist/loader.mjs --test apps/web/test/teams-messages.test.ts apps/web/test/teams-api.test.ts
JAVA_HOME=/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home \
ANDROID_HOME="$HOME/Library/Android/sdk" \
./apps/android/native/gradlew -p acceptance/teams-t1-20261029/probes/android \
  assembleDebug testDebugUnitTest --no-daemon --max-workers=1 --console=plain \
  -Dorg.gradle.jvmargs=-Xmx1024m
```

The iOS project setup and unsigned build command are in the
[T1 native probe instructions](../../teams-t1-20261029/probes/README.md).
The Firebase package checkout remains pinned to 12.19.2. Rebuild creates the
worktree-owned DerivedData; do not point it at a foreign worktree's build output.

```sh
xcrun swiftc -module-cache-path /private/tmp/lingxi-teams-t2-swift-cache \
  apps/ios/native/Sources/Teams/TeamMessageProjection.swift \
  acceptance/teams-t2-20261029/probes/ios/check-messages.swift \
  -o /private/tmp/lingxi-teams-t2-check-messages
/private/tmp/lingxi-teams-t2-check-messages packages/teams/fixtures/messaging.json
xcrun swiftc -module-cache-path /private/tmp/lingxi-teams-t2-swift-cache \
  apps/ios/native/Sources/App/CanonicalDeepLink.swift \
  apps/ios/native/Sources/Account/PushMessagePolicy.swift \
  acceptance/teams-t2-20261029/probes/ios/check-push.swift \
  -o /private/tmp/lingxi-teams-t2-check-push
/private/tmp/lingxi-teams-t2-check-push
```

Real Electron + Web runtime:

```sh
TEAM_RUNTIME_PHASE=messages \
TEAM_SPIKE_COMMAND="node $PWD/apps/firebase/scripts/teams-members-runtime.mjs" \
npm --prefix apps/firebase run test:teams
```

This uses only `demo-lingxi-teams-t0` and localhost. The actual sandboxed Electron
preload/main SDK/renderer IndexedDB probe runs first. The script then prints a
Web URL and its temporary scratch directory. Create `<scratch>/stop` to shut down.
It uses a scratch-local Vite cache, rather than modifying borrowed dependencies.
The synthetic users are `teams-owner@example.test` and `teams-member@example.test`,
with the fixture-only password in that script; never substitute real credentials.

Using independent browser contexts: sign in, create/invite/join a team, open
Conversations and create a DM. Exchange messages, explicitly mark read, then set
the member context offline for at least 600 seconds. Queue a member message and
recall; edit an earlier owner message and send a backlog while that context is
offline. Reconnect, verify stable IDs/ascending seq, latest edit and recall, no
pending intents, and derived unread. Reload the page, reopen the team/conversation,
verify persistence, then open another same-account tab and test read propagation.
Finally remove/rejoin the member, verify the old view stays empty and the fresh
membership uses generation 2. Create/send group and channel messages. Inspect a
390 px viewport for overflow. The recorded manual results are in
`../browser-result.json`; the screenshot contains synthetic messages only.
