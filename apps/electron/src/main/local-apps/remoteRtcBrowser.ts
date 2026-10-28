/** Injected in the authenticated app origin. RTC setup is deliberately asynchronous. */
export const LOCAL_APP_RTC_BROWSER_SOURCE = `(()=>{
  if (!window.RTCPeerConnection || !window.fetch) return;
  const nativeFetch = window.fetch.bind(window), allowed = new Set(['accept','accept-language','if-none-match','if-modified-since']);
  let pc, channel, nextId = 1, ready = false, pending = new Map(), chunks = [], length = 0;
  const fail = () => { ready = false; try { pc?.close(); } catch {} for (const p of pending.values()) p.reject(new Error('RTC unavailable')); pending.clear(); };
  const send = value => { const bytes = new TextEncoder().encode(JSON.stringify(value)); if (bytes.length > 65536) throw Error('large request'); for (let i=0;i<bytes.length;i+=16383) { const end=Math.min(i+16383,bytes.length), part=new Uint8Array(end-i+1); part[0]=bytes.length<=16383?0:end===bytes.length?2:1; part.set(bytes.subarray(i,end),1); channel.send(part); } };
  const gather = pc => new Promise(resolve => { if(pc.iceGatheringState==='complete') return resolve(); const timer=setTimeout(done,3000); function done(){clearTimeout(timer);pc.removeEventListener('icegatheringstatechange',check);resolve()} function check(){if(pc.iceGatheringState==='complete')done()} pc.addEventListener('icegatheringstatechange',check); });
  (async()=>{ try {
    const config=await nativeFetch('/__lingxi_remote_rtc',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({type:'config'})}); if(!config.ok) return;
    const {iceServers}=await config.json(); pc=new RTCPeerConnection({iceServers}); channel=pc.createDataChannel('lingxi-agent',{ordered:true}); channel.binaryType='arraybuffer';
    channel.onopen=()=>{ready=true}; channel.onclose=fail; channel.onerror=fail; pc.onconnectionstatechange=()=>{if(['failed','closed'].includes(pc.connectionState))fail()};
    channel.onmessage=e=>{try {const part=new Uint8Array(e.data), kind=part[0]; if(!part.length||part.length>16384||![0,1,2].includes(kind)||(kind===0&&length)||(kind===2&&!length)) throw Error(); length+=part.length-1;if(length>524288)throw Error(); chunks.push(part.slice(1)); if(kind===1)return;const all=new Uint8Array(length);let at=0;for(const c of chunks){all.set(c,at);at+=c.length}chunks=[];length=0;const msg=JSON.parse(new TextDecoder('utf-8',{fatal:true}).decode(all));const p=pending.get(msg.id);if(p){pending.delete(msg.id);p.resolve(msg)}} catch {fail()} };
    await pc.setLocalDescription(await pc.createOffer()); await gather(pc);
    const res=await nativeFetch('/__lingxi_remote_rtc',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({sdp:pc.localDescription.sdp})}); if(!res.ok)throw Error(); const answer=await res.json(); await pc.setRemoteDescription({type:'answer',sdp:answer.sdp});
    setTimeout(()=>{if(!ready)fail()},15000);
  } catch {fail()} })();
  window.fetch=async (input, init)=>{
    let request; try {request=new Request(input,init)} catch {return nativeFetch(input,init)}
    const url=new URL(request.url); const fallback=()=>nativeFetch(request);
    if(!ready || channel.readyState!=='open' || channel.bufferedAmount>262144 || pending.size>=8 || url.origin!==location.origin || !['GET','HEAD'].includes(request.method) || request.signal.aborted || request.cache!=='default' || request.credentials!=='omit' || request.mode==='no-cors' || request.redirect!=='follow' || request.integrity || url.pathname.startsWith('/__lingxi_remote') || url.hash) return fallback();
    const headers={}; for(const [key,value] of request.headers){if(!allowed.has(key)||key==='accept'&&value.includes('text/event-stream'))return fallback();headers[key]=value}
    const id=nextId++; let timer, abort;
    try {
      const result=new Promise((resolve,reject)=>{pending.set(id,{resolve,reject});timer=setTimeout(()=>{pending.delete(id);reject(Error('timeout'))},15000);abort=()=>{pending.delete(id);reject(new DOMException('Aborted','AbortError'))};request.signal.addEventListener('abort',abort,{once:true});});
      send({id,method:request.method,path:url.pathname+url.search,headers}); const msg=await result;
      if(msg.fallback)return fallback(); const bytes=Uint8Array.from(atob(msg.body),c=>c.charCodeAt(0)); const decorate=res=>{const clone=res.clone.bind(res);return Object.defineProperties(res,{url:{value:request.url},type:{value:'basic'},redirected:{value:false},clone:{value:()=>decorate(clone())}})}; return decorate(new Response([204,205,304].includes(msg.status)||request.method==='HEAD'?null:bytes,{status:msg.status,statusText:msg.statusText,headers:msg.headers}));
    } catch(error) {if(request.signal.aborted)throw error;return fallback()} finally {clearTimeout(timer);request.signal.removeEventListener('abort',abort);pending.delete(id)}
  };
})();`;
