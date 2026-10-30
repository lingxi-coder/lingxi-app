import { test } from 'node:test';
import assert from 'node:assert/strict';
import { promises as fs, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import { LocalAppAgentBroker, type LocalAppAgentAuthorization } from '../src/main/local-apps/agentBroker.ts';

const model={providerId:'p',modelId:'m',label:'Model',canSelect:true,supportsAttachments:true,supportsVision:true,supportsStructuredOutput:true};
const authorization: LocalAppAgentAuthorization = {settings:{enabled:true,model,tools:[{id:'builtin:Read',kind:'builtin',name:'Read'}],skills:['extract-text'],background:false,maxTurns:20,timeoutMs:600000},requestedTools:['builtin:Read'],requestedSkills:['extract-text'],releaseRevision:1,releaseSourceDirectory:'/release',skillSpecs:[{id:'extract-text',path:'skills/extract-text/SKILL.md'}],releaseFiles:[{path:'skills/extract-text/SKILL.md',sha256:'a'.repeat(64),bytes:18,executable:false}],model};

const chatCreation = (title: string) => ({ requestId: randomUUID(), title });

// Durable Agent writes use FileHandle.writeFile, followed by fsync and a
// beforeCommit authorization check. Gate that real write, before rename.
function holdAgentWrites(matches: (path: string) => boolean, written: () => Promise<void>): () => void {
  const writable = fs as any, open = writable.open;
  writable.open = async (...args: any[]) => {
    const file = await open(...args);
    if (typeof args[0] === 'string' && matches(args[0])) {
      const writeFile = file.writeFile.bind(file);
      file.writeFile = async (...values: any[]) => {
        const result = await writeFile(...values);
        await written();
        return result;
      };
    }
    return file;
  };
  return () => { writable.open = open; };
}

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
  const chat=await broker.createChat(owner,chatCreation('Keep this chat'));
  const lockOwner=broker.run(other,{...input,runId:'blocking-task'});await waiting;
  const controller=new AbortController();
  const deleteResult=assert.rejects(broker.deleteChat(owner,chat.chatId,controller.signal),/revoked/);
  const archiveResult=assert.rejects(broker.archiveChat(owner,chat.chatId,controller.signal),/revoked/);
  const runResult=assert.rejects(broker.run(owner,{...input,runId:'revoked-task'},controller.signal),/revoked/);
  controller.abort(new Error('revoked'));release();
  await Promise.all([deleteResult,archiveResult,runResult,lockOwner]);
  assert.equal((await broker.getChat(owner,chat.chatId)).archived,false);
  assert.equal((await broker.list(owner)).tasks.some(task=>task.runId==='revoked-task'),false);
  assert.ok((await fs.stat(join(root,'apps',owner.appId,'agent','agui-v1','chats',chat.chatId,'chat.json'))).isFile());
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
  const pending=assert.rejects(broker.createChat(owner,chatCreation('Revoked'),controller.signal),/revoked/);
  await waiting;controller.abort(new Error('revoked'));release();await pending;
  assert.deepEqual(await fs.readdir(join(root,'apps',owner.appId,'agent','agui-v1','chats')),[]);
  const restarted=new LocalAppAgentBroker(options);
  try {await restarted.initialize();assert.deepEqual((await restarted.listChats(owner)).chats,[]);}
  finally {await restarted.dispose();}
});

