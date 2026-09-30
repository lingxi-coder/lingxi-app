import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, realpathSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { HostController } from '../src/main/host';
import { DiagnosticBuffer } from '../src/main/host-utils';
import { SettingsStore } from '../src/main/settings';
import { CH_GIT_REQUEST } from '../src/shared/git';
test('Git IPC requires known session and trusted main frame, works without model, and releases watches', async () => {
 const dir=mkdtempSync(join(tmpdir(),'lingxi-git-host-'));const project=realpathSync(dir);
 const settings=new SettingsStore(join(dir,'settings'));settings.addProject(project);
 const scope={projectPath:project,sessionId:'11111111-2222-4333-8444-555555555555'};settings.setActiveSessionDraft(scope);
 const handlers=new Map<string,Function>();const ipc={handle:(key:string,fn:Function)=>handlers.set(key,fn),removeHandler:(key:string)=>handlers.delete(key)};
 const bridge={registerIpc(){},registerWindow(){},get:(id:string)=>id===scope.sessionId?{projectPath:project,connectionState:{status:'idle'}}:{projectPath:'/different'}};
 let watched=0,closed=0,calls=0;let repository=false;
 const service={onChanged:()=>()=>{},watch:async()=>{watched++;return()=>{closed++;};},request:async(_scope:unknown,request:any)=>{calls++;if(request.kind==='init')repository=true;return{status:{repository}};}};
 const host=new HostController(settings,bridge as any,new DiagnosticBuffer(),undefined,ipc as any);host.attachGit(service as any);
 const frame={url:'http://127.0.0.1:4242'};const sender={mainFrame:frame,isDestroyed:()=>false,once(){},send(){}};
 host.registerWindow(sender as any,frame.url);host.registerIpc();const event={sender,senderFrame:frame};
 const invoke=(owner:unknown,request:unknown,from:any=event)=>handlers.get(CH_GIT_REQUEST)!(from,owner,request);
 try {
  await assert.rejects(invoke(scope,{kind:'status'},{sender,senderFrame:{...frame}}),/unauthorized/);
  await assert.rejects(invoke({...scope,sessionId:'22222222-3333-4444-8555-666666666666'},{kind:'status'}),/different project/);
  await assert.rejects(invoke(scope,[]),/invalid Git request/);assert.equal(calls,0);
  assert.equal((await invoke(scope,{kind:'status'})).status.repository,false);
  await invoke(scope,{kind:'init'});await new Promise(resolve=>setImmediate(resolve));
  assert.equal(calls,2);assert.equal(watched,3);assert.equal(closed,2);
  // Project selection can use a draft scope while the host has already activated a session.
  assert.equal((await invoke({...scope,sessionId:'__draft__'},{kind:'status'})).status.repository,true);
  await assert.rejects(invoke({projectPath:join(dir,'unknown'),sessionId:'__draft__'},{kind:'status'}));
  host.dispose();await new Promise(resolve=>setImmediate(resolve));assert.equal(closed,watched);
 }finally{host.dispose();rmSync(dir,{recursive:true,force:true});}
});

/**
 * A watch is a live filesystem subscription held per renderer, per project.
 * `detachGit` releases them when a WINDOW goes away, and nothing released them
 * when a PROJECT did — so removing a project left its watcher running against a
 * directory the user had just told us to forget, feeding change events for a
 * scope no surface can display, for the rest of the session.
 */
test('removing a project releases the Git watch it left running', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'lingxi-git-remove-'));
  const project = realpathSync(dir);
  const settings = new SettingsStore(join(dir, 'settings'));
  settings.addProject(project);
  const scope = { projectPath: project, sessionId: '11111111-2222-4333-8444-555555555555' };
  settings.setActiveSessionDraft(scope);
  const handlers = new Map<string, Function>();
  const ipc = { handle: (key: string, fn: Function) => handlers.set(key, fn), removeHandler: (key: string) => handlers.delete(key) };
  const bridge = {
    registerIpc() {}, registerWindow() {}, hasActiveWork: () => false,
    get: () => ({ projectPath: project, connectionState: { status: 'idle' } }),
    closeProject: async () => undefined,
  };
  let watched = 0, closed = 0;
  const service = { onChanged: () => () => {}, watch: async () => { watched++; return () => { closed++; }; }, request: async () => ({ status: { repository: true } }) };
  const host = new HostController(settings, bridge as any, new DiagnosticBuffer(), undefined, ipc as any);
  host.attachGit(service as any);
  (host as any).bootstrap = () => ({ settings: settings.getPublic() });
  const frame = { url: 'http://127.0.0.1:4242' };
  const sender = { mainFrame: frame, isDestroyed: () => false, once() {}, send() {} };
  host.registerWindow(sender as any, frame.url);
  host.registerIpc();
  try {
    await handlers.get(CH_GIT_REQUEST)!({ sender, senderFrame: frame }, scope, { kind: 'status' });
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(watched, 1);
    assert.equal(closed, 0, 'precondition: the watch is live before the project is removed');

    await (host as any).removeProjectInternal(project);
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(closed, 1, 'removing the project must stop its watcher');
  } finally { host.dispose(); rmSync(dir, { recursive: true, force: true }); }
});
