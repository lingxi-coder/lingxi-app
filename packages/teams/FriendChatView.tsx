import { useEffect, useId, useRef, useState, useSyncExternalStore, type ReactElement } from 'react';
import { nowFriends } from './AddFriendView';
import { FriendLeaveDialog, FriendProfileDialog } from './friend-dialogs';
import { FriendBotsPanel } from './FriendBotsPanel';
import { chatEnded, FRIEND_CHAT } from './friends.js';
import type { FriendChat } from './friends-workspace.js';
import type { FriendsViewProps } from './FriendsSidebar';
import { CreationBanner, TeamConversationView } from './TeamConversationView';
import { Icon, PopupMenu, type MenuItem, type RenderMarkdown } from './ui';
import type { TeamsWorkspace } from './workspace.js';

export interface FriendChatViewProps extends FriendsViewProps {
  renderMarkdown?: RenderMarkdown;
  /** The current time for "Today" and "Yesterday". Defaults to now. */
  now?: number | Date;
}

/**
 * A friend's chat (D9): the Team conversation view on the pair-pinned workspace, with the friend in the header and none of the
 * Team affordances. The header's Bots action opens the friendship's Bots (D21), whose Bot chats (D22) use the same view. An ended
 * friendship replaces all of it with `friends_chat_unavailable` and a Back action.
 */
export function FriendChatView(props: FriendChatViewProps): ReactElement | null {
  const { workspace, snapshot, zh } = props;
  const chat = snapshot.chat;
  if (!chat) return null;
  if (chat.ended) return <Unavailable zh={zh} onBack={() => workspace.show('home')} />;
  if (!chat.workspace) return <section className="tw-scope tw-conv tw-conv-empty"><p className="tw-empty-line" role="status">{zh ? '正在加载…' : 'Loading…'}</p></section>;
  // Another friend is another chat: what the view holds (the open panel, the welcome) is not carried over.
  return <PairChat key={chat.uid} {...props} chat={chat} pair={chat.workspace} />;
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
  const t = (en: string, cn: string) => zh ? cn : en;
  const view = useSyncExternalStore(pair.subscribe, pair.getSnapshot);
  const [welcome, setWelcome] = useState(chat.joined);
  const [panel, setPanel] = useState(false);
  const botsRef = useRef<HTMLButtonElement>(null), botsId = useId();
  // The friend's row menu asks for the Shared Bots with the chat (`FriendChat.botsRequest`).
  useEffect(() => { if (chat.botsRequest) setPanel(true); }, [chat.botsRequest]);
  useEffect(() => {
    if (!welcome) return;
    const timer = setTimeout(() => setWelcome(false), 6000);
    return () => clearTimeout(timer);
  }, [welcome]);
  const back = () => workspace.show('home');
  // D9, D25: a send, a reconnect or a Bot command that finds the friendship over, or a server-confirmed directory without the chat.
  const gone = chatEnded(view.messagingError) || chatEnded(view.actionError) || chatEnded(view.botsError)
    || (!view.conversations.some(value => value.id === FRIEND_CHAT) && !view.messagingConnecting && !view.offline);
  if (gone) return <Unavailable zh={zh} onBack={back} />;
  const conversation = view.conversation;
  const toChat = () => pair.selectConversation(FRIEND_CHAT);
  if (!conversation) {
    // A Bot chat being created is not in the directory yet, and one that was discarded or never appeared leaves a selection with nothing behind it.
    return <section className="tw-scope tw-conv tw-conv-empty">
      {view.creation ? <CreationBanner workspace={pair} snapshot={view} zh={zh} /> : <p className="tw-empty-line" role="status">{t('Loading…', '正在加载…')}</p>}
      {view.selectedConversationId !== FRIEND_CHAT ? <div className="tw-empty-actions"><button type="button" onClick={toChat}>{t('Back', '返回')}</button></div> : null}
    </section>;
  }
  const friendChat = conversation.id === FRIEND_CHAT;
  const close = () => { setPanel(false); botsRef.current?.focus(); };
  // D22: the friend's header and menu belong to the friend chat alone; a Bot chat is the plain Teams view with a way back.
  const actions = <>
    <button ref={botsRef} type="button" className="tw-icon-btn" aria-expanded={panel} aria-controls={botsId} aria-label={t('Shared Bots', '共享的 Bot')} title={t('Shared Bots', '共享的 Bot')}
      onClick={() => setPanel(open => !open)}><Icon name="bot" /></button>
    <FriendMenu workspace={workspace} snapshot={snapshot} zh={zh} chat={chat} locale={locale} renderAvatar={renderAvatar} />
  </>;
  return <div className="tw-friends-chat">
    {welcome ? <p className="tw-banner" role="status">{nowFriends(chat.name, zh)}</p> : null}
    <TeamConversationView workspace={pair} snapshot={view} zh={zh} renderAvatar={renderAvatar} renderMarkdown={renderMarkdown} locale={locale} now={now}
      friend={friendChat ? { uid: chat.uid, name: chat.name, handle: chat.handle, actions } : undefined} onBack={friendChat ? undefined : toChat} />
    {panel && friendChat ? <FriendBotsPanel id={botsId} pair={pair} view={view} chat={chat} zh={zh} onClose={close} /> : null}
  </div>;
}

/** D10: View profile, Remove friend, Block. */
function FriendMenu({ workspace, snapshot, zh, chat, locale, renderAvatar }: FriendsViewProps & { chat: FriendChat }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [menu, setMenu] = useState(false);
  const [dialog, setDialog] = useState<'profile' | 'remove' | 'block'>();
  const moreRef = useRef<HTMLButtonElement>(null);
  const card = snapshot.cards.find(value => value.uid === chat.uid);
  const items: MenuItem[] = [
    { id: 'profile', label: t('View profile', '查看资料'), onSelect: () => setDialog('profile') },
    { id: 'remove', label: t('Remove friend', '删除好友'), danger: true, separated: true, onSelect: () => setDialog('remove') },
    { id: 'block', label: t('Block', '屏蔽'), danger: true, onSelect: () => setDialog('block') },
  ];
  const close = () => setDialog(undefined);
  return <div className="tw-switcher-wrap tw-friends-more">
    <button ref={moreRef} type="button" className="tw-icon-btn" aria-haspopup="menu" aria-expanded={menu} aria-label={t('More', '更多')} title={t('More', '更多')}
      onClick={() => setMenu(value => !value)}><Icon name="more" /></button>
    {menu ? <PopupMenu label={chat.name} items={items} anchor={moreRef} align="end" onClose={restore => { setMenu(false); if (restore) moreRef.current?.focus(); }} /> : null}
    {dialog === 'profile' ? <FriendProfileDialog workspace={workspace} zh={zh} uid={chat.uid} name={chat.name} handle={chat.handle} since={card?.since}
      locale={locale} renderAvatar={renderAvatar} onClose={close} /> : null}
    {dialog === 'remove' || dialog === 'block' ? <FriendLeaveDialog workspace={workspace} zh={zh} uid={chat.uid} name={chat.name} action={dialog} onClose={close} /> : null}
  </div>;
}
