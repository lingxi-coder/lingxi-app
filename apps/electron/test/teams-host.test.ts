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
 await f.privateStore.saveExecution('owner',{schemaVersion:1,scope:{teamId,sharedBotId,conversationId,sessionEpoch:b.sessionEpoch},teamId,generation:1,taskId:'task00001',deadlineAt:deadline,commandId:'delivery001',
  payload:{deploymentId:b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',permitId:'permit0001',inputGrantIds:b.grantIds}});
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

const seedDelivery=async(f:ReturnType<typeof fixture>,deadlineAt:number,deploymentId?:string)=>{
 const b=(await f.privateStore.bindings('owner'))[0]!,payload={deploymentId:deploymentId??b.deploymentId,hostId:f.privateStore.hostId,accountEpoch:b.accountEpoch,taskId:'task00001',permitId:'permit0001',inputGrantIds:b.grantIds};
 const intent={schemaVersion:1 as const,scope:{teamId,sharedBotId,conversationId,sessionEpoch:b.sessionEpoch},teamId,generation:1,taskId:'task00001',deadlineAt,commandId:'delivery001',payload};
 await f.privateStore.saveExecution('owner',intent);
 await f.privateStore.saveDelivery('owner',{...intent,payload:{...payload,outcome:'failed',text:'Failed',finishedAt:deadlineAt-1,outputAudience:{mode:'published_inputs'},usage:{status:'missing'}}});
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
