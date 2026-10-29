import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';
import { createServer } from 'node:http';
import { LocalAppRuntime } from '../src/main/local-apps/runtime.ts';

const require = createRequire(import.meta.url);
const bundledHost = build({
  entryPoints: [new URL('../src/main/local-apps/host.ts', import.meta.url).pathname],
  bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'], metafile: true,
  plugins: [{ name: 'bridge-client-test-resolution', setup(builder) {
    builder.onResolve({ filter: /^@lingxi\/bridge-client(?:\/|$)/ }, args => ({ path: fileURLToPath(import.meta.resolve(args.path)) }));
  } }],
}).then(async result => {
  if (process.env.LINGXI_TEST_ARTIFACT_DIR) await fs.writeFile(join(process.env.LINGXI_TEST_ARTIFACT_DIR, 'remote-host-inputs.json'), JSON.stringify({
    bridgeClientEntry: import.meta.resolve('@lingxi/bridge-client'), inputs: Object.keys(result.metafile!.inputs),
  }, null, 2));
  return result;
});
const id = `a${'1'.repeat(32)}`;
const owner = { accountUid: 'account', deviceId: 'phone', hostId: 'desktop' };
const model = { providerId:'p', modelId:'m', label:'Model', canSelect:true, supportsAttachments:true, supportsVision:true, supportsStructuredOutput:true };

async function fixture(t: { after(fn: () => Promise<void>): void }, launchTimeoutMs?: number) {
  const root = await fs.mkdtemp(join(tmpdir(),'remote-app-host-'));
  let windowCount = 0, running = false, revision = 1, runId = 'run-one', liveSession = false, honorAbort = true, ended = 0;
  let startHook = async (_signal?: AbortSignal) => {};
  const electron = { BrowserWindow: class {
    static getAllWindows(){return [];}
    private closed?:()=>void;private destroyed=false;
    webContents={id:17,on(){},setWindowOpenHandler(){}};
    constructor(){windowCount++;}isDestroyed(){return this.destroyed;}on(name:string,fn:()=>void){if(name==='closed')this.closed=fn;}
    destroy(){this.destroyed=true;this.closed?.();}async loadURL(){throw new Error('load failed');}show(){}focus(){}
  }, ipcMain:{removeHandler(){}}, session:{fromPartition:()=>({flushStorageData(){}})}, dialog:{showMessageBox:async()=>({response:1})} };
  const module = { exports: {} };
  const hostSetTimeout = (fn: () => void, ms: number) => {
    const timer = setTimeout(fn, ms === 900_000 && launchTimeoutMs !== undefined ? launchTimeoutMs : ms);
    if (ms === 900_000 && launchTimeoutMs !== undefined) timer.unref = () => timer;
    return timer;
  };
  // Keep Node fetch, streams and promise async contexts in the same realm as
  // node:test. A separate vm realm can trip Node's native async_id assertion
  // when real HTTP responses settle while a test is tearing down.
  const load = new Function('module','exports','require','setTimeout','clearTimeout',(await bundledHost).outputFiles[0]!.text);
  load(module,module.exports,(name:string)=>name === 'electron' ? electron : require(name),hostSetTimeout,clearTimeout);
  const LocalAppHost = (module.exports as any).LocalAppHost;
  const host = new LocalAppHost({core:{onDisconnected(){}},configHome:root,onRemoteAccessEnded:()=>{ended++;},preload:'/unused',mainWindow:()=>undefined,edit:async()=>{},agentCatalog:async()=>({models:[model],tools:[],skills:[]})});
  const row:any = {id,name:'Remote app',summary:'',revision:1,currentRevision:1,releaseDigest:'d'.repeat(64),background:false,grants:['agent'],phase:'stopped',manifest:{schema:2,targets:[],name:'Remote app',entry:{type:'static',path:'index.html'},capabilities:['agent'],agent:{tools:[],skills:[]}},agent:{enabled:true,model,tools:[],skills:[],background:false,maxTurns:5,timeoutMs:10000}};
  const store:any = {root, get:async()=>structuredClone(row), getRevision:async()=>structuredClone(row), list:async()=>[structuredClone(row)], save:async(value:any)=>{Object.assign(row,structuredClone(value));}, assertTarget:async()=>{}, revisionPath:()=>root, core:{request:async()=>({digest:row.releaseDigest,files:[]})}, publish:async()=>{row.revision++;row.currentRevision=row.revision;return structuredClone(row);} };
  host.store = store;
  const runtime:any = {status:(value:any)=>({...value,phase:running?'running':'stopped',runningRevision:running?revision:undefined,runId:running?runId:undefined}),
    phase:()=>running?'running':'stopped', revision:()=>running?revision:undefined, invocationId:()=>running?runId:undefined,manifest:()=>row.manifest,
    owner:()=>running?({appId:id,runId,revision,releaseDigest:row.releaseDigest}):undefined,
    start:async(_id:string,_repair:boolean,signal?:AbortSignal)=>{await startHook(signal);if(honorAbort)signal?.throwIfAborted(); running=true;revision=row.revision;}, stop:async()=>{running=false;},exclusive:async(_id:string,fn:()=>Promise<unknown>)=>fn()};
  host.runtime=runtime;
  const gateway:any = {open:async(run:any,requestedOwner:any)=>{assert.equal(requestedOwner.deviceId,owner.deviceId); liveSession=true;return {app_id:run.appId,revision:run.revision,url:`https://${id}-${'1'.repeat(20)}.apps.example.com/__lingxi_remote_bootstrap#ticket`,expires_at:Date.now()+60000};},
    hasAccess:()=>liveSession,hasRun:(_id:string,run:string)=>liveSession && run===runId,hasSession:(_id:string,run:string,session:string)=>liveSession && run===runId && session==='s'.repeat(43),revokeApp:(revoked:string)=>{if(revoked===id)liveSession=false;},revokeOwner:(revoked:any)=>{if(revoked.deviceId===owner.deviceId)liveSession=false;},revokeAll:()=>{liveSession=false;},dispose:async()=>{liveSession=false;}};
  host.remoteGateway=gateway;
  host.setAgentBroker({cancelApp:async()=>{},dispose:async()=>{}});
  t.after(async()=>{ if (host.changeTimer) clearTimeout(host.changeTimer); await fs.rm(root,{recursive:true,force:true}); });
  return {host,row,store,runtime,get windowCount(){return windowCount;},get ended(){return ended;},setStartHook:(fn:(signal?:AbortSignal)=>Promise<void>)=>{startHook=fn;},ignoreStartAbort:()=>{honorAbort=false;},setRun:(value:boolean)=>{running=value;}, get liveSession(){return liveSession;}};
}

