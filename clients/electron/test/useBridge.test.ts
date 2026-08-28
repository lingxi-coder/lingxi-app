import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';

import {
  beginProjectCatalogRequest,
  claimSlashTurn,
  clearCancellationRuntime,
  clearSlashTurnClaim,
  displayedSession,
  isLatestOperation,
  isLatestProjectCatalogRequest,
  isRuntimeRemovedState,
  nextOperationId,
  pendingCountAfterResponse,
  pruneRuntimeMaps,
  reconcilePendingCount,
  recoverLatestNavigationFailure,
  removeRuntimeFromMaps,
  resetBridgeRuntimeState,
  shouldApplyBootstrapSnapshot,
  shouldClearPendingPermissions,
  shouldReleaseSlashTurn,
  shouldResetBridgeRuntime,
} from '../src/renderer/bridge/useBridge';
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

test('a display-only slash command releases the turn it pre-claimed', () => {
  const pending = new Map<string, boolean>();
  claimSlashTurn(pending, 's1');

  // /status never starts a turn; its result must hand the composer back.
  assert.equal(shouldReleaseSlashTurn(pending, 's1'), true);
});

test('a slash command that expanded into a turn does NOT release it', () => {
  const pending = new Map<string, boolean>();
  claimSlashTurn(pending, 's1');
  // turn_started proves the command became a real turn.
  clearSlashTurnClaim(pending, 's1');

  // router.rs:938 can still emit a display-only result as a fallback; if that
  // released the turn, the composer would unlock mid-turn.
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
  // the composer permanently read-only after any of those commands.
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
  // engine-forwarded path must keep pre-claiming the composer.
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
    'beginLocalCommand calls beginSlashCommand — this is the composer-bricking regression: a locally '
      + 'handled command would pre-claim `running` and never receive an event that releases it, '
      + 'permanently locking the composer after a bare /model.',
  );
});
