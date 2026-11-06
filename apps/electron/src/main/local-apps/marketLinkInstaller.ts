import type { DesktopLocalApp } from '../../shared/localApps.js';

export function marketApplicationId(value: unknown): string {
  if (typeof value !== 'string' || !/^[\w-]{1,128}$/u.test(value)) throw new Error('Invalid market application ID');
  return value;
}

/** Links contain an identity only; the trusted market supplies the package. */
export class MarketLinkInstaller {
  private readonly pending = new MarketInstallGuard<DesktopLocalApp | null>();
  constructor(private readonly options: {
    list(): Promise<DesktopLocalApp[]>;
    preview(id: string): Promise<{ id: string; name: string; summary: string }>;
    confirm(app: { name: string; summary: string }): Promise<boolean>;
    install(id: string): Promise<DesktopLocalApp>;
    installed(): void;
  }) {}
  install(value: unknown): Promise<DesktopLocalApp | null> {
    const id = marketApplicationId(value);
    return this.pending.run(id, () => this.perform(id));
  }
  private async perform(id: string): Promise<DesktopLocalApp | null> {
    const existing = (await this.options.list()).find(app => app.marketOrigin?.appId === id && !app.marketOrigin.remixedFrom);
    if (existing) return existing;
    const app = await this.options.preview(id);
    if (app.id !== id || typeof app.name !== 'string' || typeof app.summary !== 'string') throw new Error('Market returned an invalid application');
    if (!await this.options.confirm(app)) return null;
    const installed = await this.options.install(id);
    this.options.installed();
    return installed;
  }
}

/** Shared duplicate-click guard; persistent identity remains the destination store's responsibility. */
export class MarketInstallGuard<T> {
  private readonly pending = new Map<string, Promise<T>>();
  run(key: string, operation: () => Promise<T>): Promise<T> {
    const prior = this.pending.get(key); if (prior) return prior;
    const next = operation().finally(() => this.pending.delete(key)); this.pending.set(key,next); return next;
  }
}
