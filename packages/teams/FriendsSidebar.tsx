import { useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { friendsErrorText, offerErrorText } from './errors.js';
import { matchesFilter, friendCollator, offerExpiry, sectionCards, SECTION_ROWS, unreadOf, type FriendCard, type OfferAction, type TeamOffer } from './friends.js';
import type { FriendsSnapshot, FriendsWorkspace } from './friends-workspace.js';
import { formatListTime } from './stream.js';
import { Avatar, Icon, PopupMenu, TeamMark, UnreadBadge, useDocumentShown, type MenuItem, type RenderAvatar } from './ui';
import { pointerPreview } from './workspace.js';

/** What every Friends view takes. */
export interface FriendsViewProps {
  workspace: FriendsWorkspace;
  snapshot: FriendsSnapshot;
  zh: boolean;
  /** Draw your own avatar for a person; return nothing for the initials avatar. */
  renderAvatar?: RenderAvatar;
  /** BCP 47 tag for times and name order (D5); defaults to `zh-CN` or `en-US` from `zh`. */
  locale?: string;
}

export interface FriendsSidebarProps extends FriendsViewProps {
  /** The current time, for the "Yesterday" and weekday labels. Defaults to now. */
  now?: number | Date;
}

/**
 * The Friends root (D3): the header with Add friend and the more menu, the name banner, the team invitations (D27), the requests (D4),
 * the filter and the friends in last-activity order (D5). A friend opens its chat; the two screens behind the menu and Add friend are pages of the workspace.
 */
export function FriendsSidebar({ workspace, snapshot, zh, renderAvatar, locale, now }: FriendsSidebarProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const tag = locale ?? (zh ? 'zh-CN' : 'en-US');
  const collator = useMemo(() => friendCollator(tag), [tag]);
  const sections = useMemo(() => sectionCards(snapshot.cards, collator), [snapshot.cards, collator]);
  const [query, setQuery] = useState('');
  const [menu, setMenu] = useState(false);
  const [notice, setNotice] = useState('');
  const moreRef = useRef<HTMLButtonElement>(null), requestsRef = useRef<HTMLElement>(null);
  const nowMs = now === undefined ? Date.now() : typeof now === 'number' ? now : now.getTime();
  const friends = useMemo(() => sections.friends.filter(card => matchesFilter(card, query)), [sections.friends, query]);
  const requests = sections.received.length + sections.sent.length;
  const searching = query.trim() !== '';
  const shown = useDocumentShown();

  // D28: the badge and the offers' state follow the clock, so they are worked out again whenever the view is shown.
  useEffect(() => { if (shown) workspace.recount(); }, [shown, workspace]);
  useEffect(() => {
    if (snapshot.requestsFocus) requestsRef.current?.scrollIntoView?.({ block: 'start' });
  }, [snapshot.requestsFocus]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(''), 6000);
    return () => clearTimeout(timer);
  }, [notice]);

  const items: MenuItem[] = [
    { id: 'profile', label: t('Profile settings', '个人资料设置'), onSelect: () => workspace.show('profile') },
    { id: 'blocked', label: t('Blocked people', '已屏蔽的人'), onSelect: () => workspace.show('blocked') },
  ];
  const empty = !snapshot.loading && !snapshot.loadError && !snapshot.cards.length && !snapshot.offers.length;

  return <nav className="tw-scope tw-sidebar" aria-label={t('Friends', '好友')}>
    <header className="tw-sidebar-head">
      <h2 className="tw-friends-title">{t('Friends', '好友')}</h2>
      <button type="button" className="tw-primary" onClick={() => workspace.show('add')}><Icon name="plus" size={14} />{t('Add friend', '添加好友')}</button>
      <div className="tw-switcher-wrap tw-friends-more">
        <button ref={moreRef} type="button" className="tw-icon-btn" aria-haspopup="menu" aria-expanded={menu} aria-label={t('More', '更多')} title={t('More', '更多')}
          onClick={() => setMenu(open => !open)}><Icon name="more" /></button>
        {menu ? <PopupMenu label={t('More', '更多')} items={items} anchor={moreRef} align="end"
          onClose={restore => { setMenu(false); if (restore) moreRef.current?.focus(); }} /> : null}
      </div>
    </header>

    {snapshot.profile?.displayName === 'Member' ? <div className="tw-banner" role="status">
      <span>{t('Set your name so friends recognize you.', '设置你的名字，好友才能认出你。')}</span>
      <button type="button" className="tw-link" onClick={() => workspace.show('profile')}>{t('Set name', '设置名字')}</button>
    </div> : null}
    {notice ? <p className="tw-banner" role="status">{notice}</p> : null}

    {snapshot.loading ? <p className="tw-empty-line" role="status">{t('Loading friends…', '正在加载好友…')}</p>
      : snapshot.loadError ? <div className="tw-empty">
        <p role="alert">{t("Couldn't load your friends. Check your connection and try again.", '无法加载好友，请检查网络后重试。')}</p>
        <div className="tw-empty-actions"><button type="button" onClick={workspace.retry}>{t('Retry', '重试')}</button></div>
      </div>
      : empty ? <div className="tw-empty">
        <h2>{t('No friends yet', '还没有好友')}</h2>
        <p>{t('Add someone by @username, email or friend link to chat one-to-one.', '通过 @用户名、邮箱或好友链接添加好友，即可一对一聊天。')}</p>
        <div className="tw-empty-actions"><button type="button" className="tw-primary" onClick={() => workspace.show('add')}>{t('Add friend', '添加好友')}</button></div>
      </div>
      : <div className="tw-sections">
        {snapshot.offers.length ? <section ref={requestsRef} className="tw-section" aria-labelledby="tw-friends-offers">
          <div className="tw-section-head"><h3 id="tw-friends-offers">{t('Team invitations', '团队邀请')}</h3></div>
          <Offers offers={snapshot.offers} now={snapshot.now} tag={tag} workspace={workspace} zh={zh}
            onJoined={name => setNotice(t(`You joined “${name}”`, `你已加入“${name}”`))} />
        </section> : null}
        {requests ? <section ref={snapshot.offers.length ? undefined : requestsRef} className="tw-section" aria-labelledby="tw-friends-requests">
          <div className="tw-section-head"><h3 id="tw-friends-requests">{t('Friend requests', '好友请求')}</h3></div>
          <Requests title={t('Received', '收到的请求')} cards={sections.received} kind="received" workspace={workspace} zh={zh} renderAvatar={renderAvatar}
            onAccepted={name => setNotice(t(`You and ${name} are now friends`, `你和 ${name} 已成为好友`))} />
          <Requests title={t('Sent', '已发送的请求')} cards={sections.sent} kind="sent" workspace={workspace} zh={zh} renderAvatar={renderAvatar} />
        </section> : null}
        <input type="search" className="tw-friends-filter" aria-label={t('Search friends', '搜索好友')} placeholder={t('Search friends', '搜索好友')}
          value={query} onChange={event => setQuery(event.target.value)} />
        {searching && !friends.length ? <p className="tw-empty-line" role="status">{t('No matching friends', '没有匹配的好友')}</p> : null}
        <ul className="tw-rows" aria-label={t('Friends', '好友')}>
          {friends.map(card => <FriendRow key={card.uid} card={card} uid={snapshot.uid} zh={zh} tag={tag} now={nowMs} renderAvatar={renderAvatar}
            selected={snapshot.page === 'chat' && snapshot.chat?.uid === card.uid} onOpen={() => workspace.openChat(card.uid)} />)}
        </ul>
      </div>}
  </nav>;
}

