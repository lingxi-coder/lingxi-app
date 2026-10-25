import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { BrowserIntegration, BROWSER_EXTENSION_ID, BROWSER_PROTOCOL, browserPermission } from '../src/main/browserIntegration.js';
import type { ComputerDevicePolicy } from '../src/shared/computerSettings.js';
import { keyDescription, permittedCdpMethod, permitsNavigation } from '../browser-extension/policy.js';

const policy = (): ComputerDevicePolicy => ({ revision:'one',enabled:true,apps:[],browser:{ history:'ask',siteTools:true,fullCdp:false,defaultBrowsing:'ask',defaultDownloads:'ask',sites:[] } });
async function fixture() {
  const root = mkdtempSync(join(tmpdir(),'lingxi-browser-test-'));
  let current = policy();
  const service = new BrowserIntegration({ policy:() => current,descriptorPath:join(root,'connection.json'),nativeHostPath:join(root,'native-host'),bridgeExecutable:process.execPath,extensionDirectory:fileURLToPath(new URL('../browser-extension',import.meta.url)),nativeManifestPath:join(root,'manifest.json') });
  const agent = await service.start(); await service.install();
  const native = JSON.parse(readFileSync(join(root,'connection.json.native'),'utf8')) as { url:string;token:string };
  const effects: Record<string,unknown>[] = [];
  const controls: string[] = [];
  const peerId = 'controlled-peer';
  const post = async (descriptor: typeof agent,path:string,input:unknown,headers:Record<string,string> = {}) => {
    const response = await fetch(descriptor.url+path,{ method:'POST',headers:{ authorization:`Bearer ${descriptor.token}`,'content-type':'application/json',...headers },body:JSON.stringify(input) });
    return { status:response.status,data:await response.json() as any };
  };
  const nativePost = (input:Record<string,unknown>) => post(native,'/extension/message',{ ...input,protocol:BROWSER_PROTOCOL,extensionId:BROWSER_EXTENSION_ID,peerId });
  await nativePost({ type:'hello' });
  const controller = new AbortController();
  let holdAction: string | undefined;
  let held: (() => void) | undefined;
  let heldMessage: any;
  const seenHold = new Promise<void>(resolve => { held = resolve; });
  const pump = (async () => {
    while (!controller.signal.aborted) {
      const response = await fetch(native.url+'/extension/poll',{ headers:{ authorization:`Bearer ${native.token}`,'x-lingxi-browser-peer':peerId },signal:controller.signal });
      if (response.status !== 200) break;
      const message = await response.json() as any;
      if (message.type !== 'command') { controls.push(message.type); if (message.type === 'disconnect') break; continue; }
      const input = message.payload;
      if (input.action === holdAction) { heldMessage = message; held?.(); continue; }
      let result:unknown;
      if (input.action === 'inspect') result = { url:'https://example.test/path',id:7 };
      else if (input.action === 'ping') result = { connected:true };
      else if (input.action === 'tabs') result = { tabs:[{ id:7,url:'https://example.test/path',title:'Allowed' },{ id:8,url:'https://private.test/',title:'Private' }] };
      else { effects.push(input); result = input.action === 'screenshot' ? { image:{ data:'base64-test',mimeType:'image/png' } } : { ok:true,tabId:7 }; }
      await nativePost({ id:message.id,result });
    }
  })().catch(error => { if (!controller.signal.aborted) throw error; });
  return {
    service,effects,controls,agent,native,post,nativePost,
    call:(input:Record<string,unknown>) => post(agent,'/agent',{ sessionId:'controlled-session',...input }),
    change:(update:(value:ComputerDevicePolicy)=>void) => { const next = structuredClone(current); update(next); current = next; service.policyChanged(); },
    hold:(action:string) => { holdAction = action; return seenHold; },
    release:async () => { effects.push(heldMessage.payload); await nativePost({ id:heldMessage.id,result:{ ok:true } }); },
    close:async () => { controller.abort(); await service.close(); await pump; rmSync(root,{ recursive:true,force:true }); },
  };
}

