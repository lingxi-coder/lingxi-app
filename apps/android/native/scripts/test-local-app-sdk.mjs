import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

// Exercise the exact document-start script shipped by Android, with a fake native
// transport only. It does not substitute generated Kotlin or runtime bindings.
const source = readFileSync(new URL('../app/src/main/java/com/lingxi/code/apps/AppSdkScript.kt', import.meta.url), 'utf8');
const template = source.split('return """')[1].split('""".trimIndent()')[0];
function browser(mainFrame = true) {
  const messages = [];
  const config = { origin: 'http://127.0.0.1:24131', runId: 'run1', services: ['api'] };
  const bridge = { postMessage(value) { messages.push(JSON.parse(value)); } };
  const window = { lingxiNative: bridge };
  window.top = mainFrame ? window : {};
  vm.runInNewContext(template.replace('$configuration', JSON.stringify(config)), { window, lingxiNative: bridge });
  return { api: window.localApp, messages, bridge };
}

test('serviceUrl is synchronous and includes only this run and service', () => {
  const { api, messages } = browser();
  assert.equal(api.serviceUrl('api'), 'http://127.0.0.1:24131/__run/run1/__service/api/');
  assert.equal(api.serviceUrl('api', '/v1/hello%20world?q=a..b'), 'http://127.0.0.1:24131/__run/run1/__service/api/v1/hello%20world?q=a..b');
  assert.equal(messages.length, 0);
});

test('serviceUrl rejects unregistered services, escape paths and malformed paths', () => {
  const { api } = browser();
  assert.throws(() => api.serviceUrl('other'));
  for (const path of ['https://external.test/', '//evil', '/../x', '/%2e%2e/x', '/a/%2f/x', '/%5c/x', '/%00', '/#x', '/%zz']) {
    assert.throws(() => api.serviceUrl('api', path), undefined, path);
  }
});

test('socket URL and info wait for the verified native response', async () => {
  const { api, messages, bridge } = browser();
  const socket = api.serviceSocketUrl('api', '/events');
  assert.equal(typeof socket.then, 'function');
  assert.equal(messages[0].method, 'serviceSocketUrl');
  bridge.onmessage({ data: JSON.stringify({ id: messages[0].id, ok: true, value: 'ws://127.0.0.1:24131/__run/run1/__service/api/events' }) });
  assert.match(await socket, /^ws:/);
  const info = api.info();
  bridge.onmessage({ data: JSON.stringify({ id: messages[1].id, ok: false, error: 'Application run has stopped' }) });
  await assert.rejects(info, /stopped/);
});

test('native operations are explicit and public generic invoke is absent', async () => {
  const { api, messages, bridge } = browser();
  assert.equal(api.native, undefined);
  assert.equal(Object.isFrozen(api), true);
  const notify = api.notify('Ready');
  const share = api.share.text('Hello');
  const audio = api.microphone.record();
  assert.deepEqual(messages.map(({ method, args }) => [method, args]), [['notify', { body: 'Ready' }], ['share', { text: 'Hello' }], ['microphone.record', {}]]);
  for (const message of messages) bridge.onmessage({ data: JSON.stringify({ id: message.id, ok: true, value: null }) });
  await Promise.all([notify, share, audio]);
});

test('child frames receive no SDK', () => {
  assert.equal(browser(false).api, undefined);
});
