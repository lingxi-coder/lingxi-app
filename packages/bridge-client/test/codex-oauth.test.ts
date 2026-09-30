import assert from 'node:assert/strict';
import { test } from 'node:test';
import { BridgeClient } from '../src/client.js';
import { validateClientEvent } from '../src/validation.js';

test('OAuth updates reach host listeners without remaining in async event queue', () => {
  const client = new BridgeClient();
  const event = { type: 'openai_oauth_updated', session: { access_token: 'private-access', refresh_token: 'private-refresh', expires_at: 123, fedramp: false } };
  const received: unknown[] = [];
  client.on('event', value => received.push(value));
  (client as any).onMessage(Buffer.from(JSON.stringify({ type: 'event', payload: event })));
  assert.deepEqual(received, [event]);
  assert.deepEqual((client as any).eventQueue, []);
});

test('OAuth validation normalizes nullable optional fields and rejects malformed sessions', () => {
  const session = { access_token: 'private-access', expires_at: 123, fedramp: false };
  assert.deepEqual(validateClientEvent({ type: 'openai_oauth_updated', session: { ...session, refresh_token: null, account_id: null } }), { type: 'openai_oauth_updated', session });
  assert.throws(() => validateClientEvent({ type: 'openai_oauth_updated', session: { ...session, expires_at: 'invalid' } }), /invalid OAuth/);
});
