import test from 'node:test';
import assert from 'node:assert/strict';
import { createServer, request as httpRequest } from 'node:http';
import { LocalAppRemoteGateway, type RemoteGatewayAdapter } from '../src/main/local-apps/remoteGateway.js';
import { LocalAppServiceBroker } from '../src/main/local-apps/serviceBroker.js';

const id = `a${'1'.repeat(32)}`, other = `a${'2'.repeat(32)}`;
const owner = { accountUid: 'account', deviceId: 'phone', hostId: 'desktop' };
async function freePort(): Promise<number> {
  const server = createServer(); await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  const port = (server.address() as {port:number}).port; await new Promise<void>(resolve => server.close(() => resolve())); return port;
}
async function request(port: number, host: string, path: string, options: {method?: string; body?: string; cookie?: string; origin?: string; omit?: boolean} = {}): Promise<{status:number; body:string; cookie?:string}> {
  return new Promise((resolve, reject) => {
    const req = httpRequest({hostname:'127.0.0.1',port,path,method:options.method ?? 'GET',headers:{host,...(options.cookie ? {cookie:options.cookie} : {}),...(options.origin ? {origin:options.origin} : {}),...(options.omit ? {'x-lingxi-local-app-credentials':'omit'} : {}),...(options.body ? {'content-type':'application/json'} : {})}}, res => {
      const chunks: Buffer[] = []; res.on('data', c => chunks.push(c)); res.on('end', () => resolve({status:res.statusCode!,body:Buffer.concat(chunks).toString(),cookie:res.headers['set-cookie']?.[0]?.split(';')[0]}));
    }); req.on('error', reject); req.end(options.body);
  });
}
async function setup(bufferBudget?: number) {
  const port = await freePort(); let current = true;
  const adapter: RemoteGatewayAdapter = { current: async () => current, resource: async (_run, req) => new Response(`resource:${new URL(req.url).pathname}`), invoke: async (_run, method) => method, socket: async () => undefined };
  const gateway = new LocalAppRemoteGateway('https://{appId}-{sessionId}.apps.example.com', port, adapter, bufferBudget);
  return {port,gateway,adapter,invalidate:()=>{current=false;}};
}
async function session(gateway: LocalAppRemoteGateway, port: number) {
  const opened = await gateway.open({appId:id,revision:1,runId:'run'},owner), ticket = new URL(opened.url).hash.slice(1);
  const host = new URL(opened.url).host;
  const result = await request(port,host,'/__lingxi_remote_bootstrap',{method:'POST',origin:`https://${host}`,body:JSON.stringify({ticket})});
  assert.equal(result.status,204); assert.ok(result.cookie); return {ticket,cookie:result.cookie!,opened,host};
}
test('single use fragment tickets, app origin isolation, owner revocation and run lifetime', async () => {
  const {port,gateway,invalidate} = await setup();
  try {
    const s = await session(gateway,port); assert.ok(new URL(s.opened.url).hash.length > 1); // ticket stays in fragment
    assert.equal((await request(port,s.host,'/')).status,401);
    assert.equal((await request(port,s.host.replace(id, other),'/',{cookie:s.cookie})).status,401);
    assert.equal((await request(port,s.host,'/__lingxi_remote_bootstrap',{method:'POST',origin:`https://${s.host}`,body:JSON.stringify({ticket:s.ticket})})).status,403);
    assert.equal((await request(port,s.host,'/asset',{cookie:s.cookie})).body,'resource:/asset');
    assert.equal((await request(port,s.host,'/__lingxi_remote_invoke',{method:'POST',cookie:s.cookie,origin:'https://evil.example',body:'{"method":"info","args":{}}'})).status,403);
    assert.equal((await request(port,s.host,'/__lingxi_remote_invoke',{method:'POST',cookie:s.cookie,origin:`https://${s.host}`,body:'{"method":"info","args":{}}'})).body,'{"result":"info"}');
    gateway.revokeOwner(owner); assert.equal((await request(port,s.host,'/',{cookie:s.cookie})).status,401);
    const next = await session(gateway,port); invalidate(); assert.equal((await request(port,next.host,'/',{cookie:next.cookie})).status,401);
  } finally { await gateway.dispose(); }
});
test('rejects unsafe origin configuration and enforces request body limits', async () => {
  const {port,gateway} = await setup();
  try {
    assert.throws(()=>new LocalAppRemoteGateway('http://{appId}-{sessionId}.example.com',port,{} as RemoteGatewayAdapter));
    assert.throws(()=>new LocalAppRemoteGateway('https://example.com/{appId}-{sessionId}',port,{} as RemoteGatewayAdapter));
    assert.throws(()=>new LocalAppRemoteGateway('https://{appId}.apps.example.com',port,{} as RemoteGatewayAdapter));
    assert.throws(()=>new LocalAppRemoteGateway('https://{appId}.{sessionId}.apps.example.com',port,{} as RemoteGatewayAdapter));
    assert.throws(()=>new LocalAppRemoteGateway('https://prefix-{appId}-{sessionId}.apps.example.com',port,{} as RemoteGatewayAdapter));
    const s = await session(gateway,port);
    assert.equal((await request(port,s.host,'/__lingxi_remote_invoke',{method:'POST',cookie:s.cookie,origin:`https://${s.host}`,body:'x'.repeat(1024*1024+1)})).status,400);
  } finally { await gateway.dispose(); }
});

