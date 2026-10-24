import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';

// Exercise the exact document-start script shipped by Android, with a fake native
// transport only. It does not substitute generated Kotlin or runtime bindings.
const source = readFileSync(new URL('../app/src/main/java/com/lingxi/code/apps/AppSdkScript.kt', import.meta.url), 'utf8');
const template = source.split('return """')[1].split('""".trimIndent()')[0];
function browser(mainFrame = true, platform = 'android') {
  const timers = new Map(); let nextTimer = 0;
  const messages = [];
  const config = { origin: 'http://127.0.0.1:24131', runId: 'run1', services: ['api'] };
  const bridge = { postMessage(value) { messages.push(JSON.parse(value)); } };
  const window = { lingxiNative: bridge };
  window.top = mainFrame ? window : {};
  let script = template.replace('$configuration', JSON.stringify(config));
  if (platform === 'ios') {
    const ios = readFileSync(new URL('../../../ios/native/Sources/Apps/LocalAppBrowser.swift', import.meta.url), 'utf8');
    const start = ios.indexOf('(() => { const base = ');
    const end = ios.indexOf('"""', start);
    assert(start >= 0 && end > start, 'real iOS SDK source must be present');
    script = ios.slice(start, end).replace(String.raw`\(base)`, JSON.stringify(config.origin + '/__run/run1/__service/'))
      .replace(String.raw`\(names)`, JSON.stringify(config.services)).replaceAll('\\\\', '\\');
    let id = 0; const pending = new Map();
    window.webkit = {messageHandlers:{localApp:{postMessage: value => new Promise((resolve,reject) => {
      const key = String(++id); messages.push({id:key,...value}); pending.set(key,{resolve,reject});
    })}}};
    bridge.onmessage = event => { const value = JSON.parse(event.data); const item = pending.get(value.id); pending.delete(value.id);
      if (value.ok) item.resolve(value.value); else item.reject(new Error(value.error)); };
  }
  vm.runInNewContext(script, { window, lingxiNative: bridge, URL, console, setTimeout: fn => { timers.set(++nextTimer, fn); return nextTimer; }, clearTimeout: id => timers.delete(id) });
  return { api: window.localApp, messages, bridge, timers };
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

const flush = () => new Promise(resolve => setImmediate(resolve));
function reply(bridge, message, value) {
  bridge.onmessage({data: JSON.stringify({id: message.id, ok: true, value})});
}
for (const platform of ['android', 'ios']) {
test(`${platform}: Agent exposes only task operations and transports image and schema inputs`, async () => {
  const {api, messages, bridge} = browser(true, platform);
  assert.deepEqual(Object.keys(api.agent).sort(), ['cancel', 'get', 'run', 'subscribe']);
  assert.equal(Object.isFrozen(api.agent), true);
  const input = {requestId:'photo-1', prompt:'Estimate', images:[{media_type:'image/png',base64:'AA=='}], outputSchema:{type:'object'}};
  const promise = api.agent.run(input);
  assert.equal(messages[0].method, 'agent.run');
  assert.deepEqual(messages[0].args, input);
  reply(bridge, messages[0], {runId:'task1'});
  assert.equal((await promise).runId, 'task1');
});
test(`${platform}: Agent reconnect replays current sequence and stops polling on terminal state`, async () => {
  const {api, messages, bridge, timers} = browser(true, platform);
  const events = [];
  api.agent.subscribe('task1', value => events.push(value));
  reply(bridge, messages[0], {runId:'task1',sequence:3,state:'running',updatedAt:12});
  await flush();
  assert.equal(events.length, 1);
  const next = [...timers.values()][0]; timers.clear(); next();
  reply(bridge, messages[1], {runId:'task1',sequence:3,state:'running',updatedAt:12});
  await flush(); assert.equal(events.length, 1);
  const finish = [...timers.values()][0]; timers.clear(); finish();
  reply(bridge, messages[2], {runId:'task1',sequence:4,state:'succeeded',updatedAt:13,result:{ok:true}});
  await flush();
  assert.equal(events.length, 2); assert.equal(timers.size, 0);
});
test(`${platform}: Agent unsubscribe suppresses a late native reply and rescheduling`, async () => {
  const {api, messages, bridge, timers} = browser(true, platform);
  let calls = 0;
  const unsubscribe = api.agent.subscribe('task1', () => calls++);
  unsubscribe();
  reply(bridge, messages[0], {runId:'task1',sequence:1,state:'running'});
  await flush();
  assert.equal(calls, 0); assert.equal(timers.size, 0);
});

}
