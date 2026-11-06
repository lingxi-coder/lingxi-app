import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  mkdirSync,
  writeFileSync,
  symlinkSync,
  linkSync,
  readFileSync,
  readdirSync,
} from 'node:fs';
import { join } from 'node:path';
import { botFixture } from './bot-helpers.js';
import {
  prepareBotPackage,
  restoreBotPackage,
  validateBotManifest,
  validatePublicBotText,
} from '../src/main/bots/package.js';
import { BotService } from '../src/main/bots/service.js';
import {
  BotMarketInstaller,
  type BotInstallOptions,
} from '../src/main/bots/marketInstaller.js';
import {
  MarketApiClient,
  publishMarketPackage,
} from '../src/main/local-apps/marketClient.js';
import { sha256, unpackSource } from '../src/main/local-apps/sourcePackage.js';
import { parseDeepLink } from '../../../packages/deep-links/index.js';

async function fixture(t: Parameters<typeof botFixture>[0]) {
  const f = botFixture(t, { ids: true });
  const bot = f.store.create({
    name: 'Travel helper',
    persona: 'Use only published travel notes.',
    model: 'owner-private-model',
  });
  mkdirSync(join(f.store.homeDir(bot.bot_id), 'skills', 'packing'), {
    recursive: true,
  });
  writeFileSync(
    join(f.store.homeDir(bot.bot_id), 'skills', 'packing', 'SKILL.md'),
    'Prepare a packing list.',
  );
  mkdirSync(join(f.store.homeDir(bot.bot_id), 'memory'));
  writeFileSync(
    join(f.store.homeDir(bot.bot_id), 'memory', 'private.md'),
    'owner private memory',
  );
  writeFileSync(
    join(f.store.homeDir(bot.bot_id), 'settings.json'),
    '{"secret":"private-settings"}',
  );
  const selection = {
    botId: bot.bot_id,
    expectedRevision: bot.revision,
    version: '1.0.0',
    license: 'LicenseRef-Author-Defined',
    licenseText: 'The author grants use of these texts.',
    skills: ['packing'],
    resources: [{ id: 'travel-notes', required: true }],
    examples: ['Make a packing list'],
  };
  const prepared = await prepareBotPackage(
    f.store,
    selection,
    join(f.root, 'exports'),
  );
  const service = new BotService({
    store: f.store,
    platform: 'darwin',
    runtimes: {
      newSession: async () => {
        throw new Error('unused');
      },
      openSession: async () => {},
      closeProject: async () => {},
    },
    catalogFor: () => ({
      list: async () => ({ sessions: [] }),
      find: async () => undefined,
      workspaceSettings: async () => ({
        environment: {},
        configHome: '/unused',
      }),
      invalidate: () => {},
    }),
  });
  return { ...f, bot, selection, prepared, service };
}
test('export contains exactly reviewed public text; private Bot state and credentials are never read', async (t) => {
  const f = await fixture(t);
  const p = f.prepared;
  assert.deepEqual(
    p.preview.files.map((f) => f.path),
    ['persona.md', 'LICENSE', 'skills/packing/SKILL.md', 'lingxi-bot.json'],
  );
  const serialized = JSON.stringify(p.preview);
  for (const secret of [
    'owner-private-model',
    'owner private memory',
    'private-settings',
    f.bot.bot_id,
    f.store.homeDir(f.bot.bot_id),
  ])
    assert.ok(!serialized.includes(secret));
  const target = join(f.root, 'verified');
  await unpackSource(p.archive, p.inventory, target, 'lingxi-bot.json');
  assert.equal(
    readFileSync(join(target, 'persona.md'), 'utf8'),
    'Use only published travel notes.',
  );
  assert.throws(
    () => validateBotManifest({ ...p.preview.manifest, model: 'author-model' }),
    /unsupported/,
  );
  assert.throws(
    () =>
      validateBotManifest({
        ...p.preview.manifest,
        files: [
          ...p.preview.manifest.files,
          { ...p.preview.manifest.files[0], path: 'PERSONA.md' },
        ],
      }),
    /allowlist/,
  );
  assert.throws(
    () => validatePublicBotText('memory/private.md', 'text'),
    /forbidden/,
  );
  assert.throws(
    () => validatePublicBotText('skills/run.sh', 'echo hi'),
    /forbidden/,
  );
  assert.throws(
    () => validatePublicBotText('persona.md', 'Read /Users/owner/private.txt'),
    /private bindings or local paths/,
  );
});
test('export rejects linked files/directories, private fields, changed revisions and unselected Skills', async (t) => {
  assert.throws(
    () =>
      validatePublicBotText(
        'persona.md',
        '{"credentialSlot":"private-owner-binding"}',
      ),
    /credentials/,
  );
  const f = await fixture(t);
  const file = join(
    f.store.homeDir(f.bot.bot_id),
    'skills',
    'packing',
    'SKILL.md',
  );
  assert.throws(
    () => validatePublicBotText('persona.md', 'sk-' + 'a'.repeat(30)),
    /credentials/,
  );
  await assert.rejects(
    prepareBotPackage(
      f.store,
      { ...f.selection, model: 'secret' } as any,
      join(f.root, 'exports'),
    ),
    /unsupported/,
  );
  await assert.rejects(
    prepareBotPackage(
      f.store,
      { ...f.selection, expectedRevision: 99 },
      join(f.root, 'exports'),
    ),
    /changed/,
  );
  await assert.rejects(
    prepareBotPackage(
      f.store,
      { ...f.selection, skills: ['../memory'] },
      join(f.root, 'exports'),
    ),
    /skill name/,
  );
  linkSync(file, join(f.root, 'hardlink'));
  await assert.rejects(
    prepareBotPackage(f.store, f.selection, join(f.root, 'exports')),
    /without links/,
  );
  mkdirSync(join(f.root, 'linked-skill'));
  writeFileSync(join(f.root, 'linked-skill', 'SKILL.md'), 'public');
  symlinkSync(
    join(f.root, 'linked-skill'),
    join(f.store.homeDir(f.bot.bot_id), 'skills', 'linked'),
  );
  await assert.rejects(
    prepareBotPackage(
      f.store,
      { ...f.selection, skills: ['linked'] },
      join(f.root, 'exports'),
    ),
    /directory links/,
  );
});
function installer(
  f: Awaited<ReturnType<typeof fixture>>,
  state: {
    downloads: number;
    confirmations: number;
    changed?: () => void;
    account?: string;
    corrupt?: boolean;
  },
) {
  const p = f.prepared;
  const release = {
    id: 'release1',
    appId: 'listing1',
    resourceKind: 'bot',
    schema: 3,
    manifest: p.preview.manifest,
    files: p.inventory,
    dependencies: [],
    releaseDigest: p.preview.digest,
    sha256: sha256(p.archive),
    sizeBytes: p.archive.length,
    downloadPath: '/api/v3/bots/listing1/versions/release1/download',
  };
  const fetcher = (async (input: URL | RequestInfo, init?: RequestInit) => {
    const path = String(input);
    assert.equal(
      (init?.headers as Record<string, string>)?.Authorization,
      'Bearer own-token',
    );
    if (path.endsWith('/download')) {
      state.downloads++;
      return new Response(state.corrupt ? Buffer.from('corrupt') : p.archive);
    }
    if (path.includes('/versions?'))
      return Response.json({ data: [{ id: 'release1' }] });
    return Response.json({ data: release });
  }) as typeof fetch;
  const api = new MarketApiClient(
    async () => 'own-token',
    'https://market.test/api/v3/',
    fetcher,
  );
  return new BotMarketInstaller({
    store: f.store,
    createBot: (input, origin, prepare) =>
      f.service.createFromMarket(input, origin, prepare),
    api,
    accountId: () => state.account ?? 'own-account',
    assertAvailable: () => {},
    confirm: async () => {
      state.confirmations++;
      return true;
    },
    changed: () => state.changed?.(),
  });
}
test('Bot installer coalesces clicks, persists instance origin, and replays after a lost completion ACK without duplication', async (t) => {
  const f = await fixture(t),
    state = {
      downloads: 0,
      confirmations: 0,
      changed: () => {
        throw new Error('lost ACK');
      },
    };
  const first = installer(f, state);
  const promise = first.install('listing1');
  assert.equal(first.install('listing1'), promise);
  await assert.rejects(promise, /lost ACK/);
  assert.equal(f.store.list().length, 2);
  const installed = f.store.list().find((b) => b.marketOrigin)!;
  assert.notEqual(installed.bot_id, f.bot.bot_id);
  assert.equal(installed.model, undefined);
  assert.deepEqual(installed.workspaces, []);
  assert.equal(installed.remote, undefined);
  assert.equal(installed.accept_from, undefined);
  const retried = await installer(f, {
    downloads: 0,
    confirmations: 0,
  }).install('listing1', 'release1');
  assert.equal(retried?.bot_id, installed.bot_id);
  assert.equal(f.store.list().length, 2);
  const copied = await installer(f, { downloads: 0, confirmations: 0 }).install(
    'listing1',
    'release1',
    'explicit-copy',
  );
  assert.notEqual(copied?.bot_id, installed.bot_id);
  assert.equal(f.store.list().length, 3);
  assert.equal(
    readFileSync(
      join(f.store.homeDir(installed.bot_id), 'skills', 'packing', 'SKILL.md'),
      'utf8',
    ),
    'Prepare a packing list.',
  );
});
test('corrupt or interrupted downloads never create a partial Bot and retain retry identity', async (t) => {
  const f = await fixture(t);
  await assert.rejects(
    installer(f, { downloads: 0, confirmations: 0, corrupt: true }).install(
      'listing1',
      'release1',
    ),
    /checksum/,
  );
  assert.equal(f.store.list().length, 1);
  const directories = readdirSync(join(f.userData, 'bot-market-installs'));
  assert.equal(directories.length, 1);
  const before = JSON.parse(
    readFileSync(
      join(f.userData, 'bot-market-installs', directories[0]!, 'intent.json'),
      'utf8',
    ),
  );
  const installed = await installer(f, {
    downloads: 0,
    confirmations: 0,
  }).install('listing1', 'release1');
  assert.equal(installed?.marketOrigin?.installIntent, before.requestId);
  assert.equal(f.store.list().length, 2);
});
test('shared package publication journal reuses fixed mutation keys and bytes after ACK loss', async (t) => {
  const f = await fixture(t),
    calls: { path: string; key: string | undefined }[] = [];
  let fail = true;
  const api = new MarketApiClient(
    async () => 'token',
    'https://market.test/api/v3/',
    (async (input, init) => {
      const path = new URL(String(input)).pathname;
      calls.push({
        path,
        key: (init?.headers as Record<string, string>)?.['Idempotency-Key'],
      });
      if (path === '/api/v3/bots') {
        if (fail) {
          fail = false;
          throw new Error('create ACK lost');
        }
        return Response.json({ data: { id: 'listing1' } });
      }
      if (path.endsWith('/versions'))
        return Response.json({
          data: {
            id: 'release1',
            upload: { path: '/api/v3/versions/release1/artifact' },
          },
        });
      if (path.endsWith('/publish'))
        return Response.json({ data: { status: 'validating' } });
      return Response.json({ data: {} });
    }) as typeof fetch,
  );
  const options = {
    writer: api,
    directory: join(f.root, 'receipts'),
    digest: f.prepared.preview.digest,
    metadata: {
      name: 'Travel helper',
      summary: 'Helpful',
      description: 'Role',
      category: 'tools',
      notes: 'Initial',
    },
    collection: 'bots' as const,
    archive: async () => f.prepared.archive,
  };
  await assert.rejects(publishMarketPackage(options), /ACK lost/);
  const result = await publishMarketPackage(options);
  assert.equal(result.appId, 'listing1');
  assert.equal(calls[0]?.key, calls[1]?.key);
  assert.ok(calls[0]?.key);
});
test('Bot and application deep links remain separate and reject credentials/foreign package URLs', () => {
  assert.equal(
    parseDeepLink('lingxi://market/bot/listing1?versionId=release1').route,
    'market/bot/listing1',
  );
  assert.equal(parseDeepLink('lingxi://market/app1').route, 'market/app1');
  for (const value of [
    'lingxi://market/bot/listing1?token=secret',
    'lingxi://market/bot/listing1?workspace=private',
    'lingxi://market/bot/listing1?versionId=../bad',
    'lingxi://market/app1?versionId=release1',
  ])
    assert.throws(() => parseDeepLink(value));
});

