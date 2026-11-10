import { useEffect, useRef, useState, useSyncExternalStore, type ReactElement } from 'react';
import { nowFriends } from './AddFriendView';
import { friendsErrorText } from './errors.js';
import { chatEnded } from './friends.js';
import type { FriendChat } from './friends-workspace.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { TeamConversationView } from './TeamConversationView';
import { Dialog, Icon, PopupMenu, type MenuItem, type RenderMarkdown } from './ui';
import type { TeamsWorkspace } from './workspace.js';

export interface FriendChatViewProps extends FriendsViewProps {
  renderMarkdown?: RenderMarkdown;
  /** The current time for "Today" and "Yesterday". Defaults to now. */
  now?: number | Date;
}

/**
 * A friend's chat (D9): the Team conversation view on the pair-pinned workspace, with the friend in the header and none of the
 * Team affordances. An ended friendship replaces the chat with `friends_chat_unavailable` and a Back action.
 */
export function FriendChatView(props: FriendChatViewProps): ReactElement | null {
  const { workspace, snapshot, zh } = props;
  const chat = snapshot.chat;
  if (!chat) return null;
  if (chat.ended) return <Unavailable zh={zh} onBack={() => workspace.show('home')} />;
  if (!chat.workspace) return <section className="tw-scope tw-conv tw-conv-empty"><p className="tw-empty-line" role="status">{zh ? '正在加载…' : 'Loading…'}</p></section>;
  return <PairChat {...props} chat={chat} pair={chat.workspace} />;
}

function Unavailable({ zh, onBack }: { zh: boolean; onBack(): void }): ReactElement {
  return <section className="tw-scope tw-conv tw-conv-empty">
    <div className="tw-empty">
      <h2>{zh ? '此聊天已不可用。' : 'This chat is no longer available.'}</h2>
      <div className="tw-empty-actions"><button type="button" onClick={onBack}>{zh ? '返回' : 'Back'}</button></div>
    </div>
  </section>;
}

function PairChat({ workspace, snapshot, zh, renderAvatar, renderMarkdown, locale, now, chat, pair }: FriendChatViewProps & { chat: FriendChat; pair: TeamsWorkspace }): ReactElement {
  const view = useSyncExternalStore(pair.subscribe, pair.getSnapshot);
  const [welcome, setWelcome] = useState(chat.joined);
  useEffect(() => {
    if (!welcome) return;
    const timer = setTimeout(() => setWelcome(false), 6000);
    return () => clearTimeout(timer);
  }, [welcome]);
  const back = () => workspace.show('home');
  // D9: a send or a reconnect that finds the friendship over, or a server-confirmed directory without the conversation.
  const gone = chatEnded(view.messagingError) || chatEnded(view.actionError) || (!view.conversation && !view.messagingConnecting && !view.offline);
  if (gone) return <Unavailable zh={zh} onBack={back} />;
  if (!view.conversation) return <section className="tw-scope tw-conv tw-conv-empty"><p className="tw-empty-line" role="status">{zh ? '正在加载…' : 'Loading…'}</p></section>;
  return <div className="tw-friends-chat">
    {welcome ? <p className="tw-banner" role="status">{nowFriends(chat.name, zh)}</p> : null}
    <TeamConversationView workspace={pair} snapshot={view} zh={zh} renderAvatar={renderAvatar} renderMarkdown={renderMarkdown} locale={locale} now={now}
      friend={{ uid: chat.uid, name: chat.name, handle: chat.handle, actions: <FriendMenu workspace={workspace} snapshot={snapshot} zh={zh} chat={chat} locale={locale} /> }} />
  </div>;
}

/** D10: View profile, Remove friend, Block. */
function FriendMenu({ workspace, snapshot, zh, chat, locale }: FriendsViewProps & { chat: FriendChat }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [menu, setMenu] = useState(false);
  const [dialog, setDialog] = useState<'profile' | 'remove' | 'block'>();
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const moreRef = useRef<HTMLButtonElement>(null);
  const card = snapshot.cards.find(value => value.uid === chat.uid);
  const open = (next: 'profile' | 'remove' | 'block') => { setDialog(next); setError(''); if (next === 'profile') void workspace.refreshFriend(chat.uid); };
  const items: MenuItem[] = [
    { id: 'profile', label: t('View profile', '查看资料'), onSelect: () => open('profile') },
    { id: 'remove', label: t('Remove friend', '删除好友'), danger: true, separated: true, onSelect: () => open('remove') },
    { id: 'block', label: t('Block', '屏蔽'), danger: true, onSelect: () => open('block') },
  ];
  const leave = async (action: 'remove' | 'block') => {
    setBusy(true); setError('');
    const code = await workspace.leave(chat.uid, action);
    setBusy(false); setError(code);
  };
  const tag = locale ?? (zh ? 'zh-CN' : 'en-US');
  const since = card ? new Intl.DateTimeFormat(tag, { dateStyle: 'medium' }).format(card.since) : '';
  return <div className="tw-switcher-wrap tw-friends-more">
    <button ref={moreRef} type="button" className="tw-icon-btn" aria-haspopup="menu" aria-expanded={menu} aria-label={t('More', '更多')} title={t('More', '更多')}
      onClick={() => setMenu(value => !value)}><Icon name="more" /></button>
    {menu ? <PopupMenu label={chat.name} items={items} anchor={moreRef} align="end" onClose={restore => { setMenu(false); if (restore) moreRef.current?.focus(); }} /> : null}
    {dialog === 'profile' ? <Dialog title={chat.name} onClose={() => setDialog(undefined)}>
      <div className="tw-form">
        {chat.handle ? <p>@{chat.handle}</p> : null}
        {since ? <p className="tw-hint">{zh ? `${since} 起成为好友` : `Friends since ${since}`}</p> : null}
        <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setDialog(undefined)}>{t('Close', '关闭')}</button></div>
      </div>
    </Dialog> : null}
    {dialog === 'remove' || dialog === 'block' ? <Dialog role="alertdialog" title={dialog === 'remove' ? t(`Remove “${chat.name}”?`, `删除好友“${chat.name}”？`) : t(`Block “${chat.name}”?`, `屏蔽“${chat.name}”？`)} onClose={() => setDialog(undefined)}>
      <div className="tw-form">
        <p>{dialog === 'remove'
          ? t('You will no longer be friends, and the chat is deleted for both of you. You can add each other again later.', '你们将不再是好友，聊天记录会对双方删除。之后可以重新添加。')
          : t("They are also removed from your friends and the chat is deleted for both of you. Until you unblock them, they can't find you or send you requests.", '同时会将对方从好友中移除，并对双方删除聊天记录。在你解除屏蔽之前，对方无法找到你或向你发送请求。')}</p>
        {error ? <p role="alert" className="tw-error">{friendsErrorText(error, zh)}</p> : null}
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setDialog(undefined)}>{t('Cancel', '取消')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void leave(dialog)}>{dialog === 'remove' ? t('Remove friend', '删除好友') : t('Block', '屏蔽')}</button>
        </div>
      </div>
    </Dialog> : null}
  </div>;
}
