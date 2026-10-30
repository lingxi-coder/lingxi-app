import assert from 'node:assert/strict';
import { test } from 'node:test';
import { TEAM_APPROVAL_PREFIX, TeamApprovalsGateway, TeamOwnerApprovalBroker, type TeamOwnerApprovalRequest } from '../src/main/remoteControl/teamApprovals.js';
import type { RemoteViewer, RemoteViewerChannel } from '../src/main/remoteControl/gateway.js';

const viewer: RemoteViewer = { account_uid: 'owner', host_id: 'rc-host', device_id: 'phone', conn_id: 'connection', device_name: 'Phone', platform: 'ios' };
const target = { ownerUid: 'owner', ownerGeneration: 4, hostId: 'team-private-host', botId: 'bot-id', botRevision: 8,
  teamId: 'team-id', conversationId: 'conversation', botKey: 'bot:owner:team-private-host:bot-id', triggerId: 'trigger-id' };
const tick = () => new Promise<void>(resolve => setImmediate(resolve));

function fixture() {
  let remote = true, current = true, approved = true, active = true, now = 100, rcGeneration = 1;
  let owner: { uid: string; epoch: number } | undefined = { uid: 'owner', epoch: 9 };
  let local: ((value: boolean) => void) | undefined;
  let localSignal: AbortSignal | undefined;
  const notifications: Array<{ method: string; params: any }> = [], baseCalls: string[] = [];
  const channel: RemoteViewerChannel = { notify(method, params) { notifications.push({ method, params }); }, async request() { return {}; } };
  const broker = new TeamOwnerApprovalBroker({ botRemoteEnabled: botId => remote && botId === target.botId, now: () => now });
  const base = { async handle(_viewer: RemoteViewer, method: string, _params: unknown, _channel: RemoteViewerChannel): Promise<unknown> { baseCalls.push(method); return method.startsWith('sessions.') ? { sessions: [{ session_id: 'ordinary' }] } : 'ordinary-result'; }, viewerClosed() {} };
  const gateway = new TeamApprovalsGateway(base, broker, {
    ownerContext: () => owner, hostId: () => 'rc-host', active: () => active,
    approvedViewer: v => approved && v.conn_id === viewer.conn_id && v.device_id === viewer.device_id,
    generation: () => rcGeneration, remoteEnabled: value => remote && value.botId === target.botId,
  });
  const controller = new AbortController();
  const input: TeamOwnerApprovalRequest = { target, accountEpoch: 9, toolName: 'Bash', input: { command: 'ls scoped-workspace' },
    assertCurrent() { if (!current || owner?.uid !== target.ownerUid || owner.epoch !== 9) throw new Error('stale binding/account'); },
    showLocal: signal => { localSignal = signal; return new Promise<boolean>(resolve => { local = resolve; }); },
  };
  const request = () => broker.request(input, controller.signal);
  const nonce = () => broker.list('owner', 9)[0]!.nonce;
  const attach = async () => { const id = TEAM_APPROVAL_PREFIX + nonce(); await gateway.handle(viewer, 'session.attach', { session_id: id }, channel); return id; };
  const respond = (id: string, type = 'allow_once', extra = {}) => gateway.handle(viewer, 'permission.respond', { session_id: id, request_id: 1, decision: { type }, ...extra }, channel);
  return { broker, gateway, controller, input, request, nonce, attach, respond, channel, notifications, baseCalls, base,
    resolveLocal: (value: boolean) => local!(value), localSignal: () => localSignal,
    setRemote: (value: boolean) => { remote = value; }, setCurrent: (value: boolean) => { current = value; },
    setOwner: (value: typeof owner) => { owner = value; }, setApproved: (value: boolean) => { approved = value; },
    setActive: (value: boolean) => { active = value; }, setTime: (value: number) => { now = value; },
    advanceRC: () => { rcGeneration++; }, close() { gateway.close(); broker.close(); } };
}

