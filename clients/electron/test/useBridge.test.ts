import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import {
  appendTrackedTurnDelta,
  bindTrackedTurn,
  beginProjectCatalogRequest,
  claimSlashTurn,
  clearCancellationRuntime,
  clearSlashTurnClaim,
  clearTrackedTurnState,
  completeTrackedTurn,
  createDesktopTurnToken,
  dequeueTrackedTurn,
  discardAudioBindings,
  enqueueTrackedTurn,
  pruneAudioBindings,
  sessionAudioBindings,
  displayedSession,
  isLatestOperation,
  isLatestProjectCatalogRequest,
  isPermissionRequestGone,
  isRuntimeRemovedState,
  nextOperationId,
  pendingCountAfterResponse,
  pruneRuntimeMaps,
  reconcilePendingCount,
  recoverLatestNavigationFailure,
  removeRuntimeFromMaps,
  dialogFocusTarget,
  resetBridgeRuntimeState,
  restartBridgePreconditionError,
  restartBridgeSingleFlight,
  restartBridgeWithTimeout,
  shouldApplyBootstrapSnapshot,
  shouldClearPendingPermissions,
  shouldReleaseSlashTurn,
  shouldResetBridgeRuntime,
} from '../src/renderer/bridge/useBridge';
import type { AudioRequestDeps } from '../src/renderer/audio/requests';
import { emptyConversation } from '../src/renderer/bridge/conversation';
import { emptyDesktopState } from '../src/renderer/bridge/desktopState';

function deferred<T>(): {
  promise: Promise<T>;
  resolve(value: T): void;
} {
  let resolvePromise!: (value: T) => void;
  const promise = new Promise<T>((resolve) => { resolvePromise = resolve; });
  return { promise, resolve: resolvePromise };
}

test('bridge spawning requests a full renderer reset', () => {
  assert.equal(shouldResetBridgeRuntime({ status: 'spawning' }), true);
  assert.equal(shouldResetBridgeRuntime({ status: 'connected' }), false);

  const reset = resetBridgeRuntimeState();
  assert.deepEqual(reset.conversation, emptyConversation());
  assert.deepEqual(reset.desktop, emptyDesktopState());
  assert.deepEqual(reset.permissionQueue, []);
  assert.deepEqual(reset.computerAccessQueue, []);
  assert.deepEqual(reset.askUserQuestionQueue, []);
});

test('pending navigation is displayed before the host finishes loading it', () => {
  const persisted = { projectPath: '/projects/old', sessionId: 'old-session' };
  const pending = { projectPath: '/projects/new', sessionId: 'new-session' };

  assert.deepEqual(displayedSession(persisted, pending), pending);
  assert.deepEqual(displayedSession(persisted, null), persisted);
  assert.equal(displayedSession(undefined, null), undefined);
});

test('pending permission ui is cleared across restart and disconnect states', () => {
  assert.equal(shouldClearPendingPermissions({ status: 'spawning' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'idle' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'disconnected', reason: 'socket closed' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'error', message: 'boom' }), true);
  assert.equal(shouldClearPendingPermissions({ status: 'connecting' }), false);
  assert.equal(shouldClearPendingPermissions({ status: 'connected' }), false);
});

test('failed prompt submission releases a cancellation task for the next turn', () => {
  const pending = Promise.resolve();
  const cancelling = { current: true };
  const task = { current: pending as Promise<void> | null };

  clearCancellationRuntime(cancelling, task);

  assert.equal(cancelling.current, false);
  assert.equal(task.current, null);
});

test('authoritative runtime snapshots prune stale per-session renderer state', () => {
  const runtimeStates = new Map([['active', { value: 1 }], ['stale', { value: 2 }]]);
  const turnActive = new Map([['active', true], ['stale', true]]);
  const cancelling = new Map([['active', { current: false }], ['stale', { current: true }]]);

  pruneRuntimeMaps(['active'], runtimeStates, turnActive, cancelling);

  assert.deepEqual([...runtimeStates.keys()], ['active']);
  assert.deepEqual([...turnActive.keys()], ['active']);
  assert.deepEqual([...cancelling.keys()], ['active']);
});

