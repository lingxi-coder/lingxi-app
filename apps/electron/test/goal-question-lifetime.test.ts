import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import ts from 'typescript';
import type { AskUserQuestionRequestDto, ClientEvent } from '@lingxi/bridge-client';
import { SessionRuntime } from '../src/main/bridge.js';
import {
  emptyRuntimeState,
  pendingCountAfterResponse,
  retireTurnAskUserQuestions,
  type RuntimeState,
} from '../src/renderer/bridge/bridgeRuntimeState.js';

function question(request_id: number, sessionOwned?: boolean): AskUserQuestionRequestDto {
  return {
    request_id,
    questions: [{
      question: 'Start this goal?', header: 'Goal', multi_select: false,
      options: [{ label: 'Yes', description: 'Start the goal' }, { label: 'No', description: 'Dismiss' }],
    }],
    ...(sessionOwned === undefined ? {} : { sessionOwned }),
  };
}

const turnEnded: ClientEvent = {
  type: 'turn_ended', outcome: { type: 'end_turn' },
  cost: { total_usd: 0, input_tokens: 0, output_tokens: 0, api_calls: 0, session_duration_secs: 0, formatted: '', by_account: [] },
};

function hostFixture() {
  const runtime = new SessionRuntime({ launchConfig: () => ({ workspace: '/workspace', trusted: true }) });
  const handlers = new Map<string, (...args: any[]) => unknown>();
  const answers: unknown[] = [];
  const cancellations: number[] = [];
  const client = {
    on: (name: string, handler: (...args: any[]) => unknown) => { handlers.set(name, handler); return client; },
    answerAskUserQuestion: (...args: unknown[]) => { answers.push(args); },
    cancelAskUserQuestion: (id: number) => { cancellations.push(id); },
    cancel: () => undefined,
    close: () => undefined,
  };
  (runtime as any).activeWorkspace = '/workspace';
  (runtime as any).broadcast = () => undefined;
  (runtime as any).wireClient(client, 0);
  return { runtime, answers, cancellations, event: async (event: ClientEvent) => { await handlers.get('event')!(event); } };
}

test('session Goal stays pending through turn completion and the next turn, then accepts an idle answer', async () => {
  const { runtime, event, answers } = hostFixture();
  const goal = question(12, true);
  await event({ type: 'turn_started', turn_id: 1 });
  await event({ type: 'ask_user_question', request: question(11) });
  await event({ type: 'ask_user_question', request: goal });
  await event(turnEnded);

  assert.deepEqual(runtime.pendingAskUserQuestions, [goal]);
  assert.deepEqual(runtime.remotePending().questions, [goal]);
  const replay: ClientEvent[] = [];
  (runtime as any).sendClientEvent = (_window: unknown, value: ClientEvent) => { replay.push(value); };
  runtime.replayPendingInteractions({} as never);
  assert.deepEqual(replay, [{ type: 'ask_user_question', request: goal }]);
  assert.deepEqual(answers, [], 'turn completion never chooses or approves a Goal');

  await event({ type: 'turn_started', turn_id: 2 });
  assert.deepEqual(runtime.pendingAskUserQuestions, [goal]);
  await event(turnEnded);
  assert.equal(runtime.turnActive, false);
  runtime.answerAskUserQuestion(12, { 'Start this goal?': 'Yes' });
  assert.deepEqual(answers, [[12, { 'Start this goal?': 'Yes' }]]);
  assert.deepEqual(runtime.pendingAskUserQuestions, []);
});