test('remote access IPC defaults off, persists trusted opt-in, and launches headlessly', async t => {
  const f=await fixture(t);
  assert.deepEqual(JSON.parse(JSON.stringify(await f.host.request({action:'remote-settings'}))),{});
  await assert.rejects(f.host.remoteOpen(id,owner),/not enabled/);
  await f.host.request({action:'remote-access',id,enabled:true});
  assert.equal((await f.host.request({action:'remote-settings'}))[id],true);
  assert.equal((await f.host.remoteList())[0].id,id);
  const opened=await f.host.remoteOpen(id,owner);
  assert.equal(opened.app_id,id);assert.equal(opened.revision,1);assert.equal(f.windowCount,0);assert.equal(f.runtime.phase(),'running');
  assert.equal(await f.host.remoteCurrent({appId:id,revision:1,runId:'run-one'}),true);
  await f.host.request({action:'remote-access',id,enabled:false});
  assert.equal(f.liveSession,false); assert.equal(await f.host.remoteCurrent({appId:id,revision:1,runId:'run-one'}),false);
  assert.deepEqual(JSON.parse(JSON.stringify(await f.host.request({action:'remote-settings'}))),{});
});

test('idle check sees a remote opening that arrives while the app list is loading', async t => {
  const f = await fixture(t);
  let release!: () => void, entered!: () => void, access = false;
  const blocked = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  f.host.list = async () => { entered(); await blocked; return []; };
  f.host.remoteGateway.hasAccess = () => access;
  const check = f.host.hasBackgroundApps();
  await waiting; access = true; release();
  assert.equal(await check, true);
});

test('concurrent Desktop version requests retain their own intent and expected revision', async t => {
  const f = await fixture(t);
  let release!: () => void, entered!: () => void, current = 3;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  const calls: number[] = [];
  f.host.openOnce = async (_id: string, _signal: AbortSignal, _repair: boolean, _restart: boolean, revision: number, expected: number) => {
    calls.push(revision);
    if (calls.length === 1) { entered(); await gate; }
    if (current !== expected) throw new Error('current version changed');
    current = revision;
  };
  const first = f.host.open(id, false, true, undefined, 1, 3);
  const second = assert.rejects(f.host.open(id, false, true, undefined, 2, 3), /current version changed/);
  await waiting; release(); await Promise.all([first, second]);
  assert.deepEqual(calls, [1, 2]); assert.equal(current, 1);
});

test('concurrent version selections do not report a superseded selection as successful', async t => {
  const f = await fixture(t); const calls: number[] = [];
  f.host.openOnce = async (_id: string, _signal: AbortSignal, _repair: boolean, _restart: boolean, revision: number) => { calls.push(revision); };
  const first = assert.rejects(f.host.request({ action: 'useVersion', id, revision: 1, expectedCurrentRevision: 3 }), /cancelled/);
  const second = f.host.request({ action: 'useVersion', id, revision: 2, expectedCurrentRevision: 3 });
  await Promise.all([first, second]); assert.deepEqual(calls, [2]);
});

test('Desktop opening callers cancel independently', async t => {
  for (const cancelFirst of [true, false]) {
    const f = await fixture(t);
    let release!: () => void, entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; });
    const waiting = new Promise<void>(resolve => { entered = resolve; });
    f.host.openOnce = async (_id: string, signal: AbortSignal) => { entered(); await gate; signal.throwIfAborted(); };
    const first = new AbortController(), second = new AbortController();
    const a = f.host.open(id, false, false, first.signal); await waiting;
    const b = f.host.open(id, false, false, second.signal);
    const cancelled = assert.rejects(cancelFirst ? a : b);
    (cancelFirst ? first : second).abort(); await cancelled;
    release(); await (cancelFirst ? b : a);
  }
});

test('Stop cancels an incompatible Desktop opening waiting for another opening', async t => {
  const f = await fixture(t);
  let release!: () => void, entered!: () => void, calls = 0;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  f.host.openOnce = async (_id: string, signal: AbortSignal) => { calls++; entered(); await gate; signal.throwIfAborted(); };
  const first = assert.rejects(f.host.open(id)); await waiting;
  const second = assert.rejects(f.host.open(id, false, true));
  const stop = f.host.request({ action: 'stop', id });
  release(); await Promise.all([first, second, stop]);
  assert.equal(calls, 1);
});

