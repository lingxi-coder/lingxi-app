import { DIRECT_FAILURE_MEMORY_MS, systemClock, type Clock } from '@lingxi/bridge-client/remote';
import type { StorageLike } from './env';

export const TUNNEL_MEMORY_STORAGE_KEY = 'lingxi.remote.tunnel';

/**
 * Which computers could not be reached directly lately. A host that needed the
 * RTDB tunnel is dialled through the tunnel for ten minutes without a failed
 * direct attempt first. Kept in memory, and in `sessionStorage` (when it is
 * there) so a reload of the tab does not forget.
 */
export class TunnelMemory {
  private readonly until = new Map<string, number>();

  constructor(
    private readonly clock: Clock = systemClock,
    private readonly storage?: StorageLike,
    private readonly ttlMs: number = DIRECT_FAILURE_MEMORY_MS,
  ) {
    this.load();
  }

  /** Whether `hostId` should skip the direct attempt. */
  prefers(hostId: string): boolean {
    const until = this.until.get(hostId);
    if (until === undefined) return false;
    if (this.clock.now() < until) return true;
    this.until.delete(hostId);
    this.save();
    return false;
  }

  /** Dial `hostId` through the tunnel for the next ten minutes. */
  remember(hostId: string): void {
    this.until.set(hostId, this.clock.now() + this.ttlMs);
    this.save();
  }

  forget(hostId: string): void {
    if (this.until.delete(hostId)) this.save();
  }

  private load(): void {
    try {
      const parsed: unknown = JSON.parse(this.storage?.getItem(TUNNEL_MEMORY_STORAGE_KEY) ?? 'null');
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return;
      const now = this.clock.now();
      for (const [hostId, until] of Object.entries(parsed)) {
        if (typeof until === 'number' && Number.isFinite(until) && until > now) this.until.set(hostId, until);
      }
    } catch {
      // An unreadable value is as good as none.
    }
  }

  private save(): void {
    try {
      const now = this.clock.now();
      const live = [...this.until].filter(([, until]) => until > now);
      this.storage?.setItem(TUNNEL_MEMORY_STORAGE_KEY, JSON.stringify(Object.fromEntries(live)));
    } catch {
      // Storage may be full or blocked; the in-memory copy still works.
    }
  }
}