test('revocation before an archive rename preserves the original chat', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'remote-agent-archive-revoke-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>({result:{text:'done',attachments:[],messageId:'m'},usage:{turns:1}})});
  t.after(async()=>{await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const chat=await broker.createChat(owner,chatCreation('Keep this chat'));
  const target=join(root,'apps',owner.appId,'agent','agui-v1','chats',chat.chatId,'chat.json');
  const before=await fs.readFile(target,'utf8');
  let release!:()=>void, entered!:()=>void;
  const blocked=new Promise<void>(resolve=>{release=resolve;});
  const waiting=new Promise<void>(resolve=>{entered=resolve;});
  const restore=holdAgentWrites(path=>path.startsWith(`${target}.`) && path.endsWith('.tmp'),async()=>{entered();await blocked;});
  const controller=new AbortController();
  try {
    const pending=assert.rejects(broker.archiveChat(owner,chat.chatId,controller.signal),/revoked/);
    await waiting;controller.abort(new Error('revoked'));release();await pending;
    assert.equal((await broker.getChat(owner,chat.chatId)).archived,false);
    assert.equal(await fs.readFile(target,'utf8'),before);
    assert.deepEqual((await fs.readdir(join(root,'apps',owner.appId,'agent','agui-v1','chats',chat.chatId))).sort(),['assets','chat.json']);
  } finally { release();restore(); }
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
  const chat=await broker.createChat(owner,chatCreation('Chat'));
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

test('app withdrawal fences queued chat mutations and permits calls after regrant', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-app-mutation-fence-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>{throw new Error('unexpected');}});
  let release!:()=>void, entered!:()=>void;
  const gate=new Promise<void>(resolve=>{release=resolve;}), locked=new Promise<void>(resolve=>{entered=resolve;});
  t.after(async()=>{release();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const first=await broker.createChat(owner,chatCreation('Keep')), second=await broker.createChat(owner,chatCreation('Keep too'));
  const blocker=(broker as any).exclusive(async()=>{entered();await gate;});await locked;
  const session=new AbortController();
  const archive=broker.archiveChat(owner,first.chatId,session.signal), remove=broker.deleteChat(owner,second.chatId,session.signal);
  const archiveRejected=assert.rejects(archive), removeRejected=assert.rejects(remove);
  const cancellation=broker.cancelApp(owner.appId);release();await Promise.all([cancellation,blocker,archiveRejected,removeRejected]);
  assert.equal(session.signal.aborted,false);
  assert.equal((await broker.getChat(owner,first.chatId)).archived,false);
  assert.equal((await broker.getChat(owner,second.chatId)).archived,false);
  await broker.archiveChat(owner,first.chatId,session.signal);
  assert.equal((await broker.getChat(owner,first.chatId)).archived,true);
});

test('app withdrawal before task commit prevents durable admission', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-app-admission-fence-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>{throw new Error('unexpected');}});
  let release!:()=>void, entered!:()=>void;
  const gate=new Promise<void>(resolve=>{release=resolve;}), waiting=new Promise<void>(resolve=>{entered=resolve;});
  t.after(async()=>{release();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  await broker.initialize();
  const restore=holdAgentWrites(path=>path.includes('/agent/agui-v1/tasks/') && path.endsWith('.tmp'),async()=>{entered();await gate;});
  try {
    const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
    const pending=assert.rejects(broker.run(owner,input,new AbortController().signal));
    await waiting;const cancellation=broker.cancelApp(owner.appId);release();await Promise.all([cancellation,pending]);
    assert.deepEqual((await broker.list(owner)).tasks,[]);
    assert.deepEqual(await fs.readdir(join(root,'apps',owner.appId,'agent','agui-v1','tasks')),[]);
  } finally {release();restore();}
});

test('app withdrawal before chat rename cancels creation and archive', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-app-chat-commit-fence-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>{throw new Error('unexpected');}});
  t.after(async()=>{await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  await broker.initialize();
  let release!:()=>void, entered!:()=>void, gate:Promise<void>, waiting:Promise<void>;
  const block=()=>{gate=new Promise<void>(resolve=>{release=resolve;});waiting=new Promise<void>(resolve=>{entered=resolve;});};
  let restore=holdAgentWrites(path=>path.includes('/agent/agui-v1/chats/') && path.includes('/chat.json.') && path.endsWith('.tmp'),async()=>{entered();await gate;});
  try {
    block();const creation=assert.rejects(broker.createChat(owner,chatCreation('Revoked')));
    await waiting!;const creatingCancellation=broker.cancelApp(owner.appId);release();await Promise.all([creatingCancellation,creation]);
    assert.deepEqual((await broker.listChats(owner)).chats,[]);
    assert.deepEqual(await fs.readdir(join(root,'apps',owner.appId,'agent','agui-v1','chats')),[]);
    restore();
    const chat=await broker.createChat(owner,chatCreation('Keep'));
    block();restore=holdAgentWrites(path=>path.includes('/chat.json.') && path.endsWith('.tmp'),async()=>{entered();await gate;});
    const archive=assert.rejects(broker.archiveChat(owner,chat.chatId));
    await waiting!;const archiveCancellation=broker.cancelApp(owner.appId);release();await Promise.all([archiveCancellation,archive]);
    assert.equal((await broker.getChat(owner,chat.chatId)).archived,false);
  } finally {release();restore();}
});

