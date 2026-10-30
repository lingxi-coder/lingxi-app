import { LocalAppCoreClient } from '../../src/main/local-apps/coreClient';
import { resolveServerBin } from '../../src/main/bridgeDiscovery';
import { app, BrowserWindow, protocol, dialog, session, type Session } from 'electron';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { LocalAppHost } from '../../src/main/local-apps/host';
import { LOCAL_APP_SERVICE_COOKIE_ORIGIN } from '../../src/main/local-apps/serviceCookies';

const root = process.env.LINGXI_TEST_USER_DATA!;
process.on('uncaughtException', error => { process.stderr.write(`Uncaught: ${error.stack}\n`); app.exit(1); });
process.on('unhandledRejection', error => { process.stderr.write(`Unhandled: ${String(error)}\n`); app.exit(1); });
app.setPath('userData', join(root, 'profile'));
protocol.registerSchemesAsPrivileged([{ scheme: 'lingxi-app', privileges: { standard: true, secure: true, supportFetchAPI: true, corsEnabled: true, stream: true, allowServiceWorkers: true } }]);
app.commandLine.appendSwitch('disable-features', 'WebMCP');
app.on('window-all-closed', () => undefined);
const originalDialog = dialog.showMessageBox;
for (const event of ['before-quit', 'will-quit', 'quit'] as const) app.on(event, () => process.stderr.write(`App lifecycle: ${event}\n`));
async function until(check: () => Promise<unknown>) { for (let i = 0; i < 200; i++) { if (await check()) return; await delay(30); } throw new Error('Timed out waiting for fixture'); }
const html = (version: number, displayedVersion = version) => `<!doctype html><template id="inert-host-resource-decoys"><template><!-- </template><head> --><script>window.__fakeTemplateClose='</template><head><script src="/__lingxi_webmcp.js">'</script><head><script src="/__lingxi_webmcp.js"></script><script src="/__lingxi_local_app_sdk.js"></script></head></template></template><noscript><head><script src="/__lingxi_webmcp.js"></script></head></noscript><!-- <head><script src="/__lingxi_webmcp.js"></script><script src="/__lingxi_local_app_sdk.js"></script></head> --><script\u00a0src="/__lingxi_webmcp.js"></script><script\u000Bsrc="/__lingxi_webmcp.js"></script>${version === 1 ? '<base href="data:text/html,"><base href="https://example.invalid/assets/"><script src="/__lingxi_webmcp.js"></script><script src="/__lingxi_local_app_sdk.js"></script>' : version === 2 ? '<base href="lingxi-app://app/custom/"><script src="/__lingxi_webmcp.js"></script>' : '<base href="http://127.0.0.1:9/ignored/"><script src="/__lingxi_webmcp.js"></script>'}<script type="text/javascript\u00a0" src="/__lingxi_webmcp.js"></script><script type="text/javascript\u000B" src="/__lingxi_webmcp.js"></script><script><!--<script></script><head><script src="/__lingxi_webmcp.js"></script><script src="/__lingxi_local_app_sdk.js"></script></script><script type="application/json" src="/__lingxi_webmcp.js"></script><script async src="/__lingxi_webmcp.js"></script><script defer src="/__lingxi_webmcp.js"></script><script type="module" src="/__lingxi_webmcp.js"></script><script src="/missing.js" src="/__lingxi_webmcp.js"></script><div data-fake-head="<head>" data-fake-resources="/__lingxi_webmcp.js __lingxi_local_app_sdk.js"></div><script>window.__fakeHead='<head>';window.__fakeResources='/__lingxi_webmcp.js /__lingxi_local_app_sdk.js'</script><style>.fake::after{content:"<head>"}body{font-family:system-ui;background:#fbf9f3;padding:48px;color:#284337}h1{font-size:32px}.chars{display:flex;gap:20px}button{font-size:56px;border:0;border-radius:20px;padding:30px;background:#dff0df;cursor:pointer}</style><meta charset="utf-8"><title>识字花园</title><script id="app-start">window.__webmcpAtAppStart=Boolean(document.modelContext);window.__localAppSdkAtAppStart=Boolean(window.localApp)</script><head></head><h1>识字花园 · v${displayedVersion}</h1><p>每天认识几个新朋友</p><div class="chars"><button onclick="localStorage.count=String(Number(localStorage.count||0)+1)">山</button><button>水</button><button>花</button></div>`;
let host: LocalAppHost;
const trace = (message: string) => process.stderr.write(`LocalApp fixture: ${message}\n`);
async function main() {
  await app.whenReady();
  trace('await app.whenReady();');
  const library = new BrowserWindow({ show: true, width: 1100, height: 800, webPreferences: { preload: join(root, 'preload.cjs'), sandbox: true, contextIsolation: true, nodeIntegration: false } });
  const configHome = join(root, 'config');
  const trustedSessions = new Map<string, { projectPath: string; mode: 'code' | 'chat' }>();
  const resolveAuthoringBinding = async (_appId: string | undefined, ref: { projectPath: string; sessionId: string }) => {
    const source = trustedSessions.get(ref.sessionId);
    if (!source || source.mode !== 'code' || source.projectPath !== ref.projectPath) throw new Error('Choose a Code session with its dedicated source Project');
    const canonical = await fs.realpath(source.projectPath);
    return { sourceDirectory: canonical, projectRef: { kind: 'desktopPath' as const, path: canonical }, authoringSessionId: ref.sessionId };
  };
  const createHost = () => new LocalAppHost({ configHome, core: new LocalAppCoreClient(resolveServerBin(), configHome), preload: join(root, 'localApp.cjs'),
    nodeExecutable: process.execPath, mainWindow: () => library, edit: async () => undefined, resolveAuthoringBinding });
  let discovery: { port: number; token: string };
  let url: string;
  const startHost = async () => {
    host = createHost();
    await host.initialize();
    host.registerMainWindow(library, process.env.LINGXI_TEST_LIBRARY_URL!);
    discovery = JSON.parse(await fs.readFile(join(configHome, 'desktop-local-app-host.json'), 'utf8'));
    url = `http://127.0.0.1:${discovery.port}/local-app`;
  };
  await startHost();
  trace('await host.initialize();');
  await library.loadURL(process.env.LINGXI_TEST_LIBRARY_URL!);
  await library.webContents.executeJavaScript(`(()=>{window.__localAppChangeCount=0;window.lingxi.localApps.onChanged(()=>window.__localAppChangeCount++);return true})()`);
  trace('await library.loadURL(process.env.LINGXI_TEST_LIBRARY_URL!);');
  assert.equal((await fetch(url, { method: 'POST', body: '{}' })).status, 403);
  const toolResult = async (input: object) => {
    const response = await fetch(url, { method: 'POST', headers: { Authorization: `Bearer ${discovery.token}` }, body: JSON.stringify(input) });
    return { status: response.status, value: await response.json() };
  };
  const tool = async (input: object) => { const result = await toolResult(input); assert.equal(result.status, 200, JSON.stringify(result.value)); return result.value; };
  const nativeSource = async (name: string, mode: 'code' | 'chat' = 'code') => {
    const requestedPath = join(root, 'source-projects', name);
    await fs.mkdir(requestedPath, { recursive: true, mode: 0o700 });
    const projectPath = await fs.realpath(requestedPath);
    const sessionId = randomUUID();
    trustedSessions.set(sessionId, { projectPath, mode });
    return { projectPath, sessionId };
  };
  const publishUpdate = async (id: string, input: Record<string, unknown>) => {
    const current = await tool({ action: 'read', id });
    assert.equal(current.id, id, 'read must return the app being updated');
    assert.ok(Number.isSafeInteger(current.expectedRevision), 'publish updates must use the revision from the latest host read');
    const row = await host.store.get(id);
    const authoring = row.authoring;
    const source = authoring?.authoringSessionId && authoring?.projectRef?.kind === 'desktopPath'
      ? { source_session: { projectPath: authoring.projectRef.path, sessionId: authoring.authoringSessionId }, source_path: authoring.sourceDirectory }
      : {};
    return await tool({ ...input, ...source, action: 'publish', id, expectedRevision: current.expectedRevision });
  };
  const firstSource = await nativeSource('first-app');
  const firstBinding = await host.request({ action: 'prepareAuthoring', session: firstSource }) as any;
  assert.equal(firstBinding.authoringSessionId, firstSource.sessionId);
  assert.equal(firstBinding.sourceDirectory, firstSource.projectPath);
  assert.deepEqual(firstBinding.projectRef, { kind: 'desktopPath', path: firstSource.projectPath });
  const nonCodeSource = await nativeSource('chat-not-code', 'chat');
  await assert.rejects(host.request({ action: 'prepareAuthoring', session: nonCodeSource }), /Code session/);
  const appProject = join(host.store.importsRoot, 'native-created-source');
  await fs.writeFile(join(firstSource.projectPath, 'lingxi-app.json'), JSON.stringify({ schema: 2, targets: [{ platform: 'macos', environment: 'native' }],
    name: '识字花园', summary: '每天学习汉字，记录进步。', entry: { type: 'static', path: 'index.html' } }));
  await fs.writeFile(join(firstSource.projectPath, 'index.html'), html(1));
  await fs.rm(appProject, { recursive: true, force: true });
  await fs.cp(firstSource.projectPath, appProject, { recursive: true });
  const sourceSession = { projectPath: firstSource.projectPath, sessionId: firstSource.sessionId };
  const invalidPrecommit = await toolResult({ action: 'publish', name: '识字花园', staged_path: firstSource.projectPath,
    source_path: firstSource.projectPath, source_session: sourceSession });
  assert.equal(invalidPrecommit.status, 400, 'pre-commit staging failure is returned to the tool');
  assert.match(invalidPrecommit.value.error, /staging directory/);
  const pendingBeforeRestart = JSON.parse(await fs.readFile(join(host.store.root, 'authoring-pending.v1.json'), 'utf8'));
  assert.equal(pendingBeforeRestart.entries.find((entry: any) => entry.authoring.authoringSessionId === firstSource.sessionId)?.state, 'pending');
  await host.dispose();
  await startHost();
  const first = await tool({ action: 'publish', name: '识字花园', summary: '每天学习汉字，记录进步。',
    staged_path: appProject, source_path: firstSource.projectPath, source_session: sourceSession });
  const firstRow = await host.store.get(first.id);
  assert.equal(firstRow.authoring?.authoringSessionId, firstSource.sessionId, 'the first release and trusted Code binding commit together');
  assert.equal(firstRow.authoring?.appId, first.id, 'Core assigns authoring ownership to the new app');
  assert.equal(JSON.parse(await fs.readFile(join(host.store.root, 'authoring-pending.v1.json'), 'utf8')).entries
    .find((entry: any) => entry.authoring.authoringSessionId === firstSource.sessionId)?.state, 'consumed');
  await until(() => library.webContents.executeJavaScript(`Boolean(document.querySelector('.local-app-tile-open'))`));

  const racingSource = await nativeSource('concurrent-publish');
  const racingBinding = await host.request({ action: 'prepareAuthoring', session: racingSource }) as any;
  assert.equal(racingBinding.authoringSessionId, racingSource.sessionId);
  await fs.writeFile(join(racingSource.projectPath, 'lingxi-app.json'), JSON.stringify({ schema: 2,
    targets: [{ platform: 'macos', environment: 'native' }], name: 'Concurrent publish probe',
    entry: { type: 'static', path: 'index.html' } }));
  await fs.writeFile(join(racingSource.projectPath, 'index.html'), '<!doctype html><title>Concurrent publish probe</title>');
  const racingStage = join(host.store.importsRoot, 'concurrent-publish');
  await fs.cp(racingSource.projectPath, racingStage, { recursive: true });
  const racingRequest = { action: 'publish', name: 'Concurrent publish probe', staged_path: racingStage,
    source_path: racingSource.projectPath, source_session: racingSource };
  const racingResults = await Promise.all([toolResult(racingRequest), toolResult(racingRequest)]);
  const racingSuccess = racingResults.find(result => result.status === 200);
  const racingRejected = racingResults.find(result => result.status === 400);
  assert.ok(racingSuccess, 'one serialized first publication should commit');
  assert.ok(racingRejected, 'the parallel duplicate first publication should be rejected');
  assert.match(racingRejected.value.error, new RegExp(racingSuccess.value.id));
  assert.equal((await host.list()).filter(row => row.authoring?.sourceDirectory === racingSource.projectPath).length, 1,
    'parallel model publishes cannot create duplicate apps for one native source');
  await host.store.core.request({ op: 'remove', appId: racingSuccess.value.id });

  await library.webContents.executeJavaScript(`document.querySelector('.local-app-tile-open').click()`);
  const guest = () => BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${first.id}/`));
  await until(async () => guest() && await guest()!.webContents.executeJavaScript(`Boolean(window.localApp && document.querySelector('.chars'))`));
  let window = guest()!;
  assert.equal(await window.webContents.executeJavaScript('window.__webmcpAtAppStart'), true, 'WebMCP loads before the app script');
  assert.equal(await window.webContents.executeJavaScript('window.__localAppSdkAtAppStart'), true, 'the Local App SDK loads before the app script');
  assert.equal(await window.webContents.executeJavaScript('Array.from(document.scripts).filter(script=>script.hasAttribute("data-lingxi-webmcp")).length'), 0, 'an existing blocking WebMCP script is reused without a duplicate');
  assert.equal(await window.webContents.executeJavaScript('Array.from(document.scripts).filter(script=>script.getAttribute("src")==="/__lingxi_local_app_sdk.js").length'), 1, 'a Local App SDK-looking comment, attribute, or inline string does not suppress the real host script');
  const webmcpResult = await window.webContents.executeJavaScript(`(async()=> {
    const context=document.modelContext;
    const registration=new AbortController();
    await context.registerTool({
      name:'local-app.echo',
      description:'Echo a value from the Local App page',
      inputSchema:{type:'object',properties:{value:{type:'string'}}},
      execute(input){return {echo:input.value};}
    }, {signal:registration.signal});
    const tool=(await context.getTools()).find(item=>item.name==='local-app.echo');
    if(!tool) throw new Error('registered Local App tool was not discovered');
    const result=await context.executeTool(tool,{value:'page input'});
    registration.abort();
    const remains=(await context.getTools()).some(item=>item.name==='local-app.echo');
    const resource=await fetch(location.origin+'/__lingxi_webmcp.js');
    return {result, remains, resourceStatus:resource.status, resourceType:resource.headers.get('content-type')};
  })()`);
  assert.deepEqual(webmcpResult, { result: '{"echo":"page input"}', remains: false, resourceStatus: 200, resourceType: 'text/javascript; charset=utf-8' });
  assert.equal(await window.webContents.executeJavaScript(`typeof require + ':' + typeof window.lingxi`), 'undefined:undefined');
  assert.equal(await window.webContents.executeJavaScript(`window.localApp.clipboard.read().then(()=>false,()=>true)`), true);
  await window.webContents.executeJavaScript(`localStorage.count='7'`);
  await host.open(first.id);
  assert.equal(guest()!.id, window.id, 'opening the active revision reuses its window');
  const previousWindow = window;
  await publishUpdate(first.id, { html: html(2) });
  await host.open(first.id);
  window = guest()!;
  assert.equal(await window.webContents.executeJavaScript('window.__webmcpAtAppStart'), true, 'the injected WebMCP script runs before app code when the HTML has no explicit head and contains a later ignored head tag');
  assert.equal(await window.webContents.executeJavaScript('Array.from(document.scripts).filter(script=>script.hasAttribute("data-lingxi-webmcp")).length'), 1, 'automatic WebMCP injection adds exactly one blocking loader');
  assert.equal(await window.webContents.executeJavaScript('window.__localAppSdkAtAppStart'), true, 'the Local App SDK loads before app code after revision replacement');
  assert.notEqual(window.id, previousWindow.id, 'opening an updated revision replaces the old window');
  assert.equal(previousWindow.isDestroyed(), true, 'the previous revision window is destroyed after replacement');
  assert.match(await window.webContents.executeJavaScript(`document.querySelector('h1').textContent`), /v2/);
  assert.equal(await window.webContents.executeJavaScript(`localStorage.count`), '7');
  assert.equal(
    await window.webContents.executeJavaScript(`document.modelContext.getTools().then(tools=>tools.some(tool=>tool.name==='local-app.echo'))`),
    false,
    'a replacement revision starts with an empty WebMCP context',
  );

  assert.equal(await window.webContents.executeJavaScript('document.baseURI'), 'lingxi-app://app/custom/', 'the custom scheme remains the active document base');

  await publishUpdate(first.id, { html: html(3, 2) });
  await host.open(first.id);
  window = guest()!;
  assert.equal(await window.webContents.executeJavaScript('window.__webmcpAtAppStart'), true, 'a valid cross-origin HTTP base cannot suppress early WebMCP injection');
  assert.equal(await window.webContents.executeJavaScript('Array.from(document.scripts).filter(script=>script.hasAttribute("data-lingxi-webmcp")).length'), 1, 'the HTTP base receives one injected WebMCP loader');
  assert.match(await window.webContents.executeJavaScript(`document.querySelector('h1').textContent`), /v2/);

  // A delayed closed callback from an older window must not stop a reopen.
  const originalGet = host.store.get.bind(host.store);
  const originalClear = (host as any).clearCodeCache.bind(host);
  for (const { cancel, background } of [{ cancel: false, background: false }, { cancel: true, background: false }, { cancel: true, background: true }]) {
    await host.request({ action: 'background', id: first.id, enabled: background });
    let releaseClose!: () => void, releaseCache!: () => void;
    let closeStarted!: () => void, cacheStarted!: () => void;
    const closeGate = new Promise<void>(resolve => { releaseClose = resolve; });
    const cacheGate = new Promise<void>(resolve => { releaseCache = resolve; });
    const closing = new Promise<void>(resolve => { closeStarted = resolve; });
    const caching = new Promise<void>(resolve => { cacheStarted = resolve; });
    let delayClose = true;
    let cacheEntered = false;
    host.store.get = async id => { const wait = id === first.id && delayClose; if (wait) delayClose = false; const row = await originalGet(id); if (wait) { closeStarted(); await closeGate; } return row; };
    (host as any).clearCodeCache = async (id: string) => { if (id === first.id) { cacheEntered = true; cacheStarted(); await cacheGate; } await originalClear(id); };
    window.close(); await closing;
    const abort = new AbortController();
    const reopening = host.open(first.id, false, false, abort.signal);
    const completed = cancel ? assert.rejects(reopening) : reopening;
    await delay(60);
    assert.equal(cacheEntered, false, 'reopening waits for the previous window cleanup');
    releaseClose(); await caching;
    const reopeningPhase = host.runtime.phase(first.id);
    if (cancel) abort.abort();
    releaseCache(); await completed;
    host.store.get = originalGet; (host as any).clearCodeCache = originalClear;
    assert.equal(reopeningPhase, 'running');
    if (cancel && !background) await until(async () => host.runtime.phase(first.id) === 'stopped');
    assert.equal(host.runtime.phase(first.id), cancel && !background ? 'stopped' : 'running');
    if (cancel) await host.open(first.id);
    window = guest()!;
  }
  const holdCodeCache = (id: string) => {
    let enter!: () => void, release!: () => void;
    const entered = new Promise<void>(resolve => { enter = resolve; });
    const gate = new Promise<void>(resolve => { release = resolve; });
    let first = true;
    (host as any).clearCodeCache = async (appId: string) => {
      if (appId === id && first) { first = false; enter(); await gate; }
      await originalClear(appId);
    };
    return { entered, release, restore: () => { release(); (host as any).clearCodeCache = originalClear; } };
  };
  // A completed Stop/Revoke must release the canceled opening before a caller
  // can immediately open again. Rejoining its aborted promise loses the reopen.
  for (const action of ['stop', 'revoke']) {
    await host.request({ action: 'background', id: first.id, enabled: false });
    window.close(); await until(async () => host.runtime.phase(first.id) === 'stopped');
    const cache = holdCodeCache(first.id);
    try {
      const canceled = assert.rejects(host.open(first.id), /abort|cancel/i);
      await cache.entered;
      const reopened = host.request({ action, id: first.id }).then(() => host.open(first.id)).then(() => ({ error: undefined }), error => ({ error }));
      await until(async () => host.runtime.phase(first.id) === 'stopped');
      await delay(0); cache.release();
      await canceled;
      const result = await reopened;
      assert.equal(result.error, undefined, `${action} must settle the old opening before an immediate reopen`);
      assert.equal(host.runtime.phase(first.id), 'running');
      window = guest()!;
      assert.match(await window.webContents.executeJavaScript(`document.querySelector('h1').textContent`), /v2/);
    } finally { cache.restore(); }
  }
  // Cancellation can already be cleaning up when Stop arrives. Its completion
  // must include that cleanup, not only the rejected window-loading promise.
  window.close(); await until(async () => host.runtime.phase(first.id) === 'stopped');
  const cleanupCache = holdCodeCache(first.id);
  const originalStop = host.runtime.stop.bind(host.runtime);
  let enterCleanup!: () => void, finishCleanup!: () => void;
  const cleanupStarted = new Promise<void>(resolve => { enterCleanup = resolve; });
  const cleanupGate = new Promise<void>(resolve => { finishCleanup = resolve; });
  let holdCleanup = true;
  host.runtime.stop = async id => {
    if (id === first.id && holdCleanup) { holdCleanup = false; enterCleanup(); await cleanupGate; }
    await originalStop(id);
  };
  try {
    const abort = new AbortController();
    const canceled = assert.rejects(host.open(first.id, false, false, abort.signal), /abort|cancel/i);
    await cleanupCache.entered;
    abort.abort(); cleanupCache.release(); await cleanupStarted;
    const reopened = host.request({ action: 'stop', id: first.id }).then(() => host.open(first.id)).then(() => ({ error: undefined }), error => ({ error }));
    await until(async () => host.runtime.phase(first.id) === 'stopped');
    await delay(0); finishCleanup(); await canceled;
    assert.equal((await reopened).error, undefined, 'Stop must drain canceled opening cleanup before the next Open');
    assert.equal(host.runtime.phase(first.id), 'running');
    window = guest()!;
  } finally { finishCleanup(); cleanupCache.restore(); host.runtime.stop = originalStop; }
  // Disabling background work while a foreground reopen is loading must not
  // stop the runtime that will serve its new independent window.
  await host.request({ action: 'background', id: first.id, enabled: true });
  window.close(); await until(async () => !guest());
  const foregroundCache = holdCodeCache(first.id);
  try {
    const reopened = host.open(first.id);
    await foregroundCache.entered;
    await host.request({ action: 'background', id: first.id, enabled: false });
    foregroundCache.release(); await reopened;
    assert.equal(host.runtime.phase(first.id), 'running', 'a pending foreground reopen is not background work');
    window = guest()!;
    assert.match(await window.webContents.executeJavaScript(`document.querySelector('h1').textContent`), /v2/);
    assert.equal(await window.webContents.executeJavaScript(`localStorage.count`), '7');
  } finally { foregroundCache.restore(); }
  await host.request({ action: 'background', id: first.id, enabled: false });
  assert.match(await window.webContents.executeJavaScript(`document.querySelector('h1').textContent`), /v2/);
  const evidence = process.env.LINGXI_LOCAL_APP_EVIDENCE;
  if (evidence) { await fs.mkdir(evidence, { recursive: true }); await fs.writeFile(join(evidence, 'local-app-window.png'), (await window.webContents.capturePage()).toPNG()); }
  window.close(); await until(async () => (await host.list())[0]!.phase === 'stopped');
  await host.open(first.id); window = guest()!;
  trace('await host.open(first.id); window = guest()!;');
  assert.equal(await window.webContents.executeJavaScript(`localStorage.count`), '7');

  // Fixture-only dialog choice; production authorization logic is exercised unchanged.
  let grants = 0;
  dialog.showMessageBox = (async (...args: any[]) => { const options = args.at(-1); assert.ok(options.detail.includes('local programs') || options.message.startsWith('Delete')); grants++; return { response: 1, checkboxChecked: false }; }) as typeof dialog.showMessageBox;
  const source = join(host.store.importsRoot, 'server'); await fs.mkdir(source);
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Local backend', summary: 'Node service, installed and managed by LingXi', entry: { type: 'static', path: 'index.html' }, capabilities: ['files', 'clipboard'], install: [{ command: 'node -e "console.log(process.versions.node)"' }], processes: [{ name: 'api', command: 'node server.cjs', ready: { path: '/health', timeoutMs: 10000 } }] }));
  await fs.writeFile(join(source, 'index.html'), '<!doctype html><title>Local backend</title><h1 id="result">Loading…</h1><script>fetch("/__services/api/hello").then(r=>r.text()).then(t=>document.querySelector("h1").textContent=t)</script>');
  await fs.writeFile(join(source, 'server.cjs'), `const server=require('http').createServer((req,res)=>{
    if(req.url==='/exit'){res.end('bye');setTimeout(()=>process.exit(7),10)}
    else if(req.url==='/set-cookie'){res.setHeader('Set-Cookie','private_app_cookie=app-a; Path=/; HttpOnly; SameSite=Strict; Secure');res.end('set')}
    else if(req.url==='/delete-cookie'){res.setHeader('Set-Cookie','private_app_cookie=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict; Secure');res.end('deleted')}
    else if(req.url==='/set-path-cookie'){res.setHeader('Set-Cookie','path_cookie=scoped; Path=/scoped; Max-Age=60; SameSite=Lax');res.end('set')}
    else if(req.url==='/set-shared-root'){res.setHeader('Set-Cookie','shared_path_cookie=root; Path=/; Max-Age=60');res.end('set')}
    else if(req.url==='/set-shared-private'){res.setHeader('Set-Cookie','shared_path_cookie=private; Path=/private; Max-Age=60');res.end('set')}
    else if(req.url==='/private/delete-shared'){res.setHeader('Set-Cookie','shared_path_cookie=; Path=/private; Max-Age=0');res.end('deleted')}
    else if(req.url==='/domain-cookie'){res.setHeader('Set-Cookie','domain_cookie=unsafe; Domain=127.0.0.1; Path=/; Max-Age=60');res.end('set')}
    else if(req.url==='/host-cookie'){res.setHeader('Set-Cookie','__Host-app_cookie=host; Path=/; Secure; Max-Age=60');res.end('set')}
    else if(req.url==='/expired-cookie'){res.setHeader('Set-Cookie','expired_cookie=old; Path=/; Expires=Thu, 01 Jan 1970 00:00:00 GMT; Secure');res.end('set')}
    else if(req.url==='/cookie'||req.url==='/scoped/cookie'||req.url==='/private/cookie'){res.end(req.headers.cookie||'')}
    else res.end('Backend is running')
  }); server.on('upgrade',(req,socket)=>{const key=require('crypto').createHash('sha1').update(req.headers['sec-websocket-key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');socket.write('HTTP/1.1 101 Switching Protocols\\r\\nUpgrade: websocket\\r\\nConnection: Upgrade\\r\\nSec-WebSocket-Accept: '+key+'\\r\\n\\r\\n');socket.write(Buffer.concat([Buffer.from([129,12]),Buffer.from('socket ready')]));socket.on('data',()=>socket.end(Buffer.from([136,0])));});server.listen(process.env.PORT,'127.0.0.1')`);
  const backend = await tool({ action: 'publish', staged_path: source });
  const partition = session.fromPartition(`persist:local-app-${backend.id}`);
  let permissionCheck: NonNullable<Parameters<Session['setPermissionCheckHandler']>[0]>;
  let permissionRequest: NonNullable<Parameters<Session['setPermissionRequestHandler']>[0]>;
  const setCheck = partition.setPermissionCheckHandler.bind(partition), setRequest = partition.setPermissionRequestHandler.bind(partition);
  partition.setPermissionCheckHandler = handler => { if (handler) permissionCheck = handler; setCheck(handler); };
  partition.setPermissionRequestHandler = handler => { if (handler) permissionRequest = handler; setRequest(handler); };
  await tool({ action: 'start', id: backend.id });
  trace("await tool({ action: 'start', id: backend.id });");
  let serverWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${backend.id}/`))!;
  await until(() => serverWindow.webContents.executeJavaScript(`document.querySelector('h1').textContent === 'Backend is running'`));
  await serverWindow.webContents.executeJavaScript(`window.localApp.files.write('data','progress.txt','saved')`);
  assert.equal(await serverWindow.webContents.executeJavaScript(`window.localApp.files.read('data','progress.txt')`), 'saved');
  assert.equal(await serverWindow.webContents.executeJavaScript(`window.localApp.files.read('data','../app.json').then(()=>false,()=>true)`), true);
  for (const path of ['.', './', 'child/']) assert.equal(await serverWindow.webContents.executeJavaScript(`window.localApp.files.write('data',${JSON.stringify(path)},'outside').then(()=>false,()=>true)`), true);
  assert.equal((await fs.readdir(host.store.directory(backend.id))).some(name => /^data\..*\.tmp$/.test(name)), false);
  const permission = (url: string, isMainFrame: boolean) => new Promise<boolean>(resolve => permissionRequest(serverWindow.webContents, 'clipboard-read', resolve, { isMainFrame, requestingUrl: url }));
  assert.equal(permissionCheck(serverWindow.webContents, 'clipboard-read', `lingxi-app://${backend.id}.invalid`, { isMainFrame: true }), false);
  assert.equal(await permission('https://embedded.example/', false), false);
  assert.equal(await permission(`lingxi-app://${backend.id}/`, true), true);
  assert.equal(await host.hasBackgroundApps(), false);
  assert.match(await serverWindow.webContents.executeJavaScript(`window.localApp.serviceSocketUrl('api','/live')`), /^ws:\/\/127.0.0.1:\d+\/[a-f0-9]{64}\/live$/);
  assert.equal(await serverWindow.webContents.executeJavaScript(`new Promise(async (resolve,reject)=>{const socket=new WebSocket(await window.localApp.serviceSocketUrl('api','/live')); let message;socket.onmessage=e=>{message=e.data;socket.close()};socket.onclose=()=>resolve(message);socket.onerror=reject;})`), 'socket ready');
  await fetch(host.runtime.service(backend.id, 'api')!.url + '/exit');
  await until(async () => host.runtime.phase(backend.id) === 'failed');
  const failedOpenCache = holdCodeCache(backend.id);
  try {
    const reopening = host.open(backend.id).then(() => undefined, error => error);
    await failedOpenCache.entered;
    await fetch(host.runtime.service(backend.id, 'api')!.url + '/exit');
    await until(async () => host.runtime.phase(backend.id) === 'failed');
    failedOpenCache.release();
    const error = await reopening;
    assert.ok(error, 'a backend failure while preparing the window must reject the reopen');
    assert.match(error.message, /api exited \(7\)/);
  } finally { failedOpenCache.restore(); }
  const crashedBackendWindow = serverWindow;
  await host.open(backend.id);
  serverWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${backend.id}/`))!;
  assert.notEqual(serverWindow.id, crashedBackendWindow.id, 'a recovered backend invocation receives a replacement window');
  assert.equal(crashedBackendWindow.isDestroyed(), true, 'the failed invocation window is destroyed after recovery');
  assert.equal(host.runtime.phase(backend.id), 'running');
  const backendUrl = host.runtime.service(backend.id, 'api')!.url;
  await host.request({ action: 'background', id: backend.id, enabled: true });
  assert.equal(await host.hasBackgroundApps(), true);
  serverWindow.close(); await delay(200); assert.equal((await fetch(backendUrl)).status, 200);
  await host.open(backend.id); assert.equal(grants, 1);
  await until(() => library.webContents.executeJavaScript(`document.querySelectorAll('.local-app-tile').length === 2`));
  if (evidence) await fs.writeFile(join(evidence, 'local-app-library.png'), (await library.webContents.capturePage()).toPNG());
  const activeBackendWindow = () => BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${backend.id}/`))!;
  const other = await tool({ action: 'publish', name: 'Other backend', staged_path: source });
  await tool({ action: 'start', id: other.id });
  const otherWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${other.id}/`))!;
  const cookiePage = activeBackendWindow();
  const appFetch = (path: string, credentials?: RequestCredentials) => cookiePage.webContents.executeJavaScript(`fetch(${JSON.stringify(`/__services/api${path}`)},${JSON.stringify(credentials ? { credentials } : {})}).then(response=>response.text())`);
  await appFetch('/set-cookie', 'omit');
  assert.equal((await partition.cookies.get({ url: `${LOCAL_APP_SERVICE_COOKIE_ORIGIN}/__services/api/cookie` })).some(cookie => cookie.name === 'private_app_cookie'), false, 'credentials=omit does not store Set-Cookie');
  assert.equal(await appFetch('/cookie', 'omit'), '', 'credentials=omit does not send cookies');
  await appFetch('/set-cookie');
  assert.match(await appFetch('/cookie'), /private_app_cookie=app-a/);
  assert.equal(await appFetch('/cookie', 'omit'), '', 'credentials=omit does not send already stored cookies');
  assert.match(await appFetch('/cookie', 'include'), /private_app_cookie=app-a/, 'credentials=include sends the host-selected cookie');
  const cookieJarUrl = `${LOCAL_APP_SERVICE_COOKIE_ORIGIN}/__services/api/cookie`;
  const storedCookies = await partition.cookies.get({ url: cookieJarUrl });
  const appCookie = storedCookies.find(cookie => cookie.name === 'private_app_cookie');
  assert.ok(appCookie, 'service cookie is stored in the App partition');
  assert.equal(appCookie.httpOnly, true);
  assert.equal(appCookie.secure, true);
  assert.equal(appCookie.sameSite, 'strict');
  assert.equal(appCookie.session, true, 'cookies without expiry remain session cookies');
  assert.doesNotMatch(await cookiePage.webContents.executeJavaScript('document.cookie'), /private_app_cookie/);

  await appFetch('/set-path-cookie');
  assert.match(await appFetch('/scoped/cookie'), /path_cookie=scoped/);
  assert.doesNotMatch(await appFetch('/cookie'), /path_cookie=scoped/, 'Path limits a cookie to matching app requests');
  await appFetch('/set-shared-root');
  await appFetch('/set-shared-private');
  assert.match(await appFetch('/private/cookie'), /shared_path_cookie=private/);
  assert.match(await appFetch('/private/cookie'), /shared_path_cookie=root/);
  await appFetch('/private/delete-shared');
  const afterPrivateDelete = await appFetch('/private/cookie');
  assert.match(afterPrivateDelete, /shared_path_cookie=root/, 'expiring a same-name private-path cookie preserves the root-path cookie');
  assert.doesNotMatch(afterPrivateDelete, /shared_path_cookie=private/);
  await appFetch('/domain-cookie');
  await appFetch('/host-cookie');
  await appFetch('/expired-cookie');
  const afterRejectedCookies = await partition.cookies.get({ url: cookieJarUrl });
  assert.equal(afterRejectedCookies.some(cookie => cookie.name === 'domain_cookie'), false, 'upstream Domain attributes are rejected instead of mapped across origins');
  assert.equal(afterRejectedCookies.some(cookie => cookie.name === '__Host-app_cookie'), false, '__Host- cookies are rejected when service Path=/ cannot be preserved');
  assert.equal(afterRejectedCookies.some(cookie => cookie.name === 'expired_cookie'), false, 'already expired cookies are not persisted');
  await appFetch('/delete-cookie');
  assert.doesNotMatch(await appFetch('/cookie'), /private_app_cookie=app-a/, 'Max-Age=0 removes only the matching host-only cookie');
  assert.doesNotMatch(await otherWindow.webContents.executeJavaScript(`fetch('/__services/api/cookie').then(r=>r.text())`), /private_app_cookie|path_cookie/, 'another App partition cannot read this App’s service cookies');
  assert.equal((await session.defaultSession.cookies.get({ url: cookieJarUrl })).some(cookie => cookie.name === 'private_app_cookie'), false, 'service cookies never enter the default session');
  await host.request({ action: 'stop', id: other.id });

  const manifest = JSON.parse(await fs.readFile(join(source, 'lingxi-app.json'), 'utf8'));
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ ...manifest, capabilities: [] }));
  await publishUpdate(backend.id, { staged_path: source });
  assert.equal(await activeBackendWindow().webContents.executeJavaScript(`window.localApp.files.read('data','progress.txt')`), 'saved', 'publishing must not change running capabilities');
  await host.open(backend.id);
  assert.equal(await activeBackendWindow().webContents.executeJavaScript(`window.localApp.files.read('data','progress.txt').then(()=>false,()=>true)`), true);
  assert.equal(await new Promise(resolve => permissionRequest(activeBackendWindow().webContents, 'clipboard-read', resolve, { isMainFrame: true, requestingUrl: `lingxi-app://${backend.id}/` })), false);
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify(manifest));
  await publishUpdate(backend.id, { staged_path: source }); await host.open(backend.id);

  // Server landing paths must retain their browser base URL. Exercise actual
  // relative scripts/fetches, URL escaping, compression and an in-place reload.
  const nestedSource = join(host.store.importsRoot, 'nested-server'); await fs.mkdir(nestedSource);
  const nestedManifest = { schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Nested backend', entry: { type: 'server', process: 'web', path: '/ui nested/识字 #?.html' },
    processes: [{ name: 'web', command: 'node server.cjs', ready: { path: '/health', timeoutMs: 10000 } }] };
  await fs.writeFile(join(nestedSource, 'lingxi-app.json'), JSON.stringify(nestedManifest));
  await fs.writeFile(join(nestedSource, 'server.cjs'), `const http=require('http'),zlib=require('zlib');
    http.createServer((req,res)=>{const path=decodeURIComponent(new URL(req.url,'http://localhost').pathname);
      let body,type;
      if(path==='/health'){res.end('ready');return}
      if(path==='/ui nested/识字 #?.html'&&req.method==='POST'){
        let data='';req.on('data',chunk=>data+=chunk);req.on('end',()=>res.end(req.method+':'+data));return;
      }
      if(path==='/ui nested/识字 #?.html'||path==='/ui nested/'){
        body='<!doctype html><title>Nested backend</title><h1>Loading</h1><script src="app.js"></script>';type='text/html';
      }else if(path==='/ui nested/app.js'){
        body='fetch("data.json").then(r=>r.json()).then(data=>document.querySelector("h1").textContent=data.message)';type='text/javascript';
      }else if(path==='/ui nested/data.json'){
        body=JSON.stringify({message:'Nested compressed backend works'});type='application/json';
      }else{res.writeHead(404);res.end('wrong path: '+path);return}
      const compressed=zlib.gzipSync(body);res.writeHead(200,{'Content-Type':type,'Content-Encoding':'gzip','Content-Length':compressed.length});res.end(compressed);
    }).listen(process.env.PORT,'127.0.0.1');`);
  const nested = await tool({ action: 'publish', staged_path: nestedSource });
  await host.open(nested.id);
  let nestedWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${nested.id}/`))!;
  assert.equal(new URL(nestedWindow.webContents.getURL()).pathname, '/ui%20nested/%E8%AF%86%E5%AD%97%20%23%3F.html');
  await until(() => nestedWindow.webContents.executeJavaScript(`document.querySelector('h1')?.textContent === 'Nested compressed backend works'`));
  const home = await nestedWindow.webContents.executeJavaScript(`fetch('/?from=home').then(async r=>({url:r.url,body:await r.text()}))`);
  assert.equal(new URL(home.url).pathname, '/ui%20nested/%E8%AF%86%E5%AD%97%20%23%3F.html');
  assert.equal(new URL(home.url).search, '?from=home');
  assert.match(home.body, /<script src="app.js">/);
  assert.equal(await nestedWindow.webContents.executeJavaScript(`fetch('/',{method:'POST',body:'saved=value'}).then(r=>r.text())`), 'POST:saved=value');
  await nestedWindow.loadURL(`lingxi-app://${nested.id}/`);
  await until(() => nestedWindow.webContents.executeJavaScript(`document.querySelector('h1')?.textContent === 'Nested compressed backend works'`));
  if (evidence) await fs.writeFile(join(evidence, 'local-app-nested-server.png'), (await nestedWindow.webContents.capturePage()).toPNG());
  nestedManifest.entry.path = '/ui nested/';
  await fs.writeFile(join(nestedSource, 'lingxi-app.json'), JSON.stringify(nestedManifest));
  const previousNestedWindow = nestedWindow;
  await publishUpdate(nested.id, { staged_path: nestedSource });
  await host.open(nested.id);
  nestedWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${nested.id}/`))!;
  assert.notEqual(nestedWindow.id, previousNestedWindow.id, 'publishing a new nested-server revision replaces its window');
  assert.equal(previousNestedWindow.isDestroyed(), true, 'the previous nested-server window is destroyed after replacement');
  assert.equal(new URL(nestedWindow.webContents.getURL()).pathname, '/ui%20nested/');
  await until(() => nestedWindow.webContents.executeJavaScript(`document.querySelector('h1')?.textContent === 'Nested compressed backend works'`));
  await host.request({ action: 'stop', id: nested.id });

  const cancelSource = join(host.store.importsRoot, 'cancel'); await fs.mkdir(cancelSource);
  await fs.writeFile(join(cancelSource, 'index.html'), '<!doctype html>cancel');
  await fs.writeFile(join(cancelSource, 'lingxi-app.json'), JSON.stringify({ schema: 2, targets: [{ platform: 'macos', environment: 'native' }], name: 'Canceled install', entry: { type: 'static', path: 'index.html' }, install: [{ command: `node -e "console.log('cancellable-begun');setInterval(()=>{},1000)"` }] }));
  const cancelApp = await tool({ action: 'publish', staged_path: cancelSource });
  const abort = new AbortController();
  const abandoned = assert.rejects(fetch(url, { method: 'POST', headers: { Authorization: `Bearer ${discovery.token}` }, body: JSON.stringify({ action: 'start', id: cancelApp.id }), signal: abort.signal }));
  await until(async () => host.runtime.logs(cancelApp.id).some(log => log.text === 'cancellable-begun\n'));
  abort.abort(); await abandoned;
  await until(async () => host.runtime.phase(cancelApp.id) === 'stopped');
  // Optional acceptance project exercises real dependency installation and a
  // production Vite/TypeScript build; ordinary CI stays network-independent.
  if (process.env.LINGXI_TEST_VITE_PROJECT) {
    const staged = join(host.store.importsRoot, 'vite');
    await fs.cp(process.env.LINGXI_TEST_VITE_PROJECT, staged, { recursive: true });
    const framework = await tool({ action: 'publish', staged_path: staged });
    await tool({ action: 'start', id: framework.id });
    const frameworkWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${framework.id}/`))!;
    await until(() => frameworkWindow.webContents.executeJavaScript(`document.querySelector('#app')?.textContent === 'Vite + TypeScript ready'`));
    assert.ok(host.runtime.logs(framework.id).some(line => line.text.includes('vite build')));
    if (evidence) await fs.writeFile(join(evidence, 'vite-local-app-window.png'), (await frameworkWindow.webContents.capturePage()).toPNG());
    await host.request({ action: 'stop', id: framework.id });
  }
  const originalPicker = dialog.showOpenDialog;
  let finishPicker!: (value: Electron.OpenDialogReturnValue) => void, pickerStarted!: () => void;
  const pickerReady = new Promise<void>(resolve => { pickerStarted = resolve; });
  dialog.showOpenDialog = (() => { pickerStarted(); return new Promise(resolve => { finishPicker = resolve; }); }) as typeof dialog.showOpenDialog;
  const pickerContents = activeBackendWindow().webContents;
  const picking = assert.rejects((host as any).native({ sender: pickerContents, senderFrame: pickerContents.mainFrame }, { method: 'files.pickDirectory' }), /no longer active/);
  await pickerReady;
  const lastBackendUrl = host.runtime.service(backend.id, 'api')!.url;
  await host.request({ action: 'revoke', id: backend.id });
  finishPicker({ canceled: false, filePaths: [root] }); await picking; await delay(50);
  dialog.showOpenDialog = originalPicker;
  await assert.rejects(fs.access(join(host.store.directory(backend.id), 'file-grants.json')));
  await assert.rejects(fetch(lastBackendUrl));
  assert.deepEqual((await host.store.get(backend.id)).grants, []);
  dialog.showMessageBox = (async (...args: any[]) => { const signal = args.at(-1).signal; return await new Promise(resolve => signal.addEventListener('abort', () => resolve({ response: 0, checkboxChecked: false }), { once: true })); }) as typeof dialog.showMessageBox;
  const canceledOpen = host.open(backend.id); const canceled = assert.rejects(canceledOpen);
  await delay(20); await host.request({ action: 'stop', id: backend.id }); await canceled;
  assert.deepEqual((await host.store.get(backend.id)).grants, []);
  dialog.showMessageBox = originalDialog;

  // Simulate a response failure after the Rust record/release committed. The
  // host must recover the owner from Core and refuse a duplicate first publish
  // after restart, while retaining the explicit app id in the error.
  const uncertainSource = await nativeSource('post-commit-retry');
  await fs.writeFile(join(uncertainSource.projectPath, 'lingxi-app.json'), JSON.stringify({ schema: 2,
    targets: [{ platform: 'macos', environment: 'native' }], name: 'Committed before response',
    entry: { type: 'static', path: 'index.html' } }));
  await fs.writeFile(join(uncertainSource.projectPath, 'index.html'), '<!doctype html><title>Commit probe</title>');
  await host.request({ action: 'prepareAuthoring', session: uncertainSource });
  const uncertainStage = join(host.store.importsRoot, 'post-commit-retry');
  await fs.cp(uncertainSource.projectPath, uncertainStage, { recursive: true });
  const originalPublish = host.store.publish.bind(host.store);
  let loseResponse = true;
  host.store.publish = async input => {
    const published = await originalPublish(input);
    if (loseResponse) { loseResponse = false; throw new Error('fixture simulated response loss after Core commit'); }
    return published;
  };
  const uncertainRequest = { action: 'publish', staged_path: uncertainStage, source_path: uncertainSource.projectPath,
    source_session: uncertainSource };
  const lostResponse = await toolResult(uncertainRequest);
  assert.equal(lostResponse.status, 400);
  assert.match(lostResponse.value.error, /simulated response loss/);
  host.store.publish = originalPublish;
  const committed = (await host.list()).filter(row => row.authoring?.sourceDirectory === uncertainSource.projectPath);
  assert.equal(committed.length, 1, 'the Core commit exists once even though the tool response was lost');
  await host.dispose();
  await startHost();
  const retryAfterRestart = await toolResult(uncertainRequest);
  assert.equal(retryAfterRestart.status, 400, 'the uncertain retry must not create a second app');
  assert.match(retryAfterRestart.value.error, new RegExp(committed[0]!.id));
  assert.equal((await host.list()).filter(row => row.authoring?.sourceDirectory === uncertainSource.projectPath).length, 1);
  await host.store.core.request({ op: 'remove', appId: committed[0]!.id });

  await host.dispose(); library.destroy();
  trace('await host.dispose(); library.destroy();');
  process.stdout.write(JSON.stringify({ passed: true, library: true, backend: true, updates: true, grants: true, framework: Boolean(process.env.LINGXI_TEST_VITE_PROJECT) }) + '\n');
}
main().then(() => { trace('quitting completed fixture'); setTimeout(() => app.quit(), 20); }, async error => { console.error(error.stack ?? error); await host?.dispose().catch(() => undefined); app.exit(1); });
