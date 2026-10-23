import assert from 'node:assert/strict';
import test from 'node:test';
import type { ConversationControlsDto, MessageDto, PermissionRequest } from '@lingxi/bridge-client';
import { RemoteRpcError, type RemotePending, type RemoteSeqRecord, type SessionAttachParams } from '@lingxi/bridge-client/remote';
import { RemoteClient, questionAnswers, selectablePermissionModes, type SessionController } from '../src/remote/remoteClient';
import { imageRefOf, IMAGE_REF_SCHEME } from '../src/remote/sessionModel';
import type { HostConnection } from './support/fakeHost';
import { drive, harness, type Harness } from './support/remoteHarness';

const SESSION = 'sess-1';
const NO_PENDING: RemotePending = { permissions: [], questions: [], computer_access: [] };

const message = (role: 'user' | 'assistant', text: string): MessageDto => ({ role, blocks: [{ type: 'text', text }] });
const event = (seq: number, e: object): RemoteSeqRecord => ({ seq, record: { type: 'event', event: e as never } });
const permission = (id: number): PermissionRequest => ({ request_id: id, kind: { type: 'tool_use_confirm', tool_name: 'Bash', tool_input_json: '{"command":"ls"}', default_allow: false } });

const controls: ConversationControlsDto = {
  qualified_model: 'p/model-a',
  permission: {
    requested: 'default',
    effective: 'default',
    options: [
      { mode: 'default', available: true },
      { mode: 'acceptEdits', available: true },
      { mode: 'plan', available: false },
      { mode: 'bypassPermissions', available: true },
    ],
  },
  reasoning: { requested: { type: 'automatic' }, effective: { type: 'automatic' }, spec: { options: [], provider_default: { type: 'automatic' }, forced_reasoning: false, editable: true } },
};

/** A desktop that serves one session from a script. */
class SessionScript {
  epoch = 'e1';
  messages: MessageDto[] = [message('user', 'hi'), message('assistant', 'hello')];
  records: RemoteSeqRecord[] = [];
  cursorSeq = 0;
  pending: RemotePending = NO_PENDING;
  running = false;
  firstIndex = 0;
  history: MessageDto[] = [];
  attaches: SessionAttachParams[] = [];
  calls: Array<{ method: string; params: any }> = [];
  failWith = new Map<string, string>();
  /** For a resume: the records after `after_seq`. */
  deltaFor: ((after: number) => RemoteSeqRecord[] | undefined) | undefined;

  constructor(h: Harness) {
    h.host.requestHandler = (method, params, conn) => this.handle(method, params as any, conn);
  }

  private handle(method: string, params: any, conn: HostConnection): unknown {
    this.calls.push({ method, params });
    const failure = this.failWith.get(method);
    if (failure) throw new RemoteRpcError(failure as never);
    switch (method) {
      case 'sessions.watch':
      case 'sessions.list':
        return { sessions: [{ session_id: SESSION, title: 'Fix‮ bug', project_label: 'app', status: 'idle', viewers: 0 }] };
      case 'session.attach': {
        this.attaches.push(params);
        const delta = params.resume && this.deltaFor?.(params.resume.after_seq);
        if (delta) {
          for (const item of delta) void conn.notify('session.event', { session_id: SESSION, ...item });
          return { mode: 'delta', epoch: this.epoch, pending: this.pending };
        }
        this.sendSnapshot(conn);
        return { mode: 'snapshot', epoch: this.epoch };
      }
      case 'session.history':
        return { messages: this.history, first_index: 0, has_more: false };
      case 'models.list':
        return { models: ['a', 'b'], current: 'a' };
      case 'prompt.send':
        return { turn_id: 42 };
      default:
        return {};
    }
  }

  sendSnapshot(conn: HostConnection): void {
    const to = (p: object) => void conn.notify('session.snapshot', { session_id: SESSION, ...p });
    to({ phase: 'begin', epoch: this.epoch, first_message_index: this.firstIndex });
    to({ phase: 'messages', messages: this.messages });
    to({ phase: 'records', records: this.records });
    to({ phase: 'end', cursor: { epoch: this.epoch, seq: this.cursorSeq }, pending: this.pending, turn: { running: this.running }, controls });
  }
}

