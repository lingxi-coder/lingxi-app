import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalAppAgentBroker, type LocalAppAgentAuthorization } from '../src/main/local-apps/agentBroker.ts';

const model={providerId:'p',modelId:'m',label:'Model',canSelect:true,supportsAttachments:true,supportsVision:true,supportsStructuredOutput:true};
const authorization: LocalAppAgentAuthorization = {settings:{enabled:true,model,tools:[{id:'builtin:Read',kind:'builtin',name:'Read'}],skills:['extract-text'],background:false,maxTurns:20,timeoutMs:600000},requestedTools:['builtin:Read'],requestedSkills:['extract-text'],releaseRevision:1,releaseSourceDirectory:'/release',skillSpecs:[{id:'extract-text',path:'skills/extract-text/SKILL.md'}],releaseFiles:[{path:'skills/extract-text/SKILL.md',sha256:'a'.repeat(64),bytes:18,executable:false}],model};

test('broker preserves remote session identity for later task revalidation', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'remote-agent-owner-'));let valid=true;const observed:string[]=[];
  let continueExecution!:()=>void, started!:()=>void;
  const entered=new Promise<void>(resolve=>{started=resolve;});
  const broker=new LocalAppAgentBroker({root,authorize:async owner=>{observed.push(owner.remoteSessionId ?? 'missing');return authorization;},revalidate:async owner=>{observed.push(owner.remoteSessionId ?? 'missing');return valid;},execute:async execution=>{started();await new Promise<void>(resolve=>{continueExecution=resolve;});await execution.assertAuthorized();return {result:{text:'done',output:{ok:true},attachments:[],messageId:'m'},usage:{turns:1}};}});
  t.after(async()=>{await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const {runId}=await broker.run(owner,input);await entered;valid=false;continueExecution();
  const deadline=Date.now()+3000;let task=await broker.get(owner,runId);
  while(!['failed','cancelled','interrupted','succeeded'].includes(task.state) && Date.now()<deadline){await new Promise(resolve=>setTimeout(resolve,5));task=await broker.get(owner,runId);}
  assert.notEqual(task.state,'succeeded');assert.ok(observed.length>=2);assert.ok(observed.every(value=>value===owner.remoteSessionId));
});

test('session revocation cancels its actual active task after a terminal retry and leaves other sessions running', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'remote-agent-cancel-'));
  const running=new Map<string,()=>void>();
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async execution=>{
    await new Promise<void>((resolve,reject)=>{running.set(execution.runId,resolve);execution.signal.addEventListener('abort',()=>reject(execution.signal.reason),{once:true});});
    return {result:{text:'done',output:{ok:true},attachments:[],messageId:'m'},usage:{turns:1}};
  }});
  t.after(async()=>{await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const waitFor=async(condition:()=>Promise<boolean>)=>{const until=Date.now()+3000;while(!await condition()){assert.ok(Date.now()<until,'Agent task did not settle');await new Promise(resolve=>setTimeout(resolve,5));}};
  const old=await broker.run(owner,input);await waitFor(async()=>running.has(old.runId));running.get(old.runId)!();
  await waitFor(async()=>(await broker.get(owner,old.runId)).state==='succeeded');
  const next=await broker.run(owner,{...input,runId:'active-second'});await waitFor(async()=>running.has(next.runId));
  assert.deepEqual(await broker.run(owner,input),old);
  await broker.cancelRemoteSession(owner);assert.equal((await broker.get(owner,next.runId)).state,'cancelled');
  const other={...owner,remoteSessionId:'t'.repeat(43)};
  const live=await broker.run(other,{...input,runId:'active-other-session'});await waitFor(async()=>running.has(live.runId));
  await broker.run(owner,{...input,runId:'active-other-session'});
  await broker.cancelRemoteSession(owner);assert.equal((await broker.get(other,live.runId)).state,'running');
  await broker.cancelRemoteSession(other);assert.equal((await broker.get(other,live.runId)).state,'cancelled');
});

test('revoked queued chat mutations and admissions do not commit after the ledger lock opens', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'remote-agent-queued-revoke-'));
  let release!:()=>void, entered!:()=>void;
  const blocked=new Promise<void>(resolve=>{release=resolve;});
  const waiting=new Promise<void>(resolve=>{entered=resolve;});
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const other={appId:`a${'2'.repeat(32)}`,releaseDigest:'2'.repeat(64),invocationId:'other',remoteSessionId:'t'.repeat(43)};
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const broker=new LocalAppAgentBroker({root,authorize:async principal=>{
    if(principal.appId===other.appId){entered();await blocked;}
    return authorization;
  },revalidate:async()=>true,execute:async()=>({result:{text:'done',output:{ok:true},attachments:[],messageId:'m'},usage:{turns:1}})});
  t.after(async()=>{release();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const chat=await broker.createChat(owner,{title:'Keep this chat'});
  const lockOwner=broker.run(other,{...input,runId:'blocking-task'});await waiting;
  const controller=new AbortController();
  const deleteResult=assert.rejects(broker.deleteChat(owner,chat.chatId,controller.signal),/revoked/);
  const archiveResult=assert.rejects(broker.archiveChat(owner,chat.chatId,controller.signal),/revoked/);
  const runResult=assert.rejects(broker.run(owner,{...input,runId:'revoked-task'},controller.signal),/revoked/);
  controller.abort(new Error('revoked'));release();
  await Promise.all([deleteResult,archiveResult,runResult,lockOwner]);
  assert.equal((await broker.getChat(owner,chat.chatId)).archived,false);
  assert.equal((await broker.list(owner)).tasks.some(task=>task.runId==='revoked-task'),false);
  assert.ok((await fs.stat(join(root,'apps',owner.appId,'agent','chats',chat.chatId,'chat.json'))).isFile());
});

