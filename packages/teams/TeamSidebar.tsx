import { useEffect, useMemo, useRef, useState, type KeyboardEvent, type ReactElement } from 'react';
import { messageErrorText, teamsErrorText } from './errors.js';
import type { TeamConversation } from './messages.js';
import { NewBotDmDialog, NewConversationDialog } from './NewConversationDialog';
import { formatListTime } from './stream.js';
import { CreateTeamDialog, JoinTeamDialog } from './team-dialogs';
import { avatarColor, ConversationLead, Icon, initials, platformNotice, PopupMenu, rovingIndex, UnreadBadge, type DrawerTab, type MenuItem, type RenderAvatar } from './ui';
import { conversationLabel, conversationPreview, groupConversations, unreadCount, type TeamsSnapshot, type TeamsWorkspace } from './workspace.js';

/** Rows shown per section before "Show more". The open conversation is always shown. */
export const ROW_LIMIT = 50;

export interface TeamSidebarProps {
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  zh: boolean;
  /** Draw your own avatar for a person or Bot; return nothing for the initials avatar. */
  renderAvatar?: RenderAvatar;
  /**
   * Open team settings (the details drawer), optionally on a tab. Without it the team menu has no settings entry.
   * The sidebar has no button for this besides that entry: the conversation header toggles the same drawer, and the
   * entry is the way in while no conversation (and so no header) is open.
   */
  onOpenSettings?(tab?: DrawerTab): void;
  /** A conversation was chosen, including the one already open. */
  onConversationSelected?(id: string): void;
  /** Show a conversation's details. Without it the row menu has no such entry. */
  onOpenDetails?(id: string): void;
  /** An invitation opened from a link. The join dialog opens with it filled in, but never joins by itself. */
  initialInvite?: string;
  onInviteHandled?(): void;
  /** BCP 47 tag for times; defaults to `zh-CN` or `en-US` from `zh`. */
  locale?: string;
  /** The current time, for the "Yesterday" and weekday labels. Defaults to now. */
  now?: number | Date;
  /**
   * Narrow the lists to the conversations whose name or newest-message preview contains this text (any case, surrounding
   * space ignored). While it is set every match is listed, with no "Show more".
   */
  query?: string;
}

export interface ConnectionStatus { kind: 'ok' | 'offline' | 'error'; label: string; detail: string; action?: 'refresh' | 'reconnect' }
/**
 * The line under the team switcher. A failed refetch of the team list is an error with a retry;
 * a controller that is offline says so and can reconnect; otherwise the team is connected. Nothing
 * while the first list is loading or when there is no team to be connected to.
 */
export function connectionStatus(snapshot: Pick<TeamsSnapshot, 'loading' | 'selectedTeamId' | 'loadError' | 'offline' | 'messagingError'>, zh: boolean): ConnectionStatus | undefined {
  const t = (en: string, cn: string) => zh ? cn : en;
  if (snapshot.loading) return undefined;
  if (snapshot.loadError) return { kind: 'error', label: t('Could not refresh teams', '无法刷新团队'), detail: teamsErrorText(snapshot.loadError, zh), action: 'refresh' };
  if (!snapshot.selectedTeamId) return undefined;
  if (snapshot.offline) {
    return {
      kind: 'offline', label: t('Offline · saved messages', '离线 · 本机缓存'), action: 'reconnect',
      detail: snapshot.messagingError ? messageErrorText(snapshot.messagingError, zh) : t('Showing saved messages. Reconnect to confirm access and send pending messages.', '当前显示本机缓存。联网后确认权限并发送待发消息。'),
    };
  }
  return { kind: 'ok', label: t('Connected', '已连接'), detail: t('Recently loaded messages are available offline.', '已加载的近期消息可离线查看。') };
}

type DialogState = { kind: 'createTeam' } | { kind: 'joinTeam'; code: string } | { kind: 'channel' } | { kind: 'direct' } | { kind: 'botDm' } | null;

/**
 * The left column of the Teams workspace: team switcher, connection line, and the open team's
 * Channels, Direct messages and Bot DMs. It owns only transient UI state (menus, dialogs, the
 * focused row); everything else comes from the workspace.
 */
