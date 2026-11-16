import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type {
  BotMarketOrigin,
  BotPackageManifest,
} from '../../shared/botMarket.js';
import { MarketApiClient, durableWrite } from '../local-apps/marketClient.js';
import {
  MarketInstallGuard,
  marketApplicationId,
} from '../local-apps/marketLinkInstaller.js';
import { canonical, sha256 } from '../local-apps/sourcePackage.js';
import {
  checkedBotRelease,
  downloadBotRelease,
  installedDescription,
} from './marketRelease.js';
import type { BotCreateInput, BotRecord, BotStore } from './store.js';

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
    const checked = await checkedBotRelease(request, listingId, versionId);
    const { manifest, digest } = checked;
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
    const { content, skills } = await downloadBotRelease(api, checked, directory);
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
          description: installedDescription(manifest.version),
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
  }
}