test('runtime disposal removes every per-session renderer collection', () => {
  const runtimeStates = new Map([['removed', { value: 1 }]]);
  const turnActive = new Map([['removed', true]]);
  const cancelling = new Map([['removed', { current: true }]]);

  removeRuntimeFromMaps('removed', runtimeStates, turnActive, cancelling);

  assert.equal(runtimeStates.size, 0);
  assert.equal(turnActive.size, 0);
  assert.equal(cancelling.size, 0);
  assert.equal(isRuntimeRemovedState({ status: 'disconnected', reason: 'session runtime disposed' }), true);
  assert.equal(isRuntimeRemovedState({ status: 'disconnected', reason: 'socket closed' }), false);
});

test('stale bootstrap snapshots are ignored before they can prune renderer state', async () => {
  let latestRevision: number | null = null;
  const removedRuntimeIds = new Set<string>();
  const runtimeStates = new Map([['current', { value: 1 }]]);
  const newer = deferred<{ revision: number; runtimeIds: string[] }>();
  const older = deferred<{ revision: number; runtimeIds: string[] }>();

  const apply = async (snapshotPromise: Promise<{ revision: number; runtimeIds: string[] }>) => {
    const snapshot = await snapshotPromise;
    if (!shouldApplyBootstrapSnapshot(latestRevision, snapshot.revision)) return;
    latestRevision = snapshot.revision;
    removedRuntimeIds.clear();
    pruneRuntimeMaps(snapshot.runtimeIds, runtimeStates);
  };

  const newerApply = apply(newer.promise);
  const olderApply = apply(older.promise);
  newer.resolve({ revision: 8, runtimeIds: ['current'] });
  await newerApply;
  removedRuntimeIds.add('recently-removed');
  runtimeStates.set('recently-opened', { value: 3 });
  older.resolve({ revision: 7, runtimeIds: [] });
  await olderApply;

  assert.equal(latestRevision, 8);
  assert.deepEqual([...runtimeStates.keys()], ['current', 'recently-opened']);
  assert.deepEqual([...removedRuntimeIds], ['recently-removed']);
  assert.equal(shouldApplyBootstrapSnapshot(8, 7), false);
  assert.equal(shouldApplyBootstrapSnapshot(8, 8), false);
});

test('deferred navigation responses only apply the latest operation', async () => {
  let latestOperationId = 0;
  const applied: string[] = [];
  const first = deferred<string>();
  const second = deferred<string>();
  const firstOperationId = nextOperationId(latestOperationId);
  latestOperationId = firstOperationId;
  const secondOperationId = nextOperationId(latestOperationId);
  latestOperationId = secondOperationId;

  const applyResponse = async (operationId: number, response: Promise<string>) => {
    const value = await response;
    if (isLatestOperation(operationId, latestOperationId)) applied.push(value);
  };

  const firstApply = applyResponse(firstOperationId, first.promise);
  const secondApply = applyResponse(secondOperationId, second.promise);
  second.resolve('second project');
  await secondApply;
  first.resolve('first project');
  await firstApply;

  assert.deepEqual(applied, ['second project']);
});

test('latest failed navigation reapplies the authoritative host snapshot', async () => {
  let latestOperationId = 2;
  let activeSession = 'optimistic-b';
  const authoritative = deferred<{ activeSession: string }>();
  const recovery = recoverLatestNavigationFailure(
    2,
    (operationId) => operationId === latestOperationId,
    () => authoritative.promise,
    (snapshot) => { activeSession = snapshot.activeSession; },
  );

  authoritative.resolve({ activeSession: 'completed-a' });
  await recovery;

  assert.equal(activeSession, 'completed-a');
  latestOperationId = 3;
});

test('project catalog request generations reject stale deferred responses', async () => {
  const generations = new Map<string, number>();
  const projectPath = '/projects/current';
  const firstGeneration = beginProjectCatalogRequest(generations, projectPath);
  const secondGeneration = beginProjectCatalogRequest(generations, projectPath);
  const first = deferred<string>();
  const second = deferred<string>();
  const applied: string[] = [];
  const apply = async (generation: number, response: Promise<string>) => {
    const value = await response;
    if (isLatestProjectCatalogRequest(generations, projectPath, generation)) applied.push(value);
  };

  const firstApply = apply(firstGeneration, first.promise);
  const secondApply = apply(secondGeneration, second.promise);
  second.resolve('new');
  await secondApply;
  first.resolve('old');
  await firstApply;

  assert.deepEqual(applied, ['new']);
});

