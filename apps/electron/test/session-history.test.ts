import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { runInNewContext } from 'node:vm';
import { test } from 'node:test';
import ts from 'typescript';
import type { MessageDto } from '@lingxi/bridge-client';
import { historyCursor, initialHistory, recentTranscript } from '../src/shared/sessionHistory';
import { conversationFromMessages, emptyConversation, prependHistory, reduceEvent } from '../src/renderer/bridge/conversation';
import { addRuntimeResources, emptyRuntimeCenterState, prependRuntimeHistory, resourcesFromRestoredMessages, reduceRuntimeCenterEvent } from '../src/renderer/bridge/runtimeCenterState';
import { conversationPlans } from '../src/renderer/bridge/planDocuments';
import { transcriptRowUuids } from '../src/renderer/model/runItem';

const uuid = (n: number) => `00000000-0000-4000-8000-${n.toString(16).padStart(12, '0')}`;
const message = (n: number): MessageDto => ({ transcript_uuid: uuid(n), role: 'user', blocks: [{ type: 'text', text: `message ${n}` }] });
const boundary = (n: number): MessageDto => ({ transcript_uuid: uuid(n), role: 'system', blocks: [{ type: 'compact_boundary', summary: '', messages_before: 10, messages_after: 2 }] });

test('initial replay retains only the latest compact and all later messages', () => {
  const rows = [message(1), boundary(2), message(3), boundary(4), message(5)];
  assert.deepEqual(recentTranscript(rows), rows.slice(3));
  assert.deepEqual(initialHistory(recentTranscript(rows)).before, { uuid: uuid(4) });
  assert.equal(initialHistory([message(1)]).before, null);
  const state = reduceEvent(emptyConversation(), { type: 'session_resumed', session_id: uuid(100), messages: rows } as any);
  assert.equal(state.items.length, 2);
});

test('older pages preserve current item identities, streaming state and latest plan', () => {
  let state = conversationFromMessages([message(80)]);
  state = reduceEvent(state, { type: 'turn_started', turn: 1 } as any);
  state = reduceEvent(state, { type: 'text_delta', text: 'streaming' });
  const current = state.items.at(-1), currentIndex = state.openAssistantIndex;
  const prepended = prependHistory(state, [message(1), message(2)], 'first-page');
  assert.equal(prepended.items.at(-1), current);
  assert.equal(prepended.nextId, state.nextId);
  assert.equal(prepended.openAssistantIndex, currentIndex + 2);
  assert.equal(prepended.plan, state.plan);
  const streamed = reduceEvent(prepended, { type: 'text_delta', text: ' more' });
  assert.equal((streamed.items.at(-1) as any).text, 'streaming more');
  const second = prependHistory(streamed, [message(0)], 'second-page');
  assert.equal(new Set(second.items.map(item => item.id)).size, second.items.length);
  assert.equal(prependHistory(second, [message(0)], 'duplicate-page'), second);
});

test('a tool split across history pages retains its completed result and recovers its input', () => {
  const result: MessageDto = { role: 'user', transcript_uuid: uuid(2), blocks: [{ type: 'tool_result', id: 'tool1', tool: 'Read', result_json: '"done"', is_error: false, display: { body_lines: 1, body: 'done' } }] };
  const call: MessageDto = { role: 'assistant', transcript_uuid: uuid(1), blocks: [{ type: 'tool_use', id: 'tool1', tool: 'Read', input_json: '{"file_path":"a.ts"}', header: { verb: 'read', label: 'Read', title: 'Read a.ts', primary: 'a.ts' } as any }] };
  const state = prependHistory(conversationFromMessages([result]), [call], 'previous');
  assert.equal(state.items.length, 1);
  assert.equal((state.items[0] as any).status, 'done');
  assert.equal((state.items[0] as any).view.title, 'Read a.ts');
  assert.deepEqual((state.items[0] as any).nativeInput, { file_path: 'a.ts' });
  assert.deepEqual(new Set(transcriptRowUuids(state.items[0])), new Set([uuid(1), uuid(2)]));
  assert.equal(prependHistory(state, [call], 'repeated'), state);
});

