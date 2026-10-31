import assert from 'node:assert/strict';
import { createServer, request as proxyRequest } from 'node:http';
import { createServer as createHttpsServer } from 'node:https';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm, mkdir, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL, fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';
import { randomUUID } from 'node:crypto';
import { build } from 'esbuild';
import test from 'node:test';
import { LocalAppRemoteGateway } from '../src/main/local-apps/remoteGateway.ts';
import { REMOTE_LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';
import { withFakeChatCheckpoint } from './fixtures/local-app-chat-checkpoint.ts';

// Opt in with an installed Playwright module. The test uses only a loopback gateway and test identities.
const playwrightModule = process.env.LINGXI_TEST_PLAYWRIGHT_MODULE;
const appId = `a${'1'.repeat(32)}`;
const otherId = `a${'2'.repeat(32)}`;
const owner = { accountUid: 'test-account', deviceId: 'test-device', hostId: 'test-host' };
const run = { appId, revision: 1, runId: 'test-run' };
async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port = server.address().port;
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return port;
}

async function loadSourceHost() {
  const bundled = await build({
    stdin: { contents: "export { LocalAppHost } from './host.ts'; export { LocalAppAgentBroker } from './agentBroker.ts';", resolveDir: new URL('../src/main/local-apps/', import.meta.url).pathname, loader: 'ts' },
    bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'], metafile: true,
    // esbuild has its own package resolver. Carry the test process's isolated
    // bridge-client resolution into the source Host bundle as well.
    plugins: [{ name: 'bridge-client-test-resolution', setup(builder) {
      builder.onResolve({ filter: /^@lingxi\/bridge-client(?:\/|$)/ }, args => ({ path: fileURLToPath(import.meta.resolve(args.path)) }));
    } }],
  });
  if (process.env.LINGXI_TEST_ARTIFACT_DIR) await writeFile(join(process.env.LINGXI_TEST_ARTIFACT_DIR, 'source-host-inputs.json'), JSON.stringify({
    bridgeClientEntry: import.meta.resolve('@lingxi/bridge-client'), inputs: Object.keys(bundled.metafile.inputs),
  }, null, 2));
  const require = createRequire(import.meta.url);
  const electron = {
    BrowserWindow: class { static getAllWindows() { return []; } },
    ipcMain: { removeHandler() {} },
    session: { fromPartition: () => ({ flushStorageData() {} }) },
    dialog: { showMessageBox: async () => ({ response: 1 }) },
    net: { fetch: async url => new Response(await readFile(new URL(url)), { headers: { 'content-type': 'text/html; charset=utf-8' } }) },
  };
  const module = { exports: {} };
  // Use the Node test realm, as the existing Host suite does for real HTTP.
  new Function('module', 'exports', 'require', bundled.outputFiles[0].text)(module, module.exports, name => name === 'electron' ? electron : require(name));
  return module.exports;
}

