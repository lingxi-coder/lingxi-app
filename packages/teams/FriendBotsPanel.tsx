import { useEffect, useRef, type KeyboardEvent, type ReactElement } from 'react';
import { BotDirectory } from './BotDirectory';
import { messageErrorText } from './errors.js';
import type { FriendChat } from './friends-workspace.js';
import { Icon } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

/**
 * The Bots of a friendship (D21), a panel over the friend chat: the Bot cards the chat's pair workspace fetched, Message to open a Bot DM
 * (D22) and, on Desktop, sharing and Stop sharing (D23). All of it is the Teams Bot directory in its `friend` mode.
 */
export function FriendBotsPanel({ pair, view, chat, zh, onClose }: {
  pair: TeamsWorkspace; view: TeamsSnapshot; chat: FriendChat; zh: boolean; onClose(): void;
}): ReactElement | null {
  const t = (en: string, cn: string) => zh ? cn : en;
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { closeRef.current?.focus(); }, []);
  const team = view.team;
  if (!team) return null;
  // A dialog of the directory (the share review, Stop sharing) takes Escape first.
  const escape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && !event.defaultPrevented && !(event.target as HTMLElement).closest('dialog')) { event.stopPropagation(); onClose(); }
  };
  return <section className="tw-scope tw-friends-bots" aria-label={t('Shared Bots', '共享的 Bot')} onKeyDown={escape}>
    <header className="tw-conv-head">
      <div className="tw-conv-title"><h2>{t('Shared Bots', '共享的 Bot')}</h2></div>
      <button ref={closeRef} type="button" className="tw-icon-btn" aria-label={t('Close', '关闭')} title={t('Close', '关闭')} onClick={onClose}><Icon name="x" /></button>
    </header>
    <div className="tw-friends-bots-body tw-embed">
      <BotDirectory key={`${view.uid}:${team.id}:${team.generation}`} friend={chat.name} api={pair.api} uid={view.uid} team={team} members={view.directory} bots={view.bots} zh={zh} busy={view.messageBusy}
        status={view.botsUnverified ? t('Bot availability could not be verified.', '未能确认 Bot 可用状态。') : !view.botsLoaded ? t('Loading Bots…', '正在加载 Bot…') : ''}
        attempt={pair.attempt} onOpen={async bot => { await pair.startBotDm(bot); onClose(); }} onChanged={pair.refreshBots} />
      {view.actionError ? <p role="alert" className="tw-error">{messageErrorText(view.actionError, zh)}</p> : null}
    </div>
  </section>;
}
