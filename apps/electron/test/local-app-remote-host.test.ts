import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { runInNewContext } from 'node:vm';
import { build } from 'esbuild';
import { createServer } from 'node:http';

const require = createRequire(import.meta.url);
const bundledHost = build({ entryPoints: [new URL('../src/main/local-apps/host.ts', import.meta.url).pathname], bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'] });
const id = `a${'1'.repeat(32)}`;
const owner = { accountUid: 'account', deviceId: 'phone', hostId: 'desktop' };
const model = { providerId:'p', modelId:'m', label:'Model', canSelect:true, supportsAttachments:true, supportsVision:true, supportsStructuredOutput:true };

async function fixture(t: { after(fn: () => Promise<void>): void }) {
  const root = await fs.mkdtemp(join(tmpdir(),'remote-app-host-'));
  let windowCount = 0, running = false, revision = 1, runId = 'run-one', liveSession = false;
  let startHook = async () => {};
  const electron = { BrowserWindow: class { constructor() { windowCount++; } }, ipcMain:{removeHandler(){}}, dialog:{showMessageBox:async()=>({response:1})} };
  const module = { exports: {} };
  runInNewContext((await bundledHost).outputFiles[0]!.text, { module, exports:module.exports, require:(name:string)=>name === 'electron' ? electron : require(name), Buffer,process,console,structuredClone,URL,Request,Response,Headers,fetch,AbortController,AbortSignal,setTimeout,clearTimeout });
  const LocalAppHost = (module.exports as any).LocalAppHost;
  const host = new LocalAppHost({core:{onDisconnected(){}},configHome:root,preload:'/unused',mainWindow:()=>undefined,edit:async()=>{},agentCatalog:async()=>({models:[model],tools:[],skills:[]})});
  const row:any = {id,name:'Remote app',summary:'',revision:1,currentRevision:1,releaseDigest:'d'.repeat(64),background:false,grants:['agent'],phase:'stopped',manifest:{schema:2,targets:[],name:'Remote app',entry:{type:'static',path:'index.html'},capabilities:['agent'],agent:{tools:[],skills:[]}},agent:{enabled:true,model,tools:[],skills:[],background:false,maxTurns:5,timeoutMs:10000}};
  const store:any = {root, get:async()=>structuredClone(row), getRevision:async()=>structuredClone(row), list:async()=>[structuredClone(row)], assertTarget:async()=>{}, revisionPath:()=>root, core:{request:async()=>({digest:row.releaseDigest,files:[]})}, publish:async()=>{row.revision++;row.currentRevision=row.revision;return structuredClone(row);} };
  host.store = store;
  const runtime:any = {status:(value:any)=>({...value,phase:running?'running':'stopped',runningRevision:running?revision:undefined,runId:running?runId:undefined}),
    phase:()=>running?'running':'stopped', revision:()=>running?revision:undefined, invocationId:()=>running?runId:undefined,manifest:()=>row.manifest,
    owner:()=>running?({appId:id,runId,revision,releaseDigest:row.releaseDigest}):undefined,
    start:async()=>{await startHook(); running=true;revision=row.revision;}, stop:async()=>{running=false;},exclusive:async(_id:string,fn:()=>Promise<unknown>)=>fn()};
  host.runtime=runtime;
  const gateway:any = {open:async(run:any,requestedOwner:any)=>{assert.equal(requestedOwner.deviceId,owner.deviceId); liveSession=true;return {app_id:run.appId,revision:run.revision,url:`https://${id}-${'1'.repeat(20)}.apps.example.com/__lingxi_remote_bootstrap#ticket`,expires_at:Date.now()+60000};},
    hasAccess:()=>liveSession,hasRun:(_id:string,run:string)=>liveSession && run===runId,hasSession:(_id:string,run:string,session:string)=>liveSession && run===runId && session==='s'.repeat(43),revokeApp:()=>{liveSession=false;},revokeOwner:()=>{liveSession=false;},revokeAll:()=>{liveSession=false;},dispose:async()=>{liveSession=false;}};
  host.remoteGateway=gateway;
  host.setAgentBroker({cancelApp:async()=>{},dispose:async()=>{}});
  t.after(async()=>{ if (host.changeTimer) clearTimeout(host.changeTimer); await fs.rm(root,{recursive:true,force:true}); });
  return {host,row,store,runtime,get windowCount(){return windowCount;},setStartHook:(fn:()=>Promise<void>)=>{startHook=fn;},setRun:(value:boolean)=>{running=value;}, get liveSession(){return liveSession;}};
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

test('owner revoke, stop, release publication and disable invalidate a remote open during startup', async t => {
  const f=await fixture(t); await f.host.request({action:'remote-access',id,enabled:true});
  for (const action of ['owner','stop','publish','disable'] as const) {
    let release!:()=>void, enter!:()=>void;
    const entered=new Promise<void>(resolve=>{enter=resolve;});
    f.setRun(false);f.setStartHook(()=>new Promise<void>(resolve=>{release=resolve;enter();}));
    const opening=f.host.remoteOpen(id,owner);await entered;
    if(action==='owner')f.host.remoteRevokeOwner(owner);
    if(action==='stop')await f.host.request({action:'stop',id});
    if(action==='publish'){f.host.publishAuthoring=async()=>({});await f.host.request({action:'publish',id,html:'next'});}
    if(action==='disable')await f.host.request({action:'remote-access',id,enabled:false});
    release(); await assert.rejects(opening,/changed|not enabled/);assert.equal(f.liveSession,false);
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
  f.host.setAgentBroker({run:async()=>({runId:'agent-run'}),cancel:async(_owner:unknown,runId:string)=>{cancelled.push(runId);}});
  const controller=new AbortController();
  const result=await f.host.remoteInvoke({appId:id,revision:1,runId:'run-one'},'agent.tasks.start',{input:{}},controller.signal,'s'.repeat(43));
  assert.equal(result.runId,'agent-run'); controller.abort();
  await new Promise(resolve=>setTimeout(resolve,0));
  assert.deepEqual(cancelled,['agent-run']); assert.equal(f.liveSession,true);
});

test('Agent task admitted after browser revocation is immediately cancelled', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  let release!:()=>void, entered!:()=>void;const enteredPromise=new Promise<void>(resolve=>{entered=resolve;});const cancelled:string[]=[];
  f.host.setAgentBroker({run:async()=>{entered();await new Promise<void>(resolve=>{release=resolve;});return {runId:'late-run'};},cancel:async(_owner:unknown,runId:string)=>{cancelled.push(runId);}});
  const controller=new AbortController();
  const pending=f.host.remoteInvoke({appId:id,revision:1,runId:'run-one'},'agent.tasks.start',{input:{}},controller.signal,'s'.repeat(43));
  await enteredPromise;controller.abort();release();await pending;assert.deepEqual(cancelled,['late-run']);
});

test('remote browser access keeps the host alive and its unused headless run stops', async t => {
  const f=await fixture(t);await f.host.request({action:'remote-access',id,enabled:true});await f.host.remoteOpen(id,owner);
  assert.equal(await f.host.hasBackgroundApps(),true);
  f.host.remoteRevokeOwner(owner);
  assert.equal(await f.host.hasBackgroundApps(),false);
  await f.host.remoteAccessEnded(id,'run-one');
  assert.equal(f.runtime.phase(),'stopped');
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
