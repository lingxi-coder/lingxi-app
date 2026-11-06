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
 const options = {userData:directory,isPackaged:false,resourcesPath:directory,broker,accounts:async()=>undefined,changed:(value:DecisionSnapshot)=>changes.push(value),execute:execute ?? (async (value:unknown)=>{envelope=value;return result;})};
 return { holdCredential: () => { broker.resolve = async () => new Promise<string | undefined>(() => undefined); }, host:new DecisionHost(options),reopen:()=>new DecisionHost(options),directory,secrets,changes,envelope:()=>envelope,clean:()=>rmSync(directory,{recursive:true,force:true})};
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
