import { PassThrough } from 'node:stream';
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

test('exact selections pass their path to separate catalog queries without a UUID fallback', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-source-'));
  const id = '11111111-2222-4333-8444-555555555555';
  const sourceA = join(project, 'a', `${id}.jsonl`), sourceB = join(project, 'b', `${id}.jsonl`);
  const calls: string[][] = [];
  let missingA = false, wrongCopy = false;
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server',
    spawnProcess: ((_bin: string, args: readonly string[]) => {
      calls.push([...args]);
      const path = args[args.indexOf('--session-transcript-path') + 1];
      const selected = wrongCopy ? sourceB : path;
      return fakeChild(JSON.stringify({ version: 1, sessions: missingA && path === sourceA ? [] : [{
        uuid: id, title: selected === sourceA ? 'A' : 'B', modified_rfc3339: '', message_count: 1,
        mode: 'code', path: selected, empty_session: false,
      }] }));
    }) as any });
  try {
    const [a, b] = await Promise.all([
      catalog.select(project, id, { scope: 'current', transcriptPath: sourceA }),
      catalog.select(project, id, { scope: 'current', transcriptPath: sourceB }),
    ]);
    assert.equal(a?.path, sourceA);
    assert.equal(b?.path, sourceB);
    assert.equal(calls.length, 2, 'separate physical sources must not share a pending lookup');
    for (const [index, path] of [sourceA, sourceB].entries()) {
      assert.deepEqual(calls[index]?.slice(-6), ['--session-scope', 'current', '--session-id', id, '--session-transcript-path', path]);
    }
    missingA = true;
    assert.equal(await catalog.select(project, id, { scope: 'current', transcriptPath: sourceA }), undefined);
    assert.equal(calls.length, 3, 'a missing source must not trigger an unscoped UUID retry');
    missingA = false; wrongCopy = true;
    await assert.rejects(catalog.select(project, id, { scope: 'current', transcriptPath: sourceA }), /selection has changed/);
    const before = calls.length;
    await assert.rejects(catalog.select(project, id, { scope: 'current', transcriptPath: 'relative.jsonl' }), /invalid selected transcript path/);
    assert.equal(calls.length, before);
  } finally { rmSync(project, { recursive: true, force: true }); }
});

test('product paths use native product discovery without merging workspace settings', async () => {
  const brandHome = join(tmpdir(), '.another-product');
  const globalConfigPath = join(tmpdir(), 'another-product-config.json');
  let flags: readonly string[] | undefined;
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server',
    spawnProcess: ((_bin: string, args: readonly string[]) => {
      flags = args;
      return fakeChild(JSON.stringify({ version: 2, brand_home: brandHome, workspace_root: brandHome + '-workspaces', app_projects_root: join(brandHome + '-workspaces', 'apps'), global_config_path: globalConfigPath }));
    }) as any });
  assert.deepEqual(await catalog.productPaths(), { brandHome, workspaceRoot: brandHome + '-workspaces', appProjectsRoot: join(brandHome + '-workspaces', 'apps'), globalConfigPath });
  assert.deepEqual(flags, ['--product-paths-json']);
});