test('late session Goal publication survives foreground cancellation while ordinary questions stay turn-scoped', async () => {
  const { runtime, event, answers, cancellations } = hostFixture();
  await event({ type: 'turn_started', turn_id: 3 });
  await event({ type: 'ask_user_question', request: question(20, true) });
  await event({ type: 'ask_user_question', request: question(21, false) });
  runtime.cancelTurn(3);
  assert.deepEqual(runtime.pendingAskUserQuestions.map((request) => request.request_id), [20]);
  await event({ type: 'ask_user_question', request: question(22) });
  await event({ type: 'ask_user_question', request: question(23, true) });
  assert.deepEqual(runtime.pendingAskUserQuestions.map((request) => request.request_id), [20, 23]);
  await event(turnEnded);
  await event({ type: 'ask_user_question', request: question(24) });
  await event({ type: 'ask_user_question', request: question(25, true) });
  assert.deepEqual(runtime.pendingAskUserQuestions.map((request) => request.request_id), [20, 23, 25]);
  assert.deepEqual(answers, []);
  runtime.cancelAskUserQuestion(23);
  assert.deepEqual(cancellations, [23]);
  assert.deepEqual(runtime.pendingAskUserQuestions.map((request) => request.request_id), [20, 25]);
});

test('session end and disconnect retire session Goals and reject delayed publication', async () => {
  const ended = hostFixture();
  await ended.event({ type: 'ask_user_question', request: question(30, true) });
  await ended.event({ type: 'session_ended' });
  await ended.event({ type: 'ask_user_question', request: question(31, true) });
  assert.deepEqual(ended.runtime.pendingAskUserQuestions, []);
  assert.throws(() => ended.runtime.answerAskUserQuestion(30, { 'Start this goal?': 'Yes' }), /not pending/);

  const disconnected = hostFixture();
  await disconnected.event({ type: 'ask_user_question', request: question(32, true) });
  await (disconnected.runtime as any).stopBridge();
  await disconnected.event({ type: 'ask_user_question', request: question(33, true) });
  assert.deepEqual(disconnected.runtime.pendingAskUserQuestions, []);
  assert.deepEqual(disconnected.answers, []);
});

test('connection recovery retires session questions without clearing independent background permissions', async () => {
  const { runtime, event } = hostFixture();
  await event({ type: 'ask_user_question', request: question(40, true) });
  (runtime as any).pendingPermissionIds.set(41, { request_id: 41, backgroundOwned: true });
  (runtime as any).backgroundPermissionIds.add(41);
  (runtime as any).clearTurnInteractions(false, 'session');
  assert.deepEqual(runtime.pendingAskUserQuestions, []);
  assert.equal((runtime as any).pendingPermissionIds.has(41), true);
});

const hookSource = ts.createSourceFile('useBridge.ts', readFileSync(new URL('../src/renderer/bridge/useBridge.ts', import.meta.url), 'utf8'), ts.ScriptTarget.Latest, true);

function productionCallback(predicate: (node: ts.Node) => boolean): string {
  let found: ts.Node | undefined;
  const visit = (node: ts.Node): void => {
    if (!found && predicate(node)) found = node;
    ts.forEachChild(node, visit);
  };
  visit(hookSource);
  assert.ok(found, 'production callback is present');
  return found.getText(hookSource);
}

const eventReducer = productionCallback((node) => ts.isArrowFunction(node)
  && ts.isCallExpression(node.parent) && node.parent.expression.getText(hookSource) === 'updateRuntime'
  && node.getText(hookSource).includes('reduceEventWithPendingFusion(state.conversation'));
const cancelCallback = productionCallback((node) => ts.isArrowFunction(node)
  && ts.isCallExpression(node.parent) && node.parent.expression.getText(hookSource) === 'useCallback'
  && ts.isVariableDeclaration(node.parent.parent) && node.parent.parent.name.getText(hookSource) === 'cancel');

function evaluateCallback<T>(source: string, context: Record<string, unknown>): T {
  const output = ts.transpileModule(`globalThis.callback = ${source};`, {
    compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
  }).outputText;
  runInNewContext(output, context);
  return context.callback as T;
}

function rendererEvent(state: RuntimeState, event: ClientEvent): RuntimeState {
  return evaluateCallback<(state: RuntimeState) => RuntimeState>(eventReducer, {
    event, sessionId: 'session', restoringFusion: false, pendingFusion: undefined,
    bootstrapRef: { current: { runtimes: [] } }, Set,
    reduceEventWithPendingFusion: (value: unknown) => value,
    reduceDesktopEvent: (value: unknown) => value,
    reduceRuntimeCenterEvent: (value: unknown) => value,
    retireTurnAskUserQuestions, pendingCountAfterResponse,
  })(state);
}