test('broker shutdown aborts pending and future mutations before commit', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-shutdown-mutation-fence-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
  const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>{throw new Error('unexpected');}});
  const chat=await broker.createChat(owner,chatCreation('Keep'));
  let releaseWrite!:()=>void, writeEntered!:()=>void;
  const writeGate=new Promise<void>(resolve=>{releaseWrite=resolve;}), writing=new Promise<void>(resolve=>{writeEntered=resolve;});
  const restore=holdAgentWrites(path=>path.includes('/agent/agui-v1/chats/') && path.includes('/chat.json.') && path.endsWith('.tmp'),async()=>{writeEntered();await writeGate;});
  t.after(async()=>{releaseWrite();restore();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const create=assert.rejects(broker.createChat(owner,chatCreation('Revoked')));await writing;
  const archive=assert.rejects(broker.archiveChat(owner,chat.chatId));
  const disposing=broker.dispose();
  releaseWrite();await Promise.all([archive,create,disposing]);
  assert.equal((await broker.getChat(owner,chat.chatId)).archived,false);
  assert.equal((await broker.listChats(owner)).chats.length,1);
  await assert.rejects(broker.createChat(owner,chatCreation('After shutdown')));
  await assert.rejects(broker.deleteChat(owner,chat.chatId));
});

test('broker shutdown aborts active execution while another app holds the ledger lock', async t => {
  const root=await fs.mkdtemp(join(tmpdir(),'agent-shutdown-active-fence-'));
  const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)}, other={...owner,appId:`a${'2'.repeat(32)}`};
  let release!:()=>void, entered!:()=>void, started!:(signal:AbortSignal)=>void;
  const gate=new Promise<void>(resolve=>{release=resolve;}), locked=new Promise<void>(resolve=>{entered=resolve;}), executing=new Promise<AbortSignal>(resolve=>{started=resolve;});
  const broker=new LocalAppAgentBroker({root,authorize:async principal=>{if(principal.appId===other.appId){entered();await gate;}return authorization;},revalidate:async()=>true,execute:async execution=>{
    if(execution.owner.appId===owner.appId){started(execution.signal);await new Promise<void>((_resolve,reject)=>execution.signal.addEventListener('abort',()=>reject(execution.signal.reason),{once:true}));}
    return {result:{text:'done',attachments:[],messageId:'m'},usage:{turns:1}};
  }});
  t.after(async()=>{release();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
  const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
  const run=await broker.run(owner,{...input,runId:'active'});const signal=await executing;
  const blocker=broker.run(other,{...input,runId:'blocker'});const rejected=assert.rejects(blocker);await locked;
  const disposing=broker.dispose();const abortedBeforeUnlock=signal.aborted;
  release();await Promise.all([disposing,rejected]);
  assert.equal(abortedBeforeUnlock,true);
  assert.equal((await broker.get(owner,run.runId)).state,'interrupted');
});

for (const kind of ['admission', 'chat creation'] as const) {
  test(`broker shutdown waits for ${kind} rollback with no active task`, async t => {
    const root=await fs.mkdtemp(join(tmpdir(),'agent-shutdown-drain-'));
    const owner={appId:`a${'1'.repeat(32)}`,releaseDigest:'1'.repeat(64),invocationId:'run',remoteSessionId:'s'.repeat(43)};
    const broker=new LocalAppAgentBroker({root,authorize:async()=>authorization,revalidate:async()=>true,execute:async()=>{throw new Error('unexpected');}});
    await broker.initialize();
    let release!:()=>void, entered!:()=>void;
    const gate=new Promise<void>(resolve=>{release=resolve;}), waiting=new Promise<void>(resolve=>{entered=resolve;});
    const restore=holdAgentWrites(path=>path.endsWith('.tmp') && (kind==='admission' ? path.includes('/agent/agui-v1/tasks/') : path.includes('/agent/agui-v1/chats/') && path.includes('/chat.json.')),async()=>{entered();await gate;});
    t.after(async()=>{release();restore();await broker.dispose();await fs.rm(root,{recursive:true,force:true});});
    const input=JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json',import.meta.url),'utf8'));
    const rejected=assert.rejects(kind==='admission' ? broker.run(owner,input) : broker.createChat(owner,chatCreation('Revoked')));
    await waiting;
    let disposed=false;const disposing=broker.dispose().then(()=>{disposed=true;});
    await new Promise(resolve=>setImmediate(resolve));const returnedBeforeRollback=disposed;
    release();await Promise.all([rejected,disposing]);
    assert.equal(returnedBeforeRollback,false);
    const directory=join(root,'apps',owner.appId,'agent','agui-v1',kind==='admission'?'tasks':'chats');
    assert.deepEqual(await fs.readdir(directory),[]);
  });
}

