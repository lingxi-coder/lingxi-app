import { spawn } from 'node:child_process';
import { randomUUID, timingSafeEqual } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:https';
import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';

const maximumIdle = 5 * 60_000;
const maximumLifetime = 30 * 60_000;
const bodyLimit = 3 * 1024 * 1024;
const qualifiedImage = /^(?:[a-z0-9][a-z0-9./:_-]*@)?sha256:[a-f0-9]{64}$/u;
const scopeDigest = /^[a-f0-9]{64}$/u;

function openWorker(image) {
  const name = 'lingxi-dataset-' + randomUUID();
  const child = spawn('/usr/bin/docker', ['run', '--rm', '-i', '--name', name, '--pull', 'never',
    '--runtime', 'runsc', '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
    '--memory', '640m', '--cpus', '1', '--pids-limit', '128', '--tmpfs', '/tmp:rw,nosuid,nodev,size=128m', image],
  { stdio: ['pipe', 'pipe', 'ignore'] });
  const lines = createInterface({ input: child.stdout });
  let answer;
  function clear(error, value) {
    if (!answer) return;
    clearTimeout(answer.timer);
    if (error) answer.reject(error); else answer.resolve(value);
    answer = undefined;
  }
  function stop() {
    clear(new Error('Renderer closed'));
    lines.close();
    child.stdin.end();
    child.kill('SIGTERM');
    spawn('/usr/bin/docker', ['rm', '-f', name], { stdio: 'ignore' }).on('error', () => {});
  }
  lines.on('line', line => {
    if (line.length > 5 * 1024 * 1024) { stop(); return; }
    try { clear(null, JSON.parse(line)); } catch { stop(); }
  });
  child.on('exit', () => clear(new Error('Renderer exited')));
  child.on('error', () => clear(new Error('Renderer unavailable')));
  child.stdin.on('error', () => clear(new Error('Renderer unavailable')));
  return {
    request(value) {
      if (answer) return Promise.reject(new Error('Renderer busy'));
      return new Promise((resolve, reject) => {
        answer = { resolve, reject, timer: setTimeout(stop, 10_000) };
        child.stdin.write(JSON.stringify(value) + '\n');
      });
    },
    stop,
  };
}

function sharedSecret(value, expected) {
  const candidate = Buffer.from(value);
  return candidate.length === expected.length && timingSafeEqual(candidate, expected);
}

function error(status, code) {
  return Object.assign(new Error(code), { status, code });
}

export function createRenderer({ secret, worker, clock = Date.now }) {
  if (!/^[A-Za-z0-9_-]{43}$/u.test(secret)) throw new Error('A dedicated renderer credential is required');
  const expected = Buffer.from('Bearer ' + secret);
  const sessions = new Map();
  function close(id, value) {
    sessions.delete(id);
    value.worker.stop();
  }
  function collect() {
    const now = clock();
    for (const [id, value] of sessions) {
      if (value.accessed + maximumIdle < now || value.created + maximumLifetime < now) close(id, value);
    }
  }
  async function operate(path, body) {
    collect();
    if (!scopeDigest.test(body?.owner ?? '') || !scopeDigest.test(body?.scope ?? '') ||
        (path !== '/close' && !scopeDigest.test(body?.tenant ?? ''))) throw error(400, 'RENDER_INVALID');
    if (path === '/open') {
      const sameTenant = [...sessions.values()].filter(value => value.tenant === body.tenant);
      const sameOwner = [...sessions.values()].filter(value => value.owner === body.owner);
      if (sameTenant.length >= 20 || sameOwner.length >= 2) throw error(429, 'RENDER_QUOTA');
      const { width, height } = body.dimensions ?? {};
      if (!Number.isInteger(width) || !Number.isInteger(height) || width < 176 || width > 1440 || height < 160 || height > 900 ||
          typeof body.bundle?.html !== 'string' || !body.bundle?.assets || !body.bundle?.actions) throw error(400, 'RENDER_INVALID');
      const id = randomUUID();
      const value = { owner: body.owner, tenant: body.tenant, scope: body.scope, created: clock(), accessed: clock(), worker: worker(), busy: true };
      sessions.set(id, value);
      try {
        const frame = await value.worker.request({ action: 'mount', bundle: body.bundle, state: body.state,
          stateVersion: body.stateVersion, dimensions: body.dimensions });
        if (frame.error) throw error(503, frame.error);
        return { ...frame, session: id };
      } catch (failure) {
        close(id, value);
        throw failure;
      } finally { value.busy = false; }
    }
    const value = sessions.get(body.session);
    if (!value || value.owner !== body.owner || value.scope !== body.scope ||
        (path !== '/close' && value.tenant !== body.tenant)) throw error(404, 'RENDER_GONE');
    if (path === '/close') { close(body.session, value); return { closed: true }; }
    if (path !== '/frame' || !['frame', 'input', 'result'].includes(body.command?.action)) throw error(400, 'RENDER_INVALID');
    if (value.busy) throw error(409, 'RENDER_BUSY');
    value.busy = true;
    value.accessed = clock();
    try {
      return await value.worker.request(body.command);
    } catch (failure) {
      close(body.session, value);
      throw failure;
    } finally { value.busy = false; }
  }
  const interval = setInterval(collect, 30_000);
  interval.unref();
  return {
    async fetch(request) {
      const headers = { 'Cache-Control': 'no-store' };
      const reply = (value, status = 200) => Response.json(value, { status, headers });
      if (request.method !== 'POST' || !sharedSecret(request.headers.get('Authorization') ?? '', expected)) {
        return reply({ error: 'RENDER_FORBIDDEN' }, 403);
      }
      try {
        const body = await request.text();
        if (Buffer.byteLength(body) > bodyLimit) return reply({ error: 'RENDER_INVALID' }, 413);
        return reply(await operate(new URL(request.url).pathname, JSON.parse(body)));
      } catch (failure) { return reply({ error: failure.code ?? 'RENDER_UNAVAILABLE' }, failure.status ?? 503); }
    },
    dispose() {
      clearInterval(interval);
      for (const [id, value] of sessions) close(id, value);
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const image = process.env.LINGXI_RENDER_IMAGE;
  if (!qualifiedImage.test(image ?? '')) throw new Error('Expected an immutable renderer worker image');
  const renderer = createRenderer({ secret: readFileSync(process.env.LINGXI_RENDER_KEY_FILE, 'utf8').trim(), worker: () => openWorker(image) });
  createServer({ key: readFileSync(process.env.LINGXI_RENDER_TLS_KEY), cert: readFileSync(process.env.LINGXI_RENDER_TLS_CERT) }, async (input, output) => {
    const chunks = [];
    let bytes = 0;
    try {
      for await (const chunk of input) {
        bytes += chunk.length;
        if (bytes > bodyLimit) { output.writeHead(413).end(); return; }
        chunks.push(chunk);
      }
      const request = new Request(new URL(input.url, 'https://renderer.local'), { method: input.method, headers: input.headers,
        ...(chunks.length && ['POST', 'PUT'].includes(input.method) ? { body: Buffer.concat(chunks) } : {}) });
      const reply = await renderer.fetch(request);
      output.writeHead(reply.status, Object.fromEntries(reply.headers)).end(Buffer.from(await reply.arrayBuffer()));
    } catch { output.writeHead(503).end(); }
  }).listen(Number(process.env.PORT ?? 18444), process.env.LINGXI_RENDER_BIND ?? '127.0.0.1');
  process.once('SIGTERM', () => { renderer.dispose(); process.exit(0); });
}
