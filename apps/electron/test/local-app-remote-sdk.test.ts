import { test } from 'node:test';
import assert from 'node:assert/strict';
import { File } from 'node:buffer';
import { webcrypto } from 'node:crypto';
import { runInNewContext } from 'node:vm';
import { LOCAL_APP_SDK, REMOTE_LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';
import { LOCAL_APP_RTC_BROWSER_SOURCE } from '../src/main/local-apps/remoteRtcBrowser.ts';
import { chatCreationIndexedDb } from './support/chat-creation-indexeddb.ts';

const remoteAppId = `a${'1'.repeat(32)}`;
const remoteOrigin = `https://${remoteAppId}-${'c'.repeat(20)}.desktop.apps.example`;
const creation = { requestId: 'e2222222-2222-4222-8222-222222222222', title: '  Remote plan  ' };
const chatSummary = (input: { requestId: string; title?: string }) => ({
  chatId: input.requestId, title: input.title?.trim() ?? 'New chat', createdAt: 1, updatedAt: 1, archived: false, state: 'idle',
});

// Execute the complete published browser bundle and its actual remote bridge.
// The mock stops at HTTP: it preserves the same JSON resource envelopes as Host.
function remotePage(options: {
  db?: ReturnType<typeof chatCreationIndexedDb>;
  origin?: string;
  id?: string;
  revision?: number;
  invoke?: (method: string, args: any) => Promise<unknown>;
} = {}) {
  const origin = options.origin ?? remoteOrigin;
  const calls: Array<{ method: string; args: any }> = [];
  const sandbox: Record<string, any> = {
    Request, Response, Headers, URL, File, ArrayBuffer, TextEncoder, TextDecoder, AbortController,
    crypto: webcrypto, btoa: globalThis.btoa, setTimeout, clearTimeout,
    indexedDB: options.db ?? chatCreationIndexedDb(),
    location: new URL(origin), navigator: {}, document: {},
    dispatchEvent: () => true,
    CustomEvent: class { constructor(public type: string, public options?: unknown) {} },
    fetch: async (input: Request | string, init?: RequestInit) => {
      const request = new Request(input instanceof Request ? input : new URL(input, origin), init);
      assert.equal(new URL(request.url).pathname, '/__lingxi_remote_invoke');
      const { method, args } = await request.json() as { method: string; args: any };
      calls.push({ method, args });
      const result = method === 'info'
        ? { id: options.id ?? remoteAppId, name: 'Remote plan', revision: options.revision ?? 1, platform: 'browser' }
        : await options.invoke?.(method, args);
      return Response.json({ result });
    },
  };
  sandbox.window = sandbox;
  sandbox.top = sandbox;
  sandbox.structuredClone = runInNewContext('(value) => JSON.parse(JSON.stringify(value))', sandbox);
  const inPage = runInNewContext('(value) => JSON.parse(value)', sandbox) as (value: string) => any;
  runInNewContext(REMOTE_LOCAL_APP_SDK, sandbox);
  return { api: sandbox.localApp as any, calls, inPage: (value: unknown) => inPage(JSON.stringify(value)) };
}

test('remote fetch preserves Request bodies and omit keeps only host authentication', async () => {
  const requests: Request[] = [];
  const window: any = { fetch: async (request: Request) => { requests.push(request); return new Response('ok'); } };
  window.top = window;
  const remoteBridge = REMOTE_LOCAL_APP_SDK.slice(0, -LOCAL_APP_SDK.length - 1);
  runInNewContext(remoteBridge, { window, location:{origin:'https://app.example'}, Request, Response, Headers, URL, navigator:{}, document:{}, Object, Promise, Error, TypeError, Array });
  const ordinary = new Request('https://app.example/api',{method:'POST',body:'ordinary'});
  await window.fetch(ordinary);
  assert.equal(await requests[0]!.text(),'ordinary');
  assert.equal(requests[0]!.credentials,'same-origin');
  const omitted = new Request('https://app.example/api',{method:'POST',body:'private',credentials:'omit'});
  await window.fetch(omitted);
  assert.equal(await requests[1]!.text(),'private');
  assert.equal(requests[1]!.credentials,'same-origin');
  assert.equal(requests[1]!.headers.get('x-lingxi-local-app-credentials'),'omit');
  const outside = new Request('https://other.example/api',{method:'POST',body:'outside',credentials:'omit'});
  await window.fetch(outside);
  assert.equal(await requests[2]!.text(),'outside');
  assert.equal(requests[2]!.credentials,'omit');
});

test('complete SDK and RTC fallback preserve an authenticated omit POST', async () => {
  const requests: Request[] = [];
  const window:any = {fetch:async (input:Request | string, init?:RequestInit)=>{requests.push(new Request(input,init));return new Response('ok');}};
  window.top=window; window.location={origin:'https://app.example',href:'https://app.example/',protocol:'https:',host:'app.example'};
  const globals = {window,location:window.location,Request,Response,Headers,URL,navigator:{},document:{},Object,Promise,Error,TypeError,Array,console,setTimeout,clearTimeout,TextEncoder,TextDecoder,AbortController,CustomEvent:class {}};
  runInNewContext(REMOTE_LOCAL_APP_SDK + '\n' + LOCAL_APP_RTC_BROWSER_SOURCE,globals);
  await window.fetch(new Request('https://app.example/api',{method:'POST',body:'payload',credentials:'omit'}));
  assert.equal(await requests[0]!.text(),'payload');
  assert.equal(requests[0]!.credentials,'same-origin');
  assert.equal(requests[0]!.headers.get('x-lingxi-local-app-credentials'),'omit');
});

test('remote file picker validates limits before reading and propagates read failures', async () => {
  let selected: any[] = [], input: any, reads=0;
  const window: any={fetch:async()=>new Response('ok')};window.top=window;
  const document={createElement:()=>{ input={files:selected,click(){queueMicrotask(()=>input.onchange?.());}};return input; }};
  runInNewContext(REMOTE_LOCAL_APP_SDK.slice(0,-LOCAL_APP_SDK.length-1),{window,location:{origin:'https://app.example'},Request,Response,Headers,URL,navigator:{},document,Object,Promise,Error,TypeError,Array,Number});
  const file=(size:number, read:()=>Promise<ArrayBuffer> = async()=>{reads++;return new ArrayBuffer(size);})=>({name:'image.png',type:'image/png',size,lastModified:1,arrayBuffer:read});
  selected=[file(25*1024*1024+1)];
  await assert.rejects(window.__lingxiLocalAppBridge.files.pick({multiple:true}),/size limit/);assert.equal(reads,0);assert.equal(input.onchange,null);
  selected=[file(20*1024*1024),file(20*1024*1024),file(11*1024*1024)];
  await assert.rejects(window.__lingxiLocalAppBridge.files.pick({multiple:true}),/size limit/);assert.equal(reads,0);
  selected=Array.from({length:9},()=>file(1));
  await assert.rejects(window.__lingxiLocalAppBridge.files.pick({multiple:true}),/Too many/);assert.equal(reads,0);
  selected=[file(1,async()=>{throw new Error('read failed');})];
  await assert.rejects(window.__lingxiLocalAppBridge.files.pick({multiple:false}),/read failed/);assert.equal(input.onchange,null);assert.equal(input.oncancel,null);
});


test('remote service socket URLs reject paths that escape their service prefix', async () => {
  const window: any = {fetch: async () => new Response('ok')}; window.top = window;
  const location = {origin:'https://app.example',protocol:'https:',host:'app.example'};
  runInNewContext(REMOTE_LOCAL_APP_SDK.slice(0,-LOCAL_APP_SDK.length-1), {window,location,Request,Response,Headers,URL,navigator:{},document:{},Object,Promise,Error,TypeError,Array});
  const socket = window.__lingxiLocalAppBridge.serviceSocketUrl;
  assert.equal(await socket('api','/ws?topic=1'),'wss://app.example/__services/api/ws?topic=1');
  for (const path of ['/../other/ws','/%2e%2e/other/ws','/x%2f..%2fother','/x%5cother','/%00']) {
    await assert.rejects(socket('api',path), /Invalid service URL/);
  }
});

test('remote bridge forwards the complete Chat creation contract without dropping uncertainty', async () => {
  const calls: Array<{ method: string; args: unknown }> = [];
  const unavailable = { chats: [], hasUncertainChats: true, unavailableChats: [{ chatId: creation.requestId, code: 'chat_storage_uncertain' }], nextCursor: 'opaque-next' };
  const sandbox: any = { Request, Response, Headers, URL, navigator: {}, document: {}, location: new URL(remoteOrigin) };
  sandbox.window = sandbox; sandbox.top = sandbox;
  sandbox.fetch = async (_input: unknown, init: RequestInit) => {
    const { method, args } = JSON.parse(String(init.body));
    calls.push({ method, args });
    return Response.json({ result: { ok: true, value: method.endsWith('.list') ? unavailable : chatSummary(args) } });
  };
  runInNewContext(REMOTE_LOCAL_APP_SDK.slice(0, -LOCAL_APP_SDK.length - 1), sandbox);
  const chats = sandbox.__lingxiLocalAppBridge.agent.chats;
  assert.equal(typeof chats.creationStatus, 'function');
  assert.deepEqual(await chats.create(creation), { ok: true, value: chatSummary(creation) });
  assert.deepEqual(await chats.creationStatus(creation), { ok: true, value: chatSummary(creation) });
  assert.deepEqual(await chats.list({ limit: 1, cursor: 'opaque-cursor' }), { ok: true, value: unavailable });
  assert.deepEqual(calls, [
    { method: 'agent.chats.create', args: creation },
    { method: 'agent.chats.creationStatus', args: creation },
    { method: 'agent.chats.list', args: { limit: 1, cursor: 'opaque-cursor' } },
  ]);
});

test('remote UUID creation uses the authenticated App identity and resolves a lost acknowledgement explicitly', async () => {
  const db = chatCreationIndexedDb();
  const chats = new Map<string, ReturnType<typeof chatSummary>>();
  let submissions = 0;
  const invoke = async (method: string, input: any) => {
    if (method === 'agent.chats.create') {
      submissions++;
      if (!chats.has(input.requestId)) chats.set(input.requestId, chatSummary(input));
      if (submissions === 1) throw new Error('lost response after durable host admission');
      return { ok: true, value: chats.get(input.requestId) };
    }
    if (method === 'agent.chats.creationStatus') return { ok: true, value: chats.get(input.requestId) ?? null };
    if (method === 'agent.chats.list') return { ok: true, value: { chats: [...chats.values()], hasUncertainChats: true, unavailableChats: [{ chatId: 'f3333333-3333-4333-8333-333333333333', code: 'chat_storage_uncertain' }] } };
    throw new Error(`Unexpected method ${method}`);
  };
  const first = remotePage({ db, invoke });
  await assert.rejects(first.api.agent.chats.create(creation), { code: 'host_unavailable', requestId: creation.requestId, definitive: false });
  assert.equal(submissions, 1);
  const refreshed = remotePage({ db, invoke });
  assert.deepEqual(JSON.parse(JSON.stringify(await refreshed.api.agent.chats.pendingCreations())), [{ ...creation, state: 'pending' }]);
  assert.equal(submissions, 1, 'refresh must not replay a mutation');
  await assert.rejects(refreshed.api.agent.chats.forgetCreation(creation.requestId), { code: 'chat_creation_unresolved' });
  assert.equal((await refreshed.api.agent.chats.creationStatus(creation)).id, creation.requestId);
  assert.equal(submissions, 1, 'status is a read, including after an acknowledgement is lost');
  const [one, two] = await Promise.all([refreshed.api.agent.chats.create(creation), refreshed.api.agent.chats.create(creation)]);
  assert.deepEqual([one.id, two.id], [creation.requestId, creation.requestId]);
  assert.equal(submissions, 2, 'one explicit retry may dispatch only once per page');
  assert.equal(chats.size, 1);
  const list = await refreshed.api.agent.chats.list({ limit: 10 });
  assert.equal(list.hasUncertainChats, true);
  assert.deepEqual(JSON.parse(JSON.stringify(list.unavailableChats)), [{ chatId: 'f3333333-3333-4333-8333-333333333333', code: 'chat_storage_uncertain' }]);
  await refreshed.api.agent.chats.forgetCreation(creation.requestId);
  assert.deepEqual(JSON.parse(JSON.stringify(await refreshed.api.agent.chats.pendingCreations())), []);
});

test('remote creation receipts separate App revisions while a new browser origin recovers from the host', async () => {
  const db = chatCreationIndexedDb();
  const invoke = async (method: string) => {
    if (method === 'agent.chats.create') return { ok: false, error: { code: 'host_unavailable', message: 'offline' } };
    if (method === 'agent.chats.creationStatus') return { ok: true, value: chatSummary(creation) };
    throw new Error(`Unexpected method ${method}`);
  };
  const original = remotePage({ db, invoke });
  await assert.rejects(original.api.agent.chats.create(creation), { code: 'host_unavailable' });
  const revision = remotePage({ db, revision: 2, invoke });
  const other = remotePage({ db, id: `a${'2'.repeat(32)}`, origin: `https://a${'2'.repeat(32)}-${'d'.repeat(20)}.desktop.apps.example`, invoke });
  assert.deepEqual(JSON.parse(JSON.stringify(await revision.api.agent.chats.pendingCreations())), []);
  assert.deepEqual(JSON.parse(JSON.stringify(await other.api.agent.chats.pendingCreations())), []);
  // IndexedDB belongs to an origin. A fresh nonce gets fresh browser storage;
  // only explicit host reads recover existing Chat resources there.
  const reopened = remotePage({ origin: `https://${remoteAppId}-${'e'.repeat(20)}.desktop.apps.example`, invoke });
  assert.deepEqual(JSON.parse(JSON.stringify(await reopened.api.agent.chats.pendingCreations())), []);
  assert.equal((await reopened.api.agent.chats.creationStatus(creation)).id, creation.requestId);
  assert.deepEqual(JSON.parse(JSON.stringify(await reopened.api.agent.chats.pendingCreations())), []);
  assert.equal(reopened.calls.some(call => call.method === 'agent.chats.create'), false);
  assert.deepEqual(JSON.parse(JSON.stringify(await original.api.agent.chats.pendingCreations())), [{ ...creation, state: 'pending' }]);
});

test('remote malformed Chat creation never dispatches and uncertain host errors preserve receipts', async () => {
  const page = remotePage({ invoke: async () => ({ ok: false, error: { code: 'chat_storage_uncertain', message: 'Cannot confirm Chat storage' } }) });
  for (const input of [{ title: 'Old contract' }, { requestId: creation.requestId.toUpperCase() }, { ...creation, extra: true }]) {
    await assert.rejects(page.api.agent.chats.create(input), { code: 'invalid_chat' });
  }
  assert.equal(page.calls.some(call => call.method.startsWith('agent.')), false);
  await assert.rejects(page.api.agent.chats.create(creation), { code: 'chat_storage_uncertain', definitive: false, requestId: creation.requestId });
  await assert.rejects(page.api.agent.chats.creationStatus(creation), { code: 'chat_storage_uncertain' });
  await assert.rejects(page.api.agent.chats.forgetCreation(creation.requestId), { code: 'chat_creation_unresolved' });
  assert.deepEqual(JSON.parse(JSON.stringify(await page.api.agent.chats.pendingCreations())), [{ ...creation, state: 'pending' }]);
});
