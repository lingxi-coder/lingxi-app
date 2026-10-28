import assert from 'node:assert/strict';
import { createServer, request as proxyRequest } from 'node:http';
import { createServer as createHttpsServer } from 'node:https';
import { execFileSync } from 'node:child_process';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import test from 'node:test';
import { LocalAppRemoteGateway } from '../src/main/local-apps/remoteGateway.ts';
import { REMOTE_LOCAL_APP_SDK } from '../src/preload/localAppSdk.ts';

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

test('real browser: remote gateway bootstrap, SDK, app cookies, isolation and revocation', { skip: !playwrightModule && 'Set LINGXI_TEST_PLAYWRIGHT_MODULE to an installed Playwright entry point', timeout: 60_000 }, async () => {
  const { chromium } = await import(pathToFileURL(playwrightModule).href);
  const port = await freePort();
  const gateway = new LocalAppRemoteGateway(`http://{appId}-{sessionId}.localhost:${port}`, port, {
    current: async candidate => candidate.appId === appId && candidate.runId === run.runId && candidate.revision === run.revision,
    invoke: async (_run, method) => { assert.equal(method, 'info'); return { appId, revision: 1 }; },
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
