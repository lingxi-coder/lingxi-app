import assert from 'node:assert/strict';
import { test } from 'node:test';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { createHash, randomUUID } from 'node:crypto';
import vm from 'node:vm';
import { originOf, permitsNavigation, permittedCdpMethod, keyDescription } from '../browser-extension/policy.js';

function event() { const listeners: Function[] = []; return { listeners,addListener:(listener:Function) => listeners.push(listener),emit:(...args:unknown[]) => listeners.forEach(listener => listener(...args)) }; }
function worker() {
  const incoming = event(); const disconnected = event(); const debuggerEvents = event();
  const tabs = new Map<number,any>(); const contexts = new Map<number,vm.Context>(); const replies = new Map<string,(message:any)=>void>();
  const effects: any[] = []; const commands:any[] = [];
  let tabIndex = 0; let contextIndex = 0;
  let evaluationTimeout = false; let abortedDownloads = 0;
  const downloadFetch = (...args:any[]) => evaluationTimeout ? new Promise((_resolve,reject) => {
    args[1].signal.addEventListener('abort',() => { abortedDownloads++; reject(new Error('aborted')); },{ once:true });
  }) : fetch(args[0],args[1]);
  const debuggerAttached = new Set<number>();
  const attachCalls:number[] = [];
  let setupGate: { method:string,arrived:()=>void,wait:Promise<void> } | undefined;
  const pauseSetup = async (method:string) => { const gate = setupGate; if (gate?.method === method) { setupGate = undefined; gate.arrived(); await gate.wait; } };
  let cssViewport = { clientWidth:1000,clientHeight:700,offsetX:0,offsetY:0,pageX:0,pageY:0,scale:1 };
  let holdTab: number | undefined; let release: (() => void) | undefined; let held: (() => void) | undefined;
  const heldPromise = new Promise<void>(resolve => { held = resolve; });
  class Reader {
    result?: string; onload?:()=>void; onerror?:(error:unknown)=>void;
    readAsDataURL(blob:Blob) { void blob.arrayBuffer().then(bytes => { this.result = `data:${blob.type};base64,${Buffer.from(bytes).toString('base64')}`; this.onload?.(); }).catch(error => this.onerror?.(error)); }
  }
  const chrome:any = {
    runtime:{ id:'nciganheacfkdbpdfdnjnjdkcabfbkmg',onStartup:event(),onInstalled:event(),connectNative:() => ({ onMessage:incoming,onDisconnect:disconnected,disconnect:()=>disconnected.emit(),postMessage:(message:any) => { if (message.id) { replies.get(message.id)?.(message); replies.delete(message.id); } } }) },
    alarms:{ create:()=>undefined,onAlarm:event() },action:{ onClicked:event() },
    tabs:{ onRemoved:event(),create:async({url}:any) => { const tab = { id:++tabIndex,url,status:'complete' }; tabs.set(tab.id,tab); return tab; },get:async(id:number) => { if (!tabs.has(id)) throw new Error('Missing tab'); return tabs.get(id); },group:async()=>1 },
    tabGroups:{ update:async()=>undefined },
    downloads:{ download:async(input:any) => { effects.push(input); return 71; } },
    debugger:{ onEvent:debuggerEvents,onDetach:event(),attach:async(target:any)=>{ attachCalls.push(target.tabId); await pauseSetup('attach'); debuggerAttached.add(target.tabId); },detach:async(target:any)=>{ await pauseSetup('detach'); debuggerAttached.delete(target.tabId); },
      sendCommand:async(target:any,method:string,params:any = {}) => {
        commands.push({ target,method,params });
        await pauseSetup(method);
        if (method === 'Page.getFrameTree') return { frameTree:{ frame:{ id:'frame',url:tabs.get(target.tabId).url } } };
        if (method === 'Page.getLayoutMetrics') return { cssVisualViewport:{ ...cssViewport } };
        if (method === 'Page.captureScreenshot') return { data:'controlled-image' };
        if (method === 'Page.navigate') { tabs.get(target.tabId).url = params.url; return {}; }
        if (method === 'Page.createIsolatedWorld') {
          const id = ++contextIndex;
          contexts.set(id,vm.createContext({ location:{ origin:new URL(tabs.get(target.tabId).url).origin },fetch:downloadFetch,AbortController,Blob,FileReader:Reader,URL }));
          return { executionContextId:id };
        }
        if (method === 'Runtime.evaluate') {
          try {
            const evaluation = vm.runInContext(params.expression,contexts.get(params.contextId)!);
            if (evaluationTimeout && params.awaitPromise) { void Promise.resolve(evaluation).catch(() => undefined); return { exceptionDetails:{ text:'Runtime evaluation timed out' } }; }
            return { result:{ value:await evaluation } };
          }
          catch (error) { return { exceptionDetails:{ text:error instanceof Error ? error.message : String(error) } }; }
        }
        if (method === 'Input.insertText' && target.tabId === holdTab) { held?.(); await new Promise<void>(resolve => { release = resolve; }); }
        return {};
      },
    },
  };
  const source = readFileSync(new URL('../browser-extension/worker.js',import.meta.url),'utf8').replace(/^import[^\n]*\n/,'');
  vm.runInNewContext(source,{ chrome,crypto:{ randomUUID },URL,setTimeout,clearTimeout,originOf,permitsNavigation,permittedCdpMethod,keyDescription,console });
  const command = (payload:any,revision = 'one') => {
    const id = randomUUID(); const result = new Promise<any>(resolve => replies.set(id,resolve));
    incoming.emit({ type:'command',id,revision,payload }); return result;
  };
  const policy = (origin:string) => ({ revision:'one',sites:[],allowedOrigins:[origin],defaultBrowsing:'ask',fullCdp:false,siteTools:true });
  const holdSetup = (method:string) => {
    let arrived!:()=>void; let release!:()=>void;
    const held = new Promise<void>(resolve => { arrived = resolve; });
    const wait = new Promise<void>(resolve => { release = resolve; });
    setupGate = { method,arrived,wait }; return { held,release };
  };
  return { command,policy,holdSetup,debuggerAttached,attachCalls,timeoutDownloads:()=>{ evaluationTimeout = true; },abortedDownloads:()=>abortedDownloads,viewport:(value:typeof cssViewport)=>{ cssViewport = value; },nativeEvent:(source:any,method:string,params:any)=>debuggerEvents.emit(source,method,params),controls:(value:any)=>incoming.emit(value),reconnect:()=>chrome.action.onClicked.emit(),effects,commands,hold:(tabId:number)=>{ holdTab = tabId; return heldPromise; },release:()=>release?.() };
}