test('streams responses and aborts them on owner revocation', async () => {
  const {port,gateway,adapter} = await setup(); let controller!: ReadableStreamDefaultController<Uint8Array>;
  adapter.resource = async () => new Response(new ReadableStream({start(c){controller=c; c.enqueue(new TextEncoder().encode('data: first\n\n'));}}), {headers:{'content-type':'text/event-stream'}});
  try {
    const s = await session(gateway,port);
    await new Promise<void>((resolve,reject) => {
      const req = httpRequest({hostname:'127.0.0.1',port,path:'/events',headers:{host:s.host,cookie:s.cookie}}, res => {
        res.once('data', chunk => { try { assert.equal(chunk.toString(),'data: first\n\n'); gateway.revokeOwner(owner); } catch (e) { reject(e); } });
        res.once('close', resolve); res.once('error', () => resolve());
      }); req.on('error',reject); req.end();
    });
    assert.throws(() => controller.enqueue(new Uint8Array([1])));
  } finally { await gateway.dispose(); }
});

test('RTC signaling requires same-origin session and closes with the session', async () => {
  const {port,gateway,adapter} = await setup(); let signal: AbortSignal | undefined;
  adapter.rtc = async (_run, _request, _sessionId, ownedSignal) => { signal = ownedSignal; return Response.json({ok:true}); };
  try {
    const s = await session(gateway,port);
    assert.equal((await request(port,s.host,'/__lingxi_remote_rtc',{method:'POST',cookie:s.cookie,body:'{}'})).status,403);
    assert.equal((await request(port,s.host,'/__lingxi_remote_rtc',{cookie:s.cookie})).status,405);
    assert.equal((await request(port,s.host,'/__lingxi_remote_rtc',{method:'POST',cookie:s.cookie,origin:`https://${s.host}`,body:'{}'})).status,200);
    assert.equal(signal?.aborted,false); gateway.revokeOwner(owner); assert.equal(signal?.aborted,true);
  } finally { await gateway.dispose(); }
});

test('revocation wins races with ticket redemption and ticket creation', async () => {
  const {port,gateway,adapter} = await setup();
  try {
    const opened = await gateway.open({appId:id,revision:1,runId:'run'},owner), ticket = new URL(opened.url).hash.slice(1);
    const host = new URL(opened.url).host;
    let release!: () => void, entered!: () => void;
    const enteredPromise = new Promise<void>(resolve => { entered=resolve; });
    adapter.current = async () => { entered(); await new Promise<void>(resolve => { release=resolve; }); return true; };
    const pending = request(port,host,'/__lingxi_remote_bootstrap',{method:'POST',origin:`https://${host}`,body:JSON.stringify({ticket})});
    await enteredPromise; gateway.revokeOwner(owner); release(); assert.equal((await pending).status,403);
    let continueOpen!: () => void, openEntered!: () => void; const waiting = new Promise<void>(resolve=>{openEntered=resolve;});
    let calls = 0; adapter.current = async () => { if (++calls === 1) { openEntered(); await new Promise<void>(resolve=>{continueOpen=resolve;}); } return true; };
    const creating = gateway.open({appId:id,revision:1,runId:'run'},owner); await waiting; gateway.revokeOwner(owner); continueOpen(); await assert.rejects(creating);
  } finally { await gateway.dispose(); }
});

