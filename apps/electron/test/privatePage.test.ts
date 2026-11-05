/**
 * Private pages: the transcript slot, the session URL, the webview guard,
 * and the bridge that serves the document. There is no public URL.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { MessageDto, PrivatePageServeDto } from '@lingxi/bridge-client';
import { conversationFromMessages, emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import { guardPrivatePageWebview, PrivatePageRouter, type PrivatePageBackend } from '../src/main/privatePage';
import { parsePrivatePageUrl, privatePageUrl, PRIVATE_PAGE_PARTITION } from '../src/shared/privatePage';

const SESSION = '01234567-89ab-cdef-0123-456789abcdef';

test('a page URL stays on this computer and rejects a public address', () => {
  const url = privatePageUrl(SESSION, 'notes', 2);
  assert.equal(url, `lingxi-page://page/${SESSION}/notes/2`);
  assert.deepEqual(parsePrivatePageUrl(url!), { sessionId: SESSION, id: 'notes', revision: 2 });
  assert.equal(privatePageUrl('not-a-session', 'notes', 1), null);
  assert.equal(privatePageUrl(SESSION, '../x', 1), null);
  assert.equal(privatePageUrl(SESSION, 'notes', 0), null);
  for (const bad of [
    'https://example.com/notes',
    'file:///tmp/notes.html',
    `lingxi-page://page/${SESSION}/notes/2?x=1`,
    `lingxi-page://page/${SESSION}/../notes/1`,
    `lingxi-page://page/${SESSION}/notes/0`,
    `lingxi-page://page/${SESSION}/notes/01`,
  ]) assert.equal(parsePrivatePageUrl(bad), null, bad);
});

test('a pending page settles in place and a discarded one leaves no row', () => {
  let state = reduceEvent(emptyConversation(), { type: 'turn_started', turn_id: 1 });
  state = reduceEvent(state, { type: 'text_delta', text: 'The page:' });
  state = reduceEvent(state, { type: 'page_block', status: 'pending' });
  state = reduceEvent(state, { type: 'page_block', status: 'ready', reference: { id: 'notes', revision: 1 } });
  assert.deepEqual(state.items.map((item) => item.type), ['narration', 'page']);
  const slot = state.items[1];
  assert.deepEqual(slot?.type === 'page' ? [slot.status, slot.reference] : null, ['ready', { id: 'notes', revision: 1 }]);

  let dropped = reduceEvent(emptyConversation(), { type: 'page_block', status: 'pending' });
  dropped = reduceEvent(dropped, { type: 'page_block', status: 'discarded' });
  assert.equal(dropped.items.length, 0);
  dropped = reduceEvent(reduceEvent(emptyConversation(), { type: 'page_block', status: 'pending' }), { type: 'message_complete' } as never);
  assert.equal(dropped.items.some((item) => item.type === 'page'), false);
});

test('replay restores a private page and not a visualization', () => {
  const messages = [{
    role: 'assistant',
    blocks: [
      { type: 'text', text: 'Notes:' },
      { type: 'page', reference: { id: 'notes', revision: 3 } },
      { type: 'page' },
    ],
  }] as unknown as MessageDto[];
  const items = conversationFromMessages(messages).items;
  assert.deepEqual(items.map((item) => item.type === 'page' ? item.status : item.type), ['narration', 'ready', 'unavailable']);
  assert.equal(items.some((item) => item.type === 'visualization'), false);
});

test('only a parsed page on its partition may attach', () => {
  let prevented = 0;
  const event = { preventDefault: () => { prevented += 1; } };
  const src = privatePageUrl(SESSION, 'notes', 1)!;
  assert.equal(guardPrivatePageWebview(event, {}, { src: 'https://example.com', partition: 'persist:main' }), false);
  assert.equal(prevented, 0);

  assert.equal(guardPrivatePageWebview(event, {}, { src: 'lingxi-page://page/bad', partition: PRIVATE_PAGE_PARTITION }), true);
  assert.equal(prevented, 1);

  const prefs: Record<string, unknown> = { nodeIntegration: true, preloadURL: 'file:///evil.js', sandbox: false };
  assert.equal(guardPrivatePageWebview(event, prefs, { src, partition: PRIVATE_PAGE_PARTITION }), true);
  assert.equal(prevented, 1);
  assert.equal(prefs['sandbox'], true);
  assert.equal(prefs['nodeIntegration'], false);
  assert.equal(prefs['partition'], PRIVATE_PAGE_PARTITION);
  assert.equal('preloadURL' in prefs, false);
  assert.equal('preload' in prefs, false);
});

test('the router serves through that session and misses an unknown one', async () => {
  const calls: string[] = [];
  const backend: PrivatePageBackend = {
    async pageServe(id, revision): Promise<PrivatePageServeDto> {
      calls.push(`${id}:${revision}`);
      return {
        status: 200,
        headers: [['Content-Type', 'text/html; charset=utf-8'], ['Content-Security-Policy', "default-src 'none'"]],
        body_base64: Buffer.from('<p>Hello</p>').toString('base64'),
      };
    },
  };
  const router = new PrivatePageRouter({ get: (id) => id === SESSION ? backend : undefined });
  const served = await router.serve(privatePageUrl(SESSION, 'notes', 4)!);
  assert.equal(served.status, 200);
  assert.equal(served.body.toString(), '<p>Hello</p>');
  assert.deepEqual(calls, ['notes:4']);
  assert.equal((await router.serve(privatePageUrl('aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee', 'notes', 4)!)).status, 404);
  assert.equal((await router.serve('https://example.com/notes')).status, 404);
  assert.equal((await router.serve('file:///tmp/notes.html')).body.toString(), 'not found');
});
