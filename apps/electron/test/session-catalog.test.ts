import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { ProjectSessionCatalog } from '../src/main/session-catalog';

function fakeChild(stdoutBody: string) {
  const child = new EventEmitter() as EventEmitter & {
    stdout: EventEmitter;
    stderr: EventEmitter;
    kill(): boolean;
  };
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  child.kill = () => true;
  queueMicrotask(() => {
    child.stdout.emit('data', Buffer.from(stdoutBody));
    child.emit('close', 0, null);
  });
  return child;
}

test('product paths use native product discovery without merging workspace settings', async () => {
  const configHome = join(tmpdir(), '.another-product');
  let flags: readonly string[] | undefined;
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server',
    spawnProcess: ((_bin: string, args: readonly string[]) => {
      flags = args;
      return fakeChild(JSON.stringify({ version: 1, config_home: configHome, app_projects_root: join(configHome, '.apps') }));
    }) as any });
  assert.deepEqual(await catalog.productPaths(), { configHome, appProjectsRoot: join(configHome, '.apps') });
  assert.deepEqual(flags, ['--product-paths-json']);
});

test('product directory discovery rejects malformed and nonabsolute native paths', async () => {
  const configHome = join(tmpdir(), '.another-product');
  const valid = { version: 1, config_home: configHome, app_projects_root: join(configHome, '.apps') };
  for (const response of ['{', JSON.stringify({ ...valid, version: 2 }),
    JSON.stringify({ ...valid, config_home: 'relative' }), JSON.stringify({ ...valid, app_projects_root: 'relative' }),
    JSON.stringify({ ...valid, config_home: configHome + '\0' })]) {
    const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server',
      spawnProcess: (() => fakeChild(response)) as any });
    await assert.rejects(catalog.productPaths(), /invalid product paths response/);
  }
});