test('Stop cancels a version selection waiting for an earlier Desktop opening', async t => {
  const f = await fixture(t);
  let release!: () => void, entered!: () => void, calls = 0;
  const gate = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  f.host.openOnce = async (_id: string, signal: AbortSignal) => { calls++; entered(); await gate; signal.throwIfAborted(); };
  const first = assert.rejects(f.host.open(id)); await waiting;
  const version = assert.rejects(f.host.request({ action: 'useVersion', id, revision: 2, expectedCurrentRevision: 1 }), /cancelled/);
  const stop = f.host.request({ action: 'stop', id });
  release(); await Promise.all([first, version, stop]); assert.equal(calls, 1);
});

test('remote opening survives cancellation of a Desktop opening', async t => {
  const f = await fixture(t); await f.host.setRemoteAccess(id, true);
  const workspace = join(f.store.root, 'runtime-workspace');
  await fs.mkdir(workspace); await fs.writeFile(join(workspace, 'index.html'), '<!doctype html>');
  f.store.prepareWorkspace = async () => workspace;
  const runtime = new LocalAppRuntime(f.store, () => {}); f.host.runtime = runtime;
  let entered!: () => void;
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  f.host.openOnce = async (_id: string, signal: AbortSignal) => {
    entered(); await new Promise<void>((_resolve, reject) => signal.addEventListener('abort', () => reject(new Error('Desktop cancelled')), { once: true }));
  };
  const controller = new AbortController();
  const local = assert.rejects(f.host.open(id, false, false, controller.signal)); await waiting;
  const remote = f.host.remoteOpen(id, owner);
  await new Promise(resolve => setTimeout(resolve, 0));
  controller.abort(); await local;
  try { assert.equal((await remote).app_id, id); assert.equal(runtime.phase(id), 'running'); }
  finally { await runtime.dispose(); }
});

test('a last Desktop caller cancelling at runtime commit releases the orphaned run', async t => {
  const f = await fixture(t);
  const workspace = join(f.store.root, 'runtime-workspace');
  await fs.mkdir(workspace); await fs.writeFile(join(workspace, 'index.html'), '<!doctype html>');
  f.store.prepareWorkspace = async () => workspace;
  const controller = new AbortController();
  const runtime = new LocalAppRuntime(f.store, () => {
    if (runtime.phase(id) === 'running') controller.abort();
  });
  f.host.runtime = runtime;
  try {
    const opening = f.host.open(id, false, false, controller.signal);
    await assert.rejects(opening);
    await f.host.opening.get(id)?.promise.catch(() => undefined);
    assert.equal(controller.signal.aborted, true);
    assert.equal(runtime.phase(id), 'stopped');
  } finally { await runtime.dispose(); }
});

test('revoking the first of two owners does not cancel their shared real runtime startup', async t => {
  const f = await fixture(t);
  const second = { ...owner, deviceId: 'second-phone' };
  const workspace = join(f.store.root, 'runtime-workspace');
  await fs.mkdir(workspace); await fs.writeFile(join(workspace, 'index.html'), '<!doctype html>');
  let release!: () => void, entered!: () => void, joined!: () => void, calls = 0;
  const blocked = new Promise<void>(resolve => { release = resolve; });
  const waiting = new Promise<void>(resolve => { entered = resolve; });
  const secondJoined = new Promise<void>(resolve => { joined = resolve; });
  f.store.prepareWorkspace = async () => { entered(); await blocked; return workspace; };
  const runtime = new LocalAppRuntime(f.store, () => {});
  const start = runtime.start.bind(runtime);
  runtime.start = (...args: Parameters<typeof runtime.start>) => { if (++calls === 2) joined(); return start(...args); };
  f.host.runtime = runtime;
  const granted: string[] = [];
  f.host.remoteGateway.open = async (run: any, requestedOwner: any) => {
    granted.push(requestedOwner.deviceId);
    return { app_id: run.appId, revision: run.revision, url: 'https://example.test/', expires_at: Date.now() + 60000 };
  };
  f.host.remoteGateway.hasRun = () => granted.length > 0;
  f.host.remoteGateway.hasAccess = () => granted.length > 0;
  await f.host.setRemoteAccess(id, true);
  const first = f.host.remoteOpen(id, owner);
  const firstRejected = assert.rejects(first, /changed/);
  await waiting;
  const other = f.host.remoteOpen(id, second);
  await secondJoined;
  f.host.remoteRevokeOwner(owner);
  release();
  try {
    await firstRejected;
    assert.equal((await other).app_id, id);
    assert.deepEqual(granted, ['second-phone']);
    assert.equal(runtime.phase(id), 'running');
  } finally { await runtime.dispose(); }
});

test('a real runtime committed just before owner revocation is reconciled', async t => {
  const f = await fixture(t);
  const workspace = join(f.store.root, 'runtime-workspace');
  await fs.mkdir(workspace); await fs.writeFile(join(workspace, 'index.html'), '<!doctype html>');
  f.store.prepareWorkspace = async () => workspace;
  let committed = false;
  const runtime = new LocalAppRuntime(f.store, () => {
    if (!committed && runtime.phase(id) === 'running') {
      committed = true;
      f.host.remoteRevokeOwner(owner);
    }
  });
  f.host.runtime = runtime;
  await f.host.setRemoteAccess(id, true);
  try {
    await assert.rejects(f.host.remoteOpen(id, owner), /changed/);
    for (let attempt = 0; attempt < 50 && runtime.phase(id) === 'running'; attempt++) await new Promise(resolve => setTimeout(resolve, 5));
    assert.equal(committed, true);
    assert.equal(f.liveSession, false);
    assert.equal(runtime.phase(id), 'stopped');
    assert.equal(f.host.remoteManagedRuns.has(id), false);
  } finally { await runtime.dispose(); }
});