test('real browser: remote gateway bootstrap, SDK, app cookies, isolation and revocation', { skip: !playwrightModule && 'Set LINGXI_TEST_PLAYWRIGHT_MODULE to an installed Playwright entry point', timeout: 60_000 }, async () => {
  const { chromium } = await import(pathToFileURL(playwrightModule).href);
  const port = await freePort();
  const gateway = new LocalAppRemoteGateway(`http://{appId}-{sessionId}.localhost:${port}`, port, {
    current: async candidate => candidate.appId === appId && candidate.runId === run.runId && candidate.revision === run.revision,
    invoke: async (_run, method) => { assert.equal(method, 'info'); return { appId, revision: 1 }; },
    invokeGuard: () => () => true,
    socket: async () => undefined,
    resource: async (_run, request) => {
      const path = new URL(request.url).pathname;
      if (path === '/') return new Response('<!doctype html><title>Gateway app</title><script src="/__lingxi_local_app_sdk.js"></script><main>Static app</main>', { headers: { 'content-type': 'text/html; charset=utf-8' } });
      if (path === '/__lingxi_local_app_sdk.js') return new Response(REMOTE_LOCAL_APP_SDK, { headers: { 'content-type': 'text/javascript; charset=utf-8' } });
      if (path === '/set-cookie') return new Response('ok', { headers: { 'set-cookie': 'app_session=original; Path=/; SameSite=Strict' } });
      if (path === '/inspect') return Response.json({ cookie: request.headers.get('cookie') }, { headers: { 'set-cookie': 'app_session=changed; Path=/; SameSite=Strict' } });
      if (/^\/__services\/(api|other)\/login$/.test(path)) {
        const name = path.split('/')[2];
        return new Response('ok', { headers: { 'set-cookie': `service_session=${name}; Path=/; HttpOnly; SameSite=Strict` } });
      }
      if (/^\/__services\/(api|other)\/inspect$/.test(path)) return Response.json({ cookie: request.headers.get('cookie') });
      if (path === '/events') return new Response(new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode('first')); } }), { headers: { 'content-type': 'text/plain' } });
      return new Response('missing', { status: 404 });
    },
  });
  let browser;
  try {
    const opened = await gateway.open(run, owner);
    const ticket = new URL(opened.url).hash.slice(1);
    const origin = new URL(opened.url).origin;
    assert.match(new URL(origin).hostname, new RegExp(`^${appId}-[a-f0-9]{20}\\.localhost$`));
    browser = await chromium.launch({ headless: true, ...(process.env.LINGXI_TEST_CHROME ? { executablePath: process.env.LINGXI_TEST_CHROME } : {}) });
    const context = await browser.newContext();
    const page = await context.newPage();
    // Do not allow a navigation failure to print the one-use fragment ticket.
    await page.goto(opened.url).catch(() => { throw new Error('Bootstrap navigation failed'); });
    await page.waitForURL(`${origin}/`);
    assert.equal(await page.title(), 'Gateway app');
    assert.equal(await page.locator('main').innerText(), 'Static app');
    assert.equal(await page.evaluate(() => location.hash), '');
    await page.waitForFunction(() => typeof window.localApp?.info === 'function');
    assert.deepEqual(await page.evaluate(() => window.localApp.info()), { appId, revision: 1 });
    const cookies = await context.cookies(origin);
    const auth = cookies.find(cookie => cookie.name === 'lingxi_remote');
    assert.ok(auth && auth.httpOnly);
    assert.equal((await page.evaluate(() => document.cookie)).includes('lingxi_remote'), false);
    assert.equal(await page.evaluate(async () => (await fetch('/set-cookie')).status), 200);
    const omitted = await page.evaluate(async () => {
      const response = await fetch('/inspect', { credentials: 'omit' });
      return { status: response.status, body: await response.json() };
    });
    assert.equal(omitted.status, 200);
    assert.equal(omitted.body.cookie === null, true);
    assert.equal((await context.cookies(origin)).find(cookie => cookie.name === 'app_session')?.value, 'original');
    const ordinary = await page.evaluate(async () => (await fetch('/inspect')).json());
    // Do not print a cookie header if gateway isolation regresses.
    assert.equal(ordinary.cookie === 'app_session=original', true);
    const serviceCookies = await page.evaluate(async () => {
      await fetch('/__services/api/login'); await fetch('/__services/other/login');
      const api = await (await fetch('/__services/api/inspect')).json();
      const other = await (await fetch('/__services/other/inspect')).json();
      return {api:api.cookie, other:other.cookie};
    });
    assert.equal(serviceCookies.api.includes('service_session=api'), true);
    assert.equal(serviceCookies.api.includes('service_session=other'), false);
    assert.equal(serviceCookies.other.includes('service_session=other'), true);
    assert.equal(serviceCookies.other.includes('service_session=api'), false);
    const otherOrigin = `http://${otherId}-${'a'.repeat(20)}.localhost:${port}`;
    const isolated = await browser.newContext();
    await isolated.addCookies([{ name: auth.name, value: auth.value, url: otherOrigin, httpOnly: true, sameSite: 'Strict' }]);
    const otherPage = await isolated.newPage();
    const denied = await otherPage.goto(`${otherOrigin}/`);
    assert.equal(denied.status(), 401);
    await isolated.close();

    // A fresh launch must not reuse the first origin or accept its cookie.
    const reopened = await gateway.open(run, owner);
    const newOrigin = new URL(reopened.url).origin;
    assert.equal(newOrigin !== origin, true);
    const reopenedTicket = new URL(reopened.url).hash.slice(1);
    const wrongOriginTicket = await page.evaluate(async ticketValue => (await fetch('/__lingxi_remote_bootstrap', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ ticket: ticketValue }),
    })).status, reopenedTicket);
    assert.equal(wrongOriginTicket, 403);
    assert.equal((await context.cookies(newOrigin)).some(cookie => cookie.name === 'lingxi_remote'), false);
    const newContext = await browser.newContext();
    await newContext.addCookies([{ name: auth.name, value: auth.value, url: newOrigin, httpOnly: true, sameSite: 'Strict' }]);
    const reopenedPage = await newContext.newPage();
    assert.equal((await reopenedPage.goto(`${newOrigin}/`)).status(), 401);
    await reopenedPage.goto(reopened.url).catch(() => { throw new Error('Reopened bootstrap navigation failed'); });
    await reopenedPage.waitForURL(`${newOrigin}/`);
    assert.equal(await reopenedPage.title(), 'Gateway app');
    const reopenedAuth = (await newContext.cookies(newOrigin)).find(cookie => cookie.name === 'lingxi_remote');
    assert.ok(reopenedAuth && reopenedAuth.httpOnly);
    assert.equal(reopenedAuth.value !== auth.value, true);
    assert.equal(new URL(page.url()).origin, origin);
    assert.equal(await page.evaluate(async () => (await fetch('/inspect')).status), 200);
    const reused = await page.evaluate(async ticketValue => (await fetch('/__lingxi_remote_bootstrap', {
      method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ ticket: ticketValue }),
    })).status, ticket);
    assert.equal(reused, 403);
    const firstChunk = await page.evaluate(async () => {
      const response = await fetch('/events');
      const reader = response.body.getReader();
      const first = await reader.read();
      window.streamOutcome = 'pending';
      reader.read().then(result => { window.streamOutcome = result.done ? 'closed' : 'unexpected data'; }, () => { window.streamOutcome = 'closed'; });
      return new TextDecoder().decode(first.value);
    });
    assert.equal(firstChunk, 'first');
    gateway.revokeOwner(owner);
    await page.waitForFunction(() => window.streamOutcome !== 'pending');
    assert.equal(await page.evaluate(() => window.streamOutcome), 'closed');
    assert.equal(await page.evaluate(async () => (await fetch('/inspect')).status), 401);
    assert.equal(await reopenedPage.evaluate(async () => (await fetch('/inspect')).status), 401);
    await newContext.close();
    await context.close();
  } finally {
    await browser?.close();
    await gateway.dispose();
  }
});