async function open(h: Harness): Promise<{ client: RemoteClient; controller: SessionController; script: SessionScript; conn: () => HostConnection }> {
  await h.trustDevice();
  const connection = h.connect();
  const client = new RemoteClient(connection, { clock: h.clock });
  const script = new SessionScript(h);
  connection.start();
  await h.until(connection, 'ready');
  const controller = client.session(SESSION);
  return { client, controller, script, conn: () => [...h.host.connections.values()].at(-1)! };
}

const live = (h: Harness, controller: SessionController) => drive(h.clock, () => controller.getState().status === 'live', 'the session to go live');

test('a snapshot becomes the same rows the desktop shows, with prompts, extras and the pending set', async () => {
  const h = await harness();
  const { client, controller, script } = await open(h);
  script.records = [
    { seq: 1, record: { type: 'prompt', turn_id: 7, text: 'run ls', origin: 'device', device_name: 'Pixel‮ 8' } },
    event(2, { type: 'turn_started', turn_id: 7 }),
    event(3, { type: 'text_delta', text: 'Working' }),
    { seq: 4, record: { type: 'omitted', event_type: 'tool_use_result', size_bytes: 2_000_000 } },
  ];
  script.cursorSeq = 4;
  script.pending = { permissions: [permission(9)], questions: [], computer_access: [] };
  script.running = true;
  await drive(h.clock, () => client.getState().sessions !== undefined, 'the session list');
  assert.equal(client.getState().sessions![0]!.title, 'Fix bug', 'direction controls are stripped from titles');

  controller.activate();
  await live(h, controller);
  const state = controller.getState();
  const items = state.transcript.conversation.items;
  assert.deepEqual(items.map((i) => (i.type === 'narration' ? `${i.role}:${i.text}` : i.type)), ['user:hi', 'assistant:hello', 'user:run ls', 'assistant:Working']);
  const prompt = items[2]!;
  assert.deepEqual(state.transcript.promptMeta.get(prompt.id), { origin: 'device', deviceName: 'Pixel 8', turnId: 7 });
  assert.equal(state.transcript.extras.length, 1);
  assert.equal(state.transcript.extras[0]!.kind, 'omitted');
  assert.equal(state.transcript.extras[0]!.afterItemId, items.at(-1)!.id);
  assert.equal(state.transcript.conversation.running, true);
  assert.equal(state.turn.running, true);
  assert.equal(state.pending.permissions[0]!.request_id, 9);
  assert.equal(state.controls?.qualified_model, 'p/model-a');
  assert.deepEqual(script.attaches, [{ session_id: SESSION }], 'a first attach does not resume');
});

test('live records apply in order, repeats are ignored, and early ones wait for the snapshot', async () => {
  const h = await harness();
  const { controller, script, conn } = await open(h);
  script.records = [event(1, { type: 'turn_started' }), event(2, { type: 'text_delta', text: 'a' })];
  script.cursorSeq = 2;
  controller.activate();
  await live(h, controller);

  const send = (seq: number, e: object) => void conn().notify('session.event', { session_id: SESSION, ...event(seq, e) });
  send(2, { type: 'text_delta', text: 'a' }); // already in the snapshot
  send(3, { type: 'text_delta', text: 'b' });
  send(3, { type: 'text_delta', text: 'b' }); // a repeat
  send(4, { type: 'text_delta', text: 'c' });
  await drive(h.clock, () => JSON.stringify(controller.getState().transcript.conversation.items).includes('abc'), 'the live text');
  const texts = controller.getState().transcript.conversation.items.filter((i) => i.type === 'narration').map((i) => (i as any).text);
  assert.ok(texts.includes('abc'), texts.join('|'));
});

test('events that arrive before the snapshot ends are applied after it, minus what it covers', async () => {
  const h = await harness();
  const { controller, script, conn } = await open(h);
  script.records = [event(1, { type: 'turn_started' }), event(2, { type: 'text_delta', text: 'a' })];
  script.cursorSeq = 2;
  const original = script.sendSnapshot.bind(script);
  script.sendSnapshot = (c) => {
    // Two live records slip in ahead of the snapshot's end: one already covered, one new.
    void c.notify('session.event', { session_id: SESSION, ...event(2, { type: 'text_delta', text: 'a' }) });
    void c.notify('session.event', { session_id: SESSION, ...event(3, { type: 'text_delta', text: 'b' }) });
    original(c);
  };
  controller.activate();
  await live(h, controller);
  await drive(h.clock, () => JSON.stringify(controller.getState().transcript.conversation.items).includes('ab'), 'the held record');
  assert.ok(conn());
  assert.ok(!JSON.stringify(controller.getState().transcript.conversation.items).includes('aab'));
});

