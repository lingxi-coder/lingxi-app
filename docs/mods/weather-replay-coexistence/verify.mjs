import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';
import {resolve} from 'node:path';
const [replayPath, weatherPath] = process.argv.slice(2);
if (!replayPath || !weatherPath) throw new Error('Usage: node verify.mjs REPLAY_MODULE WEATHER_MODULE');
const modulePaths = { 'replay-theater': replayPath, 'token-weather': weatherPath };
for(const [run,order] of [['replay-theater','token-weather'],['token-weather','replay-theater']].entries()) {
 const hooks=[];
 const $={session:{usage:async()=>({context:{tokens:23000,window:1000000,percent:2.3}}),cwd:async()=>'/fixture'},command:{register:async()=>{}},clock:{sleep:async()=>{}},fs:{exists:async()=>false},ui:{invalidate:()=>{},open:async()=>{throw Error('Pane unavailable')},close:()=>{},resolve:()=>Object.fromEntries(['Box','Text','Button'].map(type=>[type,props=>({type,props,children:props.children??[]})]))}};
 for(const mod of order){const module=await import(pathToFileURL(resolve(modulePaths[mod])).href+`?run=${run}`);module.register((event,filter,fn)=>hooks.push({event,filter:fn?filter:{},fn:fn??filter}));}
 const dispatch=(event,input,terminal=null)=>{const chain=hooks.filter(h=>h.event===event&&Object.entries(h.filter).every(([k,v])=>input[k]===v));const go=i=>i===chain.length?Promise.resolve(terminal):Promise.resolve(chain[i].fn($,input,()=>go(i+1)));return go(0)};
 const render=()=>dispatch('ui.render',{component:'AbovePrompt',surface:'desktop',bodyColumns:100}, {type:'Text',children:['ENGINE_BASE']});
 const text=t=>JSON.stringify(t);
 const button=(tree,label)=>{if(tree?.type==='Button'&&tree.props.label===label)return tree;for(const child of Array.isArray(tree?.children)?tree.children:[]) {const found=button(child,label);if(found)return found;}return null};
 await dispatch('session.start',{});let tree=await render();assert.match(text(tree),/Clear/);assert.match(text(tree),/ENGINE_BASE/,'weather must retain downstream content');
 await dispatch('turn.start',{});for(const [tool,extra] of [['Write',{content:'pelican\n'}],['Write',{content:'before\n'}],['Edit',{old_string:'before',new_string:'after'}]])await dispatch('tool.call',{tool,file_path:'/fixture/file',...extra});await dispatch('turn.complete',{});
 tree=await render();assert.match(text(tree),/Clear/,'weather must survive replay hint');assert.match(text(tree),/Replay: 3 edits/);assert.match(text(tree),/ENGINE_BASE/);
 await button(tree,'Replay').props.onPress();tree=await render();assert.match(text(tree),/Replay Theater/);assert.match(text(tree),/Clear/,'weather must survive open replay');assert.match(text(tree),/step 1 of 3/);
 button(tree,'Next ▶').props.onPress();tree=await render();assert.match(text(tree),/step 2 of 3/);assert.match(text(tree),/Clear/);
 button(tree,'◀ Prev').props.onPress();tree=await render();assert.match(text(tree),/step 1 of 3/);
 button(tree,'Close').props.onPress();tree=await render();assert.match(text(tree),/Replay: 3 edits/);assert.match(text(tree),/Clear/);assert.match(text(tree),/ENGINE_BASE/);assert.doesNotMatch(text(tree),/Replay Theater|step 1 of 3/,'Close must remove the expanded replay view');
 console.log('PASS',order.join(' -> '),'before edits, hint, open, next, prev, close; engine content preserved');
}
