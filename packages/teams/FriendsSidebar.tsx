import { useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { friendsErrorText, offerErrorText } from './errors.js';
import { FriendLeaveDialog, FriendProfileDialog } from './friend-dialogs';
import { matchesFilter, friendCollator, LEGACY_FRIEND_VERSION, offerExpiry, sectionCards, SECTION_ROWS, unreadOf, type FriendCard, type OfferAction, type TeamOffer } from './friends.js';
import type { FriendsSnapshot, FriendsWorkspace } from './friends-workspace.js';
import { formatListTime } from './stream.js';
import { Avatar, Icon, PopupMenu, rovingKeys, TeamMark, UnreadBadge, useDocumentShown, type MenuItem, type RenderAvatar } from './ui';
import { pointerPreview } from './workspace.js';
import { intlTag, useTeamsText } from './i18n.js';

/** What every Friends view takes. */
export interface FriendsViewProps {
  workspace: FriendsWorkspace;
  snapshot: FriendsSnapshot;
  /** Draw your own avatar for a person; return nothing for the initials avatar. */
  renderAvatar?: RenderAvatar;
  /** The app's language, as a code or a BCP 47 tag (`en`, `zh-Hans`, `zh-TW`, `ja`, `ko`): it picks the language of the text, the locale of times and the name order (D5). */
  locale: string;
}

export interface FriendsSidebarProps extends FriendsViewProps {
  /** The current time, for the "Yesterday" and weekday labels. Defaults to now. */
  now?: number | Date;
}

/**
 * The Friends root (D3): the header with Add friend and the more menu, the name banner, the team invitations (D27), the requests (D4),
 * the filter and the friends in last-activity order (D5). A friend opens its chat; the two screens behind the menu and Add friend are pages of the workspace.
 * The friends are one tab stop: the arrow keys, Home and End move between the rows, and a row's menu (View profile, Shared Bots, Remove friend)
 * opens from its more button, a right click, the Menu key or Shift+F10.
 */
export function FriendsSidebar({ workspace, snapshot, renderAvatar, locale, now }: FriendsSidebarProps): ReactElement {
  const t = useTeamsText(locale);
  const tag = intlTag(locale);
  const collator = useMemo(() => friendCollator(tag), [tag]);
  const sections = useMemo(() => sectionCards(snapshot.cards, collator), [snapshot.cards, collator]);
  const [query, setQuery] = useState('');
  const [menu, setMenu] = useState(false);
  const [notice, setNotice] = useState('');
  const [menuFor, setMenuFor] = useState(''), [focusId, setFocusId] = useState('');
  const [dialog, setDialog] = useState<{ kind: 'profile' | 'remove'; uid: string }>();
  // The dialog shows the friend as the list has them now (a refreshed name, for one), and is gone with the friendship.
  const dialogCard = dialog && snapshot.cards.find(card => card.uid === dialog.uid && card.state === 'friends');
  const moreRef = useRef<HTMLButtonElement>(null), requestsRef = useRef<HTMLElement>(null);
  const nowMs = now === undefined ? Date.now() : typeof now === 'number' ? now : now.getTime();
  const friends = useMemo(() => sections.friends.filter(card => matchesFilter(card, query)), [sections.friends, query]);
  const requests = sections.received.length + sections.sent.length;
  const searching = query.trim() !== '';
  const shown = useDocumentShown();
  const openUid = snapshot.page === 'chat' ? snapshot.chat?.uid : undefined;
  const tabStop = friends.some(card => card.uid === focusId) ? focusId : friends.some(card => card.uid === openUid) ? openUid! : friends[0]?.uid ?? '';

  // D28: the badge and the offers' state follow the clock, so they are worked out again whenever the view is shown.
  useEffect(() => { if (shown) workspace.recount(); }, [shown, workspace]);
  useEffect(() => {
    if (snapshot.requestsFocus) requestsRef.current?.scrollIntoView?.({ block: 'start' });
  }, [snapshot.requestsFocus]);
  // A row's menu belongs to the list it was opened on.
  useEffect(() => { setMenuFor(''); }, [query]);
  useEffect(() => { if (dialog && !dialogCard) setDialog(undefined); }, [dialog, dialogCard]);
  useEffect(() => {
    if (!notice) return;
    const timer = setTimeout(() => setNotice(''), 6000);
    return () => clearTimeout(timer);
  }, [notice]);

  const items: MenuItem[] = [
    { id: 'profile', label: t('friends_profile_title'), onSelect: () => workspace.show('profile') },
    { id: 'blocked', label: t('friends_blocked_title'), onSelect: () => workspace.show('blocked') },
  ];
  const empty = !snapshot.loading && !snapshot.loadError && !snapshot.cards.length && !snapshot.offers.length;

  return <nav className="tw-scope tw-sidebar" aria-label={t('friends_title')}>
    <header className="tw-sidebar-head">
      <h2 className="tw-friends-title">{t('friends_title')}</h2>
      <button type="button" className="tw-primary" onClick={() => workspace.show('add')}><Icon name="plus" size={14} />{t('friends_add')}</button>
      <div className="tw-switcher-wrap tw-friends-more">
        <button ref={moreRef} type="button" className="tw-icon-btn" aria-haspopup="menu" aria-expanded={menu} aria-label={t('tw_more')} title={t('tw_more')}
          onClick={() => setMenu(open => !open)}><Icon name="more" /></button>
        {menu ? <PopupMenu label={t('tw_more')} items={items} anchor={moreRef} align="end"
          onClose={restore => { setMenu(false); if (restore) moreRef.current?.focus(); }} /> : null}
      </div>
    </header>

    {snapshot.profileNameUnset ? <div className="tw-banner" role="status">
      <span>{t('friends_name_prompt')}</span>
      <button type="button" className="tw-link" onClick={() => workspace.show('profile')}>{t('friends_name_prompt_action')}</button>
    </div> : null}
    {notice ? <p className="tw-banner" role="status">{notice}</p> : null}

    {snapshot.loading ? <p className="tw-empty-line" role="status">{t('friends_loading')}</p>
      : snapshot.loadError ? <div className="tw-empty">
        <p role="alert">{t('friends_load_error')}</p>
        <div className="tw-empty-actions"><button type="button" onClick={workspace.retry}>{t('common_retry')}</button></div>
      </div>
      : empty ? <div className="tw-empty">
        <h2>{t('friends_empty_title')}</h2>
        <p>{t('friends_empty_body')}</p>
        <div className="tw-empty-actions"><button type="button" className="tw-primary" onClick={() => workspace.show('add')}>{t('friends_add')}</button></div>
      </div>
      : <div className="tw-sections" onKeyDown={rovingKeys}>
        {snapshot.offers.length ? <section ref={requestsRef} className="tw-section" aria-labelledby="tw-friends-offers">
          <div className="tw-section-head"><h3 id="tw-friends-offers">{t('friends_section_offers')}</h3></div>
          <Offers offers={snapshot.offers} now={snapshot.now} tag={tag} workspace={workspace} locale={locale}
            onJoined={name => setNotice(t('friends_offer_joined_fmt', name))} />
        </section> : null}
        {requests ? <section ref={snapshot.offers.length ? undefined : requestsRef} className="tw-section" aria-labelledby="tw-friends-requests">
          <div className="tw-section-head"><h3 id="tw-friends-requests">{t('friends_section_requests')}</h3></div>
          <Requests title={t('friends_requests_received')} cards={sections.received} kind="received" workspace={workspace} locale={locale} renderAvatar={renderAvatar}
            onAccepted={name => setNotice(t('friends_now_friends_fmt', name))} />
          <Requests title={t('friends_requests_sent')} cards={sections.sent} kind="sent" workspace={workspace} locale={locale} renderAvatar={renderAvatar} />
        </section> : null}
        <input type="search" className="tw-friends-filter" aria-label={t('friends_filter_placeholder')} placeholder={t('friends_filter_placeholder')}
          value={query} onChange={event => setQuery(event.target.value)} />
        {searching && !friends.length ? <p className="tw-empty-line" role="status">{t('friends_filter_empty')}</p> : null}
        <ul className="tw-rows" aria-label={t('friends_title')}>
          {friends.map(card => <FriendRow key={card.uid} card={card} uid={snapshot.uid} locale={locale} tag={tag} now={nowMs} renderAvatar={renderAvatar}
            selected={card.uid === openUid} tabStop={card.uid === tabStop} menuOpen={menuFor === card.uid} onMenu={setMenuFor} onFocusRow={setFocusId}
            onOpen={() => workspace.openChat(card.uid)} onBots={() => workspace.openChat(card.uid, false, true)}
            onProfile={() => setDialog({ kind: 'profile', uid: card.uid })} onRemove={() => setDialog({ kind: 'remove', uid: card.uid })} />)}
        </ul>
      </div>}

    {dialog?.kind === 'profile' && dialogCard ? <FriendProfileDialog workspace={workspace} uid={dialogCard.uid} name={dialogCard.displayName} handle={dialogCard.handle}
      since={dialogCard.since} locale={tag} renderAvatar={renderAvatar} onClose={() => setDialog(undefined)} /> : null}
    {dialog?.kind === 'remove' && dialogCard ? <FriendLeaveDialog workspace={workspace} locale={locale} uid={dialogCard.uid} teamId={dialogCard.teamId} name={dialogCard.displayName} action="remove"
      onClose={() => setDialog(undefined)} /> : null}
  </nav>;
}

function FriendRow({ card, uid, locale, tag, now, renderAvatar, selected, tabStop, menuOpen, onMenu, onFocusRow, onOpen, onBots, onProfile, onRemove }: {
  card: FriendCard; uid: string; locale: string; tag: string; now: number; renderAvatar?: RenderAvatar; selected: boolean; tabStop: boolean; menuOpen: boolean;
  onMenu(uid: string): void; onFocusRow(uid: string): void; onOpen(): void; onBots(): void; onProfile(): void; onRemove(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const mainRef = useRef<HTMLButtonElement>(null), moreRef = useRef<HTMLButtonElement>(null), origin = useRef<'main' | 'more'>('more');
  const unread = unreadOf(card);
  const time = formatListTime(card.lastMessage?.at, now, tag);
  const items: MenuItem[] = [
    { id: 'profile', label: t('friends_menu_view_profile'), onSelect: onProfile },
    { id: 'bots', label: t('friends_bots_title'), onSelect: onBots },
    { id: 'remove', label: t('friends_menu_remove'), danger: true, separated: true, onSelect: onRemove },
  ];
  const open = (from: 'main' | 'more') => { origin.current = from; onMenu(menuOpen ? '' : card.uid); };
  return <li className="tw-row" data-selected={selected} data-unread={unread > 0}>
    {/* The browser raises `contextmenu` for the Menu key and Shift+F10 on the focused row too. */}
    <button ref={mainRef} type="button" className="tw-row-main" data-row={card.uid} aria-current={selected ? 'page' : undefined} tabIndex={tabStop ? 0 : -1}
      onClick={onOpen} onFocus={() => onFocusRow(card.uid)} onContextMenu={event => { event.preventDefault(); open('main'); }}>
      <span className="tw-row-lead"><Avatar identity={{ kind: 'user', id: card.uid, name: card.displayName }} size={40} render={renderAvatar} /></span>
      <span className="tw-row-text">
        <span className="tw-row-top"><span className="tw-row-title">{card.displayName}</span>{time ? <time className="tw-row-time" dateTime={card.lastMessage?.at ?? undefined}>{time}</time> : null}</span>
        <span className="tw-row-bottom">
          <span className="tw-row-preview">{card.lastMessage ? pointerPreview(card.lastMessage, uid, locale) : t('teams_preview_none')}</span>
          <UnreadBadge count={unread} locale={locale} />
        </span>
      </span>
    </button>
    <button ref={moreRef} type="button" className="tw-row-more" aria-label={`${t('tw_more')}: ${card.displayName}`} title={t('tw_more')}
      aria-haspopup="menu" aria-expanded={menuOpen} tabIndex={tabStop ? 0 : -1} onClick={() => open('more')}><Icon name="more" /></button>
    {menuOpen ? <PopupMenu label={card.displayName} items={items} anchor={moreRef} align="end"
      onClose={restore => { onMenu(''); if (restore) (origin.current === 'more' ? moreRef : mainRef).current?.focus(); }} /> : null}
  </li>;
}

/** One group of requests (D4): at most 20 rows, then "Show all (n)". A row is disabled while one of its actions runs. */
function Requests({ title, cards, kind, workspace, locale, renderAvatar, onAccepted }: {
  title: string; cards: FriendCard[]; kind: 'received' | 'sent'; workspace: FriendsWorkspace; locale: string; renderAvatar?: RenderAvatar; onAccepted?(name: string): void;
}): ReactElement | null {
  const t = useTeamsText(locale);
  const [all, setAll] = useState(false);
  const [busy, setBusy] = useState<string>();
  const [failed, setFailed] = useState<{ uid: string; code: string }>();
  if (!cards.length) return null;
  const shown = all ? cards : cards.slice(0, SECTION_ROWS);
  const act = async (card: FriendCard, action: 'accept' | 'decline' | 'cancel') => {
    setBusy(card.uid); setFailed(undefined);
    const code = await workspace.respond(card.uid, action, card.version ?? LEGACY_FRIEND_VERSION);
    setBusy(undefined);
    if (code) setFailed({ uid: card.uid, code }); else if (action === 'accept') onAccepted?.(card.displayName);
  };
  return <div className="tw-friends-requests">
    <h4 className="tw-friends-group">{title}</h4>
    <ul className="tw-people" aria-label={title}>
      {shown.map(card => <li key={card.uid} className="tw-request" aria-busy={busy === card.uid}>
        <Avatar identity={{ kind: 'user', id: card.uid, name: card.displayName }} size={40} render={renderAvatar} />
        <div className="tw-person-text">
          <b>{card.displayName}</b>
          {card.handle ? <small>@{card.handle}</small> : null}
          {card.note ? <small className="tw-request-note">{card.note}</small> : null}
          {kind === 'sent' ? <small>{t('friends_request_sent')}</small> : null}
          {failed?.uid === card.uid ? <small role="alert" className="tw-error">{friendsErrorText(failed.code, locale)}</small> : null}
        </div>
        <div className="tw-person-actions">
          {kind === 'received' ? <>
            <button type="button" className="tw-primary" disabled={busy === card.uid} onClick={() => void act(card, 'accept')}>{t('friends_request_accept')}</button>
            <button type="button" disabled={busy === card.uid} onClick={() => void act(card, 'decline')}>{t('friends_request_decline')}</button>
          </> : <button type="button" disabled={busy === card.uid} onClick={() => void act(card, 'cancel')}>{t('friends_request_cancel')}</button>}
        </div>
      </li>)}
    </ul>
    {!all && cards.length > SECTION_ROWS ? <button type="button" className="tw-link tw-more" onClick={() => setAll(true)}>{t('friends_show_all_fmt', cards.length)}</button> : null}
  </div>;
}

/**
 * The team invitations (D27): at most 20 rows, then "Show all (n)". Join and Decline for a live offer, Dismiss for an expired one; a row is
 * disabled while one of its actions runs, and what a failed action says (D30) stays under the row.
 */
function Offers({ offers, now, tag, workspace, locale, onJoined }: {
  offers: readonly TeamOffer[]; now: number; tag: string; workspace: FriendsWorkspace; locale: string; onJoined(teamName: string): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const [all, setAll] = useState(false);
  const [busy, setBusy] = useState<string>();
  const [failed, setFailed] = useState<{ teamId: string; action: OfferAction; code: string }>();
  const shown = all ? offers : offers.slice(0, SECTION_ROWS);
  const act = async (offer: TeamOffer, action: 'acceptOffer' | 'declineOffer') => {
    setBusy(offer.teamId); setFailed(undefined);
    const code = await (action === 'acceptOffer' ? workspace.acceptOffer(offer.teamId, offer.version ?? LEGACY_FRIEND_VERSION) : workspace.declineOffer(offer.teamId, offer.version ?? LEGACY_FRIEND_VERSION));
    setBusy(undefined);
    if (code) setFailed({ teamId: offer.teamId, action, code }); else if (action === 'acceptOffer') onJoined(offer.teamName);
  };
  const relative = useMemo(() => new Intl.RelativeTimeFormat(tag, { numeric: 'always' }), [tag]);
  return <div className="tw-friends-requests">
    <ul className="tw-people" aria-label={t('friends_section_offers')}>
      {shown.map(offer => {
        const expiry = offerExpiry(offer.expiresAt, now);
        const when = expiry.state === 'live' ? relative.format(expiry.value, expiry.unit) : '';
        return <li key={offer.teamId} className="tw-request" aria-busy={busy === offer.teamId}>
          <TeamMark id={offer.teamId} name={offer.teamName} size={40} />
          <div className="tw-person-text">
            <b>{offer.teamName}</b>
            <small>{t('friends_offer_invited_by_fmt', offer.fromName)}</small>
            <small>{expiry.state === 'live' ? t('friends_offer_expires_fmt', when) : t('friends_offer_expired')}</small>
            {failed?.teamId === offer.teamId ? <small role="alert" className="tw-error">{offerErrorText(failed.action, failed.code, locale)}</small> : null}
          </div>
          <div className="tw-person-actions">
            {expiry.state === 'live' ? <>
              <button type="button" className="tw-primary" disabled={busy === offer.teamId} onClick={() => void act(offer, 'acceptOffer')}>{t('friends_offer_join')}</button>
              <button type="button" disabled={busy === offer.teamId} onClick={() => void act(offer, 'declineOffer')}>{t('friends_request_decline')}</button>
            </> : <button type="button" disabled={busy === offer.teamId} onClick={() => void act(offer, 'declineOffer')}>{t('friends_offer_dismiss')}</button>}
          </div>
        </li>;
      })}
    </ul>
    {!all && offers.length > SECTION_ROWS ? <button type="button" className="tw-link tw-more" onClick={() => setAll(true)}>{t('friends_show_all_fmt', offers.length)}</button> : null}
  </div>;
}
