import test from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { PassThrough } from 'node:stream';
import { mkdirSync, writeFileSync, readFileSync, symlinkSync, linkSync, chmodSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import type { ChildProcessWithoutNullStreams } from 'node:child_process';
import { TeamsApiError } from '../../../packages/teams';
import { TeamsService, type TeamAccountContext } from '../src/main/teams/service';
import { TeamConnector, DELIVERY_SKEW_MS, type TeamHostApi } from '../src/main/teams/connector';
import { TeamPrivateStore, digest, type TeamModelRoute, type TeamPersonaSnapshot } from '../src/main/teams/privateStore';
import { runTeamHost, TeamHostFailure, type TeamHostProcessOptions, type TeamHostResult } from '../src/main/teams/process';
import { botFixture } from './bot-helpers';
import type { PublishLocalBotInput } from '../../../packages/teams/execution';
const teamId='team00001',sharedBotId='sharedbot001',conversationId='chat00001';
const tick=()=>new Promise<void>(resolve=>setImmediate(resolve));
const cursor={scope:null,sessionRevision:0,lastMessageSeq:0,lastChangeSeq:0,bootstrapFromSeq:0};
function route():TeamModelRoute{return {model_provider:'anthropic',model_protocol:'anthropic_messages',model_id:'synthetic',model_endpoint:'https://api.anthropic.com',credential_id:'anthropic',model_profile:{profile_name:'anthropic'}};}
function accounts(){return {providerAccountHome:'/synthetic',providerAccounts:{storeScope:'fixture',configRevision:1,accounts:[],health:[]},providerAccountCredentials:{}};}
function snapshot():TeamPersonaSnapshot{const text='Only the published role';return {...route(),instructions:text,provider_account_profiles:[route().model_profile],documents:[{resourceId:'resource001',revision:1,kind:'persona',name:'Role',text,digest:digest(text)}]};}
function fakeService(){
 const events=new EventEmitter();let context:TeamAccountContext|undefined={uid:'owner',epoch:0};const commands:any[]=[],tasks:any[]=[],reads:any[]=[],cards:any[]=[];let deployment:any,inputs:any[]=[];const publicationReceipts=new Map<string,any>();
 const service:TeamHostApi={hostContext:()=>context,assertHostContext:c=>{if(!context||c.uid!==context.uid||c.epoch!==context.epoch)throw Error('INVALID_SESSION');},list:async()=>({teams:[{id:teamId,name:'Team',memberCount:1,role:'owner',generation:1}]}),onChanged:cb=>{events.on('change',cb);return()=>events.off('change',cb);},onMessagesChanged:cb=>{events.on('messages',cb);return()=>events.off('messages',cb);},
 hostMessages:async<T>(c,command)=>{service.assertHostContext(c);commands.push(command);const p=command.payload as any;let result:any={};
 if(command.action==='publishSharedBot'){if(publicationReceipts.has(command.commandId as string))return publicationReceipts.get(command.commandId as string) as T;deployment=p.deployment;inputs=p.inputs.map((v:any)=>({...v,grantId:digest(JSON.stringify([p.sharedBotId,p.expectedRevision+1,v.resourceId]))}));const previous=cards.findIndex(card=>card.sharedBotId===p.sharedBotId);if(previous>=0)cards.splice(previous,1);cards.push({sharedBotId:p.sharedBotId,state:'published',revision:p.expectedRevision+1,maintainer:{uid:'owner',generation:1}});result={sharedBotId:p.sharedBotId,revision:p.expectedRevision+1,grantIds:inputs.map(v=>v.grantId)};publicationReceipts.set(command.commandId as string,result);}
 else if(command.action==='listSharedBots')result={bots:cards};
 else if(command.action==='activateDeployment'){assert.equal(p.expectedAccountEpoch,deployment.accountEpoch);deployment.accountEpoch=p.newAccountEpoch;}
 else if(command.action==='listHostTasks')result={tasks:tasks.filter(t=>t.state==='queued')};
 else if(command.action==='prepareTask'||command.action==='validateTask'||command.action==='startTask'){const task=tasks.find(t=>t.taskId===p.taskId);result={task:{...task},messageId:task.messageId,messageSeq:task.messageSeq,inputs,deployment};if(command.action==='startTask'){task.state='running';result={...result,task:{...task},execute:true,permitId:'permit00001'};}}
 else if(command.action==='completeTask'){const task=tasks.find(t=>t.taskId===p.taskId);if(task)task.state='completed';}
 return result as T;},
 hostReadChat:async(_c,request)=>{reads.push(request);if(request.mode==='changes')return {messages:[],lastSeq:1000,lastChangeSeq:1000,hasMore:false};const through=request.mode==='recent'?request.throughSeq:request.mode==='range'?request.toSeqInclusive:1;const from=request.mode==='recent'?Math.max(0,through-200):request.mode==='range'?request.fromSeqExclusive:0;
 return {messages:Array.from({length:through-from},(_,i)=>({messageId:`message${String(from+i+1).padStart(6,'0')}`,seq:from+i+1,changeSeq:from+i+1,changeId:`change${String(from+i+1).padStart(6,'0')}`,author:{kind:'user' as const,uid:i%2?'human2':'human1',displayName:'Same name'},createdAt:1700000000000+i,text:`original ${i}`})),lastSeq:1000,lastChangeSeq:1000,hasMore:from>0};},};
 return {service,commands,tasks,reads,wake:()=>events.emit('messages'),flicker:()=>events.emit('change',{invalidated:true}),switch:(uid='other',epoch=1)=>{context={uid,epoch};events.emit('change',{invalidated:true});}};
}
function fixture(t:any,run?:TeamHostProcessOptions extends never?never:(o:TeamHostProcessOptions)=>Promise<TeamHostResult>){
 const f=botFixture(t),privateStore=new TeamPrivateStore(f.userData),mock=fakeService(),bot=f.store.create({name:'Personal',persona:'PRIVATE_ROLE_NOT_SHARED',model:'anthropic/synthetic'});let selected:string[]=[];
 const connector=new TeamConnector({service:mock.service,store:f.store,privateStore,binaries:()=>({host:'/mock/host',worker:'/mock/worker'}),defaultModel:()=>undefined,modelRoute:async()=>({route:route(),profiles:[route().model_profile],assertCurrent:async()=>{}}),modelAccounts:async()=>accounts(),chooseFiles:async()=>selected,validateModel:async()=>{},platform:'darwin',retryMs:60000,run:run??(async o=>o.bootstrap.type==='inspect_scope'?{status:'inspected',cursor}:{status:'completed',cursor,text:'Done',finishedAt:Date.now()})});
 t.after(()=>connector.close());const input:PublishLocalBotInput={teamId,generation:1,publicationId:'publication001',sharedBotId,instanceId:bot.bot_id,expectedRevision:0,usePolicy:{mode:'all'},fileTokens:[],quotas:{team:100,bot:100,member:10},role:'Explicit published role'};
 return {...f,privateStore,mock,bot,connector,input,select:(paths:string[])=>{selected=paths;}};
}
/** The record of a task whose permit this host holds, as `execute()` saves it. */
async function savedExecution(f:{privateStore:TeamPrivateStore},fields:{scope:any;teamId:string;taskId:string;deadlineAt:number;commandId:string;payload:Record<string,unknown>}&Record<string,unknown>){
 const batch=await f.privateStore.dataRef('owner',{batch:fields.taskId});return {schemaVersion:2 as const,generation:1,epoch:1,messageSeq:1,lastChangeSeq:1000,batch,ask:false,dispatched:0,recoveries:0,...fields};
}
async function settle(check:()=>boolean){for(let i=0;i<150;i++){if(check())return;await new Promise(r=>setTimeout(r,5));}assert.ok(check(),'condition settled');}

test('execution storage never reads retired host or writes personal definitions',async t=>{
 const f=fixture(t);mkdirSync(join(f.userData,'team-bot-host'));writeFileSync(join(f.userData,'team-bot-host','sentinel'),'old');const before=readFileSync(f.store.recordPath(f.bot.bot_id),'utf8');
 await f.connector.publishLocalBot('owner',f.input);assert.equal(readFileSync(f.store.recordPath(f.bot.bot_id),'utf8'),before);assert.equal(readFileSync(join(f.userData,'team-bot-host','sentinel'),'utf8'),'old');
 const b=(await f.privateStore.bindings('owner'))[0]!;assert.notEqual(b.sharedBotId,b.instanceId);assert.notEqual(b.deploymentId,f.privateStore.hostId);assert.equal(b.accountEpoch,1);const s=await f.privateStore.snapshot('owner',b.snapshot);assert.equal(s.instructions,f.input.role);assert.ok(!JSON.stringify(s).includes('PRIVATE_ROLE_NOT_SHARED'));
});
test('native selection freezes text and exact Skill grants; renderer paths cannot authorize files',async t=>{
 const f=fixture(t),path=join(f.root,'SKILL.md');writeFileSync(path,'Published skill text');f.select([path]);const [selected]=await f.connector.selectSharedFiles('owner',f.bot.bot_id);assert.equal(selected!.kind,'skill');assert.equal((selected as any).path,undefined);writeFileSync(path,'Changed after selection');
 await f.connector.publishLocalBot('owner',{...f.input,fileTokens:[selected!.token]});const binding=(await f.privateStore.bindings('owner'))[0]!;const s=await f.privateStore.snapshot('owner',binding.snapshot);assert.equal(s.documents[1]!.text,'Published skill text');assert.equal(s.documents[1]!.kind,'skill');
 await assert.rejects(f.connector.publishLocalBot('owner',{...f.input,publicationId:'publication002',sharedBotId:'sharedbot002',fileTokens:[path]}),/INVALID_SHARED_FILE_TOKEN/);
});
test('chooser rejects symlinks, hardlinks and tokens after account switch',async t=>{
 const f=fixture(t),path=join(f.root,'public.txt');writeFileSync(path,'Shared');const link=join(f.root,'linked.txt');symlinkSync(path,link);f.select([link]);await assert.rejects(f.connector.selectSharedFiles('owner',f.bot.bot_id));
 const hard=join(f.root,'hard.txt');linkSync(path,hard);f.select([hard]);await assert.rejects(f.connector.selectSharedFiles('owner',f.bot.bot_id),/INVALID_SHARED_FILE/);
 const fresh=join(f.root,'fresh.txt');writeFileSync(fresh,'Fresh');f.select([fresh]);const [selection]=await f.connector.selectSharedFiles('owner',f.bot.bot_id);f.mock.switch();await assert.rejects(f.connector.publishLocalBot('other',{...f.input,fileTokens:[selection!.token]}),/INVALID_SHARED_FILE_TOKEN/);
});
test('snapshots reject content tampering and store modes reject public directories',async t=>{
 const f=fixture(t),value=snapshot(),key=await f.privateStore.saveSnapshot('owner',value),paths=await f.privateStore.owner('owner');writeFileSync(join(paths.snapshots,key),'{}');await assert.rejects(f.privateStore.snapshot('owner',key),/INVALID_TEAM_SNAPSHOT/);
 chmodSync(paths.bindings,0o755);await assert.rejects(f.privateStore.bindings('owner'),/UNSAFE_TEAM_STORAGE/);
});
test('large input is a verified private transport reference, not a JSONL frame',async t=>{
 const f=fixture(t),value={messages:Array.from({length:450},(_,i)=>({seq:i+1,text:'x'.repeat(1000)}))},ref=await f.privateStore.dataRef('owner',value);assert.ok(ref.bytes>256*1024);assert.ok(ref.path.startsWith((await f.privateStore.owner('owner')).runtime));assert.equal(digest(readFileSync(ref.path,'utf8')),ref.sha256);await f.privateStore.releaseDataRef('owner',ref);assert.equal(existsSync(ref.path),false);
});
test('delivery outbox is immutable and account-private',async t=>{
 const f=fixture(t),delivery={schemaVersion:1 as const,teamId,generation:1,taskId:'task00001',deadlineAt:Date.now()+1000,commandId:'delivery001',payload:{text:'One result'}};await f.privateStore.saveDelivery('owner',delivery);await f.privateStore.saveDelivery('owner',delivery);await assert.rejects(f.privateStore.saveDelivery('owner',{...delivery,payload:{text:'changed'}}),/UNSAFE_TEAM_STORAGE/);assert.deepEqual(await f.privateStore.deliveries('other'),[]);
});
test('a write torn at exit leaves only a skipped temporary file, never a partial record',async t=>{
 const f=fixture(t),paths=await f.privateStore.owner('owner');
 for(const dir of [paths.outbox,paths.executions])writeFileSync(join(dir,'.write-0123456789abcdef'),'{"schemaVer',{mode:0o600});
 assert.deepEqual(await f.privateStore.deliveries('owner'),[]);assert.deepEqual(await f.privateStore.executions('owner'),[]);
 const delivery={schemaVersion:1 as const,teamId,generation:1,taskId:'task00002',deadlineAt:Date.now()+1000,commandId:'delivery002',payload:{text:'Kept'}};
 await f.privateStore.saveDelivery('owner',delivery);assert.deepEqual(await f.privateStore.deliveries('owner'),[delivery]);
 await assert.rejects(f.privateStore.saveDelivery('owner',{...delivery,payload:{text:'changed'}}),/UNSAFE_TEAM_STORAGE/);
});
test('all Bots read the service data session, including current ACL and account fence',async()=>{
 let uid='owner',epoch=0;const service=new TeamsService({snapshot:()=>({user:{uid}}),sessionEpoch:()=>epoch,getIdToken:async()=>'',onChanged:()=>()=>{}},{});const request={teamId,generation:1,conversationId,mode:'before' as const,beforeSeq:20};let reads=0;
 (service as any).dataSession={read:async(value:unknown)=>{assert.equal(value,request);reads++;return {messages:[],lastSeq:20,lastChangeSeq:20,hasMore:false};},close:async()=>{}};
 await service.hostReadChat(service.hostContext()!,request);assert.equal(reads,1);const old=service.hostContext()!;uid='other';epoch++;await assert.rejects(service.hostReadChat(old,request),/INVALID_SESSION/);service.close();
});
test('public message boundary rejects retired execution and private host commands',async()=>{
 const service=new TeamsService({snapshot:()=>({user:{uid:'owner'}}),getIdToken:async()=>'',onChanged:()=>()=>{}},{});
 for(const action of ['bindBot','claimBot','startBot','finishBot','botQueue','prepareTask','startTask','heartbeatDeployment'])await assert.rejects(service.messages({teamId,generation:1,action} as any,'owner'),/INVALID_INPUT/);service.close();
});
test('scheduler is bounded at four and serializes the same Bot Chat',async t=>{
 const runs:TeamHostProcessOptions[]=[];const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};runs.push(o);return new Promise<TeamHostResult>((_resolve,reject)=>o.signal.addEventListener('abort',()=>reject(Error('abort')),{once:true}));});
 await f.connector.publishLocalBot('owner',f.input);for(let i=0;i<7;i++)f.mock.tasks.push({taskId:`task0000${i}`,conversationId:i<2?'samechat01':`chat0000${i}`,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>runs.length===4);assert.equal(new Set(runs.map(r=>r.bootstrap.scope.conversationId)).size,4);assert.equal(runs.filter(r=>r.bootstrap.scope.conversationId==='samechat01').length,1);f.mock.switch();f.connector.revoke();await settle(()=>runs.every(r=>r.signal.aborted));
});
test('bootstrap includes latest 200 messages with separate author and original time',async t=>{
 let batch:any;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};batch=JSON.parse(readFileSync(o.bootstrap.inputBatchRef!.path,'utf8'));return {status:'completed',cursor,text:'Done',finishedAt:Date.now()};});await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'human2',generation:1},state:'queued',messageId:'message000450',messageSeq:450,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>Boolean(batch));assert.equal(batch.messages.length,200);assert.equal(batch.fromSeqExclusive,250);assert.equal(batch.messages[0].seq,251);assert.notEqual(batch.messages[0].author.uid,batch.messages[1].author.uid);assert.equal(batch.messages[0].createdAt,1700000000000);assert.deepEqual(batch.changes,[]);
 await settle(()=>f.mock.tasks[0].state==='completed');assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,1);
});
test('subsequent input delivers the complete 450 message interval without truncation',async t=>{
 let batch:any;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor:{...cursor,scope:o.bootstrap.scope,sessionRevision:1,lastMessageSeq:200,lastChangeSeq:200}};batch=JSON.parse(readFileSync(o.bootstrap.inputBatchRef!.path,'utf8'));return {status:'completed',cursor,text:'Done',finishedAt:Date.now()};});await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'human2',generation:1},state:'queued',messageId:'message000650',messageSeq:650,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>Boolean(batch));assert.equal(batch.messages.length,450);assert.equal(batch.fromSeqExclusive,200);assert.equal(batch.toSeqInclusive,650);assert.equal(batch.bootstrap,false);assert.ok(f.mock.reads.some(r=>r.mode==='changes'&&r.afterChangeSeq===200));
});
function processFixture(version=3){const child=new EventEmitter() as ChildProcessWithoutNullStreams;Object.assign(child,{stdin:new PassThrough(),stdout:new PassThrough(),stderr:new PassThrough(),exitCode:null,kill:()=>{}});let spawnOptions:any;child.stdin.once('data',()=>{setImmediate(()=>{child.stdout.emit('data',Buffer.from(JSON.stringify({version,type:'result',status:'inspected',cursor})+'\n'));child.emit('close',0);});});return {child,spawn:(_exe:any,_args:any,options:any)=>{spawnOptions=options;return child;},options:()=>spawnOptions};}
test('private major3 process rejects retired frames and clears inherited environment',async()=>{
 for(const version of [1,3]){const f=processFixture(version);const run=runTeamHost({executable:'/mock',home:'/private-home',tmp:'/private-tmp',bootstrap:{version:3,type:'inspect_scope',storageRoot:'/private-root',workerExecutable:'/worker',scope:{teamId,sharedBotId,conversationId,sessionEpoch:1}},signal:new AbortController().signal,assertCurrent:()=>{},spawnProcess:f.spawn});if(version===1)await assert.rejects(run,/TEAM_HOST_PROTOCOL/);else assert.equal((await run).status,'inspected');assert.equal(f.options().env.HOME,'/private-home');assert.equal(f.options().env.OPENAI_API_KEY,undefined);assert.equal(f.options().env.LINGXI_CONFIG_DIR,'/private-home');}
});