test('owner revoke, stop, release publication and disable invalidate a remote open during startup', async t => {
  const f=await fixture(t); await f.host.request({action:'remote-access',id,enabled:true});
  for (const action of ['owner','stop','publish','disable'] as const) {
    let release!:()=>void, enter!:()=>void;
    const entered=new Promise<void>(resolve=>{enter=resolve;});
    f.setRun(false);f.setStartHook(()=>new Promise<void>(resolve=>{release=resolve;enter();}));
    const opening=f.host.remoteOpen(id,owner);const rejected=assert.rejects(opening,/changed|not enabled/);await entered;
    if(action==='owner')f.host.remoteRevokeOwner(owner);
    if(action==='stop')await f.host.request({action:'stop',id});
    if(action==='publish'){f.host.publishAuthoring=async()=>({});await f.host.request({action:'publish',id,html:'next'});}
    if(action==='disable')await f.host.request({action:'remote-access',id,enabled:false});
    release(); await rejected;assert.equal(f.liveSession,false);
  }
});

test('headless Agent access requires a live browser session and its current run', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  const identity={appId:id,releaseDigest:f.row.releaseDigest,invocationId:'run-one'};
  assert.equal((await f.host.authorizeAgent(identity)).settings.enabled,true);
  const remoteIdentity = {...identity, remoteSessionId:'s'.repeat(43)};
  assert.equal((await f.host.authorizeAgent(remoteIdentity)).settings.enabled,true);
  f.host.remoteGateway.hasSession=()=>false;
  await assert.rejects(f.host.authorizeAgent(remoteIdentity),/remote session/);
  f.host.remoteRevokeOwner(owner);
  await assert.rejects(f.host.authorizeAgent(identity),/background access/);
  await f.host.remoteOpen(id,owner);f.setRun(false);
  await assert.rejects(f.host.authorizeAgent(identity),/no longer active/);
});

test('revoking a browser session cancels its Agent task even while another session remains live', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  const cancelled:string[]=[];
  const controller=new AbortController();
  f.host.setAgentBroker({run:async(_owner:any,_input:any,signal:AbortSignal)=>{assert.equal(signal,controller.signal);return {runId:'agent-run'};},cancelRemoteSession:async(owner:any)=>{cancelled.push(owner.remoteSessionId);}});
  const result=await f.host.remoteInvoke({appId:id,revision:1,runId:'run-one'},'agent.tasks.start',{input:{}},controller.signal,'s'.repeat(43));
  assert.equal(result.runId,'agent-run'); controller.abort();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(cancelled,['s'.repeat(43)]); assert.equal(f.liveSession,true);
});

test('Agent task admitted after browser revocation is immediately cancelled', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  let release!:()=>void, entered!:()=>void;const enteredPromise=new Promise<void>(resolve=>{entered=resolve;});const cancelled:string[]=[];
  f.host.setAgentBroker({run:async()=>{entered();await new Promise<void>(resolve=>{release=resolve;});return {runId:'late-run'};},cancelRemoteSession:async(owner:any)=>{cancelled.push(owner.remoteSessionId);}});
  const controller=new AbortController();
  const pending=f.host.remoteInvoke({appId:id,revision:1,runId:'run-one'},'agent.tasks.start',{input:{}},controller.signal,'s'.repeat(43));
  await enteredPromise;controller.abort();release();await pending;assert.deepEqual(cancelled,['s'.repeat(43)]);
});

test('remote Agent mutations receive the browser session cancellation signal', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  const controller=new AbortController(), seen:string[]=[];
  const check=(name:string)=>(...args:any[])=>{assert.equal(args.at(-1),controller.signal);seen.push(name);return name==='sendChat'?{runId:'chat-run'}:undefined;};
  f.host.setAgentBroker({cancel:check('cancel'),createChat:check('createChat'),archiveChat:check('archiveChat'),deleteChat:check('deleteChat'),sendChat:check('sendChat'),cancelChat:check('cancelChat'),cancelRemoteSession:async()=>{}});
  const run={appId:id,revision:1,runId:'run-one'}, session='s'.repeat(43);
  for(const method of ['agent.tasks.cancel','agent.chats.create','agent.chats.archive','agent.chats.delete','agent.chats.send','agent.chats.cancel']) {
    await f.host.remoteInvoke(run,method,{runId:'task',chatId:'chat',input:{}},controller.signal,session);
  }
  assert.deepEqual(seen,['cancel','createChat','archiveChat','deleteChat','sendChat','cancelChat']);
});