test('interaction response patches win over an old pending summary until it catches up', () => {
  assert.equal(pendingCountAfterResponse(undefined, 2), 1);
  assert.equal(pendingCountAfterResponse(1, 1), 0);
  assert.equal(pendingCountAfterResponse(undefined, 3), 2);
  assert.equal(reconcilePendingCount(0, 1), 0);
  assert.equal(reconcilePendingCount(0, 0), undefined);
  assert.equal(reconcilePendingCount(1, 0), undefined);
});

test('stale permission responses are treated as an already-resolved interaction', () => {
  assert.equal(
    isPermissionRequestGone(new Error('Error invoking remote method: permission request is not pending')),
    true,
  );
  assert.equal(isPermissionRequestGone(new Error('bridge client not connected')), false);
});

test('permission resolution is the renderer queue terminal state', () => {
  const source = useBridgeSource();
  const resolutionBody = sliceBetweenMarkers(
    source,
    "if (event.type === 'permission_request_resolved') {",
    "if (event.type === 'turn_ended' || event.type === 'session_ended') {",
    'permission resolution handling',
  );
  assert.match(resolutionBody, /permissionQueue: state\.permissionQueue\.filter/);
  assert.match(resolutionBody, /resolvedPermissionIds/);

  const requestBody = sliceBetweenMarkers(
    source,
    'const offPermission = host.onPermission',
    'const offComputerAccess = host.onComputerAccess',
    'permission request ingestion',
  );
  assert.match(requestBody, /resolvedPermissionIds\.has/);
});

test('a display-only slash command releases the turn it pre-claimed', () => {
  const pending = new Map<string, boolean>();
  claimSlashTurn(pending, 's1');

  // /status never starts a turn; its result must release the pre-claim.
  assert.equal(shouldReleaseSlashTurn(pending, 's1'), true);
});

test('a slash command that expanded into a turn does NOT release it', () => {
  const pending = new Map<string, boolean>();
  claimSlashTurn(pending, 's1');
  // turn_started proves the command became a real turn.
  clearSlashTurnClaim(pending, 's1');

  // router.rs:938 can still emit a display-only result as a fallback; if that
  // released the turn, running state would clear mid-turn.
  assert.equal(shouldReleaseSlashTurn(pending, 's1'), false);
});

test('a result for a session that never dispatched a slash command releases nothing', () => {
  assert.equal(shouldReleaseSlashTurn(new Map(), 's1'), false);
});

/**
 * Slice the source text between two markers, and refuse to pass silently if
 * either marker can't be found or the slice is suspiciously small. There is
 * no React test harness in this repo (established earlier in this plan), so
 * a hook body like `beginLocalCommand`'s can't be exercised directly — the
 * source text is the only thing this gate can read. A gate whose extraction
 * silently matches nothing would pass forever and protect nothing.
 */
function sliceBetweenMarkers(source: string, startMarker: string, endMarker: string, label: string): string {
  const start = source.indexOf(startMarker);
  const end = start >= 0 ? source.indexOf(endMarker, start) : -1;
  assert.ok(
    start >= 0 && end > start,
    `could not locate ${label} in useBridge.ts via '${startMarker}' .. '${endMarker}'. `
      + 'If this function was renamed or reordered (not a regression), update the markers this test looks for.',
  );
  const body = source.slice(start, end);
  assert.ok(
    body.trim().length > 40,
    `${label} extraction in useBridge.ts produced a suspiciously short body ("${body}"). `
      + 'This gate cannot protect anything until the markers actually bracket the real function.',
  );
  return body;
}