test('lost publication ACK retries the same intent without another model probe',async t=>{
 const f=fixture(t);let probes=0;(f.connector as any).options.validateModel=async()=>{probes++;};const original=f.mock.service.hostMessages.bind(f.mock.service);let lose=true;
 f.mock.service.hostMessages=async(c,command)=>{const result=await original(c,command);if(command.action==='publishSharedBot'&&lose){lose=false;throw Error('SERVICE_UNAVAILABLE');}return result;};
 await assert.rejects(f.connector.publishLocalBot('owner',f.input),/SERVICE_UNAVAILABLE/);const retry=await f.connector.publishLocalBot('owner',f.input);assert.equal(retry.sharedBotId,sharedBotId);assert.equal(probes,1);
 const publishes=f.mock.commands.filter(c=>c.action==='publishSharedBot');assert.equal(publishes.length,2);assert.deepEqual(publishes[0],publishes[1]);await assert.rejects(f.connector.publishLocalBot('owner',{...f.input,role:'changed'}),/COMMAND_ID_CONFLICT/);
});
test('publication refuses model failure before enabling deployment',async t=>{
 const f=fixture(t);(f.connector as any).options.validateModel=async()=>{throw Error('TEAM_MODEL_UNAVAILABLE');};await assert.rejects(f.connector.publishLocalBot('owner',f.input),/TEAM_MODEL_UNAVAILABLE/);assert.equal(f.mock.commands.filter(c=>c.action==='publishSharedBot').length,0);assert.deepEqual(await f.privateStore.bindings('owner'),[]);
});
test('identical read results use independent transport lifetimes',async t=>{
 const f=fixture(t),a=await f.privateStore.dataRef('owner',{same:'source'}),b=await f.privateStore.dataRef('owner',{same:'source'});assert.equal(a.sha256,b.sha256);assert.notEqual(a.path,b.path);await f.privateStore.releaseDataRef('owner',a);assert.ok(existsSync(b.path));await f.privateStore.releaseDataRef('owner',b);
});

test('rejected republication preserves the active binding and running worker',async t=>{
 const runs:TeamHostProcessOptions[]=[];const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};runs.push(o);return new Promise<TeamHostResult>((_resolve,reject)=>o.signal.addEventListener('abort',()=>reject(Error('abort')),{once:true}));});
 await f.connector.publishLocalBot('owner',f.input);const prior=(await f.privateStore.bindings('owner'))[0]!;
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>runs.length===1);
 const original=f.mock.service.hostMessages.bind(f.mock.service);f.mock.service.hostMessages=async(c,command)=>{if(command.action==='publishSharedBot'&&(command.payload as any).expectedRevision===1)throw Error('BOT_REVISION_CONFLICT');return original(c,command);};
 await assert.rejects(f.connector.publishLocalBot('owner',{...f.input,publicationId:'publication002',expectedRevision:1,role:'New role rejected'}),/BOT_REVISION_CONFLICT/);
 assert.deepEqual((await f.privateStore.bindings('owner'))[0],prior);assert.equal(runs[0]!.signal.aborted,false);f.connector.close();
});
test('host pagination continues past an empty stale page to a live task',async t=>{
 let runs=0;const f=fixture(t,async o=>o.bootstrap.type==='inspect_scope'?{status:'inspected',cursor}:{status:'completed',cursor,text:`Done ${++runs}`,finishedAt:Date.now()});
 await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service);
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='listHostTasks'&&!(command.payload as any).after)return {tasks:[],nextCursor:'page00001'} as any;return original(c,command);};
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>runs===1);assert.ok(f.mock.commands.some(c=>c.action==='listHostTasks'&&c.payload.after==='page00001'));
});
test('a fresh source watermark uses a new start command after a denied permit',async t=>{
 let runs=0,starts=0,watermark=1000;const commands:any[]=[];const f=fixture(t,async o=>o.bootstrap.type==='inspect_scope'?{status:'inspected',cursor}:{status:'completed',cursor,text:`Done ${++runs}`,finishedAt:Date.now()});(f.connector as any).options.retryMs=10;
 await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service),read=f.mock.service.hostReadChat.bind(f.mock.service);
 f.mock.service.hostReadChat=async(c,q)=>({...await read(c,q),lastChangeSeq:watermark});
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='startTask'){commands.push(command);if(++starts===1){watermark=1001;return {execute:false,taskId:'task00001'} as any;}}return original(c,command);};
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>runs===1);
 assert.equal(commands.length,2);assert.notEqual(commands[0].commandId,commands[1].commandId);assert.deepEqual(commands.map(c=>c.payload.expectedLastChangeSeq),[1000,1001]);
});
test('history broker rejects future before, range and byId messages',async t=>{
 let checked=false;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};
 await assert.rejects(o.readChat!({kind:'before',before_seq:3,limit:1}),/HISTORY_BOUND_EXCEEDED/);
 await assert.rejects(o.readChat!({kind:'range',from_seq_exclusive:0,to_seq_inclusive:2}),/HISTORY_BOUND_EXCEEDED/);
 await assert.rejects(o.readChat!({kind:'by_id',message_id:'future0001'}),/HISTORY_BOUND_EXCEEDED/);checked=true;
 return {status:'completed',cursor,text:'Bounded',finishedAt:Date.now()};});await f.connector.publishLocalBot('owner',f.input);
 const read=f.mock.service.hostReadChat.bind(f.mock.service);f.mock.service.hostReadChat=async(c,q)=>{const result=await read(c,q);return q.mode==='byId'?{...result,messages:result.messages.map(m=>({...m,seq:999}))}:result;};
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>checked);
 assert.equal(f.mock.reads.filter(q=>q.mode==='before'||q.mode==='range').length,0);
});

test('old publication receipts never replace a newer publication binding',async t=>{
 const f=fixture(t);await f.connector.publishLocalBot('owner',f.input);
 await f.connector.publishLocalBot('owner',{...f.input,publicationId:'publication002',expectedRevision:1,role:'Second published role'});
 const current=(await f.privateStore.bindings('owner'))[0]!;assert.equal(current.revision,2);
 const receipt=await f.connector.publishLocalBot('owner',f.input);assert.equal(receipt.revision,1);assert.deepEqual((await f.privateStore.bindings('owner'))[0],current);
 await assert.rejects(f.privateStore.saveBinding('owner',{...current,revision:1}),/STALE_PUBLICATION_BINDING/);
});
test('replaying the same publication preserves the advanced host account fence',async t=>{
 const f=fixture(t);await f.connector.publishLocalBot('owner',f.input);f.connector.revoke();
 await settle(()=>f.mock.commands.some(c=>c.action==='activateDeployment'));await f.connector.idle();const current=(await f.privateStore.bindings('owner'))[0]!;assert.ok(current.accountEpoch>1);
 await f.connector.publishLocalBot('owner',f.input);assert.deepEqual((await f.privateStore.bindings('owner'))[0],current);
 await assert.rejects(f.privateStore.saveBinding('owner',{...current,accountEpoch:1}),/STALE_PUBLICATION_BINDING/);
});
test('a denied start followed by account activation uses the full new start payload identity',async t=>{
 let runs=0,starts=0;const commands:any[]=[];const f=fixture(t,async o=>o.bootstrap.type==='inspect_scope'?{status:'inspected',cursor}:{status:'completed',cursor,text:`Done ${++runs}`,finishedAt:Date.now()});(f.connector as any).options.retryMs=10;
 await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service);
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='startTask'){commands.push(command);if(++starts===1){queueMicrotask(()=>f.connector.revoke());return {execute:false,taskId:'task00001'} as any;}}return original(c,command);};
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>runs===1);
 assert.equal(commands.length,2);assert.equal(commands[0].payload.expectedLastChangeSeq,commands[1].payload.expectedLastChangeSeq);assert.ok(commands[1].payload.accountEpoch>commands[0].payload.accountEpoch);assert.notEqual(commands[0].commandId,commands[1].commandId);
});

