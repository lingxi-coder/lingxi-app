import { useContext, useEffect, useMemo, useRef, useState, type ReactElement } from 'react';
import { ChatTopbarTarget, connectionStatus } from './ChatTopbar';
import { pendingPayload, type TeamConversation } from './messages.js';
import { snippet } from './compose.js';
import { NewBotDmDialog, NewConversationDialog } from './NewConversationDialog';
import { formatListTime } from './stream.js';
import { CreateTeamDialog, JoinTeamDialog } from './team-dialogs';
import { ConversationLead, Icon, platformNotice, PopupMenu, rovingKeys, TeamMark, UnreadBadge, type MenuItem, type RenderAvatar } from './ui';
import { conversationLabel, conversationPreview, groupConversations, unreadCount, type TeamsSnapshot, type TeamsWorkspace } from './workspace.js';
import { intlTag, useTeamsText } from './i18n.js';

export { connectionStatus, type ConnectionStatus } from './ChatTopbar';

/** Rows shown per section before "Show more". The open conversation is always shown. */
export const ROW_LIMIT = 50;

export interface TeamSidebarProps {
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  /** Draw your own avatar for a person or Bot; return nothing for the initials avatar. */
  renderAvatar?: RenderAvatar;
  /** Open team settings from the existing menu and return focus to the switcher on Back. */
  onOpenSettings?(opener?: HTMLButtonElement): void;
  /** A conversation was chosen, including the one already open. */
  onConversationSelected?(id: string): void;
  /** Show a conversation's details. Without it the row menu has no such entry. */
  onOpenDetails?(id: string): void;
  /** Show Bot services (Bots offered outside Teams). Without it the sidebar has no such entry. */
  onOpenServices?(): void;
  /** An invitation opened from a link. The join dialog opens with it filled in, but never joins by itself. */
  initialInvite?: string;
  onInviteHandled?(): void;
  onInviteResolved?(): void;
  /** The app's language, as a code or a BCP 47 tag (`en`, `zh-Hans`, `zh-TW`, `ja`, `ko`): it picks the language of the text and the locale of times. */
  locale: string;
  /** The current time, for the "Yesterday" and weekday labels. Defaults to now. */
  now?: number | Date;
  /**
   * Narrow the lists to the conversations whose name or newest-message preview contains this text (any case, surrounding
   * space ignored). While it is set every match is listed, with no "Show more".
   */
  query?: string;
}

type DialogState = { kind: 'createTeam' } | { kind: 'joinTeam'; code: string } | { kind: 'channel' } | { kind: 'direct' } | { kind: 'botDm' } | null;

/**
 * The left column of the Teams workspace: team switcher, connection line, and the open team's
 * Channels, Direct messages and Bot chats. It owns only transient UI state (menus, dialogs, the
 * focused row); everything else comes from the workspace.
 */