test('remote Agent request cannot reuse authorization across app withdrawal and regrant', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  let release!:()=>void, entered!:()=>void, calls=0;
  const gate=new Promise<void>(resolve=>{release=resolve;}), waiting=new Promise<void>(resolve=>{entered=resolve;});
  const authorize=f.host.authorizeAgent.bind(f.host);
  f.host.authorizeAgent=async (...args:any[])=>{const result=await authorize(...args);entered();await gate;return result;};
  f.host.setAgentBroker({cancelApp:async()=>{},archiveChat:async()=>{calls++;}});
  const run={appId:id,revision:1,runId:'run-one'}, session=new AbortController();
  const pending=assert.rejects(f.host.remoteInvoke(run,'agent.chats.archive',{chatId:'chat'},session.signal,'s'.repeat(43)));
  await waiting;await f.host.cancelAgentApp(id,'changed');f.host.blockedAgentApps.delete(id);release();await pending;
  assert.equal(calls,0);
  await f.host.remoteInvoke(run,'agent.chats.archive',{chatId:'chat'},session.signal,'s'.repeat(43));
  assert.equal(calls,1);
});

test('remote browser access keeps the host alive and its unused headless run stops', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  assert.equal(await f.host.hasBackgroundApps(),true);
  f.host.remoteRevokeOwner(owner);
  assert.equal(await f.host.hasBackgroundApps(),false);
  await f.host.remoteAccessEnded(id,'run-one');
  assert.equal(f.runtime.phase(),'stopped');
});

test('the last revoked Agent session reconciles its run after cancellation settles', async t => {
  const f=await fixture(t);f.row.agent.background=true;await f.host.setRemoteAccess(id,true);await f.host.remoteOpen(id,owner);
  let active=true, release!:()=>void;const cancelling=new Promise<void>(resolve=>{release=resolve;});
  f.host.setAgentBroker({run:async()=>({runId:'agent-run'}),hasActiveApp:()=>active,cancelApp:async()=>{},cancelRemoteSession:async()=>{await cancelling;active=false;}});
  const controller=new AbortController();
  await f.host.remoteInvoke({appId:id,revision:1,runId:'run-one'},'agent.tasks.start',{input:{}},controller.signal,'s'.repeat(43));
  f.host.remoteRevokeOwner(owner);controller.abort();await f.host.remoteAccessEnded(id,'run-one');assert.equal(f.runtime.phase(),'running');
  release();for(let attempt=0;attempt<30 && f.runtime.phase()==='running';attempt++)await new Promise(resolve=>setTimeout(resolve,5));
  assert.equal(f.runtime.phase(),'stopped');
});

test('a settled Agent from an older invocation cannot clean up its replacement', async t => {
  const f=await fixture(t);let notify!:(owner:any)=>void, calls=0, unsubscribed=false;
  f.host.setAgentBroker({onSettled:(listener:any)=>{notify=listener;return()=>{unsubscribed=true;};}});
  f.host.remoteAccessEnded=async()=>{calls++;};
  f.host.remoteManagedRuns.set(id,'replacement');
  notify({appId:id,invocationId:'old'});assert.equal(calls,0);
  notify({appId:id,invocationId:'replacement'});assert.equal(calls,1);
  f.host.setAgentBroker({});assert.equal(unsubscribed,true);
});

test('the last remote access closes idle RTC before rechecking process lifetime', async t => {
  const f=await fixture(t);await f.host.setRemoteAccess(id,true);await f.host.remoteOpen(id,owner);
  const events:string[]=[];f.host.options.onRemoteAccessEnded=()=>{events.push('check-idle');};
  f.host.setRemoteRtc({releaseIdle(){events.push('release-rtc');},dispose(){}});
  await f.host.remoteAccessEnded(id,'run-one');assert.equal(events.includes('release-rtc'),false);
  events.length=0;f.host.remoteRevokeOwner(owner);await f.host.remoteAccessEnded(id,'run-one');
  assert.ok(events.indexOf('release-rtc')>=0);assert.ok(events.indexOf('release-rtc')<events.indexOf('check-idle'));
});

test('remote launch deadline and revocation abort actual startup work', async t => {
  for (const revoke of [false, true]) {
    const f=await fixture(t,revoke ? undefined : 10);await f.host.request({action:'remote-access',id,enabled:true});
    let entered!:()=>void;const started=new Promise<void>(resolve=>{entered=resolve;});
    f.setStartHook(signal=>new Promise<void>((_resolve,reject)=>{entered();signal!.addEventListener('abort',()=>reject(signal!.reason),{once:true});}));
    const pending=f.host.remoteOpen(id,owner);const rejected=assert.rejects(pending,revoke ? /changed/ : /timed out/);
    await started;if(revoke)f.host.remoteRevokeOwner(owner);await rejected;
    assert.equal(f.runtime.phase(),'stopped');assert.equal(f.liveSession,false);assert.equal(f.host.remoteOpening.size,0);
  }
});

test('a runtime activation committed after launch timeout is still cleaned up', async t => {
  const f=await fixture(t,10);await f.host.request({action:'remote-access',id,enabled:true});f.ignoreStartAbort();
  let release!:()=>void;f.setStartHook(()=>new Promise<void>(resolve=>{release=resolve;}));
  await assert.rejects(f.host.remoteOpen(id,owner),/timed out/);assert.equal(f.host.remoteOpening.size,0);
  release();
  for(let count=0;count<20;count++){await new Promise(resolve=>setTimeout(resolve,5));if(!f.host.remoteManagedRuns.has(id))break;}
  assert.equal(f.runtime.phase(),'stopped');assert.equal(f.liveSession,false);
});

test('foreground browser access survives disabling independent background and Agent grants', async t => {
  const f=await fixture(t);f.row.background=true;await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  await f.host.request({action:'background',id,enabled:false});
  assert.equal(f.runtime.phase(),'running');assert.equal(f.liveSession,true);
  await f.host.request({action:'agent-settings',id,settings:{...f.row.agent,enabled:false}});
  assert.equal(f.runtime.phase(),'running');assert.equal(f.liveSession,true);
  assert.equal(f.row.grants.includes('agent'),false);
});

