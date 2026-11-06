import { botMarketLink } from '../src/teams/market-link';
import productConfig from '../../../config/product-config.json';
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

test('Bot discovery reuses configured unified market and Desktop handoff', () => { const link = new URL(botMarketLink()); assert.equal(link.origin, new URL(productConfig.localAppMarketApiRoot).origin); assert.equal(link.pathname, '/'); assert.equal(link.searchParams.get('resourceKind'), 'bot'); assert.equal(link.searchParams.get('platform'), 'macos'); });