export function TeamSidebar({ workspace, snapshot, renderAvatar, onOpenSettings, onConversationSelected, onOpenDetails, onOpenServices, initialInvite, onInviteHandled, onInviteResolved, locale, now, query }: TeamSidebarProps): ReactElement {
  const t = useTeamsText(locale);
  const tag = intlTag(locale);
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const [dialog, setDialog] = useState<DialogState>(null);
  const [menuFor, setMenuFor] = useState('');
  const [focusId, setFocusId] = useState('');
  const [limits, setLimits] = useState({ channels: ROW_LIMIT, direct: ROW_LIMIT, botDirect: ROW_LIMIT });
  const switcherRef = useRef<HTMLButtonElement>(null);
  const handled = useRef(onInviteHandled);
  handled.current = onInviteHandled;
  useEffect(() => { if (initialInvite) { setDialog({ kind: 'joinTeam', code: initialInvite }); handled.current?.(); } }, [initialInvite]);
  // A change of account or membership drops transient UI that belonged to the old view.
  useEffect(() => { setMenuFor(''); setSwitcherOpen(false); }, [snapshot.authority, snapshot.selectedTeamId]);

  const groups = useMemo(() => groupConversations(snapshot.conversations, snapshot.uid), [snapshot.conversations, snapshot.uid]);
  const selectedId = snapshot.selectedConversationId, uid = snapshot.uid;
  const nowMs = now === undefined ? Date.now() : typeof now === 'number' ? now : now.getTime();
  function authorName(author: import('./messages').MessageAuthor): string {
    if (author.kind === 'task') return author.displayName;
    if (author.kind === 'shared_bot') return snapshot.bots.find(bot => bot.sharedBotId === author.sharedBotId)?.name || t('teams_bot_tag');
    return snapshot.directory.find(member => member.uid === author.uid)?.displayName || (author.uid === uid ? t('teams_you') : t('teams_member'));
  }
  const needle = (query ?? '').trim().toLocaleLowerCase(), searching = needle !== '';
  const matches = (conversation: TeamConversation): boolean => {
    const label = conversationLabel(conversation, snapshot.directory, uid, { bots: snapshot.bots, locale });
    const preview = conversationPreview(conversation, uid, locale, last => authorName(last.author));
    return label.toLocaleLowerCase().includes(needle) || preview.toLocaleLowerCase().includes(needle);
  };
  const narrowed = (rows: TeamConversation[]) => searching ? rows.filter(matches) : rows;
  const sections = [
    { id: 'channels' as const, title: t('teams_group_channels'), add: t('teams_new_channel_title'), empty: t('tw_sidebar_no_channels'), rows: narrowed(groups.channels), dialog: { kind: 'channel' } as const },
    { id: 'direct' as const, title: t('teams_group_direct'), add: t('teams_new_message_title'), empty: t('tw_sidebar_no_dms'), rows: narrowed(groups.direct), dialog: { kind: 'direct' } as const },
    { id: 'botDirect' as const, title: t('teams_group_bot_chats'), add: t('teams_new_bot_title'), empty: t('tw_sidebar_no_bot_chats'), rows: narrowed(groups.botDirect), dialog: { kind: 'botDm' } as const },
  ];
  const shown = sections.map(section => {
    const index = section.rows.findIndex(row => row.id === selectedId);
    const limit = searching ? section.rows.length : Math.max(limits[section.id], index + 1);
    return { ...section, visible: section.rows.slice(0, limit), hidden: Math.max(0, section.rows.length - limit) };
  });
  const flat = shown.flatMap(section => section.visible);
  const tabStop = flat.some(row => row.id === focusId) ? focusId : flat.some(row => row.id === selectedId) ? selectedId : flat[0]?.id ?? '';

  const select = (id: string) => { workspace.selectConversation(id); onConversationSelected?.(id); };

  const otherUnread = Object.entries(snapshot.unreadByTeam).reduce((sum, [id, count]) => id === snapshot.selectedTeamId ? sum : sum + count, 0);
  const team = snapshot.team;
  const teamItems: MenuItem[] = [
    ...snapshot.teams.map(value => ({
      id: `team:${value.id}`, label: value.name, radio: true, checked: value.id === snapshot.selectedTeamId,
      detail: (snapshot.unreadByTeam[value.id] ?? 0) > 0 && value.id !== snapshot.selectedTeamId ? <UnreadBadge count={snapshot.unreadByTeam[value.id]!} locale={locale} /> : undefined,
      onSelect: () => workspace.selectTeam(value.id),
    })),
    ...(team && onOpenSettings ? [{ id: 'settings', label: t('tw_sidebar_team_settings'), icon: 'panel' as const, separated: true, onSelect: () => onOpenSettings(switcherRef.current ?? undefined) }] : []),
    { id: 'create', label: t('tw_sidebar_create_team'), icon: 'plus', separated: !(team && onOpenSettings), onSelect: () => setDialog({ kind: 'createTeam' }) },
    { id: 'join', label: t('teams_join'), icon: 'plus', onSelect: () => setDialog({ kind: 'joinTeam', code: '' }) },
  ];
  const hasTopbar = useContext(ChatTopbarTarget) !== undefined;
  const status = hasTopbar ? undefined : connectionStatus(snapshot, locale);
  const loadingRows = !!snapshot.selectedTeamId && snapshot.offline && snapshot.conversations.length === 0 && !snapshot.messagingError;

  return <nav className="tw-scope tw-sidebar" aria-label={t('teams_title')}>
    <header className="tw-sidebar-head">
      <div className="tw-switcher-wrap">
        <button ref={switcherRef} type="button" className="tw-switcher" aria-haspopup="menu" aria-expanded={switcherOpen}
          onClick={() => setSwitcherOpen(open => !open)}>
          {team ? <TeamMark id={team.id} name={team.name} avatar={snapshot.detail?.team.id === team.id ? snapshot.detail.profile?.avatar : undefined} /> : null}
          <span className="tw-switcher-name">{team?.name ?? (snapshot.loading ? t('teams_loading') : t('tw_sidebar_no_team'))}</span>
          {otherUnread > 0 ? <UnreadBadge count={otherUnread} locale={locale} /> : null}
          <Icon name="chevronDown" size={14} />
        </button>
        {switcherOpen ? <PopupMenu label={t('teams_title')} items={teamItems} anchor={switcherRef}
          onClose={restore => { setSwitcherOpen(false); if (restore) switcherRef.current?.focus(); }} /> : null}
      </div>
    </header>
    {team ? <button type="button" className="tw-home" aria-current={!selectedId ? 'page' : undefined} onClick={() => select('')}>{t('tw_sidebar_home')}</button> : null}
    {status ? <div className="tw-conn" role="status" data-kind={status.kind} title={status.detail}>
      <span className="tw-dot" aria-hidden="true" />
      <span className="tw-conn-text"><span className="tw-conn-label">{status.label}</span>{status.kind !== 'ok' ? <span className="tw-conn-detail">{status.detail}</span> : null}</span>
      {status.action ? <button type="button" className="tw-link" onClick={() => status.action === 'refresh' ? void workspace.refresh() : workspace.reconnect()}>{status.action === 'refresh' ? t('common_retry') : t('teams_reconnect')}</button> : null}
    </div> : null}

    {snapshot.loading ? <p className="tw-empty-line" role="status">{t('tw_sidebar_loading_teams')}</p>
      : !snapshot.teams.length ? <div className="tw-empty">
        <h2>{t('tw_sidebar_welcome')}</h2>
        <p>{snapshot.loadError ? t('teams_load_failed') : t('tw_sidebar_empty_hint')}</p>
        <div className="tw-empty-actions">
          <button type="button" className="tw-primary" onClick={() => setDialog({ kind: 'createTeam' })}>{t('tw_sidebar_create_team')}</button>
          <button type="button" onClick={() => setDialog({ kind: 'joinTeam', code: '' })}>{t('tw_sidebar_join')}</button>
        </div>
        <p className="tw-hint">{platformNotice(locale)}</p>
      </div>
      : !team ? <p className="tw-empty-line" role="status">{t('tw_sidebar_opening_team')}</p>
      : <div className="tw-sections" onKeyDown={rovingKeys}>
        {loadingRows ? <p className="tw-empty-line" role="status">{t('tw_sidebar_loading_conversations')}</p> : null}
        {searching && !loadingRows && !flat.length ? <p className="tw-empty-line" role="status">{t('teams_no_matches')}</p> : null}
        {shown.map(section => {
          const headId = `tw-section-${section.id}`;
          return <section key={section.id} className="tw-section" aria-labelledby={headId}>
            <div className="tw-section-head">
              <h3 id={headId}>{section.title}</h3>
              <button type="button" className="tw-icon-btn" aria-label={section.add} title={section.add} onClick={() => setDialog(section.dialog)}><Icon name="plus" /></button>
            </div>
            {section.visible.length ? <ul className="tw-rows">
              {section.visible.map(conversation => <ConversationRow key={conversation.id} conversation={conversation} snapshot={snapshot} renderAvatar={renderAvatar}
                selected={conversation.id === selectedId} tabStop={conversation.id === tabStop} locale={tag} now={nowMs} authorName={authorName}
                menuOpen={menuFor === conversation.id} onMenu={setMenuFor} onFocusRow={setFocusId}
                onSelect={select} onMarkRead={id => void workspace.markRead(id)} onOpenDetails={onOpenDetails}
                onPrefetch={workspace.prefetchConversation} onCancelPrefetch={workspace.cancelConversationPrefetch} />)}
            </ul> : loadingRows || searching ? null : <p className="tw-empty-line">{section.empty}</p>}
            {section.hidden > 0 ? <button type="button" className="tw-link tw-more" onClick={() => setLimits(old => ({ ...old, [section.id]: old[section.id] + ROW_LIMIT }))}>{t('mobile_bots_show_more_fmt', section.hidden)}</button> : null}
          </section>;
        })}
      </div>}

    {onOpenServices ? <button type="button" className="tw-link tw-sidebar-services" onClick={onOpenServices}>{t('services_title')}</button> : null}
    {dialog?.kind === 'createTeam' ? <CreateTeamDialog workspace={workspace} snapshot={snapshot} locale={locale} onClose={() => setDialog(null)} /> : null}
    {dialog?.kind === 'joinTeam' ? <JoinTeamDialog workspace={workspace} snapshot={snapshot} locale={locale} initialCode={dialog.code} onClose={() => { setDialog(null); onInviteResolved?.(); }} /> : null}
    {dialog?.kind === 'channel' ? <NewConversationDialog workspace={workspace} snapshot={snapshot} locale={locale} mode="channel" renderAvatar={renderAvatar} onClose={() => setDialog(null)} /> : null}
    {dialog?.kind === 'direct' ? <NewConversationDialog workspace={workspace} snapshot={snapshot} locale={locale} mode="direct" renderAvatar={renderAvatar} onClose={() => setDialog(null)} /> : null}
    {dialog?.kind === 'botDm' ? <NewBotDmDialog workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderAvatar} onClose={() => setDialog(null)} /> : null}
  </nav>;
}

