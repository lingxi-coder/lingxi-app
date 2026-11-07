import { useState } from 'react';
import { createRoot } from 'react-dom/client';
import { Decision } from '../../src/renderer/components/settings/pages/Decision';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { DecisionApi, DecisionInput, DecisionSnapshot } from '../../src/shared/decision';
localStorage.setItem('lingxi.desktop.language','en');
let snapshot: DecisionSnapshot = {revision:'initial',configuration:null,credentialConfigured:false};
let count=0,hold=false;
let release:(()=>void)|undefined;
let rejectTest:((error:Error)=>void)|undefined;
const listeners=new Set<(state:DecisionSnapshot)=>void>();
const calls:DecisionInput[]=[];
const publish=()=>{for(const listener of listeners)listener(structuredClone(snapshot));};
const api:DecisionApi={
 snapshot:async()=>structuredClone(snapshot),
 onChanged:listener=>{listeners.add(listener);return()=>listeners.delete(listener);},
 save:async(revision,configuration,credential)=>{
  if(hold){hold=false;await new Promise<void>(done=>{release=done;});}
  if(revision!==snapshot.revision)throw new Error('Decision settings changed in another window');
  snapshot={revision:`r${++count}`,configuration,credentialConfigured:credential!==undefined?!!credential:snapshot.credentialConfigured};publish();return structuredClone(snapshot);
 },
 evaluate:async(_id,input)=>{
  calls.push(structuredClone(input));
  if(hold){hold=false;await new Promise<void>((done,reject)=>{release=done;rejectTest=reject;});}
  return {state_revision:0,answers:Object.fromEntries(Object.entries(input.questions).map(([name,q])=>[name,q.type==='binary'?{type:'binary',probability:0.9}:q.type==='choice'?{type:'choice',option:Object.keys(q.options)[0],probabilities:null,confidence:null}:{type:'score',value:0.5,probabilities:null,confidence:null}])),usage:{input_tokens:10,output_tokens:null},actual_model:'served-model',elapsed_ms:12};
 },
 cancel:async()=>{rejectTest?.(new Error('Decision operation cancelled'));rejectTest=undefined;},
};
const fixture={calls,hold(){hold=true;},release(){release?.();release=undefined;},external(){snapshot={...snapshot,revision:`r${++count}`,configuration:{source:'jev',protocol:'typesafe_systemone',endpoint:'https://api.typesafe.ai/v1/systemone',model:'external-model'}};publish();}};
Object.assign(window,{lingxi:{decision:api,providerAccounts:{snapshot:async()=>({accounts:[]})}},decisionFixture:fixture});
function Fixture(){const[dark,setDark]=useState(false);Object.assign(fixture,{setDark});const t=tokens(dark);return <Theme.Provider value={t}><main style={{height:'100vh',overflow:'auto',background:t.windowBg}}><div style={{maxWidth:720,padding:24,margin:'0 auto'}}><Decision/></div></main></Theme.Provider>;}
createRoot(document.getElementById('root')!).render(<Fixture/>);
