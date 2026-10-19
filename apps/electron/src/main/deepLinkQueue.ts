import { randomUUID } from 'node:crypto';
import { buildDeepLink, parseDeepLink, type DeepLinkTarget } from '../../../../packages/deep-links/index';

export type DeepLinkDelivery = { id: string; target: DeepLinkTarget };
/** The main process owns delivery until the renderer acknowledges navigation.
 * Re-reading after renderer recreation returns the same delivery identity.
 */
export class DeepLinkQueue {
  private readonly pending: Array<DeepLinkDelivery & { key: string }> = [];
  constructor(private readonly capacity = 32) {}
  enqueue(raw: string): DeepLinkDelivery | null {
    const target = parseDeepLink(raw);
    const key = buildDeepLink(target);
    const existing = this.pending.find(item => item.key === key);
    if (existing) return null;
    if (this.pending.length >= this.capacity) throw new Error('Too many pending LingXi links. Open or dismiss the queued links, then try again.');
    const delivery = { id: randomUUID(), target, key };
    this.pending.push(delivery);
    return { id: delivery.id, target };
  }
  next(): DeepLinkDelivery | null {
    const item = this.pending[0];
    return item ? { id: item.id, target: item.target } : null;
  }
  acknowledge(id: string): boolean {
    if (this.pending[0]?.id !== id) return false;
    this.pending.shift();
    return true;
  }
  get hasPending(): boolean { return this.pending.length > 0; }
}
