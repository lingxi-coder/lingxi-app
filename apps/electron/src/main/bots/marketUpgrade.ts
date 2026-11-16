import { constants, promises as fs } from 'node:fs';
import { dirname, join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type {
  BotPackageManifest,
  BotUpgradeFile,
  BotUpgradePlan,
} from '../../shared/botMarket.js';
import type { MarketApiClient } from '../local-apps/marketClient.js';
import {
  MarketInstallGuard,
  marketApplicationId,
} from '../local-apps/marketLinkInstaller.js';
import { canonical, sha256 } from '../local-apps/sourcePackage.js';
import { writeFileAtomic } from './files.js';
import {
  checkedBotRelease,
  downloadBotRelease,
  installedDescription,
  packageSkillName,
  type BotReleaseContent,
  type CheckedBotRelease,
} from './marketRelease.js';
import { validateBotManifest } from './package.js';
import { BotConflictError, type BotRecord, type BotStore } from './store.js';

/**
 * What becomes of one file when an installed Bot moves from the release it was installed as (`base`) to another (`next`),
 * given the file as it is in the Bot now (`local`). Each is the file's SHA-256, or `undefined` where the file is not there.
 *
 * - `none`: the release did not change it and neither did the owner.
 * - `kept`: the release did not change it; the owner did, and their file stays.
 * - `same`: the owner's file already is the new release's.
 * - `update` / `add` / `remove`: the owner left it as installed, so it follows the release.
 * - `conflict`: both changed it, differently. It stays as it is until the owner chooses.
 */
export type UpgradeState =
  | 'none'
  | 'kept'
  | 'same'
  | 'update'
  | 'add'
  | 'remove'
  | 'conflict';
export function upgradeState(
  base: string | undefined,
  local: string | undefined,
  next: string | undefined,
): UpgradeState {
  if (base === next) return local === base ? 'none' : 'kept';
  if (local === next) return 'same';
  if (local === base)
    return next === undefined ? 'remove' : base === undefined ? 'add' : 'update';
  return 'conflict';
}

/** What the new release may read that the installed one may not, and the reverse. Any addition needs the owner's word. */
export function accessChange(
  base: BotPackageManifest,
  next: BotPackageManifest,
): Pick<BotUpgradePlan, 'tools' | 'resources' | 'expands'> {
  const tools = (m: BotPackageManifest) => [
    ...new Set(m.capabilities.map((c) => c.tool)),
  ];
  const ids = (m: BotPackageManifest) => m.resources.map((r) => r.id);
  const required = (m: BotPackageManifest) =>
    m.resources.filter((r) => r.required).map((r) => r.id);
  const minus = (a: string[], b: string[]) =>
    a.filter((v) => !b.includes(v)).sort();
  const added = {
    tools: minus(tools(next), tools(base)),
    resources: minus(ids(next), ids(base)),
  };
  return {
    tools: { added: added.tools, removed: minus(tools(base), tools(next)) },
    resources: {
      added: added.resources,
      removed: minus(ids(base), ids(next)),
      // A resource the Bot could already use that sharing now asks for. It widens nothing.
      nowRequired: minus(required(next), required(base)).filter(
        (id) => !added.resources.includes(id),
      ),
    },
    expands: added.tools.length > 0 || added.resources.length > 0,
  };
}

interface Item {
  key: string;
  kind: 'role' | 'skill';
  name: string;
  /** Where the file is in the Bot. */
  path: string;
  base?: string;
  local?: string;
  next?: string;
  mine: string | null;
  theirs: string | null;
  state: UpgradeState;
}

export interface BotUpgradeOptions {
  store: BotStore;
  api: MarketApiClient;
  accountId(): string | undefined;
  assertAvailable(): void;
  /** Commits the record once the files are in place. */
  upgradeBot(
    botId: string,
    expectedRevision: number,
    release: {
      releaseId: string;
      digest: string;
      name?: string;
      description?: string;
    },
  ): BotRecord;
}

const MAX_TEXT = 1024 * 1024;

/**
 * Moves a Bot that was installed from the market to a newer release of its listing (design 16.4).
 *
 * The Bot's own changes are its local overlay: the role and Skills as they are on disk, compared by digest with the release
 * it was installed as (`market-package.json`). A file the owner left alone follows the new release; a file only the owner
 * changed stays; a file both changed is a conflict, and the Bot stays on its release until the owner has chosen for every
 * one of them. A release that reads more than the installed one needs the owner's agreement as well. Declining is doing
 * nothing: the Bot keeps its release, its files, its conversations and its memory.
 *
 * Only the role, the packaged Skills, `market-package.json` and the record are ever written. Skills the owner added,
 * memory, conversations, settings, the model and what was published to a Team are not touched: a Team runs the snapshot
 * it was published with, so tasks under way keep the role and files they began with, and the new release reaches a Team
 * only when its maintainer publishes again.
 */
export class BotMarketUpgrader {
  private readonly guard = new MarketInstallGuard<BotRecord>();
  constructor(private readonly options: BotUpgradeOptions) {}

  /** The newest release of the Bot's listing, as a plan; `null` when the Bot is on it already. Changes nothing. */
  async check(botIdValue: unknown): Promise<BotUpgradePlan | null> {
    const { record, account } = this.begin(botIdValue);
    const origin = record.marketOrigin!;
    const versions = await this.request<{ id: string }[]>(
      account,
      `bots/${origin.listingId}/versions?limit=1`,
    );
    const latest = versions[0]?.id;
    if (!latest || marketApplicationId(latest) === origin.releaseId)
      return null;
    return (await this.plan(record.bot_id, account, latest)).plan;
  }

  /** Carries a reviewed plan out. Refused, with nothing written, unless the plan still holds and every choice it needs was made. */
  apply(input: {
    botId: unknown;
    expectedRevision: unknown;
    releaseId: unknown;
    plan: unknown;
    choices: unknown;
    approveAccess: unknown;
  }): Promise<BotRecord> {
    const { record, account } = this.begin(input.botId);
    const releaseId = marketApplicationId(input.releaseId);
    if (
      typeof input.plan !== 'string' ||
      !/^[a-f0-9]{64}$/.test(input.plan) ||
      !Number.isSafeInteger(input.expectedRevision) ||
      !input.choices ||
      typeof input.choices !== 'object' ||
      Array.isArray(input.choices) ||
      Object.values(input.choices).some((c) => c !== 'mine' && c !== 'theirs')
    )
      throw new Error('Invalid Bot upgrade request');
    const choices = input.choices as Record<string, 'mine' | 'theirs'>;
    return this.guard.run(record.bot_id, async () => {
      const { plan, items, checked, base } = await this.plan(
        record.bot_id,
        account,
        releaseId,
      );
      // The release, the Bot's files or its record changed since the owner looked: they look again.
      if (plan.digest !== input.plan || plan.expectedRevision !== input.expectedRevision)
        throw new Error('BOT_UPGRADE_CHANGED');
      if (plan.expands && input.approveAccess !== true)
        throw new Error('BOT_UPGRADE_APPROVAL_REQUIRED');
      const conflicts = items.filter((i) => i.state === 'conflict');
      if (conflicts.some((i) => !choices[i.key]))
        throw new Error('BOT_UPGRADE_CHOICE_REQUIRED');
      this.account(account);
      this.options.assertAvailable();
      const { store } = this.options;
      if (store.get(record.bot_id).revision !== plan.expectedRevision)
        throw new BotConflictError();
      for (const item of items) {
        const take =
          item.state === 'update' ||
          item.state === 'add' ||
          item.state === 'remove' ||
          (item.state === 'conflict' && choices[item.key] === 'theirs');
        if (!take) continue;
        if (item.theirs !== null) writeFileAtomic(item.path, item.theirs);
        else {
          await fs.rm(item.path, { force: true });
          // The Skill's folder goes with it only when nothing of the owner's is left in it.
          await fs.rmdir(dirname(item.path)).catch(() => {});
        }
      }
      // From here the Bot's files are the merged ones. The package it is compared with next time is the new release.
      writeFileAtomic(
        join(store.botDir(record.bot_id), 'market-package.json'),
        canonical(checked.manifest),
      );
      const current = store.get(record.bot_id);
      const next = checked.manifest;
      return this.options.upgradeBot(record.bot_id, plan.expectedRevision, {
        releaseId,
        digest: checked.digest,
        // The name and the description follow the release only while they are still the ones the installed release gave.
        ...(current.name === base.name && next.name !== base.name
          ? { name: next.name }
          : {}),
        ...(current.description === installedDescription(base.version) &&
        next.version !== base.version
          ? { description: installedDescription(next.version) }
          : {}),
      });
    });
  }

  private begin(botIdValue: unknown): { record: BotRecord; account: string } {
    if (typeof botIdValue !== 'string')
      throw new Error('Invalid Bot upgrade request');
    this.options.assertAvailable();
    const record = this.options.store.get(botIdValue);
    const origin = record.marketOrigin;
    if (!origin) throw new Error('This Bot was not installed from the market');
    const account = this.options.accountId();
    if (!account) throw new Error('Sign in to update a Bot');
    // A release is downloaded with the authority of the account that installed the Bot, from the market it came from.
    if (account !== origin.accountId || this.options.api.base.href !== origin.market)
      throw new Error('Sign in with the account that installed this Bot');
    return { record, account };
  }
  private account(account: string): void {
    if (this.options.accountId() !== account)
      throw new Error('Account changed during Bot update');
  }
  private async request<T>(account: string, path: string): Promise<T> {
    this.account(account);
    const result = await this.options.api.request<T>(path, undefined, true);
    this.account(account);
    return result;
  }

  /** The manifest of the release the Bot was installed as. */
  private async installed(botId: string): Promise<BotPackageManifest> {
    const text = await readPrivate(
      join(this.options.store.botDir(botId), 'market-package.json'),
    );
    if (text === null || text === undefined)
      throw new Error('INVALID_BOT_PACKAGE');
    return validateBotManifest(JSON.parse(text));
  }

  private async plan(
    botId: string,
    account: string,
    releaseId: string,
  ): Promise<{
    plan: BotUpgradePlan;
    items: Item[];
    checked: CheckedBotRelease;
    /** The release the Bot is on. */
    base: BotPackageManifest;
  }> {
    const { store, api } = this.options;
    const record = store.get(botId);
    const origin = record.marketOrigin!;
    const base = await this.installed(botId);
    const checked = await checkedBotRelease(
      (path) => this.request(account, path),
      origin.listingId,
      releaseId,
    );
    const next = checked.manifest;
    // Each look unpacks in a folder of its own, gone afterwards: a look and an upgrade of the same Bot can overlap.
    const directory = join(
      store.locations.userData,
      'bot-market-upgrades',
      randomUUID(),
    );
    await fs.mkdir(directory, { recursive: true, mode: 0o700 });
    let downloaded: BotReleaseContent;
    try {
      downloaded = await downloadBotRelease(api, checked, directory);
    } finally {
      await fs.rm(directory, { recursive: true, force: true });
    }
    this.account(account);
    const hash = (m: BotPackageManifest, path: string) =>
      m.files.find((f) => f.path === path)?.sha256;
    const home = store.homeDir(botId);
    const items: Item[] = [];
    const add = async (
      item: Pick<Item, 'key' | 'kind' | 'name' | 'path' | 'base' | 'next' | 'theirs'>,
    ) => {
      const mine = await readPrivate(item.path);
      // A file that is not a plain readable text is the owner's doing: it is never mistaken for the installed one.
      const local =
        mine === undefined
          ? 'irregular'
          : mine === null
            ? undefined
            : sha256(Buffer.from(mine));
      const state = upgradeState(item.base, local, item.next);
      if (mine === undefined && state === 'conflict')
        throw new Error(`BOT_UPGRADE_FILE: ${item.name}`);
      items.push({ ...item, local, mine: mine ?? null, state });
    };
    await add({
      key: 'role',
      kind: 'role',
      name: next.name,
      path: join(home, 'LINGXI.md'),
      base: hash(base, base.entry),
      next: hash(next, next.entry),
      theirs: downloaded.content.get(next.entry)!,
    });
    // A Skill is the same Skill across releases by its name, whatever the case of its folder.
    const skills = new Map<string, { name: string; base?: string; next?: string; theirs: string | null }>();
    for (const s of base.skills) {
      const name = packageSkillName(s.path);
      skills.set(name.toLowerCase(), { name, base: hash(base, s.path), theirs: null });
    }
    for (const s of downloaded.skills) {
      const prior = skills.get(s.name.toLowerCase());
      skills.set(s.name.toLowerCase(), {
        name: prior?.name ?? s.name,
        ...(prior?.base !== undefined ? { base: prior.base } : {}),
        next: hash(next, s.path),
        theirs: s.text,
      });
    }
    for (const [key, s] of [...skills].sort(([a], [b]) => (a < b ? -1 : 1)))
      await add({
        key: `skill:${key}`,
        kind: 'skill',
        name: s.name,
        path: join(home, 'skills', s.name, 'SKILL.md'),
        base: s.base,
        next: s.next,
        theirs: s.theirs,
      });
    const files: BotUpgradeFile[] = items
      .filter((i) => i.state !== 'none' && i.state !== 'same')
      .map((i) => ({
        key: i.key,
        kind: i.kind,
        name: i.name,
        state: i.state as BotUpgradeFile['state'],
        ...(i.state === 'conflict' ? { mine: i.mine, theirs: i.theirs } : {}),
      }));
    const access = accessChange(base, next);
    const digest = sha256(
      Buffer.from(
        canonical({
          schema: 1,
          botId,
          revision: record.revision,
          from: origin.digest,
          to: checked.digest,
          files: items.map((i) => [i.key, i.state, i.local ?? null, i.next ?? null]),
          access,
        }),
      ),
    );
    return {
      plan: {
        kind: 'upgrade',
        botId,
        expectedRevision: record.revision,
        from: { releaseId: origin.releaseId, version: base.version },
        to: { releaseId, version: next.version, name: next.name },
        digest,
        files,
        ...(base.license !== next.license
          ? { license: { from: base.license, to: next.license } }
          : {}),
        ...access,
      },
      items,
      checked,
      base,
    };
  }
}

/**
 * A private text file of the Bot: its text, `null` when it is not there, `undefined` when it is there but is not a plain
 * readable text (a link, a folder, too large, not UTF-8). Links are never followed.
 */
async function readPrivate(path: string): Promise<string | null | undefined> {
  let handle;
  try {
    handle = await fs.open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  } catch (error) {
    const code = (error as NodeJS.ErrnoException).code;
    return code === 'ENOENT' || code === 'ENOTDIR' ? null : undefined;
  }
  try {
    const info = await handle.stat();
    if (!info.isFile() || info.size > MAX_TEXT) return undefined;
    return new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(
      await handle.readFile(),
    );
  } catch {
    return undefined;
  } finally {
    await handle.close();
  }
}
