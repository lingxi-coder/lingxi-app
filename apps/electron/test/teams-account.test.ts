import test from 'node:test';
import assert from 'node:assert/strict';
import { TeamsService } from '../src/main/teams/service';

test('Teams mutation is bound to the panel account across a token refresh', async context => {
  let uid = 'alice', changed: (() => void) | undefined, release!: (value: string) => void, calls = 0;
  const token = new Promise<string>(resolve => { release = resolve; });
  const account = { snapshot: () => ({ user: { uid } }), getIdToken: () => token,
    onChanged(fn: () => void) { changed = fn; return () => { changed = undefined; }; } };
  context.mock.method(globalThis, 'fetch', async () => { calls++; return Response.json({ ok: true }); });
  const service = new TeamsService(account, {});
  const operation = service.execute({ action: 'join', code: 'a'.repeat(32), requestId: 'request01' }, 'alice');
  uid = 'bob'; changed?.(); release('synthetic-token');
  await assert.rejects(operation, /INVALID_SESSION/u);
  assert.equal(calls, 0, 'must reject before sending a token from the replacement account');
  await assert.rejects(service.execute({ action: 'join', code: 'a'.repeat(32), requestId: 'request02' }, 'alice'), /INVALID_SESSION/u);
  assert.equal(calls, 0); service.close();
});
test('Teams response from a prior account is discarded', async context => {
  let uid = 'alice', changed: (() => void) | undefined, release!: (value: Response) => void, sent!: () => void;
  const sending = new Promise<void>(resolve => { sent = resolve; });
  const response = new Promise<Response>(resolve => { release = resolve; });
  const account = { snapshot: () => ({ user: { uid } }), getIdToken: async () => 'synthetic-token',
    onChanged(fn: () => void) { changed = fn; return () => {}; } };
  context.mock.method(globalThis, 'fetch', async () => { sent(); return response; });
  const service = new TeamsService(account, {});
  const operation = service.execute({ action: 'create', teamId: 'request01', name: 'Synthetic' }, 'alice');
  await sending; uid = 'bob'; changed?.(); release(Response.json({ teamId: 'request01' }));
  await assert.rejects(operation, /INVALID_SESSION/u); service.close();
});