test('session revocation aborts its active executor without waiting for the ledger lock', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'remote-agent-immediate-abort-'));
  let release!:()=>void, entered!:()=>void, executionStarted!:(signal:AbortSignal)=>void;
  const blocked=new Promise<void>(resolve=>{release=resolve;});
  const waiting=new Promise<void>(resolve=>{entered=resolve;});
  const started=new Promise<AbortSignal>(resolve=>{executionStarted=resolve;});
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const other={appId:`a${'2'.repeat(32)}`,releaseDigest:'2'.repeat(64),invocationId:'other',remoteSessionId:'t'.repeat(43)};
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const broker=new LocalAppAgentBroker({root,authorize:async principal=>{
    if(principal.appId===other.appId){entered();await blocked;}
    return authorization;
  },revalidate:async()=>true,execute:async execution=>{
    if(execution.owner.appId===owner.appId){
      executionStarted(execution.signal);
      await new Promise<void>((_resolve,reject)=>execution.signal.addEventListener('abort',()=>reject(execution.signal.reason),{once:true}));
    }
    return {result:{text:'done',output:{ok:true},attachments:[],messageId:'m'},usage:{turns:1}};
  }});
  t.after(async()=>{release();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const active=await broker.run(owner,{...input,runId:'running-task'});const signal=await started;
  const lockOwner=broker.run(other,{...input,runId:'blocking-task'});await waiting;
  const cancellation=broker.cancelRemoteSession(owner);
  assert.equal(signal.aborted,true,'the executor must receive cancellation while the ledger is locked');
  release();await Promise.all([cancellation,lockOwner]);
  assert.equal((await broker.get(owner,active.runId)).state,'cancelled');
});