test('product directory discovery rejects malformed and nonabsolute native paths', async () => {
  const brandHome = join(tmpdir(), '.another-product');
  const valid = { version: 2, brand_home: brandHome, workspace_root: brandHome + '-workspaces', app_projects_root: join(brandHome + '-workspaces', 'apps'), global_config_path: null };
  for (const response of ['{', JSON.stringify({ ...valid, version: 1 }), JSON.stringify({ ...valid, workspace_root: undefined }), JSON.stringify({ ...valid, workspace_root: 'relative' }),
    JSON.stringify({ ...valid, brand_home: 'relative' }), JSON.stringify({ ...valid, app_projects_root: 'relative' }),
    JSON.stringify({ ...valid, brand_home: brandHome + '\0' }),
    JSON.stringify({ ...valid, global_config_path: 'relative' }), JSON.stringify({ ...valid, global_config_path: undefined }),
    JSON.stringify({ ...valid, global_config_path: brandHome + '\0' })]) {
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

test('project catalog rejects missing mode metadata instead of guessing', async () => {
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
    await assert.rejects(catalog.list(project), /invalid session row/);
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
      sessions: [{ uuid: 'not-a-uuid', title: 'Bad', modified_rfc3339: '', message_count: 1, mode: 'code', path: 'bad.jsonl', empty_session: false }],
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
    message_count: 1, mode: 'code',
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

test('each space is limited after hidden sessions are excluded', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-spaces-'));
  const rows = Array.from({ length: 405 }, (_, index) => ({
    uuid: `00000000-0000-4${index.toString(16).padStart(3, '0')}-8000-${index.toString(16).padStart(12, '0')}`,
    title: `Session ${index}`, modified_rfc3339: '', message_count: 1,
    mode: index < 203 ? 'code' : 'chat', path: `session-${index}.jsonl`, empty_session: false,
  }));
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server',
    spawnProcess: (() => fakeChild(JSON.stringify({ version: 1, sessions: rows }))) as any });
  try {
    const listed = await catalog.list(project, 'current', (row) => row.uuid !== rows[0]!.uuid && row.uuid !== rows[203]!.uuid);
    assert.equal(listed.sessions.filter((row) => row.mode === 'code').length, 200);
    assert.equal(listed.sessions.filter((row) => row.mode === 'chat').length, 200);
    assert.ok(listed.sessions.some((row) => row.uuid === rows[200]!.uuid));
    assert.ok(listed.sessions.some((row) => row.uuid === rows[403]!.uuid));
    assert.ok(!listed.sessions.some((row) => row.uuid === rows[0]!.uuid || row.uuid === rows[203]!.uuid));
  } finally { rmSync(project, { recursive: true, force: true }); }
});

test('project catalog preserves validated zero-count rows and applies the sidebar limit to catalog output', async () => {
  const project = mkdtempSync(join(tmpdir(), 'lingxi-catalog-resumable-'));
  const emptyAnchor = {
    uuid: '11111111-2222-4333-8444-555555555556',
    title: 'New mobile session',
    modified_rfc3339: '2026-08-26T00:00:00Z',
    message_count: 0, mode: 'code',
    path: 'empty.jsonl',
    empty_session: true,
  };
  const metadataOnly = Array.from({ length: 200 }, (_, index) => ({
    uuid: `10000000-0000-4${String(index).padStart(3, '0')}-8000-${String(index).padStart(12, '0')}`,
    title: 'Control Rename',
    modified_rfc3339: `2026-08-26T00:${String(index % 60).padStart(2, '0')}:00Z`,
    message_count: 0, mode: 'code',
    path: `metadata-${index}.jsonl`,
    empty_session: false,
  }));
  const resumable = {
    uuid: '22222222-3333-4444-8555-666666666666',
    title: 'Inspect the session catalog',
    modified_rfc3339: '2026-08-25T00:00:00Z',
    message_count: 2, mode: 'code',
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
        sessions: [{ uuid, title: 'Cached', modified_rfc3339: '', message_count: 1, mode: 'code', path: 'cached.jsonl', empty_session: false }],
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
        sessions: [{ uuid, title: 'Narrowed', modified_rfc3339: '', message_count: 1, mode: 'code', path: 'narrowed.jsonl', empty_session: false }],
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
  const row = { uuid: '11111111-2222-4333-8444-555555555555', title: '中文标题😀', path: '/session/会话.jsonl', modified_rfc3339: '', message_count: 1, mode: 'code', empty_session: false };
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
      ? JSON.stringify({ version: 2, brand_home: configHome, workspace_root: configHome + '-workspaces', app_projects_root: join(configHome + '-workspaces', 'apps'), global_config_path: null })
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
    assert.equal((await bot.productPaths()).brandHome, botHome);
    assert.equal((await bot.workspaceSettings(project)).configHome, botHome);
    await bot.list(project);
    assert.deepEqual(environments.map((env) => env?.['LINGXI_CONFIG_DIR']), [botHome, botHome, botHome]);
    environments.length = 0;
    assert.equal((await owner.productPaths()).brandHome, '/owner/.lingxi');
    await owner.list(project);
    assert.deepEqual(environments.map((env) => env?.['LINGXI_CONFIG_DIR']), ['/owner/.lingxi', '/owner/.lingxi']);
  } finally {
    if (previous === undefined) delete process.env['LINGXI_CONFIG_DIR']; else process.env['LINGXI_CONFIG_DIR'] = previous;
    rmSync(project, { recursive: true, force: true });
  }
});


test('account helper uses Host settings and sends secret input only on stdin', async () => {
  let flags: readonly string[] | undefined, envelope: any;
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server', providerAccountChannel: 'production', appVersion: '1.2.3',
    spawnProcess: ((_bin: string, args: readonly string[], options: any) => {
      flags = args;
      assert.deepEqual(options.stdio, ['pipe', 'pipe', 'pipe']);
      const child = fakeChild(JSON.stringify({ storeScope: 'host-production', configRevision: 2, accounts: [], health: [] })) as any;
      child.stdin = new PassThrough(); child.stdin.on('data', bytes => { envelope = JSON.parse(bytes.toString()); });
      return child;
    }) as any });
  await catalog.providerAccountCommand({ action: 'enable', expectedRevision: 1, accountId: 'account-a', enabled: false, displayName: 'irrelevant' });
  assert.deepEqual(flags, ['--provider-account-command']);
  assert.equal(envelope.credential_channel, 'production');
  assert.equal(envelope.backend, 'desktop-broker');
  assert.equal(envelope.metadata_only, true);
  assert.equal(Object.hasOwn(envelope, 'profiles_json'), false);
  assert.equal(Object.hasOwn(envelope, 'provider_region'), false);
  assert.equal(envelope.app_version, '1.2.3');
  assert.equal(envelope.credential, undefined);
  assert.deepEqual(JSON.parse(envelope.request_json), { action: 'set_enabled', expectedRevision: 1, accountId: 'account-a', enabled: false });
  const credential = JSON.stringify({ credential_revision: 1, identity_generation: 1, kind: { kind: 'api_key', key: 'private-test-credential' } });
  await catalog.providerAccountCommand({ action: 'test', expectedRevision: 2, accountId: 'account-a' }, credential);
  assert.equal(envelope.credential, credential);
  assert.deepEqual(JSON.parse(envelope.request_json), { action: 'test', expectedRevision: 2, accountId: 'account-a' });
  assert.equal(JSON.stringify(flags).includes('private-test-credential'), false);
  await catalog.providerAccountCommand({ action: 'list', expectedRevision: 2 }, undefined, ['account-a']);
  assert.deepEqual(envelope.credential_presence, ['account-a']);
  assert.equal(Object.hasOwn(envelope, 'profiles_json'), false);
  assert.equal(Object.hasOwn(envelope, 'provider_region'), false);
  assert.equal(envelope.app_version, '1.2.3');
  assert.equal(envelope.credential, undefined);
  await assert.rejects(catalog.providerAccountCommand({ action: 'add', expectedRevision: 2 }, credential), /do not accept credentials/);
  await assert.rejects(catalog.providerAccountCommand({ action: 'enable', expectedRevision: 2 }, undefined, []), /only available for account listing/);
});