test('actual worker download path never follows a denied redirect and only saves materialized data URLs',async () => {
  let blockedRequests = 0;
  const blockedServer = createServer((_request,response) => { blockedRequests++; response.end('secret'); });
  await new Promise<void>(resolve => blockedServer.listen(0,'127.0.0.1',resolve));
  const blockedOrigin = `http://127.0.0.1:${(blockedServer.address() as {port:number}).port}`;
  const server = createServer((request,response) => {
    if (request.url === '/redirect') { response.writeHead(302,{ location:blockedOrigin+'/private' }); response.end(); }
    else { response.writeHead(200,{ 'content-type':'text/plain' }); response.end('approved'); }
  });
  await new Promise<void>(resolve => server.listen(0,'127.0.0.1',resolve));
  const address = server.address() as { port:number }; const origin = `http://127.0.0.1:${address.port}`;
  try {
    const w = worker();
    const policy = { ...w.policy(origin),sites:[{ origin:blockedOrigin,browsing:'deny',downloads:'deny' }] };
    const navigation = await w.command({ action:'navigate',sessionId:'one',url:origin+'/page',expectedOrigin:origin,policy });
    assert.ok(navigation.result);
    const rejected = await w.command({ action:'download',sessionId:'one',url:origin+'/redirect',expectedOrigin:origin,policy });
    assert.ok(rejected.error); assert.equal(blockedRequests,0); assert.equal(w.effects.length,0);
    const saved = await w.command({ action:'download',sessionId:'one',url:origin+'/file',expectedOrigin:origin,policy });
    assert.equal(saved.result.downloadId,71);
    assert.equal(w.effects.length,1); assert.match(w.effects[0].url,/^data:text\/plain;base64,/); assert.equal(w.effects[0].saveAs,true);
  } finally { server.closeAllConnections(); blockedServer.closeAllConnections(); await Promise.all([new Promise<void>(resolve => server.close(()=>resolve())),new Promise<void>(resolve => blockedServer.close(()=>resolve()))]); }
});

test('the fixed extension identity matches its shipped public manifest key',() => {
  const manifest = JSON.parse(readFileSync(new URL('../browser-extension/manifest.json',import.meta.url),'utf8'));
  const id = createHash('sha256').update(Buffer.from(manifest.key,'base64')).digest('hex').slice(0,32).split('').map(value => String.fromCharCode(97+parseInt(value,16))).join('');
  assert.equal(id,'nciganheacfkdbpdfdnjnjdkcabfbkmg');
  assert.equal(manifest.externally_connectable,undefined);
});