test('history cursors reject invalid offsets and mismatched identity carriers', () => {
  assert.deepEqual(historyCursor({ uuid: uuid(1) }), { uuid: uuid(1) });
  for (const value of [{ uuid: 'bad' }, { uuid: uuid(1), offset: -1, identity: 'file' },
    { uuid: uuid(1), offset: 3 }, { uuid: uuid(1), offset: 4, identity: 'file', path: '/secret' }]) assert.throws(() => historyCursor(value));
});

test('an Agent result crossing the compact display cut recovers its name and child identity', () => {
  const result: MessageDto = { role: 'user', transcript_uuid: uuid(2), blocks: [{ type: 'tool_result', id: 'agent', tool: '', result_json: '{"agentId":"saved-child","status":"completed"}', is_error: false, display: { body_lines: 1, body: 'completed' } }] };
  const call: MessageDto = { role: 'assistant', transcript_uuid: uuid(1), blocks: [{ type: 'tool_use', id: 'agent', tool: 'Agent', input_json: '{"description":"Inspect"}', header: { verb: 'agent', label: 'Agent', title: 'Inspect' } }] };
  const state = prependHistory(conversationFromMessages([result]), [call], 'older');
  const item = state.items[0];
  assert.equal(item.type, 'tool');
  if (item.type !== 'tool') return;
  assert.equal(item.tool, 'Agent');
  assert.equal(item.agentId, 'saved-child');
  assert.deepEqual(item.nativeOutput, { agentId: 'saved-child', status: 'completed' });
});