test('after a reconnect the session resumes from its last epoch and sequence', async () => {
  const h = await harness();
  const { controller, script, conn } = await open(h);
  script.records = [event(1, { type: 'turn_started' }), event(2, { type: 'text_delta', text: 'a' })];
  script.cursorSeq = 2;
  controller.activate();
  await live(h, controller);
  script.deltaFor = (after) => [event(after + 1, { type: 'text_delta', text: 'b' })];
  script.pending = { permissions: [permission(5)], questions: [], computer_access: [] };

  conn().silent = true; // the desktop goes quiet; the keepalive ends the link and the viewer re-dials
  await drive(h.clock, () => controller.getState().status === 'offline', 'the session to notice', { stepMs: 1000 });
  await drive(h.clock, () => script.attaches.length === 2, 'the second attach', { stepMs: 100 });
  assert.deepEqual(script.attaches[1], { session_id: SESSION, resume: { epoch: 'e1', after_seq: 2 } });
  await drive(h.clock, () => controller.getState().status === 'live' && controller.getState().pending.permissions.length === 1, 'the delta');
  assert.ok(JSON.stringify(controller.getState().transcript.conversation.items).includes('ab'));
});

test('a reset clears the session and a new snapshot replaces it', async () => {
  const h = await harness();
  const { controller, script, conn } = await open(h);
  controller.activate();
  await live(h, controller);
  assert.equal(controller.getState().transcript.conversation.items.length, 2);

  script.epoch = 'e2';
  script.messages = [message('user', 'fresh')];
  void conn().notify('session.reset', { session_id: SESSION });
  script.sendSnapshot(conn());
  await drive(h.clock, () => JSON.stringify(controller.getState().transcript.conversation.items).includes('fresh'), 'the new snapshot');
  assert.equal(controller.getState().transcript.conversation.items.length, 1);
  assert.equal(script.attaches.length, 1, 'no second attach while the desktop sends the snapshot itself');
});

test('a reset with no snapshot behind it asks for one', async () => {
  const h = await harness();
  const { controller, script, conn } = await open(h);
  controller.activate();
  await live(h, controller);
  void conn().notify('session.reset', { session_id: SESSION });
  await drive(h.clock, () => script.attaches.length === 2, 'the fallback attach', { stepMs: 500 });
  assert.deepEqual(script.attaches[1], { session_id: SESSION }, 'not a resume: the base changed');
});

test('permission answers go out as allow-once or deny, and losing the race is quiet', async () => {
  const h = await harness();
  const { controller, script } = await open(h);
  controller.activate();
  await live(h, controller);
  await controller.respondPermission(9, { type: 'allow_once' });
  await controller.respondPermission(10, { type: 'deny' });
  assert.deepEqual(script.calls.filter((c) => c.method === 'permission.respond').map((c) => c.params), [
    { session_id: SESSION, request_id: 9, decision: { type: 'allow_once' } },
    { session_id: SESSION, request_id: 10, decision: { type: 'deny' } },
  ]);
  script.failWith.set('permission.respond', 'not_pending');
  await controller.respondPermission(11, { type: 'allow_once' });
  assert.equal(controller.getState().answeredElsewhere, true);
  script.failWith.set('question.answer', 'forbidden');
  await assert.rejects(controller.answerQuestion(1, { q: 'a' }), (e: unknown) => e instanceof RemoteRpcError && e.code === 'forbidden');
});

test('question answers use the label, the typed text, or labels joined by commas', () => {
  const request = {
    request_id: 1,
    questions: [
      { question: 'Pick one', header: 'a', multi_select: false, options: [{ label: 'X', description: '' }] },
      { question: 'Pick some', header: 'b', multi_select: true, options: [{ label: 'P', description: '' }, { label: 'Q', description: '' }] },
      { question: 'Free', header: 'c', multi_select: false, options: [] },
      { question: 'Skipped', header: 'd', multi_select: false, options: [] },
    ],
  };
  const answers = questionAnswers(
    request,
    new Map([['Pick one', ['X']], ['Pick some', ['P', 'Q']]]),
    new Map([['Free', '  my own '], ['Pick some', 'R']]),
  );
  assert.deepEqual(answers, { 'Pick one': 'X', 'Pick some': 'P, Q, R', Free: 'my own' });
});

