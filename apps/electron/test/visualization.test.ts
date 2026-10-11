/**
 * Inline visualizations: the live/replay reducer, the main-process router's
 * token ownership, the `<webview>` guard and the IPC validators.
 */

import { test } from 'node:test';
import assert from 'node:assert/strict';

import type { MessageDto, VisualizationServeDto } from '@lingxi/bridge-client';
import { conversationFromMessages, emptyConversation, reduceEvent } from '../src/renderer/bridge/conversation';
import {
  guardVisualizationWebview,
  visualizationPath,
  VisualizationRouter,
  type VisualizationBackend,
} from '../src/main/visualization';
import {
  parseVisualizationReference,
  parseVisualizationTheme,
  VISUALIZATION_PARTITION,
  VISUALIZATION_SHELL_URL,
} from '../src/shared/visualization';

const TOKEN_A = 'a'.repeat(64);
const TOKEN_B = 'b'.repeat(64);
const THEME = { dark: false, tokens: {} };

test('a pending slot settles in place and closes the narration it interrupts', () => {
  let s = reduceEvent(emptyConversation(), { type: 'turn_started', turn_id: 1 });
  s = reduceEvent(s, { type: 'text_delta', text: 'Here is the chart:' });
  s = reduceEvent(s, { type: 'visualization_block', status: 'pending' });
  s = reduceEvent(s, { type: 'visualization_block', status: 'ready', reference: { id: 'chart', revision: 2 } });
  s = reduceEvent(s, { type: 'text_delta', text: 'Notice the dip.' });
  assert.deepEqual(s.items.map((item) => item.type), ['narration', 'visualization', 'narration']);
  const slot = s.items[1];
  assert.equal(slot?.type, 'visualization');
  assert.deepEqual(slot?.type === 'visualization' ? [slot.status, slot.reference] : null, ['ready', { id: 'chart', revision: 2 }]);
});

test('a discarded or abandoned placeholder leaves no row behind', () => {
  let s = reduceEvent(emptyConversation(), { type: 'turn_started', turn_id: 1 });
  s = reduceEvent(s, { type: 'visualization_block', status: 'pending' });
  assert.equal(reduceEvent(s, { type: 'visualization_block', status: 'discarded' }).items.length, 0);
  s = reduceEvent(s, { type: 'message_complete' } as never);
  assert.equal(s.items.some((item) => item.type === 'visualization'), false);
});

test('replay restores widgets and the follow-up chip on the user bubble', () => {
  const messages = [
    { role: 'assistant', blocks: [
      { type: 'text', text: 'Chart:' },
      { type: 'visualization', reference: { id: 'chart', revision: 1 } },
      { type: 'visualization' },
    ] },
    { role: 'user', blocks: [{ type: 'text', text: 'Why the dip?' }], visualization_context: { id: 'chart', revision: 1, title: 'Sales' } },
  ] as unknown as MessageDto[];
  const items = conversationFromMessages(messages).items;
  assert.deepEqual(items.map((item) => item.type === 'visualization' ? item.status : item.type), ['narration', 'ready', 'unavailable', 'narration']);
  const user = items[3];
  assert.deepEqual(user?.type === 'narration' ? user.visualizationContext : null, { id: 'chart', revision: 1, title: 'Sales' });
});

function backend(token: string, calls: string[]): VisualizationBackend {
  return {
    async visualizationMount() {
      return { token, generation: 1, doc_url: `lingxi-viz://visualization/doc/${token}`, title: 'Chart' };
    },
    async visualizationServe(path): Promise<VisualizationServeDto> {
      calls.push(`serve:${path}`);
      return { status: 200, headers: [['Content-Type', 'text/plain']], body_base64: Buffer.from(path).toString('base64') };
    },
    async visualizationWriteState(token) {
      calls.push(`write:${token}`);
      return { saved: true, version: 2 };
    },
    async visualizationUnmount(token) {
      calls.push(`unmount:${token}`);
    },
  };
}