test('owner sees approval-only pending DTO; RC and Team host identities stay distinct', async () => {
  const f = fixture(); const result = f.request();
  const listed = await f.gateway.handle(viewer, 'sessions.list', {}, f.channel) as any;
  assert.equal(listed.sessions.length, 2); const id = listed.sessions[1].session_id;
  assert.equal(listed.sessions[1].project_label, 'Allow once / Deny only');
  await f.gateway.handle(viewer, 'session.attach', { session_id: id }, f.channel);
  const end = f.notifications.find(row => row.params.phase === 'end')!.params;
  assert.equal(end.pending.permissions.length, 1);
  assert.equal(end.pending.permissions[0].suppress_always_allow_rule, true);
  assert.equal(end.pending.permissions[0].kind.default_allow, false);
  assert.equal(end.pending.permissions[0].auto_mode_prompt, undefined);
  assert.deepEqual(end.pending.questions, []); assert.deepEqual(end.pending.computer_access, []);
  assert.equal(end.controls, undefined); assert.deepEqual(end.turn, { running: false });
  await f.respond(id); assert.equal(await result, true); f.close();
});

test('reserved capabilities reject every ordinary operation, including source-human questions', async () => {
  const f = fixture(); const result = f.request(); const id = await f.attach();
  for (const method of ['prompt.send', 'question.answer', 'question.cancel', 'turn.cancel', 'controls.set_model', 'controls.set_reasoning', 'controls.set_permission_mode', 'models.list', 'file.offer', 'file.commit', 'file.download', 'image.get', 'session.history', 'media.playback_receipt', 'bot.thread.create', 'execute']) {
    await assert.rejects(f.gateway.handle(viewer, method, { session_id: id, command: 'arbitrary' }, f.channel), /only allow once or deny/);
  }
  await assert.rejects(f.gateway.handle(viewer, 'prompt.send', { session_id: TEAM_APPROVAL_PREFIX + 'unknown-nonce' }, f.channel), /only allow once or deny/);
  assert.deepEqual(f.baseCalls, []); f.controller.abort(); assert.equal(await result, false); f.close();
});

test('only allow_once and deny with exact request identity are accepted', async () => {
  const f = fixture(); const result = f.request(); const id = TEAM_APPROVAL_PREFIX + f.nonce();
  await assert.rejects(f.respond(id), /only allow once or deny/);
  await f.attach();
  for (const type of ['allow_always', 'allow_auto', 'bypassPermissions', 'true', 'allow_once ']) await assert.rejects(f.respond(id, type), /only allow once or deny/);
  await assert.rejects(f.respond(id, 'allow_once', { command: 'shell' }), /only allow once or deny/);
  await assert.rejects(f.respond(id, 'allow_once', { request_id: 2 }), /only allow once or deny/);
  await f.respond(id, 'deny'); assert.equal(await result, false);
  await assert.rejects(f.respond(id), /no longer pending/); f.close();
});

test('RC wins exactly once and aborts the native dialog; late native allow cannot regrant', async () => {
  const f = fixture(); const result = f.request(); await tick(); const id = await f.attach();
  await f.respond(id, 'deny'); assert.equal(await result, false); assert.equal(f.localSignal()!.aborted, true);
  f.resolveLocal(true); await tick(); assert.equal(f.broker.list('owner', 9).length, 0); f.close();
});

test('native wins exactly once and removes pending card from attached/watch viewers', async () => {
  const f = fixture(); const result = f.request(); await tick();
  await f.gateway.handle(viewer, 'sessions.watch', {}, f.channel); const id = await f.attach();
  f.resolveLocal(true); assert.equal(await result, true); await tick();
  assert.ok(f.notifications.some(row => row.method === 'session.pending' && row.params.pending.permissions.length === 0));
  assert.ok(f.notifications.some(row => row.method === 'session.closed' && row.params.session_id === id));
  assert.ok(f.notifications.some(row => row.method === 'sessions.changed' && row.params.sessions.length === 1));
  await assert.rejects(f.respond(id), /no longer pending/); f.close();
});

