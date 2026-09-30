import React, { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { GitReview, GitTopBar, GitEnvironment, GitWorkspaceProvider } from '../../src/renderer/components/GitReview';
import '../../src/renderer/components/GitReview.css';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { GitApi, GitRequest, GitStatus } from '../../src/shared/git';
const changed = new Set<() => void>(); const calls: { session: string; request: GitRequest }[] = []; let revision = 1; let failure = ''; let deferred: (() => void) | undefined; let hold = false; let holdDiff = false; let releaseDiff: (() => void) | undefined; let diffPatch: string | undefined;
const statuses = new Map<string, GitStatus>();
function status(session: string) { if (!statuses.has(session)) statuses.set(session, { available:true, repository:true, root:'/tmp/review', gitDir:'/tmp/review/.git',commonDir:'/tmp/review/.git', branch:session === 'a' ? 'main' : 'feature/mobile', head:'abc123456789', ahead:1, behind:0,files:session === 'a' ? [{path:'src/workspace.ts',index:' ',working:'M',untracked:false,conflict:false,additions:2,deletions:1,binary:false},{path:'README.md',index:'M',working:' ',untracked:false,conflict:false,additions:1,deletions:0,binary:false}] : [],token:String(revision),busy:false,merging:false,mergeOwned:false,remotes:['origin'] }); return statuses.get(session)!; }
const patch='diff --git a/src/workspace.ts b/src/workspace.ts\nindex 123..456 100644\n--- a/src/workspace.ts\n+++ b/src/workspace.ts\n@@ -1,3 +1,4 @@\n export function workspace() {\n-  return "local";\n+  const name = "你好";\n+  return name;\n }\n';
const api: GitApi={ onChanged(callback) { const listener=()=>callback({root:'/tmp/review'});changed.add(listener);return()=>{changed.delete(listener);}; },async request(scope, request) { calls.push({session:scope.sessionId,request}); const current=status(scope.sessionId); if(hold && request.kind==='commit') { hold=false; await new Promise<void>(resolve=>{deferred=resolve;}); } if(failure===request.kind) throw new Error('Test operation failed; draft retained');
 switch(request.kind) {
 case 'status':return {status:structuredClone({...current,token:String(revision)})};
 case 'branches':return {branches:[{name:current.branch,remote:false,current:true,worktree:'/tmp/review'},{name:'feature/search',remote:false,current:false},{name:'feature/occupied',remote:false,current:false,worktree:'/tmp/other'},{name:'origin/main',remote:true,current:false}]};
 case 'diff':if (holdDiff) { holdDiff = false; await new Promise<void>(resolve => { releaseDiff = resolve; }); } return {diff:{patch:diffPatch ?? patch,binary:false,truncated:false,token:String(revision),files:structuredClone(current.files)}};
 case 'history':return {commits:[{id:'abc123456789',parents:[],author:'Lin',date:'2026-09-11',subject:'Keep workspace state isolated',body:'Preserve the active session.'}]};
 case 'stashList':return {stashes:[{id:'stash-1',ref:'stash@{0}',subject:'Saved work',date:'2026-09-11'}]};
 case 'conflictRead':return {conflict:{path:request.path,base:'base\n',ours:'ours\n',theirs:'theirs\n',result:'<<<<<<< HEAD\nours\n=======\ntheirs\n>>>>>>> branch\n',binary:false,token:String(revision)}};
 case 'stage':current.files.forEach(f=>{if(!request.paths||request.paths.includes(f.path)){f.index='M';f.working=' ';}});break;
 case 'unstage':current.files.forEach(f=>{if(!request.paths||request.paths.includes(f.path)){f.index=' ';f.working='M';}});break;
 case 'commit':current.files=current.files.filter(f=>f.index===' ');break;
 case 'checkout':current.branch=request.branch;break;
 case 'conflictResolve':current.files=current.files.filter(f=>f.path!==request.path);break;
 }
 revision++; return {output:'Completed',status:structuredClone({...current,token:String(revision)})};
 }};
window.lingxi={git:api} as typeof window.lingxi;
function Fixture(){const [session,setSession]=useState('a');const[dark,setDark]=useState(false);Object.assign(window,{gitFixture:{calls,holdDiff:()=>{holdDiff=true;},releaseDiff:()=>{releaseDiff?.();releaseDiff=undefined;},diffPending:()=>!!releaseDiff,editDiff:()=>{diffPatch=patch.replace("你好", "再见");revision++;changed.forEach(cb=>cb());},revision:()=>revision,bump:()=>{revision++;changed.forEach(cb=>cb());},session:setSession,theme:setDark,fail:(kind:string)=>{failure=kind;},hold:()=>{hold=true;},release:()=>{deferred?.();deferred=undefined;},conflict:()=>{status(session).files.push({path:'conflict.txt',index:'U',working:'U',untracked:false,conflict:true,additions:0,deletions:0,binary:false});revision++;changed.forEach(cb=>cb());},clean:()=>{status(session).files=[];revision++;changed.forEach(cb=>cb());},busy:(value:boolean)=>{status(session).busy=value;revision++;changed.forEach(cb=>cb());}}});
return <Theme.Provider value={tokens(dark)}><GitWorkspaceProvider scope={{projectPath:'/tmp/review',sessionId:session}} onOpen={()=>{}} onTerminal={()=>{}}><div style={{height:'100vh',display:'flex',fontFamily:'system-ui',background:dark?'#17181b':'#fff',color:dark?'#ddd':'#242629'}}><aside style={{width:205,padding:22,background:dark?'#202126':'#f3f3f6'}}>LingXi Code<br/><br/>Projects<br/><br/>LingXi-Next</aside><main style={{flex:1,padding:24,display:'flex',flexDirection:'column'}}><header style={{display:'flex',justifyContent:'space-between'}}>Desktop Git management<GitTopBar/></header><div style={{flex:1,padding:28}}>Review your changes before committing.<div id="overview-fixture" style={{width:300,marginLeft:'auto',marginTop:20,border:'1px solid #8883',borderRadius:18,padding:14}}><GitEnvironment/><p>Plan</p><p>Subagents</p></div></div><textarea id="composer" placeholder="Ask anything" style={{borderRadius:18,padding:18}}/></main><aside style={{width:520,display:'flex',flexDirection:'column',borderLeft:'1px solid #8883'}}><header style={{padding:18,borderBottom:'1px solid #8883'}}>Review</header><GitReview/></aside></div></GitWorkspaceProvider></Theme.Provider>;
}createRoot(document.getElementById('root')!).render(<Fixture/>);