test('sharing rejects a 257th app without breaking the existing catalog settings', async t => {
  const f=await fixture(t);const enabled=Array.from({length:256},(_,i)=>`a${i.toString(16).padStart(32,'0')}`);
  f.store.list=async()=>[f.row,...enabled.map(id=>({...f.row,id}))];
  await fs.writeFile(f.host.remoteSettingsPath,JSON.stringify({version:1,enabled}));
  await assert.rejects(f.host.setRemoteAccess(id,true),/256/);assert.equal(Object.keys(await f.host.remoteSettings()).length,256);
  await f.host.setRemoteAccess(enabled[0],false);await f.host.setRemoteAccess(id,true);assert.equal((await f.host.remoteSettings())[id],true);
});

test('deleted apps do not consume sharing slots', async t => {
  const f=await fixture(t);const enabled=Array.from({length:256},(_,i)=>`a${i.toString(16).padStart(32,'0')}`);
  await fs.writeFile(f.host.remoteSettingsPath,JSON.stringify({version:1,enabled}));
  await f.host.setRemoteAccess(id,true);assert.deepEqual(Object.keys(await f.host.remoteSettings()),[id]);
  f.runtime.remove=async()=>{f.setRun(false);};await f.host.request({action:'remove',id});assert.equal((await f.host.remoteSettings())[id],undefined);
});

test('a rejected cleanup does not poison the next queued cleanup', async t => {
  const f=await fixture(t);let reject!:(reason:Error)=>void;let called=false;
  const first=f.host.queueRunCleanup(id,()=>new Promise<void>((_resolve,fail)=>{reject=fail;}));
  const failed=assert.rejects(first,/cleanup failed/);const next=f.host.queueRunCleanup(id,async()=>{called=true;});
  await new Promise(resolve=>setTimeout(resolve,0));reject(new Error('cleanup failed'));await failed;await next;assert.equal(called,true);
});

test('revoking another device does not abort this devices pending launch or existing session', async t => {
  const f=await fixture(t);await f.host.setRemoteAccess(id,true);await f.host.remoteOpen(id,owner);
  let release!:()=>void,entered!:()=>void;const started=new Promise<void>(resolve=>{entered=resolve;});
  f.setStartHook(()=>new Promise<void>(resolve=>{release=resolve;entered();}));
  const pending=f.host.remoteOpen(id,owner);await started;f.host.remoteRevokeOwner({...owner,deviceId:'other-phone'});assert.equal(f.liveSession,true);
  release();assert.equal((await pending).app_id,id);assert.equal(f.liveSession,true);
});

test('disabling or deleting another app does not cancel this apps pending launch', async t => {
  for(const action of ['disable','remove']){
    const f=await fixture(t),other=`a${'2'.repeat(32)}`;
    f.store.get=async(requested:string)=>structuredClone({...f.row,id:requested});f.store.list=async()=>[f.row,{...f.row,id:other}];f.runtime.remove=async()=>{};
    await f.host.setRemoteAccess(id,true);let release!:()=>void,entered!:()=>void;const started=new Promise<void>(resolve=>{entered=resolve;});
    f.setStartHook(()=>new Promise<void>(resolve=>{release=resolve;entered();}));const pending=f.host.remoteOpen(id,owner);await started;
    if(action==='remove')await f.host.request({action:'remove',id:other});else await f.host.setRemoteAccess(other,false);
    release();assert.equal((await pending).app_id,id);assert.equal(f.liveSession,true);
  }
});

test('closing Desktop while remote startup is pending preserves the shared run', async t => {
  const f=await fixture(t);await f.host.setRemoteAccess(id,true);f.setRun(true);
  let release!:()=>void,entered!:()=>void;const started=new Promise<void>(resolve=>{entered=resolve;});let cancelled=0;
  f.host.setAgentBroker({cancelApp:async()=>{cancelled++;}});f.setStartHook(()=>new Promise<void>(resolve=>{release=resolve;entered();}));
  const pending=f.host.remoteOpen(id,owner);await started;
  await f.host.queueRunCleanup(id,()=>f.host.desktopWindowClosed(id,'run-one',1));assert.equal(cancelled,0);assert.equal(f.runtime.phase(),'running');
  release();await pending;assert.equal(f.liveSession,true);
});

test('failed Desktop cache setup and navigation preserve an existing browser run', async t => {
  for(const beforeWindow of [true,false]){
    const f=await fixture(t);await f.host.setRemoteAccess(id,true);await f.host.remoteOpen(id,owner);
    f.host.configureSession=async()=>{if(beforeWindow)throw new Error('cache failed');};f.host.clearCodeCache=async()=>{};
    await assert.rejects(f.host.open(id),beforeWindow?/cache failed/:/load failed/);
    assert.equal(f.runtime.phase(),'running');assert.equal(f.liveSession,true);
  }
});

test('cleanup notifies the app idle checker after the last remote run stops', async t => {
  const f=await fixture(t);await f.host.setRemoteAccess(id,true);await f.host.remoteOpen(id,owner);
  const before=f.ended;f.host.remoteRevokeOwner(owner);await f.host.remoteAccessEnded(id,'run-one');
  assert.equal(f.runtime.phase(),'stopped');assert.ok(f.ended>before);
});