test('restart reconciles an acknowledged publication candidate without a second model probe',async t=>{
 const f=fixture(t);let probes=0;(f.connector as any).options.validateModel=async()=>{probes++;};const original=f.mock.service.hostMessages.bind(f.mock.service);let fail=true;
 f.mock.service.hostMessages=async(c,command)=>{const result=await original(c,command);if(command.action==='publishSharedBot'&&fail){fail=false;f.connector.close();throw Error('SERVICE_UNAVAILABLE');}return result;};
 await assert.rejects(f.connector.publishLocalBot('owner',f.input),/SERVICE_UNAVAILABLE/);assert.deepEqual(await f.privateStore.bindings('owner'),[]);
 const restarted=new TeamConnector({...((f.connector as any).options)});t.after(()=>restarted.close());await restarted.idle();
 const binding=(await f.privateStore.bindings('owner'))[0]!;assert.equal(binding.publicationId,f.input.publicationId);assert.equal(binding.revision,1);assert.ok(binding.accountEpoch>1);assert.equal(probes,1);
});
test('lost delivery ACK survives host restart and account activation without a new model call',async t=>{
 let modelCalls=0;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};modelCalls++;return {status:'completed',cursor,text:'Exact durable reply',finishedAt:Date.now(),usage:{status:'partial',inputTokens:17,outputTokens:5}};});
 await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service);let lose=true;
 f.mock.service.hostMessages=async(c,command)=>{const result=await original(c,command);if(command.action==='completeTask'&&lose){lose=false;f.connector.close();throw Error('SERVICE_UNAVAILABLE');}return result;};
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>!lose);assert.equal((await f.privateStore.deliveries('owner')).length,1);
 const restarted=new TeamConnector({...((f.connector as any).options)});t.after(()=>restarted.close());await restarted.idle();assert.deepEqual(await f.privateStore.deliveries('owner'),[]);assert.equal(modelCalls,1);
 const delivered=f.mock.commands.filter(c=>c.action==='completeTask');assert.equal(delivered.length,2);const [first,second]=delivered;assert.ok(second.payload.accountEpoch>first.payload.accountEpoch);assert.notEqual(first.commandId,second.commandId);assert.deepEqual({...second.payload,accountEpoch:first.payload.accountEpoch},first.payload);assert.deepEqual(second.payload.usage,{status:'partial',inputTokens:17,outputTokens:5});
});

test('future appends between range and change reads preserve the captured task watermark',async t=>{
 let batch:any;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor:{...cursor,scope:o.bootstrap.scope,sessionRevision:1,lastMessageSeq:200,lastChangeSeq:200}};
 batch=JSON.parse(readFileSync(o.bootstrap.inputBatchRef!.path,'utf8'));return {status:'completed',cursor,text:'Done',finishedAt:Date.now()};});await f.connector.publishLocalBot('owner',f.input);
 const read=f.mock.service.hostReadChat.bind(f.mock.service);f.mock.service.hostReadChat=async(c,q)=>{const result=await read(c,q);return q.mode==='changes'?{...result,lastChangeSeq:1001}:result;};
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000201',messageSeq:201,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>Boolean(batch));
 assert.equal(batch.lastChangeSeq,1000);assert.equal(batch.toSeqInclusive,201);assert.deepEqual(batch.messages.map((m:any)=>m.seq),[201]);assert.deepEqual(batch.changes,[]);
 assert.equal(f.mock.commands.find(c=>c.action==='startTask')?.payload.expectedLastChangeSeq,1000);
});
test('an older source edited between range and change reads rejects before the start permit',async t=>{
 let modelCalls=0;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor:{...cursor,scope:o.bootstrap.scope,sessionRevision:1,lastMessageSeq:200,lastChangeSeq:200}};modelCalls++;return {status:'completed',cursor,text:'Unexpected',finishedAt:Date.now()};});await f.connector.publishLocalBot('owner',f.input);
 const read=f.mock.service.hostReadChat.bind(f.mock.service);f.mock.service.hostReadChat=async(c,q)=>{const result=await read(c,q);return q.mode==='changes'?{...result,lastChangeSeq:1001,messages:[{messageId:'message000002',seq:2,changeSeq:1001,changeId:'changed0001',author:{kind:'user' as const,uid:'member',displayName:'Member'},createdAt:1700000000002,editedAt:Date.now(),text:'New edit'}]}:result;};
 f.mock.tasks.push({taskId:'task00001',conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000201',messageSeq:201,createdAt:Date.now(),deadline:Date.now()+300000});f.mock.wake();await settle(()=>f.mock.reads.some(r=>r.mode==='changes')&&(f.connector as any).active.size===0);
 assert.equal(modelCalls,0);assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,0);assert.deepEqual(await f.privateStore.executions('owner'),[]);
});

const queued=(taskId='task00001')=>({taskId,conversationId,sharedBotId,requester:{uid:'member',generation:1},state:'queued',messageId:'message000001',messageSeq:1,createdAt:Date.now(),deadline:Date.now()+300000});
const completions=(f:{mock:{commands:any[]}})=>f.mock.commands.filter(c=>c.action==='completeTask');
const hold=(runs:TeamHostProcessOptions[],release:Array<(r:TeamHostResult)=>void>=[])=>async(o:TeamHostProcessOptions):Promise<TeamHostResult>=>{
 if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};
 runs.push(o);return new Promise<TeamHostResult>((resolve,reject)=>{release.push(resolve);o.signal.addEventListener('abort',()=>reject(Error('INVALID_SESSION')),{once:true});});};

