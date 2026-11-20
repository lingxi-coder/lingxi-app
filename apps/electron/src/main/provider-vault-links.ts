import { mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

type Links = Record<string, Record<string, string>>;
const ID = /^[A-Za-z0-9._:-]{1,128}$/u;

/**
 * Which provider account on this Mac each vault record belongs to, per LingXi account (uid → vault id → account id).
 * A pull reconnects only the account an earlier pull or sync linked, never a hand-added one that happens to share its name,
 * and a sync of a second account of one provider gets its own vault record instead of replacing the first one's key.
 * Ids only: no key or label is kept here. Without a path the links live in memory (tests).
 */
export class ProviderVaultLinks {
  private memory: Links = {};
  private readonly path?: string;
  constructor(userData?: string) { if (userData) this.path = join(userData, 'provider-vault-links.json'); }

  accountFor(uid: string, vaultId: string): string | undefined { return this.read()[uid]?.[vaultId]; }
  vaultIdFor(uid: string, accountId: string): string | undefined {
    return Object.entries(this.read()[uid] ?? {}).find(([, account]) => account === accountId)?.[0];
  }
  /** Links `vaultId` to `accountId`, dropping any other link either of them had. */
  link(uid: string, vaultId: string, accountId: string): void {
    if (!ID.test(uid) || !ID.test(vaultId) || !ID.test(accountId)) return;
    const links = this.read(), own = Object.fromEntries(Object.entries(links[uid] ?? {}).filter(([vault, account]) => vault !== vaultId && account !== accountId));
    this.write({ ...links, [uid]: { ...own, [vaultId]: accountId } });
  }
  /** Keeps only the links whose account still exists on this Mac. */
  prune(uid: string, accountIds: readonly string[]): void {
    const links = this.read(), own = links[uid];
    if (!own) return;
    const kept = Object.fromEntries(Object.entries(own).filter(([, account]) => accountIds.includes(account)));
    if (Object.keys(kept).length !== Object.keys(own).length) this.write({ ...links, [uid]: kept });
  }

  private read(): Links {
    if (!this.path) return this.memory;
    try {
      const bytes = readFileSync(this.path);
      if (bytes.length > 262_144) return {};
      const value = JSON.parse(bytes.toString('utf8')) as { version?: unknown; links?: unknown };
      if (value.version !== 1 || !value.links || typeof value.links !== 'object') return {};
      const links: Links = {};
      for (const [uid, own] of Object.entries(value.links as Record<string, unknown>)) {
        if (!ID.test(uid) || !own || typeof own !== 'object') continue;
        links[uid] = Object.fromEntries(Object.entries(own as Record<string, unknown>).filter((entry): entry is [string, string] => ID.test(entry[0]) && typeof entry[1] === 'string' && ID.test(entry[1])));
      }
      return links;
    } catch { return {}; }
  }
  private write(links: Links): void {
    if (!this.path) { this.memory = links; return; }
    mkdirSync(dirname(this.path), { recursive: true });
    const temporary = `${this.path}.${process.pid}.tmp`;
    writeFileSync(temporary, JSON.stringify({ version: 1, links }), { mode: 0o600 });
    renameSync(temporary, this.path);
  }
}
