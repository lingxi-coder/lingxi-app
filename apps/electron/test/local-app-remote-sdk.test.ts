import { test } from 'node:test';
import assert from 'node:assert/strict';
import { runInNewContext } from 'node:vm';
import { LOCAL_APP_SDK, REMOTE_LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';
import { LOCAL_APP_RTC_BROWSER_SOURCE } from '../src/main/local-apps/remoteRtcBrowser.ts';

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