test('a cache or listener flicker mid-run neither aborts the run nor re-activates the deployment',async t=>{
 const runs:TeamHostProcessOptions[]=[],release:Array<(r:TeamHostResult)=>void>=[];const f=fixture(t,hold(runs,release));
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await settle(()=>runs.length===1);
 const activations=f.mock.commands.filter(c=>c.action==='activateDeployment').length,epoch=(await f.privateStore.bindings('owner'))[0]!.accountEpoch;
 f.mock.flicker();f.mock.flicker();await f.connector.idle();await new Promise(r=>setTimeout(r,30));
 assert.equal(runs[0]!.signal.aborted,false);assert.equal(f.mock.commands.filter(c=>c.action==='activateDeployment').length,activations);assert.equal((await f.privateStore.bindings('owner'))[0]!.accountEpoch,epoch);
 release[0]!({status:'completed',cursor,text:'Survived',finishedAt:Date.now()});await settle(()=>completions(f).length===1);
 assert.equal(completions(f)[0].payload.outcome,'completed');assert.equal(completions(f)[0].payload.text,'Survived');assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,1);
});
test('explicit revocation mid-run aborts the worker and ends the task with one failed delivery',async t=>{
 for(const revoke of ['stopBinding','revoke'] as const){
  const runs:TeamHostProcessOptions[]=[],events:Array<[string,any]>=[];const f=fixture(t,hold(runs));(f.connector as any).options.retryMs=10;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
  await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await settle(()=>runs.length===1);
  if(revoke==='stopBinding')f.connector.stopBinding(teamId,sharedBotId);else f.connector.revoke();
  await settle(()=>completions(f).length===1);await f.connector.idle();await new Promise(r=>setTimeout(r,60));
  assert.equal(runs[0]!.signal.aborted,true);assert.equal(completions(f).length,1);assert.equal(completions(f)[0].payload.outcome,'failed');assert.ok(completions(f)[0].payload.text);
  assert.equal(runs.length,1);assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,1);assert.deepEqual(await f.privateStore.executions('owner'),[]);assert.deepEqual(await f.privateStore.deliveries('owner'),[]);
  assert.ok(events.some(([event,d])=>event==='team_task_aborted'&&d.taskId==='task00001'&&d.reason===(revoke==='revoke'?'REVOKED':'BINDING_STOPPED')));
  assert.ok(events.some(([event,d])=>event==='team_task_failed'&&d.taskId==='task00001'&&d.aborted===true));
  f.connector.close();
 }
});
test('a server-rejected failed delivery is dropped instead of retried forever',async t=>{
 const runs:TeamHostProcessOptions[]=[];const f=fixture(t,hold(runs));(f.connector as any).options.retryMs=10;await f.connector.publishLocalBot('owner',f.input);
 const original=f.mock.service.hostMessages.bind(f.mock.service);f.mock.service.hostMessages=async(c,command)=>{if(command.action==='completeTask'){await original(c,command);throw new TeamsApiError('TASK_REVOKED',403);}return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>runs.length===1);f.connector.stopBinding(teamId,sharedBotId);
 await settle(()=>completions(f).length===1);await settle(()=>true);await new Promise(r=>setTimeout(r,80));await f.connector.idle();
 assert.equal(completions(f).length,1);assert.deepEqual(await f.privateStore.deliveries('owner'),[]);assert.deepEqual(await f.privateStore.executions('owner'),[]);
});
test('a worker failure after the permit delivers one failed result and never replays the permit',async t=>{
 let runs=0;const events:Array<[string,any]>=[];const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};runs++;throw new TeamHostFailure('MODEL_UNAVAILABLE');});
 (f.connector as any).options.retryMs=10;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await settle(()=>completions(f).length===1);await f.connector.idle();await new Promise(r=>setTimeout(r,100));
 assert.equal(runs,1);assert.equal(completions(f).length,1);assert.equal(completions(f)[0].payload.outcome,'failed');assert.equal(completions(f)[0].payload.text,'The assistant could not complete this request.');
 assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,1);assert.deepEqual(await f.privateStore.executions('owner'),[]);assert.deepEqual(await f.privateStore.deliveries('owner'),[]);
 assert.deepEqual(events.find(([event])=>event==='team_task_failed')?.[1],{taskId:'task00001',code:'MODEL_UNAVAILABLE',aborted:false});assert.ok(!JSON.stringify(events).includes('PRIVATE_ROLE_NOT_SHARED'));
});
test('a leftover execution with no result is finished as failed once its deadline passes',async t=>{
 let inspections=0,clock=Date.now();const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result'){inspections++;return {status:'not_found',cursor};}throw Error('model must not run');});
 (f.connector as any).options.retryMs=10;(f.connector as any).options.now=()=>clock;await f.connector.publishLocalBot('owner',f.input);const b=(await f.privateStore.bindings('owner'))[0]!,deadline=clock+60000;
 await f.privateStore.saveExecution('owner',await savedExecution(f,{scope:{teamId,sharedBotId,conversationId,sessionEpoch:b.sessionEpoch},teamId,taskId:'task00001',deadlineAt:deadline,commandId:'delivery001',
  payload:{deploymentId:b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',permitId:'permit0001',inputGrantIds:b.grantIds}}));
 f.mock.wake();await settle(()=>inspections>=1);await new Promise(r=>setTimeout(r,40));assert.equal(completions(f).length,0);assert.equal((await f.privateStore.executions('owner')).length,1);
 clock=deadline+1;f.mock.wake();await settle(()=>completions(f).length===1);await f.connector.idle();
 assert.equal(completions(f)[0].payload.outcome,'failed');assert.equal(completions(f)[0].payload.permitId,'permit0001');assert.ok(completions(f)[0].payload.finishedAt<=deadline);
 assert.deepEqual(await f.privateStore.executions('owner'),[]);assert.deepEqual(await f.privateStore.deliveries('owner'),[]);
 const seen=inspections;f.mock.wake();await new Promise(r=>setTimeout(r,80));await f.connector.idle();assert.equal(inspections,seen);assert.equal(completions(f).length,1);
});
test('a fromCache snapshot reaches listeners but never revokes the host; an account reset does',async t=>{
 let uid='owner',epoch=0,notify=()=>{},revokes=0,snapshotCallback:((s:{metadata:{fromCache:boolean}})=>void)|undefined;
 const service=new TeamsService({snapshot:()=>({user:{uid}}),sessionEpoch:()=>epoch,getIdToken:async()=>'',onChanged:cb=>{notify=cb;return()=>{};}},{});t.after(()=>service.close());
 (service as any).session={db:{},close:async()=>{}};(service as any).sdk={collection:()=>({}),onSnapshot:(_c:unknown,_o:unknown,next:typeof snapshotCallback)=>{snapshotCallback=next;return()=>{};}};
 (service as any).request=async()=>({team:{generation:1}});
 service.attachHost({localBots:async()=>[],selectSharedFiles:async()=>[],publishLocalBot:async()=>({sharedBotId,revision:1}),revoke:()=>{revokes++;},stopBinding:()=>{},close:()=>{}});
 const events:any[]=[];service.onChanged(e=>events.push(e));await service.detail(teamId,'owner');assert.ok(snapshotCallback);
 snapshotCallback!({metadata:{fromCache:true}});assert.deepEqual(events,[{invalidated:true}]);assert.equal(revokes,0);
 uid='other';epoch++;notify();assert.equal(revokes,1);
});

const seedDelivery=async(f:ReturnType<typeof fixture>,deadlineAt:number,deploymentId?:string,team=teamId)=>{
 const b=(await f.privateStore.bindings('owner'))[0]!,payload={deploymentId:deploymentId??b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',permitId:'permit0001',inputGrantIds:b.grantIds};
 await f.privateStore.saveExecution('owner',await savedExecution(f,{scope:{teamId:team,sharedBotId,conversationId,sessionEpoch:b.sessionEpoch},teamId:team,taskId:'task00001',deadlineAt,commandId:'delivery001',payload}));
 await f.privateStore.saveDelivery('owner',{schemaVersion:1,teamId:team,generation:1,taskId:'task00001',deadlineAt,commandId:'delivery001',payload:{...payload,outcome:'failed',text:'Failed',finishedAt:deadlineAt-1,outputAudience:{mode:'published_inputs'},usage:{status:'missing'}}});
};
const refuseCompletions=(f:ReturnType<typeof fixture>,code:string,status:number)=>{const original=f.mock.service.hostMessages.bind(f.mock.service),attempts={count:0};
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='completeTask'){attempts.count++;throw new TeamsApiError(code,status);}return original(c,command);};return attempts;};
const pendingFiles=async(f:ReturnType<typeof fixture>)=>[(await f.privateStore.deliveries('owner')).length,(await f.privateStore.executions('owner')).length];
const wakeAndIdle=async(f:ReturnType<typeof fixture>)=>{f.mock.wake();await f.connector.idle();};

test('TASK_EXPIRED from a host clock ahead of the server keeps the result and delivers it on the next try',async t=>{
 const events:Array<[string,any]>=[];const f=fixture(t,async o=>o.bootstrap.type==='inspect_scope'?{status:'inspected',cursor}:o.bootstrap.type==='inspect_result'?{status:'not_found',cursor}:{status:'completed',cursor,text:'Skewed but done',finishedAt:Date.now()});
 (f.connector as any).options.retryMs=10;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);await f.connector.publishLocalBot('owner',f.input);
 const original=f.mock.service.hostMessages.bind(f.mock.service);let refused=0;
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='completeTask'&&!refused++)throw new TeamsApiError('TASK_EXPIRED',409);return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>f.mock.tasks[0].state==='completed');await f.connector.idle();
 assert.equal(refused,2);assert.equal(completions(f).length,1);assert.equal(completions(f)[0].payload.text,'Skewed but done');assert.equal(completions(f)[0].payload.outcome,'completed');
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.ok(!events.some(([event])=>event==='team_task_delivery_rejected'));
});
test('a refused delivery is dropped only once the deadline plus the skew margin has passed',async t=>{
 for(const [code,status] of [['DEPLOYMENT_REQUIRED',403],['MEMBERSHIP_REQUIRED',403],['STALE_MEMBERSHIP',403],['TASK_EXPIRED',409]] as const){
  const events:Array<[string,any]>=[];let clock=Date.now();const f=fixture(t);(f.connector as any).options.now=()=>clock;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
  await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const deadline=clock+60000;await seedDelivery(f,deadline);const attempts=refuseCompletions(f,code,status);
  clock=deadline+DELIVERY_SKEW_MS;await wakeAndIdle(f);assert.ok(attempts.count>=1);assert.deepEqual(await pendingFiles(f),[1,1],code);assert.ok(!events.some(([event])=>event==='team_task_delivery_rejected'));
  clock=deadline+DELIVERY_SKEW_MS+1;await wakeAndIdle(f);
  assert.deepEqual(await pendingFiles(f),[0,0],code);assert.deepEqual(events.filter(([event])=>event==='team_task_delivery_rejected'),[['team_task_delivery_rejected',{taskId:'task00001',code}]]);
  f.connector.close();
 }
});
test('DEPLOYMENT_REQUIRED past the margin drops the delivery only once the binding activation is confirmed current',async t=>{
 const events:Array<[string,any]>=[];let clock=Date.now();const f=fixture(t);(f.connector as any).options.now=()=>clock;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
 await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const deadline=clock+60000;await seedDelivery(f,deadline);const attempts=refuseCompletions(f,'DEPLOYMENT_REQUIRED',403);
 // A lost activation response: the local binding keeps the old epoch and the replay cannot get through yet.
 (f.connector as any).activated.clear();const hostMessages=f.mock.service.hostMessages;
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='activateDeployment')throw new TeamsApiError('SERVICE_UNAVAILABLE',503);return hostMessages(c,command);};
 clock=deadline+DELIVERY_SKEW_MS+1;await wakeAndIdle(f);assert.ok(attempts.count>=1);assert.deepEqual(await pendingFiles(f),[1,1]);assert.ok(!events.some(([event])=>event==='team_task_delivery_rejected'));
 f.mock.service.hostMessages=hostMessages;await wakeAndIdle(f);
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.deepEqual(events.filter(([event])=>event==='team_task_delivery_rejected'),[['team_task_delivery_rejected',{taskId:'task00001',code:'DEPLOYMENT_REQUIRED'}]]);
});
test('DEPLOYMENT_REQUIRED past the margin drops the delivery of a paused Bot, whose deployment is never activated again',async t=>{
 const events:Array<[string,any]>=[];let clock=Date.now();const f=fixture(t);(f.connector as any).options.now=()=>clock;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
 await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const deadline=clock+60000;await seedDelivery(f,deadline);const attempts=refuseCompletions(f,'DEPLOYMENT_REQUIRED',403);
 // After a restart nothing is activated, and discovery never activates a paused card.
 (f.connector as any).activated.clear();const hostMessages=f.mock.service.hostMessages;
 f.mock.service.hostMessages=async(c,command)=>{const result:any=await hostMessages(c,command);return (command.action==='listSharedBots'?{bots:result.bots.map((bot:any)=>({...bot,state:'paused'}))}:result) as any;};
 const activations=f.mock.commands.filter(c=>c.action==='activateDeployment').length;
 clock=deadline+DELIVERY_SKEW_MS;await wakeAndIdle(f);assert.ok(attempts.count>=1);assert.deepEqual(await pendingFiles(f),[1,1]);
 clock=deadline+DELIVERY_SKEW_MS+1;await wakeAndIdle(f);
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.deepEqual(events.filter(([event])=>event==='team_task_delivery_rejected'),[['team_task_delivery_rejected',{taskId:'task00001',code:'DEPLOYMENT_REQUIRED'}]]);
 assert.equal(f.mock.commands.filter(c=>c.action==='activateDeployment').length,activations);
});
test('a task deferred again and again for the same reason is logged once',async t=>{
 const events:Array<[string,any]>=[];const f=fixture(t,async o=>o.bootstrap.type==='inspect_scope'?{status:'inspected',cursor:{...cursor,scope:o.bootstrap.scope,sessionRevision:1,lastMessageSeq:200,lastChangeSeq:200}}:{status:'completed',cursor,text:'Unexpected',finishedAt:Date.now()});
 (f.connector as any).options.retryMs=10;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);await f.connector.publishLocalBot('owner',f.input);
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>f.mock.commands.filter(c=>c.action==='prepareTask').length>=3);await f.connector.idle();
 assert.deepEqual(events.filter(([event])=>event==='team_task_deferred'),[['team_task_deferred',{taskId:'task00001',code:'TASK_INPUT_ALREADY_COMMITTED'}]]);
});
test('an always-permanent refusal drops the delivery before the deadline',async t=>{
 const f=fixture(t);await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();await seedDelivery(f,Date.now()+60000);refuseCompletions(f,'TASK_REVOKED',403);
 await wakeAndIdle(f);assert.deepEqual(await pendingFiles(f),[0,0]);
});
test('a transient server error never drops a delivery, even long past the deadline',async t=>{
 const f=fixture(t);let clock=Date.now();(f.connector as any).options.now=()=>clock;await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();await seedDelivery(f,clock+1000);const attempts=refuseCompletions(f,'SERVICE_UNAVAILABLE',503);
 clock+=DELIVERY_SKEW_MS*10;await wakeAndIdle(f);assert.ok(attempts.count>=1);assert.deepEqual(await pendingFiles(f),[1,1]);
});
test('a delivery whose binding is gone after a republish is dropped only after the deadline plus the margin',async t=>{
 const events:Array<[string,any]>=[];let clock=Date.now();const f=fixture(t);(f.connector as any).options.now=()=>clock;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
 await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const deadline=clock+60000;await seedDelivery(f,deadline,'olddeployment01');
 clock=deadline+DELIVERY_SKEW_MS;await wakeAndIdle(f);assert.deepEqual(await pendingFiles(f),[1,1]);assert.equal(completions(f).length,0);
 clock=deadline+DELIVERY_SKEW_MS+1;await wakeAndIdle(f);assert.deepEqual(await pendingFiles(f),[0,0]);assert.equal(completions(f).length,0);
 assert.deepEqual(events.filter(([event])=>event==='team_task_delivery_rejected'),[['team_task_delivery_rejected',{taskId:'task00001',code:'BINDING_MISSING'}]]);
});
test('a saved delivery without a deadline is rejected by the reader',async t=>{
 const f=fixture(t);await f.privateStore.saveDelivery('owner',{schemaVersion:1,teamId,generation:1,taskId:'task00001',commandId:'delivery001',payload:{}} as any);
 await assert.rejects(f.privateStore.deliveries('owner'),/UNSAFE_TEAM_STORAGE/);
});
test('a revoke while startTask is pending still saves the execution and delivers a failed result',async t=>{
 let modelRuns=0;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};modelRuns++;return {status:'completed',cursor,text:'Must not run',finishedAt:Date.now()};});
 (f.connector as any).options.retryMs=10;await f.connector.publishLocalBot('owner',f.input);
 const original=f.mock.service.hostMessages.bind(f.mock.service);let entered=false,release=()=>{};const gate=new Promise<void>(resolve=>{release=resolve;});
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='startTask'){entered=true;await gate;}return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>entered);f.connector.revoke();release();
 await settle(()=>completions(f).length===1);await f.connector.idle();
 assert.equal(modelRuns,0);assert.equal(completions(f)[0].payload.outcome,'failed');assert.equal(completions(f)[0].payload.permitId,'permit00001');assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,1);assert.deepEqual(await pendingFiles(f),[0,0]);
});
test('an account change while startTask is pending keeps the permit in the sending account and delivers its failed result when that account returns',async t=>{
 let modelRuns=0;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};modelRuns++;return {status:'completed',cursor,text:'Must not run',finishedAt:Date.now()};});
 (f.connector as any).options.retryMs=10;await f.connector.publishLocalBot('owner',f.input);
 const original=f.mock.service.hostMessages.bind(f.mock.service);let entered=false,release=()=>{};const gate=new Promise<void>(resolve=>{release=resolve;});
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='startTask'){const result=await original(c,command);entered=true;await gate;return result;}return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>entered);
 f.mock.switch();f.connector.revoke();release();
 // The other account is current: nothing runs, nothing is reported with the stale credentials, and nothing lands in the other account's store.
 await new Promise(r=>setTimeout(r,80));await f.connector.idle();
 assert.equal(modelRuns,0);assert.equal(completions(f).length,0);assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,1);
 assert.deepEqual([(await f.privateStore.deliveries('other')).length,(await f.privateStore.executions('other')).length],[0,0]);
 const [delivery]=await f.privateStore.deliveries('owner');assert.equal(delivery!.payload.outcome,'failed');assert.equal(delivery!.payload.permitId,'permit00001');assert.equal((await f.privateStore.executions('owner')).length,1);
 // The sending account signs back in: the failed result ends the task without another start or a model run.
 f.mock.switch('owner',2);await settle(()=>completions(f).length===1);await f.connector.idle();
 assert.equal(completions(f)[0].payload.outcome,'failed');assert.equal(completions(f)[0].payload.permitId,'permit00001');assert.equal(modelRuns,0);assert.equal(f.mock.commands.filter(c=>c.action==='startTask').length,1);
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.deepEqual([(await f.privateStore.deliveries('other')).length,(await f.privateStore.executions('other')).length],[0,0]);
});
test('an account change while startTask is pending and denied writes nothing and does not defer the task for the returning account',async t=>{
 const f=fixture(t);(f.connector as any).options.retryMs=60000;await f.connector.publishLocalBot('owner',f.input);
 const original=f.mock.service.hostMessages.bind(f.mock.service);let entered=false,release=()=>{},starts=0;const gate=new Promise<void>(resolve=>{release=resolve;});
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='startTask'){starts++;if(starts===1){entered=true;await gate;return {execute:false,taskId:'task00001'} as any;}}return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>entered);
 f.mock.switch();f.connector.revoke();release();await new Promise(r=>setTimeout(r,40));await f.connector.idle();
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.deepEqual([(await f.privateStore.deliveries('other')).length,(await f.privateStore.executions('other')).length],[0,0]);
 f.mock.switch('owner',2);await settle(()=>completions(f).length===1);assert.equal(starts,2);
});
test('the service hands a start permit back after an account reset mid-request, and discards every other late answer',async t=>{
 let uid='owner',epoch=0,notify=()=>{};const service=new TeamsService({snapshot:()=>({user:{uid}}),sessionEpoch:()=>epoch,getIdToken:async()=>'token',onChanged:cb=>{notify=cb;return()=>{};}},{});t.after(()=>service.close());
 (service as any).connect=async()=>({db:{}});(service as any).sdk={collection:()=>({}),onSnapshot:()=>()=>{}};
 const context:TeamAccountContext={uid:'owner',epoch:(service as any).epoch};const real=globalThis.fetch;t.after(()=>{globalThis.fetch=real;});
 const answer=(body:unknown)=>{globalThis.fetch=(async()=>{uid='other';epoch++;notify();return new Response(JSON.stringify(body),{status:200});}) as typeof fetch;};
 answer({execute:true,permitId:'permit00001'});
 assert.deepEqual(await service.hostMessages(context,{schemaVersion:1,commandId:'command001',teamId,generation:1,action:'startTask',payload:{}}),{execute:true,permitId:'permit00001'});
 uid='owner';epoch++;notify();const second:TeamAccountContext={uid:'owner',epoch:(service as any).epoch};answer({tasks:[]});
 await assert.rejects(service.hostMessages(second,{schemaVersion:1,commandId:'command002',teamId,generation:1,action:'prepareTask',payload:{}}),/INVALID_SESSION/);
 // A continue permit is granted once, like a start permit; a late answer to a question is discarded like any other.
 uid='owner';epoch++;notify();const third:TeamAccountContext={uid:'owner',epoch:(service as any).epoch};answer({execute:true,permitId:'permit00002',epoch:2});
 assert.deepEqual(await service.hostMessages(third,{schemaVersion:1,commandId:'command003',teamId,generation:1,action:'continueTask',payload:{}}),{execute:true,permitId:'permit00002',epoch:2});
 uid='owner';epoch++;notify();const fourth:TeamAccountContext={uid:'owner',epoch:(service as any).epoch};answer({state:'waiting'});
 await assert.rejects(service.hostMessages(fourth,{schemaVersion:1,commandId:'command004',teamId,generation:1,action:'pauseTask',payload:{}}),/INVALID_SESSION/);
});
test('a Bot edit stops only the shared-Bot runs bound to that Bot',async t=>{
 const runs:TeamHostProcessOptions[]=[];const f=fixture(t,hold(runs));(f.connector as any).options.retryMs=10;
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await settle(()=>runs.length===1);
 const activations=f.mock.commands.filter(c=>c.action==='activateDeployment').length;
 f.connector.botChanged('unrelatedBot01');await f.connector.idle();await new Promise(r=>setTimeout(r,30));
 assert.equal(runs[0]!.signal.aborted,false);assert.equal(f.mock.commands.filter(c=>c.action==='activateDeployment').length,activations);
 f.connector.botChanged(f.bot.bot_id);await settle(()=>completions(f).length===1);assert.equal(runs[0]!.signal.aborted,true);assert.equal(completions(f)[0].payload.outcome,'failed');
});
test('validateTask polling keeps the run through transient errors and aborts it on a definitive refusal',async t=>{
 const runs:TeamHostProcessOptions[]=[];const f=fixture(t,hold(runs));(f.connector as any).options.retryMs=10;await f.connector.publishLocalBot('owner',f.input);
 const original=f.mock.service.hostMessages.bind(f.mock.service);let validations=0,failure:Error|undefined=new TeamsApiError('SERVICE_UNAVAILABLE',503);
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='validateTask'){validations++;if(failure)throw failure;}return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>runs.length===1);
 const until=async(check:()=>boolean)=>{for(let i=0;i<120&&!check();i++)await new Promise(r=>setTimeout(r,50));assert.ok(check(),'condition settled');};
 await until(()=>validations>=1);assert.equal(runs[0]!.signal.aborted,false);
 failure=new Error('network down');const seen=validations;await until(()=>validations>seen);assert.equal(runs[0]!.signal.aborted,false);
 failure=new TeamsApiError('DEPLOYMENT_REQUIRED',403);await until(()=>runs[0]!.signal.aborted);
 await settle(()=>completions(f).length===1);assert.equal(completions(f)[0].payload.outcome,'failed');
});

