import { useEffect, useRef, useState } from 'react';
import { LIST_ITEM_BYTES } from './execution-validation.js';
import { executionCommand, type ExecutionScope, type SharedBotCard, type SharedList, type SharedListAuthor, type SharedListChange, type SharedListHistory, type SharedListLoggedChange, type SharedListRevision, type SharedListWritten } from './execution.js';
import type { TeamMember, TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog } from './ui';
import { intlTag, teamsText, useTeamsText } from './i18n.js';

const failure = (error: unknown) => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
const fits = (text: string) => new TextEncoder().encode(text).length <= LIST_ITEM_BYTES;

/** One change of a list revision in words. A change that renamed and ticked an item says both. */
export function listChangeText(change: SharedListLoggedChange, locale: string): string {
  const t = teamsText(locale);
  if (change.op === 'add') return t('teams_list_added_fmt', change.text);
  const was = change.before.text;
  if (change.op === 'remove') return t('teams_list_removed_fmt', was);
  const parts: string[] = [];
  if (change.text !== undefined && change.text !== was) parts.push(t('teams_list_renamed_fmt', was, change.text));
  const name = change.text ?? was;
  if (change.done !== undefined && change.done !== change.before.done) parts.push(change.done ? t('teams_list_ticked_fmt', name) : t('teams_list_unticked_fmt', name));
  if (!parts.length) return t('teams_list_unchanged_fmt', name);
  return parts.map((part, index) => index && t.locale === 'en' ? part[0]!.toLowerCase() + part.slice(1) : part).join(t('tw_list_change_separator'));
}
/** Who made a revision: a member by name, or the Bot and the member whose request it was working on. */
export function listAuthorText(author: SharedListAuthor, { uid, members, botName, locale }: { uid: string; members: TeamMember[]; botName: string; locale: string }): string {
  const t = teamsText(locale);
  const person = (who: { uid: string; generation: number }, self: string) => who.uid === uid ? self
    : members.find(member => member.uid === who.uid && member.generation === who.generation)?.displayName ?? t('teams_list_history_former_inline');
  if (author.kind === 'user') return person(author, t('teams_list_history_you'));
  return t('teams_list_history_for_fmt', botName, person(author.requester, t('teams_list_history_you_inline')));
}

/**
 * A Bot's shared list: the people who may use the Bot and the Bot's own runs change the same items.
 * A change is saved only on top of the list it was made from. When someone else, or the Bot, saved first, nothing is saved:
 * the list is read again, what the person typed stays where it is, and saving it again is their decision.
 */
