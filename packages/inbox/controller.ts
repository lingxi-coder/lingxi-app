import {
  freezeRead,
  matchesInboxCategory,
  orderItems,
  type InboxCategory,
  type InboxQueryCategory,
  type InboxCursor,
  type InboxNotice,
  type InboxFilter,
  type InboxItem,
  type InboxMutation,
  type InboxPage,
  type InboxPreferences,
  type InboxRequest,
  type InboxReadBatch,
} from './contracts.js';
import { browserInboxStorage, enqueue, type InboxStorage } from './storage.js';

export interface InboxSnapshot {
  cloudUnavailable?: boolean;
  items: InboxItem[];
  filter: InboxFilter;
  category?: InboxCategory;
  searchPrefix?: string;
  archived: boolean;
  badge: number;
  hasMore: boolean;
  busy: boolean;
  offline: boolean;
  error: string;
  pending: number;
  batchBusy?: boolean;
}
interface Options {
  uid: string;
  request: InboxRequest;
  storage?: InboxStorage;
  scope?: 'messages' | 'assistant';
  watch?: (dirty: () => void, badge: (n: number) => void, error: (e: Error) => void) => () => void;
}
const LEASE_MS = 5 * 60_000;
const message = (error: unknown) => (error instanceof Error ? error.message : 'INBOX_REQUEST_FAILED');
const forbidden = (error: string) =>
  [
    'STALE_MEMBERSHIP',
    'SOURCE_ACCESS_REQUIRED',
    'MEMBERSHIP_REQUIRED',
    'INVALID_SESSION',
    'SIGN_IN_REQUIRED',
    'permission-denied',
    'unauthenticated',
  ].includes(error);
