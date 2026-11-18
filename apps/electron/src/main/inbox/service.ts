import { EventEmitter } from 'node:events';
import { watchInbox } from '../../../../../packages/inbox/firestore.js';
import {
  INBOX_PAGE_SIZE,
  isUnread,
  needsAttention,
  type InboxCommand,
  type InboxItem,
  type InboxPage,
} from '../../../../../packages/inbox/contracts.js';
import type { Account, TeamsService } from '../teams/service.js';
import type { NotificationKind } from '../../shared/notificationPreferences.js';
import type { SessionRef } from '../../shared/settings.js';
import type { AccountSessionStore } from '../account.js';
import type {
  AssistantActivityPreview,
  AssistantActivityPage,
  InboxNotification,
  InboxSignal,
} from '../../shared/inbox.js';
import {
  LocalInboxStore,
  localEntry,
  ordinal,
  safeId,
  validateTarget,
  type DesktopInboxSession,
  type LocalEntry,
} from './local.js';
import { SummaryPublisher } from './publisher.js';
import { NoticeDelivery } from './notices.js';
import { NotificationDeliveryLedger } from './deliveryLedger.js';
export type { DesktopInboxSession } from './local.js';

export interface InboxOptions {
  account: Account;
  teams: Pick<TeamsService, 'firestore'>;
  userData: string;
  localSessions: () => Promise<DesktopInboxSession[]>;
  sourceCredentials: AccountSessionStore;
  activityPreview?: (ref: SessionRef, through: number) => Promise<AssistantActivityPreview>;
  activityPage?: (ref: SessionRef, after: number) => Promise<AssistantActivityPage>;
  isFocused?: () => boolean;
  notify?: (notice: InboxNotification & { key: string; generation: number }) => void;
  notificationsEnabled?: () => boolean;
  endpoint?: string;
  fetcher?: typeof fetch;
  watch?: typeof watchInbox;
}
function transient(error: unknown) {
  return (
    error instanceof TypeError ||
    (error instanceof Error &&
      ['INBOX_OFFLINE', 'INBOX_SERVICE_UNAVAILABLE', 'TimeoutError', 'AbortError'].includes(
        error.message.startsWith('INBOX_') ? error.message : error.name,
      ))
  );
}
const allowed = new Set([
  'list',
  'get',
  'notices',
  'markAllRead',
  'markAllReadSnapshot',
  'readBatchStatus',
  'markRead',
  'markUnread',
  'preferences',
  'accountPreferences',
  'peerReceipt',
  'presence',
]);
type Position = { key: string; lastActivityAt: number; pinOrder?: number | null; titleSort?: string; title?: string };
const normalizeTitle = (s: string) => s.normalize('NFKC').trim().toLowerCase();
const titleOf = (p: Position) => p.titleSort ?? normalizeTitle(p.title ?? '');
const byTitle = (a: Position, b: Position) =>
  Buffer.compare(Buffer.from(titleOf(a)), Buffer.from(titleOf(b))) || (a.key < b.key ? -1 : Number(a.key > b.key));
const byPosition = (a: Position, b: Position) =>
  (a.pinOrder ?? Number.MAX_SAFE_INTEGER) - (b.pinOrder ?? Number.MAX_SAFE_INTEGER) ||
  b.lastActivityAt - a.lastActivityAt ||
  (a.key < b.key ? -1 : Number(a.key > b.key));
