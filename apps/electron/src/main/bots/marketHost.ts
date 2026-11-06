import { dialog, shell } from 'electron';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type {
  BotMarketRequest,
  BotMarketResponse,
} from '../../shared/botMarket.js';
import {
  localAppMarketApiRoot,
  localAppMarketUrl,
} from '../../shared/productConfig.js';
import {
  durableWrite,
  MarketApiClient,
  publishMarketPackage,
} from '../local-apps/marketClient.js';
import { MarketInstallGuard } from '../local-apps/marketLinkInstaller.js';
import { sha256 } from '../local-apps/sourcePackage.js';
import {
  prepareBotPackage,
  restoreBotPackage,
  type PreparedBotPackage,
} from './package.js';
import { BotMarketInstaller } from './marketInstaller.js';
import type { BotStore } from './store.js';
import type { BotService } from './service.js';
import { botUi } from './rendererIpc.js';

export class BotMarketHost {
  private readonly api: MarketApiClient;
  private readonly installer: BotMarketInstaller;
  private readonly prepared = new Map<string, PreparedBotPackage>();
  private readonly publications = new MarketInstallGuard<BotMarketResponse>();
  constructor(
    private readonly options: {
      store: BotStore;
      service: BotService;
      accountToken(): Promise<string>;
      accountId(): string | undefined;
      changed?(): void;
    },
  ) {
    this.api = new MarketApiClient(
      options.accountToken,
      new URL('/api/v3/', localAppMarketApiRoot).href,
    );
    this.installer = new BotMarketInstaller({
      store: options.store,
      createBot: (input, origin, prepare) =>
        options.service.createFromMarket(input, origin, prepare),
      api: this.api,
      accountId: options.accountId,
      assertAvailable: () => options.service.assertAvailable(),
      changed: () => options.changed?.(),
      confirm: async (manifest) => {
        const result = await dialog.showMessageBox({
          type: 'question',
          title: 'Bot Market',
          message: `Download ${manifest.name}?`,
          detail: `Version ${manifest.version} · ${manifest.license}\n\n${manifest.skills.length} text Skills; ${manifest.resources.length} resource requirements.\n\nCreates a personal Bot. Choose your own model and authorize your own resources before sharing with a Team.`,
          buttons: ['Cancel', 'Download Bot'],
          defaultId: 1,
          cancelId: 0,
        });
        return result.response === 1;
      },
    });
  }
  private async remember(prepared: PreparedBotPackage): Promise<void> {
    while (this.prepared.size >= 8) {
      const oldest = this.prepared.keys().next().value!;
      const previous = this.prepared.get(oldest)!;
      this.prepared.delete(oldest);
      await fs.rm(previous.directory, { recursive: true, force: true });
    }
    this.prepared.set(prepared.preview.id, prepared);
  }
  async handle(value: unknown): Promise<BotMarketResponse> {
    if (!value || typeof value !== 'object' || Array.isArray(value))
      throw new Error('Invalid Bot market request');
    const request = value as BotMarketRequest;
    if (request.action === 'browse') {
      const url = new URL(localAppMarketUrl);
      url.searchParams.set('resourceKind', 'bot');
      url.searchParams.set('platform', 'macos');
      await shell.openExternal(url.href);
      return;
    }
    if (request.action === 'manage') {
      await shell.openExternal(new URL('/mine', localAppMarketUrl).href);
      return;
    }
    if (request.action === 'install') {
      const record = await this.installer.install(
        request.listingId,
        request.versionId,
        request.copyIntent,
        request.intentId,
      );
      return record ? botUi(record) : null;
    }
    if (request.action === 'recover') {
      const owner = this.options.accountId();
      if (!owner) throw new Error('Sign in to recover a publication');
      await this.options.accountToken();
      if (this.options.accountId() !== owner)
        throw new Error('Account changed during publication recovery');
      const scope = sha256(Buffer.from(`${this.api.base.href}:${owner}`));
      const directory = join(
        this.options.store.botDir(request.botId),
        'publications',
        scope,
      );
      let receipt: { releases: Record<string, unknown> };
      try {
        receipt = JSON.parse(
          await fs.readFile(join(directory, 'receipt.json'), 'utf8'),
        );
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === 'ENOENT') return null;
        throw error;
      }
      const digest = Object.keys(receipt.releases).at(-1);
      if (!digest || !/^[a-f0-9]{64}$/.test(digest))
        throw new Error('Invalid publication recovery receipt');
      const prepared = await restoreBotPackage(
        this.options.store,
        request.botId,
        join(directory, digest),
        join(
          this.options.store.locations.userData,
          'bot-package-previews',
          randomUUID(),
        ),
      );
      if (this.options.accountId() !== owner) {
        await fs.rm(prepared.directory, { recursive: true, force: true });
        throw new Error('Account changed during publication recovery');
      }
      await this.remember(prepared);
      return prepared.preview;
    }
    if (request.action === 'prepare') {
      const prepared = await prepareBotPackage(
        this.options.store,
        request.selection,
        join(this.options.store.locations.userData, 'bot-package-previews'),
      );
      await this.remember(prepared);
      return prepared.preview;
    }
    if (request.action !== 'publish' && request.action !== 'export')
      throw new Error('Unsupported Bot market action');
    const prepared = this.prepared.get(request.previewId);
    if (
      !prepared ||
      request.confirmed !== true ||
      request.digest !== prepared.preview.digest
    )
      throw new Error(
        'Review and confirm the exact Bot package contents first',
      );
    if (
      this.options.store.get(prepared.sourceBotId).revision !==
      prepared.sourceRevision
    )
      throw new Error('Bot changed; prepare a new publication preview');
    if (request.action === 'export') {
      const result = await dialog.showSaveDialog({
        title: 'Export reviewed Bot package',
        defaultPath: `${prepared.preview.manifest.name}.zip`,
        filters: [{ name: 'Bot package', extensions: ['zip'] }],
      });
      if (!result.canceled && result.filePath)
        await durableWrite(result.filePath, prepared.archive);
      return;
    }
    const owner = this.options.accountId();
    if (!owner) throw new Error('Sign in before publishing a Bot');
    for (const key of [
      'name',
      'summary',
      'description',
      'category',
      'notes',
    ] as const)
      if (typeof request.metadata?.[key] !== 'string')
        throw new Error('Invalid Bot publication metadata');
    for (const [key, maximum, minimum] of [
      ['name', 60, 1],
      ['summary', 160, 1],
      ['description', 6000, 0],
      ['notes', 2000, 0],
    ] as const) {
      const size = Array.from(request.metadata[key].trim()).length;
      if (size < minimum || size > maximum)
        throw new Error(
          `Invalid publication ${key}; use ${minimum}–${maximum} characters`,
        );
    }
    if (
      ![
        'tools',
        'productivity',
        'games',
        'education',
        'visualization',
        'other',
      ].includes(request.metadata.category)
    )
      throw new Error('Invalid publication category');
    return this.publications.run(
      `${owner}:${prepared.sourceBotId}`,
      async () => {
        const writer = new MarketApiClient(
          async () => {
            const token = await this.options.accountToken();
            if (this.options.accountId() !== owner)
              throw new Error('Account changed during publication');
            if (
              this.options.store.get(prepared.sourceBotId).revision !==
              prepared.sourceRevision
            )
              throw new Error('Bot changed during publication');
            return token;
          },
          this.api.base.href,
          this.api.fetcher,
        );
        await writer.token();
        const scope = sha256(Buffer.from(`${this.api.base.href}:${owner}`));
        const directory = join(
          this.options.store.botDir(prepared.sourceBotId),
          'publications',
          scope,
        );
        await durableWrite(
          join(directory, prepared.preview.digest, 'review.json'),
          JSON.stringify(prepared.preview),
        );
        const result = await publishMarketPackage({
          writer,
          directory,
          digest: prepared.preview.digest,
          metadata: request.metadata,
          collection: 'bots',
          archive: async () => prepared.archive,
        });
        return {
          botId: result.appId,
          versionId: result.versionId,
          status: result.status,
        };
      },
    );
  }
}