test('remote Chat identities survive session and release replacement, remain App-owned and report uncertain claims', { timeout: 10_000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'remote-chat-contract-'));
  const principal = { appId: `a${'1'.repeat(32)}`, releaseDigest: '1'.repeat(64), invocationId: 'first-run', remoteSessionId: 's'.repeat(43) };
  const reopened = { ...principal, remoteSessionId: 't'.repeat(43) };
  const upgraded = { ...reopened, releaseDigest: '2'.repeat(64), invocationId: 'replacement-run' };
  const otherApp = { ...principal, appId: `a${'2'.repeat(32)}` };
  const admitted = { requestId: 'a4444444-4444-4444-8444-444444444444', title: '  Exact title  ' };
  const missing = { requestId: 'b5555555-5555-4555-8555-555555555555' };
  const uncertain = { requestId: 'c6666666-6666-4666-8666-666666666666' };
  let valid = true, allocations = 0;
  const options = {
    root, authorize: async () => authorization, revalidate: async () => valid,
    uuid: () => { allocations++; return randomUUID(); },
    execute: async () => { throw new Error('Chat resource reads must not execute an Agent'); },
  };
  const broker = new LocalAppAgentBroker(options);
  t.after(async () => { await broker.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  assert.equal(await broker.chatCreationStatus(principal, missing), null);
  await assert.rejects(fs.stat(join(root, 'apps', principal.appId, 'agent', 'agui-v1', 'chats', missing.requestId)), { code: 'ENOENT' });
  const [first, duplicate] = await Promise.all([broker.createChat(principal, admitted), broker.createChat(reopened, admitted)]);
  assert.equal(first.chatId, admitted.requestId);
  assert.deepEqual(duplicate, first);
  assert.equal(allocations, 1, 'a retry under another browser session must not allocate another native Chat session');
  assert.deepEqual(await broker.chatCreationStatus(upgraded, admitted), first);
  await assert.rejects(broker.createChat(reopened, { ...admitted, title: 'Exact title' }), { code: 'chat_create_conflict' });
  assert.equal(await broker.chatCreationStatus(otherApp, admitted), null);
  const isolated = await broker.createChat(otherApp, admitted);
  assert.equal(isolated.chatId, first.chatId, 'the same requestId in another App names a distinct resource');
  assert.equal(allocations, 2);
  const privateSession = async (appId: string) => JSON.parse(await fs.readFile(join(root, 'apps', appId, 'agent', 'agui-v1', 'chats', admitted.requestId, 'chat.json'), 'utf8')).sessionId;
  assert.notEqual(await privateSession(principal.appId), await privateSession(otherApp.appId));

  await fs.mkdir(join(root, 'apps', principal.appId, 'agent', 'agui-v1', 'chats', uncertain.requestId));
  await assert.rejects(broker.createChat(reopened, uncertain), { code: 'chat_storage_uncertain' });
  await assert.rejects(broker.chatCreationStatus(reopened, uncertain), { code: 'chat_storage_uncertain' });
  const page = await broker.listChats(reopened);
  assert.deepEqual(page.chats, [first]);
  assert.equal(page.hasUncertainChats, true);
  assert.deepEqual(page.unavailableChats, [{ chatId: uncertain.requestId, code: 'chat_storage_uncertain' }]);
  assert.deepEqual((await broker.listChats(otherApp)).chats, [isolated]);
  assert.equal((await broker.listChats(otherApp)).hasUncertainChats, undefined);

  valid = false;
  await assert.rejects(broker.createChat(reopened, admitted), { code: 'permission_revoked' });
  await assert.rejects(broker.chatCreationStatus(reopened, admitted), { code: 'permission_revoked' });
  valid = true;
  await broker.deleteChat(upgraded, first.chatId);
  await assert.rejects(broker.createChat(reopened, admitted), { code: 'chat_create_deleted' });
  await assert.rejects(broker.chatCreationStatus(upgraded, admitted), { code: 'chat_create_deleted' });
  assert.deepEqual(await broker.chatCreationStatus(otherApp, admitted), isolated);
});