// A small model of the server's start and complete checks (apps/firebase/src/shared-bots.mjs): TASK_EXPIRED outside [startedAt, min(deadline, now)], one stored result digest, command receipts only for successes.
const serverModel=(f:ReturnType<typeof fixture>,server:()=>number)=>{
 const original=f.mock.service.hostMessages.bind(f.mock.service),state={startedAt:0,resultDigest:undefined as string|undefined,receipts:new Map<string,string>(),attempts:[] as Array<{commandId:string;finishedAt:number;outcome:string}>};
 f.mock.service.hostMessages=async(c,command)=>{
  if(command.action==='startTask'){const result:any=await original(c,command);state.startedAt=server();return result;}
  if(command.action==='completeTask'){const p=command.payload as any,{accountEpoch:_epoch,...stable}=p,requestDigest=digest(JSON.stringify(command.payload)),resultDigest=digest(JSON.stringify(stable)),task=f.mock.tasks[0];
   const record=(outcome:string)=>state.attempts.push({commandId:command.commandId as string,finishedAt:p.finishedAt,outcome});
   const receipt=state.receipts.get(command.commandId as string);if(receipt&&receipt!==requestDigest){record('COMMAND_ID_CONFLICT');throw new TeamsApiError('COMMAND_ID_CONFLICT',409);}
   if(state.resultDigest){if(state.resultDigest!==resultDigest){record('TASK_RESULT_CONFLICT');throw new TeamsApiError('TASK_RESULT_CONFLICT',409);}record('replayed');return {taskId:task.taskId} as any;}
   if(p.finishedAt<state.startedAt||p.finishedAt>task.deadline||p.finishedAt>server()){record('TASK_EXPIRED');throw new TeamsApiError('TASK_EXPIRED',409);}
   state.resultDigest=resultDigest;state.receipts.set(command.commandId as string,requestDigest);record('accepted');return original(c,command);}
  return original(c,command);};
 return state;
};
const crash:(o:TeamHostProcessOptions)=>Promise<TeamHostResult>=async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};throw Error('worker crashed');};

test('a host clock behind the server still delivers a synthesized failure once it catches up, never past one accepted result',async t=>{
 let server=Date.now();const f=fixture(t,crash);(f.connector as any).options.now=()=>server-1000;
 await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const model=serverModel(f,()=>server);
 f.mock.tasks.push({...queued(),deadline:server+300000});f.mock.wake();await settle(()=>model.attempts.length>=1);await f.connector.idle();
 assert.deepEqual(model.attempts.map(a=>a.outcome),['TASK_EXPIRED']);assert.deepEqual(await pendingFiles(f),[1,1]);
 server+=500;await wakeAndIdle(f);assert.deepEqual(await pendingFiles(f),[1,1]);assert.ok(model.attempts.length>=2);
 server+=600;await wakeAndIdle(f);
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.equal(model.attempts.filter(a=>a.outcome==='accepted').length,1);assert.ok(!model.attempts.some(a=>a.outcome==='TASK_RESULT_CONFLICT'||a.outcome==='COMMAND_ID_CONFLICT'));
 const last=completions(f).at(-1);assert.equal(last.payload.outcome,'failed');assert.ok(last.payload.finishedAt>=model.startedAt&&last.payload.finishedAt<=server);
});
test('a restamped delivery whose earlier send succeeded unseen ends as delivered, not as a second result',async t=>{
 let server=Date.now();const f=fixture(t,crash);(f.connector as any).options.now=()=>server-1000;
 await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const model=serverModel(f,()=>server);
 f.mock.tasks.push({...queued(),deadline:server+300000});f.mock.wake();await settle(()=>model.attempts.length>=1);await f.connector.idle();
 // The next accepted send loses its answer.
 const sent=f.mock.service.hostMessages.bind(f.mock.service);let lost=false;
 f.mock.service.hostMessages=async(c,command)=>{const result=await sent(c,command);if(command.action==='completeTask'&&!lost){lost=true;throw new TeamsApiError('SERVICE_UNAVAILABLE',503);}return result;};
 server+=1100;await wakeAndIdle(f);assert.ok(lost);assert.deepEqual(await pendingFiles(f),[1,1]);server+=100;await wakeAndIdle(f);
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.equal(model.attempts.filter(a=>a.outcome==='accepted').length,1);
});
test('a real worker result is never restamped, whatever the server answers',async t=>{
 let server=Date.now();const finishedAt=server-1000;const f=fixture(t,async o=>o.bootstrap.type==='inspect_scope'?{status:'inspected',cursor}:o.bootstrap.type==='inspect_result'?{status:'not_found',cursor}:{status:'failed',cursor,text:'Worker failed',finishedAt});
 (f.connector as any).options.now=()=>server-1000;await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const model=serverModel(f,()=>server);
 f.mock.tasks.push({...queued(),deadline:server+300000});f.mock.wake();await settle(()=>model.attempts.length>=1);await f.connector.idle();
 for(const step of [500,600,1000]){server+=step;await wakeAndIdle(f);}
 assert.ok(model.attempts.length>=3);assert.ok(model.attempts.every(a=>a.finishedAt===finishedAt&&a.outcome==='TASK_EXPIRED'));assert.equal(new Set(model.attempts.map(a=>a.commandId)).size,1);assert.deepEqual(await pendingFiles(f),[1,1]);
});
test('a server 401 past the deadline margin keeps the delivery',async t=>{
 const events:Array<[string,any]>=[];let clock=Date.now();const f=fixture(t);(f.connector as any).options.now=()=>clock;(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
 await f.connector.publishLocalBot('owner',f.input);await f.connector.idle();const deadline=clock+60000;await seedDelivery(f,deadline);const attempts=refuseCompletions(f,'INVALID_SESSION',401);
 clock=deadline+DELIVERY_SKEW_MS+1;await wakeAndIdle(f);await wakeAndIdle(f);assert.ok(attempts.count>=1);assert.deepEqual(await pendingFiles(f),[1,1]);assert.ok(!events.some(([event])=>event==='team_task_delivery_rejected'));
});
test('a transient 401 on discovery leaves a running task alone, a definitive refusal stops it',async t=>{
 const runs:TeamHostProcessOptions[]=[];const f=fixture(t,hold(runs));await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await settle(()=>runs.length===1);
 const original=f.mock.service.hostMessages.bind(f.mock.service);let failure:TeamsApiError|undefined=new TeamsApiError('INVALID_SESSION',401);
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='listSharedBots'&&failure)throw failure;return original(c,command);};
 await wakeAndIdle(f);assert.equal(runs[0]!.signal.aborted,false);
 failure=new TeamsApiError('MEMBERSHIP_REQUIRED',403);await wakeAndIdle(f);assert.equal(runs[0]!.signal.aborted,true);
});
// D24: a friendship's team (`f_` + 40 hex) is never in `list()`, and the server decides whether the friendship still stands.
const pairTeam=`f_${'ab12'.repeat(10)}`;
const sentTo=(f:ReturnType<typeof fixture>,action:string,team:string)=>f.mock.commands.filter(c=>c.action===action&&c.teamId===team);
// The friendship is over for the server: every command of the team is answered 403 MEMBERSHIP_REQUIRED. The actions that reach it are returned.
const endFriendship=(f:ReturnType<typeof fixture>,team=pairTeam)=>{const original=f.mock.service.hostMessages.bind(f.mock.service),reached:string[]=[];
 f.mock.service.hostMessages=async(c,command)=>{if(command.teamId===team){reached.push(command.action as string);throw new TeamsApiError('MEMBERSHIP_REQUIRED',403);}return original(c,command);};return reached;};
