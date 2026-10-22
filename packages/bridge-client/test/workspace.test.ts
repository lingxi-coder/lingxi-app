import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import { validateSessionWorkspaceState, validateSetWorkingDirectoryResult } from '../src/workspace.js';
import type { Frame } from '../src/protocol.js';

function wire(reply?: unknown) {
  const client = new BridgeClient();
  const internal = client as unknown as {
    ws: unknown; onMessage(data: Buffer): void; onClose(code: number, reason: string): void;
    pendingResponses: Map<number, unknown>;
  };
  let sent: Extract<Frame, { type: 'request' }> | undefined;
  internal.ws = { readyState: 1, send(value: string) {
    sent = JSON.parse(value);
    if (reply !== undefined) internal.onMessage(Buffer.from(JSON.stringify({
      type: 'response', payload: { id: sent!.payload.id, result: reply },
    })));
  } };
  return { client, internal, sent: () => sent };
}

test('set_cwd preserves canonical trust echo and validates the native result union', async () => {
  const request = { path: '../目录', trust_accepted: true, trusted_directory: '/目录' };
  for (const result of [
    { status: 'ok', cwd: '/目录', changed: true, transcript_relocated: true },
    { status: 'ok', cwd: '/目录', changed: false, transcript_relocated: false },
    { status: 'needs_trust', directory: '/目录', trust_root: '/' },
    { status: 'rejected', reason: 'busy', message: 'Busy' },
  ]) {
    const fixture = wire(result);
    assert.deepEqual(await fixture.client.setWorkingDirectory(request), result);
    assert.equal(fixture.sent()?.payload.method, 'set_cwd');
    assert.deepEqual(fixture.sent()?.payload.params, request);
    assert.equal(fixture.internal.pendingResponses.size, 0);
  }
});

test('workspace reads and directory additions use correlated private controls', async () => {
  const state = { cwd: '/project', additional_directories: ['/other', '/空间'],
    has_conversation: false, prompt_directories: ['/project/skills-only'] };
  const fixture = wire(state);
  assert.deepEqual(await fixture.client.getWorkspaceState(), state);
  assert.equal(fixture.sent()?.payload.method, 'workspace_state');
  assert.deepEqual(await fixture.client.addWorkingDirectory('../other'), state);
  assert.equal(fixture.sent()?.payload.method, 'add_directory');
  assert.deepEqual(fixture.sent()?.payload.params, { path: '../other' });
});

test('selected transcript resume retains the exact path and awaits its private acknowledgement', async () => {
  const fixture = wire({ status: 'ok' });
  const session = '11111111-2222-4333-8444-555555555555';
  await fixture.client.resumeSessionFromTranscript(session, '/store/选中.jsonl');
  assert.equal(fixture.sent()?.payload.method, 'resume_selected_session');
  assert.deepEqual(fixture.sent()?.payload.params, { session_id: session, transcript_path: '/store/选中.jsonl' });
  await assert.rejects(wire({ status: 'rejected' }).client.resumeSessionFromTranscript(session, '/other.jsonl'));
});

test('a resume interaction owner replaces the fixed RPC deadline and can abort its pending request', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const fixture = wire();
  const owner = new AbortController();
  let outcome = 'pending';
  const resume = fixture.client.resumeSessionFromTranscript('11111111-2222-4333-8444-555555555555', '/selected.jsonl', owner.signal)
    .then(() => { outcome = 'resolved'; }, () => { outcome = 'rejected'; });
  t.mock.timers.tick(30_001);
  await Promise.resolve();
  assert.equal(outcome, 'pending', 'a user considering a recovery approval must not hit the ordinary RPC timeout');
  assert.equal(fixture.internal.pendingResponses.size, 1);
  owner.abort();
  await resume;
  assert.equal(outcome, 'rejected');
  assert.equal(fixture.internal.pendingResponses.size, 0);
  fixture.internal.onMessage(Buffer.from(JSON.stringify({ type: 'response', payload: {
    id: fixture.sent()!.payload.id, result: { status: 'ok' },
  } })));
  assert.equal(outcome, 'rejected', 'a late acknowledgement cannot revive an aborted owner');

  const alreadyAborted = wire();
  await assert.rejects(alreadyAborted.client.resumeSessionFromTranscript('id', '/selected.jsonl', owner.signal), /aborted/);
  assert.equal(alreadyAborted.sent(), undefined);
});

test('workspace controls reject malformed replies and disconnect without replaying a move', async () => {
  for (const value of [null, [], {}, { status: 'ok', cwd: '/x', changed: true },
    { status: 'needs_trust', directory: '' }, { status: 'rejected', reason: 'busy' }]) {
    assert.throws(() => validateSetWorkingDirectoryResult(value));
  }
  for (const value of [{ cwd: '/x' }, { cwd: '/x', additional_directories: ['a\0b'] },
    { cwd: '/x', additional_directories: [], has_conversation: 'false' },
    { cwd: '/x', additional_directories: [], prompt_directories: ['/bad\0root'] }]) {
    assert.throws(() => validateSessionWorkspaceState(value));
  }
  const fixture = wire();
  const pending = fixture.client.setWorkingDirectory({ path: '/next' });
  const rejected = assert.rejects(pending, /connection closed/);
  fixture.internal.onClose(1006, 'disconnected');
  await rejected;
  assert.equal(fixture.internal.pendingResponses.size, 0);
});
