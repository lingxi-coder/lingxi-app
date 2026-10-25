#!/usr/bin/env node
/**
 * Opt-in real Chrome extension/native-relay acceptance. Never uses a personal profile.
 * From apps/electron:
 * node --import ../../packages/bridge-client/node_modules/tsx/dist/loader.mjs \
 *   scripts/browser-real-acceptance.mjs --chrome /path/to/chrome \
 *   --relay /path/to/bridge-server --receipt /path/to/receipt.json
 * LINGXI_BROWSER_ACCEPTANCE_CHROME / _RELAY / _OUTPUT may replace CLI paths.
 * The relay must implement --browser-native-host; no model credentials are used.
 */
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';

function options() {
  const result = {
    chrome: process.env.LINGXI_BROWSER_ACCEPTANCE_CHROME,
    relay: process.env.LINGXI_BROWSER_ACCEPTANCE_RELAY,
    output: process.env.LINGXI_BROWSER_ACCEPTANCE_OUTPUT,
    receipt: undefined,
    scale: '2',
  };
  const args = process.argv.slice(2);
  for (let index = 0; index < args.length; index++) {
    const key = args[index]?.replace(/^--/, '');
    if (!Object.hasOwn(result, key) || !args[index]?.startsWith('--') || !args[index+1]) throw new Error('Use --chrome <executable> --relay <executable> [--output <new-directory>] [--receipt <json>] [--scale <1|2>]');
    result[key] = args[++index];
  }
  if (!result.chrome || !result.relay) throw new Error('The Chrome and native relay executable paths are required');
  if (!['1','2'].includes(result.scale)) throw new Error('Scale must be 1 or 2');
  result.chrome = resolve(result.chrome); result.relay = resolve(result.relay);
  if (!existsSync(result.chrome) || !existsSync(result.relay)) throw new Error('A required acceptance executable is missing');
  return result;
}
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const wait = milliseconds => new Promise(resolve => setTimeout(resolve,milliseconds));
function cleanError(error,opts) {
  let message = error instanceof Error ? error.message : String(error);
  for (const path of [opts.chrome,opts.relay,opts.output,opts.receipt].filter(Boolean)) message = message.replaceAll(path,'<configured-path>');
  return message.replace(/\/Users\/[^\s'"<>]+/g,'<user-path>').replace(/\/(?:private\/)?var\/folders\/[^\s'"<>]+/g,'<temporary-path>').slice(0,1000);
}
function privateEnvironment() {
  return Object.fromEntries(['PATH','HOME','TMPDIR','LANG','LC_ALL','TZ'].flatMap(key => process.env[key] === undefined ? [] : [[key,process.env[key]]]));
}
async function listen(server) {
  await new Promise((resolve,reject) => { server.once('error',reject); server.listen(0,'127.0.0.1',() => { server.off('error',reject); resolve(); }); });
  return `http://127.0.0.1:${server.address().port}`;
}
async function closeServer(server) {
  server.closeAllConnections();
  await new Promise(resolve => server.close(resolve));
}
function writeReceipt(path,receipt) {
  mkdirSync(dirname(path),{ recursive:true });
  const temporary = `${path}.${randomUUID()}.tmp`;
  writeFileSync(temporary,`${JSON.stringify(receipt,null,2)}\n`,{ mode:0o600 }); renameSync(temporary,path);
}

async function run(opts) {
  // Node's repository tsx loader is intentionally supplied explicitly by the caller.
  const { BrowserIntegration } = await import('../src/main/browserIntegration.ts');
  const root = opts.output ? resolve(opts.output) : mkdtempSync(join(tmpdir(),'lingxi-browser-real-'));
  if (opts.output) { if (existsSync(root)) throw new Error('The acceptance output directory must be new'); mkdirSync(root,{ recursive:true,mode:0o700 }); }
  const profile = join(root,'profile'); mkdirSync(profile,{ mode:0o700 });
  const extension = fileURLToPath(new URL('../browser-extension',import.meta.url));
  const manifest = join(profile,'NativeMessagingHosts','com.lingxi.browser.json');
  assert.equal(existsSync(manifest),false);
  const sourceRoot = fileURLToPath(new URL('../../../',import.meta.url));
  const sources = [
    'apps/electron/src/main/browserIntegration.ts',
    'apps/electron/browser-extension/worker.js',
    'apps/electron/browser-extension/policy.js',
    'apps/electron/browser-extension/manifest.json',
    'apps/bridge-server/src/browser_native_host.rs',
    'apps/electron/scripts/browser-real-acceptance.mjs',
  ];
  const receipt = {
    status:'running',
    runAtUtc:new Date().toISOString(),
    chromeVersion:execFileSync(opts.chrome,['--version'],{ encoding:'utf8',timeout:5000,maxBuffer:4096,env:privateEnvironment() }).trim().slice(0,128),
    requestedDeviceScaleFactor:Number(opts.scale),
    profile:'new temporary profile; no personal browser profile or account',
    nativeRegistration:'inside temporary profile only',
    extension:'canonical unpacked source, without instrumentation',
    nativeRelaySha256:hash(readFileSync(opts.relay)),
    sourceSha256:Object.fromEntries(sources.map(path => [path,hash(readFileSync(join(sourceRoot,path)))])),
    checks:[],
    signedPackageAcceptance:false,
    alignmentScope:'LingXi adapter behavior against the Claude Code 2.1.310 reference; not full Anthropic extension protocol equality',
    limitations:[
      'Downloads requested through LingXi are bounded same-origin downloads; autonomous website blob/data downloads are not controlled browser-wide.',
      'This acceptance uses a real Chrome extension and native relay, not a signed Desktop package.',
    ],
  };
  let requestsA = 0; let requestsB = 0;
  const serverB = createServer((_request,response) => { requestsB++; response.end('Denied origin'); });
  const originB = await listen(serverB);
  let originA;
  const serverC = createServer((_request,response) => {
    response.writeHead(200,{ 'content-type':'text/html' });
    response.end(`<script>addEventListener('message',event=>{if(['nested-ready','nested-attempted'].includes(event.data))parent.postMessage(event.data,'*');if(event.data==='trigger-nested-navigation')frames[0].postMessage('trigger-nested-navigation','*')})</script><iframe src="${originA}/inner"></iframe>`);
  });
  await listen(serverC);
  // localhost vs 127.0.0.1 forces an out-of-process site boundary, unlike ports alone.
  const originC = `http://localhost:${serverC.address().port}`;
  const serverA = createServer((request,response) => {
    requestsA++;
    if (request.url === '/redirect') { response.writeHead(302,{ location:originB+'/private' }); response.end(); return; }
    response.writeHead(200,{ 'content-type':'text/html' });
    if (request.url === '/frames') { response.end(`<script>window.nestedReady=false;window.nestedAttempted=false;addEventListener('message',event=>{if(event.data==='nested-ready')window.nestedReady=true;if(event.data==='nested-attempted')window.nestedAttempted=true})</script><iframe src="${originC}/middle"></iframe>`); return; }
    if (request.url === '/inner') { response.end(`<script>parent.postMessage('nested-ready','*');addEventListener('message',event=>{if(event.data==='trigger-nested-navigation'){parent.postMessage('nested-attempted','*');location.href=${JSON.stringify(originB+'/nested-private')}}})</script>Nested frame`); return; }
    response.end('<!doctype html><title>LingXi native browser acceptance</title><style>body{background:#eef4fb}input{position:absolute;left:20px;top:20px;width:220px;height:40px;font-size:18px}h1{position:absolute;top:100px;font:24px sans-serif}</style><input id="entry" value=""><h1>LingXi native extension acceptance</h1>');
  });
  originA = await listen(serverA);
  let policy = { revision:randomUUID(),enabled:true,apps:[],browser:{ history:'ask',siteTools:true,fullCdp:true,defaultBrowsing:'ask',defaultDownloads:'ask',sites:[{ origin:originB,browsing:'deny',downloads:'deny' },{ origin:originC,browsing:'allow',downloads:'deny' }] } };
  const service = new BrowserIntegration({ policy:()=>policy,descriptorPath:join(root,'connection.json'),nativeHostPath:join(root,'native-host'),bridgeExecutable:opts.relay,extensionDirectory:extension,nativeManifestPath:manifest });
  let browser;
  let stage = 'initialization';
  let ownManifest;
  const stderrHash = createHash('sha256'); let stderrBytes = 0;
  try {
    const agent = await service.start(); await service.install(); ownManifest = readFileSync(manifest);
    stage = 'native_handshake';
    browser = spawn(opts.chrome,[`--user-data-dir=${profile}`,`--disable-extensions-except=${extension}`,`--load-extension=${extension}`,`--force-device-scale-factor=${opts.scale}`,'--site-per-process','--headless','--no-sandbox','--no-first-run','--no-default-browser-check','--disable-background-networking','--password-store=basic','--use-mock-keychain','about:blank'],{ detached:true,stdio:['ignore','ignore','pipe'],env:privateEnvironment() });
    browser.stderr.on('data',chunk => { stderrBytes += chunk.length; stderrHash.update(chunk); });
    const launchFailure = new Promise((_,reject) => browser.once('error',() => reject(new Error('Chrome failed to launch'))));
    await Promise.race([launchFailure,(async()=>{ for (let index=0;index<100&&!service.status().connected;index++) await wait(200); assert.equal(service.status().connected,true,'The real extension did not complete its native handshake'); })()]);
    receipt.checks.push('real_native_handshake');
    const sessionId = `real-${randomUUID()}`;
    const call = async input => {
      const response = await fetch(agent.url+'/agent',{ method:'POST',headers:{ authorization:`Bearer ${agent.token}`,'content-type':'application/json' },body:JSON.stringify({ sessionId,...input }),signal:AbortSignal.timeout(40_000) });
      return { status:response.status,data:await response.json() };
    };
    stage = 'navigation_approval';
    const navigation = { action:'navigate',url:originA+'/page' };
    const approval = await call(navigation); assert.equal(approval.status,200); assert.ok(approval.data.approval); assert.equal(requestsA,0);
    receipt.checks.push('ask_before_navigation');
    const granted = await call({ ...navigation,approvalNonce:approval.data.approval.nonce,approved:true });
    assert.equal(granted.status,200,granted.data.error); assert.ok(granted.data.result.tabId);
    receipt.checks.push('approved_navigation');
    stage = 'screenshot';
    const captured = await call({ action:'screenshot' }); assert.equal(captured.status,200,captured.data.error);
    const png = Buffer.from(captured.data.result.image.data,'base64'); assert.equal(png.subarray(0,8).toString('hex'),'89504e470d0a1a0a');
    const viewport = captured.data.result.viewport; assert.ok(viewport.width>0&&viewport.height>0);
    receipt.screenshot = { bytes:png.length,sha256:hash(png),imageWidth:png.readUInt32BE(16),imageHeight:png.readUInt32BE(20),viewport };
    writeFileSync(join(root,'browser.png'),png,{ mode:0o600 });
    receipt.checks.push('actual_page_screenshot_and_css_viewport');
    stage = 'click_and_type';
    const imagePoint = { x:80*receipt.screenshot.imageWidth/viewport.width,y:40*receipt.screenshot.imageHeight/viewport.height };
    const click = await call({ action:'click',x:imagePoint.x*viewport.width/receipt.screenshot.imageWidth+viewport.offsetX,y:imagePoint.y*viewport.height/receipt.screenshot.imageHeight+viewport.offsetY });
    assert.equal(click.status,200,click.data.error);
    const typed = await call({ action:'type',text:'Aligned 2.1.310' }); assert.equal(typed.status,200,typed.data.error);
    const actual = await call({ action:'cdp',method:'Runtime.evaluate',params:{ expression:"document.getElementById('entry').value",returnByValue:true } });
    assert.equal(actual.status,200,actual.data.error); assert.equal(actual.data.result.result.value,'Aligned 2.1.310');
    receipt.checks.push('actual_click_and_type_with_image_to_css_mapping');
    stage = 'site_deny';
    const denied = await call({ action:'navigate',url:originB+'/private' }); assert.equal(denied.status,400); assert.equal(requestsB,0);
    receipt.checks.push('explicit_site_deny');
    stage = 'redirect_fence';
    const redirected = await call({ action:'navigate',url:originA+'/redirect' }); assert.equal(redirected.status,400); assert.equal(requestsB,0);
    receipt.checks.push('cross_origin_redirect_blocked_before_destination');
    stage = 'nested_cross_site_iframe_guard';
    const framed = await call({ action:'navigate',url:originA+'/frames' }); assert.equal(framed.status,200,framed.data.error);
    let nestedReady = false;
    for (let attempt=0;attempt<50;attempt++) {
      const probe = await call({ action:'cdp',method:'Runtime.evaluate',params:{ expression:'window.nestedReady',returnByValue:true } });
      assert.equal(probe.status,200,probe.data.error);
      if (probe.data.result.result.value === true) { nestedReady = true; break; }
      await wait(100);
    }
    assert.equal(nestedReady,true,'Nested cross-site iframe did not finish loading');
    receipt.checks.push('nested_cross_site_iframe_loaded');
    const triggered = await call({ action:'cdp',method:'Runtime.evaluate',params:{ expression:"frames[0].postMessage('trigger-nested-navigation','*');true",returnByValue:true } });
    assert.equal(triggered.status,200,triggered.data.error);
    let nestedAttempted = false;
    for (let attempt=0;attempt<50;attempt++) {
      const probe = await call({ action:'cdp',method:'Runtime.evaluate',params:{ expression:'window.nestedAttempted',returnByValue:true } });
      assert.equal(probe.status,200,probe.data.error);
      if (probe.data.result.result.value === true) { nestedAttempted = true; break; }
      await wait(100);
    }
    assert.equal(nestedAttempted,true,'Nested iframe never attempted the denied navigation');
    await wait(750); assert.equal(requestsB,0,'A nested frame reached the denied origin');
    receipt.checks.push('nested_cross_site_iframe_navigation_denied_before_destination');
    const restored = await call(navigation); assert.equal(restored.status,200,restored.data.error);
    stage = 'revocation';
    policy = { ...policy,revision:randomUUID(),enabled:false }; service.policyChanged();
    const revoked = await call({ action:'type',text:'should not appear' }); assert.equal(revoked.status,400); assert.match(revoked.data.error,/turned off|permissions changed/i);
    receipt.checks.push('revocation_blocks_input');
    stage = 'connection_removal';
    await service.remove(); assert.equal(service.status().connected,false); assert.equal(existsSync(manifest),false);
    receipt.checks.push('native_connection_removal');
    receipt.status = 'passed';
  } catch (error) {
    receipt.status = 'failed'; receipt.failure = { stage,message:cleanError(error,opts) };
  } finally {
    // The child owns a new process group. No existing Chrome process is targeted.
    if (browser?.pid) {
      try { process.kill(-browser.pid,'SIGTERM'); } catch { /* Already exited. */ }
      await Promise.race([new Promise(resolve => browser.once('exit',resolve)),wait(5000)]);
      try { process.kill(-browser.pid,'SIGKILL'); } catch { /* Already exited. */ }
    }
    if (existsSync(manifest)&&ownManifest&&readFileSync(manifest).equals(ownManifest)) await service.remove().catch(()=>{ receipt.cleanupFailure = true; });
    await service.close().catch(()=>{ receipt.cleanupFailure = true; });
    await Promise.all([closeServer(serverA),closeServer(serverB),closeServer(serverC)]);
    rmSync(profile,{ recursive:true,force:true });
    receipt.finishedAtUtc = new Date().toISOString();
    receipt.cleanup = { profileRemoved:!existsSync(profile),nativeManifestRemoved:!existsSync(manifest) };
    receipt.browserStderr = { bytes:stderrBytes,sha256:stderrHash.digest('hex') };
    writeReceipt(join(root,'receipt.json'),receipt);
    if (opts.receipt) writeReceipt(resolve(opts.receipt),receipt);
  }
  console.log(JSON.stringify({ status:receipt.status,checks:receipt.checks.length,cleanup:receipt.cleanup,...(receipt.failure ? { failure:receipt.failure } : {}) }));
  if (receipt.status !== 'passed' || receipt.cleanupFailure) process.exitCode = 1;
}

try { await run(options()); }
catch (error) { console.error(JSON.stringify({ status:'failed',error:error instanceof Error ? error.message.replace(/\/Users\/[^\s'"<>]+/g,'<user-path>').slice(0,1000) : 'Acceptance setup failed' })); process.exitCode = 1; }
