import { test } from 'node:test';
import assert from 'node:assert/strict';
import type { ClientEvent, MessageDto, PermissionRequest } from '@lingxi/bridge-client';
import { emptyRuntimeCenterState, resetRuntimeCenterConnection, reduceRuntimeCenterEvent, reduceRuntimeCenterPermission, openRuntimeCenterItem, closeRuntimeCenterItem, setRuntimeInspectorOpen } from '../src/renderer/bridge/runtimeCenterState';
const session = 'session-a';
const start = (id: string, plan = '# Draft'): ClientEvent => ({ type: 'tool_use_started', id, tool: 'ExitPlanMode', input_json: JSON.stringify({ plan }) });
const result = (id: string, plan = '', is_error = false): ClientEvent => ({ type: 'tool_use_result', id, tool: 'ExitPlanMode', result_json: JSON.stringify({ plan }), is_error });
const request = (request_id = 1, plan = '# Approval body'): PermissionRequest => ({ request_id, kind: { type: 'exit_plan_mode', plan } });
const event = (state: ReturnType<typeof emptyRuntimeCenterState>, value: ClientEvent) => reduceRuntimeCenterEvent(state, value, session);
test('approval and successful updated content stay separate from todos', () => {
 let s = event(emptyRuntimeCenterState(), start('a'));
 s = reduceRuntimeCenterPermission(s, request(), session);
 assert.deepEqual(s.submittedPlan, { id: 'a', content: '# Approval body', status: 'pending' });
 s = event(s, { type: 'permission_request_resolved', request_id: 1, resolution: 'approved' });
 assert.equal(s.submittedPlan?.status, 'approved');
 s = event(s, result('a', '# Updated'));
 s = event(s, { type: 'plan_updated', tasks: [{ subject: 'Todo', state: 'pending' }] });
 assert.equal(s.submittedPlan?.content, '# Updated');
 assert.equal(s.plan[0]?.subject, 'Todo');
});
test('explicit denial survives a tool error; unclassified errors are failed', () => {
 let s = reduceRuntimeCenterPermission(event(emptyRuntimeCenterState(), start('a')), request(), session);
 s = event(s, { type: 'permission_request_resolved', request_id: 1, resolution: 'denied' });
 s = event(s, result('a', '', true));
 assert.equal(s.submittedPlan?.status, 'rejected');
 s = event(event(s, start('b')), result('b', 'Permission denied', true));
 assert.equal(s.submittedPlan?.status, 'failed');
 assert.equal(s.submittedPlan?.content, '# Draft');
});
test('replayed starts and late older results never replace latest nonempty plan', () => {
 let s = event(event(emptyRuntimeCenterState(), start('old', '# Old')), start('new', '# New'));
 s = event(event(s, result('old', '# Old updated')), start('old', '# Replay'));
 s = event(s, start('empty', '  '));
 s = reduceRuntimeCenterPermission(s, request(4, ''), session);
 assert.equal(s.submittedPlan?.id, 'new');
 assert.equal(s.submittedPlanState.calls.length, 3);
});
test('permission may precede tool start and resolution precede permission replay', () => {
 let s = reduceRuntimeCenterPermission(emptyRuntimeCenterState(), request(), session);
 assert.equal(s.submittedPlan, null);
 s = event(s, { type: 'permission_request_resolved', request_id: 1, resolution: 'denied' });
 s = event(s, start('late'));
 s = reduceRuntimeCenterPermission(s, request(), session);
 assert.equal(s.submittedPlan?.status, 'rejected');
 assert.equal(s.submittedPlan?.content, '# Approval body');
});
test('history requires explicit success evidence and ignores ordinary prose', () => {
 const messages: MessageDto[] = [{ role: 'assistant', blocks: [
 { type: 'text', text: '# Not submitted' },
 { type: 'tool_use', id: 'h', tool: 'ExitPlanMode', input_json: '{"plan":"# Stored"}' },
 { type: 'tool_result', id: 'h', tool: 'ExitPlanMode', result_json: '{}', is_error: false },
 ] }];
 let s = event(emptyRuntimeCenterState(), { type: 'session_resumed', session_id: session, mode: 'code', messages } as ClientEvent);
 assert.equal(s.submittedPlan?.status, 'submitted');
 s = event(s, { type: 'message_complete', message: { role: 'assistant', blocks: [
 { type: 'tool_use', id: 'v', tool: 'ExitPlanMode', input_json: '{"plan":"# Verified"}' },
 { type: 'tool_result', id: 'v', tool: 'ExitPlanMode', result_json: '{"plan_mode":false}', is_error: false },
 ] } });
 assert.equal(s.submittedPlan?.status, 'approved');
});
test('resumed session clears prior plan and foreign or worker approvals are ignored', () => {
 let s = event(emptyRuntimeCenterState(), start('a'));
 assert.equal(reduceRuntimeCenterPermission(s, { ...request(), owner: { session_id: 'other' } }, session), s);
 assert.equal(reduceRuntimeCenterPermission(s, { ...request(), worker: { name: 'agent', color: 'blue' } }, session), s);
 s = event(s, { type: 'session_resumed', session_id: session, mode: 'code', messages: [] } as ClientEvent);
 assert.equal(s.submittedPlan, null);
});
test('inspector hiding retains tabs and closing final tab leaves landing open', () => {
 const item = { kind: 'section' as const, id: 'todos' as const };
 let s = openRuntimeCenterItem(emptyRuntimeCenterState(), item);
 s = setRuntimeInspectorOpen(s, false);
 assert.deepEqual(s.tabs, [item]);
 s = closeRuntimeCenterItem(setRuntimeInspectorOpen(s, true), item);
 assert.deepEqual(s.tabs, []);
 assert.equal(s.activeItem, null);
 assert.equal(s.inspectorOpen, true);
});
test('completed message replay cannot undo live updated approval content', () => {
 let s = event(event(emptyRuntimeCenterState(), start('a')), result('a', '# Updated'));
 s = event(s, { type: 'message_complete', message: { role: 'assistant', blocks: [
 { type: 'tool_use', id: 'a', tool: 'ExitPlanMode', input_json: '{"plan":"# Draft"}' },
 { type: 'tool_result', id: 'a', tool: 'ExitPlanMode', result_json: '{"plan":"# Old","plan_mode":false}', is_error: false },
 ] } });
 assert.equal(s.submittedPlan?.content, '# Updated');
});
test('result-only restored plan uses structured body without inventing approval', () => {
 const s = event(emptyRuntimeCenterState(), { type: 'message_complete', message: { role: 'user', blocks: [
 { type: 'tool_result', id: 'a', tool: 'ExitPlanMode', result_json: '{"plan":"# Saved"}', is_error: false },
 ] } });
 assert.deepEqual(s.submittedPlan, { id: 'a', content: '# Saved', status: 'submitted' });
});
test('unrelated resolutions stay bounded and resolved unknown requests never attach to later calls', () => {
 let s = emptyRuntimeCenterState();
 for (let id = 0; id < 500; id++) s = event(s, { type: 'permission_request_resolved', request_id: id, resolution: 'approved' });
 assert.equal(Object.keys(s.submittedPlanState.resolutions).length, 0);
 s = event(s, start('new'));
 s = reduceRuntimeCenterPermission(s, request(1, '# Stale'), session);
 assert.equal(s.submittedPlan?.status, 'submitted');
 assert.equal(s.submittedPlan?.content, '# Draft');
});
test('delayed denial resolution cannot downgrade successful tool execution', () => {
 let s = reduceRuntimeCenterPermission(event(emptyRuntimeCenterState(), start('a')), request(), session);
 s = event(s, result('a', '# Approved'));
 s = event(s, { type: 'permission_request_resolved', request_id: 1, resolution: 'denied' });
 assert.equal(s.submittedPlan?.status, 'approved');
});
test('same-session history restore preserves approval that arrived first', () => {
 let s = reduceRuntimeCenterPermission(event(emptyRuntimeCenterState(), start('a')), request(), session);
 s = event(s, { type: 'session_resumed', session_id: session, mode: 'code', messages: [] } as ClientEvent);
 assert.deepEqual(s.submittedPlan, { id: 'a', content: '# Approval body', status: 'pending' });
 s = event(s, { type: 'permission_request_resolved', request_id: 1, resolution: 'approved' });
 assert.equal(s.submittedPlan?.status, 'approved');
});
test('permission preceding history attaches to the recovered tool id', () => {
 let s = reduceRuntimeCenterPermission(emptyRuntimeCenterState(), request(), session);
 s = event(s, { type: 'session_resumed', session_id: session, mode: 'code', messages: [{ role: 'assistant', blocks: [
 { type: 'tool_use', id: 'a', tool: 'ExitPlanMode', input_json: '{"plan":"# Draft"}' },
 ] }] } as ClientEvent);
 assert.deepEqual(s.submittedPlan, { id: 'a', content: '# Approval body', status: 'pending' });
});
test('duplicate tool results cannot undo completed plan', () => {
 let s = event(event(emptyRuntimeCenterState(), start('a')), result('a', '# Approved'));
 s = event(s, result('a', '# Error', true));
 assert.deepEqual(s.submittedPlan, { id: 'a', content: '# Approved', status: 'approved' });
});

