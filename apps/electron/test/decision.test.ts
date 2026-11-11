import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { DecisionHost } from '../src/main/decision.js';
import { parseDecisionConfiguration, type DecisionResult, type DecisionSnapshot } from '../src/shared/decision.js';
const configuration = { source: 'custom' as const, protocol: 'typesafe_systemone' as const, endpoint: 'http://127.0.0.1:8000/v1/systemone', model: 'laya' };
const input = { state: 'test', questions: { answer: { type: 'binary' as const, instructions: 'Is it valid?' } } };
const result: DecisionResult = { state_revision: 0, answers: { answer: { type: 'binary', probability: 0.8 } }, usage: { input_tokens: null, output_tokens: null }, actual_model: 'test', elapsed_ms: 1 };
function fixture(execute?: (value: unknown, signal: AbortSignal) => Promise<DecisionResult>) {
 const directory = mkdtempSync(join(tmpdir(), 'lingxi-decision-')); const secrets = new Map<string,string>(); const changes: DecisionSnapshot[] = []; let envelope: unknown;
 const broker = { delete: async (id: string) => { secrets.delete(id); }, preview: async (id: string) => ({ providerId:id,configured:secrets.has(id) }), set: async (id: string,key:string) => { assert.match(id, /^[a-z0-9][a-z0-9._-]{0,63}$/); secrets.set(id,key); return { providerId:id,configured:true }; }, resolve:async(id:string)=>secrets.get(id) };
 const options: ConstructorParameters<typeof DecisionHost>[0] = {userData:directory,isPackaged:false,resourcesPath:directory,broker,accounts:async()=>undefined,changed:(value:DecisionSnapshot)=>changes.push(value),execute:execute ?? (async (value:unknown)=>{envelope=value;return result;})};
 return { broker, set accounts(value: typeof options.accounts) { options.accounts = value; }, holdCredential: () => { broker.resolve = async () => new Promise<string | undefined>(() => undefined); }, host:new DecisionHost(options),reopen:()=>new DecisionHost(options),directory,secrets,changes,envelope:()=>envelope,clean:()=>rmSync(directory,{recursive:true,force:true})};
}
test('global settings survive restart and reject stale saves; secrets stay private',async()=>{
 const f=fixture();try {
  const initial=await f.host.snapshot();assert.equal(initial.configuration,null);
  const saved=await f.host.command(1,{action:'save',revision:initial.revision,configuration,credential:'test-private-key'}) as DecisionSnapshot;
  assert.equal(saved.credentialConfigured,true);assert.equal(f.changes.length,1);
  assert.deepEqual(await f.reopen().snapshot(),saved);
  assert.ok(!readFileSync(join(f.directory,'decision.json'),'utf8').includes('test-private-key'));
  assert.ok(!JSON.stringify(saved).includes('test-private-key'));
  await assert.rejects(f.host.command(1,{action:'save',revision:initial.revision,configuration:null}),/changed/);
  await f.host.command(1,{action:'evaluate',operationId:'test-operation',input});
  assert.equal((f.envelope() as {credential:string}).credential,'test-private-key');
 }finally{f.clean();}
});
test('changing endpoint never forwards an old credential to the new server',async()=>{
 const f=fixture();try {await f.host.command(1,{action:'save',revision:'initial',configuration,credential:'secret'});
 const state=await f.host.snapshot();await f.host.command(1,{action:'save',revision:state.revision,configuration:{...configuration,endpoint:'http://localhost:9000/v1/systemone'}});
 await f.host.command(1,{action:'evaluate',operationId:'operation',input});assert.equal((f.envelope() as {credential:unknown}).credential,null);
 }finally{f.clean();}
});
test('an interrupted settings save removes its new secret and preserves the saved binding', async () => {
 const f=fixture(); try {
  const saved=await f.host.command(1,{action:'save',revision:'initial',configuration,credential:'original'}) as DecisionSnapshot;
  await assert.rejects(f.host.command(1,{action:'save',revision:saved.revision,configuration,credential:'replacement'},()=>{throw new Error('Window navigated');}),/navigated/);
  assert.equal(f.secrets.size,1); assert.deepEqual([...f.secrets.values()],['original']);
  assert.deepEqual(await f.host.snapshot(),saved);
 } finally { f.clean(); }
});
test('renderer cancellation discards late results',async()=>{
 let release!:(value:DecisionResult)=>void;const f=fixture(async()=>new Promise(resolve=>{release=resolve;}));try {
  await f.host.command(1,{action:'save',revision:'initial',configuration});const pending=f.host.command(1,{action:'evaluate',operationId:'operation',input});
  await f.host.command(2,{action:'cancel',operationId:'operation'});await f.host.command(1,{action:'cancel',operationId:'operation'});release(result);await assert.rejects(pending,/cancelled/);
 }finally{f.clean();}
});
test('corrupt settings are preserved and unsupported protocols and credential URLs are rejected',async()=>{
 const f=fixture();try {writeFileSync(join(f.directory,'decision.json'),'{broken');await assert.rejects(f.host.snapshot());await assert.rejects(f.host.command(1,{action:'save',revision:'initial',configuration}));assert.equal(readFileSync(join(f.directory,'decision.json'),'utf8'),'{broken');
  for(const endpoint of ['file:///tmp/test','http://user:secret@localhost/v1/systemone','https://localhost/x?key=secret'])assert.throws(()=>parseDecisionConfiguration({...configuration,endpoint}));
  assert.throws(()=>parseDecisionConfiguration({...configuration,protocol:'openai_chat'}));
 }finally{f.clean();}
});