test('renderer retains Goal visibility and counts across turns without tombstoning its replay', () => {
  const goal = question(51, true);
  let state: RuntimeState = {
    ...emptyRuntimeState({ status: 'connected' }),
    askUserQuestionQueue: [question(50), goal],
    pendingInteractionsOverride: 2, pendingAskUserQuestionsOverride: 2,
  };
  state = rendererEvent(state, turnEnded);
  assert.equal(state.askUserQuestionQueue[0], goal);
  assert.equal(state.askUserQuestionQueue.length, 1);
  assert.equal(state.pendingInteractionsOverride, 1);
  assert.equal(state.pendingAskUserQuestionsOverride, 1);
  assert.deepEqual([...state.resolvedAskUserQuestionIds], [50]);
  state = rendererEvent(state, { type: 'turn_started', turn_id: 2 });
  state = rendererEvent(state, { type: 'ask_user_question', request: goal });
  state = rendererEvent(state, { type: 'ask_user_question', request: question(50) });
  assert.equal(state.askUserQuestionQueue.length, 1, 'replay is deduplicated and old foreground questions stay retired');
  assert.equal(state.askUserQuestionQueue[0], goal);
  assert.equal(state.pendingInteractionsOverride, 1);
});

test('renderer accepts late idle Goal publication, resolves it explicitly, and retires the session queue on close', () => {
  const goal = question(60, true);
  let state = rendererEvent(emptyRuntimeState({ status: 'connected' }), turnEnded);
  state = rendererEvent(state, { type: 'ask_user_question', request: goal });
  assert.equal(state.askUserQuestionQueue[0], goal);
  assert.equal(state.pendingAskUserQuestionsOverride, 1);
  state = rendererEvent(state, { type: 'ask_user_question_resolved', request_id: 60 });
  assert.equal(state.askUserQuestionQueue.length, 0);
  assert.equal(state.pendingInteractionsOverride, 0);
  assert.equal(state.pendingAskUserQuestionsOverride, 0);
  assert.equal(state.resolvedAskUserQuestionIds.has(60), true);
  state = rendererEvent(state, { type: 'ask_user_question', request: goal });
  assert.equal(state.askUserQuestionQueue.length, 0, 'resolved Goal replay remains tombstoned');
  state = rendererEvent(state, { type: 'ask_user_question', request: question(61, true) });
  state = rendererEvent(state, { type: 'session_ended' });
  assert.equal(state.askUserQuestionQueue.length, 0);
  assert.equal(state.pendingAskUserQuestionsOverride, 0);
  assert.equal(state.resolvedAskUserQuestionIds.size, 0, 'session teardown resets its request-id namespace');
});

test('renderer cancellation preserves session Goals while retiring ordinary questionnaires', async () => {
  const goal = question(71, true);
  let state: RuntimeState = {
    ...emptyRuntimeState({ status: 'connected' }), askUserQuestionQueue: [question(70), goal],
  };
  const cancel = evaluateCallback<() => Promise<void>>(cancelCallback, {
    activeSessionIdRef: { current: 'session' }, sessionLoadingRef: { current: false },
    host: { cancel: async () => undefined }, turnActiveRefs: { current: new Map([['session', true]]) },
    runtimeStatesRef: { current: new Map([['session', state]]) }, runningSubagentIds: () => [],
    cancellingRefs: { current: new Map() }, cancellationTasks: { current: new Map() },
    pendingFusionDispatches: { current: new Map() }, Set, retireTurnAskUserQuestions,
    updateRuntime: (_id: string, update: (state: RuntimeState) => RuntimeState) => { state = update(state); },
    capture: (error: unknown) => { throw error; },
  });
  await cancel();
  assert.equal(state.askUserQuestionQueue[0], goal);
  assert.equal(state.askUserQuestionQueue.length, 1);
  assert.equal(state.pendingInteractionsOverride, 1);
  assert.equal(state.pendingAskUserQuestionsOverride, 1);
  assert.deepEqual([...state.resolvedAskUserQuestionIds], [70]);
});
