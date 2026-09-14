import { test } from 'node:test';
import assert from 'node:assert/strict';
import { narrationPlans, conversationPlans } from '../src/renderer/bridge/planDocuments';
import type { RunItem } from '../src/renderer/model/runItem';
const assistant = (id: string, text: string): RunItem => ({ type: 'narration', role: 'assistant', id, text });
test('only explicit assistant plans become documents', () => {
 assert.equal(narrationPlans([assistant('a','# Plan\nNormal answer'),assistant('b','```xml\n<proposed_plan>example</proposed_plan>\n```'),{type:'narration',role:'user',id:'c',text:'<proposed_plan>user</proposed_plan>'}]).length,0);
 assert.deepEqual(narrationPlans([assistant('d','<proposed_plan>\n# 中文计划\n正文\n</proposed_plan>')]),[{id:'d',content:'# 中文计划\n正文',status:'submitted'}]);
});
test('streamed and restored plans keep stable identities and session isolation', () => {
 assert.equal(narrationPlans([assistant('a','<proposed_plan>\n# Draft')])[0]?.id,'a');
 assert.equal(narrationPlans([assistant('a','<proposed_plan>\n# Draft done\n</proposed_plan>')])[0]?.content,'# Draft done');
 assert.deepEqual(conversationPlans([],[]),[]);
});
test('latest plan follows transcript order across sources', () => {
 const calls=[{id:'tool',content:'# New',status:'approved' as const}];
 const items:RunItem[]=[assistant('old','<proposed_plan># Old</proposed_plan>'),{type:'tool',id:'tool',tool:'ExitPlanMode',status:'done',view:{label:'Plan'} as never}];
 assert.deepEqual(conversationPlans(items,calls).map(p=>p.id),['old','tool']);
});
test('structured Chinese and English proposal headings use cards, code examples do not', () => {
 const plan = '# 环境切换 — 修改方案\n\n## 不变量\n一致。\n\n## 验证\n测试。';
 assert.equal(narrationPlans([assistant('p',plan)])[0]?.content,plan);
 assert.equal(narrationPlans([assistant('p','# Migration plan\n## Changes\nA\n## Tests\nB')]).length,1);
 assert.equal(narrationPlans([assistant('p','```md\n'+plan+'\n```')]).length,0);
 assert.equal(narrationPlans([{type:'narration',id:'u',role:'user',text:plan}]).length,0);
});