test('connection reset clears request generation and fails abandoned approval', () => {
 let s = reduceRuntimeCenterPermission(event(emptyRuntimeCenterState(), start('old')), request(100), session);
 s = event(s, { type: 'permission_request_resolved', request_id: 99, resolution: 'approved' });
 s = resetRuntimeCenterConnection({ ...s, overviewOpen: true });
 assert.equal(s.overviewOpen, false);
 assert.equal(s.submittedPlan?.status, 'failed');
 assert.equal(s.submittedPlanState.calls[0]?.requestId, undefined);
 assert.equal(s.submittedPlanState.lastResolvedRequestId, undefined);
 assert.equal(s.submittedPlanState.waitingRequest, undefined);
 assert.deepEqual(s.submittedPlanState.resolutions, {});
 s = reduceRuntimeCenterPermission(event(s, start('new')), request(1, '# New approval'), session);
 assert.deepEqual(s.submittedPlan, { id: 'new', content: '# New approval', status: 'pending' });
});
test('connection reset retains completed plans but history does not retain abandoned live calls', () => {
 const completed = event(event(emptyRuntimeCenterState(), start('done')), result('done', '# Done'));
 assert.deepEqual(resetRuntimeCenterConnection(completed).submittedPlan, completed.submittedPlan);
 let pending = resetRuntimeCenterConnection(reduceRuntimeCenterPermission(event(emptyRuntimeCenterState(), start('old')), request(100), session));
 pending = event(pending, { type: 'session_resumed', session_id: session, mode: 'code', messages: [] } as ClientEvent);
 assert.equal(pending.submittedPlan, null);
 assert.deepEqual(pending.submittedPlanState.calls, []);
});