test('documents and state writes route only to the session that minted the token', async () => {
  const calls: string[] = [];
  const runtimes = new Map([['s1', backend(TOKEN_A, calls)], ['s2', backend(TOKEN_B, calls)]]);
  const router = new VisualizationRouter({ get: (id) => runtimes.get(id), any: () => runtimes.get('s1') });
  const mount = await router.mount('s1', { id: 'chart', revision: 1 }, THEME, 'en', false);
  assert.equal(mount?.token, TOKEN_A);

  const doc = await router.serve(`lingxi-viz://visualization/doc/${TOKEN_A}`);
  assert.equal(doc.status, 200);
  assert.equal((await router.serve(`lingxi-viz://visualization/doc/${TOKEN_B}`)).status, 404);

  assert.equal((await router.writeState('s2', TOKEN_A, 1, 1, '{}', 'null')).reason, 'stale_mount');
  assert.equal((await router.writeState('s1', TOKEN_A, 1, 1, '{}', 'null')).saved, true);

  await router.unmount('s2', TOKEN_A);
  assert.equal(calls.includes(`unmount:${TOKEN_A}`), false);
  router.forgetSession('s1');
  assert.equal((await router.serve(`lingxi-viz://visualization/doc/${TOKEN_A}`)).status, 404);
});

test('a mount whose document is off-origin is refused', async () => {
  const rogue: VisualizationBackend = { ...backend(TOKEN_A, []), async visualizationMount() {
    return { token: TOKEN_A, generation: 1, doc_url: 'https://example.com/doc', title: 'x' };
  } };
  const router = new VisualizationRouter({ get: () => rogue, any: () => rogue });
  assert.equal(await router.mount('s1', { id: 'chart', revision: 1 }, THEME, 'en', false), null);
});

test('static assets are fetched once and unknown paths are 404', async () => {
  const calls: string[] = [];
  const only = backend(TOKEN_A, calls);
  const router = new VisualizationRouter({ get: () => only, any: () => only });
  await router.serve('lingxi-viz://visualization/shell.js');
  await router.serve('lingxi-viz://visualization/shell.js');
  assert.deepEqual(calls, ['serve:/shell.js']);
  for (const url of [
    'lingxi-viz://visualization/secret.txt',
    'lingxi-viz://visualization/asset/a%2Fb.js',
    'lingxi-viz://other/shell.js',
    'lingxi-viz://visualization/shell.js?x=1',
    'https://visualization/shell.js',
  ]) assert.equal((await router.serve(url)).status, 404, url);
  assert.equal(visualizationPath('not a url'), null);
});

test('only the shell on the visualization partition may attach, with forced preferences', () => {
  let prevented = 0;
  const event = { preventDefault: () => { prevented += 1; } };
  const prefs: Record<string, unknown> = {
    nodeIntegration: true, preloadURL: 'file:///evil.js', sandbox: false, partition: VISUALIZATION_PARTITION,
  };
  assert.equal(guardVisualizationWebview(event, prefs, { src: VISUALIZATION_SHELL_URL, partition: VISUALIZATION_PARTITION }, '/p.js', false), true);
  assert.equal(prevented, 0);
  assert.equal(prefs['nodeIntegration'], false);
  assert.equal(prefs['sandbox'], true);
  assert.equal(prefs['preload'], '/p.js');
  assert.equal('preloadURL' in prefs, false);
  // Electron routes the guest's session through this preference.
  assert.equal(prefs['partition'], VISUALIZATION_PARTITION);
  assert.equal(prefs['disablePopups'], true);

  for (const params of [
    { src: 'https://example.com', partition: VISUALIZATION_PARTITION },
    { src: VISUALIZATION_SHELL_URL, partition: 'persist:main' },
    { src: VISUALIZATION_SHELL_URL, partition: VISUALIZATION_PARTITION, allowpopups: 'true' },
    { src: VISUALIZATION_SHELL_URL, partition: VISUALIZATION_PARTITION, disablewebsecurity: 'true' },
  ]) assert.equal(guardVisualizationWebview(event, {}, params, '/p.js', false), false);
  assert.equal(prevented, 4);
});

test('IPC validators reject malformed references and drop unsafe theme tokens', () => {
  assert.deepEqual(parseVisualizationReference({ id: 'chart_1', revision: 3 }), { id: 'chart_1', revision: 3 });
  for (const bad of [null, { id: '../x', revision: 1 }, { id: 'x', revision: 0 }, { id: 'x', revision: 1.5 }]) {
    assert.throws(() => parseVisualizationReference(bad));
  }
  assert.deepEqual(parseVisualizationTheme({ dark: true, tokens: {
    '--color-text-primary': '#fff',
    '--bad': 'url(javascript:alert(1))',
    color: 'red',
    '--x': '}</style><script>',
  } }), { dark: true, tokens: { '--color-text-primary': '#fff' } });
});