test('project catalog uses the allowlisted environment and validates session UUIDs', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-project-'));
  let spawnedEnv: NodeJS.ProcessEnv | undefined;
  const validId = '11111111-2222-4333-8444-555555555555';
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: ((_bin: string, _args: readonly string[], options: { env?: NodeJS.ProcessEnv }) => {
      spawnedEnv = options.env;
      return fakeChild(JSON.stringify({
        version: 1,
        sessions: [{
          uuid: validId,
          title: 'Session',
          modified_rfc3339: '',
          message_count: 1,
          mode: 'chat',
          path: 'session.jsonl',
          empty_session: false,
          resume_model: 'openrouter/cohere/north-mini-code:free',
        }],
      }));
    }) as any,
  });
  const previousSecret = process.env['ANTHROPIC_API_KEY'];
  process.env['ANTHROPIC_API_KEY'] = 'must-not-cross-catalog-boundary';
  try {
    const result = await catalog.list(project);
    assert.equal(result.sessions[0]?.uuid, validId);
    assert.equal(result.sessions[0]?.mode, 'chat');
    assert.equal(result.sessions[0]?.resume_model, 'openrouter/cohere/north-mini-code:free');
    assert.equal(spawnedEnv?.['ANTHROPIC_API_KEY'], undefined);
  } finally {
    if (previousSecret === undefined) delete process.env['ANTHROPIC_API_KEY'];
    else process.env['ANTHROPIC_API_KEY'] = previousSecret;
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog defaults missing mode metadata to code for older payloads', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-legacy-mode-'));
  const validId = '11111111-2222-4333-8444-555555555557';
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({
      version: 1,
      sessions: [{ uuid: validId, title: 'Legacy', modified_rfc3339: '', message_count: 1, path: 'legacy.jsonl', empty_session: false }],
    }))) as any,
  });
  try {
    const result = await catalog.list(project);
    assert.equal(result.sessions[0]?.mode, 'code');
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog rejects malformed session identities', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-invalid-'));
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({
      version: 1,
      sessions: [{ uuid: 'not-a-uuid', title: 'Bad', modified_rfc3339: '', message_count: 1, path: 'bad.jsonl', empty_session: false }],
    }))) as any,
  });
  try {
    await assert.rejects(catalog.list(project), /invalid session row/);
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog keeps the UI list bounded but supports exact lookup beyond the bound', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-exact-'));
  const rows = Array.from({ length: 201 }, (_, index) => ({
    uuid: `00000000-0000-4${String(index).padStart(3, '0')}-8000-${String(index).padStart(12, '0')}`,
    title: `Session ${index}`,
    modified_rfc3339: '',
    message_count: 1,
    path: `session-${index}.jsonl`,
    empty_session: false,
  }));
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({ version: 1, sessions: rows }))) as any,
  });
  const outsideBound = rows[200].uuid;
  try {
    const listed = await catalog.list(project);
    assert.equal(listed.sessions.length, 200);
    assert.equal(listed.sessions.some((session) => session.uuid === outsideBound), false);
    assert.equal((await catalog.find(project, outsideBound))?.title, 'Session 200');
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog preserves validated zero-count rows and applies the sidebar limit to catalog output', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-resumable-'));
  const emptyAnchor = {
    uuid: '11111111-2222-4333-8444-555555555556',
    title: 'New mobile session',
    modified_rfc3339: '2026-08-26T00:00:00Z',
    message_count: 0,
    path: 'empty.jsonl',
    empty_session: true,
  };
  const metadataOnly = Array.from({ length: 200 }, (_, index) => ({
    uuid: `10000000-0000-4${String(index).padStart(3, '0')}-8000-${String(index).padStart(12, '0')}`,
    title: 'Control Rename',
    modified_rfc3339: `2026-08-26T00:${String(index % 60).padStart(2, '0')}:00Z`,
    message_count: 0,
    path: `metadata-${index}.jsonl`,
    empty_session: false,
  }));
  const resumable = {
    uuid: '22222222-3333-4444-8555-666666666666',
    title: 'Inspect the session catalog',
    modified_rfc3339: '2026-08-25T00:00:00Z',
    message_count: 2,
    path: 'conversation.jsonl',
    empty_session: false,
  };
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({
      version: 1,
      sessions: [emptyAnchor, ...metadataOnly, resumable],
    }))) as any,
  });
  try {
    const listed = await catalog.list(project);
    assert.equal(listed.sessions[0]?.uuid, emptyAnchor.uuid);
    assert.equal(listed.sessions.length, 200);
    assert.equal((await catalog.find(project, emptyAnchor.uuid))?.message_count, 0);
    assert.equal((await catalog.find(project, emptyAnchor.uuid))?.empty_session, true);
    assert.equal((await catalog.find(project, metadataOnly[0]!.uuid))?.empty_session, false);
    assert.equal((await catalog.find(project, resumable.uuid))?.title, resumable.title);
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

test('project catalog coalesces concurrent reads and invalidates after a mutation', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-cache-'));
  const uuid = '33333333-4444-4555-8666-777777777777';
  let spawns = 0;
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: (() => {
      spawns += 1;
      return fakeChild(JSON.stringify({
        version: 1,
        sessions: [{ uuid, title: 'Cached', modified_rfc3339: '', message_count: 1, path: 'cached.jsonl', empty_session: false }],
      }));
    }) as any,
  });
  try {
    const [listed, repeated] = await Promise.all([
      catalog.list(project),
      catalog.list(project),
    ]);
    assert.equal(spawns, 1);
    assert.equal(listed.sessions[0]?.uuid, uuid);
    assert.equal(repeated.sessions[0]?.title, 'Cached');
    assert.equal((await catalog.find(project, uuid))?.uuid, uuid, 'a fresh catalog answers lookups');
    assert.equal(spawns, 1);
    await catalog.list(project);
    assert.equal(spawns, 1);
    catalog.invalidate(project);
    await catalog.list(project);
    assert.equal(spawns, 2);
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

test('session lookups without a fresh catalog read only that session', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-narrowed-'));
  const uuid = '44444444-5555-4666-8777-888888888888';
  const spawned: string[][] = [];
  const catalog = new ProjectSessionCatalog({
    serverBin: '/bridge-server',
    spawnProcess: ((_bin: string, args: readonly string[]) => {
      spawned.push([...args]);
      return fakeChild(JSON.stringify({
        version: 1,
        sessions: [{ uuid, title: 'Narrowed', modified_rfc3339: '', message_count: 1, path: 'narrowed.jsonl', empty_session: false }],
      }));
    }) as any,
  });
  try {
    const [first, second] = await Promise.all([catalog.find(project, uuid), catalog.find(project, uuid)]);
    assert.equal(first?.title, 'Narrowed');
    assert.equal(second?.title, 'Narrowed');
    assert.equal(spawned.length, 1, 'concurrent lookups of one session share a process');
    assert.deepEqual(spawned[0]?.slice(-3), ['--list-sessions-json', '--session-id', uuid]);
    assert.equal(await catalog.find(project, '55555555-6666-4777-8888-999999999999'), undefined);
    await catalog.list(project);
    assert.equal(spawned.at(-1)?.includes('--session-id'), false, 'a one-row answer never stands in for the catalog');
  } finally {
    rmSync(project, { recursive: true, force: true });
  }
});

function chunkedChild(buffers: Buffer[], code = 0, stderr = false) {
  const child = new EventEmitter() as EventEmitter & { stdout: EventEmitter; stderr: EventEmitter; kill(): boolean };
  child.stdout = new EventEmitter();
  child.stderr = new EventEmitter();
  child.kill = () => true;
  queueMicrotask(() => {
    for (const buffer of buffers) (stderr ? child.stderr : child.stdout).emit('data', buffer);
    child.emit('close', code, null);
  });
  return child;
}