test('remote retry can observe App-owned tasks but cannot transfer revocation ownership or cancel Desktop work', { timeout: 10_000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'remote-task-ownership-'));
  const remote = { appId: `a${'1'.repeat(32)}`, releaseDigest: '1'.repeat(64), invocationId: 'run', remoteSessionId: 's'.repeat(43) };
  const retryingSession = { ...remote, remoteSessionId: 't'.repeat(43) };
  const { remoteSessionId: _session, ...desktop } = remote;
  const replacement = { ...remote, releaseDigest: '2'.repeat(64), invocationId: 'replacement' };
  const otherApp = { ...remote, appId: `a${'2'.repeat(32)}` };
  const input = JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json', import.meta.url), 'utf8'));
  const running = new Map<string, AbortSignal>();
  const broker = new LocalAppAgentBroker({ root, authorize: async () => authorization, revalidate: async () => true,
    execute: async execution => {
      running.set(`${execution.owner.appId}:${execution.runId}`, execution.signal);
      await new Promise<void>((_resolve, reject) => execution.signal.addEventListener('abort', () => reject(execution.signal.reason), { once: true }));
      throw new Error('cancelled execution must not succeed');
    },
  });
  t.after(async () => { await broker.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  const waitFor = async (runId: string, appId = remote.appId) => {
    const until = Date.now() + 3000;
    while (!running.has(`${appId}:${runId}`)) { assert.ok(Date.now() < until, 'executor did not start'); await new Promise(resolve => setTimeout(resolve, 5)); }
    return running.get(`${appId}:${runId}`)!;
  };
  await broker.run(remote, { ...input, runId: 'remote-task' });
  const remoteSignal = await waitFor('remote-task');
  await assert.rejects(broker.run(desktop, { ...input, runId: 'desktop-task' }), { code: 'busy' });
  await broker.run(otherApp, { ...input, runId: 'remote-task' });
  const otherSignal = await waitFor('remote-task', otherApp.appId);
  assert.deepEqual(await broker.run(retryingSession, { ...input, runId: 'remote-task' }), { runId: 'remote-task' });
  assert.equal((await broker.get(retryingSession, 'remote-task')).state, 'running');
  await assert.rejects(broker.get(replacement, 'remote-task'), { code: 'not_found' });
  await assert.rejects(broker.events(replacement, 'remote-task'), { code: 'not_found' });
  await assert.rejects(broker.cancel(replacement, 'remote-task'), { code: 'not_found' });
  await assert.rejects(broker.get({ ...remote, appId: `a${'3'.repeat(32)}` }, 'remote-task'), { code: 'not_found' });
  await broker.cancelRemoteSession(retryingSession);
  assert.equal(remoteSignal.aborted, false, 'an identical retry must not take ownership of another session admission');
  await broker.cancelRemoteSession(remote);
  assert.equal(remoteSignal.aborted, true);
  assert.equal(otherSignal.aborted, false);
  assert.equal((await broker.get(replacement, 'remote-task')).state, 'cancelled', 'terminal history remains App-owned after a release replacement');
  assert.ok((await broker.events(replacement, 'remote-task')).events.some(event => event.type === 'RUN_ERROR' || event.type === 'RUN_FINISHED'));
  await broker.run(desktop, { ...input, runId: 'desktop-task' });
  const desktopSignal = await waitFor('desktop-task');
  await broker.cancelRemoteSession(remote);
  assert.equal(desktopSignal.aborted, false, 'remote session revocation must not cancel a Desktop admission');
  await broker.cancelApp(remote.appId);
  assert.equal(desktopSignal.aborted, true);
  assert.equal(otherSignal.aborted, false);
});

