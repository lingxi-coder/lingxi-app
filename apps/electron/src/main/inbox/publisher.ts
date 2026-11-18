import { createHash, createHmac, randomBytes } from 'node:crypto';
import type { InboxItem, InboxPreferences } from '../../../../../packages/inbox/contracts.js';
import type { AccountSessionStore } from '../account.js';
import type { LocalEntry, LocalInboxStore } from './local.js';

export type CloudRequest = <T>(uid: string, epoch: number, command: object) => Promise<T>;

/** A committed runtime snapshot is the retry source. Checkpoints advance only after server acceptance. */
export class SummaryPublisher {
  private master?: Promise<string>;
  private active?: Promise<void>;
  private dirty = false;
  private stopped = false;
  private consentEpoch = 0;
  private retry?: ReturnType<typeof setTimeout>;
  constructor(
    private readonly store: LocalInboxStore,
    private readonly vault: AccountSessionStore,
    private readonly context: () => { uid: string | null; epoch: number },
    private readonly catalog: () => Promise<Map<string, LocalEntry>>,
    private readonly post: CloudRequest,
    private readonly changed: () => void,
    private readonly failed: (error: Error) => void,
  ) {}
  consentChanged() {
    this.consentEpoch++;
    return this.request();
  }
  request(): Promise<void> {
    clearTimeout(this.retry);
    this.dirty = true;
    return (this.active ??= this.run().finally(() => {
      this.active = undefined;
    }));
  }
  private async run() {
    while (this.dirty && !this.stopped) {
      this.dirty = false;
      const { uid, epoch } = this.context();
      if (!uid) continue;
      try {
        await this.sync(uid, epoch);
      } catch (error) {
        if (this.stopped) return;
        if (this.context().epoch !== epoch || this.dirty) continue;
        this.failed(error instanceof Error ? error : new Error('INBOX_REQUEST_FAILED'));
        this.retry = setTimeout(() => void this.request().catch(() => {}), 30_000);
        this.retry.unref?.();
        throw error;
      }
    }
  }
  private secret(uid: string, source: string): Promise<string> {
    if (!this.vault.persistent) return Promise.reject(new Error('SECURE_STORAGE_REQUIRED'));
    this.master ??= (async () => {
      let value = await this.vault.read();
      if (value === null) {
        value = randomBytes(32).toString('base64url');
        await this.vault.write(value);
      }
      if (!/^[A-Za-z0-9_-]{43}$/u.test(value)) throw new Error('INVALID_SOURCE_CREDENTIAL');
      return value;
    })().catch((error) => {
      this.master = undefined;
      throw error;
    });
    return this.master.then((key) =>
      createHmac('sha256', key)
        .update(JSON.stringify([uid, source]))
        .digest('base64url'),
    );
  }
  private async preferences(
    uid: string,
    epoch: number,
    key: string,
    generation: number,
    expectedPreferencesRevision: number,
    patch: Partial<InboxPreferences>,
  ) {
    let result: { item?: InboxItem };
    try {
      result = await this.post(uid, epoch, {
        action: 'preferences',
        key,
        generation,
        expectedPreferencesRevision,
        patch,
      });
    } catch (error) {
      if (!(error instanceof Error) || error.message !== 'PREFERENCE_REVISION_CONFLICT') throw error;
      // The newer profile is authoritative; replaying a migration against its new revision would clobber an owner's edit.
      result = await this.post(uid, epoch, { action: 'get', key, generation });
    }
    if (result.item && !this.stopped && this.context().epoch === epoch)
      this.store.remember(JSON.stringify(uid), result.item);
  }
  private async syncPromotedRead(uid: string, epoch: number, key: string, generation: number, ceiling: number) {
    const through = this.store.promotedRead(uid, key);
    if (!through || through > ceiling) return;
    const operationId = createHash('sha256')
      .update(JSON.stringify(['promoted', key, generation, through]))
      .digest('hex');
    await this.post(uid, epoch, { action: 'markRead', key, generation, through, manualUnreadRevision: 0, operationId });
    if (!this.stopped && this.context().epoch === epoch) this.store.promotedReadSent(uid, key, through);
  }
  private async sync(uid: string, epoch: number) {
    const consentEpoch = this.consentEpoch;
    const enabled = this.store.enabled(uid),
      consentRevision = this.store.consentRevision(uid);
    const bindings = new Map(this.store.sources(uid).map((s) => [s.source, s]));
    if (!enabled && ![...bindings.values()].some((b) => b.enabled)) return;
    const check = () => {
      if (
        this.stopped ||
        this.consentEpoch !== consentEpoch ||
        this.context().epoch !== epoch ||
        this.store.enabled(uid) !== enabled ||
        this.store.consentRevision(uid) !== consentRevision
      )
        throw new Error('INVALID_SESSION');
    };
    const entries = enabled ? await this.catalog() : new Map<string, LocalEntry>();
    check();
    const scope = JSON.stringify(uid);
    this.store.observeArchives(scope, entries.values());
    const legacy = this.store.pendingArchives(scope);
    const disabled = this.store.disabledChats(uid);
    const sources = new Set([...bindings.keys(), ...[...entries.values()].map((e) => e.item.ref.sourceId)]);
    for (const sourceId of sources) {
      check();
      const sourceSecret = await this.secret(uid, sourceId);
      check();
      let binding = bindings.get(sourceId);
      if (
        !binding ||
        Boolean(binding.enabled) !== enabled ||
        !binding.generation ||
        this.store.confirmedRevision(uid, sourceId) !== consentRevision
      ) {
        // Record intent prior to the network so a crash after bind can still revoke it.
        if (!binding) this.store.bind(uid, sourceId, 0, true);
        const result = await this.post<{ generation: number; enabled: boolean }>(uid, epoch, {
          action: 'assistantSource',
          consentRevision,
          sourceId,
          sourceSecret,
          enabled,
        });
        if (this.stopped || this.context().epoch !== epoch) throw new Error('INVALID_SESSION');
        this.store.bind(uid, sourceId, result.generation, result.enabled);
        this.store.confirm(uid, sourceId, consentRevision);
        binding = { source: sourceId, generation: result.generation, enabled: Number(result.enabled) };
        this.changed();
      }
      if (!enabled) continue;
      const generation = binding.generation;
      // Explicit removals persist even if the source catalog later omits the session.
      for (const [key, revision] of disabled) {
        const [, source, conversationId] = key.split(':');
        if (
          source !== sourceId ||
          !this.store.mayHavePublished(uid, key, generation) ||
          this.store.confirmedChat(uid, key, generation, revision)
        )
          continue;
        check();
        if (this.store.chatConsent(uid, key).revision !== revision) continue;
        const sourceRevision = Math.max(
          this.store.publication(uid, key)?.revision ?? 0,
          entries.get(key)?.item.sourceRevision ?? 0,
        );
        await this.post(uid, epoch, {
          action: 'assistantRemove',
          sourceId,
          sourceSecret,
          conversationId,
          generation,
          sourceRevision,
          conversationConsentRevision: revision,
        });
        check();
        this.store.confirmChat(uid, key, generation, revision, true);
        this.changed();
      }
      for (const entry of entries.values()) {
        if (entry.item.ref.sourceId !== sourceId) continue;
        check();
        const { key } = entry.item;
        const chat = this.store.chatConsent(uid, key);
        if (!chat.enabled) continue;
        const checkChat = () => {
          check();
          const current = this.store.chatConsent(uid, key);
          if (!current.enabled || current.revision !== chat.revision) throw new Error('STALE_CONSENT');
        };
        const signature = createHash('sha256').update(JSON.stringify(entry.summary)).digest('hex');
        const prev = this.store.publication(uid, key);
        if (prev?.generation === generation && legacy.has(key)) {
          const archived = legacy.get(key)!;
          await this.preferences(uid, epoch, key, generation, this.store.archiveExpected(scope, key), { archived });
          check();
          this.store.archiveSynced(scope, key, archived);
        }
        if (
          prev?.generation === generation &&
          prev.signature === signature &&
          this.store.confirmedChat(uid, key, generation, chat.revision)
        ) {
          await this.syncPromotedRead(uid, epoch, key, generation, entry.item.sourceCursor);
          continue;
        }
        checkChat();
        const revision = Math.max(entry.item.sourceRevision, prev?.generation === generation ? prev.revision + 1 : 1);
        this.store.attempt(uid, key, generation);
        await this.post(uid, epoch, {
          action: 'assistantPublish',
          conversationConsentRevision: chat.revision,
          ...entry.summary,
          sourceRevision: revision,
          sourceSecret,
          generation,
        });
        checkChat();
        if (
          prev?.generation !== generation ||
          (chat.revision > 1 && !this.store.confirmedChat(uid, key, generation, chat.revision))
        ) {
          const personal = this.store.row(JSON.stringify(uid), entry);
          if (personal.readThrough > entry.summary.baseline)
            await this.post(uid, epoch, {
              action: 'markRead',
              key,
              generation,
              through: personal.readThrough,
              manualUnreadRevision: 0,
            });
          if (personal.manualUnread)
            await this.post(uid, epoch, {
              action: 'markUnread',
              key,
              generation,
              operationId: 'migration-' + generation + '-' + personal.manualUnreadRevision,
            });
          if (personal.archived || personal.pinOrder !== null || personal.notificationMode !== 'mentions') {
            await this.preferences(uid, epoch, key, generation, 0, {
              archived: personal.archived,
              pinOrder: personal.pinOrder,
              notificationMode: personal.notificationMode,
            });
          }
        }
        checkChat();
        this.store.published(uid, key, generation, revision, signature);
        this.store.confirmChat(uid, key, generation, chat.revision, false);
        await this.syncPromotedRead(uid, epoch, key, generation, entry.item.sourceCursor);
        if (legacy.has(key)) this.store.archiveSynced(scope, key, legacy.get(key)!);
        this.changed();
      }
    }
  }
  close() {
    this.stopped = true;
    clearTimeout(this.retry);
  }
}