test('real browser: HTTPS proxy accepts secure host-only cookies and isolates reopened sessions', { skip: !playwrightModule && 'Set LINGXI_TEST_PLAYWRIGHT_MODULE to an installed Playwright entry point', timeout: 60_000 }, async () => {
  const { chromium } = await import(pathToFileURL(playwrightModule).href);
  const directory = await mkdtemp(join(tmpdir(), 'lingxi-gateway-tls-'));
  let gateway, browser, proxy;
  try {
    const keyPath = join(directory, 'key.pem'), certPath = join(directory, 'cert.pem');
    execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', keyPath, '-out', certPath, '-days', '1', '-subj', '/CN=localhost'], { stdio: 'ignore' });
    const backendPort = await freePort();
    proxy = createHttpsServer({ key: await readFile(keyPath), cert: await readFile(certPath) }, (req, res) => {
      // Preserve Host and Origin, as the TLS terminating reverse proxy must.
      const upstream = proxyRequest({ host: '127.0.0.1', port: backendPort, method: req.method, path: req.url, headers: req.headers }, response => {
        res.writeHead(response.statusCode, response.headers);
        response.pipe(res);
      });
      upstream.on('error', () => { if (!res.headersSent) res.writeHead(502); res.end(); });
      req.pipe(upstream);
    });
    await new Promise((resolve, reject) => { proxy.once('error', reject); proxy.listen(0, '127.0.0.1', resolve); });
    const tlsPort = proxy.address().port;
    gateway = new LocalAppRemoteGateway(`https://{appId}-{sessionId}.localhost:${tlsPort}`, backendPort, {
      current: async candidate => candidate.appId === appId && candidate.runId === run.runId && candidate.revision === run.revision,
      invoke: async (_run, method) => { assert.equal(method, 'info'); return { appId, revision: 1 }; },
      invokeGuard: () => () => true,
      socket: async () => undefined,
      resource: async (_run, request) => {
        const path = new URL(request.url).pathname;
        if (path === '/') return new Response('<!doctype html><title>Secure gateway app</title><script src="/__lingxi_local_app_sdk.js"></script>', { headers: { 'content-type': 'text/html; charset=utf-8' } });
        if (path === '/__lingxi_local_app_sdk.js') return new Response(REMOTE_LOCAL_APP_SDK, { headers: { 'content-type': 'text/javascript; charset=utf-8' } });
        return new Response('missing', { status: 404 });
      },
    });
    const opened = await gateway.open(run, owner);
    const origin = new URL(opened.url).origin;
    browser = await chromium.launch({ headless: true, ...(process.env.LINGXI_TEST_CHROME ? { executablePath: process.env.LINGXI_TEST_CHROME } : {}) });
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const page = await context.newPage();
    const bootstrapResponse = page.waitForResponse(response => response.url() === `${origin}/__lingxi_remote_bootstrap` && response.request().method() === 'POST');
    await page.goto(opened.url).catch(() => { throw new Error('HTTPS bootstrap navigation failed'); });
    const setCookie = (await (await bootstrapResponse).allHeaders())['set-cookie'] ?? '';
    // Check cookie attributes without printing the credential on failure.
    assert.equal(/^__Host-lingxi_remote=/u.test(setCookie), true);
    assert.equal(/(?:^|;)\s*Secure(?:;|$)/iu.test(setCookie), true);
    assert.equal(/(?:^|;)\s*HttpOnly(?:;|$)/iu.test(setCookie), true);
    assert.equal(/(?:^|;)\s*Path=\/(?:;|$)/iu.test(setCookie), true);
    assert.equal(/(?:^|;)\s*Domain=/iu.test(setCookie), false);
    await page.waitForURL(`${origin}/`);
    assert.equal(await page.title(), 'Secure gateway app');
    assert.equal(await page.evaluate(() => location.hash), '');
    await page.waitForFunction(() => typeof window.localApp?.info === 'function');
    assert.deepEqual(await page.evaluate(() => window.localApp.info()), { appId, revision: 1 });
    const auth = (await context.cookies(origin)).find(cookie => cookie.name === '__Host-lingxi_remote');
    assert.ok(auth && auth.secure && auth.httpOnly);
    assert.equal(auth.path, '/');
    assert.equal(auth.domain, new URL(origin).hostname);
    assert.equal((await page.evaluate(() => document.cookie)).includes('__Host-lingxi_remote'), false);

    const reopened = await gateway.open(run, owner);
    const newOrigin = new URL(reopened.url).origin;
    assert.equal(newOrigin !== origin, true);
    assert.equal((await context.cookies(newOrigin)).some(cookie => cookie.name === '__Host-lingxi_remote'), false);
    const isolated = await browser.newContext({ ignoreHTTPSErrors: true });
    await isolated.addCookies([{ name: auth.name, value: auth.value, url: newOrigin, httpOnly: true, secure: true, sameSite: 'Strict' }]);
    const other = await isolated.newPage();
    assert.equal((await other.goto(`${newOrigin}/`)).status(), 401);
    await other.goto(reopened.url).catch(() => { throw new Error('HTTPS reopened bootstrap navigation failed'); });
    await other.waitForURL(`${newOrigin}/`);
    await other.waitForFunction(() => typeof window.localApp?.info === 'function');
    assert.deepEqual(await other.evaluate(() => window.localApp.info()), { appId, revision: 1 });
    const nextAuth = (await isolated.cookies(newOrigin)).find(cookie => cookie.name === '__Host-lingxi_remote');
    assert.ok(nextAuth && nextAuth.secure && nextAuth.httpOnly);
    assert.equal(nextAuth.value !== auth.value, true);
    gateway.revokeOwner(owner);
    assert.equal(await page.evaluate(async () => (await fetch('/')).status), 401);
    assert.equal(await other.evaluate(async () => (await fetch('/')).status), 401);
    await isolated.close();
    await context.close();
  } finally {
    await browser?.close();
    await gateway?.dispose();
    if (proxy) { proxy.closeAllConnections(); await new Promise(resolve => proxy.close(resolve)); }
    await rm(directory, { recursive: true, force: true });
  }
});