function FriendRow({ card, uid, zh, tag, now, renderAvatar, selected, onOpen }: {
  card: FriendCard; uid: string; zh: boolean; tag: string; now: number; renderAvatar?: RenderAvatar; selected: boolean; onOpen(): void;
}): ReactElement {
  const unread = unreadOf(card);
  const time = formatListTime(card.lastMessage?.at, now, tag);
  return <li className="tw-row" data-selected={selected} data-unread={unread > 0}>
    <button type="button" className="tw-row-main" aria-current={selected ? 'page' : undefined} onClick={onOpen}>
      <span className="tw-row-lead"><Avatar identity={{ kind: 'user', id: card.uid, name: card.displayName }} size={32} render={renderAvatar} /></span>
      <span className="tw-row-text">
        <span className="tw-row-top"><span className="tw-row-title">{card.displayName}</span>{time ? <time className="tw-row-time" dateTime={card.lastMessage?.at ?? undefined}>{time}</time> : null}</span>
        <span className="tw-row-bottom">
          <span className="tw-row-preview">{card.lastMessage ? pointerPreview(card.lastMessage, uid, zh) : zh ? '暂无消息' : 'No messages yet'}</span>
          <UnreadBadge count={unread} zh={zh} />
        </span>
      </span>
    </button>
  </li>;
}

