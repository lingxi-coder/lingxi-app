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
const mode = process.argv[1]; let runId, sequence=0, start;
const out = (x,done) => process.stdout.write(JSON.stringify({protocolVersion:1,...(runId ? {runId,sequence:++sequence}:{}),...(x.type==='finished'?{cleanupStatus:start?.appChat || start?.input.tools.length || start?.input.skills.length ? 'complete':'not_required'}:{}),...x})+'\n',done);
const finish = () => out({type:'finished',status:'succeeded',text:'done',result:{color:'red'},messageId:'native-message-id',...(start.appChat?{chatId:start.appChat.chatId,chatCheckpoint:{generationId:start.appChat.generationId,sessionId:start.appChat.sessionId,byteLen:100,sha256:'a'.repeat(64)}}:{}),usage:{inputTokens:7,outputTokens:3,costUsd:0.02,turns:2,costEstimated:true,private:'secret'}});
const lines = readline.createInterface({input:process.stdin});
lines.on('close', () => {
 if(!start) return;
 if(mode==='nonzero_after_success') process.exit(7);
 if(mode==='hold_after_success') fs.writeFileSync('terminal_closed','terminal consumed; stdin closed');
});
lines.on('line', raw => {
 const message=JSON.parse(raw);
 if(message.type==='catalog') {
  const marker=message.configHome+'/.config.json';
  if(mode==='initializes_config' && !fs.existsSync(marker)) fs.writeFileSync(marker,'{"initialized":true}');
  if(mode==='changes_config') fs.writeFileSync(marker,JSON.stringify({updated:Date.now(),pid:process.pid}));
  out({type:'catalog',models:[{provider_id:'test',request_model:'vision',display_model:'Test vision',capabilities:{vision:true,tools:true}}],
   builtins:[{name:'Read'}],mcpTools:[{serverConfigId:'server',configRevision:mode==='changed'?'changed':'one',rawToolName:'search',definitionFingerprint:'definition',configIndex:0,label:'Nutrition'}],
   mcpConfigs:[{name:'nutrition',headers:{Authorization:'PRIVATE_MCP_HEADER'}}]});
 } else if(message.type==='start') {
  runId=message.runId; start=message; fs.writeFileSync('start.json',JSON.stringify(message));
  if(mode==='wrong_owner') return out({type:'started',runId:'other-app-run'});
  out({type:'started'});
  if(mode==='error') { process.stderr.write('PRIVATE_SECRET_IN_STDERR'); return out({type:'finished',status:'failed',error:{code:'model_execution_failed',message:'PRIVATE_SECRET_IN_ERROR'}}); }
  if(mode.startsWith('context_error:')) return out({type:'finished',status:'failed',error:{code:mode.slice('context_error:'.length),message:'PRIVATE_SECRET_IN_ERROR /Users/private/request.json'}});
  if(mode==='cleanup_error') return out({type:'finished',status:'failed',cleanupStatus:'unconfirmed',error:{code:'tool_cleanup_incomplete',message:'PRIVATE_SECRET_IN_ERROR'}});
  if(mode==='no_terminal') return process.exit(0);
  if(mode==='missing_result') return out({type:'finished',status:'succeeded',text:'{"color":"red"}'});
  if(mode==='missing_chat_checkpoint') return out({type:'finished',status:'succeeded',text:'done',messageId:'native-message-id',chatId:start.appChat.chatId});
  if(mode==='wait' || mode==='wait_ignore_cancel' || mode==='wait_unconfirmed' || mode==='wait_wrong_cleanup_owner') { process.on('SIGTERM',()=>{fs.writeFileSync('sigterm','signalled');setTimeout(()=>process.exit(0),120);}); out({type:'permission',requestId:9,toolName:'Read',input:{path:'test-file'}}); return; }
  if(mode==='hold_after_success') { setInterval(()=>{if(fs.existsSync('exit_release')) process.exit(0);},10); finish(); return; }
  if(mode==='question') return out({type:'question',requestId:8,questions:[{question:'Which color?',header:'Color',multi_select:false,options:[{label:'Red',description:'Test'}]}]});
  if(mode==='refresh') return out({type:'credential_required',provider:'codex'});
  out({type:'text',delta:'some final text'}); out({type:'tool_started',toolCallId:'c1',tool:'Read'}); out({type:'tool_finished',toolCallId:'c1',tool:'Read'}); out({type:'usage',usage:{inputTokens:7}}); finish();
  if(mode==='trailing_frame') out({type:'text',delta:'late output'});
 } else if(message.type==='cancel') {
  fs.writeFileSync('cancel_received','cancel received');
  if(mode==='wait' || mode==='wait_unconfirmed' || mode==='wait_wrong_cleanup_owner') setTimeout(()=>out({type:'finished',status:'cancelled',...(mode==='wait_unconfirmed'?{cleanupStatus:'unconfirmed'}:{}),...(mode==='wait_wrong_cleanup_owner'?{runId:'another-task'}:{})},()=>process.exit(0)),120);
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
const execution:LocalAppAgentExecution={runId:'run-one',taskStorageId:'11111111-1111-4111-8111-111111111111',owner:{appId:'a11111111111111111111111111111111',releaseDigest:'1'.repeat(64),invocationId:'inv-one'},input:{requestId:'run-one',protocolThreadId:'thread-one',protocolInputDigest:'a'.repeat(64),protocolMessageSha256:'b'.repeat(64),prompt:'Describe the test image.',contentParts:[{kind:'text',text:'Describe the test image.'},{kind:'attachment',name:'photo.png',mediaType:'image/png',base64:'iVBORw=='}],outputSchema:{type:'object'},tools:[],skills:[]},
  authorization:{settings:{enabled:true,model:{providerId:'test',modelId:'vision'},tools:[],skills:[],background:false,maxTurns:4,timeoutMs:10000},requestedTools:[],requestedSkills:[],releaseRevision:1,
    releaseSourceDirectory:'/release/source',skillSpecs:[],releaseFiles:[],model:{providerId:'test',modelId:'vision',label:'Test',canSelect:true,supportsAttachments:true,supportsVision:true,supportsStructuredOutput:true}},
  // Transport fixtures have no broker; the broker suite verifies the durable marker.
  signal:controller.signal,assertAuthorized:async()=>controller.signal.throwIfAborted(),requireCleanupConfirmation:async()=>{},emit:async event=>{phases.push(event.phase);}};
 return {root,executor,execution,controller,phases,children};
}

function taskCwd(root:string, execution:LocalAppAgentExecution):string {
 return execution.chat ? join(root,'apps',execution.owner.appId,'agent/agui-v1/workspaces',execution.chat.chatId) : join(root,'apps',execution.owner.appId,'agent/agui-v1/tasks',execution.taskStorageId);
}
function requireRuntimeTool(execution:LocalAppAgentExecution):void {
 execution.input.tools=['builtin:Read'];
 execution.authorization.settings.tools=[{id:'builtin:Read',kind:'builtin',name:'Read'}];
}
async function waitForFile(path:string):Promise<void> {
 const deadline=Date.now()+5000;
 while(true) {
  try { await fs.stat(path); return; }
  catch(error) { if((error as NodeJS.ErrnoException).code!=='ENOENT') throw error; }
  if(Date.now()>=deadline) throw new Error(`child did not reach ${path}`);
  await new Promise(resolve=>setTimeout(resolve,10));
 }
}

test('catalog strips private config; execution preserves model, identity, limits and validated output',async t=>{
 const {root,executor,execution,phases}=await fixture(t);
 const catalog=await executor.catalog();assert.ok(!JSON.stringify(catalog).includes('PRIVATE_MCP_HEADER'));assert.equal(catalog.models[0].canSelect,true);
 assert.deepEqual(await executor.execute(execution),{result:{text:'done',attachments:[],messageId:'native-message-id',output:{color:'red'}},usage:{inputTokens:7,outputTokens:3,costUsd:0.02,costEstimated:true,turns:2}});
 assert.ok(phases.includes('thinking') && phases.includes('tool'));
 const sent=JSON.parse(await fs.readFile(join(root,'apps/a11111111111111111111111111111111/agent/agui-v1/tasks/11111111-1111-4111-8111-111111111111/start.json'),'utf8'));
 assert.equal(sent.model,'test/vision');assert.equal(sent.appId,'a11111111111111111111111111111111');assert.equal(sent.versionId,'1'.repeat(64));assert.equal(sent.invocationId,'inv-one');assert.equal(sent.maxTurns,4);assert.equal(sent.timeoutMs,10000);
 assert.deepEqual(sent.toolPolicy,{builtinTools:[],mcpTools:[]});assert.deepEqual(sent.mcpConfigs,[]);assert.deepEqual(sent.credentials.provider_keys,{test:'TEST_KEY'});
 assert.deepEqual(sent.modelCapabilities,{vision:true,structuredOutput:true});
 assert.equal(sent.input.protocolThreadId,'thread-one');assert.equal(sent.input.protocolInputDigest,'a'.repeat(64));assert.equal(sent.input.protocolMessageSha256,'b'.repeat(64));
});
test('changed MCP configuration fails before starting a task',async t=>{
 const {executor,execution,children}=await fixture(t,'changed');const catalog=await executor.catalog();const tool=catalog.tools.find(tool=>tool.kind==='mcp')!;
 execution.authorization.settings.tools=[{...tool,configRevision:'one'} as typeof tool];
 await assert.rejects(executor.execute(execution),{code:'tool_changed'});assert.equal(children.length,2,'only catalogs launched');
});
test('ordered AG-UI content parts use the private Rust DTO without mutating host metadata', async t => {
 const {root,executor,execution}=await fixture(t);
 execution.input.contentParts=[{kind:'text',text:'Read this.'},{kind:'attachment',name:'notes.txt',mediaType:'text/plain',base64:Buffer.from('file text').toString('base64')}];
 const original=structuredClone(execution.input);
 await executor.execute(execution);
 const sent=JSON.parse(await fs.readFile(join(root,'apps/a11111111111111111111111111111111/agent/agui-v1/tasks/11111111-1111-4111-8111-111111111111/start.json'),'utf8'));
 assert.deepEqual(sent.input.contentParts,[{kind:'text',text:'Read this.'},{kind:'attachment',name:'notes.txt',mediaType:'text/plain',base64:Buffer.from('file text').toString('base64')}]);
 assert.deepEqual(execution.input,original,'the host and page-facing metadata retain camelCase');
});
test('persistent chat finished frames retain the host chat ID and actual Harness message ID',async t=>{
 const {root,executor,execution}=await fixture(t);
 execution.chat={chatId:'11111111-1111-4111-8111-111111111112',sessionId:'22222222-2222-4222-8222-222222222222',sessionDirectory:join(root,'app-chat'),generationId:execution.taskStorageId,cleanupNonce:'b'.repeat(64),nativeUserMessageId:`msg:${execution.taskStorageId}`};
 const result=await executor.execute(execution);
 assert.equal(result.result.chatId,execution.chat.chatId);
 assert.equal(result.result.messageId,'native-message-id');
 assert.equal(result.chatCheckpoint?.generationId,execution.taskStorageId);
 assert.ok(!('chatCheckpoint' in result.result));
 const sent=JSON.parse(await fs.readFile(join(taskCwd(root,execution),'start.json'),'utf8'));
 assert.equal(sent.appChat.nativeUserMessageId,`msg:${execution.taskStorageId}`);
 assert.equal(sent.cwd,taskCwd(root,execution));
 assert.notEqual(sent.cwd,execution.chat.sessionDirectory,'private Chat storage is never the tool working directory');
});
test('persistent chat completion without private transcript evidence is rejected',async t=>{
 const {root,executor,execution}=await fixture(t,'missing_chat_checkpoint');
 execution.input.outputSchema=undefined;
 execution.chat={chatId:'11111111-1111-4111-8111-111111111112',sessionId:'22222222-2222-4222-8222-222222222222',sessionDirectory:join(root,'app-chat'),generationId:execution.taskStorageId,cleanupNonce:'b'.repeat(64),nativeUserMessageId:`msg:${execution.taskStorageId}`};
 await assert.rejects(executor.execute(execution),{code:'executor_protocol'});
});
test('cancellation aborts permission UI and lets native cleanup close the child before signalling',async t=>{
 let shown!:()=>void;const prompt=new Promise<void>(resolve=>{shown=resolve;});let uiAborted=false;
 const {root,executor,execution,controller,children}=await fixture(t,'wait',{permission:async(_input,signal)=>{shown();return new Promise(resolve=>signal.addEventListener('abort',()=>{uiAborted=true;resolve(false);},{once:true}));}});
 const task=executor.execute(execution), rejection=assert.rejects(task);await prompt;controller.abort();await rejection;
 assert.equal(uiAborted,true);assert.ok(children.every(child=>child.exitCode!==null || child.signalCode!==null));
 assert.equal(await fs.readFile(join(taskCwd(root,execution),'cancel_received'),'utf8'),'cancel received');
 await assert.rejects(fs.stat(join(taskCwd(root,execution),'sigterm')),{code:'ENOENT'});
});

test('a successful Chat terminal stays private until the child has exited normally',async t=>{
 const {root,executor,execution,children}=await fixture(t,'hold_after_success');
 execution.chat={chatId:'11111111-1111-4111-8111-111111111112',sessionId:'22222222-2222-4222-8222-222222222222',sessionDirectory:join(root,'app-chat'),generationId:execution.taskStorageId,cleanupNonce:'b'.repeat(64),nativeUserMessageId:`msg:${execution.taskStorageId}`};
 let settled=false;
 const task=executor.execute(execution).finally(()=>{settled=true;});
 await waitForFile(join(taskCwd(root,execution),'terminal_closed'));
 assert.equal(settled,false);
 assert.equal(children.at(-1)!.exitCode,null);
 await fs.writeFile(join(taskCwd(root,execution),'exit_release'),'release');
 const result=await task;
 assert.equal(result.chatCheckpoint?.generationId,execution.chat.generationId);
 assert.equal(children.at(-1)!.exitCode,0);
});

test('cancellation after a successful terminal cannot return its candidate checkpoint',async t=>{
 const {root,executor,execution,controller,children}=await fixture(t,'hold_after_success');
 execution.chat={chatId:'11111111-1111-4111-8111-111111111112',sessionId:'22222222-2222-4222-8222-222222222222',sessionDirectory:join(root,'app-chat'),generationId:execution.taskStorageId,cleanupNonce:'b'.repeat(64),nativeUserMessageId:`msg:${execution.taskStorageId}`};
 const task=executor.execute(execution), rejection=assert.rejects(task,{code:'cancelled'});
 await waitForFile(join(taskCwd(root,execution),'terminal_closed'));
 controller.abort();
 await fs.writeFile(join(taskCwd(root,execution),'exit_release'),'release');
 await rejection;
 assert.equal(children.at(-1)!.exitCode,0);
});

test('a child that ignores cancellation is signalled only after the bounded drain window',async t=>{
 let shown!:()=>void;const prompt=new Promise<void>(resolve=>{shown=resolve;});
 const {root,executor,execution,controller,children}=await fixture(t,'wait_ignore_cancel',{cancelDrainGraceMs:50,permission:async()=>{shown();return false;}});
 const task=executor.execute(execution), rejection=assert.rejects(task,{code:'cancelled'});
 await prompt;controller.abort();await rejection;
 assert.equal(await fs.readFile(join(taskCwd(root,execution),'cancel_received'),'utf8'),'cancel received');
 assert.equal(await fs.readFile(join(taskCwd(root,execution),'sigterm'),'utf8'),'signalled');
 assert.ok(children.every(child=>child.exitCode!==null || child.signalCode!==null));
});

test('runtime cancellation consumes a confirmed cleanup terminal even after permission UI was aborted',async t=>{
 let shown!:()=>void;const prompt=new Promise<void>(resolve=>{shown=resolve;});let required=0;
 const {root,executor,execution,controller,children}=await fixture(t,'wait',{permission:async(_input,signal)=>{shown();return new Promise(resolve=>signal.addEventListener('abort',()=>resolve(false),{once:true}));}});
 requireRuntimeTool(execution);
 execution.requireCleanupConfirmation=async()=>{required++;assert.equal(children.length,1,'cleanup ownership is persisted before the task child starts');};
 const task=executor.execute(execution), rejection=assert.rejects(task,{code:'cancelled'});
 await prompt;controller.abort();await rejection;
 assert.equal(required,1);assert.equal(children.at(-1)!.exitCode,0);
 await assert.rejects(fs.stat(join(taskCwd(root,execution),'sigterm')),{code:'ENOENT'});
});

for(const mode of ['wait_unconfirmed','wait_wrong_cleanup_owner','wait_ignore_cancel'] as const) {
 test(`runtime leader close cannot confirm cleanup for ${mode}`,async t=>{
  let shown!:()=>void;const prompt=new Promise<void>(resolve=>{shown=resolve;});
  const {executor,execution,controller,children}=await fixture(t,mode,{cancelDrainGraceMs:50,permission:async()=>{shown();return false;}});
  requireRuntimeTool(execution);
  const task=executor.execute(execution), rejection=assert.rejects(task,{code:'tool_cleanup_incomplete'});
  await prompt;controller.abort();await rejection;
  assert.ok(children.every(child=>child.exitCode!==null || child.signalCode!==null));
 });
}

test('a runtime child that exits zero without any terminal still has unconfirmed cleanup',async t=>{
 const {executor,execution,children}=await fixture(t,'no_terminal');
 requireRuntimeTool(execution);
 await assert.rejects(executor.execute(execution),{code:'tool_cleanup_incomplete'});
 assert.equal(children.at(-1)!.exitCode,0);
});

test('runtime child cannot start if its durable cleanup marker was not acknowledged',async t=>{
 const {executor,execution,children}=await fixture(t);
 requireRuntimeTool(execution);
 execution.requireCleanupConfirmation=async()=>{throw Object.assign(new Error('marker persistence failed'),{code:'test_storage_error'});};
 await assert.rejects(executor.execute(execution),{code:'test_storage_error'});
 assert.equal(children.length,1,'only the read-only catalog child ran');
});

for(const [mode,code] of [['nonzero_after_success','executor_closed'],['no_terminal','executor_closed'],['trailing_frame','executor_protocol'],['cleanup_error','tool_cleanup_incomplete']] as const) {
 test(`does not return a completion for ${mode}`,async t=>{
  const {executor,execution,children}=await fixture(t,mode);
  await assert.rejects(executor.execute(execution),(error:any)=>{assert.equal(error.code,code);assert.ok(!error.message.includes('PRIVATE_SECRET'));return true;});
  assert.ok(children.every(child=>child.exitCode!==null || child.signalCode!==null));
 });
}

test('invalid native Chat identity is rejected before catalog, credentials or a child can run',async t=>{
 const {root,executor,execution,children}=await fixture(t);
 execution.chat={chatId:'11111111-1111-4111-8111-111111111112',sessionId:'22222222-2222-4222-8222-222222222222',sessionDirectory:join(root,'app-chat'),generationId:execution.taskStorageId,cleanupNonce:'b'.repeat(64),nativeUserMessageId:`msg_${execution.taskStorageId}`};
 await assert.rejects(executor.execute(execution),{code:'executor_protocol'});
 assert.equal(children.length,0);
});
test('question dismissal sends typed cancellation',async t=>{
 const {root,executor,execution}=await fixture(t,'question');await executor.execute(execution);
 assert.deepEqual(JSON.parse(await fs.readFile(join(root,'apps/a11111111111111111111111111111111/agent/agui-v1/tasks/11111111-1111-4111-8111-111111111111/control.json'),'utf8')),{type:'question_cancel',requestId:8});
});
test('credential refresh keeps the selected provider and sends keys only over stdin',async t=>{
 const requests:unknown[]=[];
 const {root,executor,execution}=await fixture(t,'refresh',{credentials:async(providerId,refresh)=>{requests.push([providerId,refresh??false]);return {providerCredentials:{test:refresh?'NEW_TEST_KEY':'OLD_TEST_KEY'}};}});
 await executor.execute(execution);assert.deepEqual(requests,[['test',false],['test',true]]);
 assert.equal(JSON.parse(await fs.readFile(join(root,'apps/a11111111111111111111111111111111/agent/agui-v1/tasks/11111111-1111-4111-8111-111111111111/control.json'),'utf8')).credentials.provider_keys.test,'NEW_TEST_KEY');
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

test('a substituted Chat workspace parent is rejected before cleanup ownership or the native task starts', { skip: process.platform === 'win32' }, async t => {
 const {root,executor,execution,children}=await fixture(t);
 execution.chat={chatId:'11111111-1111-4111-8111-111111111112',sessionId:'22222222-2222-4222-8222-222222222222',sessionDirectory:join(root,'app-chat'),generationId:execution.taskStorageId,cleanupNonce:'b'.repeat(64),nativeUserMessageId:`msg:${execution.taskStorageId}`};
 const store=join(root,'apps',execution.owner.appId,'agent/agui-v1'), target=join(root,'business-data');
 await fs.mkdir(store,{recursive:true});await fs.mkdir(target);await fs.symlink(target,join(store,'workspaces'));
 let ownership=0;execution.requireCleanupConfirmation=async()=>{ownership++;};
 await assert.rejects(executor.execute(execution),{code:'agent_storage_unavailable'});
 assert.equal(children.length,1,'only the catalog child can run');assert.equal(ownership,0);
 assert.deepEqual(await fs.readdir(target),[],'no working directory was created through the link');
});

for(const code of ['chat_context_unavailable','chat_context_compaction_unsupported','chat_context_limit_exceeded','chat_context_budget_unknown','chat_context_authority_revoked'] as const) {
 test(`native context failure ${code} reaches the App as a fixed message`,async t=>{
  const {root,executor,execution}=await fixture(t,`context_error:${code}`);
  execution.chat={chatId:'11111111-1111-4111-8111-111111111112',sessionId:'22222222-2222-4222-8222-222222222222',sessionDirectory:join(root,'app-chat'),generationId:execution.taskStorageId,cleanupNonce:'b'.repeat(64),nativeUserMessageId:`msg:${execution.taskStorageId}`};
  await assert.rejects(executor.execute(execution),(error:any)=>{
   assert.equal(error.code,code);assert.ok(!error.message.includes('PRIVATE_SECRET'));assert.ok(!error.message.includes('/Users/private'));
   assert.notEqual(error.message,'The Agent could not finish this task. Review its configuration and retry.');return true;
  });
 });
}