export function TeamSidebar({ workspace, snapshot, zh, renderAvatar, onOpenSettings, onConversationSelected, onOpenDetails, initialInvite, onInviteHandled, locale, now, query }: TeamSidebarProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const tag = locale ?? (zh ? 'zh-CN' : 'en-US');
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
  const authorName = (author: { kind: 'user' | 'bot'; uid: string; botKey?: string }) => author.kind === 'bot'
    ? snapshot.bots.find(bot => bot.key === author.botKey)?.name || 'Bot'
    : snapshot.directory.find(member => member.uid === author.uid)?.displayName || (author.uid === uid ? t('You', '你') : t('Member', '成员'));
  const needle = (query ?? '').trim().toLocaleLowerCase(), searching = needle !== '';
  const matches = (conversation: TeamConversation): boolean => {
    const label = conversationLabel(conversation, snapshot.directory, uid, { bots: snapshot.bots, zh });
    const preview = conversationPreview(conversation, uid, zh, last => authorName(last.author));
    return label.toLocaleLowerCase().includes(needle) || preview.toLocaleLowerCase().includes(needle);
  };
  const narrowed = (rows: TeamConversation[]) => searching ? rows.filter(matches) : rows;
  const sections = [
    { id: 'channels' as const, title: t('Channels', '频道'), add: t('New channel', '新建频道'), empty: t('No channels yet. Create one with +.', '还没有频道，点击 + 新建。'), rows: narrowed(groups.channels), dialog: { kind: 'channel' } as const },
    { id: 'direct' as const, title: t('Direct messages', '私信'), add: t('New message', '新建私信'), empty: t('No messages yet. Start one with +.', '还没有私信，点击 + 开始。'), rows: narrowed(groups.direct), dialog: { kind: 'direct' } as const },
    { id: 'botDirect' as const, title: t('Bot DMs', 'Bot 对话'), add: t('Message a Bot', '与 Bot 对话'), empty: t('No Bot chats yet. Start one with +.', '还没有 Bot 对话，点击 + 开始。'), rows: narrowed(groups.botDirect), dialog: { kind: 'botDm' } as const },
  ];
  const shown = sections.map(section => {
    const index = section.rows.findIndex(row => row.id === selectedId);
    const limit = searching ? section.rows.length : Math.max(limits[section.id], index + 1);
    return { ...section, visible: section.rows.slice(0, limit), hidden: Math.max(0, section.rows.length - limit) };
  });
  const flat = shown.flatMap(section => section.visible);
  const tabStop = flat.some(row => row.id === focusId) ? focusId : flat.some(row => row.id === selectedId) ? selectedId : flat[0]?.id ?? '';

  const select = (id: string) => { workspace.selectConversation(id); onConversationSelected?.(id); };
  const move = (event: KeyboardEvent<HTMLElement>) => {
    const target = event.target as HTMLElement;
    if (!target.matches?.('[data-row]')) return;
    const rows = [...event.currentTarget.querySelectorAll<HTMLElement>('[data-row]')];
    const to = rovingIndex(rows.length, rows.indexOf(target), event.key);
    if (to === undefined) return;
    event.preventDefault();
    rows[to]?.focus();
  };

  const otherUnread = Object.entries(snapshot.unreadByTeam).reduce((sum, [id, count]) => id === snapshot.selectedTeamId ? sum : sum + count, 0);
  const team = snapshot.team;
  const teamItems: MenuItem[] = [
    ...snapshot.teams.map(value => ({
      id: `team:${value.id}`, label: value.name, radio: true, checked: value.id === snapshot.selectedTeamId,
      detail: (snapshot.unreadByTeam[value.id] ?? 0) > 0 && value.id !== snapshot.selectedTeamId ? <UnreadBadge count={snapshot.unreadByTeam[value.id]!} zh={zh} /> : undefined,
      onSelect: () => workspace.selectTeam(value.id),
    })),
    ...(team && onOpenSettings ? [{ id: 'settings', label: t('Team members & settings', '团队成员与设置'), icon: 'panel' as const, separated: true, onSelect: () => onOpenSettings() }] : []),
    { id: 'create', label: t('Create a team', '创建团队'), icon: 'plus', separated: !(team && onOpenSettings), onSelect: () => setDialog({ kind: 'createTeam' }) },
    { id: 'join', label: t('Join a team', '加入团队'), icon: 'plus', onSelect: () => setDialog({ kind: 'joinTeam', code: '' }) },
  ];
  const status = connectionStatus(snapshot, zh);
  const loadingRows = !!snapshot.selectedTeamId && snapshot.offline && snapshot.conversations.length === 0 && !snapshot.messagingError;

  return <nav className="tw-scope tw-sidebar" aria-label={t('Teams', '团队')}>
    <header className="tw-sidebar-head">
      <div className="tw-switcher-wrap">
        <button ref={switcherRef} type="button" className="tw-switcher" aria-haspopup="menu" aria-expanded={switcherOpen}
          onClick={() => setSwitcherOpen(open => !open)}>
          {team ? <TeamMark id={team.id} name={team.name} /> : null}
          <span className="tw-switcher-name">{team?.name ?? (snapshot.loading ? t('Loading…', '正在加载…') : t('No team', '没有团队'))}</span>
          {otherUnread > 0 ? <UnreadBadge count={otherUnread} zh={zh} /> : null}
          <Icon name="chevronDown" size={14} />
        </button>
        {switcherOpen ? <PopupMenu label={t('Teams', '团队')} items={teamItems} anchor={switcherRef}
          onClose={restore => { setSwitcherOpen(false); if (restore) switcherRef.current?.focus(); }} /> : null}
      </div>
    </header>
    {status ? <div className="tw-conn" role="status" data-kind={status.kind} title={status.detail}>
      <span className="tw-dot" aria-hidden="true" />
      <span className="tw-conn-text"><span className="tw-conn-label">{status.label}</span>{status.kind !== 'ok' ? <span className="tw-conn-detail">{status.detail}</span> : null}</span>
      {status.action ? <button type="button" className="tw-link" onClick={() => status.action === 'refresh' ? void workspace.refresh() : workspace.reconnect()}>{status.action === 'refresh' ? t('Retry', '重试') : t('Reconnect', '重新连接')}</button> : null}
    </div> : null}

    {snapshot.loading ? <p className="tw-empty-line" role="status">{t('Loading teams…', '正在加载团队…')}</p>
      : !snapshot.teams.length ? <div className="tw-empty">
        <h2>{t('A shared place for your team', '与团队保持连接')}</h2>
        <p>{snapshot.loadError ? t('Teams could not be loaded.', '无法加载团队。') : t('Create a team or join one with an invite.', '创建团队，或通过邀请加入。')}</p>
        <div className="tw-empty-actions">
          <button type="button" className="tw-primary" onClick={() => setDialog({ kind: 'createTeam' })}>{t('Create a team', '创建团队')}</button>
          <button type="button" onClick={() => setDialog({ kind: 'joinTeam', code: '' })}>{t('Join with an invite', '通过邀请加入')}</button>
        </div>
        <p className="tw-hint">{platformNotice(zh)}</p>
      </div>
      : !team ? <p className="tw-empty-line" role="status">{t('Opening team…', '正在打开团队…')}</p>
      : <div className="tw-sections" onKeyDown={move}>
        {loadingRows ? <p className="tw-empty-line" role="status">{t('Loading conversations…', '正在加载会话…')}</p> : null}
        {searching && !loadingRows && !flat.length ? <p className="tw-empty-line" role="status">{t('No matching conversations.', '没有匹配的会话。')}</p> : null}
        {shown.map(section => {
          const headId = `tw-section-${section.id}`;
          return <section key={section.id} className="tw-section" aria-labelledby={headId}>
            <div className="tw-section-head">
              <h3 id={headId}>{section.title}</h3>
              <button type="button" className="tw-icon-btn" aria-label={section.add} title={section.add} onClick={() => setDialog(section.dialog)}><Icon name="plus" /></button>
            </div>
            {section.visible.length ? <ul className="tw-rows">
              {section.visible.map(conversation => <ConversationRow key={conversation.id} conversation={conversation} snapshot={snapshot} zh={zh} renderAvatar={renderAvatar}
                selected={conversation.id === selectedId} tabStop={conversation.id === tabStop} locale={tag} now={nowMs} authorName={authorName}
                menuOpen={menuFor === conversation.id} onMenu={setMenuFor} onFocusRow={setFocusId}
                onSelect={select} onMarkRead={id => void workspace.markRead(id)} onOpenDetails={onOpenDetails} />)}
            </ul> : loadingRows || searching ? null : <p className="tw-empty-line">{section.empty}</p>}
            {section.hidden > 0 ? <button type="button" className="tw-link tw-more" onClick={() => setLimits(old => ({ ...old, [section.id]: old[section.id] + ROW_LIMIT }))}>{t(`Show ${section.hidden} more`, `显示另外 ${section.hidden} 个`)}</button> : null}
          </section>;
        })}
      </div>}

    {dialog?.kind === 'createTeam' ? <CreateTeamDialog workspace={workspace} snapshot={snapshot} zh={zh} onClose={() => setDialog(null)} /> : null}
    {dialog?.kind === 'joinTeam' ? <JoinTeamDialog workspace={workspace} snapshot={snapshot} zh={zh} initialCode={dialog.code} onClose={() => setDialog(null)} /> : null}
    {dialog?.kind === 'channel' ? <NewConversationDialog workspace={workspace} snapshot={snapshot} zh={zh} mode="channel" renderAvatar={renderAvatar} onClose={() => setDialog(null)} /> : null}
    {dialog?.kind === 'direct' ? <NewConversationDialog workspace={workspace} snapshot={snapshot} zh={zh} mode="direct" renderAvatar={renderAvatar} onClose={() => setDialog(null)} /> : null}
    {dialog?.kind === 'botDm' ? <NewBotDmDialog workspace={workspace} snapshot={snapshot} zh={zh} renderAvatar={renderAvatar} onClose={() => setDialog(null)} /> : null}
  </nav>;
}

