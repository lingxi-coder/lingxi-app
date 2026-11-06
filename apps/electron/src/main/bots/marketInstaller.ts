import { promises as fs, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type {
  BotMarketOrigin,
  BotPackageManifest,
} from '../../shared/botMarket.js';
import { MarketApiClient, durableWrite } from '../local-apps/marketClient.js';
import {
  MarketInstallGuard,
  marketApplicationId,
} from '../local-apps/marketLinkInstaller.js';
import {
  canonical,
  sha256,
  unpackSource,
  type SourceFile,
} from '../local-apps/sourcePackage.js';
import { validateBotManifest, validatePublicBotText } from './package.js';
import type { BotCreateInput, BotRecord, BotStore } from './store.js';
import { skillName } from './content.js';

interface BotRelease {
  id: string;
  appId: string;
  resourceKind: 'bot';
  schema: 3;
  manifest: BotPackageManifest;
  files: SourceFile[];
  dependencies: unknown[];
  releaseDigest: string;
  sha256: string;
  sizeBytes: number;
  downloadPath: string;
}
export interface BotInstallOptions {
  createBot(
    input: BotCreateInput,
    origin: BotMarketOrigin,
    prepare: (record: BotRecord) => void,
  ): BotRecord;
  store: BotStore;
  api: MarketApiClient;
  accountId(): string | undefined;
  confirm(manifest: BotPackageManifest): Promise<boolean>;
  changed(): void;
  assertAvailable(): void;
}
/** Downloaded Bot bytes only enter this installer. Local App backend import is never invoked. */
export class BotMarketInstaller {
  private readonly guard = new MarketInstallGuard<BotRecord | null>();
  constructor(private readonly options: BotInstallOptions) {}
  install(
    listingValue: unknown,
    versionValue?: unknown,
    copyValue?: unknown,
    intentValue?: unknown,
  ): Promise<BotRecord | null> {
    const listingId = marketApplicationId(listingValue),
      versionId =
        versionValue === undefined
          ? undefined
          : marketApplicationId(versionValue);
    const copyIntent =
      copyValue === undefined ? '' : marketApplicationId(copyValue);
    const receiptId =
      intentValue === undefined ? undefined : marketApplicationId(intentValue);
    const account = this.options.accountId();
    if (!account) throw new Error('Sign in to download a Bot');
    this.options.assertAvailable();
    return this.guard.run(
      canonical([account, listingId, versionId, copyIntent, receiptId]),
      () => this.perform(account, listingId, versionId, copyIntent, receiptId),
    );
  }
  private async perform(
    accountId: string,
    listingId: string,
    versionId: string | undefined,
    copyIntent: string,
    receiptId?: string,
  ): Promise<BotRecord | null> {
    const { store, api } = this.options;
    const checkAccount = () => {
      if (this.options.accountId() !== accountId)
        throw new Error('Account changed during Bot installation');
    };
    const request = async <T>(path: string) => {
      checkAccount();
      const result = await api.request<T>(path, undefined, true);
      checkAccount();
      return result;
    };
    const handoff = receiptId
      ? await request<{
          listingId: string;
          versionId: string;
          releaseDigest: string;
          status: string;
          instanceId: string | null;
        }>(`bot-install-intents/${receiptId}`)
      : undefined;
    if (handoff) {
      if (
        handoff.listingId !== listingId ||
        (versionId !== undefined && handoff.versionId !== versionId)
      )
        throw new Error('Bot handoff does not match this release');
      versionId = marketApplicationId(handoff.versionId);
    }
    const acknowledge = async (record: BotRecord) => {
      if (!receiptId) return record;
      const origin = record.marketOrigin;
      if (!origin || handoff?.releaseDigest !== origin.digest)
        throw new Error('Bot handoff release digest changed');
      checkAccount();
      await api.request(
        `bot-install-intents/${receiptId}/complete`,
        {
          instanceId: record.bot_id,
          releaseDigest: origin.digest,
        },
        true,
        'POST',
        {
          'Idempotency-Key': sha256(
            Buffer.from(
              `bot-install:${receiptId}:${record.bot_id}:${origin.digest}`,
            ),
          ),
        },
      );
      checkAccount();
      return record;
    };
    if (!versionId) {
      const versions = await request<{ id: string }[]>(
        `bots/${listingId}/versions?limit=1`,
      );
      versionId = versions[0]?.id;
    }
    if (!versionId) throw new Error('This Bot has no downloadable release');
    marketApplicationId(versionId);
    const intent = sha256(
      Buffer.from(
        canonical([api.base.href, accountId, listingId, versionId, copyIntent]),
      ),
    );
    const find = () =>
      store
        .list()
        .find(
          (b) =>
            b.marketOrigin?.installIntent === intent &&
            b.marketOrigin.accountId === accountId &&
            b.marketOrigin.market === api.base.href,
        );
    const prior = find();
    if (prior) return acknowledge(prior);
    if (handoff?.status === 'installed')
      throw new Error('This Bot handoff was completed on another Desktop');
    const release = await request<BotRelease>(
      `bots/${listingId}/versions/${versionId}`,
    );
    if (
      release.resourceKind !== 'bot' ||
      release.schema !== 3 ||
      release.id !== versionId ||
      release.appId !== listingId
    )
      throw new Error('Invalid Bot market release');
    const manifest = validateBotManifest(release.manifest);
    const platform =
      process.platform === 'darwin'
        ? 'macos'
        : process.platform === 'win32'
          ? 'windows'
          : 'linux';
    if (!manifest.runtime.hostingTargets.includes(platform))
      throw new Error('This Bot requires another Desktop host');
    if (!Array.isArray(release.dependencies) || release.dependencies.length)
      throw new Error('This Bot has unsupported dependencies');
    if (
      !Array.isArray(release.files) ||
      release.files.length !== manifest.files.length + 1 ||
      release.files.some((f) => f.executable !== false)
    )
      throw new Error('Invalid Bot package inventory');
    const declared = new Map(manifest.files.map((f) => [f.path, f]));
    for (const f of release.files) {
      const expected = declared.get(f.path);
      if (
        f.path !== 'lingxi-bot.json' &&
        (!expected ||
          canonical({ ...expected, executable: false }) !== canonical(f))
      )
        throw new Error('Bot file inventory mismatch');
    }
    const size = release.sizeBytes;
    if (
      !Number.isSafeInteger(size) ||
      size <= 0 ||
      size > 25 * 1024 * 1024 ||
      !/^[a-f0-9]{64}$/.test(release.sha256)
    )
      throw new Error('Bot package exceeds download limits');
    const digest = sha256(
      Buffer.from(
        canonical({
          schema: 1,
          artifactSha256: release.sha256,
          manifest,
          files: release.files,
          dependencies: [],
        }),
      ),
    );
    if (digest !== release.releaseDigest)
      throw new Error('Bot release checksum mismatch');
    if (handoff && handoff.releaseDigest !== digest)
      throw new Error('Bot handoff release digest changed');
    if (!(await this.options.confirm(manifest))) return null;
    checkAccount();
    const directory = join(
      store.locations.userData,
      'bot-market-installs',
      intent,
    );
    await durableWrite(
      join(directory, 'intent.json'),
      JSON.stringify({
        schema: 1,
        requestId: intent,
        accountId,
        listingId,
        releaseId: versionId,
        digest,
        stage: 'downloading',
      }),
    );
    // Authentication is restricted to the canonical same-origin v3 content proxy.
    const expected = new URL(
        `bots/${listingId}/versions/${versionId}/download`,
        api.base,
      ),
      download = new URL(release.downloadPath, api.base);
    if (download.href !== expected.href)
      throw new Error('Invalid authenticated Bot download route');
    const response = await api.fetcher(download, {
      headers: { Authorization: `Bearer ${await api.token()}` },
      redirect: 'error',
      signal: AbortSignal.timeout(60_000),
    });
    if (!response.ok || !response.body) throw new Error('Bot download failed');
    const chunks: Buffer[] = [];
    let received = 0;
    for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) {
      received += chunk.byteLength;
      if (received > size) throw new Error('Bot exceeds declared package size');
      chunks.push(Buffer.from(chunk));
    }
    const bytes = Buffer.concat(chunks);
    if (bytes.length !== size || sha256(bytes) !== release.sha256)
      throw new Error('Bot package checksum mismatch');
    const stage = join(directory, `stage-${randomUUID()}`);
    try {
      await unpackSource(bytes, release.files, stage, 'lingxi-bot.json');
      if (
        canonical(
          JSON.parse(await fs.readFile(join(stage, 'lingxi-bot.json'), 'utf8')),
        ) !== canonical(manifest)
      )
        throw new Error('Bot manifest differs from approved release');
      const content = new Map<string, string>();
      for (const file of manifest.files) {
        const value = new TextDecoder('utf-8', { fatal: true }).decode(
          await fs.readFile(join(stage, file.path)),
        );
        validatePublicBotText(file.path, value);
        content.set(file.path, value);
      }
      const skills = manifest.skills.map((s) => {
        const match = /^skills\/([^/]+)\/SKILL\.md$/.exec(s.path);
        if (!match) throw new Error('Bot only supports named text Skills');
        return { name: skillName(match[1]), text: content.get(s.path)! };
      });
      if (
        new Set(skills.map((s) => s.name.toLowerCase())).size !== skills.length
      )
        throw new Error('Duplicate Bot Skills');
      checkAccount();
      await durableWrite(
        join(directory, 'intent.json'),
        JSON.stringify({
          schema: 1,
          requestId: intent,
          accountId,
          listingId,
          releaseId: versionId,
          digest,
          stage: 'verified',
        }),
      );
      checkAccount();
      const origin: BotMarketOrigin = {
        market: api.base.href,
        accountId,
        listingId,
        releaseId: versionId,
        digest,
        installIntent: intent,
      };
      // Different handoff receipts can verify the same persisted install identity in parallel.
      // Recheck after the last await, then keep lookup + creation synchronous. Each caller
      // still completes its own receipt below, including when another caller created the Bot.
      const committed = find();
      const record =
        committed ??
        this.options.createBot(
          {
            name: manifest.name,
            description: `Installed Bot ${manifest.version}. Configure your model and resource authorizations before sharing.`,
            persona: content.get(manifest.entry)!,
          },
          origin,
          (draft) => {
            for (const skill of skills) {
              const dir = join(
                store.homeDir(draft.bot_id),
                'skills',
                skill.name,
              );
              mkdirSync(dir, { recursive: true, mode: 0o700 });
              writeFileSync(join(dir, 'SKILL.md'), skill.text, {
                flag: 'wx',
                mode: 0o600,
              });
            }
            writeFileSync(
              join(store.botDir(draft.bot_id), 'market-package.json'),
              canonical(manifest),
              { flag: 'wx', mode: 0o600 },
            );
          },
        );
      if (!committed) this.options.changed();
      await durableWrite(
        join(directory, 'intent.json'),
        JSON.stringify({
          schema: 1,
          requestId: intent,
          accountId,
          listingId,
          releaseId: versionId,
          digest,
          stage: 'awaiting_bindings',
          instanceId: record.bot_id,
        }),
      );
      return await acknowledge(record);
    } finally {
      await fs.rm(stage, { recursive: true, force: true });
    }
  }
}