test('account helper ignores an unavailable runtime and uses the persisted Host home', async () => {
  let envelope: any;
  const options = { serverBin: '/bridge-server',
    spawnProcess: (() => {
      const child = fakeChild(JSON.stringify({ storeScope: 'home', configRevision: 1, accounts: [], health: [] })) as any;
      child.stdin = new PassThrough(); child.stdin.on('data', bytes => { envelope = JSON.parse(bytes.toString()); }); return child;
    }) as any };
  Object.defineProperty(options, 'providerAccountConfiguration', { get: () => { throw new Error('untrusted session must not be read'); } });
  const catalog = new ProjectSessionCatalog(options);
  await catalog.providerAccountCommand({ action: 'list', expectedRevision: 1 });
  assert.equal(Object.hasOwn(envelope, 'profiles_json'), false);
  assert.equal(Object.hasOwn(envelope, 'provider_region'), false);
  assert.equal(Object.hasOwn(envelope, 'app_version'), false);
});

test('credential version publication uses a private metadata-only helper envelope without profiles or secrets', async () => {
  let envelope: any;
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server', providerAccountChannel: 'production', appVersion: '1.2.3',
    spawnProcess: ((_bin: string, args: readonly string[]) => {
      assert.deepEqual(args, ['--provider-account-command']);
      const child = fakeChild(JSON.stringify({ published: true })) as any;
      child.stdin = new PassThrough(); child.stdin.on('data', bytes => { envelope = JSON.parse(bytes.toString()); }); return child;
    }) as any });
  const version = { accountId: '11111111-1111-4111-8111-111111111111', identityGeneration: 2, credentialRevision: 3 };
  await catalog.publishProviderAccountCredentialVersion({ ...version, accessToken: 'must-not-cross-private-version-channel' } as any);
  assert.deepEqual(envelope, { metadata_only: true, backend: 'desktop-broker', credential_channel: 'production', request_json: '', publish_credential_version: version });
  await assert.rejects(catalog.publishProviderAccountCredentialVersion({ ...version, credentialRevision: 0 }), /Invalid credential version/);
});