test('browser transport separates agent and extension credentials and rejects web origins',async () => {
  const f = await fixture();
  try {
    assert.equal(f.service.status().connected,true);
    assert.equal((await f.post(f.agent,'/extension/message',{})).status,403);
    assert.equal((await f.post(f.native,'/agent',{ action:'tabs',sessionId:'x' })).status,403);
    assert.equal((await f.post(f.agent,'/agent',{ action:'tabs',sessionId:'x' },{ origin:'https://evil.test' })).status,403);
  } finally { await f.close(); }
});

test('site approval binds the entire request, scopes and revision; tabs do not leak unapproved sites',async () => {
  const f = await fixture();
  try {
    const first = await f.call({ action:'navigate',url:'https://example.test/path' });
    assert.equal(first.status,200); assert.match(first.data.approval.label,/https:\/\/example.test/); assert.equal(f.effects.length,0);
    const mismatch = await f.call({ action:'navigate',url:'https://other.test/path',approvalNonce:first.data.approval.nonce,approved:true });
    assert.equal(mismatch.status,400); assert.equal(f.effects.length,0);
    const second = await f.call({ action:'navigate',url:'https://example.test/path' });
    const accepted = await f.call({ action:'navigate',url:'https://example.test/path',approvalNonce:second.data.approval.nonce,approved:true });
    assert.equal(accepted.status,200); assert.equal(f.effects.length,1);
    assert.deepEqual((await f.call({ action:'tabs' })).data.result.tabs,[{ id:7,url:'https://example.test/path',title:'Allowed' }]);
    assert.ok((await f.call({ action:'history' })).data.approval,'browsing grant does not grant profile history');
    assert.ok((await f.call({ action:'download',url:'https://example.test/file' })).data.approval,'browsing grant does not grant downloads');
    f.change(p => { p.revision = 'two'; p.browser.defaultBrowsing = 'deny'; });
    const denied = await f.call({ action:'navigate',url:'https://example.test/path' });
    assert.equal(denied.status,400); assert.match(denied.data.error,/denied/); assert.equal(f.effects.length,1);
  } finally { await f.close(); }
});

test('policy changes reject in-flight results and invalidate pending approvals',async () => {
  const f = await fixture();
  try {
    const approval = await f.call({ action:'navigate',url:'https://example.test/path' });
    f.change(p => { p.revision = 'two'; });
    assert.equal((await f.call({ action:'navigate',url:'https://example.test/path',approvalNonce:approval.data.approval.nonce,approved:true })).status,400);
    f.change(p => { p.revision = 'three'; p.browser.defaultBrowsing = 'allow'; });
    const held = f.hold('navigate');
    const operation = f.call({ action:'navigate',url:'https://example.test/path' });
    await held;
    f.change(p => { p.revision = 'four'; p.enabled = false; });
    const result = await operation; assert.equal(result.status,400); assert.match(result.data.error,/permissions changed/);
    assert.equal(f.effects.length,0);
  } finally { await f.close(); }
});

test('removing connection prevents an already-running native host from reauthorizing itself',async () => {
  const f = await fixture();
  try {
    await f.service.remove();
    assert.equal(f.service.status().installed,false); assert.equal(f.service.status().connected,false);
    assert.equal((await f.nativePost({ type:'heartbeat' })).status,403);
    await f.service.install();
    assert.equal((await f.nativePost({ type:'hello' })).status,403,'reinstall never revives the removed native credential');
    assert.equal((await f.call({ action:'navigate',url:'https://example.test/path' })).status,200); // Approval alone is not execution.
    assert.equal(f.effects.length,0);
  } finally { await f.close(); }
});

test('canceling a queued request neither cancels the active operation nor later executes the queued action',async () => {
  const f = await fixture();
  try {
    f.change(p => { p.revision = 'two'; p.browser.defaultBrowsing = 'allow'; });
    const held = f.hold('navigate');
    const active = f.call({ action:'navigate',url:'https://example.test/active' });
    await held;
    const controller = new AbortController();
    const queued = fetch(f.agent.url+'/agent',{ method:'POST',headers:{ authorization:`Bearer ${f.agent.token}`,'content-type':'application/json' },body:JSON.stringify({ action:'navigate',sessionId:'controlled-session',url:'https://example.test/queued' }),signal:controller.signal });
    await new Promise(resolve => setTimeout(resolve,30));
    controller.abort(); await assert.rejects(queued);
    await new Promise(resolve => setTimeout(resolve,10));
    await f.release(); assert.equal((await active).status,200);
    await new Promise(resolve => setTimeout(resolve,30));
    assert.deepEqual(f.effects.map(input => input.url),['https://example.test/active']);
  } finally { await f.close(); }
});

