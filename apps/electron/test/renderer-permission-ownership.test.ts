import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';
import * as runtime from '../src/renderer/bridge/bridgeRuntimeState';
import * as conversation from '../src/renderer/bridge/conversation';
import * as desktop from '../src/renderer/bridge/desktopState';
import * as center from '../src/renderer/bridge/runtimeCenterState';
import * as tracked from '../src/renderer/bridge/trackedTurns';
import { isPermissionRequestGone, messageFrom } from '../src/renderer/bridge/bridgeConnection';
import type { HostPermissionRequest } from '../src/shared/permission';

const file = ts.createSourceFile('useBridge.ts', readFileSync(resolve('src/renderer/bridge/useBridge.ts'), 'utf8'),
  ts.ScriptTarget.Latest, true, ts.ScriptKind.TS);
function find(predicate: (node: ts.Node) => boolean): any {
  let found: ts.Node | undefined;
  const visit = (node: ts.Node): void => { if (!found && predicate(node)) found = node; ts.forEachChild(node, visit); };
  visit(file); assert.ok(found, 'production callback missing'); return found;
}
function declaration(name: string): string {
  return find((node) => ts.isVariableDeclaration(node) && node.name.getText(file) === name).initializer.getText(file);
}
function evaluate(code: string, context: any): void {
  runInNewContext(ts.transpileModule(code, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS,
  } }).outputText, context);
}
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (cause: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
function permission(request_id: number, backgroundOwned?: boolean): HostPermissionRequest {
  return { request_id, kind: { type: 'tool_use_confirm', tool_name: 'Read', tool_input_json: '{}' },
    ...(backgroundOwned === undefined ? {} : { backgroundOwned }) } as HostPermissionRequest;
}
function question(request_id: number) { return { request_id, questions: [] }; }
function computer(request_id: number) {
  return { request_id, reason: 'fixture', apps: [], tier: 'read', clipboard_read: false,
    clipboard_write: false, system_key_combos: false };
}
const computerResponse = { granted_apps: [], clipboard_read: false, clipboard_write: false, system_key_combos: false };

/** Production hook callbacks/reducers; only IPC and React state scheduling are fake. */
function harness() {
  const states = new Map([['A', runtime.emptyRuntimeState({ status: 'connected' })],
    ['B', runtime.emptyRuntimeState({ status: 'connected' })]]);
  const calls: Array<{ action: string; sessionId: string; requestId?: number }> = [], visibleErrors: string[] = [];
  const context: any = { ...runtime, ...conversation, ...desktop, ...center, ...tracked,
    useCallback: (callback: any) => callback, isPermissionRequestGone, messageFrom,
    runtimeStatesRef: { current: states }, activeSessionIdRef: { current: 'A' }, sessionLoadingRef: { current: false },
    removedRuntimeIds: { current: new Set() }, bootstrapRef: { current: null }, pendingSessionRef: { current: null },
    sharedDesktopCacheRef: { current: new Map() }, pendingFusionDispatches: { current: new Map() },
    sideQuestionTurns: { current: new Map() }, turnActiveRefs: { current: new Map() }, engineTurnActiveRefs: { current: new Map() },
    slashPendingRefs: { current: new Map() }, cancellingRefs: { current: new Map() }, cancellationTasks: { current: new Map() },
    activeTrackedTurns: { current: new Map() }, pendingTrackedTurns: { current: new Map() }, trackedSpeechListeners: { current: new Map() },
    pendingConfigurationOperations: { current: new Map() }, interruptConfigurationOperations: () => {},
    updateRuntime: (sessionId: string, update: (state: runtime.RuntimeState) => runtime.RuntimeState) => {
      states.set(sessionId, update(states.get(sessionId) ?? runtime.emptyRuntimeState()));
    },
    setRuntimeStates: (update: any) => {
      const next = update(states);
      if (next === states) return;
      states.clear(); for (const [sessionId, state] of next) states.set(sessionId, state);
    },
    setBootstrap: () => {}, scheduleProjectCatalogRefresh: () => {}, setError: (message: string) => visibleErrors.push(message),
    capture: (cause: Error) => { visibleErrors.push(cause.message); throw cause; },
    host: {
      command: async () => {},
      approve: async (sessionId: string, requestId: number) => { calls.push({ action: 'approve', sessionId, requestId }); },
      deny: async (sessionId: string, requestId: number) => { calls.push({ action: 'deny', sessionId, requestId }); },
      cancel: async (sessionId: string) => { calls.push({ action: 'cancel', sessionId }); },
      answerAskUserQuestion: async (sessionId: string, requestId: number) => { calls.push({ action: 'answerAskUserQuestion', sessionId, requestId }); },
      cancelAskUserQuestion: async (sessionId: string, requestId: number) => { calls.push({ action: 'cancelAskUserQuestion', sessionId, requestId }); },
      approveComputerAccess: async (sessionId: string, requestId: number) => { calls.push({ action: 'approveComputerAccess', sessionId, requestId }); },
      denyComputerAccess: async (sessionId: string, requestId: number) => { calls.push({ action: 'denyComputerAccess', sessionId, requestId }); },
    },
  };
  for (const name of ['completeTrackedSpeech', 'acknowledgeInteraction', 'cancel']) {
    evaluate(`globalThis.${name} = ${declaration(name)};`, context);
  }
  for (const [method, name] of [['onEvent', 'event'], ['onPermission', 'request'], ['onComputerAccess', 'computer'], ['onConnectionStateChanged', 'connection']]) {
    const call = find((node) => ts.isCallExpression(node) && node.expression.getText(file) === `host.${method}`);
    evaluate(`globalThis.${name} = ${call.arguments[0].getText(file)};`, context);
  }
  return { context, states, calls, visibleErrors,
    state: (sessionId = 'A') => states.get(sessionId)!,
    request: (request: HostPermissionRequest, sessionId = 'A') => context.request({ sessionId, event: request }),
    computer: (request: any, sessionId = 'A') => context.computer({ sessionId, event: request }),
    event: (event: any, sessionId = 'A') => context.event({ sessionId, sequence: 1, event }),
    connection: (event: any, sessionId = 'A') => context.connection({ sessionId, event }),
    actions(sessionId = 'A'): Record<string, (requestId: number, response?: any) => Promise<void>> {
      // Each React render closes over its own displayed session and card array.
      context.renderSessionId = sessionId;
      const result: any = {};
      for (const name of ['approve', 'deny', 'answerAskUserQuestion', 'cancelAskUserQuestion', 'approveComputerAccess', 'denyComputerAccess']) {
        evaluate(`globalThis.renderAction = ((activeSessionId, permissionQueue, computerAccessQueue, askUserQuestionQueue) => ${declaration(name)})(renderSessionId, runtimeStatesRef.current.get(renderSessionId).permissionQueue, runtimeStatesRef.current.get(renderSessionId).computerAccessQueue, runtimeStatesRef.current.get(renderSessionId).askUserQuestionQueue);`, context);
        result[name] = context.renderAction;
      }
      return result;
    },
  };
}

test('foreground terminal preserves validated background cards and late foreground resolution cannot decrement them', () => {
  const h = harness(), bg = permission(1, true), fg = permission(2);
  h.event({ type: 'turn_started', turn_id: 7 });
  h.request(bg); h.request(fg);
  h.event({ type: 'turn_ended', outcome: { type: 'end_turn' } });
  assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
  assert.equal(h.state().pendingInteractionsOverride, 1);
  assert.equal(h.context.turnActiveRefs.current.get('A'), false);
  h.event({ type: 'permission_request_resolved', request_id: fg.request_id });
  h.request(fg); // Delayed host resync must not resurrect a settled foreground card.
  assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
  assert.equal(h.state().pendingInteractionsOverride, 1);
  h.event({ type: 'permission_request_resolved', request_id: bg.request_id });
  assert.equal(h.state().permissionQueue.length, 0);
  assert.equal(h.state().pendingInteractionsOverride, 0);
});

test('hidden foreground and background scope resolutions cannot hide the remaining background card', () => {
  const h = harness(), bg = permission(15, true);
  h.event({ type: 'turn_started', turn_id: 7 });
  h.request(bg);
  h.event({ type: 'turn_ended', outcome: { type: 'end_turn' } });
  for (const hidden of [permission(16, false), permission(17, true)]) {
    // The SDK entry resolved while the main process scope query was pending,
    // so this request never reached the renderer's permission channel.
    h.event({ type: 'permission_request_resolved', request_id: hidden.request_id });
    h.request(hidden); // Late resync still cannot revive the resolved entry.
    assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
    assert.equal(h.state().pendingInteractionsOverride, 1);
    assert.equal(runtime.reconcilePendingCount(h.state().pendingInteractionsOverride, 1) ?? 1, 1);
  }
  h.event({ type: 'permission_request_resolved', request_id: bg.request_id });
  assert.equal(h.state().permissionQueue.length, 0);
  assert.equal(h.state().pendingInteractionsOverride, 0);
});

test('an unseen resolution leaves an unset count override unset for authoritative reconciliation', () => {
  const h = harness(), bg = permission(18, true);
  h.request(bg);
  h.event({ type: 'permission_request_resolved', request_id: 19 });
  assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
  assert.equal(h.state().pendingInteractionsOverride, undefined);
  assert.equal(runtime.reconcilePendingCount(h.state().pendingInteractionsOverride, 1), undefined);
});

test('question ACK and SDK resolution are idempotent in both orders while a background permission remains', async () => {
  for (const action of ['answerAskUserQuestion', 'cancelAskUserQuestion']) {
    for (const resolutionFirst of [true, false]) {
      const h = harness(), bg = permission(30, true), request = question(31), ack = deferred<void>();
      h.request(bg); h.event({ type: 'ask_user_question', request });
      h.state().pendingInteractionsOverride = 2; h.state().pendingAskUserQuestionsOverride = 1;
      h.context.host[action] = async () => ack.promise;
      const response = h.actions()[action](request.request_id, {});
      if (resolutionFirst) h.event({ type: 'ask_user_question_resolved', request_id: request.request_id });
      ack.resolve(); await response;
      h.event({ type: 'ask_user_question_resolved', request_id: request.request_id });
      h.event({ type: 'ask_user_question_resolved', request_id: request.request_id });
      h.event({ type: 'turn_ended', outcome: { type: 'end_turn' } });
      h.event({ type: 'ask_user_question', request });
      assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
      assert.equal(h.state().askUserQuestionQueue.length, 0);
      assert.equal(h.state().pendingInteractionsOverride, 1);
      assert.equal(h.state().pendingAskUserQuestionsOverride, 0);
    }
  }
});

test('question resolution decrements only a matching card and retains both outstanding count floors', () => {
  const h = harness(), first = question(32), second = question(33);
  h.request(permission(34, true)); h.computer(computer(35));
  h.event({ type: 'ask_user_question', request: first }); h.event({ type: 'ask_user_question', request: second });
  h.state().pendingInteractionsOverride = 0; h.state().pendingAskUserQuestionsOverride = 0;
  h.event({ type: 'ask_user_question_resolved', request_id: 36 });
  assert.equal(h.state().pendingInteractionsOverride, 4);
  assert.equal(h.state().pendingAskUserQuestionsOverride, 2);
  h.event({ type: 'ask_user_question_resolved', request_id: first.request_id });
  h.event({ type: 'ask_user_question_resolved', request_id: first.request_id });
  assert.deepEqual(Array.from(h.state().askUserQuestionQueue), [second]);
  assert.equal(h.state().pendingInteractionsOverride, 3);
  assert.equal(h.state().pendingAskUserQuestionsOverride, 1);
  const unset = harness(); unset.request(permission(37, true));
  unset.event({ type: 'ask_user_question_resolved', request_id: 38 });
  assert.equal(unset.state().pendingInteractionsOverride, undefined);
  assert.equal(unset.state().pendingAskUserQuestionsOverride, undefined);
});

test('question and computer responses follow the rendered session after navigation with colliding IDs', async () => {
  for (const action of ['answerAskUserQuestion', 'cancelAskUserQuestion', 'approveComputerAccess', 'denyComputerAccess']) {
    const h = harness(), isComputer = action.includes('ComputerAccess');
    const a = isComputer ? computer(39) : question(39), b = isComputer ? computer(39) : question(39);
    if (isComputer) { h.computer(a, 'A'); h.computer(b, 'B'); }
    else { h.event({ type: 'ask_user_question', request: a }, 'A'); h.event({ type: 'ask_user_question', request: b }, 'B'); }
    const original = h.actions('A')[action];
    h.context.activeSessionIdRef.current = 'B'; h.context.sessionLoadingRef.current = true;
    await original(39, isComputer ? computerResponse : {});
    assert.deepEqual(h.calls, [{ action, sessionId: 'A', requestId: 39 }]);
    const queue = isComputer ? 'computerAccessQueue' : 'askUserQuestionQueue';
    assert.equal(h.state('A')[queue].length, 0);
    assert.deepEqual(Array.from(h.state('B')[queue]), [b]);
  }
});

test('stale question/computer callbacks and late ACKs cannot clear replacement requests with reused IDs', async () => {
  for (const action of ['answerAskUserQuestion', 'cancelAskUserQuestion', 'approveComputerAccess', 'denyComputerAccess']) {
    const h = harness(), isComputer = action.includes('ComputerAccess');
    const make = () => isComputer ? computer(40) : question(40);
    const publish = (request: any) => isComputer ? h.computer(request) : h.event({ type: 'ask_user_question', request });
    publish(make()); const original = h.actions()[action]; publish(make());
    await original(40, isComputer ? computerResponse : {});
    assert.equal(h.calls.length, 0);
    const ack = deferred<void>(); h.context.host[action] = async () => ack.promise;
    const pending = h.actions()[action](40, isComputer ? computerResponse : {});
    const replacement = make(); publish(replacement); ack.resolve(); await pending;
    assert.deepEqual(Array.from(h.state()[isComputer ? 'computerAccessQueue' : 'askUserQuestionQueue']), [replacement]);
  }
});

test('question/computer response failures stay with their original card and cannot revive a removed runtime', async () => {
  for (const action of ['answerAskUserQuestion', 'cancelAskUserQuestion', 'approveComputerAccess', 'denyComputerAccess']) {
    for (const removed of [false, true]) {
      const h = harness(), isComputer = action.includes('ComputerAccess'), ack = deferred<void>();
      if (isComputer) h.computer(computer(41)); else h.event({ type: 'ask_user_question', request: question(41) });
      h.context.host[action] = async () => ack.promise;
      const pending = h.actions()[action](41, isComputer ? computerResponse : {});
      h.context.activeSessionIdRef.current = 'B';
      if (removed) h.connection({ status: 'disconnected', reason: 'session runtime disposed' });
      ack.reject(new Error('fake response failure')); await assert.rejects(pending, /fake response failure/);
      assert.deepEqual(h.visibleErrors, []); assert.equal(h.state('B').error, undefined);
      if (removed) assert.equal(h.states.has('A'), false);
      else assert.equal(h.state('A').error, 'fake response failure');
    }
  }
});

test('foreground terminal and optimistic Stop retain settled-question tombstones without hiding background cards', async () => {
  for (const stop of [false, true]) {
    const h = harness(), bg = permission(42, true), request = question(43);
    h.request(bg); h.event({ type: 'ask_user_question', request });
    h.context.turnActiveRefs.current.set('A', true);
    if (stop) await h.context.cancel(); else h.event({ type: 'turn_ended', outcome: { type: 'end_turn' } });
    h.event({ type: 'ask_user_question_resolved', request_id: request.request_id });
    h.event({ type: 'ask_user_question', request });
    assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
    assert.equal(h.state().askUserQuestionQueue.length, 0);
    assert.equal(h.state().pendingInteractionsOverride, 1);
    assert.equal(h.state().pendingAskUserQuestionsOverride, 0);
  }
});

test('permission resolution count stays above every remaining verified interaction card', () => {
  const h = harness(), first = permission(20, true), second = permission(21, true);
  h.request(first); h.request(second);
  h.state().computerAccessQueue = [{ request_id: 22 }] as any;
  h.state().askUserQuestionQueue = [{ request_id: 23 }] as any;
  // A stale local override must not conceal cards from any interaction queue.
  h.state().pendingInteractionsOverride = 0;
  h.event({ type: 'permission_request_resolved', request_id: 24 });
  assert.equal(h.state().pendingInteractionsOverride, 4);
  h.event({ type: 'permission_request_resolved', request_id: first.request_id });
  assert.deepEqual(Array.from(h.state().permissionQueue), [second]);
  assert.equal(h.state().pendingInteractionsOverride, 3);
  assert.equal(h.state().computerAccessQueue.length, 1);
  assert.equal(h.state().askUserQuestionQueue.length, 1);
});

test('background permission arriving after foreground terminal is actionable while the foreground is idle', async () => {
  const h = harness();
  h.event({ type: 'turn_ended', outcome: { type: 'end_turn' } });
  const bg = permission(3, true); h.request(bg);
  assert.equal(h.state().pendingInteractionsOverride, 1);
  await h.actions().approve(bg.request_id);
  assert.deepEqual(h.calls, [{ action: 'approve', sessionId: 'A', requestId: 3 }]);
  assert.equal(h.state().permissionQueue.length, 0);
});

test('optimistic foreground cancellation preserves only host validated background cards', async () => {
  const h = harness(), bg = permission(4, true), fg = permission(5, false);
  h.context.turnActiveRefs.current.set('A', true);
  h.request(bg); h.request(fg);
  await h.context.cancel();
  assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
  assert.equal(h.state().pendingInteractionsOverride, 1);
  h.event({ type: 'permission_request_resolved', request_id: fg.request_id });
  h.event({ type: 'turn_ended', outcome: { type: 'interrupted' } });
  assert.deepEqual(Array.from(h.state().permissionQueue), [bg]);
  assert.equal(h.state().pendingInteractionsOverride, 1);
});

test('permission ACK and SDK resolution are idempotent in either order, including retained-card resync', async () => {
  for (const resolutionFirst of [true, false]) {
    const h = harness(), first = permission(6, true), second = permission(7, true), ack = deferred<void>();
    h.request(first); h.request(second);
    h.event({ type: 'turn_ended', outcome: { type: 'end_turn' } });
    h.context.host.approve = async () => ack.promise;
    const approval = h.actions().approve(first.request_id);
    if (resolutionFirst) h.event({ type: 'permission_request_resolved', request_id: first.request_id });
    ack.resolve(); await approval;
    h.event({ type: 'permission_request_resolved', request_id: first.request_id });
    h.request(first);
    assert.deepEqual(Array.from(h.state().permissionQueue), [second]);
    assert.equal(h.state().pendingInteractionsOverride, 1);
  }
});

test('retained permission callbacks follow their original session after navigation, even with the same id in B', async () => {
  for (const action of ['approve', 'deny'] as const) {
    const h = harness(), a = permission(8, true), b = permission(8, true);
    h.request(a, 'A'); h.request(b, 'B');
    const original = h.actions('A')[action];
    h.context.activeSessionIdRef.current = 'B';
    h.context.sessionLoadingRef.current = true;
    await original(a.request_id);
    assert.deepEqual(h.calls, [{ action, sessionId: 'A', requestId: 8 }]);
    assert.equal(h.state('A').permissionQueue.length, 0);
    assert.deepEqual(Array.from(h.state('B').permissionQueue), [b]);
  }
});

test('a stale callback or ACK cannot act on a replacement request reusing the old id', async () => {
  const h = harness(), old = permission(9, true), replacement = permission(9, true);
  h.request(old); const original = h.actions();
  h.request(replacement);
  await original.approve(old.request_id);
  assert.equal(h.calls.length, 0);
  const ack = deferred<void>(); h.context.host.approve = async () => ack.promise;
  const approval = h.actions().approve(replacement.request_id);
  const recreated = permission(9, true); h.request(recreated);
  ack.resolve(); await approval;
  assert.deepEqual(Array.from(h.state().permissionQueue), [recreated]);
});

test('late permission failure belongs to A and cannot put its error onto newly selected B', async () => {
  const h = harness(), request = permission(10, true), ack = deferred<void>();
  h.request(request); h.context.host.deny = async () => ack.promise;
  const pending = h.actions().deny(request.request_id);
  h.context.activeSessionIdRef.current = 'B';
  ack.reject(new Error('fake permission failure'));
  await assert.rejects(pending, /fake permission failure/);
  assert.equal(h.state('A').error, 'fake permission failure');
  assert.equal(h.state('B').error, undefined);
  assert.deepEqual(h.visibleErrors, []);
  assert.deepEqual(Array.from(h.state('A').permissionQueue), [request]);
});

test('late permission failure cannot recreate a runtime removed during its response', async () => {
  const h = harness(), request = permission(14, true), ack = deferred<void>();
  h.request(request); h.context.host.deny = async () => ack.promise;
  const pending = h.actions().deny(request.request_id);
  h.context.activeSessionIdRef.current = 'B';
  h.connection({ status: 'disconnected', reason: 'session runtime disposed' });
  assert.equal(h.states.has('A'), false);
  ack.reject(new Error('fake retired runtime failure'));
  await assert.rejects(pending, /retired runtime failure/);
  assert.equal(h.states.has('A'), false);
  assert.deepEqual(h.visibleErrors, []);
});

test('session and connection teardown clear every background card and its tombstones', () => {
  for (const state of [{ status: 'spawning' }, { status: 'disconnected' }, { status: 'error', message: 'fake disconnect' }, { status: 'idle' }]) {
    const h = harness(); h.request(permission(11, true)); h.state().resolvedPermissionIds.add(12);
    h.connection(state);
    assert.equal(h.state().permissionQueue.length, 0);
    assert.equal(h.state().resolvedPermissionIds.size, 0);
  }
  const h = harness(); h.request(permission(13, true));
  h.event({ type: 'session_ended' });
  assert.equal(h.state().permissionQueue.length, 0);
  assert.equal(h.state().pendingInteractionsOverride, 0);
});