test('remote shutdown persists interruption for recovery without replaying an executor', { timeout: 10_000 }, async t => {
  const root = await fs.mkdtemp(join(tmpdir(), 'remote-interruption-recovery-'));
  const owner = { appId: `a${'1'.repeat(32)}`, releaseDigest: '1'.repeat(64), invocationId: 'run', remoteSessionId: 's'.repeat(43) };
  const input = JSON.parse(readFileSync(new URL('../vendor/local-app-agui/spec/fixtures/run-agent-input.ag-ui-1.0.3.json', import.meta.url), 'utf8'));
  let started!: () => void, executions = 0;
  const executing = new Promise<void>(resolve => { started = resolve; });
  const broker = new LocalAppAgentBroker({ root, authorize: async () => authorization, revalidate: async () => true,
    execute: async execution => {
      executions++; started();
      await new Promise<void>((_resolve, reject) => execution.signal.addEventListener('abort', () => reject(execution.signal.reason), { once: true }));
      throw new Error('interrupted');
    },
  });
  let restarted: LocalAppAgentBroker | undefined;
  t.after(async () => { await broker.dispose(); await restarted?.dispose(); await fs.rm(root, { recursive: true, force: true }); });
  await broker.run(owner, input); await executing; await broker.dispose();
  restarted = new LocalAppAgentBroker({ root, authorize: async () => authorization, revalidate: async () => true,
    execute: async () => { executions++; throw new Error('must not replay interrupted work'); },
  });
  const reopening = { ...owner, invocationId: 'reopened', remoteSessionId: 't'.repeat(43) };
  assert.equal((await restarted.get(reopening, input.runId)).state, 'interrupted');
  assert.equal((await restarted.list(reopening)).tasks.length, 1);
  assert.deepEqual(await restarted.run(reopening, input), { runId: input.runId });
  assert.equal(executions, 1);
  const events = (await restarted.events(reopening, input.runId)).events;
  assert.equal(events.some(event => event.type === 'TEXT_MESSAGE_CONTENT'), false);
  assert.ok(events.some(event => event.type === 'RUN_ERROR' || event.type === 'RUN_FINISHED'));
});

for (const revokedBy of ['session', 'app'] as const) {
  test(`Chat deletion revoked by ${revokedBy} before rename preserves the Chat and permits reauthorization`, { timeout: 10_000 }, async t => {
    const root = await fs.mkdtemp(join(tmpdir(), 'remote-delete-before-rename-'));
    const owner = { appId: `a${'1'.repeat(32)}`, releaseDigest: '1'.repeat(64), invocationId: 'run', remoteSessionId: 's'.repeat(43) };
    const reauthorized = { ...owner, remoteSessionId: 't'.repeat(43) };
    const creation = chatCreation('Keep after cancelled deletion');
    let allowed = true, release!: () => void, entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; }), writing = new Promise<void>(resolve => { entered = resolve; });
    const broker = new LocalAppAgentBroker({ root, authorize: async () => authorization, revalidate: async () => allowed,
      execute: async () => { throw new Error('Chat deletion must not execute an Agent'); },
    });
    let restore = () => {};
    const writable = fs as any, rename = writable.rename;
    t.after(async () => { release(); restore(); writable.rename = rename; await broker.dispose(); await fs.rm(root, { recursive: true, force: true }); });
    const original = await broker.createChat(owner, creation);
    const directory = join(root, 'apps', owner.appId, 'agent', 'agui-v1', 'chats', original.chatId), target = join(directory, 'chat.json');
    const before = await fs.readFile(target, 'utf8');
    await fs.writeFile(join(directory, 'assets', 'must-survive'), 'private Chat asset');
    let renames = 0;
    writable.rename = async (...args: any[]) => { if (args[1] === target) renames++; return rename(...args); };
    restore = holdAgentWrites(path => path.startsWith(`${target}.`) && path.endsWith('.tmp'), async () => { entered(); await gate; });
    const session = new AbortController(), reason = new Error('remote session revoked before deletion');
    const deletion = broker.deleteChat(owner, original.chatId, session.signal).then(() => ({ error: undefined }), error => ({ error }));
    await writing; allowed = false;
    const cancellation = revokedBy === 'session'
      ? (session.abort(reason), broker.cancelRemoteSession(owner))
      : broker.cancelApp(owner.appId, 'Application permission revoked before deletion');
    release();
    const [outcome] = await Promise.all([deletion, cancellation]);
    restore(); writable.rename = rename;
    assert.equal(renames, 0, 'the cancellation guard must reject before dispatching the tombstone rename');
    assert.equal(await fs.readFile(target, 'utf8'), before);
    assert.equal(JSON.parse(await fs.readFile(target, 'utf8')).deleted, undefined);
    assert.equal(await fs.readFile(join(directory, 'assets', 'must-survive'), 'utf8'), 'private Chat asset');
    assert.deepEqual((await fs.readdir(directory)).sort(), ['assets', 'chat.json']);
    allowed = true;
    // These are the same reads a reauthorized browser performs. A known
    // before-rename cancellation must not poison this App-owned Chat as uncertain.
    assert.deepEqual(await broker.getChat(reauthorized, original.chatId), original);
    assert.deepEqual(await broker.chatCreationStatus(reauthorized, creation), original);
    assert.deepEqual(await broker.listChats(reauthorized), { chats: [original] });
    assert.deepEqual((await broker.chatHistory(reauthorized, original.chatId)).messages, []);
    if (revokedBy === 'session') assert.equal(outcome.error, reason);
    else assert.equal(outcome.error?.code, 'permission_revoked');
    await broker.deleteChat(reauthorized, original.chatId, new AbortController().signal);
    await assert.rejects(broker.chatCreationStatus(reauthorized, creation), { code: 'chat_create_deleted' });
  });
}

