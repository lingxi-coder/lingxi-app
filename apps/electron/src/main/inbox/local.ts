import { randomUUID } from 'node:crypto';
import { mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import {
  inboxKey,
  type InboxItem,
  type InboxPreferences,
  type InboxMutation,
} from '../../../../../packages/inbox/contracts.js';
import type { SessionRef } from '../../shared/settings.js';
import type { SessionCatalogRow } from '../../shared/sessionWorkspace.js';

export interface DesktopInboxSession {
  row: SessionCatalogRow & { activity_json?: string | null };
  ref: SessionRef;
  state?: 'idle' | 'running' | 'waiting_for_input' | 'waiting_for_approval';
  remoteHostId?: string;
  archived?: boolean;
  cloudTitle?: string;
  sourceLabel?: string;
}

export const ordinal = (n: unknown): n is number => typeof n === 'number' && Number.isSafeInteger(n) && n >= 0;
export const safeId = (id: unknown): id is string => typeof id === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(id);
function label(s: string | undefined, max: number): string | undefined {
  if (!s || /[\u0000-\u001f\u007f/\\]/u.test(s)) return undefined;
  return s.trim().slice(0, max) || undefined;
}
export function localEntry(input: DesktopInboxSession) {
  if (!input.row.activity_json || !safeId(input.ref.sessionId) || input.row.uuid !== input.ref.sessionId) return;
  let activity;
  try {
    activity = JSON.parse(input.row.activity_json);
  } catch {
    return;
  }
  if (
    !activity ||
    !safeId(activity.sourceId) ||
    ![
      activity.cursor,
      activity.baselineCursor,
      activity.lastMeaningfulAt ?? 0,
      activity.sourceRevision,
      activity.pendingActionCount,
    ].every(ordinal) ||
    activity.cursor < activity.baselineCursor ||
    ![null, 'reply', 'taskResult', 'failure'].includes(activity.previewKind ?? null)
  )
    return;
  const pendingActionCount = activity.pendingActionCount as number;
  const ref = {
    kind: 'assistant' as const,
    sourceId: activity.sourceId as string,
    conversationId: input.ref.sessionId,
    generation: 1,
  };
  const state =
    input.state && input.state !== 'idle'
      ? input.state
      : pendingActionCount
        ? activity.pendingActionKind === 'approval'
          ? 'waiting_for_approval'
          : 'waiting_for_input'
        : activity.failed
          ? 'failed'
          : activity.cursor > activity.baselineCursor
            ? 'completed'
            : 'idle';
  const eventMetadata = {
    ...(ordinal(activity.failureAt) ? { failureAt: activity.failureAt } : {}),
    ...(ordinal(activity.failureRevision) ? { failureRevision: activity.failureRevision } : {}),
    ...(ordinal(activity.pendingActionAt) ? { pendingActionAt: activity.pendingActionAt } : {}),
    ...(ordinal(activity.pendingActionRevision) ? { pendingActionRevision: activity.pendingActionRevision } : {}),
  };
  const item: InboxItem = {
    schemaVersion: 1,
    key: inboxKey(ref),
    ref,
    category: 'assistant',
    title: input.row.resume_title || input.row.title || 'Assistant',
    preview:
      activity.previewKind === 'reply'
        ? 'Reply'
        : activity.previewKind === 'taskResult'
          ? 'Task completed'
          : activity.previewKind === 'failure'
            ? 'Task failed'
            : '',
    sourceCursor: activity.cursor,
    sourceRevision: activity.sourceRevision,
    lastActivityAt: Math.max(
      activity.lastMeaningfulAt ?? 0,
      activity.failed && ordinal(activity.failureAt) ? activity.failureAt : 0,
      pendingActionCount && ordinal(activity.pendingActionAt) ? activity.pendingActionAt : 0,
    ),
    readThrough: activity.baselineCursor,
    unreadCount: Math.min(100, activity.cursor - activity.baselineCursor),
    mentionCount: 0,
    ...eventMetadata,
    pendingActionCount,
    state,
    ...(activity.failed ? { attentionKind: 'failure' } : {}),
    archived: false,
    manualUnread: false,
    manualUnreadRevision: 0,
    stateRevision: 0,
    preferencesRevision: 0,
    notificationMode: 'mentions',
    pinOrder: null,
    offline: false,
  };
  const summary = {
    sourceId: ref.sourceId,
    conversationId: ref.conversationId,
    baseline: item.readThrough,
    sourceCursor: item.sourceCursor,
    sourceRevision: item.sourceRevision,
    lastActivityAt: activity.lastMeaningfulAt ?? 0,
    state,
    pendingActionCount,
    ...eventMetadata,
    previewKind: activity.previewKind ?? null,
    title: label(input.cloudTitle, 120),
    sourceLabel: label(input.sourceLabel, 80),
    remoteHostId: /^[A-Za-z0-9_-]{22}$/u.test(input.remoteHostId ?? '') ? input.remoteHostId : undefined,
  };
  return { item, summary, ref: input.ref, legacyArchived: input.archived };
}
export type LocalEntry = NonNullable<ReturnType<typeof localEntry>>;
type Personal = Pick<
  InboxItem,
  | 'readThrough'
  | 'manualUnread'
  | 'manualUnreadRevision'
  | 'stateRevision'
  | 'preferencesRevision'
  | keyof InboxPreferences
>;
type ScopedMutation = Exclude<InboxMutation, { action: 'markAllRead' }>;
export function validateTarget(p: ScopedMutation): void {
  if (
    !p ||
    typeof p.key !== 'string' ||
    !/^(assistant|team|service):[A-Za-z0-9_-]{1,128}:[A-Za-z0-9_-]{1,128}$/u.test(p.key) ||
    !ordinal(p.generation) ||
    p.generation < 1
  )
    throw new Error('INVALID_INPUT');
  if (p.action === 'markUnread' && !safeId(p.operationId)) throw new Error('INVALID_INPUT');
  if (p.action === 'markRead' && (!ordinal(p.through) || !ordinal(p.manualUnreadRevision)))
    throw new Error('INVALID_INPUT');
  if (
    p.action === 'preferences' &&
    (!p.patch ||
      typeof p.patch !== 'object' ||
      Object.keys(p.patch).some((key) => !['pinOrder', 'archived', 'notificationMode'].includes(key)) ||
      ('pinOrder' in p.patch && p.patch.pinOrder !== null && !ordinal(p.patch.pinOrder)) ||
      ('archived' in p.patch && typeof p.patch.archived !== 'boolean') ||
      ('notificationMode' in p.patch && !['all', 'none', 'mentions'].includes(String(p.patch.notificationMode))))
  )
    throw new Error('INVALID_INPUT');
}
export class LocalInboxStore {
  private readonly db: DatabaseSync;
  constructor(userData: string) {
    const dir = join(userData, 'inbox');
    mkdirSync(dir, { recursive: true, mode: 0o700 });
    this.db = new DatabaseSync(join(dir, 'state.sqlite'));
    this.db.exec(`CREATE TABLE IF NOT EXISTS device (singleton INTEGER PRIMARY KEY,id TEXT NOT NULL);
      CREATE TABLE IF NOT EXISTS consent (uid TEXT PRIMARY KEY, revision INTEGER);
      CREATE TABLE IF NOT EXISTS confirmation (uid TEXT,source TEXT,revision INTEGER,PRIMARY KEY(uid,source));
      CREATE TABLE IF NOT EXISTS account (uid TEXT PRIMARY KEY, enabled INTEGER);
      CREATE TABLE IF NOT EXISTS binding (uid TEXT,source TEXT,generation INTEGER,enabled INTEGER,PRIMARY KEY(uid,source));
      CREATE TABLE IF NOT EXISTS chat_consent (uid TEXT,key TEXT,enabled INTEGER,revision INTEGER,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS chat_attempt (uid TEXT,key TEXT,generation INTEGER,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS chat_confirm (uid TEXT,key TEXT,generation INTEGER,revision INTEGER,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS read_batch (uid TEXT,id TEXT,signature TEXT,created INTEGER,total INTEGER,processed INTEGER,changed INTEGER,PRIMARY KEY(uid,id));
      CREATE TABLE IF NOT EXISTS promoted_read(uid TEXT,key TEXT,cursor INTEGER,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS failure_read(uid TEXT,key TEXT,revision INTEGER,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS batch_failure(uid TEXT,id TEXT,key TEXT,revision INTEGER,PRIMARY KEY(uid,id,key));
      CREATE TABLE IF NOT EXISTS read_entry (uid TEXT,id TEXT,key TEXT,cursor INTEGER,manual INTEGER,PRIMARY KEY(uid,id,key));
      CREATE TABLE IF NOT EXISTS publication (uid TEXT,key TEXT,generation INTEGER,revision INTEGER,signature TEXT,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS personal (uid TEXT,key TEXT,state TEXT,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS archive_request (uid TEXT,key TEXT,expected INTEGER,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS legacy_archive (uid TEXT,key TEXT,value INTEGER,pending INTEGER,PRIMARY KEY(uid,key));
      CREATE TABLE IF NOT EXISTS manual (uid TEXT,key TEXT,id TEXT,time INTEGER,PRIMARY KEY(uid,key,id))`);
  }
  deviceId() {
    this.db.prepare('INSERT OR IGNORE INTO device VALUES(1,?)').run(randomUUID());
    return String(this.db.prepare('SELECT id FROM device WHERE singleton=1').get()!.id);
  }
  enabled(uid: string) {
    return this.db.prepare('SELECT enabled FROM account WHERE uid=?').get(uid)?.enabled === 1;
  }
  setEnabled(uid: string, enabled: boolean) {
    this.db.exec('BEGIN');
    try {
      this.db.prepare('INSERT OR REPLACE INTO account VALUES(?,?)').run(uid, Number(enabled));
      this.db.prepare('INSERT INTO consent VALUES(?,1) ON CONFLICT(uid) DO UPDATE SET revision=revision+1').run(uid);
      this.db.exec('COMMIT');
    } catch (error) {
      this.db.exec('ROLLBACK');
      throw error;
    }
  }
  consentRevision(uid: string) {
    return Number(this.db.prepare('SELECT revision FROM consent WHERE uid=?').get(uid)?.revision ?? 1);
  }
  confirmedRevision(uid: string, source: string) {
    return Number(
      this.db.prepare('SELECT revision FROM confirmation WHERE uid=? AND source=?').get(uid, source)?.revision ?? 0,
    );
  }
  confirm(uid: string, source: string, revision: number) {
    this.db.prepare('INSERT OR REPLACE INTO confirmation VALUES(?,?,?)').run(uid, source, revision);
  }
  sources(uid: string) {
    return this.db.prepare('SELECT source,generation,enabled FROM binding WHERE uid=?').all(uid) as unknown as {
      source: string;
      generation: number;
      enabled: number;
    }[];
  }
  bind(uid: string, source: string, generation: number, enabled: boolean) {
    this.db.prepare('INSERT OR REPLACE INTO binding VALUES(?,?,?,?)').run(uid, source, generation, Number(enabled));
  }
  observeArchives(uid: string, entries: Iterable<LocalEntry>) {
    const seen = new Map(
      this.db
        .prepare('SELECT key,value FROM legacy_archive WHERE uid=?')
        .all(uid)
        .map((r) => [String(r.key), Boolean(r.value)]),
    );
    const write = this.db.prepare('INSERT OR REPLACE INTO legacy_archive VALUES(?,?,?,1)');
    this.db.exec('BEGIN');
    try {
      for (const entry of entries) {
        const value = entry.legacyArchived;
        if (typeof value !== 'boolean' || value === (seen.get(entry.item.key) ?? false)) continue;
        write.run(uid, entry.item.key, Number(value));
        const row = this.row(uid, entry),
          expected = row.preferencesRevision ?? 0;
        this.db.prepare('INSERT OR REPLACE INTO archive_request VALUES(?,?,?)').run(uid, entry.item.key, expected);
        if (row.archived !== value)
          this.remember(uid, {
            ...row,
            archived: value,
            stateRevision: row.stateRevision + 1,
            preferencesRevision: expected + 1,
          });
      }
      this.db.exec('COMMIT');
    } catch (error) {
      this.db.exec('ROLLBACK');
      throw error;
    }
  }
  archiveExpected(uid: string, key: string) {
    return Number(
      this.db.prepare('SELECT expected FROM archive_request WHERE uid=? AND key=?').get(uid, key)?.expected ?? 0,
    );
  }
  pendingArchives(uid: string) {
    return new Map(
      this.db
        .prepare('SELECT key,value FROM legacy_archive WHERE uid=? AND pending=1')
        .all(uid)
        .map((row) => [String(row.key), Boolean(row.value)]),
    );
  }
  archiveSynced(uid: string, key: string, value: boolean) {
    this.db
      .prepare('UPDATE legacy_archive SET pending=0 WHERE uid=? AND key=? AND value=?')
      .run(uid, key, Number(value));
  }
  personalRows(uid: string) {
    return new Map(
      this.db
        .prepare('SELECT key,state FROM personal WHERE uid=?')
        .all(uid)
        .map((row) => [String(row.key), JSON.parse(String(row.state)) as Personal]),
    );
  }
  chatConsent(uid: string, key: string): { enabled: boolean; revision: number } {
    const row = this.db.prepare('SELECT enabled,revision FROM chat_consent WHERE uid=? AND key=?').get(uid, key);
    return { enabled: row ? Boolean(row.enabled) : true, revision: Number(row?.revision ?? 1) };
  }
  setChatConsent(uid: string, key: string, enabled: boolean) {
    this.db
      .prepare(
        'INSERT INTO chat_consent VALUES(?,?,?,2) ON CONFLICT(uid,key) DO UPDATE SET enabled=excluded.enabled,revision=chat_consent.revision+1 WHERE enabled<>excluded.enabled',
      )
      .run(uid, key, Number(enabled));
  }
  privateChatKeys(uid: string) {
    return new Set(
      this.db
        .prepare(
          `SELECT c.key FROM chat_consent c
      LEFT JOIN chat_confirm a ON a.uid=c.uid AND a.key=c.key
      WHERE c.uid=? AND (c.enabled=0 OR a.revision IS NULL OR a.revision<>c.revision)`,
        )
        .all(uid)
        .map((row) => String(row.key)),
    );
  }
  disabledChats(uid: string) {
    return new Map(
      this.db
        .prepare('SELECT key,revision FROM chat_consent WHERE uid=? AND enabled=0')
        .all(uid)
        .map((r) => [String(r.key), Number(r.revision)]),
    );
  }
  attempt(uid: string, key: string, generation: number) {
    this.db.prepare('INSERT OR REPLACE INTO chat_attempt VALUES(?,?,?)').run(uid, key, generation);
  }
  mayHavePublished(uid: string, key: string, generation: number) {
    return (
      this.db.prepare('SELECT 1 FROM chat_attempt WHERE uid=? AND key=? AND generation=?').get(uid, key, generation) ||
      this.publication(uid, key)?.generation === generation
    );
  }
  confirmedChat(uid: string, key: string, generation: number, revision: number) {
    return !!this.db
      .prepare('SELECT 1 FROM chat_confirm WHERE uid=? AND key=? AND generation=? AND revision=?')
      .get(uid, key, generation, revision);
  }
  confirmChat(uid: string, key: string, generation: number, revision: number, removed: boolean) {
    this.db.exec('BEGIN');
    try {
      this.db.prepare('INSERT OR REPLACE INTO chat_confirm VALUES(?,?,?,?)').run(uid, key, generation, revision);
      if (removed) this.db.prepare('DELETE FROM publication WHERE uid=? AND key=?').run(uid, key);
      this.db.exec('COMMIT');
    } catch (error) {
      this.db.exec('ROLLBACK');
      throw error;
    }
  }
  failureObserved(uid: string, key: string, revision: number) {
    return (
      Number(this.db.prepare('SELECT revision FROM failure_read WHERE uid=? AND key=?').get(uid, key)?.revision ?? 0) >=
      revision
    );
  }
  observeFailure(uid: string, entry: LocalEntry, revision: number | undefined) {
    if (!revision || entry.item.attentionKind !== 'failure' || revision !== entry.summary.failureRevision) return;
    this.db
      .prepare(
        'INSERT INTO failure_read VALUES(?,?,?) ON CONFLICT(uid,key) DO UPDATE SET revision=MAX(revision,excluded.revision)',
      )
      .run(uid, entry.item.key, revision);
  }
  batch(uid: string, id: string) {
    return this.db
      .prepare('SELECT signature,total,processed,changed FROM read_batch WHERE uid=? AND id=? AND created>?')
      .get(uid, id, Date.now() - 30 * 86400000) as
      | { signature: string; total: number; processed: number; changed: number }
      | undefined;
  }
  freezeBatch(uid: string, id: string, signature: string, rows: Iterable<InboxItem>) {
    const current = this.batch(uid, id);
    if (current) {
      if (current.signature !== signature) throw new Error('OPERATION_CONFLICT');
      return;
    }
    this.db.exec('BEGIN');
    try {
      const cutoff = Date.now() - 30 * 86400000;
      this.db
        .prepare('DELETE FROM read_entry WHERE (uid,id) IN (SELECT uid,id FROM read_batch WHERE created<?)')
        .run(cutoff);
      this.db.prepare('DELETE FROM read_batch WHERE created<?').run(cutoff);
      this.db.exec('DELETE FROM batch_failure WHERE (uid,id) NOT IN (SELECT uid,id FROM read_batch)');
      const insert = this.db.prepare('INSERT INTO read_entry VALUES(?,?,?,?,?)');
      const insertFailure = this.db.prepare('INSERT INTO batch_failure VALUES(?,?,?,?)');
      let count = 0;
      for (const row of rows) {
        insert.run(uid, id, row.key, row.sourceCursor, row.manualUnreadRevision);
        if (row.attentionKind === 'failure' && row.failureRevision)
          insertFailure.run(uid, id, row.key, row.failureRevision);
        count++;
      }
      this.db.prepare('INSERT INTO read_batch VALUES(?,?,?,?,?,0,0)').run(uid, id, signature, Date.now(), count);
      this.db.exec('COMMIT');
    } catch (error) {
      this.db.exec('ROLLBACK');
      throw error;
    }
  }
  promotedRead(uid: string, key: string) {
    return Number(this.db.prepare('SELECT cursor FROM promoted_read WHERE uid=? AND key=?').get(uid, key)?.cursor ?? 0);
  }
  promotedReadSent(uid: string, key: string, through: number) {
    this.db.prepare('DELETE FROM promoted_read WHERE uid=? AND key=? AND cursor<=?').run(uid, key, through);
  }
  applyBatchPage(uid: string, id: string, entries: Map<string, LocalEntry>, account: string | null) {
    this.db.exec('BEGIN');
    try {
      const page = this.db
        .prepare(
          'SELECT e.key,e.cursor,e.manual,f.revision AS failureRevision FROM read_entry e LEFT JOIN batch_failure f USING(uid,id,key) WHERE e.uid=? AND e.id=? ORDER BY e.key LIMIT 50',
        )
        .all(uid, id);
      const del = this.db.prepare('DELETE FROM read_entry WHERE uid=? AND id=? AND key=?');
      let changed = 0;
      for (const frozen of page) {
        const key = String(frozen.key),
          entry = entries.get(key);
        if (entry && entry.item.sourceCursor >= Number(frozen.cursor)) {
          if (frozen.failureRevision) this.observeFailure(uid, entry, Number(frozen.failureRevision));
          const row = this.row(uid, entry),
            through = Math.max(row.readThrough, Number(frozen.cursor));
          const clears = row.manualUnread && row.manualUnreadRevision === frozen.manual;
          // A private row can be published while the account-wide server snapshot is being admitted.
          // The new cloud row must receive the frozen cursor without clearing any newer cloud-side manual flag.
          if (
            account &&
            this.enabled(account) &&
            this.chatConsent(account, key).enabled &&
            this.db.prepare('SELECT 1 FROM chat_attempt WHERE uid=? AND key=?').get(account, key)
          ) {
            this.db
              .prepare(
                'INSERT INTO promoted_read VALUES(?,?,?) ON CONFLICT(uid,key) DO UPDATE SET cursor=MAX(cursor,excluded.cursor)',
              )
              .run(account, key, Number(frozen.cursor));
          }
          if (clears || through > row.readThrough) {
            this.remember(uid, {
              ...row,
              readThrough: through,
              manualUnread: row.manualUnread && !clears,
              stateRevision: row.stateRevision + 1,
            });
            changed++;
          }
        }
        del.run(uid, id, key);
        if (frozen.failureRevision)
          this.db.prepare('DELETE FROM batch_failure WHERE uid=? AND id=? AND key=?').run(uid, id, key);
      }
      this.db
        .prepare('UPDATE read_batch SET processed=processed+?,changed=changed+? WHERE uid=? AND id=?')
        .run(page.length, changed, uid, id);
      this.db.exec('COMMIT');
    } catch (error) {
      this.db.exec('ROLLBACK');
      throw error;
    }
    return this.batch(uid, id)!;
  }
  publications(uid: string) {
    return new Map(
      this.db
        .prepare('SELECT key,generation FROM publication WHERE uid=?')
        .all(uid)
        .map((row) => [String(row.key), Number(row.generation)]),
    );
  }
  publication(uid: string, key: string) {
    return this.db
      .prepare('SELECT generation,revision,signature FROM publication WHERE uid=? AND key=?')
      .get(uid, key) as { generation: number; revision: number; signature: string } | undefined;
  }
  published(uid: string, key: string, generation: number, revision: number, signature: string) {
    this.db
      .prepare('INSERT OR REPLACE INTO publication VALUES(?,?,?,?,?)')
      .run(uid, key, generation, revision, signature);
  }
  allowsNotifications(uid: string, sessionId: string) {
    if (!safeId(sessionId)) return false;
    const result = this.db
      .prepare('SELECT state FROM personal WHERE uid=? AND key GLOB ? LIMIT 1')
      .get(uid, 'assistant:*:' + sessionId);
    if (!result) return true;
    const personal: Personal = JSON.parse(String(result.state));
    return !personal.archived && personal.notificationMode !== 'none';
  }
  private personal(uid: string, item: InboxItem): Personal {
    const saved = this.db.prepare('SELECT state FROM personal WHERE uid=? AND key=?').get(uid, item.key);
    return saved
      ? JSON.parse(String(saved.state))
      : {
          readThrough: item.readThrough,
          manualUnread: false,
          manualUnreadRevision: 0,
          stateRevision: 0,
          preferencesRevision: 0,
          archived: false,
          notificationMode: 'mentions',
          pinOrder: null,
        };
  }
  remember(uid: string, row: InboxItem) {
    const {
      readThrough,
      manualUnread,
      manualUnreadRevision,
      stateRevision,
      preferencesRevision,
      archived,
      notificationMode,
      pinOrder,
    } = row;
    this.db.prepare('INSERT OR REPLACE INTO personal VALUES(?,?,?)').run(
      uid,
      row.key,
      JSON.stringify({
        readThrough,
        manualUnread,
        manualUnreadRevision,
        stateRevision,
        preferencesRevision: preferencesRevision ?? 0,
        archived,
        notificationMode,
        pinOrder,
      }),
    );
  }
  row(uid: string, entry: LocalEntry, rows?: Map<string, Personal>): InboxItem {
    const personal = rows ? (rows.get(entry.item.key) ?? entry.item) : this.personal(uid, entry.item);
    const readThrough = Math.max(entry.item.readThrough, personal.readThrough);
    return {
      ...entry.item,
      ...personal,
      readThrough,
      unreadCount: Math.min(100, Math.max(0, entry.item.sourceCursor - readThrough)),
    };
  }
  mutate(uid: string, entry: LocalEntry, cmd: ScopedMutation): InboxItem {
    validateTarget(cmd);
    const row = this.row(uid, entry);
    if (cmd.generation !== row.ref.generation) throw new Error('STALE_MEMBERSHIP');
    if (cmd.action === 'markUnread' && cmd.expectedManualUnreadRevision !== undefined) {
      if (!ordinal(cmd.expectedManualUnreadRevision)) throw new Error('INVALID_INPUT');
      if (cmd.expectedManualUnreadRevision < row.manualUnreadRevision) return row;
      if (cmd.expectedManualUnreadRevision !== row.manualUnreadRevision) throw new Error('MANUAL_REVISION_CONFLICT');
    }
    this.db.exec('BEGIN');
    try {
      if (cmd.action === 'preferences') {
        if (cmd.expectedPreferencesRevision !== (row.preferencesRevision ?? 0))
          throw new Error('PREFERENCE_REVISION_CONFLICT');
        if (Object.entries(cmd.patch).every(([key, value]) => row[key as keyof InboxPreferences] === value)) {
          this.db.exec('COMMIT');
          return row;
        }
        row.preferencesRevision = (row.preferencesRevision ?? 0) + 1;
        Object.assign(row, cmd.patch);
        if (cmd.patch.archived !== undefined)
          this.db.prepare('UPDATE legacy_archive SET pending=0 WHERE uid=? AND key=?').run(uid, row.key);
      }
      if (cmd.action === 'markUnread') {
        const now = Date.now();
        this.db.prepare('DELETE FROM manual WHERE time<?').run(now - 30 * 86400000);
        const inserted = this.db
          .prepare('INSERT OR IGNORE INTO manual VALUES(?,?,?,?)')
          .run(uid, row.key, cmd.operationId, now);
        if (inserted.changes) {
          row.manualUnread = true;
          row.manualUnreadRevision++;
        } else {
          this.db.exec('COMMIT');
          return row;
        }
      }
      if (cmd.action === 'markRead') {
        if (cmd.through > row.sourceCursor) throw new Error('INVALID_READ_CURSOR');
        this.observeFailure(uid, entry, cmd.observedFailureRevision);
        const next = Math.max(row.readThrough, cmd.through);
        const clear = row.manualUnread && row.manualUnreadRevision === cmd.manualUnreadRevision;
        if (next === row.readThrough && !clear) {
          this.db.exec('COMMIT');
          return row;
        }
        row.readThrough = next;
        if (clear) row.manualUnread = false;
        row.unreadCount = Math.min(100, row.sourceCursor - next);
      }
      row.stateRevision++;
      this.remember(uid, row);
      this.db.exec('COMMIT');
      return row;
    } catch (error) {
      this.db.exec('ROLLBACK');
      throw error;
    }
  }
  close() {
    this.db.close();
  }
}
