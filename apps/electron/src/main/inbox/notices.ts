import type { InboxCursor, InboxNotice } from '../../../../../packages/inbox/contracts.js';
import type { InboxNotification } from '../../shared/inbox.js';
import type { DeliveryLedger } from './deliveryLedger.js';
import type { CloudRequest } from './publisher.js';
type NoticePage = { items: InboxNotice[]; next?: InboxCursor | null };
type Alert = InboxNotification & { key: string; generation: number };

/** A dirty shard wakes one bounded pass over recent notices. A delayed notice is re-authorized at its due time. */
export class NoticeDelivery {
  private timer?: ReturnType<typeof setTimeout>;
  private wakeAt = Infinity;
  private version = 0;
  private running = false;
  private dirty = false;
  constructor(
    private readonly post: CloudRequest,
    private readonly ledger: DeliveryLedger,
    private readonly live: (uid: string, epoch: number) => boolean,
    private readonly visible: (key: string) => boolean,
    private readonly enabled: () => boolean,
    private readonly deliver: (notice: Alert) => void,
    private readonly deviceId: string,
  ) {}
  changed(uid: string, epoch: number) {
    this.dirty = true;
    if (this.running) return;
    this.schedule(uid, epoch, 125);
  }
  private schedule(uid: string, epoch: number, delay: number) {
    const next = Date.now() + delay;
    if (this.timer && this.wakeAt <= next) return;
    clearTimeout(this.timer);
    this.wakeAt = next;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      this.wakeAt = Infinity;
      void this.read(uid, epoch);
    }, delay);
    this.timer.unref?.();
  }
  private async read(uid: string, epoch: number) {
    if (!this.live(uid, epoch) || !this.enabled()) return;
    const version = this.version;
    this.running = true;
    this.dirty = false;
    let due = Infinity;
    try {
      let after: InboxCursor | undefined;
      for (let pageIndex = 0; pageIndex < 20; pageIndex++) {
        const page: NoticePage = await this.post(uid, epoch, {
          action: 'notices',
          deviceId: this.deviceId,
          ...(after ? { after } : {}),
        });
        if (!this.live(uid, epoch) || version !== this.version || !this.enabled()) return;
        const now = Date.now();
        for (const notice of page.items) {
          if (
            !notice.deliverable ||
            !Number.isSafeInteger(notice.deliverAfter) ||
            notice.lastActivityAt < now - 3600000
          )
            continue;
          const event = 'inbox:' + notice.id;
          if (this.ledger.has(event, now)) continue;
          if (notice.deliverAfter > now) {
            due = Math.min(due, notice.deliverAfter);
            continue;
          }
          if (this.visible(notice.key)) continue;
          this.deliver({
            key: notice.key,
            generation: notice.generation,
            kind: notice.kind,
            ...(typeof notice.through === 'number' && Number.isSafeInteger(notice.through) && notice.through >= 0
              ? { through: notice.through }
              : {}),
            ...(typeof notice.messageId === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(notice.messageId)
              ? { messageId: notice.messageId }
              : {}),
            ...(typeof notice.taskId === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(notice.taskId)
              ? { taskId: notice.taskId }
              : {}),
            title: 'LingXi',
            body: 'Conversation update',
          });
          this.ledger.remember([event], now);
        }
        if (!page.next || page.next.lastActivityAt < now - 3600000) break;
        after = page.next;
      }
    } catch {
      due = Date.now() + 30000;
    } finally {
      if (version === this.version) {
        this.running = false;
        if (this.live(uid, epoch) && (this.dirty || Number.isFinite(due)))
          this.schedule(uid, epoch, this.dirty ? 125 : Math.max(1, due - Date.now()));
      }
    }
  }
  reset() {
    this.version++;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.wakeAt = Infinity;
    this.running = false;
    this.dirty = false;
  }
}