test('a friendship team is discovered although list() never has it; an ordinary team missing from list() is still skipped',async t=>{
 const pair=fixture(t),ordinary=fixture(t);
 await pair.connector.publishLocalBot('owner',{...pair.input,teamId:pairTeam});pair.connector.revoke();await settle(()=>sentTo(pair,'listHostTasks',pairTeam).length>=1);await pair.connector.idle();
 for(const action of ['listSharedBots','activateDeployment','heartbeatDeployment','listHostTasks'])assert.ok(sentTo(pair,action,pairTeam).length>=1,action);
 assert.ok(pair.mock.commands.every(c=>c.generation===1&&c.teamId===pairTeam));
 // The fake's list() answers for team00001 only.
 await ordinary.connector.publishLocalBot('owner',{...ordinary.input,teamId:'team00002'});const before=ordinary.mock.commands.length;let listed=0;const list=ordinary.mock.service.list;ordinary.mock.service.list=async uid=>{listed++;return list(uid);};
 ordinary.connector.revoke();await ordinary.connector.idle();assert.ok(listed>=1);assert.equal(ordinary.mock.commands.length,before);
});
test('a friendship team that answers MEMBERSHIP_REQUIRED stops its binding and is sent nothing more until revoke(), which asks again',async t=>{
 const runs:TeamHostProcessOptions[]=[],events:Array<[string,any]>=[];const f=fixture(t,hold(runs));(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
 await f.connector.publishLocalBot('owner',{...f.input,teamId:pairTeam});f.mock.tasks.push(queued());f.mock.wake();await settle(()=>runs.length===1);const stored=await f.privateStore.bindings('owner');
 const reached=endFriendship(f);await wakeAndIdle(f);await settle(()=>runs[0]!.signal.aborted);await new Promise(r=>setTimeout(r,60));await f.connector.idle();
 assert.ok(reached.length>=1);assert.ok(events.some(([event,d])=>event==='team_task_aborted'&&d.reason==='BINDING_STOPPED'));
 const calls=reached.length;for(let pass=0;pass<3;pass++)await wakeAndIdle(f);f.mock.flicker();await f.connector.idle();assert.equal(reached.length,calls);
 assert.deepEqual(await f.privateStore.bindings('owner'),stored,'the binding stays, as for a team the person left');
 f.connector.revoke();await settle(()=>reached.length>calls);
});
test('a result waiting for a finished friendship is dropped at once, not retried until its deadline',async t=>{
 const events:Array<[string,any]>=[];const f=fixture(t);(f.connector as any).options.diagnostics=(event:string,details:any)=>events.push([event,details]);
 await f.connector.publishLocalBot('owner',{...f.input,teamId:pairTeam});await f.connector.idle();await seedDelivery(f,Date.now()+60000,undefined,pairTeam);
 const reached=endFriendship(f);await wakeAndIdle(f);
 assert.deepEqual(await pendingFiles(f),[0,0]);assert.deepEqual(events.filter(([event])=>event==='team_task_delivery_rejected'),[['team_task_delivery_rejected',{taskId:'task00001',code:'MEMBERSHIP_REQUIRED'}]]);
 assert.ok(!reached.includes('completeTask'),'the team was already known to be over');
});
test('a pending publication of a finished friendship is not replayed on every pass',async t=>{
 const f=fixture(t),original=f.mock.service.hostMessages.bind(f.mock.service);
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='publishSharedBot')throw new TeamsApiError('SERVICE_UNAVAILABLE',503);return original(c,command);};
 await assert.rejects(f.connector.publishLocalBot('owner',{...f.input,teamId:pairTeam}),/SERVICE_UNAVAILABLE/);assert.equal((await f.privateStore.publications('owner')).length,1);
 f.mock.service.hostMessages=original;const reached=endFriendship(f);await wakeAndIdle(f);assert.deepEqual(reached,['listSharedBots']);
 for(let pass=0;pass<3;pass++)await wakeAndIdle(f);assert.deepEqual(reached,['listSharedBots']);
});
test('a worker frame survives one transient failure of authorize or read_chat, and the task completes',async t=>{
 let frames=0;const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};
  await o.authorize!();await o.readChat!({kind:'range',from_seq_exclusive:0,to_seq_inclusive:1});frames++;return {status:'completed',cursor,text:'Done after retries',finishedAt:Date.now()};});
 (f.connector as any).options.retryMs=10;await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service),read=f.mock.service.hostReadChat.bind(f.mock.service);
 let validations=0,reads=0;f.mock.service.hostMessages=async(c,command)=>{if(command.action==='validateTask'&&!validations++)throw new TeamsApiError('SERVICE_UNAVAILABLE',503);return original(c,command);};
 f.mock.service.hostReadChat=async(c,q)=>{if(q.mode==='range'&&!reads++)throw new TeamsApiError('SERVICE_UNAVAILABLE',503);return read(c,q);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>completions(f).length===1);
 assert.equal(frames,1);assert.equal(reads,2);assert.ok(validations>=3);assert.equal(completions(f)[0].payload.outcome,'completed');assert.equal(completions(f)[0].payload.text,'Done after retries');
});
test('a worker frame fails at once on a definitive refusal and gives up after a bounded number of transient ones',async t=>{
 for(const [error,calls] of [[new TeamsApiError('TASK_REVOKED',403),1],[new TeamsApiError('SERVICE_UNAVAILABLE',503),3]] as const){
  const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};await o.authorize!();return {status:'completed',cursor,text:'Unexpected',finishedAt:Date.now()};});
  (f.connector as any).options.retryMs=10;await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service);let validations=0;
  f.mock.service.hostMessages=async(c,command)=>{if(command.action==='validateTask'){validations++;throw error;}return original(c,command);};
  f.mock.tasks.push(queued());f.mock.wake();await settle(()=>completions(f).length===1);
  assert.equal(validations,calls,error.code);assert.equal(completions(f)[0].payload.outcome,'failed');f.connector.close();
 }
});
test('a worker frame stops waiting to retry as soon as the run is aborted',async t=>{
 let reached=false,outcome:string|undefined;const runs:TeamHostProcessOptions[]=[];
 const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};runs.push(o);reached=true;
  try{await o.authorize!();}catch(error){outcome=error instanceof Error?error.message:String(error);}throw Error('INVALID_SESSION');});
 (f.connector as any).options.retryMs=60000;await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service);let validations=0;
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='validateTask'){validations++;throw new TeamsApiError('SERVICE_UNAVAILABLE',503);}return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>validations===1);const began=Date.now();f.connector.botChanged(f.bot.bot_id);await settle(()=>outcome!==undefined);
 assert.ok(reached);assert.ok(Date.now()-began<1000);assert.equal(validations,1);
});

// M2: questions, answers, recovery and cancellation.
const chatRoute=():TeamModelRoute=>({...route(),model_provider:'deepseek',model_protocol:'open_ai_chat',model_endpoint:'https://api.deepseek.com',credential_id:'deepseek',model_profile:{profile_name:'deepseek'}});
const checkpointDigest='c'.repeat(64),waitId=`wait_${'a'.repeat(40)}`,theWait={waitId,logicalStepId:'task00001/t0/c0',question:'Which city?',requestDigest:'d'.repeat(64)};
const checkpoint=(fields:Record<string,unknown>={})=>({status:'task_checkpoint' as const,taskId:'task00001',phase:'in_flight' as const,attempts:1,dispatchesObserved:1,continuable:true,checkpointDigest,...fields});
const interrupted=()=>{throw new Error('TEAM_HOST_INTERRUPTED');};
type Run=(o:TeamHostProcessOptions)=>Promise<TeamHostResult>;
/** A Bot on a route the runtime can continue, and the server side of pause/continue. */
function durable(t:any,worker:Run){
 const launches:TeamHostProcessOptions[]=[];
 const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};if(o.bootstrap.type==='inspect_result')return {status:'not_found',cursor};if(o.bootstrap.type!=='inspect_task')launches.push(o);return worker(o);});
 Object.assign((f.connector as any).options,{retryMs:10,modelRoute:async()=>({route:chatRoute(),profiles:[chatRoute().model_profile],assertCurrent:async()=>{}})});
 const original=f.mock.service.hostMessages.bind(f.mock.service),failures:Record<string,Error[]>={};
 f.mock.service.hostMessages=async(c,command)=>{const p=command.payload as any,task=f.mock.tasks.find(t=>t.taskId===p?.taskId);
  if(command.action!=='pauseTask'&&command.action!=='continueTask'){const failure=failures[command.action as string]?.shift();if(failure)throw failure;return original(c,command);}
  f.mock.service.assertHostContext(c);f.mock.commands.push(command);const failure=failures[command.action]?.shift();if(failure)throw failure;
  if(command.action==='pauseTask'){task.state='waiting';return {taskId:p.taskId,state:'waiting',waitId:p.waitId,messageId:'bot_question01',expiresAt:Date.now()+p.expiresInMs} as any;}
  task.state='running';delete task.resume;task.epoch=p.expectedEpoch+1;
  return {execute:true,permitId:`permit${String(task.epoch).padStart(5,'0')}`,epoch:task.epoch,task:{...task,deadline:Date.now()+300000},physicalDispatchesSpent:p.dispatchedRequests} as any;};
 const read=f.mock.service.hostReadChat.bind(f.mock.service);
 f.mock.service.hostReadChat=async(c,request)=>request.mode==='byId'&&request.messageId==='answer000001'?{messages:[{messageId:'answer000001',seq:3,changeSeq:3,changeId:'change-answer1',author:{kind:'user' as const,uid:'member',displayName:'Member'},createdAt:1,text:'Lisbon',waitId,sourceTaskId:'task00001',replyTo:'bot_question01'}],lastSeq:3,lastChangeSeq:3,hasMore:false}:read(c,request);
 const sent=(action:string)=>f.mock.commands.filter(c=>c.action===action);
 return {...f,launches,failures,sent,answer:()=>{const task=f.mock.tasks.find(t=>t.taskId==='task00001');Object.assign(task,{state:'queued',resume:{waitId,answerMessageId:'answer000001',answerSeq:3}});f.mock.wake();}};
}
const asked=()=>({status:'waiting' as const,cursor,wait:theWait,checkpointDigest,pausedAt:Date.now(),dispatchedRequests:1,attemptDispatchedRequests:1,deniedTools:0,usage:{status:'partial' as const,inputTokens:10,outputTokens:5}});
const done=(text='Planned for Lisbon')=>({status:'completed' as const,cursor,text,finishedAt:Date.now(),dispatchedRequests:2,attemptDispatchedRequests:1,deniedTools:0,usage:{status:'complete' as const,inputTokens:7,outputTokens:3}});
/** One completion was sent and the run that sent it has finished cleaning up. */
const finished=async(f:{mock:{commands:any[]};connector:TeamConnector})=>{await settle(()=>completions(f).length===1&&(f.connector as any).active.size===0);await f.connector.idle();};
const quiet=async(f:{mock:{wake():void};connector:TeamConnector})=>{for(let i=0;i<4;i++){f.mock.wake();await new Promise(r=>setTimeout(r,25));await f.connector.idle();}};