test('Bot.remote disables remote list/response while preserving owner native approval', async () => {
  const f = fixture(); const result = f.request(); await tick(); const id = await f.attach(); f.setRemote(false);
  const listed = await f.gateway.handle(viewer, 'sessions.list', {}, f.channel) as any;
  assert.equal(listed.sessions.length, 1); await assert.rejects(f.respond(id), /no longer pending/);
  f.resolveLocal(true); assert.equal(await result, true); f.close();
});

for (const boundary of ['binding', 'owner', 'epoch', 'connection', 'policy', 'rcEpoch', 'deadline'] as const) {
  test(`approval cannot cross a changed ${boundary} boundary`, async () => {
    const f = fixture(); const result = f.request(); await tick(); const id = await f.attach();
    switch (boundary) {
      case 'binding': f.setCurrent(false); break;
      case 'owner': f.setOwner({ uid: 'other', epoch: 9 }); break;
      case 'epoch': f.setOwner({ uid: 'owner', epoch: 10 }); break;
      case 'connection': f.setApproved(false); break;
      case 'policy': f.setActive(false); break;
      case 'rcEpoch': f.advanceRC(); break;
      case 'deadline': f.setTime(300100); break;
    }
    await assert.rejects(f.respond(id));
    f.resolveLocal(true);
    // RC policy/connection changes do not revoke a still-current LOCAL owner decision.
    const localAllowed = ['connection', 'policy', 'rcEpoch'].includes(boundary);
    assert.equal(await result, localAllowed); f.close();
  });
}

test('different owner/device/host cannot list or respond to capabilities', async () => {
  const f = fixture(); const result = f.request(); const id = await f.attach();
  for (const other of [{ ...viewer, account_uid: 'member', conn_id: 'member-conn' }, { ...viewer, host_id: 'other-host', conn_id: 'other-host-conn' }, { ...viewer, device_id: 'other-phone', conn_id: 'other-conn' }, { ...viewer, account_uid: undefined, conn_id: 'anonymous' }]) {
    await assert.rejects(f.gateway.handle(other, 'sessions.list', {}, f.channel));
    await assert.rejects(f.gateway.handle(other, 'permission.respond', { session_id: id, request_id: 1, decision: { type: 'allow_once' } }, f.channel));
  }
  assert.deepEqual(f.broker.list('member', 9), []); assert.deepEqual(f.broker.list('owner', 10), []);
  f.controller.abort(); assert.equal(await result, false); f.close();
});

test('account/device revocation during an awaited ordinary catalog cannot expose Team pending', async () => {
  const f = fixture(); const result = f.request(); let release!: (value: unknown) => void;
  f.base.handle = async () => new Promise(resolve => { release = resolve; });
  const listing = f.gateway.handle(viewer, 'sessions.list', {}, f.channel);
  f.advanceRC(); release({ sessions: [] }); await assert.rejects(listing, /access changed/);
  f.controller.abort(); assert.equal(await result, false); f.close();
});

test('abort, closed broker, invalid input and expired/binding-changed local results deny', async () => {
  const f = fixture(); const result = f.request(); await tick(); f.controller.abort();
  assert.equal(await result, false); assert.equal(f.localSignal()!.aborted, true);
  assert.equal(await f.broker.request(f.input, f.controller.signal), false);
  assert.equal(await f.broker.request({ ...f.input, input: { huge: 'x'.repeat(65536) } }, new AbortController().signal), false);
  f.broker.close(); assert.equal(await f.broker.request(f.input, new AbortController().signal), false); f.close();
});

test('ordinary session traffic continues through its existing gateway unchanged', async () => {
  const f = fixture(); assert.equal(await f.gateway.handle(viewer, 'prompt.send', { session_id: 'ordinary' }, f.channel), 'ordinary-result');
  assert.deepEqual(f.baseCalls, ['prompt.send']); f.close();
});