test('Desktop ACK follows atomic Bot commit and exact replay recovers a lost cross-device response', async (t) => {
  const f = await fixture(t),
    p = f.prepared;
  const release = {
    id: 'release1',
    appId: 'listing1',
    resourceKind: 'bot',
    schema: 3,
    manifest: p.preview.manifest,
    files: p.inventory,
    dependencies: [],
    releaseDigest: p.preview.digest,
    sha256: sha256(p.archive),
    sizeBytes: p.archive.length,
    downloadPath: '/api/v3/bots/listing1/versions/release1/download',
  };
  let completed: { instanceId: string; releaseDigest: string } | undefined,
    loseAck = true;
  const keys: string[] = [];
  const api = new MarketApiClient(
    async () => 'own-token',
    'https://market.test/api/v3/',
    (async (input, init) => {
      const path = new URL(String(input)).pathname;
      if (path.endsWith('/complete')) {
        const body = JSON.parse(init!.body as string);
        assert.equal(
          f.store.list().length,
          2,
          'ACK must follow one committed installation',
        );
        assert.equal(
          f.store.get(body.instanceId).marketOrigin?.digest,
          body.releaseDigest,
        );
        if (completed) assert.deepEqual(body, completed);
        completed = body;
        keys.push(
          (init!.headers as Record<string, string>)['Idempotency-Key']!,
        );
        if (loseAck) {
          loseAck = false;
          throw new Error('receipt ACK lost');
        }
        return Response.json({ data: { status: 'installed', ...completed } });
      }
      if (path.endsWith('/bot-install-intents/handoff1'))
        return Response.json({
          data: {
            id: 'handoff1',
            listingId: 'listing1',
            versionId: 'release1',
            releaseDigest: p.preview.digest,
            status: completed ? 'installed' : 'awaiting_desktop',
            instanceId: completed?.instanceId ?? null,
          },
        });
      if (path.endsWith('/download')) return new Response(p.archive);
      return Response.json({ data: release });
    }) as typeof fetch,
  );
  const options = {
    store: f.store,
    createBot: (input, origin, prepare) =>
      f.service.createFromMarket(input, origin, prepare),
    api,
    accountId: () => 'own-account',
    assertAvailable: () => {},
    confirm: async () => true,
    changed: () => {},
  };
  await assert.rejects(
    new BotMarketInstaller(options).install(
      'listing1',
      undefined,
      undefined,
      'handoff1',
    ),
    /ACK lost/,
  );
  const retried = await new BotMarketInstaller(options).install(
    'listing1',
    'release1',
    undefined,
    'handoff1',
  );
  assert.equal(retried?.bot_id, completed?.instanceId);
  assert.equal(f.store.list().length, 2);
  assert.equal(keys[0], keys[1]);
});