for (const failureAt of ['rename', 'directory-sync'] as const) {
  test(`Chat deletion remains uncertain when dispatched ${failureAt} fails despite revocation`, { timeout: 10_000 }, async t => {
    const root = await fs.mkdtemp(join(tmpdir(), 'remote-delete-dispatched-'));
    const owner = { appId: `a${'1'.repeat(32)}`, releaseDigest: '1'.repeat(64), invocationId: 'run', remoteSessionId: 's'.repeat(43) };
    const reauthorized = { ...owner, remoteSessionId: 't'.repeat(43) };
    const creation = chatCreation('Deletion outcome must remain uncertain');
    let allowed = true, release!: () => void, entered!: () => void;
    const gate = new Promise<void>(resolve => { release = resolve; }), dispatched = new Promise<void>(resolve => { entered = resolve; });
    const broker = new LocalAppAgentBroker({ root, authorize: async () => authorization, revalidate: async () => allowed,
      execute: async () => { throw new Error('Chat deletion must not execute an Agent'); },
    });
    const writable = fs as any, rename = writable.rename, open = writable.open;
    t.after(async () => { release(); writable.rename = rename; writable.open = open; await broker.dispose(); await fs.rm(root, { recursive: true, force: true }); });
    const original = await broker.createChat(owner, creation);
    const directory = join(root, 'apps', owner.appId, 'agent', 'agui-v1', 'chats', original.chatId), target = join(directory, 'chat.json');
    const before = await fs.readFile(target, 'utf8');
    await fs.writeFile(join(directory, 'assets', 'preserve-until-confirmed'), 'private Chat asset');
    if (failureAt === 'rename') {
      writable.rename = async (...args: any[]) => {
        if (args[1] !== target) return rename(...args);
        // The durable writer has crossed its dispatch boundary. A rejection
        // from this operation is not evidence that the prior guard rejected it.
        entered(); await gate; throw new Error('injected rename completion unavailable');
      };
    } else {
      writable.open = async (...args: any[]) => {
        const file = await open(...args);
        if (args[0] === directory && args[1] === 'r') {
          const sync = file.sync.bind(file);
          file.sync = async () => { await sync(); entered(); await gate; throw new Error('injected directory fsync acknowledgement unavailable'); };
        }
        return file;
      };
    }
    const session = new AbortController();
    const deletion = broker.deleteChat(owner, original.chatId, session.signal).then(() => ({ error: undefined }), error => ({ error }));
    await dispatched; allowed = false;
    const cancellation = failureAt === 'rename'
      ? (session.abort(new Error('session revoked after rename dispatch')), broker.cancelRemoteSession(owner))
      : broker.cancelApp(owner.appId, 'permission revoked after tombstone rename');
    release();
    const [outcome] = await Promise.all([deletion, cancellation]);
    writable.rename = rename; writable.open = open; allowed = true;
    assert.equal(outcome.error?.code, 'chat_storage_uncertain');
    const onDisk = await fs.readFile(target, 'utf8');
    if (failureAt === 'rename') assert.equal(onDisk, before);
    else assert.equal(JSON.parse(onDisk).deleted, true, 'the real rename committed before its directory sync acknowledgement failed');
    assert.equal(await fs.readFile(join(directory, 'assets', 'preserve-until-confirmed'), 'utf8'), 'private Chat asset');
    await assert.rejects(broker.getChat(reauthorized, original.chatId), { code: 'chat_storage_uncertain' });
    await assert.rejects(broker.chatCreationStatus(reauthorized, creation), { code: 'chat_storage_uncertain' });
    await assert.rejects(broker.createChat(reauthorized, creation), { code: 'chat_storage_uncertain' });
    assert.deepEqual(await broker.listChats(reauthorized), { chats: [], hasUncertainChats: true, unavailableChats: [{ chatId: original.chatId, code: 'chat_storage_uncertain' }] });
  });
}
