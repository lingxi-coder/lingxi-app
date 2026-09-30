import {test} from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import {renderToStaticMarkup} from 'react-dom/server';
import {emptySubmittedPlanState,reduceSubmittedPlanMessages} from '../src/renderer/bridge/submittedPlan';
import {Stage} from '../src/renderer/components/Stage';
import {Theme} from '../src/renderer/theme/ThemeContext';
import {tokens} from '../src/renderer/theme/tokens';
import type {RunItem} from '../src/renderer/model/runItem';
(globalThis as {React?:typeof React}).React=React;
test('real Stage shows a plan card outside collapsed tool summaries',()=>{
 const liveItems:RunItem[]=[{id:'plan',type:'tool',tool:'ExitPlanMode',status:'done',view:{label:'ExitPlanMode'} as never}];
 const html=renderToStaticMarkup(React.createElement(Theme.Provider,{value:tokens(false)},React.createElement(Stage,{liveItems,sessionKey:'one',submittedPlans:[{id:'plan',content:'# Actual proposal\n\nKeep data.',status:'approved'}],onOpenPlan:()=>{}})));
 assert.match(html,/Open full plan/);
 assert.match(html,/aria-label="Copy plan"/);
 assert.match(html,/Actual proposal/);
 assert.match(html,/Keep data/);
 assert.doesNotMatch(html,/Used 1 tool/);
});
test('ordinary assistant Markdown remains normal narration',()=>{
 const liveItems:RunItem[]=[{id:'text',type:'narration',role:'assistant',text:'Normal reply'}];
 const html=renderToStaticMarkup(React.createElement(Theme.Provider,{value:tokens(false)},React.createElement(Stage,{liveItems,sessionKey:'two'})));
 assert.match(html,/Normal reply/);
 assert.doesNotMatch(html,/Open full plan/);
});

test('completed ExitPlanMode without a document does not keep preparing or open an empty plan',()=>{
 const liveItems:RunItem[]=[{id:'plan',type:'tool',tool:'ExitPlanMode',status:'done',view:{label:'ExitPlanMode'} as never}];
 const state=reduceSubmittedPlanMessages(emptySubmittedPlanState(),[{blocks:[
  {type:'tool_use',id:'plan',tool:'ExitPlanMode',input_json:'{}'},
  // The payload the engine actually emits for an approved, document-less exit,
  // copied from a real transcript. This used to carry `plan_mode:false`, a key
  // `plan_mode.rs` asserts is absent from both plan tools' results — so the
  // branch it relied on could only ever fire here, and the real shape fell
  // through to 'submitted' and rendered "Preparing plan…" forever.
  {type:'tool_result',id:'plan',tool:'ExitPlanMode',result_json:JSON.stringify({plan:null,isAgent:false,filePath:'/missing/plan.md',hasTaskTool:true,planWasEdited:false,model_content:'User has approved exiting plan mode. You can now proceed.'}),is_error:false},
 ]}] as never,'empty');
 const html=renderToStaticMarkup(React.createElement(Theme.Provider,{value:tokens(false)},React.createElement(Stage,{liveItems,sessionKey:'empty',submittedPlans:state.calls,onOpenPlan:()=>{}})));
 assert.match(html,/Plan mode exited/);
 assert.match(html,/No plan document was submitted/);
 assert.doesNotMatch(html,/Preparing plan|Writing plan|Open full plan|Copy plan/);
});

for (const [status,label] of [['pending','Waiting for plan approval'],['rejected','Plan rejected'],['failed','Plan submission failed']] as const) {
 test(`empty ${status} plan displays its actual status`,()=>{
  const liveItems:RunItem[]=[{id:'plan',type:'tool',tool:'ExitPlanMode',status:status==='pending'?'running':'error',view:{label:'ExitPlanMode'} as never}];
  const html=renderToStaticMarkup(React.createElement(Theme.Provider,{value:tokens(false)},React.createElement(Stage,{liveItems,sessionKey:status,submittedPlans:[{id:'plan',content:'  ',status}],onOpenPlan:()=>{}})));
  assert.match(html,new RegExp(label));
  assert.doesNotMatch(html,/Preparing plan|Writing plan|Open full plan|Copy plan/);
 });
}

for (const [status,label] of [['rejected','Plan rejected'],['failed','Plan submission failed']] as const) {
 test(`a ${status} plan keeps its submitted document available`,()=>{
  const liveItems:RunItem[]=[{id:'plan',type:'tool',tool:'ExitPlanMode',status:'error',view:{label:'ExitPlanMode'} as never}];
  const html=renderToStaticMarkup(React.createElement(Theme.Provider,{value:tokens(false)},React.createElement(Stage,{liveItems,sessionKey:status,submittedPlans:[{id:'plan',content:'# Original proposal',status}],onOpenPlan:()=>{}})));
  assert.match(html,new RegExp(label));
  assert.match(html,/Original proposal/);
  assert.match(html,/Open full plan/);
 assert.match(html,/aria-label="Copy plan"/);
 });
}