test('canceling one session does not invalidate an active operation owned by another session',async () => {
  const w = worker(); const origin = 'https://example.test';
  const a = await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy:w.policy(origin) });
  const b = await w.command({ action:'navigate',sessionId:'two',url:origin,expectedOrigin:origin,policy:w.policy(origin) });
  assert.ok(a.result); assert.ok(b.result);
  const held = w.hold(b.result.tabId);
  const active = w.command({ action:'type',sessionId:'two',text:'hello',expectedOrigin:origin,policy:w.policy(origin) });
  await held;
  w.controls({ type:'cancel',sessionId:'one' });
  w.release();
  assert.equal((await active).result.ok,true);
});

test('reconnect ignores old native-port listeners and does not duplicate input effects',async () => {
  const w = worker(); const origin = 'https://example.test';
  await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy:w.policy(origin) });
  w.reconnect(); await new Promise(resolve => setImmediate(resolve));
  const result = await w.command({ action:'click',sessionId:'one',x:4,y:5,expectedOrigin:origin,policy:w.policy(origin) });
  assert.equal(result.result.ok,true);
  assert.equal(w.commands.filter(command => command.method === 'Input.dispatchMouseEvent' && command.params.type === 'mousePressed').length,1);
});

test('screenshots report CSS viewport and scroll origins, and changed geometry blocks subsequent clicks',async () => {
  const w = worker(); const origin = 'https://example.test';
  const policy = w.policy(origin);
  await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy });
  w.viewport({ clientWidth:400,clientHeight:300,offsetX:10,offsetY:20,pageX:100,pageY:200,scale:1 });
  const capture = await w.command({ action:'screenshot',sessionId:'one',expectedOrigin:origin,policy });
  assert.equal(capture.result.viewport.width,400); assert.equal(capture.result.viewport.height,300);
  assert.equal(capture.result.viewport.offsetX,10); assert.equal(capture.result.viewport.pageY,200);
  const clip = w.commands.find(command => command.method === 'Page.captureScreenshot').params.clip;
  assert.equal(clip.x,100); assert.equal(clip.y,200); assert.equal(clip.width,400); assert.equal(clip.height,300);
  w.viewport({ clientWidth:401,clientHeight:300,offsetX:10,offsetY:20,pageX:100,pageY:200,scale:1 });
  const changed = await w.command({ action:'click',sessionId:'one',x:20,y:30,expectedOrigin:origin,policy });
  assert.match(changed.error,/viewport changed/);
  assert.equal(w.commands.filter(command => command.method === 'Input.dispatchMouseEvent').length,0);
});

test('Fetch guards block unapproved documents, opaque Other requests and ordinary attachment responses',async () => {
  const w = worker(); const origin = 'https://example.test';
  const navigation = await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy:w.policy(origin) });
  const source = { tabId:navigation.result.tabId };
  const paused = (params:any) => { w.nativeEvent(source,'Fetch.requestPaused',{ requestId:'request',request:{ url:origin+'/file' },...params }); return w.commands.at(-1).method; };
  assert.equal(paused({ resourceType:'Other' }),'Fetch.failRequest');
  assert.equal(paused({ resourceType:'Document',request:{ url:'https://private.test/' } }),'Fetch.failRequest');
  assert.equal(paused({ resourceType:'Document',responseStatusCode:200,responseHeaders:[{ name:'Content-Disposition',value:'attachment; filename=file.txt' },{ name:'Content-Type',value:'text/plain' }] }),'Fetch.failRequest');
  assert.equal(paused({ resourceType:'Fetch',responseStatusCode:200,responseHeaders:[{ name:'Content-Disposition',value:'attachment' }] }),'Fetch.failRequest');
  assert.equal(paused({ resourceType:'Document',responseStatusCode:200,responseHeaders:[{ name:'Content-Type',value:'text/html' }] }),'Fetch.continueResponse');
});

test('explicitly authorized same-origin attachment fetch can complete its bounded download',async () => {
  let arrived:(response:any)=>void = () => undefined;
  const pending = new Promise<any>(resolve => { arrived = resolve; });
  const server = createServer((_request,response) => arrived(response));
  await new Promise<void>(resolve => server.listen(0,'127.0.0.1',resolve));
  const origin = `http://127.0.0.1:${(server.address() as {port:number}).port}`;
  try {
    const w = worker(); const policy = w.policy(origin);
    const navigation = await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy });
    const download = w.command({ action:'download',sessionId:'one',url:origin+'/attachment',expectedOrigin:origin,policy });
    const response = await pending;
    w.nativeEvent({ tabId:navigation.result.tabId },'Fetch.requestPaused',{ requestId:'download',request:{ url:origin+'/attachment' },resourceType:'Fetch',responseStatusCode:200,responseHeaders:[{ name:'Content-Disposition',value:'attachment; filename=allowed.txt' }] });
    assert.equal(w.commands.at(-1).method,'Fetch.continueResponse');
    response.writeHead(200,{ 'content-type':'text/plain','content-disposition':'attachment; filename=allowed.txt' }); response.end('approved');
    assert.equal((await download).result.downloadId,71);
    assert.match(w.effects[0].url,/^data:/);
  } finally { server.closeAllConnections(); await new Promise<void>(resolve => server.close(()=>resolve())); }
});