/** A team's square mark: its initials on a color that stays the same for the team. */
function TeamMark({ id, name }: { id: string; name: string }): ReactElement {
  return <span className="tw-team-mark" aria-hidden="true" style={{ background: avatarColor(id) }}>{initials(name)}</span>;
}

function ConversationRow({ conversation, snapshot, zh, renderAvatar, selected, tabStop, locale, now, authorName, menuOpen, onMenu, onFocusRow, onSelect, onMarkRead, onOpenDetails }: {
  conversation: TeamConversation; snapshot: TeamsSnapshot; zh: boolean; renderAvatar?: RenderAvatar; selected: boolean; tabStop: boolean; locale: string; now: number;
  authorName(author: { kind: 'user' | 'bot'; uid: string; botKey?: string }): string; menuOpen: boolean;
  onMenu(id: string): void; onFocusRow(id: string): void; onSelect(id: string): void; onMarkRead(id: string): void; onOpenDetails?(id: string): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const mainRef = useRef<HTMLButtonElement>(null), moreRef = useRef<HTMLButtonElement>(null), origin = useRef<'main' | 'more'>('more');
  const unread = unreadCount(conversation);
  const label = conversationLabel(conversation, snapshot.directory, snapshot.uid, { bots: snapshot.bots, zh });
  const preview = conversationPreview(conversation, snapshot.uid, zh, last => authorName(last.author));
  const time = formatListTime(conversation.lastMessage?.at, now, locale);
  const items: MenuItem[] = [
    ...(unread > 0 ? [{ id: 'read', label: t('Mark as read', '标为已读'), icon: 'check' as const, disabled: snapshot.offline, onSelect: () => onMarkRead(conversation.id) }] : []),
    ...(onOpenDetails ? [{ id: 'details', label: t('Conversation details', '会话详情'), icon: 'info' as const, onSelect: () => onOpenDetails(conversation.id) }] : []),
  ];
  const open = (from: 'main' | 'more') => { origin.current = from; onMenu(menuOpen ? '' : conversation.id); };
  return <li className="tw-row" data-selected={selected} data-unread={unread > 0} data-kind={conversation.kind}>
    <button ref={mainRef} type="button" className="tw-row-main" data-row={conversation.id} aria-current={selected ? 'page' : undefined} tabIndex={tabStop ? 0 : -1}
      onClick={() => onSelect(conversation.id)} onFocus={() => onFocusRow(conversation.id)}
      onContextMenu={items.length ? event => { event.preventDefault(); open('main'); } : undefined}>
      <span className="tw-row-lead"><ConversationLead conversation={conversation} label={label} snapshot={snapshot} zh={zh} render={renderAvatar} /></span>
      <span className="tw-row-text">
        <span className="tw-row-top"><span className="tw-row-title">{label}</span>{time ? <time className="tw-row-time" dateTime={conversation.lastMessage?.at ?? undefined}>{time}</time> : null}</span>
        <span className="tw-row-bottom"><span className="tw-row-preview">{preview}</span><UnreadBadge count={unread} zh={zh} /></span>
      </span>
    </button>
    {items.length ? <button ref={moreRef} type="button" className="tw-row-more" aria-label={t('Conversation actions', '会话操作')} aria-haspopup="menu" aria-expanded={menuOpen}
      tabIndex={tabStop ? 0 : -1} onClick={() => open('more')}><Icon name="more" /></button> : null}
    {menuOpen && items.length ? <PopupMenu label={label} items={items} anchor={moreRef} align="end"
      onClose={restore => { onMenu(''); if (restore) (origin.current === 'more' ? moreRef : mainRef).current?.focus(); }} /> : null}
  </li>;
}