test('catalog decodes split UTF-8 titles and transcript paths only after collecting raw bytes', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-unicode-'));
  const row = { uuid: '11111111-2222-4333-8444-555555555555', title: '中文标题😀', path: '/session/会话.jsonl', modified_rfc3339: '', message_count: 1, empty_session: false };
  const body = Buffer.from(JSON.stringify({ version: 1, sessions: [row] }));
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server',
    spawnProcess: (() => chunkedChild(Array.from(body, (byte) => Buffer.from([byte])))) as any });
  try {
    const listed = await catalog.list(project);
    assert.equal(listed.sessions[0].title, row.title);
    assert.equal(listed.sessions[0].path, row.path);
    assert.equal((await catalog.find(project, row.uuid))?.path, row.path);
  } finally { rmSync(project, { recursive: true, force: true }); }
});

test('catalog failure diagnostics preserve Unicode across stderr chunk boundaries', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-unicode-error-'));
  const message = '错误：会话目录不可用';
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server',
    spawnProcess: (() => chunkedChild(Array.from(Buffer.from(message), (byte) => Buffer.from([byte])), 1, true)) as any });
  try { await assert.rejects(catalog.list(project), (error) => error instanceof Error && error.message.endsWith(message)); }
  finally { rmSync(project, { recursive: true, force: true }); }
});

test('catalog rejects the raw stdout byte limit immediately even when the child never closes', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-limit-'));
  let kills = 0;
  const child = new EventEmitter() as EventEmitter & { stdout: EventEmitter; stderr: EventEmitter; kill(): boolean };
  child.stdout = new EventEmitter(); child.stderr = new EventEmitter(); child.kill = () => { kills++; return true; };
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server', spawnProcess: (() => child) as any });
  try {
    const load = catalog.list(project);
    const rejected = assert.rejects(load, /output is too large/);
    child.stdout.emit('data', Buffer.alloc(8 * 1024 * 1024));
    child.stdout.emit('data', Buffer.from('中'));
    await rejected;
    assert.equal(kills, 1);
    child.stdout.emit('data', Buffer.alloc(1024));
    assert.equal(kills, 1, 'late output cannot grow the settled response');
  } finally { rmSync(project, { recursive: true, force: true }); }
});

test('catalog retains a bounded UTF-8 stderr tail with the final diagnostic', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-stderr-tail-'));
  const message = '错误：最终诊断';
  const buffers = [Buffer.from('中文'.repeat(4000)), Buffer.from(message)];
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server', spawnProcess: (() => chunkedChild(buffers, 1, true)) as any });
  try {
    await assert.rejects(catalog.list(project), (error) => {
      assert.ok(error instanceof Error);
      assert.ok(error.message.endsWith(message));
      assert.ok(!error.message.includes('�'));
      assert.ok(Buffer.byteLength(error.message) < 16 * 1024 + 80);
      return true;
    });
  } finally { rmSync(project, { recursive: true, force: true }); }
});

test('a catalog of another configuration home reads that home in every mode, and the default one reads the environment\'s', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-config-home-'));
  const environments: Array<NodeJS.ProcessEnv | undefined> = [];
  const spawnProcess = ((_bin: string, args: readonly string[], options: { env?: NodeJS.ProcessEnv }) => {
    environments.push(options.env);
    const configHome = options.env?.['LINGXI_CONFIG_DIR'] ?? '/owner/.lingxi';
    return fakeChild(args.includes('--product-paths-json')
      ? JSON.stringify({ version: 1, config_home: configHome, app_projects_root: join(configHome, '.apps') })
      : args.includes('--workspace-environment-json')
        ? JSON.stringify({ version: 1, environment: {}, config_home: configHome, global_config_path: join(configHome, '.lingxi.json') })
        : JSON.stringify({ version: 1, sessions: [] }));
  }) as any;
  const previous = process.env['LINGXI_CONFIG_DIR'];
  process.env['LINGXI_CONFIG_DIR'] = '/owner/.lingxi';
  try {
    const botHome = join(project, 'bot-home');
    const bot = new ProjectSessionCatalog({ serverBin: '/bridge-server', configHome: botHome, spawnProcess });
    const owner = new ProjectSessionCatalog({ serverBin: '/bridge-server', spawnProcess });
    assert.equal((await bot.productPaths()).configHome, botHome);
    assert.equal((await bot.workspaceSettings(project)).configHome, botHome);
    await bot.list(project);
    assert.deepEqual(environments.map((env) => env?.['LINGXI_CONFIG_DIR']), [botHome, botHome, botHome]);
    environments.length = 0;
    assert.equal((await owner.productPaths()).configHome, '/owner/.lingxi');
    await owner.list(project);
    assert.deepEqual(environments.map((env) => env?.['LINGXI_CONFIG_DIR']), ['/owner/.lingxi', '/owner/.lingxi']);
  } finally {
    if (previous === undefined) delete process.env['LINGXI_CONFIG_DIR']; else process.env['LINGXI_CONFIG_DIR'] = previous;
    rmSync(project, { recursive: true, force: true });
  }
});