for (const stage of ['attach','Page.enable','Fetch.enable','Target.setAutoAttach']) {
  test(`revocation during ${stage} removes the stale attachment`,async () => {
    const w = worker(); const origin = 'https://example.test';
    const gate = w.holdSetup(stage);
    const cancelled = w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy:w.policy(origin) });
    await gate.held;
    w.controls({ type:'revoke',revision:'two' });
    gate.release();
    assert.match((await cancelled).error,/cancelled|permissions changed/);
    assert.equal(w.debuggerAttached.has(1),false);
  });

  test(`revocation during ${stage} cleans late attachment before a new operation on the same tab`,async () => {
    const w = worker(); const origin = 'https://example.test'; const policy = w.policy(origin);
    const gate = w.holdSetup(stage);
    const cancelled = w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy });
    await gate.held;
    w.controls({ type:'cancel',sessionId:'one' });
    const resumed = w.command({ action:'navigate',sessionId:'one',tabId:1,url:origin,expectedOrigin:origin,policy });
    gate.release();
    assert.match((await cancelled).error,/cancelled|permissions changed/);
    assert.ok((await resumed).result);
    assert.equal(w.debuggerAttached.has(1),true);
    assert.equal(w.attachCalls.length,2);
    w.controls({ type:'revoke',revision:'two' });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(w.debuggerAttached.has(1),false);
  });
}

test('child targets recursively guard descendants before resuming execution',async () => {
  const w = worker(); const origin = 'https://example.test';
  const navigation = await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy:w.policy(origin) });
  const tabId = navigation.result.tabId;
  w.nativeEvent({ tabId },'Target.attachedToTarget',{ sessionId:'child',targetInfo:{ targetId:'child-target' } });
  await new Promise(resolve => setImmediate(resolve));
  const child = w.commands.filter(command => command.target.sessionId === 'child');
  assert.deepEqual(child.map(command => command.method),['Fetch.enable','Target.setAutoAttach','Runtime.runIfWaitingForDebugger']);
  assert.equal(child[1].params.waitForDebuggerOnStart,true);
  w.nativeEvent({ tabId,sessionId:'child' },'Target.attachedToTarget',{ sessionId:'grandchild',targetInfo:{ targetId:'grandchild-target' } });
  await new Promise(resolve => setImmediate(resolve));
  assert.deepEqual(w.commands.filter(command => command.target.sessionId === 'grandchild').map(command => command.method),['Fetch.enable','Target.setAutoAttach','Runtime.runIfWaitingForDebugger']);
});


test('a timed-out download aborts its page fetch before revocation loses the controller',async () => {
  const w = worker(); const origin = 'https://example.test'; const policy = w.policy(origin);
  await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy });
  w.timeoutDownloads();
  const timedOut = await w.command({ action:'download',sessionId:'one',url:origin+'/slow',expectedOrigin:origin,policy });
  assert.match(timedOut.error,/timed out/);
  assert.equal(w.abortedDownloads(),1);
  w.controls({ type:'revoke',revision:'two' });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(w.debuggerAttached.has(1),false);
});


test('a new operation waits for revocation teardown before reattaching the same tab',async () => {
  const w = worker(); const origin = 'https://example.test'; const policy = w.policy(origin);
  await w.command({ action:'navigate',sessionId:'one',url:origin,expectedOrigin:origin,policy });
  const gate = w.holdSetup('detach');
  w.controls({ type:'cancel',sessionId:'one' });
  await gate.held;
  const resumed = w.command({ action:'navigate',sessionId:'one',tabId:1,url:origin+'/resumed',expectedOrigin:origin,policy });
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(w.attachCalls.length,1);
  assert.equal(w.commands.filter(command => command.method === 'Page.navigate').length,1);
  gate.release();
  assert.ok((await resumed).result);
  assert.equal(w.attachCalls.length,2);
  assert.equal(w.debuggerAttached.has(1),true);
});