test('beginLocalCommand calls the non-claiming echo, never the engine-claiming one', () => {
  // Regression under test: bridge.beginLocalCommand must call
  // beginLocalSlashCommand, which makes no `running` claim, because a
  // locally-handled command (bare /model, /permissions, /effort, /theme,
  // /config) never receives the slash_command_result/error/turn_ended that
  // would release a claim made by beginSlashCommand. Calling beginSlashCommand
  // here instead — the exact mistake this test exists to catch — would leave
  // the session permanently marked as running after any of those commands.
  const source = readFileSync(join(process.cwd(), 'src/renderer/bridge/useBridge.ts'), 'utf8');

  const runSlashCommandBody = sliceBetweenMarkers(
    source,
    'const runSlashCommand = useCallback',
    'const beginLocalCommand = useCallback',
    'runSlashCommand',
  );
  const beginLocalCommandBody = sliceBetweenMarkers(
    source,
    'const beginLocalCommand = useCallback',
    'const emitCommandOutput = useCallback',
    'beginLocalCommand',
  );

  // The converse, so the gate also catches the opposite mistake: the
  // engine-forwarded path must keep pre-claiming running state.
  assert.match(
    runSlashCommandBody,
    /\bbeginSlashCommand\b/,
    'runSlashCommand no longer calls beginSlashCommand. If this function was renamed or restructured '
      + '(not a regression), update this test; otherwise the engine-forwarded path lost its running-claim.',
  );

  assert.match(
    beginLocalCommandBody,
    /\bbeginLocalSlashCommand\b/,
    'beginLocalCommand no longer calls beginLocalSlashCommand. If this function was renamed (not a '
      + 'regression), update this test; otherwise a locally-handled command no longer echoes its typed line.',
  );
  assert.doesNotMatch(
    beginLocalCommandBody,
    /\bbeginSlashCommand\b/,
    'beginLocalCommand calls beginSlashCommand — this is the stale-running-state regression: a locally '
      + 'handled command would pre-claim `running` and never receive an event that releases it, '
      + 'permanently marking the session as running after a bare /model.',
  );
});

function useBridgeSource(): string {
  return readFileSync(join(process.cwd(), 'src/renderer/bridge/useBridge.ts'), 'utf8');
}

