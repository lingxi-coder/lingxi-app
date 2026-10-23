import { test } from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { promises as fs } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { LocalAppAgentExecutor, type LocalAppAgentExecutorOptions } from '../src/main/local-apps/agentExecutor.ts';
import type { LocalAppAgentExecution } from '../src/main/local-apps/agentBroker.ts';

// Real child pipes exercise framing, cancellation and reaping; no provider access.
const CHILD = String.raw`
const readline = require('node:readline'), fs = require('node:fs');
const mode = process.argv[1]; let runId, sequence=0;
const out = x => process.stdout.write(JSON.stringify({protocolVersion:1,...(runId ? {runId,sequence:++sequence}:{}),...x})+'\n');
const finish = () => out({type:'finished',status:'succeeded',text:'done',result:{color:'red'},usage:{inputTokens:7,outputTokens:3,totalUsd:0.02,apiCalls:2,private:'secret'}});
readline.createInterface({input:process.stdin}).on('line', raw => {
 const message=JSON.parse(raw);
 if(message.type==='catalog') {
  const marker=message.configHome+'/.config.json';
  if(mode==='initializes_config' && !fs.existsSync(marker)) fs.writeFileSync(marker,'{"initialized":true}');
  if(mode==='changes_config') fs.writeFileSync(marker,JSON.stringify({updated:Date.now(),pid:process.pid}));
  out({type:'catalog',models:[{provider_id:'test',request_model:'vision',display_model:'Test vision',capabilities:{vision:true,tools:true}}],
   builtins:[{name:'Read'}],mcpTools:[{serverConfigId:'server',configRevision:mode==='changed'?'changed':'one',rawToolName:'search',definitionFingerprint:'definition',configIndex:0,label:'Nutrition'}],
   mcpConfigs:[{name:'nutrition',headers:{Authorization:'PRIVATE_MCP_HEADER'}}]});
 } else if(message.type==='start') {
  runId=message.runId; fs.writeFileSync('start.json',JSON.stringify(message));
  if(mode==='wrong_owner') return out({type:'started',runId:'other-app-run'});
  out({type:'started'});
  if(mode==='error') { process.stderr.write('PRIVATE_SECRET_IN_STDERR'); return out({type:'finished',status:'failed',error:{code:'model_execution_failed',message:'PRIVATE_SECRET_IN_ERROR'}}); }
  if(mode==='missing_result') return out({type:'finished',status:'succeeded',text:'{"color":"red"}'});
  if(mode==='wait') { process.on('SIGTERM',()=>setTimeout(()=>process.exit(0),120)); out({type:'permission',requestId:9,toolName:'Read',input:{path:'test-file'}}); return; }
  if(mode==='question') return out({type:'question',requestId:8,questions:[{question:'Which color?',header:'Color',multi_select:false,options:[{label:'Red',description:'Test'}]}]});
  if(mode==='refresh') return out({type:'credential_required',provider:'codex'});
  out({type:'text',delta:'some final text'}); out({type:'tool_started',toolCallId:'c1',tool:'Read'}); out({type:'tool_finished',toolCallId:'c1',tool:'Read'}); out({type:'usage',usage:{inputTokens:7}}); finish();
 } else if(message.type==='question_cancel' || message.type==='credential_update') { fs.writeFileSync('control.json',JSON.stringify(message)); finish(); }
});
`;
async function fixture(t: { after(fn: () => Promise<void>): void }, mode='success', overrides: Partial<LocalAppAgentExecutorOptions>={}) {
 const root=await fs.mkdtemp(join(tmpdir(),'lingxi-agent-executor-'));
 const children: ReturnType<typeof spawn>[]=[];
 const options: LocalAppAgentExecutorOptions={configHome:root,root,serverBin:'test-bridge',
  credentials:async()=>({providerCredentials:{test:'TEST_KEY'}}),credentialStatus:async()=>new Set(['test']),permission:async()=>false,question:async()=>null,
  spawnProcess:((_bin:unknown,_args:unknown,opts:unknown)=>{
   const child=spawn(process.execPath,['-e',CHILD,mode],opts as Parameters<typeof spawn>[2]);children.push(child);return child;
  }) as typeof spawn,...overrides};
 const executor=new LocalAppAgentExecutor(options);
 t.after(async()=>{for(const child of children) if(child.exitCode===null && child.signalCode===null) child.kill('SIGKILL');await fs.rm(root,{recursive:true,force:true});});
 const phases:string[]=[], controller=new AbortController();
 const execution:LocalAppAgentExecution={runId:'run-one',owner:{appId:'app-one',revision:1,invocationId:'inv-one'},input:{requestId:'request-one',prompt:'Describe the test image.',outputSchema:{type:'object'}},
  authorization:{settings:{enabled:true,model:{providerId:'test',modelId:'vision'},tools:[],background:false,maxTurns:4,timeoutMs:10000},requestedTools:[],model:{providerId:'test',modelId:'vision',label:'Test',canSelect:true,supportsImages:true,supportsStructuredOutput:true}},
  signal:controller.signal,assertAuthorized:async()=>controller.signal.throwIfAborted(),emit:async event=>{phases.push(event.phase);}};
 return {root,executor,execution,controller,phases,children};
}

