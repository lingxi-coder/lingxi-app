import { test } from 'node:test';
import assert from 'node:assert/strict';
import { emptySubmittedPlanState, reduceSubmittedPlanEvent, reduceSubmittedPlanMessages } from '../src/renderer/bridge/submittedPlan';
import type { ClientEvent, MessageDto } from '@lingxi/bridge-client';
const body = '# 环境切换修改方案\n\n## 修复\n完整正文\n\n## 验证\n完成';
const blocks: MessageDto['blocks'] = [
 {type:'tool_use',id:'write',tool:'Write',input_json:JSON.stringify({file_path:'.lingxi/plans/environment-org-switch-fix.md',content:body})},
 {type:'tool_result',id:'write',tool:'Write',result_json:'{}',is_error:false},
 {type:'tool_use',id:'exit',tool:'ExitPlanMode',input_json:'{}'},
 {type:'tool_result',id:'exit',tool:'ExitPlanMode',result_json:JSON.stringify({plan:null,model_content:'User has approved exiting plan mode. You can now proceed.'}),is_error:false},
];
test('history uses a successful explicit plan-file write when ExitPlanMode carries no body',()=>{
 const state=reduceSubmittedPlanMessages(emptySubmittedPlanState(),[{role:'assistant',blocks}], 'session');
 assert.equal(state.calls[0]?.content,body);
 assert.equal(state.calls[0]?.status,'approved');
});
test('live plan file survives a duplicate start and fills the submitted card',()=>{
 let state=emptySubmittedPlanState();
 const start: ClientEvent={type:'tool_use_started',id:'write',tool:'Write',input_json:(blocks[0] as {input_json:string}).input_json};
 for(const e of [start,{type:'tool_use_result',id:'write',tool:'Write',result_json:'{}',is_error:false},start,{type:'tool_use_started',id:'exit',tool:'ExitPlanMode',input_json:'{}'}] as ClientEvent[]) state=reduceSubmittedPlanEvent(state,e,'session');
 assert.equal(state.calls[0]?.content,body);
});
test('failed writes, ordinary markdown files and a new planning cycle cannot supply the card',()=>{
 for(const mode of ['failed','ordinary','new-cycle']){
  let state=emptySubmittedPlanState();
  state=reduceSubmittedPlanEvent(state,{type:'tool_use_started',id:'write',tool:'Write',input_json:JSON.stringify({file_path:mode==='ordinary'?'README.md':'.lingxi/plans/p.md',content:body})},'session');
  state=reduceSubmittedPlanEvent(state,{type:'tool_use_result',id:'write',tool:'Write',result_json:'{}',is_error:mode==='failed'},'session');
  if(mode==='new-cycle') state=reduceSubmittedPlanEvent(state,{type:'tool_use_started',id:'enter',tool:'EnterPlanMode',input_json:'{}'},'session');
  state=reduceSubmittedPlanEvent(state,{type:'tool_use_started',id:'exit',tool:'ExitPlanMode',input_json:'{}'},'session');
  assert.equal(state.calls[0]?.content,'');
 }
});
