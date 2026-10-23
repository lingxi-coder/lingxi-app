import { app, BrowserWindow, protocol, dialog, session, type Session } from 'electron';
import assert from 'node:assert/strict';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { LocalAppHost } from '../../src/main/local-apps/host';

const root = process.env.LINGXI_TEST_USER_DATA!;
process.on('uncaughtException', error => { process.stderr.write(`Uncaught: ${error.stack}\n`); app.exit(1); });
process.on('unhandledRejection', error => { process.stderr.write(`Unhandled: ${String(error)}\n`); app.exit(1); });
app.setPath('userData', join(root, 'profile'));
protocol.registerSchemesAsPrivileged([{ scheme: 'lingxi-app', privileges: { standard: true, secure: true, supportFetchAPI: true, corsEnabled: true, stream: true, allowServiceWorkers: true } }]);
app.on('window-all-closed', () => undefined);
const originalDialog = dialog.showMessageBox;
for (const event of ['before-quit', 'will-quit', 'quit'] as const) app.on(event, () => process.stderr.write(`App lifecycle: ${event}\n`));
async function until(check: () => Promise<unknown>) { for (let i = 0; i < 200; i++) { if (await check()) return; await delay(30); } throw new Error('Timed out waiting for fixture'); }
const html = (version: number) => `<!doctype html><meta charset="utf-8"><title>识字花园</title><style>body{font-family:system-ui;background:#fbf9f3;padding:48px;color:#284337}h1{font-size:32px}.chars{display:flex;gap:20px}button{font-size:56px;border:0;border-radius:20px;padding:30px;background:#dff0df;cursor:pointer}</style><h1>识字花园 · v${version}</h1><p>每天认识几个新朋友</p><div class="chars"><button onclick="localStorage.count=String(Number(localStorage.count||0)+1)">山</button><button>水</button><button>花</button></div>`;
let host: LocalAppHost;
const trace = (message: string) => process.stderr.write(`LocalApp fixture: ${message}\n`);
async function main() {
  await app.whenReady();
  trace('await app.whenReady();');
  const library = new BrowserWindow({ show: true, width: 1100, height: 800, webPreferences: { preload: join(root, 'preload.cjs'), sandbox: true, contextIsolation: true, nodeIntegration: false } });
  host = new LocalAppHost({ configHome: join(root, 'config'), preload: join(root, 'localApp.cjs'), nodeExecutable: process.execPath, mainWindow: () => library, edit: async () => undefined });
  await host.initialize();
  trace('await host.initialize();');
  host.registerMainWindow(library, process.env.LINGXI_TEST_LIBRARY_URL!);
  await library.loadURL(process.env.LINGXI_TEST_LIBRARY_URL!);
  trace('await library.loadURL(process.env.LINGXI_TEST_LIBRARY_URL!);');
  const discovery = JSON.parse(await fs.readFile(join(root, 'config/desktop-local-app-host.json'), 'utf8'));
  const url = `http://127.0.0.1:${discovery.port}/local-app`;
  assert.equal((await fetch(url, { method: 'POST', body: '{}' })).status, 403);
  const tool = async (input: object) => { const response = await fetch(url, { method: 'POST', headers: { Authorization: `Bearer ${discovery.token}` }, body: JSON.stringify(input) }); const value = await response.json(); assert.equal(response.status, 200, JSON.stringify(value)); return value; };
  const first = await tool({ action: 'publish', name: '识字花园', summary: '每天学习汉字，记录进步。', html: html(1) });
  await until(() => library.webContents.executeJavaScript(`Boolean(document.querySelector('.local-app-tile-open'))`));
  await library.webContents.executeJavaScript(`document.querySelector('.local-app-tile-open').click()`);
  const guest = () => BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${first.id}/`));
  await until(async () => guest() && await guest()!.webContents.executeJavaScript(`Boolean(window.localApp && document.querySelector('.chars'))`));
  let window = guest()!;
  assert.equal(await window.webContents.executeJavaScript(`typeof require + ':' + typeof window.lingxi`), 'undefined:undefined');
  assert.equal(await window.webContents.executeJavaScript(`window.localApp.clipboard.read().then(()=>false,()=>true)`), true);
  await window.webContents.executeJavaScript(`localStorage.count='7'`);
  await tool({ action: 'publish', id: first.id, html: html(2) });
  await host.open(first.id);
  assert.equal(guest()!.id, window.id);
  assert.equal(await window.webContents.executeJavaScript(`localStorage.count`), '7');

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
    host.store.get = async id => { const wait = id === first.id && delayClose; if (wait) delayClose = false; const row = await originalGet(id); if (wait) { closeStarted(); await closeGate; } return row; };
    (host as any).clearCodeCache = async (id: string) => { if (id === first.id) { cacheStarted(); await cacheGate; } await originalClear(id); };
    window.close(); await closing;
    const abort = new AbortController();
    const reopening = host.open(first.id, false, false, abort.signal);
    const completed = cancel ? assert.rejects(reopening) : reopening;
    await caching; releaseClose(); await delay(60);
    const reopeningPhase = host.runtime.phase(first.id);
    if (cancel) abort.abort();
    releaseCache(); await completed;
    host.store.get = originalGet; (host as any).clearCodeCache = originalClear;
    assert.equal(reopeningPhase, 'running');
    assert.equal(host.runtime.phase(first.id), cancel && !background ? 'stopped' : 'running');
    if (cancel) await host.open(first.id);
    window = guest()!;
  }
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
  const source = join(host.store.root, 'imports', 'server'); await fs.mkdir(source);
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ schema: 1, name: 'Local backend', summary: 'Node service, installed and managed by LingXi', entry: { type: 'static', path: 'index.html' }, capabilities: ['files', 'clipboard'], install: [{ command: 'node -e "console.log(process.versions.node)"' }], processes: [{ name: 'api', command: 'node server.cjs', ready: { path: '/health', timeoutMs: 10000 } }] }));
  await fs.writeFile(join(source, 'index.html'), '<!doctype html><title>Local backend</title><h1 id="result">Loading…</h1><script>fetch("/__services/api/hello").then(r=>r.text()).then(t=>document.querySelector("h1").textContent=t)</script>');
  await fs.writeFile(join(source, 'server.cjs'), `const server=require('http').createServer((req,res)=>{if(req.url==='/exit'){res.end('bye');setTimeout(()=>process.exit(7),10)}else if(req.url==='/set-cookie'){res.setHeader('Set-Cookie','private_app_cookie=app-a; Path=/');res.end('set')}else if(req.url==='/cookie'){res.end(req.headers.cookie||'')}else res.end('Backend is running')}); server.on('upgrade',(req,socket)=>{const key=require('crypto').createHash('sha1').update(req.headers['sec-websocket-key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');socket.write('HTTP/1.1 101 Switching Protocols\\r\\nUpgrade: websocket\\r\\nConnection: Upgrade\\r\\nSec-WebSocket-Accept: '+key+'\\r\\n\\r\\n');socket.write(Buffer.concat([Buffer.from([129,12]),Buffer.from('socket ready')]));socket.on('data',()=>socket.end(Buffer.from([136,0])));});server.listen(process.env.PORT,'127.0.0.1')`);
  const backend = await tool({ action: 'publish', staged_path: source });
  const partition = session.fromPartition(`persist:local-app-${backend.id}`);
  let permissionCheck: NonNullable<Parameters<Session['setPermissionCheckHandler']>[0]>;
  let permissionRequest: NonNullable<Parameters<Session['setPermissionRequestHandler']>[0]>;
  const setCheck = partition.setPermissionCheckHandler.bind(partition), setRequest = partition.setPermissionRequestHandler.bind(partition);
  partition.setPermissionCheckHandler = handler => { if (handler) permissionCheck = handler; setCheck(handler); };
  partition.setPermissionRequestHandler = handler => { if (handler) permissionRequest = handler; setRequest(handler); };
  await tool({ action: 'start', id: backend.id });
  trace("await tool({ action: 'start', id: backend.id });");
  const serverWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${backend.id}/`))!;
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
  assert.match(await serverWindow.webContents.executeJavaScript(`window.localApp.serviceSocketUrl('api','/live')`), /^ws:\/\/127.0.0.1:\d+\/live$/);
  assert.equal(await serverWindow.webContents.executeJavaScript(`new Promise(async (resolve,reject)=>{const socket=new WebSocket(await window.localApp.serviceSocketUrl('api','/live')); let message;socket.onmessage=e=>{message=e.data;socket.close()};socket.onclose=()=>resolve(message);socket.onerror=reject;})`), 'socket ready');
  await fetch(host.runtime.service(backend.id, 'api')!.url + '/exit');
  await until(async () => host.runtime.phase(backend.id) === 'failed');
  await host.open(backend.id);
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
  await activeBackendWindow().webContents.executeJavaScript(`fetch('/__services/api/set-cookie').then(r=>r.text())`);
  assert.match(await activeBackendWindow().webContents.executeJavaScript(`fetch('/__services/api/cookie').then(r=>r.text())`), /private_app_cookie=app-a/);
  assert.doesNotMatch(await otherWindow.webContents.executeJavaScript(`fetch('/__services/api/cookie').then(r=>r.text())`), /private_app_cookie/);
  assert.equal((await session.defaultSession.cookies.get({ url: backendUrl })).some(cookie => cookie.name === 'private_app_cookie'), false);
  await host.request({ action: 'stop', id: other.id });

  const manifest = JSON.parse(await fs.readFile(join(source, 'lingxi-app.json'), 'utf8'));
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify({ ...manifest, capabilities: [] }));
  await tool({ action: 'publish', id: backend.id, staged_path: source });
  assert.equal(await activeBackendWindow().webContents.executeJavaScript(`window.localApp.files.read('data','progress.txt')`), 'saved', 'publishing must not change running capabilities');
  await host.open(backend.id);
  assert.equal(await activeBackendWindow().webContents.executeJavaScript(`window.localApp.files.read('data','progress.txt').then(()=>false,()=>true)`), true);
  assert.equal(await new Promise(resolve => permissionRequest(activeBackendWindow().webContents, 'clipboard-read', resolve, { isMainFrame: true, requestingUrl: `lingxi-app://${backend.id}/` })), false);
  await fs.writeFile(join(source, 'lingxi-app.json'), JSON.stringify(manifest));
  await tool({ action: 'publish', id: backend.id, staged_path: source }); await host.open(backend.id);

  // Server landing paths must retain their browser base URL. Exercise actual
  // relative scripts/fetches, URL escaping, compression and an in-place reload.
  const nestedSource = join(host.store.root, 'imports', 'nested-server'); await fs.mkdir(nestedSource);
  const nestedManifest = { schema: 1, name: 'Nested backend', entry: { type: 'server', process: 'web', path: 'ui nested/识字 #?.html' },
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
  const nestedWindow = BrowserWindow.getAllWindows().find(w => w.webContents.getURL().startsWith(`lingxi-app://${nested.id}/`))!;
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
  nestedManifest.entry.path = 'ui nested/';
  await fs.writeFile(join(nestedSource, 'lingxi-app.json'), JSON.stringify(nestedManifest));
  await tool({ action: 'publish', id: nested.id, staged_path: nestedSource });
  await host.open(nested.id);
  assert.equal(new URL(nestedWindow.webContents.getURL()).pathname, '/ui%20nested/');
  await until(() => nestedWindow.webContents.executeJavaScript(`document.querySelector('h1')?.textContent === 'Nested compressed backend works'`));
  await host.request({ action: 'stop', id: nested.id });

  const cancelSource = join(host.store.root, 'imports', 'cancel'); await fs.mkdir(cancelSource);
  await fs.writeFile(join(cancelSource, 'index.html'), '<!doctype html>cancel');
  await fs.writeFile(join(cancelSource, 'lingxi-app.json'), JSON.stringify({ schema: 1, name: 'Canceled install', entry: { type: 'static', path: 'index.html' }, install: [{ command: `node -e "console.log('cancellable-begun');setInterval(()=>{},1000)"` }] }));
  const cancelApp = await tool({ action: 'publish', staged_path: cancelSource });
  const abort = new AbortController();
  const abandoned = assert.rejects(fetch(url, { method: 'POST', headers: { Authorization: `Bearer ${discovery.token}` }, body: JSON.stringify({ action: 'start', id: cancelApp.id }), signal: abort.signal }));
  await until(async () => host.runtime.logs(cancelApp.id).some(log => log.text === 'cancellable-begun\n'));
  abort.abort(); await abandoned;
  await until(async () => host.runtime.phase(cancelApp.id) === 'stopped');
  // Optional acceptance project exercises real dependency installation and a
  // production Vite/TypeScript build; ordinary CI stays network-independent.
  if (process.env.LINGXI_TEST_VITE_PROJECT) {
    const staged = join(host.store.root, 'imports', 'vite');
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
  await host.dispose(); library.destroy();
  trace('await host.dispose(); library.destroy();');
  process.stdout.write(JSON.stringify({ passed: true, library: true, backend: true, updates: true, grants: true, framework: Boolean(process.env.LINGXI_TEST_VITE_PROJECT) }) + '\n');
}
main().then(() => { trace('quitting completed fixture'); setTimeout(() => app.quit(), 20); }, async error => { console.error(error.stack ?? error); await host?.dispose().catch(() => undefined); app.exit(1); });
