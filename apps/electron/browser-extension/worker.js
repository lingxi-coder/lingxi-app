import { originOf, permitsNavigation, permittedCdpMethod, keyDescription } from './policy.js';

const PROTOCOL = 'lingxi-browser/1';
const HOST = 'com.lingxi.browser';
const peerId = crypto.randomUUID();
const sessions = new Map();
const attached = new Set();
const attaching = new Map();
const detaching = new Map();
const tabSessions = new Map();
const controllers = new Map();
let port;
let epoch = 0;
let revision;

function send(message) { port?.postMessage({ ...message,protocol:PROTOCOL,extensionId:chrome.runtime.id,peerId }); }
function session(id) {
  if (!sessions.has(id)) sessions.set(id,{ tabs:new Set(),activeTab:undefined,groupId:undefined,policy:undefined,generation:0,downloads:new Set() });
  return sessions.get(id);
}
async function detach(tabId) { attached.delete(tabId); try { await chrome.debugger.detach({ tabId }); } catch { /* Target closed or already detached. */ } }
async function revoke(id) {
  if (id === undefined) epoch++;
  const cleanups = [];
  for (const [sessionId,state] of sessions) if (id === undefined || id === sessionId) {
    state.generation++;
    state.policy = undefined;
    for (const tabId of state.tabs) {
      attached.delete(tabId);
      const previous = detaching.get(tabId);
      const cleanup = (async () => {
        await previous;
        for (const [key,contextId] of controllers.get(tabId) ?? []) {
          let timer;
          try {
            await Promise.race([
              chrome.debugger.sendCommand({ tabId },'Runtime.evaluate',{ expression:`globalThis[${JSON.stringify(key)}]?.abort()`,contextId,timeout:1_000 }),
              new Promise(resolve => { timer = setTimeout(resolve,1_000); }),
            ]);
          } catch { /* Target closed or detached. */ }
          finally { clearTimeout(timer); }
        }
        await detach(tabId);
      })();
      detaching.set(tabId,cleanup);
      cleanups.push(cleanup.finally(() => { if (detaching.get(tabId) === cleanup) detaching.delete(tabId); }));
    }
  }
  await Promise.all(cleanups);
}
function assertLive(state,stamp,generation,expectedRevision) {
  if (epoch !== stamp || state.generation !== generation || revision !== expectedRevision || !port) throw new Error('Browser operation was cancelled or permissions changed');
}
async function ownedTab(state,tabId) {
  const id = tabId ?? state.activeTab;
  if (!Number.isSafeInteger(id) || !state.tabs.has(id)) throw new Error('No session-owned tab is available. Navigate to an approved site first.');
  return chrome.tabs.get(id);
}
async function ensureAttached(tabId,check) {
  check();
  // A revoked attachment must finish its cleanup before another generation can
  // attach to the same tab. Otherwise its late detach could remove the new one.
  const teardown = detaching.get(tabId);
  if (teardown) { await teardown; check(); return ensureAttached(tabId,check); }
  const previous = attaching.get(tabId);
  if (previous) { await previous.catch(() => undefined); check(); return ensureAttached(tabId,check); }
  if (attached.has(tabId)) return;
  const setup = (async () => {
    try {
      await chrome.debugger.attach({ tabId },'1.3'); attached.add(tabId); check();
      await chrome.debugger.sendCommand({ tabId },'Page.enable'); check();
      await chrome.debugger.sendCommand({ tabId },'Fetch.enable',{ patterns:[{ urlPattern:'*',requestStage:'Request' },{ urlPattern:'*',requestStage:'Response' }] }); check();
      await chrome.debugger.sendCommand({ tabId },'Target.setAutoAttach',{ autoAttach:true,waitForDebuggerOnStart:true,flatten:true }); check();
    } catch (error) { await detach(tabId); throw new Error(`Chrome could not enable browser permission guards: ${error.message}`); }
  })();
  attaching.set(tabId,setup);
  try { await setup; } finally { if (attaching.get(tabId) === setup) attaching.delete(tabId); }
}
async function evaluate(tabId,expression,contextId) {
  const value = await chrome.debugger.sendCommand({ tabId },'Runtime.evaluate',{ expression,returnByValue:true,awaitPromise:true,timeout:25_000,...(contextId !== undefined ? { contextId } : {}) });
  if (value.exceptionDetails) throw new Error(value.exceptionDetails.text ?? 'Page script failed');
  return value.result?.value;
}
async function verifyTarget(state,tabId,expectedOrigin) {
  const tab = await ownedTab(state,tabId);
  if (originOf(tab.url) !== expectedOrigin || !permitsNavigation(state.policy,tab.url)) throw new Error('The tab changed to a site that is not approved. Request access again.');
  if (attached.has(tab.id)) {
    const frame = await chrome.debugger.sendCommand({ tabId:tab.id },'Page.getFrameTree');
    if (originOf(frame.frameTree.frame.url) !== expectedOrigin) throw new Error('The actual page changed to an unapproved site');
  }
  return tab;
}
async function viewport(tabId) {
  const metrics = await chrome.debugger.sendCommand({ tabId },'Page.getLayoutMetrics');
  const value = metrics.cssVisualViewport;
  const result = { width:value?.clientWidth,height:value?.clientHeight,offsetX:value?.offsetX,offsetY:value?.offsetY,pageX:value?.pageX,pageY:value?.pageY,scale:value?.scale };
  if (!Object.values(result).every(Number.isFinite) || result.width <= 0 || result.height <= 0 || result.width > 32768 || result.height > 32768 || result.width*result.height > 67_108_864 || result.scale <= 0 || result.scale > 100) throw new Error('Chrome did not return a supported CSS viewport');
  return result;
}
function sameViewport(left,right) { return Object.keys(left).every(key => left[key] === right[key]); }
async function execute(message) {
  const input = message.payload;
  if (input.action === 'ping') return { connected:true,protocol:PROTOCOL };
  const state = session(input.sessionId);
  const stamp = epoch; const generation = state.generation;
  const check = () => assertLive(state,stamp,generation,message.revision);
  if (input.action === 'tabs') {
    const tabs = await Promise.all([...state.tabs].map(id => chrome.tabs.get(id).catch(() => undefined)));
    check(); return { tabs:tabs.filter(Boolean).map(tab => ({ id:tab.id,url:tab.url,title:tab.title })) };
  }
  if (input.action === 'inspect') {
    const tab = await ownedTab(state,input.tabId); check(); return { id:tab.id,url:tab.url };
  }
  if (input.policy?.revision !== message.revision) throw new Error('Browser policy is missing');
  state.policy = input.policy;
  if (input.action === 'history') {
    check(); const items = await chrome.history.search({ text:typeof input.query === 'string' ? input.query : '',maxResults:Math.min(100,Math.max(1,Number(input.limit)||20)),startTime:0 });
    check(); return { history:items.map(item => ({ url:item.url,title:item.title,lastVisitTime:item.lastVisitTime,visitCount:item.visitCount })) };
  }
  if (input.action === 'download') {
    if (originOf(input.url) !== input.expectedOrigin) throw new Error('Download origin does not match approval');
    const tab = await verifyTarget(state,input.tabId,input.expectedOrigin); check();
    await ensureAttached(tab.id,check); check();
    const tree = await chrome.debugger.sendCommand({ tabId:tab.id },'Page.getFrameTree');
    if (originOf(tree.frameTree.frame.url) !== input.expectedOrigin) throw new Error('Download page origin changed');
    const world = await chrome.debugger.sendCommand({ tabId:tab.id },'Page.createIsolatedWorld',{ frameId:tree.frameTree.frame.id,worldName:`LingXi download ${crypto.randomUUID()}` });
    check();
    const key = `__lingxi_download_${crypto.randomUUID()}`;
    const active = controllers.get(tab.id) ?? new Map(); active.set(key,world.executionContextId); controllers.set(tab.id,active);
    const downloadUrl = new URL(input.url); downloadUrl.hash = '';
    state.downloads.add(downloadUrl.href);
    const expression = `(async()=>{const controller=new AbortController();globalThis[${JSON.stringify(key)}]=controller;try{if(location.origin!==${JSON.stringify(input.expectedOrigin)})throw new Error('Download page origin changed');const response=await fetch(${JSON.stringify(input.url)},{credentials:'include',redirect:'error',cache:'no-store',signal:controller.signal});if(!response.ok||new URL(response.url).origin!==location.origin)throw new Error('Download failed or origin changed');const reader=response.body?.getReader();if(!reader)throw new Error('Download response has no body');const chunks=[];let length=0;for(;;){const{value,done}=await reader.read();if(done)break;length+=value.length;if(length>8388608){controller.abort();throw new Error('Downloads larger than 8 MiB are not supported');}chunks.push(value);}const blob=new Blob(chunks,{type:response.headers.get('content-type')||'application/octet-stream'});return await new Promise((resolve,reject)=>{const reader=new FileReader();reader.onload=()=>resolve(reader.result);reader.onerror=reject;reader.readAsDataURL(blob);});}finally{delete globalThis[${JSON.stringify(key)}];}})()`;
    let data;
    try { data = await evaluate(tab.id,expression,world.executionContextId); }
    finally {
      // A Runtime.evaluate timeout stops waiting for the result, not the page's
      // fetch. Abort it before dropping the key used by permission revocation.
      let timer;
      try {
        await Promise.race([
          chrome.debugger.sendCommand({ tabId:tab.id },'Runtime.evaluate',{ expression:`globalThis[${JSON.stringify(key)}]?.abort()`,contextId:world.executionContextId,timeout:1_000 }),
          new Promise(resolve => { timer = setTimeout(resolve,1_000); }),
        ]);
      } catch { /* A closed renderer or detached debugger already ended access. */ }
      finally { clearTimeout(timer); active.delete(key); state.downloads.delete(downloadUrl.href); }
    }
    check(); await verifyTarget(state,tab.id,input.expectedOrigin); check();
    if (typeof data !== 'string' || !data.startsWith('data:') || data.length > 12_000_000) throw new Error('Download data exceeds the supported browser limit');
    // The remote request has already completed without following redirects.
    // Saving this data URL cannot initiate a second remote request or redirect.
    const downloadId = await chrome.downloads.download({ url:data,saveAs:true }); check(); return { downloadId };
  }
  if (input.action === 'navigate') {
    if (originOf(input.url) !== input.expectedOrigin || !permitsNavigation(state.policy,input.url)) throw new Error('Navigation is not approved');
    let tab;
    state.lastViewport = undefined;
    if (input.tabId === undefined && state.activeTab === undefined) {
      check(); tab = await chrome.tabs.create({ url:'about:blank',active:false });
      if (!Number.isSafeInteger(tab.id)) throw new Error('Chrome did not create a tab');
      state.tabs.add(tab.id); state.activeTab = tab.id; tabSessions.set(tab.id,input.sessionId);
      check(); const groupId = await chrome.tabs.group({ tabIds:[tab.id],...(state.groupId !== undefined ? { groupId:state.groupId } : {}) });
      state.groupId = groupId; await chrome.tabGroups.update(groupId,{ title:`LingXi · ${input.sessionId.slice(0,8)}`,color:'blue' });
    } else tab = await ownedTab(state,input.tabId);
    check(); await ensureAttached(tab.id,check); check();
    const result = await chrome.debugger.sendCommand({ tabId:tab.id },'Page.navigate',{ url:input.url });
    check(); if (result.errorText) throw new Error(result.errorText);
    for (let attempt = 0; attempt < 100; attempt++) {
      check(); const current = await chrome.tabs.get(tab.id);
      if (current.status === 'complete') { await verifyTarget(state,tab.id,input.expectedOrigin); check(); return { tabId:tab.id,url:current.url }; }
      await new Promise(resolve => setTimeout(resolve,100));
    }
    throw new Error('Browser navigation is still loading. Inspect the tab before continuing.');
  }
  const tab = await verifyTarget(state,input.tabId,input.expectedOrigin); check();
  await ensureAttached(tab.id,check); check();
  // Resolve focus changes and external navigation immediately before each input.
  await verifyTarget(state,tab.id,input.expectedOrigin); check();
  let result;
  if (input.action === 'screenshot') {
    const capturedViewport = await viewport(tab.id); check();
    const image = await chrome.debugger.sendCommand({ tabId:tab.id },'Page.captureScreenshot',{ format:'png',captureBeyondViewport:false,clip:{ x:capturedViewport.pageX,y:capturedViewport.pageY,width:capturedViewport.width,height:capturedViewport.height,scale:1 } });
    check(); if (!sameViewport(capturedViewport,await viewport(tab.id))) throw new Error('Browser viewport changed during capture. Take a fresh screenshot.');
    state.lastViewport = capturedViewport;
    result = { image:{ data:image.data,mimeType:'image/png' },viewport:capturedViewport,tabId:tab.id };
  } else if (input.action === 'click') {
    const { x,y } = input;
    if (![x,y].every(value => Number.isFinite(value) && value >= 0 && value <= 32768)) throw new Error('Click coordinates must be finite CSS pixel positions');
    if (state.lastViewport && !sameViewport(state.lastViewport,await viewport(tab.id))) throw new Error('Browser viewport changed. Take a fresh screenshot before clicking.');
    check();
    await chrome.debugger.sendCommand({ tabId:tab.id },'Input.dispatchMouseEvent',{ type:'mouseMoved',x,y }); check();
    await chrome.debugger.sendCommand({ tabId:tab.id },'Input.dispatchMouseEvent',{ type:'mousePressed',x,y,button:'left',clickCount:1 });
    try { check(); } finally { await chrome.debugger.sendCommand({ tabId:tab.id },'Input.dispatchMouseEvent',{ type:'mouseReleased',x,y,button:'left',clickCount:1 }).catch(() => undefined); }
    result = { ok:true };
  } else if (input.action === 'type') {
    if (typeof input.text !== 'string' || input.text.length > 65536) throw new Error('Text is missing or exceeds the browser input limit');
    await chrome.debugger.sendCommand({ tabId:tab.id },'Input.insertText',{ text:input.text }); result = { ok:true };
  } else if (input.action === 'key') {
    const key = keyDescription(input.key);
    await chrome.debugger.sendCommand({ tabId:tab.id },'Input.dispatchKeyEvent',{ type:'keyDown',...key });
    try { check(); } finally { await chrome.debugger.sendCommand({ tabId:tab.id },'Input.dispatchKeyEvent',{ type:'keyUp',...key }).catch(() => undefined); }
    result = { ok:true };
  } else if (input.action === 'site_tools') {
    if (!state.policy.siteTools) throw new Error('Site tools are disabled');
    const name = input.toolName;
    const expression = `(async()=>{const api=document.modelContext;if(!api?.getTools||!api?.executeTool)throw new Error('WebMCP is unavailable on this site or Chrome version');const tools=await api.getTools();${typeof name === 'string' ? `const tool=tools.find(t=>t.name===${JSON.stringify(name)}&&t.origin===location.origin);if(!tool)throw new Error('Same-origin site tool not found');return api.executeTool(tool,${JSON.stringify(input.arguments ?? {})});` : "return tools.filter(t=>t.origin===location.origin).map(({name,description,inputSchema,annotations})=>({name,description,inputSchema,annotations}));"}})()`;
    result = { data:await evaluate(tab.id,expression) };
  } else if (input.action === 'cdp') {
    if (!state.policy.fullCdp || !permittedCdpMethod(input.method)) throw new Error('This CDP method is unavailable or would escape the authorized target');
    result = await chrome.debugger.sendCommand({ tabId:tab.id },input.method,input.params ?? {});
  } else throw new Error('Unknown browser action');
  check(); await verifyTarget(state,tab.id,input.expectedOrigin); check(); return result;
}