test('a failed remote opening releases a formerly background-only run', async t => {
  const f=await fixture(t);f.row.background=true;f.setRun(true);await f.host.request({action:'remote-access',id,enabled:true});
  let reject!: (error:Error)=>void,entered!:()=>void;const authorizing=new Promise<void>(resolve=>{entered=resolve;});
  f.host.authorize=async()=>{entered();await new Promise<void>((_resolve,fail)=>{reject=fail;});};
  const pending=f.host.remoteOpen(id,owner);const failed=assert.rejects(pending,/denied/);await authorizing;
  await f.host.request({action:'background',id,enabled:false});assert.equal(f.runtime.phase(),'running');
  reject(new Error('denied'));await failed;await f.host.remoteAccessEnded(id,'run-one');assert.equal(f.runtime.phase(),'stopped');
});

test('remote HTML injection bounds memory and cancels on request abort', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  f.row.manifest.entry={type:'server',process:'api'};f.runtime.workspace=()=>'/unused';f.runtime.service=()=>({port:12345});
  let cancelled=false;
  f.host.services.fetch=async()=>new Response(new ReadableStream<Uint8Array>({pull(controller){controller.enqueue(new Uint8Array(1024*1024));},cancel(){cancelled=true;}}),{headers:{'content-type':'text/html'}});
  const url=`https://${id}-${'1'.repeat(20)}.apps.example.com/`;
  await assert.rejects(f.host.remoteResource({appId:id,revision:1,runId:'run-one'},new Request(url)),/HTML exceeds/);
  assert.equal(cancelled,true);assert.equal(f.host.remoteHtmlBytes,0);
  let entered!:()=>void;const reading=new Promise<void>(resolve=>{entered=resolve;});cancelled=false;
  f.host.services.fetch=async()=>new Response(new ReadableStream<Uint8Array>({pull(){entered();},cancel(){cancelled=true;}}),{headers:{'content-type':'text/html'}});
  const controller=new AbortController();const pending=f.host.remoteResource({appId:id,revision:1,runId:'run-one'},new Request(url,{signal:controller.signal}));
  await reading;controller.abort();await assert.rejects(pending,/abort/i);assert.equal(cancelled,true);assert.equal(f.host.remoteHtmlBytes,0);
});

test('remote HTML leaves HEAD and no-body response statuses empty', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  f.row.manifest.entry={type:'server',process:'api'};f.runtime.workspace=()=>'/unused';f.runtime.service=()=>({port:12345});
  const run={appId:id,revision:1,runId:'run-one'},url=`https://${id}-${'1'.repeat(20)}.apps.example.com/`;
  for(const status of [204,205,304]){
    f.host.services.fetch=async()=>new Response(null,{status,headers:{'content-type':'text/html'}});
    const response=await f.host.remoteResource(run,new Request(url));assert.equal(response.status,status);assert.equal(response.body,null);
  }
  let cancelled=false;f.host.services.fetch=async()=>new Response(new ReadableStream({cancel(){cancelled=true;}}),{headers:{'content-type':'text/html'}});
  const response=await f.host.remoteResource(run,new Request(url,{method:'HEAD'}));assert.equal(response.body,null);assert.equal(cancelled,true);
});

test('remote HTML rejects unsupported encodings before injecting into encoded bytes', async t => {
  const f=await fixture(t);await f.host.setRemoteAccess(id,true);await f.host.remoteOpen(id,owner);
  f.row.manifest.entry={type:'server',process:'api'};f.runtime.workspace=()=>'/unused';f.runtime.service=()=>({port:12345});
  const run={appId:id,revision:1,runId:'run-one'},url=`https://${id}-${'1'.repeat(20)}.apps.example.com/`;
  for(const coding of ['custom','gzip, custom']){
    let cancelled=false;f.host.services.fetch=async()=>{
      const response=new Response(new Uint8Array([0xff,0,0x80]),{headers:{'content-type':'text/html','content-encoding':coding}});
      const cancel=response.body!.cancel.bind(response.body);response.body!.cancel=(reason?:unknown)=>{cancelled=true;return cancel(reason);};return response;
    };
    const response=await f.host.remoteResource(run,new Request(url));assert.equal(response.status,502);assert.equal(cancelled,true);assert.doesNotMatch(await response.text(),/__lingxi_local_app_sdk/);
  }
  f.host.services.fetch=async()=>new Response('<html>identity</html>',{headers:{'content-type':'text/html','content-encoding':'identity'}});
  assert.match(await (await f.host.remoteResource(run,new Request(url))).text(),/__lingxi_local_app_sdk/);
  f.host.services.fetch=async()=>new Response(null,{headers:{'content-type':'text/html','content-encoding':'custom'}});
  assert.equal((await f.host.remoteResource(run,new Request(url,{method:'HEAD'}))).status,502);
});

