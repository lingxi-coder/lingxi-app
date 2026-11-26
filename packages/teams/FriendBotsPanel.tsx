import { useEffect, useRef, type KeyboardEvent, type ReactElement } from 'react';
import { BotDirectory } from './BotDirectory';
import { messageErrorText } from './errors.js';
import type { FriendChat } from './friends-workspace.js';
import { Avatar, Icon, type RenderAvatar } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { useTeamsText } from './i18n.js';

/**
 * The Bots of a friendship (D21), a panel over the friend chat: the Bot cards the chat's pair workspace fetched, Message to open a Bot DM
 * (D22) and, on Desktop, sharing and Stop sharing (D23). All of it is the Teams Bot directory in its `friend` mode.
 */
export function FriendBotsPanel({ id, pair, view, chat, locale, renderAvatar, onClose }: {
  /** The id the header's Shared Bots button names in `aria-controls`. */
  id: string; pair: TeamsWorkspace; view: TeamsSnapshot; chat: FriendChat; locale: string; renderAvatar?: RenderAvatar; onClose(): void;
}): ReactElement | null {
  const t = useTeamsText(locale);
  const closeRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { closeRef.current?.focus(); }, []);
  const team = view.team;
  if (!team) return null;
  // A dialog of the directory (the share review, Stop sharing) takes Escape first.
  const escape = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key === 'Escape' && !event.defaultPrevented && !(event.target as HTMLElement).closest('dialog')) { event.stopPropagation(); onClose(); }
  };
  return <section id={id} className="tw-scope tw-friends-bots" aria-label={t('friends_bots_title')} onKeyDown={escape}>
    <header className="tw-drawer-head">
      <Avatar identity={{ kind: 'user', id: chat.uid, name: chat.name }} size={40} render={renderAvatar} />
      <div className="tw-drawer-title"><h2>{t('friends_bots_title')}</h2><p>{chat.name}</p></div>
      <button ref={closeRef} type="button" className="tw-icon-btn" aria-label={t('common_close')} title={t('common_close')} onClick={onClose}><Icon name="x" /></button>
    </header>
    <div className="tw-friends-bots-body tw-embed">
      <BotDirectory key={`${view.uid}:${team.id}:${team.generation}`} friend={chat.name} api={pair.api} uid={view.uid} team={team} members={view.directory} bots={view.bots} locale={locale} busy={view.messageBusy}
        status={view.botsUnverified ? t('teams_bots_unverified') : !view.botsLoaded ? t('teams_bots_loading') : ''}
        attempt={pair.attempt} onOpen={async bot => { await pair.startBotDm(bot); onClose(); }} onChanged={pair.refreshBots} onOpenChat={id => { pair.selectConversation(id); onClose(); }} />
      {view.actionError ? <p role="alert" className="tw-error">{messageErrorText(view.actionError, locale)}</p> : null}
    </div>
  </section>;
}