test('a Bot question pauses the task and frees its slot; the answer continues it from the checkpoint with the same input',async t=>{
 const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({wait:theWait}):o.bootstrap.type==='run'?asked():done());
 await f.connector.publishLocalBot('owner',f.input);assert.equal(f.sent('publishSharedBot')[0].payload.deployment.capabilities.durableWait,true);
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>f.sent('pauseTask').length===1);await f.connector.idle();
 const b=(await f.privateStore.bindings('owner'))[0]!,pause=f.sent('pauseTask')[0].payload,first=f.launches[0]!.bootstrap;
 assert.equal(first.type,'run');assert.equal(first.askRequester,true);assert.equal(first.continuation,undefined);
 assert.deepEqual({...pause,pausedAt:0},{deploymentId:b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',permitId:'permit00001',inputGrantIds:b.grantIds,waitId,kind:'human_input',
  question:'Which city?',requestDigest:theWait.requestDigest,checkpointDigest,pausedAt:0,expiresInMs:86400000,dispatchedRequests:1,outputAudience:{mode:'published_inputs'}});
 // Waiting holds no worker and no slot, and nothing treats it as a lost run.
 await quiet(f);assert.equal((f.connector as any).active.size,0);assert.equal(f.launches.length,1);assert.equal(f.sent('pauseTask').length,1);assert.equal(completions(f).length,0);assert.equal(f.sent('continueTask').length,0);
 const waiting=(await f.privateStore.executions('owner'))[0]!;assert.equal(waiting.wait?.waitId,waitId);assert.equal(waiting.pause,undefined);assert.ok(existsSync(waiting.batch.path));
 f.answer();await finished(f);
 assert.deepEqual(f.sent('continueTask').map(c=>c.payload),[{deploymentId:b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',expectedEpoch:1,checkpointDigest,reason:'answer',dispatchedRequests:1}]);
 const second=f.launches[1]!.bootstrap;assert.equal(f.launches.length,2);assert.equal(second.type,'continue');assert.equal(second.askRequester,true);
 assert.deepEqual(second.continuation,{epoch:2,expected_checkpoint_digest:checkpointDigest,answer:{wait_id:waitId,message_id:'answer000001',text:'Lisbon'}});
 assert.deepEqual(second.inputBatchRef,first.inputBatchRef);assert.equal(second.limits?.physical_dispatches,31);assert.equal(second.taskId,'task00001');
 const result=completions(f)[0].payload;assert.equal(result.permitId,'permit00002');assert.equal(result.outcome,'completed');assert.equal(result.text,'Planned for Lisbon');
 assert.deepEqual(result.usage,{status:'partial',inputTokens:17,outputTokens:8});
 assert.deepEqual(await f.privateStore.executions('owner'),[]);assert.deepEqual(await f.privateStore.deliveries('owner'),[]);assert.ok(!existsSync(waiting.batch.path));
});
test('a Bot on a route the runtime cannot continue is never offered the question tool',async t=>{
 const runs:TeamHostProcessOptions[]=[];const f=fixture(t,async o=>{if(o.bootstrap.type==='inspect_scope')return {status:'inspected',cursor};runs.push(o);return done();});
 await f.connector.publishLocalBot('owner',f.input);assert.equal(f.mock.commands.find(c=>c.action==='publishSharedBot').payload.deployment.capabilities.durableWait,false);
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>completions(f).length===1);assert.equal(runs[0]!.bootstrap.askRequester,undefined);
});
test('an interrupted worker is continued from its checkpoint with a new permit instead of failing the task',async t=>{
 let continued=0;const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({dispatchesObserved:continued+1}):o.bootstrap.type==='run'?interrupted():++continued===1?interrupted():done('Recovered answer'));
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await finished(f);
 assert.deepEqual(f.sent('continueTask').map(c=>[c.payload.reason,c.payload.expectedEpoch,c.payload.dispatchedRequests]),[['recovery',1,1],['recovery',2,2]]);
 assert.deepEqual(f.launches.map(o=>[o.bootstrap.type,o.bootstrap.continuation]),[['run',undefined],['continue',{epoch:2,expected_checkpoint_digest:checkpointDigest}],['continue',{epoch:3,expected_checkpoint_digest:checkpointDigest}]]);
 assert.equal(f.sent('startTask').length,1);assert.equal(completions(f).length,1);const result=completions(f)[0].payload;
 assert.equal(result.permitId,'permit00003');assert.equal(result.outcome,'completed');assert.equal(result.text,'Recovered answer');
 // The interrupted attempts reported no usage, so the total is not complete.
 assert.deepEqual(result.usage,{status:'partial',inputTokens:7,outputTokens:3});assert.deepEqual(await f.privateStore.executions('owner'),[]);
});
test('a task whose worker keeps dying, or whose checkpoint cannot be continued, ends as failed with the permit it holds',async t=>{
 const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint():interrupted());
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await finished(f);await quiet(f);
 assert.equal(f.launches.length,3);assert.equal(f.sent('continueTask').length,2);assert.equal(completions(f).length,1);
 assert.equal(completions(f)[0].payload.outcome,'failed');assert.equal(completions(f)[0].payload.permitId,'permit00003');assert.deepEqual(await f.privateStore.executions('owner'),[]);
 const g=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({continuable:false,reason:'pending_write_tool'}):interrupted());
 await g.connector.publishLocalBot('owner',g.input);g.mock.tasks.push(queued());g.mock.wake();await finished(g);
 assert.equal(g.launches.length,1);assert.equal(g.sent('continueTask').length,0);assert.equal(completions(g)[0].payload.outcome,'failed');assert.equal(completions(g)[0].payload.permitId,'permit00001');
});
test('a refused continue closes the running attempt with its own permit and never starts a worker',async t=>{
 const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint():interrupted());
 await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages;
 f.mock.service.hostMessages=async(c,command)=>{if(command.action!=='continueTask')return original(c,command);f.mock.commands.push(command);return {execute:false,taskId:'task00001'} as any;};
 f.mock.tasks.push(queued());f.mock.wake();await finished(f);await quiet(f);
 assert.equal(f.launches.length,1);assert.equal(f.sent('continueTask').length,1);assert.equal(completions(f).length,1);assert.equal(completions(f)[0].payload.permitId,'permit00001');assert.equal(completions(f)[0].payload.outcome,'failed');
});
test('a cancelled task stops at the next authorization and is closed once, without recovery',async t=>{
 const f=durable(t,async o=>{if(o.bootstrap.type==='inspect_task')return checkpoint();await o.authorize!();return done();});
 await f.connector.publishLocalBot('owner',f.input);f.failures.validateTask=[new TeamsApiError('TASK_CANCELLED',409)];
 f.mock.tasks.push(queued());f.mock.wake();await finished(f);await quiet(f);
 assert.equal(f.launches.length,1);assert.equal(f.sent('continueTask').length,0);assert.equal(completions(f).length,1);
 assert.equal(completions(f)[0].payload.outcome,'failed');assert.equal(completions(f)[0].payload.permitId,'permit00001');assert.deepEqual(await f.privateStore.executions('owner'),[]);
});
test('a question is sent again unchanged after a transient failure, and a cancelled one closes the task instead',async t=>{
 const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({wait:theWait}):asked());
 await f.connector.publishLocalBot('owner',f.input);f.failures.pauseTask=[new TeamsApiError('SERVICE_UNAVAILABLE',503),new TeamsApiError('TASK_EXPIRED',409)];
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>f.sent('pauseTask').length===3);await f.connector.idle();
 const [one,two,three]=f.sent('pauseTask');assert.deepEqual(two.payload,one.payload);assert.deepEqual(three.payload,one.payload);assert.equal(two.commandId,one.commandId);
 assert.equal(f.launches.length,1);assert.equal(completions(f).length,0);assert.equal((await f.privateStore.executions('owner'))[0]!.wait?.waitId,waitId);
 const g=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({wait:theWait}):asked());
 await g.connector.publishLocalBot('owner',g.input);g.failures.pauseTask=[new TeamsApiError('TASK_CANCELLED',409)];
 g.mock.tasks.push(queued());g.mock.wake();await finished(g);await quiet(g);
 assert.equal(g.sent('pauseTask').length,1);assert.equal(completions(g)[0].payload.outcome,'failed');assert.equal(completions(g)[0].payload.permitId,'permit00001');assert.deepEqual(await g.privateStore.executions('owner'),[]);
});
test('a host that stopped before reporting a question reports it from the checkpoint on its next start',async t=>{
 const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({wait:theWait,dispatchesObserved:4}):interrupted());
 await f.connector.publishLocalBot('owner',f.input);const b=(await f.privateStore.bindings('owner'))[0]!,started=Date.now();
 f.mock.tasks.push({...queued(),state:'running'});
 await f.privateStore.saveExecution('owner',await savedExecution(f,{scope:{teamId,sharedBotId,conversationId,sessionEpoch:b.sessionEpoch},teamId,taskId:'task00001',deadlineAt:started+60000,commandId:'delivery001',ask:true,
  payload:{deploymentId:b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',permitId:'permit0001',inputGrantIds:b.grantIds}}));
 f.mock.wake();await settle(()=>f.sent('pauseTask').length===1);await f.connector.idle();await quiet(f);
 const pause=f.sent('pauseTask')[0].payload;assert.equal(pause.waitId,waitId);assert.equal(pause.permitId,'permit0001');assert.equal(pause.dispatchedRequests,4);assert.ok(pause.pausedAt>=started&&pause.pausedAt<=Date.now());
 assert.equal(f.launches.length,0);assert.equal(f.sent('pauseTask').length,1);assert.equal(completions(f).length,0);assert.equal((await f.privateStore.executions('owner'))[0]!.wait?.waitId,waitId);
});
test('an attempt that died while continuing with an answer is recovered with the same answer',async t=>{
 let continued=0;const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({wait:theWait}):o.bootstrap.type==='run'?asked():++continued===1?interrupted():done());
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await settle(()=>f.sent('pauseTask').length===1);await f.connector.idle();
 f.answer();await finished(f);
 assert.deepEqual(f.sent('continueTask').map(c=>[c.payload.reason,c.payload.expectedEpoch]),[['answer',1],['recovery',2]]);assert.equal(f.sent('pauseTask').length,1);
 const answer={wait_id:waitId,message_id:'answer000001',text:'Lisbon'};
 assert.deepEqual(f.launches.slice(1).map(o=>o.bootstrap.continuation),[{epoch:2,expected_checkpoint_digest:checkpointDigest,answer},{epoch:3,expected_checkpoint_digest:checkpointDigest,answer}]);
 assert.equal(completions(f)[0].payload.permitId,'permit00003');
});
test('an unanswered question is forgotten after its expiry, and an answer for a wait this host does not hold starts nothing',async t=>{
 let clock=Date.now();const f=durable(t,async o=>o.bootstrap.type==='inspect_task'?checkpoint({wait:theWait}):asked());(f.connector as any).options.now=()=>clock;
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await settle(()=>f.sent('pauseTask').length===1);await f.connector.idle();
 const record=(await f.privateStore.executions('owner'))[0]!;clock=record.wait!.expiresAt+60000;await quiet(f);assert.equal((await f.privateStore.executions('owner')).length,1);
 clock=record.wait!.expiresAt+8*60000;await quiet(f);assert.deepEqual(await f.privateStore.executions('owner'),[]);assert.ok(!existsSync(record.batch.path));
 f.answer();await quiet(f);assert.equal(f.sent('continueTask').length,0);assert.equal(f.launches.length,1);assert.equal(completions(f).length,0);
});
test('a result its own run is delivering is not sent a second time by a discovery pass',async t=>{
 const f=fixture(t);(f.connector as any).options.retryMs=10;await f.connector.publishLocalBot('owner',f.input);const original=f.mock.service.hostMessages.bind(f.mock.service);
 let sends=0,release=()=>{};const gate=new Promise<void>(resolve=>{release=resolve;});
 f.mock.service.hostMessages=async(c,command)=>{if(command.action==='completeTask'){sends++;await gate;}return original(c,command);};
 f.mock.tasks.push(queued());f.mock.wake();await settle(()=>sends===1);
 for(let i=0;i<4;i++){f.mock.wake();await new Promise(r=>setTimeout(r,25));}
 assert.equal(sends,1);assert.equal((await f.privateStore.deliveries('owner')).length,1);release();await finished(f);
 assert.equal(sends,1);assert.deepEqual(await f.privateStore.deliveries('owner'),[]);
});
function frames(lines:unknown[]){const child=new EventEmitter() as ChildProcessWithoutNullStreams;Object.assign(child,{stdin:new PassThrough(),stdout:new PassThrough(),stderr:new PassThrough(),exitCode:null,kill:()=>{}});
 child.stdin.once('data',()=>{setImmediate(()=>{for(const line of lines)child.stdout.emit('data',Buffer.from(JSON.stringify(line)+'\n'));child.emit('close',0);});});return ()=>child;}
test('the private process reads a checkpoint and a waiting result only for the bootstrap that may produce them',async()=>{
 const scope={teamId,sharedBotId,conversationId,sessionEpoch:1},base={executable:'/mock',home:'/private-home',tmp:'/private-tmp',signal:new AbortController().signal,assertCurrent:()=>{}};
 const boot=(type:any,extra:Record<string,unknown>={})=>({version:3 as const,type,storageRoot:'/private-root',workerExecutable:'/worker',scope,taskId:'task00001',...extra});
 const report={version:3,type:'result',status:'task_checkpoint',taskId:'task00001',phase:'in_flight',cursor:{sessionRevision:1,lastMessageSeq:2,lastChangeSeq:2},attempts:1,dispatchesObserved:1,
  pendingToolCalls:[{logicalStepId:'task00001/t0/c0',toolName:'TeamAskRequester',readOnly:false}],continuable:true,wait:theWait,checkpointDigest};
 assert.deepEqual(await runTeamHost({...base,bootstrap:boot('inspect_task'),spawnProcess:frames([report])}),{status:'task_checkpoint',taskId:'task00001',phase:'in_flight',attempts:1,dispatchesObserved:1,continuable:true,wait:theWait,checkpointDigest});
 await assert.rejects(runTeamHost({...base,bootstrap:boot('inspect_result'),spawnProcess:frames([report])}),/TEAM_HOST_PROTOCOL/);
 await assert.rejects(runTeamHost({...base,bootstrap:boot('inspect_task',{taskId:'task00002'}),spawnProcess:frames([report])}),/TEAM_HOST_PROTOCOL/);
 await assert.rejects(runTeamHost({...base,bootstrap:boot('inspect_task'),spawnProcess:frames([{...report,wait:{...theWait,waitId:'other'}}])}),/TEAM_HOST_PROTOCOL/);
 const live={scope,sessionRevision:1,lastMessageSeq:2,lastChangeSeq:2,bootstrapFromSeq:0};
 const waiting={version:3,type:'result',status:'waiting',cursor:live,wait:theWait,checkpointDigest,pausedAt:1731300000000,dispatchedRequests:1,attemptDispatchedRequests:1,deniedTools:0,usage:{status:'missing'}};
 const paused=await runTeamHost({...base,bootstrap:boot('continue',{askRequester:true}),spawnProcess:frames([waiting])});
 assert.equal(paused.status,'waiting');assert.deepEqual((paused as any).wait,theWait);assert.equal((paused as any).checkpointDigest,checkpointDigest);assert.equal((paused as any).pausedAt,1731300000000);
 await assert.rejects(runTeamHost({...base,bootstrap:boot('run'),spawnProcess:frames([waiting])}),/TEAM_HOST_PROTOCOL/);
 await assert.rejects(runTeamHost({...base,bootstrap:boot('run',{askRequester:true}),spawnProcess:frames([{...waiting,text:'leak'}])}),/TEAM_HOST_PROTOCOL/);
 const finished={version:3,type:'result',status:'completed',cursor:live,text:'Done',finishedAt:1,dispatchedRequests:2,attemptDispatchedRequests:1,deniedTools:0,usage:{status:'missing'}};
 assert.equal(((await runTeamHost({...base,bootstrap:boot('continue'),spawnProcess:frames([finished])})) as any).attemptDispatchedRequests,1);
});

// M2: the shared list. A write is recorded in the operation ledger before it is sent.
const groceries={title:'Groceries',description:'What the household needs.'},step='task00001/t0/c1';
const stepId=(logicalStepId=step)=>digest(JSON.stringify(['task00001','task00001',logicalStepId,'store_idempotent']));
const milk={itemId:'item_0001',text:'Milk',done:false},bread={itemId:'item_0002',text:'Bread',done:false};
const addBread={expectedDataRevision:1,changes:[{op:'add',text:'Bread'}]};
/** A Bot with a shared list on a continuable route, and the server side of the list and its ledger. */
function listed(t:any,worker:Run,answers:Record<string,(payload:any)=>unknown>={}){
 const f=durable(t,worker),inner=f.mock.service.hostMessages.bind(f.mock.service);
 const replies:Record<string,(payload:any)=>unknown>={readTaskList:()=>({dataRevision:1,items:[{...milk,updatedBy:{kind:'user',uid:'member',generation:1},updatedAt:1}]}),
  recordOperation:p=>({operationId:p.operationId,state:'dispatched',dispatch:true}),writeTaskList:()=>({applied:true,dataRevision:2,items:[milk,bread]}),...answers};
 f.mock.service.hostMessages=async(c,command)=>{const result:any=await inner(c,command);
  if(command.action==='startTask'||command.action==='continueTask')return {...result,sharedList:groceries};
  return replies[command.action as string]?replies[command.action as string]!(command.payload) as any:result;};
 return {...f,input:{...f.input,sharedList:groceries}};
}
test('a Bot reads its shared list and changes it through the ledger: recorded first, then sent once under the id of its step',async t=>{
 const outcomes:any[]=[];
 const f=listed(t,async o=>{if(o.bootstrap.type==='inspect_task')return checkpoint();
  outcomes.push(await o.operate!({tool:'list_read'}),await o.operate!({tool:'list_write',logicalStepId:step,arguments:addBread}),
   await o.operate!({tool:'list_write',logicalStepId:'task00001/t1/c0',arguments:{expectedDataRevision:2,changes:[{op:'remove',itemId:'abc'}]}}));return done();});
 await f.connector.publishLocalBot('owner',f.input);const published=f.sent('publishSharedBot')[0].payload;
 assert.deepEqual(published.sharedList,groceries);assert.deepEqual(published.deployment.capabilities,{durableWait:true,writeOperations:true,delegation:false});
 f.mock.tasks.push(queued());f.mock.wake();await finished(f);
 const b=(await f.privateStore.bindings('owner'))[0]!,host={deploymentId:b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',permitId:'permit00001'};
 assert.deepEqual(f.launches[0]!.bootstrap.sharedList,groceries);
 assert.deepEqual(outcomes[0],{established:true,succeeded:true,result:{dataRevision:1,items:[milk]}});
 assert.deepEqual(outcomes[1],{established:true,succeeded:true,result:{dataRevision:2,items:[milk,bread]}});
 // What the service would refuse is never recorded: the Bot is told, and nothing is sent.
 assert.deepEqual(outcomes[2],{established:true,succeeded:false,result:{error:'INVALID_LIST_CHANGE'}});
 assert.deepEqual(f.sent('readTaskList').map(c=>c.payload),[host]);
 const records=f.sent('recordOperation'),writes=f.sent('writeTaskList');assert.equal(records.length,1);assert.equal(writes.length,1);
 assert.deepEqual({...records[0].payload,dispatchedAt:0},{...host,operationId:stepId(),logicalStepId:step,category:'store_idempotent',tool:{name:'TeamListWrite',version:'1'},
  digest:digest(JSON.stringify({changes:[{op:'add',text:'Bread'}],expectedDataRevision:1})),dispatchedAt:0});
 assert.ok(records[0].payload.dispatchedAt<=Date.now());
 assert.deepEqual(writes[0].payload,{...host,operationId:stepId(),...addBread});
 assert.ok(f.mock.commands.indexOf(records[0])<f.mock.commands.indexOf(writes[0]));
 assert.equal(completions(f)[0].payload.outcome,'completed');
});
test('a write nobody can vouch for is not answered; the continued attempt repeats the same step under the same operation id',async t=>{
 const outcomes:any[]=[];
 const f=listed(t,async o=>{if(o.bootstrap.type==='inspect_task')return checkpoint();
  const outcome=await o.operate!({tool:'list_write',logicalStepId:step,arguments:addBread});outcomes.push(outcome);if(!outcome.established)throw new TeamHostFailure('WORKER');return done('Bread is on the list');});
 await f.connector.publishLocalBot('owner',f.input);f.failures.writeTaskList=Array.from({length:3},()=>new TeamsApiError('SERVICE_UNAVAILABLE',503));
 f.mock.tasks.push(queued());f.mock.wake();await finished(f);
 assert.deepEqual(outcomes,[{established:false,succeeded:false,result:{}},{established:true,succeeded:true,result:{dataRevision:2,items:[milk,bread]}}]);
 const records=f.sent('recordOperation'),writes=f.sent('writeTaskList');
 assert.deepEqual(records.map(c=>[c.payload.operationId,c.payload.permitId]),[[stepId(),'permit00001'],[stepId(),'permit00002']]);assert.notEqual(records[0].commandId,records[1].commandId);
 assert.deepEqual(writes.map(c=>[c.payload.operationId,c.payload.permitId]),[[stepId(),'permit00002']]);
 assert.deepEqual(f.sent('continueTask').map(c=>c.payload.reason),['recovery']);
 assert.deepEqual(f.launches.map(o=>[o.bootstrap.type,o.bootstrap.sharedList]),[['run',groceries],['continue',groceries]]);
 assert.equal(completions(f)[0].payload.text,'Bread is on the list');
});
test('a write the ledger refuses is not sent: the Bot hears a refusal it can act on, a write that lost its turn is left open, a cancelled task ends',async t=>{
 const outcomes:any[]=[];let records=0;
 const f=listed(t,async o=>{if(o.bootstrap.type==='inspect_task')return checkpoint();
  for(const logicalStepId of [step,'task00001/t0/c2','task00001/t0/c3'])outcomes.push(await o.operate!({tool:'list_write',logicalStepId,arguments:addBread}));return done();},
  {recordOperation:p=>++records===1?(()=>{throw new TeamsApiError('OPERATION_LIMIT',409);})():records===2?{operationId:p.operationId,state:'dispatched',dispatch:false}:(()=>{throw new TeamsApiError('TASK_CANCELLED',409);})()});
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await finished(f);
 assert.deepEqual(outcomes,[{established:true,succeeded:false,result:{error:'OPERATION_LIMIT'}},{established:false,succeeded:false,result:{}}]);
 assert.equal(f.sent('writeTaskList').length,0);assert.equal(completions(f)[0].payload.outcome,'failed');
});
test('a list the service saved differently reaches the Bot as a refusal with the current list',async t=>{
 const outcomes:any[]=[];
 const f=listed(t,async o=>{if(o.bootstrap.type==='inspect_task')return checkpoint();outcomes.push(await o.operate!({tool:'list_write',logicalStepId:step,arguments:addBread}));return done();},
  {writeTaskList:()=>({applied:false,error:'DATA_REVISION_CONFLICT',dataRevision:4,items:[bread]})});
 await f.connector.publishLocalBot('owner',f.input);f.mock.tasks.push(queued());f.mock.wake();await finished(f);
 assert.deepEqual(outcomes,[{established:true,succeeded:false,result:{error:'DATA_REVISION_CONFLICT',dataRevision:4,items:[bread]}}]);
});
test('only a Bot on a continuable route is published with a list, and a run that was not offered the list cannot reach it',async t=>{
 const f=fixture(t);await assert.rejects(f.connector.publishLocalBot('owner',{...f.input,sharedList:groceries}),/SHARED_LIST_UNSUPPORTED/);
 for(const sharedList of [{title:' ',description:'x'},{title:'x'.repeat(121),description:'x'},{title:'x',description:''},{title:'x',description:'x',items:[]},null])
  await assert.rejects(f.connector.publishLocalBot('owner',{...f.input,sharedList} as any),/INVALID_INPUT/);
 await f.connector.publishLocalBot('owner',f.input);const published=f.mock.commands.find(c=>c.action==='publishSharedBot').payload;
 assert.equal(published.sharedList,undefined);assert.equal(published.deployment.capabilities.writeOperations,false);
 let refusal:unknown;const g=durable(t,async o=>{if(o.bootstrap.type==='inspect_task')return checkpoint();try{await o.operate!({tool:'list_read'});}catch(error){refusal=error;}return done();});
 await g.connector.publishLocalBot('owner',g.input);g.mock.tasks.push(queued());g.mock.wake();await finished(g);
 assert.equal(g.launches[0]!.bootstrap.sharedList,undefined);assert.match(String(refusal),/TEAM_HOST_PROTOCOL/);assert.equal(g.sent('readTaskList').length,0);
});
/** A host process that sends one frame, then its result once the Desktop has answered (or at once when no answer is due). */
function conversing(request:unknown,result:unknown,answered=true){const child=new EventEmitter() as ChildProcessWithoutNullStreams,written:any[]=[];
 Object.assign(child,{stdin:new PassThrough(),stdout:new PassThrough(),stderr:new PassThrough(),exitCode:null,kill:()=>{}});
 const emit=(line:unknown)=>child.stdout.emit('data',Buffer.from(JSON.stringify(line)+'\n')),end=()=>{emit(result);child.emit('close',0);};
 child.stdin.on('data',(data:Buffer)=>{for(const line of data.toString().split('\n').filter(Boolean))written.push(JSON.parse(line));
  if(written.length===1)setImmediate(()=>{emit(request);if(!answered)setTimeout(end,20);});else if(written.length===2)setImmediate(end);});
 return {spawn:()=>child,written};}
test('the private process serves list operations only to a run that was offered the list, and says when an outcome is not established',async()=>{
 const scope={teamId,sharedBotId,conversationId,sessionEpoch:1},live={scope,sessionRevision:1,lastMessageSeq:2,lastChangeSeq:2,bootstrapFromSeq:0};
 const result={version:3,type:'result',status:'completed',cursor:live,text:'Done',finishedAt:1,dispatchedRequests:2,attemptDispatchedRequests:1,deniedTools:0,usage:{status:'missing'}};
 const base={executable:'/mock',home:'/private-home',tmp:'/private-tmp',signal:new AbortController().signal,assertCurrent:()=>{}};
 const boot=(extra:Record<string,unknown>={sharedList:groceries})=>({version:3 as const,type:'run' as const,storageRoot:'/private-root',workerExecutable:'/worker',scope,taskId:'task00001',...extra});
 const write={version:3,type:'operation',id:'operation-1',tool:'list_write',logicalStepId:step,arguments:addBread},read={version:3,type:'operation',id:'operation-1',tool:'list_read',arguments:{}};
 const calls:any[]=[];let authorized=0;const authorize=async()=>{authorized++;};
 const serve=(outcome:any)=>async(operation:any)=>{calls.push(operation);return outcome;};
 let f=conversing(write,result);
 assert.equal((await runTeamHost({...base,bootstrap:boot(),authorize,operate:serve({established:true,succeeded:true,result:{dataRevision:2,items:[milk,bread]}}),spawnProcess:f.spawn})).status,'completed');
 assert.deepEqual(f.written[1],{version:3,type:'operation_result',id:'operation-1',established:true,succeeded:true,result:{dataRevision:2,items:[milk,bread]}});
 assert.deepEqual(calls,[{tool:'list_write',logicalStepId:step,arguments:addBread}]);assert.equal(authorized,1);
 f=conversing(read,result);await runTeamHost({...base,bootstrap:boot(),authorize,operate:serve({established:true,succeeded:false,result:{error:'LIST_REQUIRED'}}),spawnProcess:f.spawn});
 assert.deepEqual(f.written[1],{version:3,type:'operation_result',id:'operation-1',established:true,succeeded:false,result:{error:'LIST_REQUIRED'}});assert.deepEqual(calls[1],{tool:'list_read'});
 // An outcome that is not established carries no result and is never a success.
 f=conversing(write,result);await runTeamHost({...base,bootstrap:boot(),authorize,operate:serve({established:false,succeeded:true,result:{dataRevision:9}}),spawnProcess:f.spawn});
 assert.deepEqual(f.written[1],{version:3,type:'operation_result',id:'operation-1',established:false,succeeded:false,result:{}});
 const served=calls.length;
 for(const [bootstrap,frame,operate] of [
  [boot({}),write,serve({established:true,succeeded:true,result:{}})],[boot(),write,undefined],
  [boot(),{...write,logicalStepId:'task00002/t0/c1'},serve({established:true,succeeded:true,result:{}})],[boot(),{...write,logicalStepId:'task00001/t0'},serve({established:true,succeeded:true,result:{}})],
  [boot(),{...write,logicalStepId:undefined},serve({established:true,succeeded:true,result:{}})],[boot(),{...read,logicalStepId:step},serve({established:true,succeeded:true,result:{}})],
  [boot(),{...read,arguments:{all:true}},serve({established:true,succeeded:true,result:{}})],[boot(),{...read,tool:'list_clear'},serve({established:true,succeeded:true,result:{}})],
  [boot(),{...read,id:'read_chat-1'},serve({established:true,succeeded:true,result:{}})],[boot(),{...read,retry:true},serve({established:true,succeeded:true,result:{}})],
  [boot(),read,serve({established:true,succeeded:true,result:{items:'x'.repeat(65*1024)}})],[boot(),read,async()=>{throw new Error('SERVICE_UNAVAILABLE');}],
 ] as const)await assert.rejects(runTeamHost({...base,bootstrap:bootstrap as any,authorize,operate:operate as any,spawnProcess:conversing(JSON.parse(JSON.stringify(frame)),result,false).spawn}),/TEAM_HOST_PROTOCOL/);
 assert.equal(calls.length,served+1);
});