function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
function loader() {
  const response = deferred<any>();
  const states = new Map([[uuid(100), { conversation: conversationFromMessages([boundary(10), message(11)]), history: initialHistory([boundary(10)]), runtimeCenter: emptyRuntimeCenterState() }]]);
  let reads = 0;
  const context: any = {
    useCallback: (fn: unknown) => fn, Promise, prependHistory, messageFrom: String,
    addRuntimeResources, resourcesFromRestoredMessages, prependRuntimeHistory,
    activeSessionIdRef: { current: uuid(100) }, sessionLoadingRef: { current: false },
    runtimeStatesRef: { current: states }, historyFlights: { current: new Map() },
    updateRuntime: (id: string, update: (state: any) => any) => states.set(id, update(states.get(id))),
    host: { loadSessionHistory: () => { reads++; return response.promise; } },
  };
  const source = ts.createSourceFile('useBridge.ts', readFileSync('src/renderer/bridge/useBridge.ts', 'utf8'), ts.ScriptTarget.Latest, true);
  let initializer: ts.Expression | undefined;
  const visit = (node: ts.Node) => { if (ts.isVariableDeclaration(node) && node.name.getText(source) === 'loadMoreHistory') initializer = node.initializer; ts.forEachChild(node, visit); };
  visit(source); assert.ok(initializer);
  runInNewContext(ts.transpileModule('globalThis.load = ' + initializer.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText, context);
  return { context, response, states, get reads() { return reads; } };
}

test('repeated top-scroll events share one request and prepend exactly once', async () => {
  const f = loader();
  const first = f.context.load(), second = f.context.load();
  assert.equal(first, second);
  await Promise.resolve(); assert.equal(f.reads, 1);
  f.response.resolve({ messages: [message(1)], before: null });
  await first;
  const state = f.states.get(uuid(100))!;
  assert.equal(state.conversation.items.length, 3);
  assert.equal(state.history.before, null);
  assert.equal(state.history.loading, false);
});

test('a late page from an obsolete resume cannot mutate the replacement transcript', async () => {
  const f = loader(), opening = f.context.load();
  await Promise.resolve();
  const replacement = { conversation: conversationFromMessages([message(500)]), history: initialHistory([boundary(20)]), runtimeCenter: emptyRuntimeCenterState() };
  f.states.set(uuid(100), replacement);
  f.response.resolve({ messages: [message(1)], before: null }); await opening;
  assert.equal(f.states.get(uuid(100)), replacement);
});

test('failed history requests retain their cursor and show a retryable error', async () => {
  const f = loader(), opening = f.context.load();
  f.response.reject(new Error('offline')); await opening;
  const state = f.states.get(uuid(100))!;
  assert.equal(state.history.before?.uuid, uuid(10));
  assert.equal(state.history.loading, false);
  assert.match(state.history.error ?? '', /offline/);
});

test('images restored from separate pages keep distinct resource identities', async () => {
  const image = (n: number): MessageDto => ({ ...message(n), images: [{ media_type: 'image/png', url: `data:image/png;base64,${n}` }] });
  const f = loader(), current = f.states.get(uuid(100))!;
  current.runtimeCenter = addRuntimeResources(current.runtimeCenter, resourcesFromRestoredMessages(uuid(100), [image(11)]));
  const pending = f.context.load();
  f.response.resolve({ messages: [image(1)], before: null }); await pending;
  const resources = f.states.get(uuid(100))!.runtimeCenter.resources;
  assert.equal(resources.length, 2);
  assert.notEqual(resources[0].id, resources[1].id);
});

test('loading an older approved plan restores its card without replacing the current approval', async () => {
  const f = loader(), current = f.states.get(uuid(100))!;
  current.runtimeCenter = reduceRuntimeCenterEvent(current.runtimeCenter, {
    type: 'tool_use_started', id: 'current-plan', tool: 'ExitPlanMode', input_json: '{"plan":"Current plan"}',
  } as any, uuid(100));
  const pending = f.context.load();
  f.response.resolve({ messages: [
    { role: 'assistant', transcript_uuid: uuid(1), blocks: [{ type: 'tool_use', id: 'old-plan', tool: 'ExitPlanMode', input_json: '{}', header: { verb: 'plan', label: 'Plan', title: 'Submit plan' } }] },
    { role: 'user', transcript_uuid: uuid(2), blocks: [{ type: 'tool_result', id: 'old-plan', tool: 'ExitPlanMode', result_json: '{"plan":"Historical plan","model_content":"User has approved your plan."}', is_error: false, display: { body: 'approved', body_lines: 1 } }] },
  ], before: null });
  await pending;
  const state = f.states.get(uuid(100))!;
  const plans = conversationPlans(state.conversation.items, state.runtimeCenter.submittedPlanState.calls);
  assert.equal(plans.find(plan => plan.id === 'old-plan')?.content, 'Historical plan');
  assert.equal(plans.find(plan => plan.id === 'old-plan')?.status, 'approved');
  assert.equal(state.runtimeCenter.submittedPlan?.id, 'current-plan');
  assert.equal(state.runtimeCenter.submittedPlanState.calls.at(-1)?.status, 'submitted');
  const repeated = prependRuntimeHistory(state.runtimeCenter, uuid(100), []);
  assert.equal(repeated, state.runtimeCenter);
});

test('loading a pre-compact plan call recovers the approved plan from the current tail result', async () => {
  const f = loader(), current = f.states.get(uuid(100))!;
  const result: MessageDto = { role: 'user', transcript_uuid: uuid(12), blocks: [{ type: 'tool_result', id: 'cross-plan', tool: '', result_json: '{"plan":"Plan read from a file","model_content":"User has approved your plan."}', is_error: false, display: { body: 'approved', body_lines: 1 } }] };
  current.conversation = conversationFromMessages([boundary(10), result]);
  current.runtimeCenter = reduceRuntimeCenterEvent(current.runtimeCenter, { type: 'session_resumed', session_id: uuid(100), messages: [boundary(10), result] } as any, uuid(100));
  const pending = f.context.load();
  f.response.resolve({ messages: [{ role: 'assistant', transcript_uuid: uuid(1), blocks: [{ type: 'tool_use', id: 'cross-plan', tool: 'ExitPlanMode', input_json: '{}', header: { verb: 'plan', label: 'Plan', title: 'Submit plan' } }] }], before: null });
  await pending;
  const state = f.states.get(uuid(100))!;
  const plan = conversationPlans(state.conversation.items, state.runtimeCenter.submittedPlanState.calls).find(plan => plan.id === 'cross-plan');
  assert.equal(plan?.content, 'Plan read from a file');
  assert.equal(plan?.status, 'approved');
  assert.equal(state.runtimeCenter.submittedPlan?.id, 'cross-plan');
});