test('prompts: images only while idle, the echo is matched by turn id, failures can be retried with the same id', async () => {
  const h = await harness();
  const { controller, script, conn } = await open(h);
  controller.activate();
  await live(h, controller);
  const image = { media_type: 'image/png', base64: 'AAAA' };

  await controller.sendPrompt('look', [image]);
  const first = script.calls.find((c) => c.method === 'prompt.send')!.params;
  assert.deepEqual(first.images, [image]);
  assert.equal(controller.getState().outgoing[0]!.turnId, 42);
  void conn().notify('session.event', { session_id: SESSION, seq: 1, record: { type: 'prompt', turn_id: 42, text: 'look', origin: 'device', device_name: 'Browser' } });
  await drive(h.clock, () => controller.getState().outgoing.length === 0, 'the echo to replace the outgoing prompt');

  void conn().notify('session.event', { session_id: SESSION, seq: 2, record: { type: 'event', event: { type: 'turn_started', turn_id: 42 } } });
  await drive(h.clock, () => controller.getState().turn.running, 'the turn to start');
  script.calls.length = 0;
  script.failWith.set('prompt.send', 'internal');
  await controller.sendPrompt('more', [image]);
  assert.equal(script.calls[0]!.params.images, undefined, 'a running turn takes text only');
  assert.equal(controller.getState().outgoing[0]!.status, 'failed');
  const id = script.calls[0]!.params.client_request_id;
  script.failWith.clear();
  await controller.retryPrompt(controller.getState().outgoing[0]!.id);
  assert.equal(script.calls[1]!.params.client_request_id, id);
});

test('older messages are fetched in a page and the transcript is rebuilt around them', async () => {
  const h = await harness();
  const { controller, script } = await open(h);
  script.firstIndex = 2;
  script.history = [message('user', 'older'), message('assistant', 'oldest reply')];
  controller.activate();
  await live(h, controller);
  assert.equal(controller.getState().hasOlder, true);
  await controller.loadOlder();
  const texts = controller.getState().transcript.conversation.items.map((i) => (i as any).text);
  assert.deepEqual(texts, ['older', 'oldest reply', 'hi', 'hello']);
  assert.equal(controller.getState().hasOlder, false);
  assert.deepEqual(script.calls.find((c) => c.method === 'session.history')!.params, { session_id: SESSION, before_index: 2, limit: 100 });
});

test('controls: bypassPermissions is neither offered nor sent', async () => {
  const h = await harness();
  const { controller, script } = await open(h);
  controller.activate();
  await live(h, controller);
  assert.deepEqual(selectablePermissionModes(controller.getState().controls), ['default', 'acceptEdits']);
  await controller.setPermissionMode('bypassPermissions');
  await controller.setPermissionMode('acceptEdits');
  await controller.setModel('p/model-b');
  assert.deepEqual(script.calls.filter((c) => c.method.startsWith('controls.')).map((c) => [c.method, c.params]), [
    ['controls.set_permission_mode', { session_id: SESSION, mode: 'acceptEdits' }],
    ['controls.set_model', { session_id: SESSION, model: 'p/model-b' }],
  ]);
});

test('an unknown session is an error state, not a hang', async () => {
  const h = await harness();
  const { controller, script } = await open(h);
  script.failWith.set('session.attach', 'unknown_session');
  controller.activate();
  await drive(h.clock, () => controller.getState().status === 'error', 'the error');
  assert.equal(controller.getState().error?.code, 'unknown_session');
});

test('image-only prompts get a row, and image URLs are told apart from references', async () => {
  const h = await harness();
  const { controller, script } = await open(h);
  script.records = [{ seq: 1, record: { type: 'prompt', turn_id: 1, text: '', origin: 'desktop', images: [{ ref: 'abc', media_type: 'image/png' }, { ref: 'x', media_type: 'image/svg+xml' }] } }];
  script.cursorSeq = 1;
  controller.activate();
  await live(h, controller);
  const row = controller.getState().transcript.conversation.items.at(-1) as any;
  assert.equal(row.role, 'user');
  assert.deepEqual(row.images, [{ media_type: 'image/png', url: `${IMAGE_REF_SCHEME}abc` }], 'unsupported image types are dropped');
  assert.equal(imageRefOf(row.images[0].url), 'abc');
  assert.equal(imageRefOf('data:image/png;base64,AAAA'), undefined);
  assert.equal(imageRefOf('https://tracker.example/x.png'), 'https://tracker.example/x.png', 'a URL is never loaded, only sent back to the desktop as a reference');
});