export function SharedListDialog({ api, scope, bot, uid, members, locale, onClose }: {
  api: TeamsApi; scope: ExecutionScope; bot: Pick<SharedBotCard, 'sharedBotId' | 'name' | 'sharedList'>; uid: string; members: TeamMember[]; locale: string; onClose(): void;
}) {
  const [list, setList] = useState<SharedList>(), [text, setText] = useState(''), [editing, setEditing] = useState<{ itemId: string; text: string }>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [moved, setMoved] = useState(false);
  /** One operation id per exact change of one revision, so repeating a save whose answer was lost changes nothing twice. */
  const operation = useRef<{ key: string; id: string }>();
  const t = useTeamsText(locale);
  /** The newest revision shown. A read that answers after a newer one is dropped, so the list never goes back. */
  const latest = useRef(-1);
  const read = async () => {
    const next = await api.messages<SharedList>(executionCommand(scope, 'readList', { sharedBotId: bot.sharedBotId }));
    if (next.dataRevision < latest.current) return;
    latest.current = next.dataRevision;
    setList(next); setEditing(current => current && next.items.some(item => item.itemId === current.itemId) ? current : undefined);
  };
  /** The history as far as it was opened: the newest page, and older pages when asked for. */
  const [history, setHistory] = useState<{ revisions: SharedListRevision[]; hasMore: boolean }>(), [historyError, setHistoryError] = useState('');
  const opened = useRef(false);
  const page = async (beforeRevision?: number) => {
    try {
      const next = await api.messages<SharedListHistory>(executionCommand(scope, 'listHistory', { sharedBotId: bot.sharedBotId, ...(beforeRevision === undefined ? {} : { beforeRevision }) }));
      setHistoryError('');
      setHistory(current => {
        const known = new Map((current?.revisions ?? []).map(revision => [revision.dataRevision, revision]));
        for (const revision of next.revisions) known.set(revision.dataRevision, revision);
        // Reading the newest page again keeps the older pages already shown, so whether there is more is still what the
        // oldest page shown said.
        return { revisions: [...known.values()].sort((a, b) => b.dataRevision - a.dataRevision),
          hasMore: beforeRevision === undefined && current ? current.hasMore : next.hasMore };
      });
    } catch (e) { setHistoryError(failure(e)); }
  };
  const attempt = async (work: () => Promise<void>) => {
    if (busy) return; setBusy(true); setError('');
    try { await work(); } catch (e) { setError(failure(e)); } finally { setBusy(false); }
  };
  useEffect(() => { void attempt(read); }, []);
  // Someone else, or the Bot, changed the list: show it without waiting for Refresh. What is being typed stays.
  useEffect(() => api.onMessagesChanged(signal => {
    if (signal && !signal.bots) return;
    void read().catch(() => undefined);
    if (opened.current) void page();
  }), []);
  const write = (changes: SharedListChange[], saved?: () => void) => attempt(async () => {
    if (!list) return;
    const key = JSON.stringify([list.dataRevision, changes]);
    if (operation.current?.key !== key) operation.current = { key, id: crypto.randomUUID() };
    setMoved(false);
    try {
      await api.messages<SharedListWritten>(executionCommand(scope, 'writeList', { sharedBotId: bot.sharedBotId, operationId: operation.current.id, expectedDataRevision: list.dataRevision, changes }));
    } catch (e) {
      if (!['LIST_REVISION_CONFLICT', 'LIST_ITEM_REQUIRED'].includes(failure(e))) throw e;
      await read(); setMoved(true); return;
    }
    saved?.(); await read();
  });
  const usable = (value: string) => value.trim().length > 0 && fits(value);
  const long = !fits(text) || !!editing && !fits(editing.text);

  return <Dialog title={list?.title ?? bot.sharedList?.title ?? t('teams_list_open')} onClose={onClose}><div className="tw-form tw-list">
    {list ? <>
      <p className="tw-hint">{t('teams_list_hint_fmt', bot.name, bot.name)}</p>
      {moved ? <p role="alert">{t('tw_list_moved_below')}</p> : null}
      {list.items.length ? <ul className="tw-list-items">{list.items.map(item => <li key={item.itemId}>
        {editing?.itemId === item.itemId
          ? <form onSubmit={event => { event.preventDefault(); if (usable(editing.text) && editing.text.trim() !== item.text) void write([{ op: 'update', itemId: item.itemId, text: editing.text }], () => setEditing(undefined)); }}>
            <input aria-label={t('tw_list_item_text')} value={editing.text} onChange={event => setEditing({ itemId: item.itemId, text: event.target.value })} />
            <button type="submit" disabled={busy || !usable(editing.text) || editing.text.trim() === item.text}>{t('common_save')}</button>
            <button type="button" onClick={() => setEditing(undefined)}>{t('common_cancel')}</button>
          </form>
          : <><label><input type="checkbox" checked={item.done} disabled={busy} onChange={event => void write([{ op: 'update', itemId: item.itemId, done: event.target.checked }])} /><span data-done={item.done}>{item.text}</span></label>
            <button type="button" disabled={busy} aria-label={t('tw_list_edit_fmt', item.text)} onClick={() => setEditing({ itemId: item.itemId, text: item.text })}>{t('teams_list_edit')}</button>
            <button type="button" disabled={busy} aria-label={t('tw_list_remove_fmt', item.text)} onClick={() => void write([{ op: 'remove', itemId: item.itemId }])}>{t('teams_list_remove')}</button></>}
      </li>)}</ul> : <p className="tw-hint">{t('teams_list_empty')}</p>}
      <form className="tw-list-add" onSubmit={event => { event.preventDefault(); if (usable(text)) void write([{ op: 'add', text }], () => setText('')); }}>
        <input data-autofocus aria-label={t('tw_list_new_item')} placeholder={t('teams_list_add_placeholder')} value={text} onChange={event => setText(event.target.value)} />
        <button type="submit" className="tw-primary" disabled={busy || !usable(text)}>{t('teams_list_add')}</button>
      </form>
      {long ? <p role="alert">{t('teams_list_too_long')}</p> : null}
    </> : error ? null : <p role="status">{t('teams_list_loading')}</p>}
    {error ? <p role="alert">{messageErrorText(error, locale)}</p> : null}
    {list ? <details className="tw-list-history" onToggle={event => {
      if ((event.currentTarget as HTMLDetailsElement).open && !opened.current) { opened.current = true; void page(); }
    }}>
      <summary>{t('teams_list_history')}</summary>
      {history ? history.revisions.length ? <ol>{history.revisions.map(revision => <li key={revision.dataRevision}>
        <p><strong>{listAuthorText(revision.author, { uid, members, botName: bot.name, locale })}</strong> · <time dateTime={new Date(revision.createdAt).toISOString()}>{new Date(revision.createdAt).toLocaleString(intlTag(locale))}</time></p>
        <ul>{revision.changes.map((change, index) => <li key={index}>{listChangeText(change, locale)}</li>)}</ul>
      </li>)}</ol> : <p className="tw-hint">{t('teams_list_history_empty')}</p> : historyError ? null : <p role="status">{t('teams_list_history_loading')}</p>}
      {history?.hasMore ? <button type="button" onClick={() => void page(history.revisions.at(-1)?.dataRevision)}>{t('teams_list_history_older')}</button> : null}
      {historyError ? <p role="alert">{messageErrorText(historyError, locale)}</p> : null}
      <p className="tw-hint">{t('teams_list_history_kept')}</p>
    </details> : null}
    <div className="tw-dialog-actions">
      <button type="button" disabled={busy} onClick={() => void attempt(async () => { setMoved(false); await read(); if (opened.current) await page(); })}>{t('teams_list_refresh')}</button>
      <button type="button" onClick={onClose}>{t('common_close')}</button>
    </div>
  </div></Dialog>;
}