test('real browser: HTTPS Agent Chat recovery and revocation use source Host and durable Broker', { skip: !playwrightModule && 'Set LINGXI_TEST_PLAYWRIGHT_MODULE to an installed Playwright entry point', timeout: 60_000 }, async () => {
  const { chromium } = await import(pathToFileURL(playwrightModule).href);
  const directory = await mkdtemp(join(tmpdir(), 'lingxi-remote-agent-browser-'));
  let gateway, host, broker, browser, proxy;
  try {
    const { LocalAppHost, LocalAppAgentBroker } = await loadSourceHost();
    const source = join(directory, 'release'), root = join(directory, 'local-apps', 'v2');
    await mkdir(source, { recursive: true }); await mkdir(root, { recursive: true });
    await writeFile(join(source, 'index.html'), '<!doctype html><title>Remote Agent contract</title><main>Desktop-hosted Agent</main>');
    const model = { providerId: 'synthetic', modelId: 'test', label: 'Test model', canSelect: true, supportsAttachments: true, supportsVision: true, supportsStructuredOutput: true };
    const row = {
      id: appId, name: 'Remote Agent contract', summary: '', revision: 1, currentRevision: 1, releaseDigest: 'd'.repeat(64), background: false, grants: ['agent'],
      manifest: { schema: 2, targets: [], name: 'Remote Agent contract', entry: { type: 'static', path: 'index.html' }, capabilities: ['agent'], agent: { tools: [], skills: [] } },
      agent: { enabled: true, model, tools: [], skills: [], background: false, maxTurns: 5, timeoutMs: 60_000 },
    };
    host = new LocalAppHost({ core: { onDisconnected() {}, async dispose() {} }, configHome: directory, preload: '/unused', mainWindow: () => undefined, edit: async () => {}, agentCatalog: async () => ({ models: [model], tools: [], skills: [] }) });
    host.store = {
      root, get: async () => structuredClone(row), getRevision: async () => structuredClone(row), list: async () => [structuredClone(row)],
      save: async value => Object.assign(row, structuredClone(value)), assertTarget: async () => {}, revisionPath: () => source,
      core: { request: async () => ({ digest: row.releaseDigest, files: [] }) },
    };
    let running = false;
    host.runtime = {
      phase: () => running ? 'running' : 'stopped', revision: () => running ? 1 : undefined,
      invocationId: () => running ? run.runId : undefined, owner: () => running ? { appId, revision: 1, runId: run.runId, releaseDigest: row.releaseDigest } : undefined,
      manifest: () => row.manifest, workspace: () => source,
      start: async (_id, _repair, signal) => { signal?.throwIfAborted(); running = true; }, stop: async () => { running = false; },
      exclusive: async (_id, operation) => operation(), async dispose() { running = false; },
    };
    const executions = new Map();
    let allocations = 0;
    broker = new LocalAppAgentBroker({
      root, uuid: () => { allocations++; return randomUUID(); },
      authorize: principal => host.authorizeAgent(principal), revalidate: (principal, authorization) => host.revalidateAgent(principal, authorization),
      execute: async execution => {
        executions.set(execution.runId, execution);
        if (execution.runId === 'complete-remote-chat') return withFakeChatCheckpoint(execution, { result: { text: 'synthetic answer', attachments: [], chatId: execution.chat.chatId, messageId: 'assistant-remote' }, usage: { turns: 1 } });
        await new Promise((_resolve, reject) => execution.signal.addEventListener('abort', () => reject(execution.signal.reason), { once: true }));
        throw new Error('cancelled synthetic executor');
      },
    });
    host.setAgentBroker(broker);

    const keyPath = join(directory, 'key.pem'), certPath = join(directory, 'cert.pem');
    execFileSync('openssl', ['req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', keyPath, '-out', certPath, '-days', '1', '-subj', '/CN=localhost'], { stdio: 'ignore' });
    const backendPort = await freePort();
    proxy = createHttpsServer({ key: await readFile(keyPath), cert: await readFile(certPath) }, (req, res) => {
      const upstream = proxyRequest({ host: '127.0.0.1', port: backendPort, method: req.method, path: req.url, headers: req.headers }, response => {
        res.writeHead(response.statusCode, response.headers);
        response.once('aborted', () => res.destroy()); response.once('error', () => res.destroy());
        response.pipe(res);
      });
      upstream.on('error', () => { if (!res.headersSent) res.writeHead(502); res.end(); });
      res.on('close', () => upstream.destroy()); req.pipe(upstream);
    });
    await new Promise((resolve, reject) => { proxy.once('error', reject); proxy.listen(0, '127.0.0.1', resolve); });
    const tlsPort = proxy.address().port;
    let loseAcknowledgement = true, createCalls = 0;
    gateway = new LocalAppRemoteGateway(`https://{appId}-{sessionId}.localhost:${tlsPort}`, backendPort, {
      current: candidate => host.remoteCurrent(candidate),
      invokeGuard: (candidate, method) => host.remoteInvokeGuard(candidate, method),
      invoke: async (candidate, method, args, signal, sessionId) => {
        const result = await host.remoteInvoke(candidate, method, args, signal, sessionId);
        if (method === 'agent.chats.create') {
          createCalls++;
          // Drop only the HTTP acknowledgement after the real Host/Broker have
          // committed. This models a network failure, not a mock Chat result.
          if (loseAcknowledgement) { loseAcknowledgement = false; throw new Error('synthetic lost acknowledgement'); }
        }
        return result;
      },
      socket: async () => undefined,
      resource: (candidate, request) => new URL(request.url).pathname === '/stream'
        ? new Response(new ReadableStream({ start(controller) { controller.enqueue(new TextEncoder().encode('ready')); } }), { headers: { 'content-type': 'text/plain' } })
        : host.remoteResource(candidate, request),
    });
    host.remoteGateway = gateway;
    await host.setRemoteAccess(appId, true);
    const opened = await host.remoteOpen(appId, owner);
    const origin = new URL(opened.url).origin;
    browser = await chromium.launch({ headless: true, ...(process.env.LINGXI_TEST_CHROME ? { executablePath: process.env.LINGXI_TEST_CHROME } : {}) });
    const context = await browser.newContext({ ignoreHTTPSErrors: true });
    const page = await context.newPage();
    const ready = async (target, launch) => {
      if (launch) { await target.goto(launch).catch(() => { throw new Error('Agent bootstrap navigation failed'); }); await target.waitForURL(`${new URL(launch).origin}/`); }
      else await target.reload();
      await target.waitForFunction(() => typeof window.localApp?.agent?.chats?.creationStatus === 'function');
    };
    await ready(page, opened.url);
    assert.equal(await page.evaluate(async () => (await window.localApp.info()).id), appId);
    assert.notEqual(new URL(origin).hostname, appId);
    const creation = { requestId: 'b7777777-7777-4777-8777-777777777777', title: '  Browser recovery  ' };
    const createResult = await page.evaluate(async input => {
      try { await window.localApp.agent.chats.create(input); return { unexpectedlyAccepted: true }; }
      catch (error) { return { code: error.code, requestId: error.requestId, definitive: error.definitive }; }
    }, creation);
    assert.deepEqual(createResult, { code: 'host_unavailable', requestId: creation.requestId, definitive: false });
    assert.equal(createCalls, 1); assert.equal(allocations, 1);
    await ready(page);
    assert.deepEqual(await page.evaluate(() => window.localApp.agent.chats.pendingCreations()), [{ ...creation, state: 'pending' }]);
    assert.equal(createCalls, 1, 'same-origin refresh must not replay Chat creation');
    assert.equal(await page.evaluate(async input => (await window.localApp.agent.chats.creationStatus(input)).id, creation), creation.requestId);
    assert.equal(createCalls, 1, 'creation status must remain read-only');
    assert.deepEqual(await page.evaluate(async input => (await Promise.all([window.localApp.agent.chats.create(input), window.localApp.agent.chats.create(input)])).map(chat => chat.id), creation), [creation.requestId, creation.requestId]);
    assert.equal(createCalls, 2); assert.equal(allocations, 1);
    const conflict = await page.evaluate(async input => (await (await fetch('/__lingxi_remote_invoke', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ method: 'agent.chats.create', args: { ...input, title: 'Browser recovery' } }) })).json()).result, creation);
    assert.equal(conflict.ok, false); assert.equal(conflict.error.code, 'chat_create_conflict');
    const uncertainId = 'c8888888-8888-4888-8888-888888888888';
    await mkdir(join(root, 'apps', appId, 'agent', 'agui-v1', 'chats', uncertainId));
    assert.equal(await page.evaluate(async requestId => { try { await window.localApp.agent.chats.create({ requestId }); } catch (error) { return error.code; } }, uncertainId), 'chat_storage_uncertain');
    const list = await page.evaluate(() => window.localApp.agent.chats.list({ limit: 10 }));
    assert.deepEqual(list.chats.map(chat => chat.chatId), [creation.requestId]);
    assert.equal(list.hasUncertainChats, true);
    assert.deepEqual(list.unavailableChats, [{ chatId: uncertainId, code: 'chat_storage_uncertain' }]);

    const answer = await page.evaluate(async requestId => {
      const chat = await window.localApp.agent.chats.open(requestId);
      const file = await window.localApp.agent.file(new File(['synthetic attachment'], 'note.txt', { type: 'text/plain' }));
      return chat.send({ threadId: chat.id, runId: 'complete-remote-chat', messages: [{ id: 'browser-user', role: 'user', content: [{ type: 'text', text: 'Inspect this test file.' }, file] }] });
    }, creation.requestId);
    assert.equal(answer.newMessages.at(-1).content, 'synthetic answer');
    const attachments = executions.get('complete-remote-chat').input.contentParts.filter(part => part.kind === 'attachment');
    assert.equal(attachments.length, 1);
    assert.equal(attachments[0].name, 'note.txt');
    const history = await page.evaluate(async requestId => (await (await window.localApp.agent.chats.open(requestId)).history()).messages, creation.requestId);
    assert.deepEqual(history.map(message => message.role), ['user', 'assistant']);
    const events = await page.evaluate(() => window.localApp.agent.tasks.events({ runId: 'complete-remote-chat' }));
    assert.ok(events.events.some(event => event.type === 'RUN_FINISHED' && event.outcome?.type !== 'cancelled'));
    assert.equal(await page.evaluate(async () => (await window.localApp.agent.tasks.get('complete-remote-chat')).state), 'succeeded');
    const cursor = events.sequence;
    assert.deepEqual(await page.evaluate(afterSequence => window.localApp.agent.tasks.events({ runId: 'complete-remote-chat', afterSequence }), cursor), { events: [], sequence: cursor });

    await page.evaluate(() => localStorage.setItem('business-data', 'first-origin-only'));
    const secondOwner = { ...owner, deviceId: 'second-test-device' };
    const reopened = await host.remoteOpen(appId, secondOwner);
    const newOrigin = new URL(reopened.url).origin;
    assert.notEqual(newOrigin, origin);
    const reopenedPage = await context.newPage();
    await ready(reopenedPage, reopened.url);
    assert.equal(await reopenedPage.evaluate(() => localStorage.getItem('business-data')), null);
    assert.deepEqual(await reopenedPage.evaluate(() => window.localApp.agent.chats.pendingCreations()), []);
    assert.equal(await reopenedPage.evaluate(async input => (await window.localApp.agent.chats.creationStatus(input)).id, creation), creation.requestId);
    assert.deepEqual(await reopenedPage.evaluate(() => window.localApp.agent.chats.pendingCreations()), []);
    assert.deepEqual((await reopenedPage.evaluate(() => window.localApp.agent.chats.list())).chats.map(chat => chat.chatId), [creation.requestId]);
    assert.equal(await page.evaluate(() => localStorage.getItem('business-data')), 'first-origin-only');
    const secondHistory = await reopenedPage.evaluate(async requestId => (await (await window.localApp.agent.chats.open(requestId)).history()).messages, creation.requestId);
    assert.deepEqual(secondHistory, history);

    const startTask = (target, runId) => target.evaluate(runId => window.localApp.agent.tasks.start({ threadId: `thread-${runId}`, runId, messages: [{ id: `user-${runId}`, role: 'user', content: 'Stay active until cancelled.' }] }), runId);
    const waitExecution = async runId => {
      const until = Date.now() + 5000;
      while (!executions.has(runId)) { assert.ok(Date.now() < until, 'synthetic executor did not start'); await new Promise(resolve => setTimeout(resolve, 5)); }
      return executions.get(runId);
    };
    await startTask(reopenedPage, 'explicit-cancel'); await waitExecution('explicit-cancel');
    await reopenedPage.evaluate(() => window.localApp.agent.tasks.cancel('explicit-cancel'));
    assert.equal(executions.get('explicit-cancel').signal.aborted, true);
    assert.equal(await reopenedPage.evaluate(async () => (await window.localApp.agent.tasks.get('explicit-cancel')).state), 'cancelled');
    await startTask(page, 'first-session-active');
    const firstExecution = await waitExecution('first-session-active');
    await ready(page);
    assert.equal(await page.evaluate(async () => (await window.localApp.agent.tasks.get('first-session-active')).state), 'running');
    assert.equal(firstExecution.signal.aborted, false, 'refresh does not revoke durable work');
    await assert.rejects(startTask(reopenedPage, 'second-session-active'), /Application operation is unavailable/);
    await page.evaluate(async () => {
      const reader = (await fetch('/stream')).body.getReader(); await reader.read(); window.streamOutcome = 'pending';
      reader.read().then(value => { window.streamOutcome = value.done ? 'closed' : 'unexpected'; }, () => { window.streamOutcome = 'closed'; });
    });
    if (process.env.LINGXI_TEST_ARTIFACT_DIR) await reopenedPage.screenshot({ path: join(process.env.LINGXI_TEST_ARTIFACT_DIR, 'remote-agent-tls.png') });
    host.remoteRevokeOwner(owner);
    await page.waitForFunction(() => window.streamOutcome === 'closed', null, { timeout: 5000 });
    await reopenedPage.waitForFunction(async () => (await window.localApp.agent.tasks.get('first-session-active')).state === 'cancelled', null, { timeout: 5000 });
    assert.equal(firstExecution.signal.aborted, true);
    assert.equal(await page.evaluate(async () => (await fetch('/')).status), 401);
    await startTask(reopenedPage, 'second-session-active');
    const secondExecution = await waitExecution('second-session-active');
    host.remoteRevokeOwner(owner);
    assert.equal(secondExecution.signal.aborted, false, 'a revoked session must not acquire the replacement session admission');
    assert.equal(await reopenedPage.evaluate(async () => (await window.localApp.agent.tasks.get('second-session-active')).state), 'running');
    await host.cancelAgentApp(appId, 'test permission withdrawal');
    assert.equal(secondExecution.signal.aborted, true);
    assert.equal(await reopenedPage.evaluate(async () => { try { await window.localApp.agent.chats.creationStatus({ requestId: 'b7777777-7777-4777-8777-777777777777', title: '  Browser recovery  ' }); } catch (error) { return error.code; } }), 'host_unavailable');
    await context.close();
  } finally {
    await browser?.close();
    if (host) await host.dispose(); else { await gateway?.dispose(); await broker?.dispose(); }
    if (proxy) { proxy.closeAllConnections(); await new Promise(resolve => proxy.close(resolve)); }
    await rm(directory, { recursive: true, force: true });
  }
});
