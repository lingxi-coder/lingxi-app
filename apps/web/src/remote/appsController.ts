import { isAppsListResult, isAppsOpenResult, type RemoteLocalApp } from '@lingxi/bridge-client/remote';
import type { ViewerConnection } from './viewerConnection';

export type AppsError = 'list' | 'open' | 'popup' | 'invalid';
export interface AppsState {
  apps?: readonly RemoteLocalApp[];
  loading: boolean;
  opening?: string;
  error?: AppsError;
}

export interface AppWindow {
  closed: boolean;
  opener: unknown;
  location: { replace(url: string): void };
  close(): void;
}

type AppRequests = Pick<ViewerConnection, 'request' | 'state' | 'subscribe'>;

/** Holds no open URL or ticket; all in-flight work is invalidated on disconnect or unmount. */
export class LocalAppsController {
  private state: AppsState = { loading: false };
  private readonly listeners = new Set<() => void>();
  private generation = 0;
  private listGeneration = 0;
  private popup: AppWindow | null = null;
  private active = false;
  private online = false;
  private unsubscribe: (() => void) | undefined;
  constructor(private readonly connection: AppRequests, private readonly now: () => number = Date.now) {}
  getState = (): AppsState => this.state;
  subscribe = (listener: () => void): (() => void) => { this.listeners.add(listener); return () => this.listeners.delete(listener); };
  private set(change: Partial<AppsState>): void {
    this.state = { ...this.state, ...change };
    for (const listener of this.listeners) listener();
  }
  activate(): void {
    if (this.active) return;
    this.active = true;
    this.online = this.connection.state.phase === 'ready';
    this.unsubscribe = this.connection.subscribe((state) => {
      const ready = state.phase === 'ready';
      if (ready === this.online) return;
      this.online = ready;
      if (ready) void this.refresh();
      else this.invalidate();
    });
    if (this.online) void this.refresh();
  }
  deactivate(): void {
    this.active = false;
    this.unsubscribe?.();
    this.unsubscribe = undefined;
    this.online = false;
    this.invalidate();
  }
  private invalidate(): void {
    this.generation += 1;
    this.listGeneration += 1;
    this.popup?.close();
    this.popup = null;
    this.set({ apps: undefined, loading: false, opening: undefined, error: undefined });
  }
  async refresh(): Promise<void> {
    if (!this.active || !this.online) return;
    const generation = this.generation;
    const list = ++this.listGeneration;
    this.set({ loading: true, error: undefined });
    try {
      const result: unknown = await this.connection.request('apps.list', {});
      if (!this.active || generation !== this.generation || list !== this.listGeneration) return;
      if (!isAppsListResult(result)) throw new Error('Invalid catalog');
      this.set({ apps: result.apps, loading: false });
    } catch {
      if (this.active && generation === this.generation && list === this.listGeneration) this.set({ loading: false, error: 'list' });
    }
  }
  /** Reserve a window synchronously from the click before asking the host for a URL. */
  async open(appId: string, reserve: () => AppWindow | null): Promise<void> {
    if (!this.active || !this.online || this.state.opening !== undefined || !this.state.apps?.some((app) => app.app_id === appId)) return;
    let popup: AppWindow | null;
    try { popup = reserve(); } catch { popup = null; }
    if (popup === null) { this.set({ error: 'popup' }); return; }
    try { popup.opener = null; } catch { popup.close(); this.set({ error: 'popup' }); return; }
    this.popup = popup;
    const generation = this.generation;
    this.set({ opening: appId, error: undefined });
    try {
      const result: unknown = await this.connection.request('apps.open', { app_id: appId });
      if (!this.active || generation !== this.generation || this.popup !== popup || popup.closed) return;
      if (!isAppsOpenResult(result) || result.app_id !== appId || result.expires_at <= this.now()) {
        popup.close();
        this.set({ error: 'invalid' });
      } else {
        popup.location.replace(result.url);
      }
    } catch {
      if (this.active && generation === this.generation && this.popup === popup) { popup.close(); this.set({ error: 'open' }); }
    } finally {
      if (this.popup === popup) { this.popup = null; this.set({ opening: undefined }); }
    }
  }
}