test('cancellation also terminates a stalled credential lookup before any dispatch', async () => {
 const f=fixture(); try {
  await f.host.command(1,{action:'save',revision:'initial',configuration,credential:'secret'});
  f.holdCredential(); const pending=f.host.command(1,{action:'evaluate',operationId:'lookup',input});
  await f.host.command(1,{action:'cancel',operationId:'lookup'});
  await assert.rejects(pending,/cancelled/); assert.equal(f.envelope(),undefined);
 } finally { f.clean(); }
});

test('cancelled credential preparations retain admission until the underlying work settles', async () => {
 const f=fixture(); const release: Array<() => void> = []; let lookups=0;
 try {
  await f.host.command(1,{action:'save',revision:'initial',configuration,credential:'secret'});
  const original=f.broker.resolve;
  f.broker.resolve=async()=>{lookups++;return new Promise<string>(resolve=>release.push(()=>resolve('secret')));};
  for(let index=0;index<8;index++) {
   const operationId=`lookup-${index}`;
   const pending=f.host.command(1,{action:'evaluate',operationId,input});
   await f.host.command(1,{action:'cancel',operationId});
   await assert.rejects(pending,/cancelled/);
  }
  await assert.rejects(f.host.command(1,{action:'evaluate',operationId:'overflow',input}),/busy/);
  assert.equal(lookups,8); assert.equal(f.envelope(),undefined);
  for(const finish of release.splice(0)) finish();
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(f.envelope(),undefined,'late credential completion must not dispatch cancelled work');
  f.broker.resolve=original;
  await f.host.command(1,{action:'evaluate',operationId:'recovered',input});
  assert.ok(f.envelope(),'settled preparations release their capacity');
 } finally { for(const finish of release) finish(); f.clean(); }
});

test('saved-account evaluation requests only the configured API-key account', async () => {
 const f=fixture(); let selected:string|undefined;
 try {
  f.accounts=async accountId=>{
   selected=accountId;
   return {providerAccounts:{storeScope:'test',configRevision:1,accounts:[{accountId,providerId:'openai',executionBindingId:'openai',authKind:'api_key',enabled:true,identityGeneration:2,configRevision:1,displayName:'Selected',userPriority:0,credentialRef:{storeScope:'test',secretRef:`acct.${accountId}`}}],health:[]},providerAccountCredentials:{[accountId]:{identity_generation:2,credential_revision:3,kind:{kind:'api_key',key:'selected-key'}}}};
  };
  await f.host.command(1,{action:'save',revision:'initial',configuration:{source:'openai',protocol:'openai_decisions',endpoint:'https://api.openai.com/v1/decisions',model:'test',accountId:'selected'}});
  await f.host.command(1,{action:'evaluate',operationId:'selected',input});
  assert.equal(selected,'selected');
  assert.equal((f.envelope() as {credential:string}).credential,'selected-key');
 } finally { f.clean(); }
});

test('tool cancellation reaches the global service and discards a late backend result', async () => {
 let release!:(value:DecisionResult)=>void; let observed:AbortSignal|undefined;
 const f=fixture(async(_value,signal)=>{observed=signal;return new Promise(resolve=>{release=resolve;});});
 try {
  assert.equal(await f.host.isConfigured(),false);
  await f.host.command(1,{action:'save',revision:'initial',configuration});
  assert.equal(await f.host.isConfigured(),true);
  const controller=new AbortController();const pending=f.host.evaluateTool(input,controller.signal,30_000);
  controller.abort();await assert.rejects(pending,/cancelled/);assert.equal(observed?.aborted,true);
  release(result);
 } finally {f.clean();}
});


test('disabling revokes discovery immediately even while credential cleanup is pending', async () => {
 const f=fixture(); let release!:()=>void;
 try {
  const saved=await f.host.command(1,{action:'save',revision:'initial',configuration,credential:'secret'}) as DecisionSnapshot;
  const cleanup=new Promise<void>(resolve=>{release=resolve;});const remove=f.broker.delete;
  f.broker.delete=async id=>{await cleanup;await remove(id);};
  let notified=false;f.host.onConfigurationChanged(()=>{notified=true;});
  const pending=f.host.command(1,{action:'save',revision:saved.revision,configuration:null});
  assert.equal(notified,true);assert.equal(await f.host.isConfigured(),false);
  release();await pending;
 } finally { release?.();f.clean(); }
});
