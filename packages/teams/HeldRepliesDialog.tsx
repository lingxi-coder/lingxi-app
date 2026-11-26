import { useState, type ReactElement } from 'react';
import type { ExecutionScope, SharedBotCard } from './execution.js';
import type { TeamsApi } from './index.js';
import { heldReplyErrorText } from './errors.js';
import { HeldReplyCard } from './HeldReplyCard';
import { useHeldReplies, type HeldReplies } from './held-hooks.js';
import type { HeldRow } from './held-replies.js';
import { useTeamsText } from './i18n.js';
import { Dialog, type RenderMarkdown } from './ui';

/** The replies of this Bot that still wait for its maintainer: the ones that are not decided, ended or out of time. */
const waitingCount = (held: HeldReplies): number => held.view.rows.filter(row => row.ended === undefined && row.reply.expiresAt > held.view.now).length;

/**
 * "Replies waiting" on the card of a Bot its viewer maintains: a button with the number of replies of the Bot that wait for them across every Chat, only while there are
 * some, and the list that opens from it. The service says who maintains a Bot; when it says this person does not after all, the button is not offered again.
 * `onOpenChat` takes the person to the Chat a reply was made in.
 */
export function HeldRepliesButton({ api, scope, bot, locale, busy, renderMarkdown, onOpenChat }: {
  api: TeamsApi; scope: ExecutionScope; bot: Pick<SharedBotCard, 'sharedBotId' | 'name'>; locale: string; busy?: boolean; renderMarkdown?: RenderMarkdown; onOpenChat?(conversationId: string): void;
}): ReactElement | null {
  const t = useTeamsText(locale);
  const [open, setOpen] = useState(false);
  const held = useHeldReplies(api, scope, { sharedBotIds: [bot.sharedBotId] });
  const waiting = waitingCount(held);
  if (held.view.status === 'denied') return null;
  return <>
    {waiting > 0 || open ? <button type="button" className="tw-held-entry" disabled={busy} onClick={() => setOpen(true)}>{t('teams_held_waiting')}{waiting > 0 ? <> <span className="tw-badge">{waiting > 99 ? '99+' : waiting}</span></> : null}</button> : null}
    {open ? <HeldRepliesDialog bot={bot} held={held} locale={locale} renderMarkdown={renderMarkdown} onOpenChat={onOpenChat} onClose={() => setOpen(false)} /> : null}
  </>;
}

/**
 * Every reply of one Bot that waits for its maintainer, grouped by the Chat it was made in, newest Chat first; each is the card of the Chat, with the same decisions,
 * and each Chat can be opened.
 */
export function HeldRepliesDialog({ bot, held, locale, renderMarkdown, onOpenChat, onClose }: {
  bot: Pick<SharedBotCard, 'name'>; held: HeldReplies; locale: string; renderMarkdown?: RenderMarkdown; onOpenChat?(conversationId: string): void; onClose(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const { view } = held;
  const groups: { id: string; title: string; rows: HeldRow[] }[] = [];
  for (const row of view.rows) {
    const group = groups.find(candidate => candidate.id === row.reply.conversationId);
    if (group) group.rows.push(row); else groups.push({ id: row.reply.conversationId, title: row.reply.conversationTitle || t('teams_triage_chat_untitled'), rows: [row] });
  }
  return <Dialog title={t('teams_held_waiting')} onClose={onClose}><div className="tw-form tw-held-list">
    <p className="tw-hint">{t('teams_held_intro')}</p>
    <p className="tw-triage-scope"><b>{bot.name}</b></p>
    {view.status === 'loading' ? <p role="status">{t('teams_loading')}</p> : null}
    {view.status === 'ready' && !view.rows.length ? <p className="tw-hint">{t('teams_held_none')}</p> : null}
    {groups.map(group => <section key={group.id} className="tw-held-group" aria-label={group.title}>
      <header className="tw-held-group-head">
        <h3>{group.title}</h3>
        {onOpenChat ? <button type="button" onClick={() => { onClose(); onOpenChat(group.id); }}>{t('friends_open_chat')}</button> : null}
      </header>
      <ul className="tw-held-cards">{group.rows.map(row => <li key={row.reply.taskId}>
        <HeldReplyCard row={row} now={view.now} locale={locale} renderMarkdown={renderMarkdown} heading={false}
          onApprove={text => held.approve(row.reply.taskId, text)} onReject={() => held.reject(row.reply.taskId)} onDismiss={() => held.dismiss(row.reply.taskId)} />
      </li>)}</ul>
    </section>)}
    {view.status === 'failed' && view.error ? <p role="alert">{heldReplyErrorText(view.error, locale)}</p> : null}
    {view.status === 'failed' && !view.rows.length ? <div><button type="button" onClick={held.reload}>{t('common_retry')}</button></div> : null}
    <div className="tw-dialog-actions">
      <button type="button" disabled={view.status === 'loading'} onClick={held.reload}>{t('teams_list_refresh')}</button>
      <button type="button" data-autofocus onClick={onClose}>{t('common_close')}</button>
    </div>
  </div></Dialog>;
}