chrome.debugger.onEvent.addListener((source,method,params) => {
  const sessionId = tabSessions.get(source.tabId);
  const state = sessions.get(sessionId);
  if (method === 'Fetch.requestPaused') {
    const responseStage = params.responseStatusCode !== undefined || params.responseErrorReason !== undefined;
    const headers = params.responseHeaders ?? [];
    const attachment = headers.some(header => header.name.toLowerCase() === 'content-disposition' && /^\s*attachment(?:\s*;|\s*$)/i.test(header.value));
    const explicitDownload = params.resourceType === 'Fetch' && state?.downloads.has(params.request.url);
    const mime = headers.find(header => header.name.toLowerCase() === 'content-type')?.value.split(';')[0].trim().toLowerCase() ?? '';
    const redirect = params.responseStatusCode >= 300 && params.responseStatusCode < 400;
    const rendererType = /^(text\/|image\/|audio\/|video\/)/.test(mime) || ['application/json','application/xml','application/xhtml+xml'].includes(mime);
    const approved = state?.policy && (params.resourceType !== 'Document' || permitsNavigation(state.policy,params.request.url));
    const allowed = approved && params.resourceType !== 'Other' && (!attachment || explicitDownload) && !(responseStage && params.resourceType === 'Document' && !redirect && ![204,205].includes(params.responseStatusCode) && !rendererType);
    void chrome.debugger.sendCommand(source,allowed ? responseStage ? 'Fetch.continueResponse' : 'Fetch.continueRequest' : 'Fetch.failRequest',allowed ? { requestId:params.requestId } : { requestId:params.requestId,errorReason:'BlockedByClient' }).catch(() => undefined);
  } else if (method === 'Target.attachedToTarget') {
    const target = { tabId:source.tabId,sessionId:params.sessionId };
    // Auto-attachment is not recursive: every child must guard its own
    // descendants before it is allowed to run.
    const stamp = epoch; const generation = state?.generation; const expectedRevision = revision;
    const check = () => { if (!state?.policy) throw new Error('Browser permissions are not active'); assertLive(state,stamp,generation,expectedRevision); };
    void (async () => {
      check();
      await chrome.debugger.sendCommand(target,'Fetch.enable',{ patterns:[{ urlPattern:'*',requestStage:'Request' },{ urlPattern:'*',requestStage:'Response' }] }); check();
      await chrome.debugger.sendCommand(target,'Target.setAutoAttach',{ autoAttach:true,waitForDebuggerOnStart:true,flatten:true }); check();
      await chrome.debugger.sendCommand(target,'Runtime.runIfWaitingForDebugger');
    })().catch(() => chrome.debugger.sendCommand({ tabId:source.tabId },'Target.closeTarget',{ targetId:params.targetInfo.targetId }).catch(() => undefined));
  }
});
chrome.debugger.onDetach.addListener(source => { attached.delete(source.tabId); });
chrome.tabs.onRemoved.addListener(tabId => { attached.delete(tabId); const id = tabSessions.get(tabId); tabSessions.delete(tabId); const state = sessions.get(id); state?.tabs.delete(tabId); if (state?.activeTab === tabId) state.activeTab = [...state.tabs][0]; });

