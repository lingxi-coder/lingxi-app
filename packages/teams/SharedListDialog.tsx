import { useEffect, useRef, useState } from 'react';
import { LIST_ITEM_BYTES } from './execution-validation.js';
import { executionCommand, type ExecutionScope, type SharedBotCard, type SharedList, type SharedListAuthor, type SharedListChange, type SharedListHistory, type SharedListLoggedChange, type SharedListRevision, type SharedListWritten } from './execution.js';
import type { TeamMember, TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog } from './ui';

const failure = (error: unknown) => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
const fits = (text: string) => new TextEncoder().encode(text).length <= LIST_ITEM_BYTES;

/** One change of a list revision in words. A change that renamed and ticked an item says both. */
export function listChangeText(change: SharedListLoggedChange, zh: boolean): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  if (change.op === 'add') return t(`Added “${change.text}”`, `添加了“${change.text}”`);
  const was = change.before.text;
  if (change.op === 'remove') return t(`Removed “${was}”`, `移除了“${was}”`);
  const parts: string[] = [];
  if (change.text !== undefined && change.text !== was) parts.push(t(`Renamed “${was}” to “${change.text}”`, `将“${was}”改为“${change.text}”`));
  const name = change.text ?? was;
  if (change.done !== undefined && change.done !== change.before.done) parts.push(change.done ? t(`Ticked “${name}”`, `勾选了“${name}”`) : t(`Unticked “${name}”`, `取消勾选了“${name}”`));
  if (!parts.length) return t(`Saved “${name}” unchanged`, `保存了未更改的“${name}”`);
  return zh ? parts.join('；') : parts.map((part, index) => index ? part[0]!.toLowerCase() + part.slice(1) : part).join('; ');
}
/** Who made a revision: a member by name, or the Bot and the member whose request it was working on. */
export function listAuthorText(author: SharedListAuthor, { uid, members, botName, zh }: { uid: string; members: TeamMember[]; botName: string; zh: boolean }): string {
  const t = (en: string, cn: string) => zh ? cn : en;
  const person = (who: { uid: string; generation: number }, self: string) => who.uid === uid ? self
    : members.find(member => member.uid === who.uid && member.generation === who.generation)?.displayName ?? t('a former member', '已离开的成员');
  if (author.kind === 'user') return person(author, t('You', '你'));
  return t(`${botName}, for ${person(author.requester, 'you')}`, `${botName}（为${person(author.requester, '你')}）`);
}

/**
 * A Bot's shared list: the people who may use the Bot and the Bot's own runs change the same items.
 * A change is saved only on top of the list it was made from. When someone else, or the Bot, saved first, nothing is saved:
 * the list is read again, what the person typed stays where it is, and saving it again is their decision.
 */