test('backend cookies stay app-scoped and omit preserves host authentication', async () => {
  const {port,gateway,adapter} = await setup();
  let observed: string | null = null;
  adapter.resource = async (_run, req) => {
    observed = req.headers.get('cookie');
    const headers = new Headers(); headers.append('set-cookie', '__Host-lingxi_remote=forged; Secure; Path=/');
    headers.append('set-cookie', 'app_session=value; Domain=apps.example.com; Path=/');
    return new Response('ok',{headers});
  };
  try {
    const s = await session(gateway,port);
    const normal = await request(port,s.host,'/api',{cookie:`${s.cookie}; user=alice`});
    assert.equal(normal.status,200); assert.equal(observed,'user=alice'); assert.equal(normal.cookie,'app_session=value');
    const omitted = await request(port,s.host,'/api',{cookie:`${s.cookie}; user=alice`,omit:true});
    assert.equal(omitted.status,200); assert.equal(observed,null); assert.equal(omitted.cookie,undefined);
  } finally { await gateway.dispose(); }
});

test('WebSocket upgrade is origin and run scoped and closes on revoke', async () => {
  const backend = createServer(); let backendSocket: import('node:stream').Duplex | undefined;
  backend.on('upgrade', (req,socket) => { backendSocket = socket; assert.equal(req.headers.cookie,'app=value'); socket.write('HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n'); socket.on('data', chunk => socket.write(chunk)); });
  await new Promise<void>(resolve=>backend.listen(0,'127.0.0.1',resolve));
  const backendPort = (backend.address() as {port:number}).port;
  const {port,gateway,adapter} = await setup(); adapter.socket = async (_run,name) => name === 'api' ? backendPort : undefined;
  try {
    const s = await session(gateway,port);
    await new Promise<void>((resolve,reject) => {
      const req = httpRequest({hostname:'127.0.0.1',port,path:'/__services/api/ws',headers:{host:s.host,origin:`https://${s.host}`,cookie:`${s.cookie}; app=value`,connection:'Upgrade',upgrade:'websocket'}});
      req.on('upgrade', (_response,socket) => { socket.once('data', chunk=>{ try { assert.equal(chunk.toString(),'hello'); gateway.revokeOwner(owner); } catch (error) {reject(error);} }); socket.once('close',resolve); socket.on('error',()=>resolve()); socket.write('hello'); });
      req.on('response',()=>reject(new Error('upgrade rejected'))); req.on('error',reject); req.end();
    });
  } finally { await gateway.dispose(); backendSocket?.destroy(); backend.closeAllConnections(); await new Promise<void>(resolve=>backend.close(()=>resolve())); }
});

test('separate browser sessions for one app cannot exchange tickets or cookies', async () => {
  const {port,gateway} = await setup();
  try {
    const first = await session(gateway,port);
    const secondOpen = await gateway.open({appId:id,revision:1,runId:'run'},owner);
    const secondUrl = new URL(secondOpen.url), secondHost = secondUrl.host, secondTicket = secondUrl.hash.slice(1);
    assert.match(first.host, new RegExp(`^${id}-[a-f0-9]{20}\\.apps\\.example\\.com$`));
    assert.notEqual(first.host,secondHost);
    assert.equal((await request(port,first.host,'/__lingxi_remote_bootstrap',{method:'POST',origin:`https://${first.host}`,body:JSON.stringify({ticket:secondTicket})})).status,403);
    const redeemed = await request(port,secondHost,'/__lingxi_remote_bootstrap',{method:'POST',origin:secondUrl.origin,body:JSON.stringify({ticket:secondTicket})});
    assert.equal(redeemed.status,204); assert.ok(redeemed.cookie);
    assert.equal((await request(port,first.host,'/',{cookie:redeemed.cookie})).status,401);
    assert.equal((await request(port,secondHost,'/',{cookie:first.cookie})).status,401);
    assert.equal((await request(port,secondHost,'/__lingxi_remote_invoke',{method:'POST',cookie:redeemed.cookie,origin:`https://${first.host}`,body:'{"method":"info","args":{}}'})).status,403);
    assert.equal((await request(port,secondHost,'/',{cookie:redeemed.cookie})).status,200);
    assert.equal((await request(port,first.host,'/',{cookie:first.cookie})).status,200);
  } finally { await gateway.dispose(); }
});

