import assert from 'node:assert/strict';
import test from 'node:test';
import type { User } from 'firebase/auth';
import type { Firestore } from 'firebase/firestore';
import { webTeamsApi } from '../src/teams/api';

test('Teams closes an account before a delayed token can start a request', async context => {
  let resume: ((value: string) => void) | undefined, requests = 0;
  const user = { uid: 'synthetic-user', getIdToken: () => new Promise<string>(resolve => { resume = resolve; }) } as unknown as User;
  const api = webTeamsApi(user, {} as Firestore, () => 'synthetic-user');
  context.mock.method(globalThis, 'fetch', async () => { requests++; throw new Error('Unexpected network access'); });
  const request = api.execute({ action: 'create', teamId: 'synthetic-team', name: 'Fixture' });
  api.close(); resume!('synthetic-token'); await assert.rejects(request, /INVALID_SESSION/); assert.equal(requests, 0);
});