/** One group of requests (D4): at most 20 rows, then "Show all (n)". A row is disabled while one of its actions runs. */
function Requests({ title, cards, kind, workspace, zh, renderAvatar, onAccepted }: {
  title: string; cards: FriendCard[]; kind: 'received' | 'sent'; workspace: FriendsWorkspace; zh: boolean; renderAvatar?: RenderAvatar; onAccepted?(name: string): void;
}): ReactElement | null {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [all, setAll] = useState(false);
  const [busy, setBusy] = useState<string>();
  const [failed, setFailed] = useState<{ uid: string; code: string }>();
  if (!cards.length) return null;
  const shown = all ? cards : cards.slice(0, SECTION_ROWS);
  const act = async (card: FriendCard, action: 'accept' | 'decline' | 'cancel') => {
    setBusy(card.uid); setFailed(undefined);
    const code = await workspace.respond(card.uid, action);
    setBusy(undefined);
    if (code) setFailed({ uid: card.uid, code }); else if (action === 'accept') onAccepted?.(card.displayName);
  };
  return <div className="tw-friends-requests">
    <h4 className="tw-friends-group">{title}</h4>
    <ul className="tw-people" aria-label={title}>
      {shown.map(card => <li key={card.uid} className="tw-request" aria-busy={busy === card.uid}>
        <Avatar identity={{ kind: 'user', id: card.uid, name: card.displayName }} size={32} render={renderAvatar} />
        <div className="tw-person-text">
          <b>{card.displayName}</b>
          {card.handle ? <small>@{card.handle}</small> : null}
          {card.note ? <small className="tw-request-note">{card.note}</small> : null}
          {kind === 'sent' ? <small>{t('Request sent', '请求已发送')}</small> : null}
          {failed?.uid === card.uid ? <small role="alert" className="tw-error">{friendsErrorText(failed.code, zh)}</small> : null}
        </div>
        <div className="tw-person-actions">
          {kind === 'received' ? <>
            <button type="button" className="tw-primary" disabled={busy === card.uid} onClick={() => void act(card, 'accept')}>{t('Accept', '接受')}</button>
            <button type="button" disabled={busy === card.uid} onClick={() => void act(card, 'decline')}>{t('Decline', '拒绝')}</button>
          </> : <button type="button" disabled={busy === card.uid} onClick={() => void act(card, 'cancel')}>{t('Cancel request', '撤销请求')}</button>}
        </div>
      </li>)}
    </ul>
    {!all && cards.length > SECTION_ROWS ? <button type="button" className="tw-link tw-more" onClick={() => setAll(true)}>{t(`Show all (${cards.length})`, `显示全部（${cards.length}）`)}</button> : null}
  </div>;
}

/**
 * The team invitations (D27): at most 20 rows, then "Show all (n)". Join and Decline for a live offer, Dismiss for an expired one; a row is
 * disabled while one of its actions runs, and what a failed action says (D30) stays under the row.
 */
function Offers({ offers, now, tag, workspace, zh, onJoined }: {
  offers: readonly TeamOffer[]; now: number; tag: string; workspace: FriendsWorkspace; zh: boolean; onJoined(teamName: string): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [all, setAll] = useState(false);
  const [busy, setBusy] = useState<string>();
  const [failed, setFailed] = useState<{ teamId: string; action: OfferAction; code: string }>();
  const shown = all ? offers : offers.slice(0, SECTION_ROWS);
  const act = async (offer: TeamOffer, action: 'acceptOffer' | 'declineOffer') => {
    setBusy(offer.teamId); setFailed(undefined);
    const code = await (action === 'acceptOffer' ? workspace.acceptOffer(offer.teamId) : workspace.declineOffer(offer.teamId));
    setBusy(undefined);
    if (code) setFailed({ teamId: offer.teamId, action, code }); else if (action === 'acceptOffer') onJoined(offer.teamName);
  };
  const relative = useMemo(() => new Intl.RelativeTimeFormat(tag, { numeric: 'always' }), [tag]);
  return <div className="tw-friends-requests">
    <ul className="tw-people" aria-label={t('Team invitations', '团队邀请')}>
      {shown.map(offer => {
        const expiry = offerExpiry(offer.expiresAt, now);
        const when = expiry.state === 'live' ? relative.format(expiry.value, expiry.unit) : '';
        return <li key={offer.teamId} className="tw-request" aria-busy={busy === offer.teamId}>
          <TeamMark id={offer.teamId} name={offer.teamName} />
          <div className="tw-person-text">
            <b>{offer.teamName}</b>
            <small>{t(`Invited by ${offer.fromName}`, `邀请人：${offer.fromName}`)}</small>
            <small>{expiry.state === 'live' ? t(`Expires ${when}`, `${when} 到期`) : t('Expired', '已过期')}</small>
            {failed?.teamId === offer.teamId ? <small role="alert" className="tw-error">{offerErrorText(failed.action, failed.code, zh)}</small> : null}
          </div>
          <div className="tw-person-actions">
            {expiry.state === 'live' ? <>
              <button type="button" className="tw-primary" disabled={busy === offer.teamId} onClick={() => void act(offer, 'acceptOffer')}>{t('Join', '加入')}</button>
              <button type="button" disabled={busy === offer.teamId} onClick={() => void act(offer, 'declineOffer')}>{t('Decline', '拒绝')}</button>
            </> : <button type="button" disabled={busy === offer.teamId} onClick={() => void act(offer, 'declineOffer')}>{t('Dismiss', '移除')}</button>}
          </div>
        </li>;
      })}
    </ul>
    {!all && offers.length > SECTION_ROWS ? <button type="button" className="tw-link tw-more" onClick={() => setAll(true)}>{t(`Show all (${offers.length})`, `显示全部（${offers.length}）`)}</button> : null}
  </div>;
}