test('a different Chrome peer cannot inherit conversation grants or pending approvals',async () => {
  const f = await fixture(); const now = Date.now;
  try {
    const history = await f.call({ action:'history' });
    assert.equal((await f.call({ action:'history',approvalNonce:history.data.approval.nonce,approved:true })).status,200);
    const pending = await f.call({ action:'navigate',url:'https://example.test/path' });
    Date.now = () => now()+36_000;
    const switched = await f.post(f.native,'/extension/message',{ protocol:BROWSER_PROTOCOL,extensionId:BROWSER_EXTENSION_ID,peerId:'different-profile',type:'hello' });
    assert.equal(switched.status,200);
    assert.ok((await f.call({ action:'history' })).data.approval,'profile A history consent does not authorize profile B');
    const replayed = await f.call({ action:'navigate',url:'https://example.test/path',approvalNonce:pending.data.approval.nonce,approved:true });
    assert.equal(replayed.status,400);
    assert.equal(f.effects.length,1);
  } finally { Date.now = now; await f.close(); }
});

test('exact origin overrides and extension guards preserve site, download and CDP boundaries',() => {
  const p = policy(); p.browser.defaultBrowsing = 'allow'; p.browser.sites = [{ origin:'https://private.test',browsing:'deny',downloads:'allow' }];
  assert.equal(browserPermission(p,'https://private.test','browsing'),'deny');
  assert.equal(browserPermission(p,'https://child.private.test','browsing'),'allow');
  assert.equal(browserPermission(p,'https://private.test','downloads'),'allow');
  const guard = { ...p.browser,allowedOrigins:['https://private.test'] };
  assert.equal(permitsNavigation(guard,'https://private.test/no'),false,'a session grant cannot override a site deny');
  assert.equal(permitsNavigation(guard,'file:///etc/passwd'),false);
  assert.equal(permittedCdpMethod('Fetch.disable'),false);
  assert.equal(permittedCdpMethod('Target.createTarget'),false);
  assert.equal(permittedCdpMethod('Network.getAllCookies'),false);
  assert.equal(permittedCdpMethod('Page.setDownloadBehavior'),false);
  assert.equal(permittedCdpMethod('Runtime.evaluate'),true);
  assert.throws(() => keyDescription('cmd+v'),/Clipboard/);
  for (const chord of ['cmd+tab','alt+tab','ctrl+tab','cmd+space','ctrl+up','ctrl+meta+q']) assert.throws(() => keyDescription(chord));
  assert.deepEqual(keyDescription('shift+enter'),{ key:'Enter',modifiers:8 });
});


test('a delivered command timeout sends session cancellation before subsequent calls',async t => {
  const f = await fixture();
  try {
    f.change(value => { value.revision = 'allowed'; value.browser.defaultBrowsing = 'allow'; });
    t.mock.timers.enable({ apis:['setTimeout'] });
    const held = f.hold('navigate');
    const operation = f.call({ action:'navigate',url:'https://example.test/slow' });
    await held;
    t.mock.timers.tick(30_001);
    const timedOut = await operation;
    assert.equal(timedOut.status,400);
    assert.match(timedOut.data.error,/did not respond/);
    // The same-session next call traverses the relay after its cancellation.
    assert.equal((await f.call({ action:'tabs' })).status,200);
    assert.ok(f.controls.includes('cancel'));
    t.mock.timers.reset();
    await f.release(); // Late replies cannot resurrect the timed-out request.
    assert.equal((await f.call({ action:'tabs' })).status,200);
  } finally { t.mock.timers.reset(); await f.close(); }
});