export function SharedListDialog({ api, scope, bot, uid, members, zh, onClose }: {
  api: TeamsApi; scope: ExecutionScope; bot: Pick<SharedBotCard, 'sharedBotId' | 'name' | 'sharedList'>; uid: string; members: TeamMember[]; zh: boolean; onClose(): void;
}) {
  const [list, setList] = useState<SharedList>(), [text, setText] = useState(''), [editing, setEditing] = useState<{ itemId: string; text: string }>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [moved, setMoved] = useState(false);
  /** One operation id per exact change of one revision, so repeating a save whose answer was lost changes nothing twice. */
  const operation = useRef<{ key: string; id: string }>();
  const t = (en: string, cn: string) => zh ? cn : en;
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

  return <Dialog title={list?.title ?? bot.sharedList?.title ?? t('Shared list', '共享清单')} onClose={onClose}><div className="tw-form tw-list">
    {list ? <>
      <p className="tw-hint">{t(`Everyone who can use ${bot.name} sees this list and can change it. ${bot.name} can read and change it too.`, `所有可以使用 ${bot.name} 的人都能查看并修改此清单，${bot.name} 也可以读取和修改它。`)}</p>
      {moved ? <p role="alert">{t('The list was changed in the meantime, so your change was not saved. Check the list below, then try again if it is still needed.', '清单在此期间已被修改，你的更改未保存。请查看下方的最新清单，如仍有需要再重试。')}</p> : null}
      {list.items.length ? <ul className="tw-list-items">{list.items.map(item => <li key={item.itemId}>
        {editing?.itemId === item.itemId
          ? <form onSubmit={event => { event.preventDefault(); if (usable(editing.text) && editing.text.trim() !== item.text) void write([{ op: 'update', itemId: item.itemId, text: editing.text }], () => setEditing(undefined)); }}>
            <input aria-label={t('Item text', '条目内容')} value={editing.text} onChange={event => setEditing({ itemId: item.itemId, text: event.target.value })} />
            <button type="submit" disabled={busy || !usable(editing.text) || editing.text.trim() === item.text}>{t('Save', '保存')}</button>
            <button type="button" onClick={() => setEditing(undefined)}>{t('Cancel', '取消')}</button>
          </form>
          : <><label><input type="checkbox" checked={item.done} disabled={busy} onChange={event => void write([{ op: 'update', itemId: item.itemId, done: event.target.checked }])} /><span data-done={item.done}>{item.text}</span></label>
            <button type="button" disabled={busy} aria-label={t(`Edit “${item.text}”`, `编辑“${item.text}”`)} onClick={() => setEditing({ itemId: item.itemId, text: item.text })}>{t('Edit', '编辑')}</button>
            <button type="button" disabled={busy} aria-label={t(`Remove “${item.text}”`, `移除“${item.text}”`)} onClick={() => void write([{ op: 'remove', itemId: item.itemId }])}>{t('Remove', '移除')}</button></>}
      </li>)}</ul> : <p className="tw-hint">{t('Nothing is on the list yet.', '清单还是空的。')}</p>}
      <form className="tw-list-add" onSubmit={event => { event.preventDefault(); if (usable(text)) void write([{ op: 'add', text }], () => setText('')); }}>
        <input data-autofocus aria-label={t('New item', '新条目')} placeholder={t('Add an item', '添加条目')} value={text} onChange={event => setText(event.target.value)} />
        <button type="submit" className="tw-primary" disabled={busy || !usable(text)}>{t('Add', '添加')}</button>
      </form>
      {long ? <p role="alert">{t('This item is too long to save. Shorten it and try again.', '条目过长，无法保存。请缩短后重试。')}</p> : null}
    </> : error ? null : <p role="status">{t('Loading the list…', '正在加载清单…')}</p>}
    {error ? <p role="alert">{messageErrorText(error, zh)}</p> : null}
    {list ? <details className="tw-list-history" onToggle={event => {
      if ((event.currentTarget as HTMLDetailsElement).open && !opened.current) { opened.current = true; void page(); }
    }}>
      <summary>{t('History', '修改记录')}</summary>
      {history ? history.revisions.length ? <ol>{history.revisions.map(revision => <li key={revision.dataRevision}>
        <p><strong>{listAuthorText(revision.author, { uid, members, botName: bot.name, zh })}</strong> · <time dateTime={new Date(revision.createdAt).toISOString()}>{new Date(revision.createdAt).toLocaleString(zh ? 'zh-CN' : 'en')}</time></p>
        <ul>{revision.changes.map((change, index) => <li key={index}>{listChangeText(change, zh)}</li>)}</ul>
      </li>)}</ol> : <p className="tw-hint">{t('No changes yet.', '还没有修改。')}</p> : historyError ? null : <p role="status">{t('Loading the history…', '正在加载修改记录…')}</p>}
      {history?.hasMore ? <button type="button" onClick={() => void page(history.revisions.at(-1)?.dataRevision)}>{t('Show older changes', '显示更早的修改')}</button> : null}
      {historyError ? <p role="alert">{messageErrorText(historyError, zh)}</p> : null}
      <p className="tw-hint">{t('The latest 200 changes are kept.', '保留最近 200 次修改。')}</p>
    </details> : null}
    <div className="tw-dialog-actions">
      <button type="button" disabled={busy} onClick={() => void attempt(async () => { setMoved(false); await read(); if (opened.current) await page(); })}>{t('Refresh', '刷新')}</button>
      <button type="button" onClick={onClose}>{t('Close', '关闭')}</button>
    </div>
  </div></Dialog>;
}