export class InboxController {
  private state: InboxSnapshot = {
    items: [],
    filter: 'all',
    archived: false,
    badge: 0,
    hasMore: false,
    busy: true,
    offline: false,
    error: '',
    pending: 0,
  };
  private listeners = new Set<() => void>();
  private storage: InboxStorage;
  private next?: InboxPage['next'];
  private revision = 0;
  private closed = false;
  private started = false;
  private running?: Promise<void>;
  private batchTimer?: ReturnType<typeof setTimeout>;
  private retry?: ReturnType<typeof setTimeout>;
  private expiry?: ReturnType<typeof setTimeout>;
  private refreshTimer?: ReturnType<typeof setTimeout>;
  private stop?: () => void;
  private verifiedUntil = 0;
  private partialLocal = new Set<string>();
  private rerun = false;
  private pages = 1;
  private owner = crypto.randomUUID();
  constructor(private options: Options) {
    this.storage = options.storage ?? browserInboxStorage;
  }
  getSnapshot = () => this.state;
  get scope() {
    return this.options.scope;
  }
  subscribe = (fn: () => void) => {
    this.listeners.add(fn);
    return () => {
      this.listeners.delete(fn);
    };
  };
  private set(patch: Partial<InboxSnapshot>) {
    if (this.closed) return;
    this.state = { ...this.state, ...patch };
    for (const fn of this.listeners) fn();
  }
  private expire(verifiedAt: number) {
    this.verifiedUntil = verifiedAt + LEASE_MS;
    clearTimeout(this.expiry);
    this.expiry = setTimeout(
      () => {
        if (this.state.offline)
          this.set({ items: this.state.items.filter((i) => this.partialLocal.has(i.key)), hasMore: false });
      },
      Math.max(0, verifiedAt + LEASE_MS - Date.now()),
    );
  }
  private dirty = () => {
    clearTimeout(this.refreshTimer);
    this.refreshTimer = setTimeout(() => {
      void this.refresh();
      void this.drain();
    }, 75);
  };
  async start() {
    if (this.started || this.closed) return;
    this.started = true;
    // Render the last authorized first page before the network request. Source access expires when offline.
    try {
      const saved = await this.storage.update(this.options.uid, (record) => ({
        cache: this.scope ? record.scopedCache?.[this.scope] : record.cache,
        batch: record.batch,
        pending: record.queue.length,
      }));
      if (this.closed) return;
      if (saved.batch) {
        this.set({ batchBusy: true });
        void this.followBatch(saved.batch);
      }
      if (saved.cache && saved.cache.verifiedAt + LEASE_MS > Date.now()) {
        this.next = saved.cache.page.next;
        this.set({
          items: saved.cache.page.items.filter(item => matchesInboxCategory(item.category, this.scope)),
          hasMore: !!this.next,
          offline: true,
          pending: saved.pending,
          busy: false,
        });
        this.expire(saved.cache.verifiedAt);
      }
    } catch (error) {
      this.set({ error: message(error) });
    }
    if (this.closed) return;
    this.stop = this.options.watch?.(
      this.dirty,
      (badge) => this.set({ badge }),
      (error) => {
        if (forbidden(error.message)) {
          this.revision++;
          this.verifiedUntil = 0;
          this.next = undefined;
          this.partialLocal.clear();
          void this.storage
            .update(this.options.uid, (record) => {
              delete record.cache;
              delete record.scopedCache;
            })
            .catch(() => {});
        }
        this.set({
          offline: true,
          error: message(error),
          ...(forbidden(error.message) ? { items: [], badge: 0, hasMore: false } : {}),
        });
      },
    );
    globalThis.addEventListener?.('online', this.dirty);
    await Promise.all([this.refresh(), this.drain()]);
  }
  async setFilter(
    filter: InboxFilter,
    category?: InboxCategory,
    archived = false,
    searchPrefix = this.state.searchPrefix,
  ) {
    if (this.scope === 'assistant' || (this.scope === 'messages' && category === 'assistant')) category = undefined;
    searchPrefix = searchPrefix?.trim().normalize('NFKC').toLowerCase() || undefined;
    if (
      filter === this.state.filter &&
      category === this.state.category &&
      archived === this.state.archived &&
      searchPrefix === this.state.searchPrefix
    )
      return;
    this.next = undefined;
    this.pages = 1;
    this.set({ filter, category, archived, searchPrefix, items: [], hasMore: false });
    await this.refresh();
  }
  private queryCategory(category?: InboxCategory): { category?: InboxQueryCategory } {
    const selected = category ?? this.scope;
    return selected ? { category: selected } : {};
  }
  draftPreview(item: InboxItem): string {
    if (item.ref.kind !== 'team') return '';
    const { sourceId, generation, conversationId } = item.ref;
    try {
      return (
        globalThis.localStorage
          ?.getItem('lingxi.teamDraft.' + [this.options.uid, sourceId, generation, conversationId].join(':'))
          ?.slice(0, 160) ?? ''
      );
    } catch {
      return '';
    }
  }
  notice = async (id: string) => {
    const { item } = await this.options.request<{ item: InboxNotice }>({ action: 'notice', id });
    if (this.closed || item.id !== id || item.lastActivityAt <= Date.now() - 30 * 86400000)
      throw new Error('NOTICE_MISSING');
    return item;
  };
  notices = async (after?: InboxCursor) => {
    const page = await this.options.request<{ items: InboxNotice[]; next?: InboxCursor | null }>({
      action: 'notices',
      ...(after ? { after } : {}),
    });
    if (this.closed) throw new Error('INVALID_SESSION');
    return page;
  };
  get = async (key: string, generation?: number) => {
    const { item } = await this.options.request<{ item: InboxItem }>({
      action: 'get',
      key,
      ...(generation === undefined ? {} : { generation }),
    });
    if (this.closed || item.key !== key) throw new Error('INVALID_SESSION');
    return item;
  };
  refresh = () => this.load(false);
  more = () => (this.state.busy || !this.next ? Promise.resolve() : this.load(true));
  private async load(append: boolean) {
    if (this.closed) return;
    const version = ++this.revision,
      { filter, category, archived, searchPrefix } = this.state,
      after = append ? this.next : null;
    this.set({ busy: true });
    try {
      const rows = new Map((append ? this.state.items : []).map((item) => [item.key, item]));
      let cursor = after,
        page: InboxPage,
        first: InboxPage | undefined;
      const requested = append ? 1 : this.pages;
      for (let i = 0; i < requested; i++) {
        page = await this.options.request<InboxPage>({
          action: 'list',
          ...(searchPrefix ? { searchPrefix } : {}),
          filter,
          ...(archived ? { archived } : {}),
          ...this.queryCategory(category),
          ...(cursor ? { after: cursor } : {}),
        });
        if (version !== this.revision || this.closed) return;
        first ??= page;
        if (page.partialOffline) {
          if (!append && i === 0) {
            this.partialLocal.clear();
            if (this.verifiedUntil > Date.now())
              for (const cached of this.state.items) rows.set(cached.key, cached);
          }
          for (const item of page.items) this.partialLocal.add(item.key);
        }
        for (const item of page.items) {
          if (matchesInboxCategory(item.category, this.scope)) rows.set(item.key, item);
        }
        cursor = page.next ?? null;
        if (!cursor) break;
      }
      const queue = await this.storage
        .update(this.options.uid, (record) => record.queue.map((q) => q.command))
        .catch(() => []);
      if (this.closed || version !== this.revision) return;
      for (const command of queue) {
        if (command.action !== 'preferences') continue;
        const row = rows.get(command.key),
          expected = command.expectedPreferencesRevision;
        if (row && row.ref.generation === command.generation && (row.preferencesRevision ?? 0) === expected) {
          rows.set(row.key, { ...row, ...command.patch, preferencesRevision: expected + 1 });
        }
      }
      this.next = cursor;
      if (append) this.pages++;
      this.set({
        items: searchPrefix ? [...rows.values()] : [...rows.values()].sort(orderItems),
        hasMore: !!cursor,
        busy: false,
        offline: !!first?.partialOffline,
        cloudUnavailable: !!first?.cloudUnavailable,
        error: '',
      });
      if (first?.partialOffline) return;
      this.partialLocal.clear();
      this.expire(Date.now());
      if (first && !append && filter === 'all' && !category && !archived && !searchPrefix) {
        const page = first;
        await this.storage.update(this.options.uid, (record) => {
          const entry = { page, verifiedAt: Date.now() };
          if (this.scope) (record.scopedCache ??= {})[this.scope] = entry;
          else record.cache = entry;
        });
      }
    } catch (error) {
      if (version !== this.revision || this.closed) return;
      const reason = message(error);
      this.set({
        busy: false,
        offline: true,
        error: reason,
        ...(forbidden(reason) || this.verifiedUntil <= Date.now()
          ? { items: [], hasMore: false, badge: 0 }
          : {}),
      });
      if (forbidden(reason))
        await this.storage
          .update(this.options.uid, (record) => {
            delete record.cache;
            delete record.scopedCache;
          })
          .catch(() => {});
    }
  }
  markRead = (row: InboxItem) => this.mutate({ action: 'markRead', ...freezeRead(row) });
  markUnread = (row: InboxItem) =>
    this.mutate({
      action: 'markUnread',
      expectedManualUnreadRevision: row.manualUnreadRevision,
      key: row.key,
      generation: row.ref.generation,
      operationId: crypto.randomUUID(),
    });
  preferences = async (row: InboxItem, patch: Partial<InboxPreferences>) => {
    if (this.closed) throw new Error('INVALID_SESSION');
    const { pending, expected } = await this.storage.update(this.options.uid, (record) => {
      const predecessor = [...record.queue]
        .reverse()
        .find(
          (q) =>
            q.command.action === 'preferences' &&
            q.command.key === row.key &&
            q.command.generation === row.ref.generation,
        )?.command;
      const expected = Math.max(
        row.preferencesRevision ?? 0,
        predecessor?.action === 'preferences' ? (predecessor.expectedPreferencesRevision ?? 0) + 1 : 0,
      );
      enqueue(
        record,
        {
          action: 'preferences',
          key: row.key,
          generation: row.ref.generation,
          expectedPreferencesRevision: expected,
          patch,
        },
        crypto.randomUUID(),
      );
      return { pending: record.queue.length, expected };
    });
    this.set({
      pending,
      items: this.state.items
        .map((item) =>
          item.key === row.key && item.ref.generation === row.ref.generation
            ? { ...item, ...patch, preferencesRevision: expected + 1 }
            : item,
        )
        .sort(orderItems),
    });
    await this.drain();
  };
  async movePinned(row: InboxItem, direction: -1 | 1) {
    const pinned = this.state.items.filter((i) => i.pinOrder !== null).sort(orderItems);
    const index = pinned.findIndex((i) => i.key === row.key);
    const other = pinned[index + direction];
    if (index < 0 || !other || other.pinOrder === null || row.pinOrder === null) return;
    const adjacent = pinned[index + direction * 2];
    const lower = direction < 0 ? (adjacent?.pinOrder ?? -1) : other.pinOrder;
    const upper =
      direction < 0
        ? other.pinOrder
        : (adjacent?.pinOrder ?? Math.min(Number.MAX_SAFE_INTEGER, other.pinOrder + 4096));
    if (upper - lower > 1) {
      await this.preferences(row, { pinOrder: lower + Math.floor((upper - lower) / 2) });
      return;
    }
    if (other.pinOrder !== row.pinOrder) {
      // A full integer interval has no midpoint. Swap the two frozen ranks using each row's personal revision.
      await this.preferences(other, { pinOrder: row.pinOrder });
      if (this.state.error === 'PREFERENCE_REVISION_CONFLICT') return;
      await this.preferences(row, { pinOrder: other.pinOrder });
      return;
    }
    // Very old equal ranks need a little space. Assign the affected tied run distinct ranks before moving one row.
    const reordered = [...pinned];
    reordered.splice(index, 1);
    reordered.splice(index + direction, 0, row);
    let rank = 1024;
    for (const item of reordered) {
      if (item.pinOrder !== rank) await this.preferences(item, { pinOrder: rank });
      if (this.state.error === 'PREFERENCE_REVISION_CONFLICT') return;
      rank += 1024;
    }
  }
  /** An offline replay may resume an admitted job, but must never create a new snapshot over later arrivals. */
  async markAllReadSnapshot() {
    if (this.closed || this.state.offline || this.state.busy) throw new Error('INBOX_SNAPSHOT_OFFLINE');
    if (this.state.batchBusy) return;
    const { filter, category, archived, searchPrefix } = this.state;
    this.set({ batchBusy: true });
    const id = crypto.randomUUID();
    const pending = await this.storage
      .update(this.options.uid, (record) => (record.batch ??= { jobId: id, startedAt: Date.now() }))
      .catch((error) => {
        this.set({ batchBusy: false });
        throw error;
      });
    if (this.closed) return;
    if (pending.jobId !== id) {
      await this.followBatch(pending);
      return;
    }
    try {
      const job = await this.options.request<InboxReadBatch>({
        action: 'markAllReadSnapshot',
        operationId: id,
        requestedAt: pending.startedAt,
        filter,
        ...(archived ? { archived } : {}),
        ...this.queryCategory(category),
        ...(searchPrefix ? { searchPrefix } : {}),
      });
      await this.followBatch(pending, job);
    } catch (error) {
      this.set({ error: message(error) });
      await this.followBatch(pending);
    }
  }
  private async followBatch(pending: { jobId: string; startedAt: number }, result?: InboxReadBatch) {
    if (this.closed) return;
    const poll = (delay: number) => {
      clearTimeout(this.batchTimer);
      if (!this.closed) this.batchTimer = setTimeout(() => void this.followBatch(pending), delay);
    };
    let failure = '';
    try {
      const job =
        result ??
        (await this.options.request<InboxReadBatch>({ action: 'readBatchStatus', jobId: pending.jobId }));
      if (job?.jobId !== pending.jobId) throw new Error('INVALID_BATCH_STATUS');
      if (job.state === 'running' || job.state === 'queued') {
        poll(1500);
        return;
      }
      if (job.state !== 'complete') failure = 'INBOX_BATCH_EXPIRED';
    } catch (error) {
      const code = message(error);
      // A parallel tab can observe the local admission before the server transaction commits.
      if (code === 'INBOX_JOB_MISSING' && Date.now() - pending.startedAt < 30000) {
        poll(2000);
        return;
      }
      if (code !== 'INBOX_JOB_MISSING' && !forbidden(code)) {
        poll(5000);
        return;
      }
      failure = code;
    }
    try {
      await this.storage.update(this.options.uid, (record) => {
        if (record.batch?.jobId === pending.jobId) delete record.batch;
      });
      this.set({ batchBusy: false });
      await this.refresh();
      if (failure) this.set({ error: failure });
    } catch {
      poll(5000);
    }
  }
  async markAllRead(rows: readonly InboxItem[]) {
    const entries = [...new Map(rows.map((row) => [row.key, freezeRead(row)])).values()];
    const commands: InboxMutation[] = [];
    for (let i = 0; i < entries.length; i += 50)
      commands.push({ action: 'markAllRead', entries: entries.slice(i, i + 50) });
    await this.admit(commands);
  }
  mutate = (command: InboxMutation) =>
    this.admit([
      command.action === 'markRead' && !command.operationId
        ? { ...command, operationId: crypto.randomUUID() }
        : command,
    ]);
  private async admit(commands: InboxMutation[]) {
    if (this.closed) throw new Error('INVALID_SESSION');
    const pending = await this.storage.update(this.options.uid, (record) => {
      for (const command of commands) enqueue(record, command, crypto.randomUUID());
      return record.queue.length;
    });
    this.set({ pending });
    await this.drain();
  }
  private drain(): Promise<void> {
    if (this.running) {
      this.rerun = true;
      return this.running;
    }
    this.running = (async () => {
      do {
        this.rerun = false;
        await this.deliver();
      } while (this.rerun && !this.closed);
    })()
      .catch((error) => this.set({ error: message(error) }))
      .finally(() => {
        this.running = undefined;
      });
    return this.running;
  }
  private async deliver() {
    clearTimeout(this.retry);
    let changed = false,
      rejected = '';
    while (!this.closed) {
      const head = await this.storage.update(this.options.uid, (record) => {
        const first = record.queue[0];
        if (!first) return;
        if (!first.lease || first.lease.until <= Date.now() || first.lease.owner === this.owner)
          first.lease = { owner: this.owner, until: Date.now() + 30000 };
        return first;
      });
      if (!head || this.closed) break;
      if (head.lease?.owner !== this.owner) {
        this.retry = setTimeout(() => void this.drain(), Math.max(1, (head.lease?.until ?? 0) - Date.now()));
        break;
      }
      try {
        const result = await this.options.request<{
          item?: InboxItem;
          results?: Array<{ ok: boolean; code?: string }>;
        }>(head.command);
        if (head.command.action === 'preferences' && result?.item) {
          const item = result.item;
          this.set({
            items: this.state.items.map((v) =>
              v.key === item.key &&
              v.ref.generation === item.ref.generation &&
              (v.preferencesRevision ?? 0) <= (item.preferencesRevision ?? 0)
                ? item
                : v,
            ),
          });
        }
        const refusal = result?.results?.find((entry) => !entry.ok);
        if (refusal) rejected = refusal.code ?? 'SOURCE_ACCESS_REQUIRED';
        changed = true;
      } catch (error) {
        const reason = message(error);
        if (this.closed || ['INVALID_SESSION', 'SIGN_IN_REQUIRED', 'unauthenticated'].includes(reason)) {
          await this.storage.update(this.options.uid, (record) => {
            if (record.queue[0]?.lease?.owner === this.owner) delete record.queue[0].lease;
          });
          this.set({ items: [], hasMore: false, badge: 0, error: reason });
          break;
        }
        if (reason === 'PREFERENCE_REVISION_CONFLICT' && head.command.action === 'preferences') {
          const denied = head.command;
          const pending = await this.storage.update(this.options.uid, (record) => {
            record.queue = record.queue.filter(
              (q) =>
                q.command.action !== 'preferences' ||
                q.command.key !== denied.key ||
                q.command.generation !== denied.generation,
            );
            return record.queue.length;
          });
          this.set({ pending });
          changed = true;
          rejected = reason;
          continue;
        }
        this.set({ error: reason, offline: !forbidden(reason) });
        if (
          !forbidden(reason) &&
          !['INBOX_ITEM_MISSING', 'INVALID_INPUT', 'INVALID_READ_CURSOR'].includes(reason)
        ) {
          this.retry = setTimeout(() => void this.drain(), 5000);
          break;
        }
        // A revoked membership terminates only its rejected intent. Other frozen batch entries are idempotently retried separately.
        if (head.command.action === 'markAllRead' && head.command.entries.length > 1) {
          const entries = head.command.entries;
          await this.storage.update(this.options.uid, (record) => {
            if (record.queue[0]?.id === head.id)
              record.queue.splice(
                0,
                1,
                ...entries.map((entry) => ({
                  id: crypto.randomUUID(),
                  command: { action: 'markRead' as const, ...entry },
                })),
              );
          });
          continue;
        }
        changed = true;
        rejected = reason;
      }
      const pending = await this.storage.update(this.options.uid, (record) => {
        if (record.queue[0]?.id === head.id) record.queue.shift();
        return record.queue.length;
      });
      this.set({ pending });
    }
    if (changed && !this.closed) await this.refresh();
    if (rejected) this.set({ error: rejected });
  }
  close() {
    this.closed = true;
    this.revision++;
    clearTimeout(this.retry);
    clearTimeout(this.expiry);
    clearTimeout(this.batchTimer);
    clearTimeout(this.refreshTimer);
    this.stop?.();
    globalThis.removeEventListener?.('online', this.dirty);
    this.listeners.clear();
  }
}