test('global provider settings use a headless Host command with expected snapshot on stdin', async () => {
  let flags: readonly string[] = [], envelope: any;
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server', spawnProcess: ((_bin: string, args: readonly string[]) => {
    flags = args;
    const child = fakeChild(JSON.stringify({ raw: {}, effective: { providers: { custom: { type: 'openai' } }, providerRegion: 'international' }, locked: [] })) as any;
    child.stdin = new PassThrough(); child.stdin.on('data', bytes => { envelope = JSON.parse(bytes.toString()); });
    return child;
  }) as any });
  const expected = { providers: {} };
  const patch = { providerRegion: 'international' };
  const result = await catalog.providerSettings(patch, expected);
  assert.deepEqual(flags, ['--provider-settings-command']);
  assert.deepEqual(envelope, { patch, expected });
  assert.equal(result.effective.providerRegion, 'international');
});

test('native brand paths share discovery across concurrent Host services and survive caller mutation', async () => {
  let calls = 0;
  const brandHome = join(tmpdir(), '.configured-brand');
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server', spawnProcess: (() => {
    calls++;
    return fakeChild(JSON.stringify({ version: 2, brand_home: brandHome, workspace_root: brandHome + '-workspaces', app_projects_root: join(brandHome + '-workspaces', 'apps'), global_config_path: null }));
  }) as any });
  const rows = await Promise.all(Array.from({ length: 12 }, () => catalog.productPaths()));
  assert.equal(calls, 1, 'concurrent consumers must not launch competing native discovery processes');
  rows[0].brandHome = '/caller-mutated';
  assert.equal((await catalog.productPaths()).brandHome, brandHome);
  assert.equal(calls, 1);
});

test('a failed native brand discovery is not retained by later Host operations', async () => {
  let calls = 0;
  const brandHome = join(tmpdir(), '.configured-brand');
  const catalog = new ProjectSessionCatalog({ serverBin: '/bridge-server', spawnProcess: (() => {
    calls++;
    return fakeChild(calls === 1 ? '{' : JSON.stringify({ version: 2, brand_home: brandHome, workspace_root: brandHome + '-workspaces', app_projects_root: join(brandHome + '-workspaces', 'apps'), global_config_path: null }));
  }) as any });
  await assert.rejects(catalog.productPaths(), /invalid product paths response/);
  assert.equal((await catalog.productPaths()).brandHome, brandHome);
  assert.equal(calls, 2);
});
