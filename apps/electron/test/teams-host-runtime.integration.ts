/** Explicit macOS process acceptance; real isolated host/worker, synthetic provider only.
 * Run with TEAM_V3_HOST_BIN, TEAM_V3_WORKER_BIN from one build.
 * Production Firebase, signed package and physical member acceptance remain separate. */
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { randomUUID, createHash } from 'node:crypto';
import { mkdirSync, mkdtempSync, realpathSync, writeFileSync, readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import type { ProviderAccountSnapshot } from '../src/shared/providerAccounts';
import { TeamPrivateStore } from '../src/main/teams/privateStore';
import { runTeamHost, type TeamHostBootstrap, type TeamHostResult } from '../src/main/teams/process';
assert.equal(process.platform,'darwin');
const companion=resolve(dirname(fileURLToPath(import.meta.url)),'../../../../harness-runtime');
const binary=(key:string,name:string)=>realpathSync(process.env[key]??join(companion,'target/debug',name));
const host=binary('TEAM_V3_HOST_BIN','harness-team-host'),worker=binary('TEAM_V3_WORKER_BIN','harness-team-worker');
const binaryHashes=()=>Object.fromEntries([['host',host],['worker',worker]].map(([name,path])=>[name,createHash('sha256').update(readFileSync(path!)).digest('hex')]));
const initialHashes=binaryHashes();
const root=realpathSync(mkdtempSync(join(tmpdir(),'shared-bot-v3-integration-')));
const directory=(name:string)=>{const path=join(root,name);mkdirSync(path,{mode:0o700});return path;};
const userData=directory('app'),home=directory('owner-home'),configHome=directory('owner-config');
writeFileSync(join(configHome,'owner-secret.txt'),'OWNER_PRIVATE_SENTINEL_DO_NOT_COPY',{mode:0o600});
const privateStore=new TeamPrivateStore(userData),paths=await privateStore.owner('fixture-owner');
const requests:any[]=[];let httpRequests=0;let headerNames:string[]=[];const model=createServer(async(req,res)=>{httpRequests++;headerNames=Object.keys(req.headers);
 try {assert.equal(req.headers['x-api-key'],'synthetic-model-key');const parts:Buffer[]=[];for await(const chunk of req)parts.push(Buffer.from(chunk));const body=JSON.parse(Buffer.concat(parts).toString('utf8'));requests.push(body);
 const text=`Fixture reply ${requests.length}`;const event=(type:string,value:unknown)=>`event: ${type}\ndata: ${JSON.stringify(value)}\n\n`;
 res.writeHead(200,{'content-type':'text/event-stream'});res.end([
 event('message_start',{type:'message_start',message:{id:`msg_${requests.length}`,type:'message',role:'assistant',content:[],model:'claude-sonnet-4-20250514',stop_reason:null,stop_sequence:null,usage:{input_tokens:100,output_tokens:0}}}),
 event('content_block_start',{type:'content_block_start',index:0,content_block:{type:'text',text:''}}),
 event('content_block_delta',{type:'content_block_delta',index:0,delta:{type:'text_delta',text}}),event('content_block_stop',{type:'content_block_stop',index:0}),
 event('message_delta',{type:'message_delta',delta:{stop_reason:'end_turn',stop_sequence:null},usage:{output_tokens:5}}),event('message_stop',{type:'message_stop'}),].join(''));
 }catch{res.writeHead(500);res.end();}
});model.listen(0,'127.0.0.1');await once(model,'listening');
try {
 const address=model.address();assert.ok(address&&typeof address!=='string');const endpoint=`http://127.0.0.1:${address.port}`;
 const accountId=randomUUID(),storeScope='shared_bot_fixture';
 const account={accountId,providerId:'anthropic',displayName:'Synthetic fixture',executionBindingId:'anthropic',authKind:'api_key' as const,credentialRef:{storeScope,secretRef:`acct.${accountId}`},enabled:true,userPriority:0,configRevision:1,identityGeneration:1};
 const snapshot:ProviderAccountSnapshot={storeScope,configRevision:1,accounts:[account],health:[]};
 mkdirSync(join(configHome,'provider-accounts','versions',storeScope),{recursive:true,mode:0o700});
 writeFileSync(join(configHome,'provider-accounts',`${storeScope}.json`),JSON.stringify(snapshot),{mode:0o600});
 writeFileSync(join(configHome,'provider-accounts','versions',storeScope,`${accountId}.json`),JSON.stringify({accountId,identityGeneration:1,credentialRevision:1}),{mode:0o600});
 const profile={provider_id:'anthropic',profile_name:'anthropic',base_url:endpoint,protocol:'anthropic_messages',auth:'api_key',credential:{source:'host_managed',key:'team-selected-model'},models:[{display_model:'claude-sonnet-4-20250514',request_model:'claude-sonnet-4-20250514',billing_model:'claude-sonnet-4-20250514',capability_support:{streaming:'supported',tools:'supported'}}],extra:{}};
 const scope={teamId:'teamfixture01',sharedBotId:'sharedfixture01',conversationId:'chatfixture01',sessionEpoch:1};
 const common={version:3 as const,storageRoot:paths.runtime,workerExecutable:worker,scope};let authorizations=0;
 const run=async(bootstrap:TeamHostBootstrap)=>{console.log(JSON.stringify({phase:bootstrap.type,root}));return runTeamHost({executable:host,home:paths.home,tmp:paths.tmp,bootstrap,signal:AbortSignal.timeout(310000),assertCurrent:()=>{},authorize:async()=>{authorizations++;}}).catch(error=>{console.error(JSON.stringify({failedPhase:bootstrap.type,authorizations,modelCalls:requests.length,httpRequests,headerNames,root,code:error.code??error.message}));throw error;});};
 let inspected=await run({...common,type:'inspect_scope'});assert.equal(inspected.cursor.sessionRevision,0);
 const input=(from:number,through:number)=>Array.from({length:through-from},(_,i)=>{const seq=from+i+1;return {messageId:`message${String(seq).padStart(6,'0')}`,seq,changeSeq:seq,changeId:`change${String(seq).padStart(6,'0')}`,author:{kind:'user',uid:seq%2?'member-one':'member-two',displayName:'Same name'},createdAt:1700000000000+seq,text:`Source ${seq}: ${'shared text '.repeat(64)}`};});
 const results:TeamHostResult[]=[];const workerDurationsMs:number[]=[];
 for(const [index,to] of [200,650].entries()){
  const from=inspected.cursor.lastMessageSeq,taskId=`taskfixture0${index+1}`,messages=input(from,to);
  const batch=await privateStore.dataRef('fixture-owner',{schemaVersion:1,batchId:taskId,...scope,expectedSessionRevision:inspected.cursor.sessionRevision,fromSeqExclusive:from,toSeqInclusive:to,lastChangeSeq:to,bootstrap:index===0,currentMessageId:messages.at(-1)!.messageId,messages,changes:[]});
  const workerStarted=performance.now();
  const result=await run({...common,type:'run',taskId,invocationId:randomUUID(),instructions:'Use only published shared information. Answer briefly. Do not call tools.',inputBatchRef:batch,knowledge:[],deadlineAt:Date.now()+300000,
   model:{model_provider:'anthropic',model_protocol:'anthropic_messages',model_id:'claude-sonnet-4-20250514',model_endpoint:endpoint,model_profile:profile},
   credentials:{provider_account_home:configHome,provider_accounts:snapshot,provider_account_profiles:[profile],provider_account_credentials:{[account.accountId]:{credential_revision:1,identity_generation:account.identityGeneration,kind:{kind:'api_key',key:'synthetic-model-key'}}}},
   limits:{max_turns:2,context_window_tokens:2000000,output_tokens_per_request:64,physical_dispatches:2}});
  workerDurationsMs.push(Math.round(performance.now()-workerStarted));
  assert.equal(result.status,'completed');assert.equal(result.cursor.lastMessageSeq,to);results.push(result);await privateStore.releaseDataRef('fixture-owner',batch);
  inspected=await run({...common,type:'inspect_scope'});assert.equal(inspected.cursor.lastMessageSeq,to);
  const recovered=await run({...common,type:'inspect_result',taskId});assert.equal(recovered.text,result.text);assert.equal(recovered.finishedAt,result.finishedAt);assert.equal(requests.length,index+1,'result observation must not call model');
 }
 assert.equal(requests.length,2);const modelInput=JSON.stringify(requests[1]);assert.ok(modelInput.includes('member-one')&&modelInput.includes('member-two'));assert.ok(modelInput.includes('1700000000650'));assert.ok(modelInput.includes('Fixture reply 1'));assert.ok(!modelInput.includes('OWNER_PRIVATE_SENTINEL_DO_NOT_COPY'));assert.ok(authorizations>=4);
 const hashes=binaryHashes();assert.deepEqual(hashes,initialHashes,'runtime binaries changed during validation');
 const receipt={schemaVersion:1,privateProtocol:3,executionVersion:2,root,hashes,modelCalls:requests.length,authorizations,sourceRows:[200,450],workerDurationsMs,taskDeadlineMs:300000,sessionRevisions:results.map(r=>r.cursor.sessionRevision),resultRecovery:'no new model dispatch',acceptance:'synthetic provider + actual isolated processes only'};
 writeFileSync(join(root,'receipt.json'),JSON.stringify(receipt,null,2),{mode:0o600});console.log(JSON.stringify(receipt));
}finally{model.closeAllConnections();await new Promise<void>(resolve=>model.close(()=>resolve()));}