function connect() {
  if (port) return;
  try {
    const connection = chrome.runtime.connectNative(HOST);
    port = connection;
    connection.onDisconnect.addListener(() => { void chrome.runtime.lastError; if (port !== connection) return; port = undefined; void revoke(); });
    connection.onMessage.addListener(message => {
      if (port !== connection) return;
      if (message.type === 'ping') { send({ type:'heartbeat' }); return; }
      if (message.type === 'revoke') { revision = message.revision; void revoke(); return; }
      if (message.type === 'cancel') { void revoke(message.sessionId); return; }
      if (message.type === 'disconnect') { void revoke(); const previous = port; port = undefined; previous?.disconnect(); return; }
      if (message.type !== 'command') return;
      if (revision === undefined) revision = message.revision;
      if (message.revision !== revision) { send({ id:message.id,error:'Browser permissions changed' }); return; }
      void execute(message).then(result => send({ id:message.id,result })).catch(error => send({ id:message.id,error:error.message ?? 'Browser operation failed' }));
    });
    send({ type:'hello' });
  } catch { port = undefined; }
}
chrome.alarms.create('lingxi-browser-reconnect',{ periodInMinutes:0.5 });
chrome.alarms.onAlarm.addListener(() => { if (port) send({ type:'heartbeat' }); else connect(); });
chrome.action.onClicked.addListener(() => {
  const previous = port; port = undefined; previous?.disconnect();
  void revoke().then(() => { revision = undefined; connect(); });
});
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