function matchesArchiveView(archived: boolean, view: { archived?: boolean; filter?: string }) {
  if (view.archived === true) return archived;
  return !archived || view.filter === 'unread' || view.filter === 'attention';
}
export class InboxService {
  private readonly events = new EventEmitter();
  private readonly store: LocalInboxStore;
  private readonly publisher: SummaryPublisher;
  private readonly notices: NoticeDelivery;
  private readonly stopAccount: () => void;
  private readonly ledger: NotificationDeliveryLedger;
  private uid: string | null;
  private epoch = 0;
  private authenticationEpoch: number;
  private started = false;
  private known = new Map<string, string>();
  private localAlertTimer?: ReturnType<typeof setTimeout>;
  private alertVersion = 0;
  private localWakeAt = Infinity;
  private closed = false;
  private cloudBadge = 0;
  private readonly deviceId: string;
  private presenceActive = false;
  private presenceRevision = 0;
  private presenceChain: Promise<void> = Promise.resolve();
  private presenceTimer?: ReturnType<typeof setTimeout>;
  private stopWatch?: () => void;
  private reconnect?: ReturnType<typeof setTimeout>;
  private invalidation?: ReturnType<typeof setTimeout>;
  private publishUpdate?: ReturnType<typeof setTimeout>;
  private projection?: Promise<{ local: Map<string, InboxItem>; mirrored: Map<string, number>; badge: number }>;
  private catalogValue?: Promise<Map<string, LocalEntry>>;
  private focusedKeys = new Map<
    number,
    { sessionId: string | null; key: string | null; generation?: number; focused: () => boolean }
  >();
  constructor(private readonly opts: InboxOptions) {
    this.uid = opts.account.snapshot().user?.uid ?? null;
    this.authenticationEpoch = opts.account.sessionEpoch?.() ?? 0;
    this.store = new LocalInboxStore(opts.userData);
    this.deviceId = this.store.deviceId();
    this.ledger = new NotificationDeliveryLedger(opts.userData, () => this.uid);
    this.publisher = new SummaryPublisher(
      this.store,
      opts.sourceCredentials,
      () => ({ uid: this.uid, epoch: this.epoch }),
      () => this.catalog(),
      (uid, epoch, c) => this.post(uid, epoch, c),
      () => this.publicationChanged(),
      (e) => this.signal('error', e.message),
    );
    this.notices = new NoticeDelivery(
      (uid, epoch, c) => this.post(uid, epoch, c),
      this.ledger,
      (uid, epoch) => !this.closed && this.uid === uid && this.epoch === epoch,
      (key) => this.isKeyVisible(key),
      () => opts.notificationsEnabled?.() !== false,
      (n) => {
        if (opts.isFocused?.()) this.notifyInApp(n);
        else opts.notify?.(n);
      },
      this.deviceId,
    );
    this.stopAccount = opts.account.onChanged(() => this.syncAccount());
  }
  start() {
    if (this.started || this.closed) return;
    this.started = true;
    // Restore may finish before AccountService has a listener to notify.
    if (!this.uid)
      void this.opts.account
        .getIdToken()
        .catch(() => {})
        .then(() => {
          if (!this.closed) this.syncAccount();
        });
    this.connect();
    this.queueLocalAlerts();
    void this.publisher.request().catch(() => {});
  }
  private catalog(): Promise<Map<string, LocalEntry>> {
    if (!this.catalogValue) {
      const pending = this.opts.localSessions().then((rows) => {
        const result = new Map<string, LocalEntry>();
        for (const input of rows) {
          const entry = localEntry(input);
          if (entry) result.set(entry.item.key, entry);
        }
        if (this.catalogValue === pending)
          this.known = new Map([...result.values()].map((e) => [e.ref.sessionId, e.item.key]));
        return result;
      });
      this.catalogValue = pending;
      void pending.catch(() => {
        if (this.catalogValue === pending) this.catalogValue = undefined;
      });
    }
    return this.catalogValue;
  }
  invalidateLocal() {
    if (this.closed) return;
    this.catalogValue = undefined;
    this.projection = undefined;
    if (this.invalidation) return;
    this.invalidation = setTimeout(() => {
      this.invalidation = undefined;
      this.changed();
      void this.publisher.request().catch(() => {});
    }, 200);
    this.invalidation.unref?.();
  }
  private syncAccount() {
    const uid = this.opts.account.snapshot().user?.uid ?? null,
      auth = this.opts.account.sessionEpoch?.() ?? 0;
    if (uid === this.uid && auth === this.authenticationEpoch) return;
    this.signal('error', 'INVALID_SESSION');
    this.uid = uid;
    this.authenticationEpoch = auth;
    this.epoch++;
    this.stopWatch?.();
    this.stopWatch = undefined;
    clearTimeout(this.reconnect);
    this.focusedKeys.clear();
    this.presenceRevision++;
    clearTimeout(this.presenceTimer);
    this.presenceActive = false;
    this.projection = undefined;
    this.cloudBadge = 0;
    this.notices.reset();
    if (this.started && !this.closed) {
      this.connect();
      this.changed();
      void this.publisher.request().catch(() => {});
    }
  }
  private check(uid: string | null, epoch = this.epoch) {
    this.syncAccount();
    if (this.closed || uid !== this.uid || epoch !== this.epoch) throw new Error('INVALID_SESSION');
  }
  private connect() {
    const uid = this.uid,
      epoch = this.epoch;
    if (!uid) return;
    const error = (err: Error) => {
      if (this.closed || uid !== this.uid || epoch !== this.epoch) return;
      this.stopWatch?.();
      this.stopWatch = undefined;
      const code = (err as Error & { code?: string }).code;
      this.signal(
        'error',
        code === 'permission-denied'
          ? 'SOURCE_ACCESS_REQUIRED'
          : code === 'unauthenticated'
            ? 'INVALID_SESSION'
            : err.message,
      );
      clearTimeout(this.reconnect);
      this.reconnect = setTimeout(() => this.connect(), 5000);
      this.reconnect.unref?.();
    };
    void Promise.all([this.opts.teams.firestore(uid), import('firebase/firestore')])
      .then(([{ db }, sdk]) => {
        this.check(uid, epoch);
        this.stopWatch = (this.opts.watch ?? watchInbox)(
          sdk,
          db,
          uid,
          () => {
            if (this.closed || epoch !== this.epoch) return;
            this.signal('dirty');
            if (this.opts.notify) this.notices.changed(uid, epoch);
          },
          (count) => {
            if (this.closed || epoch !== this.epoch) return;
            this.cloudBadge = count;
            this.emitBadge();
          },
          error,
        );
      })
      .catch(error);
  }
  private signal(type: InboxSignal['type'], error?: string, count?: number) {
    if (!this.closed) this.events.emit('signal', { uid: this.uid, type, error, count } satisfies InboxSignal);
  }
  private publicationChanged() {
    this.projection = undefined;
    if (this.closed || this.publishUpdate) return;
    this.publishUpdate = setTimeout(() => {
      this.publishUpdate = undefined;
      if (!this.closed) this.changed();
    }, 500);
    this.publishUpdate.unref?.();
  }
  private changed() {
    this.projection = undefined;
    this.signal('dirty');
    this.emitBadge();
    this.queueLocalAlerts();
  }
  hasLocalSource(sessionId: string) {
    return this.known.has(sessionId);
  }
  allowsNotifications(sessionId: string, kind?: NotificationKind) {
    if (this.closed || !this.store.allowsNotifications(JSON.stringify(this.uid), sessionId)) return false;
    const key = this.known.get(sessionId);
    if (
      key &&
      this.uid &&
      (kind === 'agent_needs_input' || kind === 'permission_prompt') &&
      this.store.enabled(this.uid) &&
      !this.store.privateChatKeys(this.uid).has(key)
    ) {
      const source = key.split(':')[1];
      const generation = this.store.sources(this.uid).find((b) => b.source === source && b.enabled)?.generation;
      if (generation && this.store.publication(this.uid, key)?.generation === generation) return false;
    }
    return true;
  }
  private queueLocalAlerts(delay = 150) {
    if (this.closed || !this.started || !this.opts.notify) return;
    const next = Date.now() + delay;
    if (this.localAlertTimer && this.localWakeAt <= next) return;
    clearTimeout(this.localAlertTimer);
    this.localWakeAt = next;
    const version = ++this.alertVersion;
    this.localAlertTimer = setTimeout(() => {
      this.localAlertTimer = undefined;
      this.localWakeAt = Infinity;
      void this.checkLocalAlerts(version).catch(() => {});
    }, delay);
    this.localAlertTimer.unref?.();
  }
  private async checkLocalAlerts(version: number) {
    const uid = this.uid,
      epoch = this.epoch;
    const [entries, projection] = await Promise.all([this.catalog(), this.localProjection()]);
    if (
      this.closed ||
      version !== this.alertVersion ||
      this.epoch !== epoch ||
      this.uid !== uid ||
      this.opts.notificationsEnabled?.() === false
    )
      return;
    const now = Date.now();
    let due = Infinity;
    for (const [key, row] of projection.local) {
      const entry = entries.get(key);
      if (!entry || row.archived || row.notificationMode === 'none') continue;
      const failure =
        entry.item.attentionKind === 'failure' &&
        ordinal(entry.summary.failureAt) &&
        entry.summary.failureAt > 0 &&
        ordinal(entry.summary.failureRevision) &&
        entry.summary.failureRevision > 0;
      if (failure && this.store.failureObserved(JSON.stringify(uid), key, entry.summary.failureRevision)) continue;
      if (!failure && (!row.unreadCount || !entry.summary.previewKind)) continue;
      const eventAt = failure ? entry.summary.failureAt : entry.summary.lastActivityAt;
      if (!eventAt || eventAt < now - 3600000 || eventAt > now + 60000) continue;
      const id = failure
        ? 'failure:' + key + ':' + entry.summary.failureRevision
        : 'source:' + key + ':' + row.sourceCursor;
      if (this.ledger.has(id, now)) continue;
      const at = eventAt + (failure || entry.summary.previewKind === 'failure' ? 0 : 5000);
      if (at > now) {
        due = Math.min(due, at);
        continue;
      }
      if (this.isKeyVisible(key) || this.isVisible(entry.ref.sessionId)) continue;
      const notice = {
        key,
        generation: row.ref.generation,
        ref: entry.ref,
        ...(!failure || entry.summary.previewKind === 'failure' ? { through: row.sourceCursor } : {}),
        title: 'LingXi',
        body: 'Conversation update',
      };
      if (this.opts.isFocused?.()) this.notifyInApp(notice);
      else this.opts.notify?.(notice);
      const delivered =
        failure && entry.summary.previewKind === 'failure' ? [id, 'source:' + key + ':' + row.sourceCursor] : [id];
      this.ledger.remember(delivered, now);
    }
    if (Number.isFinite(due)) this.queueLocalAlerts(Math.max(1, due - Date.now()));
  }
  private emitBadge() {
    const uid = this.uid,
      epoch = this.epoch;
    void this.badge(uid)
      .then((n) => {
        if (!this.closed && epoch === this.epoch) this.signal('badge', undefined, n);
      })
      .catch(() => {});
  }
  onSignal(fn: (signal: InboxSignal) => void) {
    this.events.on('signal', fn);
    return () => {
      this.events.off('signal', fn);
    };
  }
  onNotification(fn: (notice: InboxNotification) => void) {
    this.events.on('notice', fn);
    return () => {
      this.events.off('notice', fn);
    };
  }
  notifyInApp(notice: InboxNotification) {
    if (!this.closed) this.events.emit('notice', notice);
  }
  setVisible(
    sender: number,
    sessionId: string | null,
    key: string | null,
    focused: () => boolean,
    generation?: number,
  ) {
    if (
      (sessionId !== null && !safeId(sessionId)) ||
      (key !== null &&
        (typeof key !== 'string' || !/^(team|assistant|service):[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{1,128}$/u.test(key)))
    )
      throw new Error('INVALID_INPUT');
    if (generation !== undefined && (!ordinal(generation) || !generation)) throw new Error('INVALID_INPUT');
    if (!sessionId && !key) this.focusedKeys.delete(sender);
    else this.focusedKeys.set(sender, { sessionId, key, generation, focused });
    this.focusChanged();
  }
  isVisible(sessionId: string) {
    return [...this.focusedKeys.values()].some((v) => v.sessionId === sessionId && v.focused());
  }
  private isKeyVisible(key: string) {
    return [...this.focusedKeys.values()].some((v) => v.key === key && v.focused());
  }
  release(sender: number) {
    if (this.focusedKeys.delete(sender)) this.focusChanged();
  }
  focusChanged() {
    this.queueLocalAlerts();
    clearTimeout(this.presenceTimer);
    const revision = ++this.presenceRevision;
    this.presenceChain = this.presenceChain
      .catch(() => {})
      .then(async () => {
        const uid = this.uid,
          epoch = this.epoch;
        if (this.closed || !uid || revision !== this.presenceRevision) return;
        const visible = [...this.focusedKeys.values()].find((v) => v.key && v.focused());
        let key = visible?.key ?? null,
          generation = visible?.generation;
        try {
          if (key?.startsWith('assistant:')) {
            const projection = await this.localProjection();
            if (projection.local.has(key)) key = null;
            else generation ??= projection.mirrored.get(key);
          }
          if (key && !generation) {
            const response = await this.post<{ item: InboxItem }>(uid, epoch, { action: 'get', key });
            generation = response.item.ref.generation;
          }
          if ((!key && !this.presenceActive) || revision !== this.presenceRevision) return;
          this.presenceActive = !!key;
          await this.post(uid, epoch, {
            action: 'presence',
            deviceId: this.deviceId,
            key,
            ...(key ? { generation } : {}),
            foreground: !!key,
          });
        } finally {
          if (!this.closed && uid === this.uid && revision === this.presenceRevision && key) {
            this.presenceTimer = setTimeout(() => this.focusChanged(), 30000);
            this.presenceTimer.unref?.();
          }
        }
      })
      .catch(() => {});
  }
  async localRef(key: string) {
    if (typeof key !== 'string' || key.length > 300) throw new Error('INVALID_INPUT');
    return (await this.catalog()).get(key)?.ref ?? null;
  }
  async activityPage(key: string, after: number) {
    if (!ordinal(after)) throw new Error('INVALID_INPUT');
    const epoch = this.epoch,
      uid = this.uid,
      ref = await this.localRef(key);
    this.check(uid, epoch);
    if (!ref || !this.opts.activityPage) throw new Error('SOURCE_ACCESS_REQUIRED');
    const result = await this.opts.activityPage(ref, after);
    this.check(uid, epoch);
    return result;
  }
  async activityPreview(key: string, through: number): Promise<AssistantActivityPreview> {
    const unavailable: AssistantActivityPreview = { available: false, messages: [] };
    if (!ordinal(through) || typeof key !== 'string' || key.length > 300) throw new Error('INVALID_INPUT');
    const uid = this.uid,
      epoch = this.epoch;
    const entry = (await this.catalog()).get(key);
    this.check(uid, epoch);
    if (!entry) throw new Error('SOURCE_ACCESS_REQUIRED');
    if (!this.opts.activityPreview || through <= entry.item.readThrough || through > entry.item.sourceCursor)
      return unavailable;
    const result = await this.opts.activityPreview(entry.ref, through);
    this.check(uid, epoch);
    if (!result.available || !Array.isArray(result.messages) || !result.messages.length || result.messages.length > 16)
      return unavailable;
    let blocks = 0,
      characters = 0;
    for (const message of result.messages) {
      if (
        message.role !== 'assistant' ||
        !Array.isArray(message.blocks) ||
        !message.blocks.length ||
        (blocks += message.blocks.length) > 16
      )
        return unavailable;
      for (const block of message.blocks)
        if (block.type !== 'text' || typeof block.text !== 'string' || (characters += block.text.length) > 32768)
          return unavailable;
    }
    return Buffer.byteLength(JSON.stringify(result.messages)) <= 256 * 1024 ? result : unavailable;
  }
  async conversationSummarySync(uid: string, key: string) {
    this.check(uid);
    const epoch = this.epoch;
    if (!uid || typeof key !== 'string' || !key.startsWith('assistant:') || !(await this.localRef(key)))
      throw new Error('SOURCE_ACCESS_REQUIRED');
    this.check(uid, epoch);
    return this.store.chatConsent(uid, key).enabled;
  }
  async setConversationSummarySync(uid: string, key: string, enabled: boolean) {
    const epoch = this.epoch;
    if (typeof enabled !== 'boolean') throw new Error('INVALID_INPUT');
    await this.conversationSummarySync(uid, key);
    this.check(uid, epoch);
    this.store.setChatConsent(uid, key, enabled);
    this.changed();
    await this.publisher.consentChanged();
  }
  summarySync(uid: string) {
    this.check(uid);
    return this.store.enabled(uid);
  }
  async setSummarySync(uid: string, enabled: boolean) {
    this.check(uid);
    if (typeof uid !== 'string' || !uid || typeof enabled !== 'boolean') throw new Error('INVALID_INPUT');
    if (enabled && !this.opts.sourceCredentials.persistent) throw new Error('SECURE_STORAGE_REQUIRED');
    this.store.setEnabled(uid, enabled);
    this.changed();
    await this.publisher.consentChanged();
  }
  private async post<T>(uid: string, epoch: number, command: object, refreshed = false): Promise<T> {
    this.check(uid, epoch);
    const token = await this.opts.account.getIdToken(refreshed);
    this.check(uid, epoch);
    if ('action' in command && command.action === 'assistantSource') {
      const bind = command as { action: 'assistantSource'; enabled: boolean; consentRevision: number };
      if (this.store.enabled(uid) !== bind.enabled || this.store.consentRevision(uid) !== bind.consentRevision)
        throw new Error('STALE_CONSENT');
    }
    // Credentials can refresh asynchronously while the owner revokes a pending publication.
    if ('action' in command && ['assistantPublish', 'assistantRemove'].includes(String(command.action))) {
      const request = command as {
        action: string;
        sourceId: string;
        conversationId: string;
        conversationConsentRevision: number;
      };
      const chat = this.store.chatConsent(uid, 'assistant:' + request.sourceId + ':' + request.conversationId);
      if (
        !this.store.enabled(uid) ||
        chat.revision !== request.conversationConsentRevision ||
        chat.enabled !== (request.action === 'assistantPublish')
      )
        throw new Error('STALE_CONSENT');
    }
    const response = await (this.opts.fetcher ?? fetch)(this.opts.endpoint ?? 'https://lingxios.net/v1/inbox', {
      method: 'POST',
      credentials: 'omit',
      redirect: 'error',
      cache: 'no-store',
      signal: AbortSignal.timeout(30_000),
      headers: { Authorization: 'Bearer ' + token, 'Content-Type': 'application/json' },
      body: JSON.stringify(command),
    });
    this.check(uid, epoch);
    if (response.status === 429 || response.status >= 500) throw new Error('INBOX_OFFLINE');
    if (response.status === 401 && !refreshed) return this.post<T>(uid, epoch, command, true);
    const value = await response.json().catch(() => ({}));
    this.check(uid, epoch);
    // An older deployment has no Inbox route. Preserve the authorized local
    // projection while cloud service is unavailable; this is not a login failure.
    if (response.status === 404 && (!value.error || value.error === 'NOT_FOUND' || value.error?.code === 'NOT_FOUND'))
      throw new Error('INBOX_SERVICE_UNAVAILABLE');
    if (!response.ok)
      throw new Error(
        typeof value.error === 'string'
          ? value.error
          : (value.error?.code ?? (response.status === 401 ? 'INVALID_SESSION' : 'INBOX_REQUEST_FAILED')),
      );
    return value;
  }
  private localProjection() {
    const uid = this.uid,
      scope = JSON.stringify(uid),
      epoch = this.epoch;
    return (this.projection ??= this.catalog()
      .then((entries) => {
        this.check(uid, epoch);
        const bindings = new Map(
          (uid && this.store.enabled(uid) ? this.store.sources(uid) : [])
            .filter((b) => b.enabled)
            .map((b) => [b.source, b.generation]),
        );
        this.store.observeArchives(scope, entries.values());
        const personal = this.store.personalRows(scope);
        const publication = uid && bindings.size ? this.store.publications(uid) : new Map<string, number>();
        const disabled = uid ? this.store.privateChatKeys(uid) : new Set<string>();
        const local = new Map<string, InboxItem>(),
          mirrored = new Map<string, number>();
        let badge = 0;
        for (const [key, entry] of entries) {
          const generation = bindings.get(entry.item.ref.sourceId);
          if (uid && generation && !disabled.has(key) && publication.get(key) === generation) {
            mirrored.set(key, generation);
            continue;
          }
          const row = this.store.row(scope, entry, personal);
          local.set(key, row);
          if (!row.archived && (isUnread(row) || needsAttention(row))) badge++;
        }
        return { local, mirrored, badge };
      })
      .catch((error) => {
        this.projection = undefined;
        throw error;
      }));
  }
  async badge(uid: string | null) {
    this.check(uid);
    const epoch = this.epoch;
    const local = await this.localProjection();
    this.check(uid, epoch);
    return this.cloudBadge + local.badge;
  }
  private async snapshotBatch(
    uid: string | null,
    epoch: number,
    c: Extract<InboxCommand, { action: 'markAllReadSnapshot' | 'readBatchStatus' }>,
  ) {
    const scope = JSON.stringify(uid),
      starting = c.action === 'markAllReadSnapshot',
      id = starting ? c.operationId : c.jobId;
    if (!safeId(id)) throw new Error('INVALID_INPUT');
    if (starting) {
      const now = Date.now();
      if (
        !this.store.batch(scope, id) &&
        (!ordinal(c.requestedAt) || c.requestedAt < now - 300000 || c.requestedAt > now + 60000)
      )
        throw new Error('BATCH_REQUEST_EXPIRED');
      if (
        (c.searchPrefix !== undefined && (typeof c.searchPrefix !== 'string' || c.searchPrefix.length > 120)) ||
        (c.filter !== undefined && !['all', 'unread', 'attention'].includes(c.filter)) ||
        (c.category !== undefined && !['assistant', 'team', 'friend', 'service'].includes(c.category)) ||
        (c.archived !== undefined && typeof c.archived !== 'boolean')
      )
        throw new Error('INVALID_INPUT');
      const prefix = normalizeTitle(c.searchPrefix ?? '');
      const signature = JSON.stringify([c.category ?? null, c.filter ?? 'all', c.archived ?? null, prefix]);
      const projection = await this.localProjection();
      this.check(uid, epoch);
      // Capture only this host's private rows. The server freezes all published rows, including local aliases.
      this.store.freezeBatch(
        scope,
        id,
        signature,
        [...projection.local.values()].filter(
          (row) =>
            isUnread(row) &&
            (c.archived === undefined || matchesArchiveView(row.archived, c)) &&
            (!c.category || c.category === row.category) &&
            (!prefix || titleOf(row).startsWith(prefix)) &&
            (c.filter !== 'attention' || needsAttention(row)),
        ),
      );
    }
    const cloud = uid
      ? await this.post<{ jobId: string; state: string; processed: number; changed?: number }>(uid, epoch, c)
      : { jobId: id, state: 'complete', processed: 0, changed: 0 };
    this.check(uid, epoch);
    let batch = this.store.batch(scope, id);
    if (!batch) {
      if (!uid) throw new Error('INBOX_JOB_MISSING');
      return cloud;
    }
    if (cloud.state === 'expired') return cloud;
    const entries = await this.catalog();
    this.check(uid, epoch);
    const previous = batch.processed;
    while (batch.processed < batch.total) {
      batch = this.store.applyBatchPage(scope, id, entries, uid);
      if (batch.processed < batch.total) {
        await new Promise<void>((resolve) => setImmediate(resolve));
        this.check(uid, epoch);
      }
    }
    if (batch.processed !== previous) {
      this.changed();
      if (uid && this.store.enabled(uid)) void this.publisher.request().catch(() => {});
    }
    return {
      ...cloud,
      processed: (cloud.processed ?? 0) + batch.processed,
      changed: (cloud.changed ?? 0) + batch.changed,
    };
  }
  async request<T>(uid: string | null, c: InboxCommand): Promise<T> {
    this.check(uid);
    if (!c || !allowed.has(c.action) || JSON.stringify(c).length > 64000) throw new Error('INVALID_INPUT');
    const epoch = this.epoch;
    if (c.action === 'markAllReadSnapshot' || c.action === 'readBatchStatus')
      return (await this.snapshotBatch(uid, epoch, c)) as T;
    if (c.action === 'list') return (await this.list(uid, epoch, c)) as T;
    if (c.action === 'markAllRead') {
      if (!Array.isArray(c.entries) || !c.entries.length || c.entries.length > 50) throw new Error('INVALID_INPUT');
      const commands = c.entries.map((e) => ({ ...e, action: 'markRead' as const }));
      commands.forEach(validateTarget);
      const projection = await this.localProjection();
      this.check(uid, epoch);
      const local = commands.filter((cmd) => projection.local.has(cmd.key)),
        remote = c.entries.filter((e) => !projection.local.has(e.key));
      const results: { key: string; ok: boolean; code?: string }[] = [];
      if (remote.length && uid)
        results.push(
          ...(await this.post<{ results: typeof results }>(uid, epoch, { action: 'markAllRead', entries: remote }))
            .results,
        );
      const entries = await this.catalog();
      this.check(uid, epoch);
      for (const cmd of local) {
        try {
          const entry = entries.get(cmd.key);
          if (!entry) throw new Error('SOURCE_ACCESS_REQUIRED');
          this.store.mutate(JSON.stringify(uid), entry, cmd);
          results.push({ key: cmd.key, ok: true });
        } catch (e) {
          if (
            !(e instanceof Error) ||
            !['SOURCE_ACCESS_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_READ_CURSOR'].includes(e.message)
          )
            throw e;
          results.push({ key: cmd.key, ok: false, code: e.message });
        }
      }
      if (local.length) this.changed();
      if (!uid) for (const e of remote) results.push({ key: e.key, ok: false, code: 'SOURCE_ACCESS_REQUIRED' });
      return { results } as T;
    }
    if (c.action === 'markRead' || c.action === 'markUnread' || c.action === 'preferences') validateTarget(c);
    if ('key' in c && c.key?.startsWith('assistant:')) {
      const key = c.key;
      const projection = await this.localProjection();
      this.check(uid, epoch);
      const item = projection.local.get(key);
      if (item) {
        if ('generation' in c && c.generation !== undefined && c.generation !== item.ref.generation)
          throw new Error('STALE_MEMBERSHIP');
        if (c.action === 'get') return { item } as T;
        if (c.action === 'presence') return { ok: true } as T;
        if (c.action === 'peerReceipt') return { readThrough: null } as T;
        if (c.action === 'markRead' || c.action === 'markUnread' || c.action === 'preferences') {
          const entry = (await this.catalog()).get(key);
          this.check(uid, epoch);
          if (!entry) throw new Error('INBOX_ITEM_MISSING');
          const changed = this.store.mutate(JSON.stringify(uid), entry, c);
          this.changed();
          return { item: changed } as T;
        }
      }
    }
    if (!uid) {
      if (c.action === 'notices') return { items: [], next: null } as T;
      if (c.action === 'accountPreferences' && c.dmReceipts === undefined) return { dmReceipts: false } as T;
      throw new Error('SIGN_IN_REQUIRED');
    }
    let result: T;
    try {
      result = await this.post<T>(uid, epoch, c);
    } catch (error) {
      if (c.action !== 'get' || !transient(error)) throw error;
      const entry = (await this.catalog()).get(c.key);
      this.check(uid, epoch);
      const generation = entry && (await this.localProjection()).mirrored.get(c.key);
      this.check(uid, epoch);
      if (!entry || !generation) throw error;
      if (c.generation !== undefined && c.generation !== generation) throw new Error('STALE_MEMBERSHIP');
      const item = this.store.row(JSON.stringify(uid), entry);
      return { item: { ...item, ref: { ...item.ref, generation } } } as T;
    }
    if (
      'key' in c &&
      typeof c.key === 'string' &&
      c.key.startsWith('assistant:') &&
      result &&
      typeof result === 'object' &&
      'item' in result
    ) {
      const projection = await this.localProjection();
      this.check(uid, epoch);
      if (projection.mirrored.has(c.key)) {
        this.store.remember(JSON.stringify(uid), result.item as InboxItem);
        if (c.action === 'markRead' && c.observedFailureRevision) {
          const entry = (await this.catalog()).get(c.key);
          this.check(uid, epoch);
          if (entry) this.store.observeFailure(JSON.stringify(uid), entry, c.observedFailureRevision);
        }
      }
    }
    return result;
  }
  private async list(uid: string | null, epoch: number, c: Extract<InboxCommand, { action: 'list' }>) {
    if (c.after && (!ordinal(c.after.lastActivityAt) || typeof c.after.key !== 'string' || c.after.key.length > 300))
      throw new Error('INVALID_INPUT');
    if (c.searchPrefix !== undefined && (typeof c.searchPrefix !== 'string' || c.searchPrefix.length > 120))
      throw new Error('INVALID_INPUT');
    const prefix = normalizeTitle(c.searchPrefix ?? ''),
      compare = prefix ? byTitle : byPosition;
    let partialOffline = false, cloudUnavailable = false;
    const cloudPage = uid
      ? this.post<InboxPage>(uid, epoch, c).catch((error) => {
          if (!transient(error)) throw error;
          partialOffline = true;
          cloudUnavailable = error instanceof Error && error.message === 'INBOX_SERVICE_UNAVAILABLE';
          return { items: [], next: null };
        })
      : { items: [], next: null };
    const [projection, cloud, entries] = await Promise.all([this.localProjection(), cloudPage, this.catalog()]);
    this.check(uid, epoch);
    const rows = new Map<string, InboxItem>();
    for (const item of cloud.items) {
      const entry = entries.get(item.key);
      if (entry) {
        if (!projection.mirrored.has(item.key)) continue;
        this.store.remember(JSON.stringify(uid), item);
        rows.set(item.key, prefix ? item : { ...item, title: entry.item.title });
      } else rows.set(item.key, item);
    }
    const localRows = partialOffline ? new Map(projection.local) : projection.local;
    if (partialOffline) {
      const scope = JSON.stringify(uid),
        personal = this.store.personalRows(scope);
      for (const [key, generation] of projection.mirrored) {
        const entry = entries.get(key);
        if (!entry) continue;
        const item = this.store.row(scope, entry, personal);
        localRows.set(key, { ...item, ref: { ...item.ref, generation } });
      }
    }
    for (const row of localRows.values()) {
      if (
        !matchesArchiveView(row.archived, c) ||
        (prefix && !titleOf(row).startsWith(prefix)) ||
        (c.category && c.category !== row.category) ||
        (c.filter === 'unread' && !isUnread(row)) ||
        (c.filter === 'attention' && !needsAttention(row))
      )
        continue;
      if (c.after && compare(row, c.after) <= 0) continue;
      // A cloud page can be sparse after membership validation. Never jump over its scanned boundary.
      if (cloud.next && compare(row, cloud.next) > 0) continue;
      rows.set(row.key, row);
    }
    const sorted = [...rows.values()].sort(compare),
      items = sorted.slice(0, INBOX_PAGE_SIZE),
      last = items.at(-1);
    const next =
      sorted.length > INBOX_PAGE_SIZE && last
        ? {
            lastActivityAt: last.lastActivityAt,
            key: last.key,
            ...(prefix ? { titleSort: titleOf(last) } : { pinOrder: last.pinOrder }),
          }
        : (cloud.next ?? null);
    return { items, next, ...(partialOffline ? { partialOffline: true } : {}), ...(cloudUnavailable ? { cloudUnavailable: true } : {}) };
  }
  close() {
    this.closed = true;
    this.epoch++;
    this.alertVersion++;
    clearTimeout(this.localAlertTimer);
    this.presenceRevision++;
    clearTimeout(this.presenceTimer);
    clearTimeout(this.reconnect);
    clearTimeout(this.invalidation);
    clearTimeout(this.publishUpdate);
    this.stopAccount();
    this.stopWatch?.();
    this.publisher.close();
    this.notices.reset();
    this.ledger.close();
    this.store.close();
    this.events.removeAllListeners();
    this.focusedKeys.clear();
  }
}