test('configuration admin waits for the terminal event with the same domain and operation id', () => {
  const source = useBridgeSource();
  const eventBody = sliceBetweenMarkers(
    source,
    "if (event.type === 'configuration_operation') {",
    "if (event.type === 'error'",
    'configuration operation event correlation',
  );
  assert.match(eventBody, /event\.status === 'succeeded' \|\| event\.status === 'failed'/);
  assert.match(eventBody, /`\$\{event\.domain\}:\$\{event\.operation_id\}`/);
  assert.match(eventBody, /pendingConfigurationOperations\.current\.get\(key\)/);
  assert.match(eventBody, /pending\.resolve\(event\)/);
  assert.match(eventBody, /pending\.reject\(new Error/);

  const dispatchBody = sliceBetweenMarkers(
    source,
    'const runConfigurationAdmin = useCallback',
    'const skillAdmin = useCallback',
    'configuration admin dispatch waiter',
  );
  assert.match(dispatchBody, /pendingConfigurationOperations\.current\.set\(key, pending\)/);
  assert.match(dispatchBody, /await command\(envelope\)/);
  assert.match(dispatchBody, /return terminal/);
});

test('tracked prompts reserve a token before dispatch and keep sendPrompt as a compatibility wrapper', () => {
  const trackedBody = sliceBetweenMarkers(
    useBridgeSource(),
    'const sendTrackedPrompt = useCallback',
    'const sendPrompt = useCallback',
    'sendTrackedPrompt',
  );

  assert.match(trackedBody, /createDesktopTurnToken/);
  assert.match(trackedBody, /enqueueTrackedTurn\(pendingTrackedTurns\.current, token\)/);
  assert.match(trackedBody, /trackedTurnSequence\.current \+= 1/);
  assert.match(trackedBody, /Failed to queue the pending message/);
  assert.match(trackedBody, /running: wasTurnActive/);

  // `Stage` renders "Not sent" for `delivery: 'failed'` and `conversation.ts`
  // sets and clears `'pending'`, but nothing ever produced `'failed'` — the
  // prompt that could not be queued sat there looking delivered. This is a
  // source guard, not a behavioural one: the branch lives inside a `useCallback`
  // with no seam a unit test can drive.
  //
  // It names the WIRE VALUE, not the local that carries the row. The first
  // version of this guard spelled out `promptItemId`, and a later refactor that
  // kept the behaviour exactly (capturing the item instead of its id, comparing
  // by reference) turned it red — a false alarm on a legitimate change is how a
  // guard teaches people to delete it.
  assert.match(
    trackedBody,
    /delivery: 'failed' as const/,
    "the send-failure branch must still mark the prompt row as not sent",
  );
  assert.match(
    trackedBody,
    /appendPendingUserPrompt\(/,
    'and must still be marking the row that was appended for this prompt',
  );

  const sendPromptBody = sliceBetweenMarkers(
    useBridgeSource(),
    'const sendPrompt = useCallback',
    'const runSlashCommand = useCallback',
    'sendPrompt',
  );
  assert.match(sendPromptBody, /const tracked = sendTrackedPrompt\(text, images, imageNames, filePaths\)/);
  assert.match(sendPromptBody, /await tracked\.queued/);
});

test('tracked turns bind by client_turn_id and otherwise fall back to FIFO', () => {
  const pending = new Map<string, ReturnType<typeof createDesktopTurnToken>[]>();
  const active = new Map<string, { token: ReturnType<typeof createDesktopTurnToken>; text: string; turnId?: number; completed: boolean }>();
  const first = createDesktopTurnToken('s1', 1, 'composer');
  const second = createDesktopTurnToken('s1', 2, 'flow');
  enqueueTrackedTurn(pending, first);
  enqueueTrackedTurn(pending, second);

  const explicit = bindTrackedTurn(
    pending,
    active,
    's1',
    { type: 'turn_started', turn_id: 7, client_turn_id: second.clientTurnId } as any,
  );
  assert.equal(explicit?.token.clientTurnId, second.clientTurnId);
  assert.deepEqual(pending.get('s1')?.map((token) => token.clientTurnId), [first.clientTurnId]);

  const fallback = bindTrackedTurn(pending, active, 's1', { type: 'turn_started', turn_id: 8 } as any);
  assert.equal(fallback?.token.clientTurnId, first.clientTurnId);
  assert.equal(pending.has('s1'), false);
});

test('tracked speech cleanup drops queued tokens and seals the active turn once', () => {
  const pending = new Map<string, ReturnType<typeof createDesktopTurnToken>[]>();
  const active = new Map<string, { token: ReturnType<typeof createDesktopTurnToken>; text: string; turnId?: number; completed: boolean }>();
  const listeners = new Map<string, Set<(event: unknown) => void>>();
  const queued = createDesktopTurnToken('s1', 1, 'composer');
  const bound = createDesktopTurnToken('s1', 2, 'flow');
  enqueueTrackedTurn(pending, queued);
  enqueueTrackedTurn(pending, bound);
  const tracked = bindTrackedTurn(pending, active, 's1', { type: 'turn_started', turn_id: 4 } as any);
  assert.equal(tracked?.token.clientTurnId, queued.clientTurnId);
  appendTrackedTurnDelta(active, 's1', 'hello');
  const completed = completeTrackedTurn(active, 's1');
  assert.equal(completed?.text, 'hello');
  assert.equal(completeTrackedTurn(active, 's1'), null, 'completion must be unique');
  const cleared = clearTrackedTurnState(pending, active, listeners as any, 's1');
  assert.deepEqual(cleared.map((token) => token.clientTurnId), [bound.clientTurnId]);
  dequeueTrackedTurn(pending, bound);
  assert.equal(pending.size, 0);
});

test('a slash release resets the cancellation runtime, not just the turn claims (event fan-out)', () => {
  // Regression under test (FINDING 1): cancel() gates on turnActiveRefs, which
  // this branch sets true even for a display-only command like /status. While
  // that command is in flight Stop is available; pressing it sets
  // cancelling.current/isCancelling. Those are
  // normally cleared only by turn_ended/session_ended, neither of which a
  // display-only command ever produces. Left set: isCancelling stays true (the
  // Stop button renders disabled on the user's NEXT real turn), and
  // cancelling.current stays true (cancel() early-returns forever after --
  // Stop goes silently inert for the rest of the session).
  const source = useBridgeSource();

  const slashResultReleaseBody = sliceBetweenMarkers(
    source,
    "if (event.type === 'slash_command_result' && shouldReleaseSlashTurn",
    "if (event.type === 'error' && shouldReleaseSlashTurn",
    'the slash_command_result release branch',
  );
  assert.match(
    slashResultReleaseBody,
    /\bclearCancellationRuntime\(/,
    'the slash_command_result release no longer calls clearCancellationRuntime -- a display-only command '
      + "leaves cancelling.current stuck true, and cancel() (`turnActiveRefs.current.get(sessionId)` gate) "
      + 'silently stops working for the rest of the session after the first Stop press during a slash command.',
  );
  assert.match(
    slashResultReleaseBody,
    /isCancelling:\s*false/,
    'the slash_command_result release no longer resets runtime isCancelling -- the Stop button renders '
      + 'disabled on the user\'s next real turn.',
  );

  const errorReleaseBody = sliceBetweenMarkers(
    source,
    "if (event.type === 'error' && shouldReleaseSlashTurn",
    'updateRuntime(sessionId, (state) => {\n        let next = { ...state, conversation: reduceEvent',
    'the error release branch',
  );
  assert.match(
    errorReleaseBody,
    /\bclearCancellationRuntime\(/,
    'the error release (dispatch reaches no engine handler / transport failure) no longer calls '
      + 'clearCancellationRuntime -- same stranding as the slash_command_result branch, via the error path.',
  );
  assert.match(
    errorReleaseBody,
    /isCancelling:\s*false/,
    'the error release no longer resets runtime isCancelling.',
  );
});

test('the runSlashCommand dispatch-failure catch resets cancellation runtime; the best-effort refresh-listings catch does not', () => {
  // Same FINDING 1 regression, but on the synchronous dispatch-failure path in
  // runSlashCommand (host.command throws before anything crosses the bridge).
  // The SECOND catch (the best-effort slash-listing refresh, which runs only
  // after dispatch already succeeded and may have started a live turn) must
  // NOT reset cancellation runtime -- doing so would strand a live turn's own
  // in-flight cancellation.
  const source = useBridgeSource();
  const runSlashCommandBody = sliceBetweenMarkers(
    source,
    'const runSlashCommand = useCallback',
    'const beginLocalCommand = useCallback',
    'runSlashCommand',
  );

  // The history-inert /btw branch has its own catch and must not reset the
  // main turn. Locate the ordinary command's dispatch after its turn claim.
  const claimStart = runSlashCommandBody.indexOf('claimSlashTurn(');
  assert.ok(claimStart >= 0, 'could not locate the ordinary slash turn claim');
  const sideQuestionBranch = runSlashCommandBody.slice(0, claimStart);
  assert.match(sideQuestionBranch, /finishSideQuestion/);
  assert.doesNotMatch(sideQuestionBranch, /clearCancellationRuntime|beginSlashCommand/);
  const firstCatchStart = runSlashCommandBody.indexOf('catch (cause) {', claimStart);
  assert.ok(firstCatchStart >= 0, 'could not locate the dispatch-failure catch inside runSlashCommand');
  const firstCatchEnd = runSlashCommandBody.indexOf('return;', firstCatchStart);
  assert.ok(firstCatchEnd > firstCatchStart, 'the dispatch-failure catch no longer ends with a return -- update this test\'s markers');
  const dispatchFailureCatch = runSlashCommandBody.slice(firstCatchStart, firstCatchEnd);

  assert.match(
    dispatchFailureCatch,
    /\bclearCancellationRuntime\(/,
    'runSlashCommand\'s dispatch-failure catch no longer calls clearCancellationRuntime -- a genuine dispatch '
      + 'failure (the command never reached the engine) still leaves cancelling.current stuck if the user had '
      + 'pressed Stop, same stranding as FINDING 1.',
  );
  assert.match(dispatchFailureCatch, /isCancelling:\s*false/, 'the dispatch-failure catch no longer resets isCancelling.');

  const secondCatchStart = runSlashCommandBody.indexOf('catch (cause) {', firstCatchEnd);
  assert.ok(secondCatchStart > firstCatchEnd, 'could not locate the second (refresh-listings) catch inside runSlashCommand');
  const secondCatchEnd = runSlashCommandBody.indexOf('}', secondCatchStart);
  const refreshListingsCatch = runSlashCommandBody.slice(secondCatchStart, secondCatchEnd);
  assert.doesNotMatch(
    refreshListingsCatch,
    /clearCancellationRuntime/,
    'the best-effort refresh-listings catch now calls clearCancellationRuntime -- this runs AFTER dispatch '
      + 'already succeeded (possibly starting a real turn), so resetting cancellation here would strand a '
      + "live turn's own in-flight cancel().",
  );
});

test('opening the composer slash popup refreshes the live slash catalog', () => {
  const bridgeSource = useBridgeSource();
  assert.match(
    bridgeSource,
    /const refreshSlashCommands = useCallback\([\s\S]*?type: 'refresh_listings'[\s\S]*?type: 'slash_commands'/,
    'UseBridge must expose a focused refresh for the live slash-command registry.',
  );

  const composerSource = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  assert.match(
    composerSource,
    /if \(!slashMenuOpen\) return;\s*void bridge\.refreshSlashCommands\(\)\.catch\(\(\) => undefined\)/,
    'opening inline slash completion must recover a catalog missed during startup.',
  );

});

test('a slash-turn claim is cleared on session reset events, not just turn_started (FINDING 2)', () => {
  // Regression under test: the reducer clears pendingSlashName on
  // session_started/session_ended/session_resumed (conversation.ts), but
  // before this fix only turn_started cleared the refs half
  // (slashPendingRefs). A stale claim surviving a session reset can later arm
  // the `error` release branch against an unrelated turn, setting
  // turnActive=false while `running` stays true: Stop renders but cancel()
  // early-returns, so a live turn becomes unstoppable.
  const source = useBridgeSource();
  const sessionResetBody = sliceBetweenMarkers(
    source,
    "if (event.type === 'turn_started') {\n        if (activeTrackedTurns.current.has(sessionId)) completeTrackedSpeech(sessionId, 'stale');",
    "if (event.type === 'slash_command_result' && shouldReleaseSlashTurn",
    'the session-reset slash-claim handling in the event fan-out',
  );
  for (const eventType of ['session_started', 'session_ended', 'session_resumed']) {
    assert.match(
      sessionResetBody,
      new RegExp(`event\\.type === '${eventType}'`),
      `the event fan-out no longer checks for '${eventType}' near where turn_started clears slashPendingRefs -- `
        + 'a session reset can leave a stale slash claim that later mislabels an unrelated error as a slash release.',
    );
  }
  assert.match(
    sessionResetBody,
    /clearSlashTurnClaim\(slashPendingRefs\.current, sessionId\)/,
    'no clearSlashTurnClaim call found alongside the session-reset event check.',
  );
});

test('a connection reset that clears turnActiveRefs also clears the outstanding slash claim (FINDING 2)', () => {
  // Same regression as above, via the OTHER path that clears turnActiveRefs
  // for a non-slash reason: a connection reset (respawn/disconnect/error/idle,
  // shouldClearPendingPermissions) in onConnectionStateChanged.
  const source = useBridgeSource();
  const connectionResetBody = sliceBetweenMarkers(
    source,
    'if (shouldClearPendingPermissions(state)) {\n        turnActiveRefs.current.set(sessionId, false);',
    "if (activeSessionIdRef.current === sessionId && state.status === 'error')",
    'the connection-reset turnActiveRefs clear in onConnectionStateChanged',
  );
  assert.match(
    connectionResetBody,
    /clearSlashTurnClaim\(slashPendingRefs\.current, sessionId\)/,
    'a connection reset clears turnActiveRefs but not slashPendingRefs -- a stale slash claim from before the '
      + 'reset can survive into the next session/turn and mislabel an unrelated error as a slash release.',
  );
});

test('restart helper rejects a hung IPC call within its caller-provided bound', async () => {
  let invoked = false;
  await assert.rejects(
    restartBridgeWithTimeout(async () => {
      invoked = true;
      await new Promise<void>(() => undefined);
    }, 5),
    /Timed out waiting for the engine to restart/,
  );
  assert.equal(invoked, true);
});

test('restart preconditions turn loading, missing host, and missing session into failures', () => {
  assert.match(restartBridgePreconditionError(true, true, 'session')?.message ?? '', /session is loading/);
  assert.match(restartBridgePreconditionError(false, false, 'session')?.message ?? '', /host unavailable/);
  assert.match(restartBridgePreconditionError(false, true, null)?.message ?? '', /Open a session/);
  assert.equal(restartBridgePreconditionError(false, true, 'session'), null);
});

test('restart helper preserves successful and failed IPC results', async () => {
  await assert.doesNotReject(() => restartBridgeWithTimeout(async () => undefined, 25));
  await assert.rejects(
    restartBridgeWithTimeout(async () => { throw new Error('restart failed'); }, 25),
    /restart failed/,
  );
});

test('restart single-flight reuses a pending session operation, then allows a later restart', async () => {
  const inFlight = new Map<string, Promise<void>>();
  const first = deferred<void>();
  let hostCalls = 0;
  const restart = () => restartBridgeSingleFlight(inFlight, 'session-1', async () => {
    hostCalls += 1;
    await first.promise;
  });

  // The first renderer caller times out, but that only ends its wait. The
  // underlying session restart remains in-flight for the retry to join.
  const initial = restartBridgeWithTimeout(restart, 5);
  await assert.rejects(initial, /Timed out waiting for the engine to restart/);
  assert.equal(hostCalls, 1);
  const retry = restartBridgeWithTimeout(restart, 50);
  await Promise.resolve();
  assert.equal(hostCalls, 1);
  first.resolve(undefined);
  await retry;
  assert.equal(inFlight.has('session-1'), false);

  await restartBridgeWithTimeout(restart, 50);
  assert.equal(hostCalls, 2);
});

test('dialog focus helper wraps both directions and captures focus that escaped', () => {
  const focusable = ['first', 'middle', 'last'] as const;
  assert.equal(dialogFocusTarget(focusable, 'last', false), 'first');
  assert.equal(dialogFocusTarget(focusable, 'first', true), 'last');
  assert.equal(dialogFocusTarget(focusable, 'outside', false), 'first');
  assert.equal(dialogFocusTarget(focusable, 'outside', true), 'last');
  assert.equal(dialogFocusTarget(focusable, 'middle', false), undefined);
  assert.equal(dialogFocusTarget([], null, false), undefined);
});

// ---------------------------------------------------------------------------
// The microphone belongs to a SESSION, not to the hook.
//
// `useBridge()` is instantiated once (App.tsx) while `SessionRuntimeManager`
// runs a Map of concurrent runtimes, each with its own engine, its own
// `AudioBridge` and its own registered `voice` tool. One recorder shared across
// all of them is not a tidiness problem: it hands one session's captured audio
// to a different session's model.
// ---------------------------------------------------------------------------

function fakeRecorder(): AudioRequestDeps['recorder'] & { stopped: number } {
  let recording = false;
  return {
    stopped: 0,
    isRecording: () => recording,
    async start() { recording = true; },
    async stop() {
      recording = false;
      (this as { stopped: number }).stopped += 1;
      return { audioBase64: '', mimeType: 'audio/webm' };
    },
  };
}

function fakeBindings(): AudioRequestDeps {
  return {
    recorder: fakeRecorder(),
    async synthesize() { return { pcmBase64: '', sampleRateHz: 0 }; },
    playback: () => ({ voiceSelection: 'system:default', rate: 1 }),
  };
}

test('each session gets its own recorder, so one session cannot answer with another session\'s audio', () => {
  const bindings = new Map<string, AudioRequestDeps>();
  const a = sessionAudioBindings(bindings, 'session-a', fakeBindings);
  const b = sessionAudioBindings(bindings, 'session-b', fakeBindings);

  assert.notEqual(
    a.recorder,
    b.recorder,
    'two concurrent sessions sharing one recorder means B\'s stop_recording finalizes A\'s clip into B\'s transcript',
  );

  void a.recorder.start({ sampleRateHz: 16000, format: 'webm' });
  assert.equal(a.recorder.isRecording(), true);
  assert.equal(
    b.recorder.isRecording(),
    false,
    'a session that never started a capture must answer is_recording with false',
  );

  assert.equal(
    sessionAudioBindings(bindings, 'session-a', fakeBindings),
    a,
    'a start/stop PAIR spans two engine requests, so one session must keep one recorder',
  );
});

test('discarding a session releases a microphone it was still holding', async () => {
  const bindings = new Map<string, AudioRequestDeps>();
  const deps = sessionAudioBindings(bindings, 'session-a', fakeBindings);
  await deps.recorder.start({ sampleRateHz: 16000, format: 'webm' });

  discardAudioBindings(bindings, 'session-a');

  assert.equal(bindings.has('session-a'), false);
  assert.equal(deps.recorder.isRecording(), false, 'nothing else holds this recorder, so the mic would stay open');
});

test('an authoritative runtime snapshot prunes the audio bindings of sessions that are gone', () => {
  const bindings = new Map<string, AudioRequestDeps>();
  sessionAudioBindings(bindings, 'live', fakeBindings);
  sessionAudioBindings(bindings, 'gone', fakeBindings);

  pruneAudioBindings(bindings, ['live']);

  assert.deepEqual([...bindings.keys()], ['live']);
});