test('revocation during asynchronous request authorization rejects before dispatch', async () => {
  const {port,gateway,adapter} = await setup();
  try {
    const s=await session(gateway,port);let release!:()=>void, entered!:()=>void, calls=0;
    const enteredPromise=new Promise<void>(resolve=>{entered=resolve;});
    adapter.current=async()=>{entered();await new Promise<void>(resolve=>{release=resolve;});return true;};
    adapter.invoke=async()=>{calls++;return {};};
    const pending=request(port,s.host,'/__lingxi_remote_invoke',{method:'POST',cookie:s.cookie,origin:`https://${s.host}`,body:'{"method":"info","args":{}}'});
    await enteredPromise;gateway.revokeOwner(owner);release();assert.equal((await pending).status,401);assert.equal(calls,0);
  } finally {await gateway.dispose();}
});

test('large Agent input and streamed service upload preserve body and content length', async () => {
  const {port,gateway,adapter}=await setup();let captured=0, seenLength='';
  adapter.invoke=async(_run,method,args)=>{assert.equal(method,'agent.tasks.start');captured=(args.input as string).length;return {ok:true};};
  adapter.resource=async(_run,req)=>{seenLength=req.headers.get('content-length') ?? '';const bytes=await req.arrayBuffer();return Response.json({size:bytes.byteLength});};
  try {
    const s=await session(gateway,port), input='x'.repeat(2*1024*1024);
    const result=await request(port,s.host,'/__lingxi_remote_invoke',{method:'POST',cookie:s.cookie,origin:`https://${s.host}`,body:JSON.stringify({method:'agent.tasks.start',args:{input}})});
    assert.equal(result.status,200);assert.equal(captured,input.length);
    await new Promise<void>((resolve,reject)=>{const req=httpRequest({hostname:'127.0.0.1',port,path:'/__services/api/upload',method:'POST',headers:{host:s.host,cookie:s.cookie,'content-length':input.length}},res=>{let data='';res.on('data',chunk=>{data+=chunk;});res.on('end',()=>{try{assert.equal(res.statusCode,200);assert.equal(JSON.parse(data).size,input.length);resolve();}catch(error){reject(error);}});});req.on('error',reject);req.end(input);});
    assert.equal(seenLength,String(input.length));
  } finally {await gateway.dispose();}
});

test('aggregate buffered upload budget is released after disconnect and revoke', async () => {
  const {port,gateway}=await setup(2*1024*1024);
  const until=async(check:()=>boolean)=>{for(let i=0;i<200;i++){if(check())return;await new Promise(resolve=>setTimeout(resolve,5));}throw new Error('timeout');};
  try {
    const s=await session(gateway,port);
    const partial=()=>{const req=httpRequest({hostname:'127.0.0.1',port,path:'/__lingxi_remote_invoke',method:'POST',headers:{host:s.host,cookie:s.cookie,origin:`https://${s.host}`,'content-type':'application/json','content-length':2*1024*1024}},res=>res.resume());req.on('error',()=>{});req.write('x'.repeat(1536*1024));return req;};
    const first=partial();await until(()=>(gateway as any).bufferedBytes >= 1536*1024);
    const oversized=JSON.stringify({method:'agent.tasks.start',args:{input:'y'.repeat(700*1024)}});
    assert.equal((await request(port,s.host,'/__lingxi_remote_invoke',{method:'POST',cookie:s.cookie,origin:`https://${s.host}`,body:oversized})).status,400);
    first.destroy();await until(()=>(gateway as any).bufferedBytes===0);
    assert.equal((await request(port,s.host,'/__lingxi_remote_invoke',{method:'POST',cookie:s.cookie,origin:`https://${s.host}`,body:oversized})).status,200);
    const second=partial();await until(()=>(gateway as any).bufferedBytes >= 1536*1024);gateway.revokeOwner(owner);await until(()=>(gateway as any).bufferedBytes===0);second.destroy();
  } finally {await gateway.dispose();}
});