test('revoked chat creation removes its incomplete directory and remains restartable', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'remote-agent-create-revoke-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const options={root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>({result:{text:'done',attachments:[] as [],messageId:'m'},usage:{turns:1}})};
  const broker=new LocalAppAgentBroker(options);
  t.after(async()=>{await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  let release!:()=>void, entered!:()=>void;
  const gate=new Promise<void>(resolve=>{release=resolve;});
  const waiting=new Promise<void>(resolve=>{entered=resolve;});
  const internal=broker as any, save=internal.saveChat.bind(broker);
  internal.saveChat=async (...args:any[])=>{entered();await gate;return save(...args);};
  const controller=new AbortController();
  const pending=assert.rejects(broker.createChat(owner,{title:'Revoked'},controller.signal),/revoked/);
  await waiting;controller.abort(new Error('revoked'));release();await pending;
  assert.deepEqual(await fs.readdir(join(root,'apps',owner.appId,'agent','chats')),[]);
  const restarted=new LocalAppAgentBroker(options);
  try {await restarted.initialize();assert.deepEqual((await restarted.listChats(owner)).chats,[]);}
  finally {await restarted.dispose();}
});

test('revocation before an archive rename preserves the original chat', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'remote-agent-archive-revoke-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>({result:{text:'done',attachments:[],messageId:'m'},usage:{turns:1}})});
  t.after(async()=>{await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const chat=await broker.createChat(owner,{title:'Keep this chat'});
  const target=join(root,'apps',owner.appId,'agent','chats',chat.chatId,'chat.json');
  const before=await fs.readFile(target,'utf8');
  const writable=fs as any, writeFile=writable.writeFile;
  let release!:()=>void, entered!:()=>void;
  const blocked=new Promise<void>(resolve=>{release=resolve;});
  const waiting=new Promise<void>(resolve=>{entered=resolve;});
  writable.writeFile=async (...args:any[])=>{
    const result=await writeFile(...args);
    if(typeof args[0]==='string' && args[0].startsWith(`${target}.`) && args[0].endsWith('.tmp')){entered();await blocked;}
    return result;
  };
  const controller=new AbortController();
  try {
    const pending=assert.rejects(broker.archiveChat(owner,chat.chatId,controller.signal),/revoked/);
    await waiting;controller.abort(new Error('revoked'));release();await pending;
    assert.equal((await broker.getChat(owner,chat.chatId)).archived,false);
    assert.equal(await fs.readFile(target,'utf8'),before);
    assert.deepEqual((await fs.readdir(join(root,'apps',owner.appId,'agent','chats',chat.chatId))).sort(),['assets','chat.json']);
  } finally { release();writable.writeFile=writeFile; }
});

test('app revocation aborts an executor while another app holds the ledger lock', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-app-abort-'));
  let release!:()=>void, entered!:()=>void, started!:(signal:AbortSignal)=>void;
  const gate=new Promise<void>(resolve=>{release=resolve;}), locked=new Promise<void>(resolve=>{entered=resolve;}), executing=new Promise<AbortSignal>(resolve=>{started=resolve;});
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)}, other={...owner,appId:`a${'2'.repeat(32)}`};
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const broker=new LocalAppAgentBroker({root,authorize:async principal=>{if(principal.appId===other.appId){entered();await gate;}return authorization;},revalidate:async()=>true,execute:async execution=>{
    if(execution.owner.appId===owner.appId){started(execution.signal);await new Promise<void>((_resolve,reject)=>execution.signal.addEventListener('abort',()=>reject(execution.signal.reason),{once:true}));}
    return {result:{text:'done',attachments:[],messageId:'m'},usage:{turns:1}};
  }});
  t.after(async()=>{release();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const run=await broker.run(owner,{...input,runId:'active'});const signal=await executing;
  const blocker=broker.run(other,{...input,runId:'blocker'});await locked;
  const cancellation=broker.cancelApp(owner.appId);
  const aborted=signal.aborted;release();await Promise.all([cancellation,blocker]);assert.equal(aborted,true);assert.equal((await broker.get(owner,run.runId)).state,'cancelled');
});

test('revocation before queued success commits prevents assistant output', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-success-revoke-'));
  let releaseLock!:()=>void, enteredLock!:()=>void, releaseResult!:()=>void, executing!:()=>void, successQueued!:()=>void;
  const lockGate=new Promise<void>(resolve=>{releaseLock=resolve;}), locked=new Promise<void>(resolve=>{enteredLock=resolve;}), resultGate=new Promise<void>(resolve=>{releaseResult=resolve;}), started=new Promise<void>(resolve=>{executing=resolve;}), queued=new Promise<void>(resolve=>{successQueued=resolve;});
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)}, other={...owner,appId:`a${'2'.repeat(32)}`};
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const broker=new LocalAppAgentBroker({root,authorize:async principal=>{if(principal.appId===other.appId){enteredLock();await lockGate;}return authorization;},revalidate:async()=>true,execute:async execution=>{
    if(execution.owner.appId===owner.appId){executing();await resultGate;}
    return {result:{text:'secret result',output:{ok:true},attachments:[],messageId:'assistant'},usage:{turns:1}};
  }});
  t.after(async()=>{releaseLock();releaseResult();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const internal=broker as any, finish=internal.finish.bind(broker);
  internal.finish=(...args:any[])=>{if(args[1]==='succeeded' && args[0].includes('target'))successQueued();return finish(...args);};
  const run=await broker.run(owner,{...input,runId:'target'});await started;
  const blocker=broker.run(other,{...input,runId:'blocker'});await locked;releaseResult();await queued;
  const cancellation=broker.cancelRemoteSession(owner);releaseLock();await Promise.all([cancellation,blocker]);
  assert.equal((await broker.get(owner,run.runId)).state,'cancelled');
  const events=(await broker.events(owner,run.runId)).events;
  assert.equal(events.some(event=>event.type.startsWith('TEXT_MESSAGE_')),false);
  assert.equal(events.some(event=>event.type==='RUN_FINISHED' && !('outcome' in event)),false);
});

test('chat reads wait for aborted admission rollback before exposing history', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-chat-admission-read-'));
  let release!:()=>void, entered!:()=>void;
  const gate=new Promise<void>(resolve=>{release=resolve;}), waiting=new Promise<void>(resolve=>{entered=resolve;});
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>({result:{text:'done',attachments:[],messageId:'m'},usage:{turns:1}})});
  t.after(async()=>{release();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const chat=await broker.createChat(owner,{title:'Chat'});
  const internal=broker as any, save=internal.saveChat.bind(broker);
  internal.saveChat=async (...args:any[])=>{await save(...args);entered();await gate;};
  const controller=new AbortController();
  const admission=assert.rejects(broker.sendChat(owner,chat.chatId,{...input,threadId:chat.chatId,runId:'aborted'},controller.signal),/revoked/);
  await waiting;let completed=0;
  const summary=broker.getChat(owner,chat.chatId).then(value=>{completed++;return value;}), list=broker.listChats(owner).then(value=>{completed++;return value;}), history=broker.chatHistory(owner,chat.chatId).then(value=>{completed++;return value;});
  await new Promise(resolve=>setImmediate(resolve));const observed=completed;
  controller.abort(new Error('revoked'));release();await admission;
  assert.equal(observed,0,'readers must not observe provisional chat state');
  assert.equal((await summary).state,'idle');assert.equal((await list).chats[0].state,'idle');assert.deepEqual((await history).messages,[]);
});