test('catalog strips private config; execution preserves model, identity, limits and validated output',async t=>{
 const {root,executor,execution,phases}=await fixture(t);
 const catalog=await executor.catalog();assert.ok(!JSON.stringify(catalog).includes('PRIVATE_MCP_HEADER'));assert.equal(catalog.models[0].canSelect,true);
 assert.deepEqual(await executor.execute(execution),{result:{text:'done',output:{color:'red'}},usage:{inputTokens:7,outputTokens:3,costUsd:0.02,turns:2}});
 assert.ok(phases.includes('thinking') && phases.includes('tool'));
 const sent=JSON.parse(await fs.readFile(join(root,'runs/run-one/start.json'),'utf8'));
 assert.equal(sent.model,'test/vision');assert.equal(sent.appId,'app-one');assert.equal(sent.versionId,'1');assert.equal(sent.invocationId,'inv-one');assert.equal(sent.maxTurns,4);assert.equal(sent.timeoutMs,10000);
 assert.deepEqual(sent.toolPolicy,{builtinTools:[],mcpTools:[]});assert.deepEqual(sent.mcpConfigs,[]);assert.deepEqual(sent.credentials.provider_keys,{test:'TEST_KEY'});
});
test('changed MCP configuration fails before starting a task',async t=>{
 const {executor,execution,children}=await fixture(t,'changed');const catalog=await executor.catalog();const tool=catalog.tools.find(tool=>tool.kind==='mcp')!;
 execution.authorization.settings.tools=[{...tool,configRevision:'one'} as typeof tool];
 await assert.rejects(executor.execute(execution),{code:'tool_changed'});assert.equal(children.length,2,'only catalogs launched');
});
test('cancellation aborts permission UI and waits for the child to close',async t=>{
 let shown!:()=>void;const prompt=new Promise<void>(resolve=>{shown=resolve;});let uiAborted=false;
 const {executor,execution,controller,children}=await fixture(t,'wait',{permission:async(_input,signal)=>{shown();return new Promise(resolve=>signal.addEventListener('abort',()=>{uiAborted=true;resolve(false);},{once:true}));}});
 const task=executor.execute(execution), rejection=assert.rejects(task);await prompt;controller.abort();await rejection;
 assert.equal(uiAborted,true);assert.ok(children.every(child=>child.exitCode!==null || child.signalCode!==null));
});
test('question dismissal sends typed cancellation',async t=>{
 const {root,executor,execution}=await fixture(t,'question');await executor.execute(execution);
 assert.deepEqual(JSON.parse(await fs.readFile(join(root,'runs/run-one/control.json'),'utf8')),{type:'question_cancel',requestId:8});
});
test('credential refresh keeps the selected provider and sends keys only over stdin',async t=>{
 const requests:unknown[]=[];
 const {root,executor,execution}=await fixture(t,'refresh',{credentials:async(providerId,refresh)=>{requests.push([providerId,refresh??false]);return {providerCredentials:{test:refresh?'NEW_TEST_KEY':'OLD_TEST_KEY'}};}});
 await executor.execute(execution);assert.deepEqual(requests,[['test',false],['test',true]]);
 assert.equal(JSON.parse(await fs.readFile(join(root,'runs/run-one/control.json'),'utf8')).credentials.provider_keys.test,'NEW_TEST_KEY');
});
for(const [mode,code] of [['wrong_owner','executor_protocol'],['missing_result','invalid_result'],['error','model_execution_failed']] as const) {
 test(`rejects ${mode} without leaking diagnostics`,async t=>{
  const {executor,execution}=await fixture(t,mode);
  await assert.rejects(executor.execute(execution),(error:any)=>{assert.equal(error.code,code);assert.ok(!error.message.includes('PRIVATE_SECRET'));return true;});
 });
}

test('catalog caches credential status but observes configuration edits before its TTL expires', async t => {
 let credentialReads = 0;
 const { root, executor, children } = await fixture(t, 'success', { credentialStatus: async () => { credentialReads++; return new Set(['test']); } });
 await executor.catalog(); await executor.catalog();
 assert.equal(children.length, 1); assert.equal(credentialReads, 1);
 await fs.writeFile(join(root, 'settings.json'), '{"model":"updated"}');
 await executor.catalog();
 assert.equal(children.length, 2, 'a settings mutation invalidates the private catalog immediately');
 executor.invalidateCatalog(); await executor.catalog();
 assert.equal(credentialReads, 2);
});

test('fresh host configuration initialization is re-read before publishing the catalog', async t => {
 const { executor, children } = await fixture(t, 'initializes_config');
 const catalog = await executor.catalog();
 assert.equal(catalog.models[0].modelId, 'vision');
 assert.equal(children.length, 2);
 await executor.catalog(); assert.equal(children.length, 2, 'only the stable snapshot is cached');
});

test('continuously changing configuration is bounded and never published', async t => {
 const { executor, children } = await fixture(t, 'changes_config');
 await assert.rejects(executor.catalog(), { code: 'configuration_changed' });
 assert.equal(children.length, 2);
 assert.ok(children.every(child => child.exitCode !== null || child.signalCode !== null));
});
