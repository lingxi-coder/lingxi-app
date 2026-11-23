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
  let stopping;
  function clear(error, value) {
    if (!answer) return;
    clearTimeout(answer.timer);
    if (error) answer.reject(error); else answer.resolve(value);
    answer = undefined;
  }
  function stop() {
    if (stopping) return stopping;
    clear(new Error('Renderer closed'));
    if (!lines.closed) lines.close();
    if (!child.stdin.writableEnded) child.stdin.end();
    if (!child.killed) child.kill('SIGTERM');
    stopping = new Promise((resolve, reject) => {
      const removal = spawn('/usr/bin/docker', ['rm', '-f', name], {
        signal: AbortSignal.timeout(5000), stdio: ['ignore', 'ignore', 'pipe'],
      });
      let diagnostic = '';
      removal.stderr.setEncoding('utf8');
      removal.stderr.on('data', chunk => { diagnostic = (diagnostic + chunk).slice(-1024); });
      removal.once('error', reject);
      removal.once('close', code => {
        if (code === 0 || diagnostic.includes('No such container')) resolve();
        else reject(new Error('Renderer container removal failed'));
      });
    });
    // Timeouts also call stop directly. The supervisor observes the same promise.
    void stopping.catch(() => { stopping = undefined; });
    return stopping;
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

export function createRenderer({ secret, worker, clock = Date.now, capacity = 2 }) {
  if (!/^[A-Za-z0-9_-]{43}$/u.test(secret)) throw new Error('A dedicated renderer credential is required');
  if (!Number.isSafeInteger(capacity) || capacity < 1) throw new Error('Invalid renderer capacity');
  const expected = Buffer.from('Bearer ' + secret);
  const sessions = new Map();
  const closing = new Map();
  let nextCollection = Infinity;
  function removeWorker(id, entry) {
    if (entry.pending) return entry.pending;
    entry.pending = Promise.resolve().then(() => entry.value.worker.stop()).then(() => {
      closing.delete(id);
      return true;
    }, () => {
      entry.retryAt = clock() + 5000;
      console.error('Renderer worker cleanup failed. Retrying after backoff.');
      return false;
    }).finally(() => { entry.pending = undefined; });
    return entry.pending;
  }
  function close(id, value) {
    if (!sessions.delete(id)) return closing.get(id)?.pending;
    const entry = { value, retryAt: clock() };
    closing.set(id, entry);
    return removeWorker(id, entry);
  }
  function expires(value) {
    return Math.min(value.accessed + maximumIdle, value.created + maximumLifetime);
  }
  function collect() {
    const now = clock();
    for (const [id, entry] of closing) {
      if (entry.retryAt <= now) void removeWorker(id, entry);
    }
    if (now <= nextCollection) return;
    nextCollection = Infinity;
    for (const [id, value] of sessions) {
      const expiry = expires(value);
      if (expiry < now) void close(id, value);
      else nextCollection = Math.min(nextCollection, expiry);
    }
  }
  async function operate(path, body) {
    collect();
    if (!scopeDigest.test(body?.owner ?? '') || !scopeDigest.test(body?.scope ?? '') ||
        (path !== '/close' && !scopeDigest.test(body?.tenant ?? ''))) throw error(400, 'RENDER_INVALID');
    if (path === '/open') {
      if (sessions.size + closing.size >= capacity) throw error(429, 'RENDER_QUOTA');
      const sameTenant = [...sessions.values()].filter(value => value.tenant === body.tenant);
      const sameOwner = [...sessions.values()].filter(value => value.owner === body.owner);
      if (sameTenant.length >= 20 || sameOwner.length >= 2) throw error(429, 'RENDER_QUOTA');
      const { width, height } = body.dimensions ?? {};
      if (!Number.isInteger(width) || !Number.isInteger(height) || width < 176 || width > 1440 || height < 160 || height > 900 ||
          typeof body.bundle?.html !== 'string' || !body.bundle?.assets || !body.bundle?.actions) throw error(400, 'RENDER_INVALID');
      const id = randomUUID();
      const value = { owner: body.owner, tenant: body.tenant, scope: body.scope, created: clock(), accessed: clock(), worker: worker(), busy: true };
      sessions.set(id, value);
      nextCollection = Math.min(nextCollection, expires(value));
      try {
        const frame = await value.worker.request({ action: 'mount', bundle: body.bundle, state: body.state,
          stateVersion: body.stateVersion, dimensions: body.dimensions, deferFrame: body.deferFrame === true });
        if (frame.error) throw error(503, frame.error);
        return { ...frame, session: id };
      } catch (failure) {
        await close(id, value);
        throw failure;
      } finally { value.busy = false; }
    }
    const value = sessions.get(body.session);
    if (!value || value.owner !== body.owner || value.scope !== body.scope ||
        (path !== '/close' && value.tenant !== body.tenant)) throw error(404, 'RENDER_GONE');
    if (path === '/close') {
      if (!await close(body.session, value)) throw error(503, 'RENDER_UNAVAILABLE');
      return { closed: true };
    }
    if (path !== '/frame' || !['frame', 'input', 'result'].includes(body.command?.action)) throw error(400, 'RENDER_INVALID');
    if (value.busy) throw error(409, 'RENDER_BUSY');
    value.busy = true;
    value.accessed = clock();
    try {
      return await value.worker.request(body.command);
    } catch (failure) {
      await close(body.session, value);
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
    async dispose() {
      clearInterval(interval);
      await Promise.all([...closing].map(([id, entry]) => removeWorker(id, entry))
        .concat([...sessions].map(([id, value]) => close(id, value))));
    },
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const image = process.env.LINGXI_RENDER_IMAGE;
  if (!qualifiedImage.test(image ?? '')) throw new Error('Expected an immutable renderer worker image');
  const renderer = createRenderer({ secret: readFileSync(process.env.LINGXI_RENDER_KEY_FILE, 'utf8').trim(), worker: () => openWorker(image),
    capacity: Number(process.env.LINGXI_RENDER_CAPACITY ?? 2) });
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
  process.once('SIGTERM', () => { void renderer.dispose().finally(() => process.exit(0)); });
}