test('publication recovery restores reviewed immutable bytes after restart and rejects tampered text', async (t) => {
  const f = await fixture(t);
  const directory = join(f.root, 'saved-publication');
  mkdirSync(directory);
  writeFileSync(join(directory, 'source.zip'), f.prepared.archive);
  writeFileSync(
    join(directory, 'review.json'),
    JSON.stringify(f.prepared.preview),
  );
  const restored = await restoreBotPackage(
    f.store,
    f.bot.bot_id,
    directory,
    join(f.root, 'restored-preview'),
  );
  assert.equal(restored.preview.digest, f.prepared.preview.digest);
  assert.deepEqual(restored.archive, f.prepared.archive);
  assert.notEqual(restored.preview.id, f.prepared.preview.id);
  const tampered = structuredClone(f.prepared.preview);
  tampered.files[0]!.text = 'changed contents';
  writeFileSync(join(directory, 'review.json'), JSON.stringify(tampered));
  await assert.rejects(
    restoreBotPackage(
      f.store,
      f.bot.bot_id,
      directory,
      join(f.root, 'tampered-preview'),
    ),
    /Reviewed file changed/,
  );
});

test(
  'concurrent direct and distinct handoff links create one Bot while ACKs stay independent after loss',
  { timeout: 5000 },
  async (t) => {
    const f = await fixture(t),
      p = f.prepared;
    const release = {
      id: 'release1',
      appId: 'listing1',
      resourceKind: 'bot',
      schema: 3,
      manifest: p.preview.manifest,
      files: p.inventory,
      dependencies: [],
      releaseDigest: p.preview.digest,
      sha256: sha256(p.archive),
      sizeBytes: p.archive.length,
      downloadPath: '/api/v3/bots/listing1/versions/release1/download',
    };
    const receipts = new Map<
      string,
      { instanceId: string; releaseDigest: string }
    >();
    const acknowledgments = new Map<string, string[]>();
    let downloads = 0,
      creates = 0,
      changes = 0,
      lost = false;
    let releaseDownloads!: () => void;
    const ready = new Promise<void>((resolve) => {
      releaseDownloads = resolve;
    });
    const api = new MarketApiClient(
      async () => 'own-token',
      'https://market.test/api/v3/',
      (async (input, init) => {
        const path = new URL(String(input)).pathname;
        const receipt =
          /\/bot-install-intents\/(handoff[12])(?:\/(complete))?$/.exec(path);
        if (receipt) {
          const id = receipt[1]!;
          if (receipt[2]) {
            const body = JSON.parse(init!.body as string);
            assert.equal(
              f.store.list().length,
              2,
              'receipt ACK follows a single committed Bot',
            );
            assert.equal(
              f.store.get(body.instanceId).marketOrigin?.digest,
              body.releaseDigest,
            );
            const previous = receipts.get(id);
            if (previous) assert.deepEqual(body, previous);
            receipts.set(id, body);
            const key = (init!.headers as Record<string, string>)[
              'Idempotency-Key'
            ]!;
            acknowledgments.set(id, [...(acknowledgments.get(id) ?? []), key]);
            if (id === 'handoff1' && !lost) {
              lost = true;
              throw new Error('concurrent receipt ACK lost');
            }
            return Response.json({ data: { status: 'installed', ...body } });
          }
          const completed = receipts.get(id);
          return Response.json({
            data: {
              id,
              listingId: 'listing1',
              versionId: 'release1',
              releaseDigest: p.preview.digest,
              status: completed ? 'installed' : 'awaiting_desktop',
              instanceId: completed?.instanceId ?? null,
            },
          });
        }
        if (path.endsWith('/download')) {
          downloads++;
          if (downloads === 3) releaseDownloads();
          await ready;
          return new Response(p.archive);
        }
        return Response.json({ data: release });
      }) as typeof fetch,
    );
    const options: BotInstallOptions = {
      store: f.store,
      api,
      accountId: () => 'own-account',
      assertAvailable: () => {},
      confirm: async () => true,
      changed: () => {
        changes++;
      },
      createBot: (input, origin, prepare) => {
        creates++;
        return f.service.createFromMarket(input, origin, prepare);
      },
    };
    const installer = new BotMarketInstaller(options);
    const results = await Promise.allSettled([
      installer.install('listing1', 'release1'),
      installer.install('listing1', 'release1', undefined, 'handoff1'),
      installer.install('listing1', 'release1', undefined, 'handoff2'),
    ]);
    assert.equal(results[0]!.status, 'fulfilled');
    assert.equal(results[1]!.status, 'rejected');
    assert.match(
      (results[1] as PromiseRejectedResult).reason.message,
      /concurrent receipt ACK lost/,
    );
    assert.equal(results[2]!.status, 'fulfilled');
    const direct = (
      results[0] as PromiseFulfilledResult<
        Awaited<ReturnType<BotMarketInstaller['install']>>
      >
    ).value!;
    const other = (results[2] as PromiseFulfilledResult<typeof direct>).value;
    assert.equal(other.bot_id, direct.bot_id);
    assert.equal(creates, 1);
    assert.equal(changes, 1);
    assert.equal(f.store.list().length, 2);
    assert.equal(receipts.get('handoff1')?.instanceId, direct.bot_id);
    assert.equal(receipts.get('handoff2')?.instanceId, direct.bot_id);
    const retried = await new BotMarketInstaller(options).install(
      'listing1',
      'release1',
      undefined,
      'handoff1',
    );
    assert.equal(retried?.bot_id, direct.bot_id);
    assert.equal(creates, 1);
    assert.equal(downloads, 3);
    assert.equal(acknowledgments.get('handoff1')?.length, 2);
    assert.equal(acknowledgments.get('handoff2')?.length, 1);
    assert.equal(
      acknowledgments.get('handoff1')?.[0],
      acknowledgments.get('handoff1')?.[1],
    );
  },
);
