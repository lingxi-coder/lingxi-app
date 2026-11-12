import { useEffect, useRef, useState } from 'react';
import { LIST_ITEM_BYTES } from './execution-validation.js';
import { executionCommand, type ExecutionScope, type SharedBotCard, type SharedList, type SharedListChange, type SharedListWritten } from './execution.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog } from './ui';

const failure = (error: unknown) => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
const fits = (text: string) => new TextEncoder().encode(text).length <= LIST_ITEM_BYTES;

/**
 * A Bot's shared list: the people who may use the Bot and the Bot's own runs change the same items.
 * A change is saved only on top of the list it was made from. When someone else, or the Bot, saved first, nothing is saved:
 * the list is read again, what the person typed stays where it is, and saving it again is their decision.
 */
export function SharedListDialog({ api, scope, bot, zh, onClose }: {
  api: TeamsApi; scope: ExecutionScope; bot: Pick<SharedBotCard, 'sharedBotId' | 'name' | 'sharedList'>; zh: boolean; onClose(): void;
}) {
  const [list, setList] = useState<SharedList>(), [text, setText] = useState(''), [editing, setEditing] = useState<{ itemId: string; text: string }>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [moved, setMoved] = useState(false);
  /** One operation id per exact change of one revision, so repeating a save whose answer was lost changes nothing twice. */
  const operation = useRef<{ key: string; id: string }>();
  const t = (en: string, cn: string) => zh ? cn : en;
  const read = async () => {
    const next = await api.messages<SharedList>(executionCommand(scope, 'readList', { sharedBotId: bot.sharedBotId }));
    setList(next); setEditing(current => current && next.items.some(item => item.itemId === current.itemId) ? current : undefined);
  };
  const attempt = async (work: () => Promise<void>) => {
    if (busy) return; setBusy(true); setError('');
    try { await work(); } catch (e) { setError(failure(e)); } finally { setBusy(false); }
  };
  useEffect(() => { void attempt(read); }, []);
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
    <div className="tw-dialog-actions">
      <button type="button" disabled={busy} onClick={() => void attempt(async () => { setMoved(false); await read(); })}>{t('Refresh', '刷新')}</button>
      <button type="button" onClick={onClose}>{t('Close', '关闭')}</button>
    </div>
  </div></Dialog>;
}
