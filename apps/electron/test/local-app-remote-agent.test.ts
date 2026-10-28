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