function ConversationRow({ conversation, snapshot, renderAvatar, selected, tabStop, locale, now, authorName, menuOpen, onMenu, onFocusRow, onSelect, onMarkRead, onOpenDetails, onPrefetch, onCancelPrefetch }: {
  conversation: TeamConversation; snapshot: TeamsSnapshot; renderAvatar?: RenderAvatar; selected: boolean; tabStop: boolean; locale: string; now: number;
  authorName(author: import('./messages').MessageAuthor): string; menuOpen: boolean;
  onMenu(id: string): void; onFocusRow(id: string): void; onSelect(id: string): void; onMarkRead(id: string): void; onOpenDetails?(id: string): void;
  onPrefetch(id: string): void; onCancelPrefetch(id: string): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const mainRef = useRef<HTMLButtonElement>(null), moreRef = useRef<HTMLButtonElement>(null), origin = useRef<'main' | 'more'>('more');
  const unread = unreadCount(conversation);
  const label = conversationLabel(conversation, snapshot.directory, snapshot.uid, { bots: snapshot.bots, locale });
  const pending = [...(snapshot.messaging?.state.outbox ?? [])].reverse().find(item => {
    const payload = pendingPayload(item);
    return payload.conversationId === conversation.id && (!('operation' in payload) || payload.operation === 'create');
  });
  const preview = pending
    ? `${pending.error ? t('tw_sidebar_not_sent') : t('tw_sidebar_pending')}: ${snippet(pendingPayload(pending).text)}`
    : conversationPreview(conversation, snapshot.uid, locale, last => authorName(last.author));
  const time = pending ? '' : formatListTime(conversation.lastMessage?.at, now, locale);
  const items: MenuItem[] = [
    ...(unread > 0 ? [{ id: 'read', label: t('tw_mark_as_read'), icon: 'check' as const, disabled: snapshot.offline, onSelect: () => onMarkRead(conversation.id) }] : []),
    ...(onOpenDetails ? [{ id: 'details', label: t('teams_conversation_details'), icon: 'info' as const, onSelect: () => onOpenDetails(conversation.id) }] : []),
  ];
  const open = (from: 'main' | 'more') => { origin.current = from; onMenu(menuOpen ? '' : conversation.id); };
  return <li className="tw-row" data-selected={selected} data-unread={unread > 0} data-kind={conversation.kind}>
    <button ref={mainRef} type="button" className="tw-row-main" data-row={conversation.id} aria-current={selected ? 'page' : undefined} tabIndex={tabStop ? 0 : -1}
      onClick={() => onSelect(conversation.id)} onFocus={event => {
        onFocusRow(conversation.id);
        if (event.currentTarget.matches(':focus-visible')) onPrefetch(conversation.id);
      }}
      onBlur={() => onCancelPrefetch(conversation.id)}
      onPointerEnter={event => { if (event.pointerType === 'mouse') onPrefetch(conversation.id); }}
      onPointerLeave={event => { if (event.pointerType === 'mouse') onCancelPrefetch(conversation.id); }}
      onContextMenu={items.length ? event => { event.preventDefault(); open('main'); } : undefined}>
      <span className="tw-row-lead"><ConversationLead conversation={conversation} label={label} snapshot={snapshot} locale={locale} render={renderAvatar} /></span>
      <span className="tw-row-text">
        <span className="tw-row-top"><span className="tw-row-title">{label}</span>{time ? <time className="tw-row-time" dateTime={conversation.lastMessage?.at ?? undefined}>{time}</time> : null}</span>
        <span className="tw-row-bottom"><span className="tw-row-preview">{preview}</span><UnreadBadge count={unread} locale={locale} /></span>
      </span>
    </button>
    {items.length ? <button ref={moreRef} type="button" className="tw-row-more" aria-label={t('tw_sidebar_conversation_actions')} aria-haspopup="menu" aria-expanded={menuOpen}
      tabIndex={tabStop ? 0 : -1} onClick={() => open('more')}><Icon name="more" /></button> : null}
    {menuOpen && items.length ? <PopupMenu label={label} items={items} anchor={moreRef} align="end"
      onClose={restore => { onMenu(''); if (restore) (origin.current === 'more' ? moreRef : mainRef).current?.focus(); }} /> : null}
  </li>;
}