test('remote host proxies login cookies, trusted forwarding headers and only local service redirects', async t => {
  const f=await fixture(t);const received:Array<{path:string;headers:import('node:http').IncomingHttpHeaders;body:string}>=[];
  const server=createServer((req,res)=>{let body='';req.on('data',chunk=>{body+=chunk;});req.on('end',()=>{
    received.push({path:req.url ?? '',headers:req.headers,body});
    const port=(server.address() as {port:number}).port;
    const target=req.url === '/external' ? 'https://accounts.example.org/login?next=foo' : req.url === '/other-port' ? `http://localhost:${port+1}/wrong` : req.url === '/relative' ? '/relative-path' : req.url === '/localhost' ? `http://localhost:${port}/local` : req.url === '/lookalike' ? `http://127.0.0.1.evil.example:${port}/other` : `http://127.0.0.1:${port}/account?view=one#top`;
    res.writeHead(302,{'location':target,'set-cookie':'auth=token; Path=/; HttpOnly'}).end();
  });});
  await new Promise<void>(resolve=>server.listen(0,'127.0.0.1',resolve));const port=(server.address() as {port:number}).port;
  f.row.manifest.entry={type:'server',process:'api'}; f.runtime.workspace=()=>'/unused';f.runtime.service=(_app:string,name:string)=>name==='api'?{port}:undefined;
  try {
    await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
    const run={appId:id,revision:1,runId:'run-one'}, origin=`https://${id}-${'1'.repeat(20)}.apps.example.com`;
    const login=await f.host.remoteResource(run,new Request(origin+'/login',{method:'POST',body:'credentials',headers:{cookie:'auth=prior','x-forwarded-host':'evil.example','x-forwarded-proto':'http','x-forwarded-port':'123','forwarded':'host=evil','x-real-ip':'1.2.3.4'}}));
    assert.equal(login.status,302);assert.equal(login.headers.get('location'),origin+'/account?view=one#top');assert.match(login.headers.get('set-cookie') ?? '',/auth=token/);
    assert.equal(received[0]!.body,'credentials');assert.equal(received[0]!.headers.cookie,'auth=prior');assert.equal(received[0]!.headers['x-forwarded-host'],new URL(origin).host);assert.equal(received[0]!.headers['x-forwarded-proto'],'https');
    assert.equal(received[0]!.headers['x-forwarded-port'],undefined);assert.equal(received[0]!.headers.forwarded,undefined);assert.equal(received[0]!.headers['x-real-ip'],undefined);
    const named=await f.host.remoteResource(run,new Request(origin+'/__services/api/login'));
    assert.equal(named.headers.get('location'),origin+'/__services/api/account?view=one#top');assert.equal(received[1]!.path,'/login');
    const external=await f.host.remoteResource(run,new Request(origin+'/external'));assert.equal(external.headers.get('location'),'https://accounts.example.org/login?next=foo');
    const other=await f.host.remoteResource(run,new Request(origin+'/other-port'));assert.equal(other.headers.get('location'),`http://localhost:${port+1}/wrong`);
    const relative=await f.host.remoteResource(run,new Request(origin+'/relative'));assert.equal(relative.headers.get('location'),'/relative-path');
    const local=await f.host.remoteResource(run,new Request(origin+'/localhost'));assert.equal(local.headers.get('location'),origin+'/local');
    const lookalike=await f.host.remoteResource(run,new Request(origin+'/lookalike'));assert.equal(lookalike.headers.get('location'),`http://127.0.0.1.evil.example:${port}/other`);
    const stream = new ReadableStream<Uint8Array>({start(controller){controller.enqueue(new TextEncoder().encode('streamed'));controller.close();}});
    const streamed=await f.host.remoteResource(run,new Request(origin+'/stream',{method:'POST',body:stream,duplex:'half'} as RequestInit));
    assert.equal(streamed.status,302);assert.equal(received.at(-1)!.body,'streamed');
  } finally {await f.host.services.dispose();server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));}
});

test('named service root redirects stay within service and HTML ranges remain unmodified', async t => {
  const f=await fixture(t);
  const server=createServer((req,res)=>{
    if(req.url==='/login') return void res.writeHead(302,{location:'/account?tab=one#top'}).end();
    if(req.url==='/partial') return void res.writeHead(206,{'content-type':'text/html','content-range':'bytes 0-4/10','etag':'"version"'}).end('<html');
    res.writeHead(200,{'content-type':'text/html','content-length':'10','etag':'"original"','content-digest':'original-digest'}).end('<html></h>');
  });
  await new Promise<void>(resolve=>server.listen(0,'127.0.0.1',resolve));const port=(server.address() as {port:number}).port;
  f.row.manifest.entry={type:'server',process:'api'};f.runtime.workspace=()=>'/unused';f.runtime.service=()=>({port});
  try {
    await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
    const run={appId:id,revision:1,runId:'run-one'},origin=`https://${id}-${'1'.repeat(20)}.apps.example.com`;
    const redirected=await f.host.remoteResource(run,new Request(origin+'/__services/api/login'));
    const partial=await f.host.remoteResource(run,new Request(origin+'/partial',{headers:{range:'bytes=0-4'}}));
    const head=await f.host.remoteResource(run,new Request(origin+'/',{method:'HEAD'}));
    const full=await f.host.remoteResource(run,new Request(origin+'/'));
    assert.match(await full.text(),/__lingxi_local_app_sdk/);assert.equal(full.headers.get('etag'),null);assert.equal(full.headers.get('content-digest'),null);assert.equal(head.headers.get('etag'),null);assert.equal(partial.headers.get('etag'),'"version"');
    assert.deepEqual({location:redirected.headers.get('location'),status:partial.status,range:partial.headers.get('content-range'),body:await partial.text(),headLength:head.headers.get('content-length')},{location:origin+'/__services/api/account?tab=one#top',status:206,range:'bytes 0-4/10',body:'<html',headLength:null});
  } finally {await f.host.services.dispose();server.closeAllConnections();await new Promise<void>(resolve=>server.close(()=>resolve()));}
});