test('browser disconnect aborts the upstream resource without revoking its session', async () => {
  const {port,gateway,adapter}=await setup();let entered!:()=>void, aborted!:()=>void;
  const ready=new Promise<void>(resolve=>{entered=resolve;}), stopped=new Promise<void>(resolve=>{aborted=resolve;});
  adapter.resource=async(_run,req)=>{entered();await new Promise<void>(resolve=>{req.signal.addEventListener('abort',()=>{aborted();resolve();},{once:true});});throw Error('aborted');};
  try {
    const s=await session(gateway,port);
    const req=httpRequest({hostname:'127.0.0.1',port,path:'/upload',method:'POST',headers:{host:s.host,cookie:s.cookie,'content-length':'3'}});req.on('error',()=>{});req.end('abc');
    await ready;req.destroy();await stopped;
    adapter.resource=async()=>new Response('alive');
    assert.equal((await request(port,s.host,'/',{cookie:s.cookie})).body,'alive');
  } finally {await gateway.dispose();}
});

test('active ticket and redeemed session keep gateway access alive until revoked', async () => {
  const {port,gateway}=await setup();
  try {
    assert.equal(gateway.hasAccess(),false);
    const opened=await gateway.open({appId:id,revision:1,runId:'run'},owner);
    assert.equal(gateway.hasAccess(),true);
    const url=new URL(opened.url);const result=await request(port,url.host,'/__lingxi_remote_bootstrap',{method:'POST',origin:url.origin,body:JSON.stringify({ticket:url.hash.slice(1)})});
    assert.equal(result.status,204);assert.equal(gateway.hasAccess(),true);
    gateway.revokeOwner(owner);assert.equal(gateway.hasAccess(),false);
  } finally {await gateway.dispose();}
});

test('real service broker streams chunked browser uploads', async () => {
  const backend=createServer((req,res)=>{let size=0;req.on('data',chunk=>{size+=chunk.length;});req.on('end',()=>res.end(JSON.stringify({size,transfer:req.headers['transfer-encoding']})));});
  await new Promise<void>(resolve=>backend.listen(0,'127.0.0.1',resolve));const backendPort=(backend.address() as {port:number}).port;
  const {port,gateway,adapter}=await setup();const broker=new LocalAppServiceBroker();
  adapter.resource=async(run,req)=>broker.fetch(run.appId,run.runId,'api',backendPort,'/upload',req,'',true);
  try {
    const s=await session(gateway,port), chunk='z'.repeat(1024*1024);
    await new Promise<void>((resolve,reject)=>{const req=httpRequest({hostname:'127.0.0.1',port,path:'/__services/api/upload',method:'POST',headers:{host:s.host,cookie:s.cookie}},res=>{let data='';res.on('data',value=>{data+=value;});res.on('end',()=>{try{assert.equal(res.statusCode,200);assert.deepEqual(JSON.parse(data),{size:2*chunk.length,transfer:'chunked'});resolve();}catch(error){reject(error);}});});req.on('error',reject);req.write(chunk);req.end(chunk);});
  } finally {await gateway.dispose();await broker.dispose();backend.closeAllConnections();await new Promise<void>(resolve=>backend.close(()=>resolve()));}
});

test('oversized Agent and service bodies are rejected from declared lengths', async () => {
  const {port,gateway,adapter}=await setup(); adapter.rtc=async()=>new Response('unexpected');
  try {
    const s=await session(gateway,port);
    const send=async(path:string,length:number)=>new Promise<number>((resolve,reject)=>{
      const req=httpRequest({hostname:'127.0.0.1',port,path,method:'POST',headers:{host:s.host,cookie:s.cookie,origin:`https://${s.host}`,'content-type':'application/json','content-length':length}},res=>{res.resume();res.on('end',()=>{resolve(res.statusCode!);req.destroy();});});req.on('error',reject);req.flushHeaders();
    });
    assert.equal(await send('/__lingxi_remote_invoke',72*1024*1024+1),400);
    assert.equal(await send('/__services/api/upload',64*1024*1024+1),400);
    assert.equal(await send('/__lingxi_remote_rtc',1024*1024+1),400);
  } finally {await gateway.dispose();}
});

test('expired unused bootstrap ticket releases process access and signals idle cleanup', async () => {
  const {gateway,adapter}=await setup();const ended:string[]=[];adapter.onAccessEnded=(appId,runId)=>ended.push(`${appId}:${runId}`);
  try {
    const opened=await gateway.open({appId:id,revision:1,runId:'run'},owner);const ticket=new URL(opened.url).hash.slice(1);
    assert.equal(gateway.hasAccess(),true);
    (gateway as any).tickets.get(ticket).expires=Date.now()-1;
    assert.equal(gateway.hasAccess(),false);assert.deepEqual(ended,[`${id}:run`]);
  } finally {await gateway.dispose();}
});
