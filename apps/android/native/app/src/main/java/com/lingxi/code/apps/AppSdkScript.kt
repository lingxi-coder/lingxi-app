package com.lingxi.code.apps

import org.json.JSONArray
import org.json.JSONObject

/** Immutable run metadata; URL construction itself never grants access to a service. */
internal fun appSdkScript(launch: AppLaunch): String {
    val configuration = JSONObject().put("origin", launch.origin).put("runId", launch.run.runId)
        .put("services", JSONArray(launch.run.services.toList())).toString()
    return """
        (() => { if (window.top !== window || !window.lingxiNative) return;
        const config=$configuration;
        const pending=new Map(); let sequence=0;
        lingxiNative.onmessage=e=>{let r;try{r=JSON.parse(e.data)}catch{return}const p=pending.get(r.id);if(!p)return;pending.delete(r.id);r.ok?p.resolve(r.value):p.reject(new Error(r.error));};
        const invoke=(method,args={})=>new Promise((resolve,reject)=>{const id=String(++sequence);pending.set(id,{resolve,reject});try{const message=JSON.stringify({id,method,args});if(message.length>24*1024*1024)throw new Error('Application request is too large');lingxiNative.postMessage(message);}catch(error){pending.delete(id);reject(error);}});
        const agent=Object.freeze({run:input=>invoke('agent.run',input),get:runId=>invoke('agent.get',{runId}),cancel:runId=>invoke('agent.cancel',{runId}),subscribe:(runId,listener)=>{
            if(typeof listener!=='function')throw new TypeError('A listener is required');let active=true,timer,last=0;
            const poll=async()=>{try{const run=await invoke('agent.get',{runId});if(active&&run.sequence>last){last=run.sequence;listener({type:'state',runId,sequence:run.sequence,time:run.updatedAt,run});}if(['succeeded','failed','cancelled','interrupted'].includes(run.state))active=false;}catch(error){if(active)console.warn('Agent subscription:',error.message);}if(active)timer=setTimeout(poll,750);};poll();return()=>{active=false;clearTimeout(timer);};
        }});
        const serviceUrl=(name,path='/')=>{
            if(typeof name!=='string'||!config.services.includes(name))throw new Error('Unknown application service');
            if(typeof path!=='string'||!path.startsWith('/')||path.startsWith('//')||path.includes('\\')||path.includes('#'))throw new Error('Invalid service path');
            const rawPath=path.split('?')[0];let decoded;try{decoded=decodeURIComponent(rawPath)}catch{throw new Error('Invalid service path')}
            if(/%2f|%5c/i.test(rawPath)||decoded.includes('\\')||decoded.includes('\u0000')||decoded.split('/').some(part=>part==='.'||part==='..'))throw new Error('Invalid service path');
            return config.origin+'/__run/'+encodeURIComponent(config.runId)+'/__service/'+encodeURIComponent(name)+path;
        };
        Object.defineProperty(window,'localApp',{configurable:false,writable:false,value:Object.freeze({
            info:()=>invoke('info'),agent,serviceUrl,serviceSocketUrl:(name,path='/')=>{try{serviceUrl(name,path);return invoke('serviceSocketUrl',{name,path});}catch(error){return Promise.reject(error);}},
            camera:Object.freeze({capture:()=>invoke('camera.capture')}),photos:Object.freeze({pick:()=>invoke('photos.pick')}),files:Object.freeze({pick:()=>invoke('files.pick')}),
            microphone:Object.freeze({record:()=>invoke('microphone.record')}),
            clipboard:Object.freeze({read:()=>invoke('clipboard.read'),write:text=>invoke('clipboard.write',{text})}),share:Object.freeze({text:text=>invoke('share',{text})}),
            notify:body=>invoke('notify',{body}),haptics:()=>invoke('haptics')
        })});
        })();
    """.trimIndent()
}
