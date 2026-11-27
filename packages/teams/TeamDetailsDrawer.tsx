import { useEffect, useId, useRef, useState, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactElement } from 'react';
import { messageErrorText, teamsErrorText } from './errors.js';
import type { TeamCommand, TeamMember, TeamRole } from './index.js';
import { BotDirectory } from './BotDirectory';
import { ChannelVisualizationSettings } from './ChannelVisualizationSettings.js';
import { canInviteFriends } from './friends.js';
import { Avatar, ConversationLead, Dialog, Icon, platformNotice, rovingIndex, TeamMark, type DrawerTab, type RenderAvatar } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { conversationLabel } from './workspace.js';
import { intlTag, useTeamsText } from './i18n.js';

export const DRAWER_MIN = 280;
export const DRAWER_MAX = 480;
export const DRAWER_DEFAULT = 360;
const STEP = 16, STEP_LARGE = 48;

/** A drawer width kept within its limits. Anything that is not a finite number becomes the default. */
export const clampDrawerWidth = (width: number): number => Number.isFinite(width) ? Math.min(DRAWER_MAX, Math.max(DRAWER_MIN, Math.round(width))) : DRAWER_DEFAULT;
/** The drawer is on the right, so dragging its left edge to the left widens it. */
export const drawerWidthForDrag = (startWidth: number, startX: number, x: number): number => clampDrawerWidth(startWidth + (startX - x));
/** The width a key gives the resize handle: arrows step (Shift: a larger step), Home and End jump to the limits. `undefined` for other keys. */
export function drawerWidthForKey(width: number, key: string, shift = false): number | undefined {
  const step = shift ? STEP_LARGE : STEP;
  if (key === 'ArrowLeft') return clampDrawerWidth(width + step);
  if (key === 'ArrowRight') return clampDrawerWidth(width - step);
  if (key === 'Home') return DRAWER_MIN;
  if (key === 'End') return DRAWER_MAX;
  return undefined;
}

/** Tabs a role can see. Plain members cannot create or revoke invitations, so they have no Invites tab. */
export const drawerTabs = (role: TeamRole): DrawerTab[] => role === 'member' ? ['members', 'bots'] : ['members', 'invites', 'bots'];
/** Owners manage everyone but themselves and other owners; admins manage plain members. */
export const canManageMember = (viewerRole: TeamRole, viewerUid: string, member: Pick<TeamMember, 'uid' | 'role'>): boolean =>
  member.uid !== viewerUid && member.role !== 'owner' && (viewerRole === 'owner' || (viewerRole === 'admin' && member.role === 'member'));

export interface TeamDetailsDrawerProps {
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  /** The shown tab, when the host keeps it; otherwise the drawer does. */
  tab?: DrawerTab;
  onTabChange?(tab: DrawerTab): void;
  /** The width in CSS pixels, when the host keeps it; otherwise the drawer does, starting at `defaultWidth`. */
  width?: number;
  defaultWidth?: number;
  onWidthChange?(width: number): void;
  onClose(): void;
  renderAvatar?: RenderAvatar;
  locale: string;
  /** Without it members have no Add friend action (D15). The host looks the person up and shows the result. */
  onAddFriend?(member: TeamMember): void;
  /** Without it the Invites tab has no Invite friends button (D26). The host shows the picker. Only an owner or an admin sees the tab. */
  onInviteFriends?(): void;
}

/** What is asked once before a consequential command (U8): the title names the thing, the body says what follows, the button is the verb. */
interface Confirmation { command: TeamCommand; title: string; body: string; confirm: string; /** A destructive act gets the danger button; a change of role the primary one. */ danger: boolean }

/**
 * The right column: the active chat, the team's member directory and settings.
 * The left edge is a resize handle (280 to 480 pixels). Escape closes the panel.
 */
export function TeamDetailsDrawer({ workspace, snapshot, tab, onTabChange, width: controlledWidth, defaultWidth, onWidthChange, onClose, renderAvatar, locale, onAddFriend, onInviteFriends }: TeamDetailsDrawerProps): ReactElement | null {
  const t = useTeamsText(locale);
  const tag = intlTag(locale);
  const id = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const drag = useRef<{ x: number; width: number } | null>(null);
  const closing = useRef(onClose);
  closing.current = onClose;
  const [ownTab, setOwnTab] = useState<DrawerTab>('members');
  const [ownWidth, setOwnWidth] = useState(() => clampDrawerWidth(defaultWidth ?? DRAWER_DEFAULT));
  const [confirmation, setConfirmation] = useState<Confirmation>();
  const [uses, setUses] = useState(1), [ttl, setTtl] = useState(86400), [inviteRole, setInviteRole] = useState<'member' | 'admin'>('member');
  const [notice, setNotice] = useState('');
  const team = snapshot.team, detail = snapshot.detail, uid = snapshot.uid;

  // Focus moves into the drawer when it opens and back to where it came from when it closes.
  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    heading.current?.focus();
    return () => { if (opener && document.contains(opener)) opener.focus(); };
  }, []);
  // Leaving, deleting, or losing the team leaves nothing to show.
  useEffect(() => { if (!team) closing.current(); }, [team]);
  // A change of account or membership invalidates what the person was about to confirm.
  useEffect(() => { setConfirmation(undefined); }, [snapshot.authority]);
  useEffect(() => { setNotice(''); }, [snapshot.selectedTeamId]);

  if (!team) return null;
  const tabs = drawerTabs(team.role);
  const requested = tab ?? ownTab;
  const current: DrawerTab = tabs.includes(requested) ? requested : 'members';
  const width = clampDrawerWidth(controlledWidth ?? ownWidth);
  const setWidth = (next: number) => {
    const value = clampDrawerWidth(next);
    if (controlledWidth === undefined) setOwnWidth(value);
    if (value !== width) onWidthChange?.(value);
  };
  const choose = (next: DrawerTab) => { setOwnTab(next); onTabChange?.(next); };
  const role = (value: TeamRole) => ({ owner: t('teams_owner'), admin: t('teams_admin'), member: t('teams_member') })[value];
  const tabLabel = (value: DrawerTab) => ({ members: t('teams_members'), invites: t('teams_invites'), bots: t('remote_bots') })[value];

  const ask = (command: TeamCommand, title: string, body: string, confirm: string, danger = true) => setConfirmation({ command, title, body, confirm, danger });
  const confirmNow = async () => {
    const pending = confirmation;
    if (!pending) return;
    const ok = await workspace.execute(pending.command);
    setConfirmation(undefined);
    // The workspace moves on to another team after this one is left or deleted; the drawer was about the old one.
    if (ok && (pending.command.action === 'leave' || pending.command.action === 'delete')) onClose();
  };
  const leaveOrDelete = team.role === 'owner'
    ? () => ask({ action: 'delete', teamId: team.id }, t('teams_confirm_delete_title_fmt', team.name),
      t('teams_confirm_delete_body'), t('teams_confirm_delete_button'))
    : () => ask({ action: 'leave', teamId: team.id, generation: team.generation }, t('teams_confirm_leave_title_fmt', team.name),
      t('teams_confirm_leave_body'), t('teams_confirm_leave_button'));

  const resizeDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    drag.current = { x: event.clientX, width };
    try { event.currentTarget.setPointerCapture(event.pointerId); } catch { /* Without capture the drag still follows the pointer over the handle. */ }
  };
  const resizeMove = (event: ReactPointerEvent<HTMLDivElement>) => { if (drag.current) setWidth(drawerWidthForDrag(drag.current.width, drag.current.x, event.clientX)); };
  const resizeUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    drag.current = null;
    try { event.currentTarget.releasePointerCapture(event.pointerId); } catch { /* Capture was never taken. */ }
  };
  const resizeKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const next = drawerWidthForKey(width, event.key, event.shiftKey);
    if (next === undefined) return;
    event.preventDefault(); setWidth(next);
  };
  const tabKey = (event: KeyboardEvent<HTMLDivElement>) => {
    const to = rovingIndex(tabs.length, tabs.indexOf(current), event.key, { prev: 'ArrowLeft', next: 'ArrowRight', wrap: true });
    if (to === undefined) return;
    event.preventDefault();
    choose(tabs[to]!);
    queueMicrotask(() => document.getElementById(`${id}-tab-${tabs[to]}`)?.focus());
  };
  const escape = (event: KeyboardEvent<HTMLElement>) => {
    // A confirmation or a menu inside the drawer takes Escape first.
    if (event.key === 'Escape' && !event.defaultPrevented && !confirmation) { event.stopPropagation(); onClose(); }
  };

  const copy = (link: string) => {
    const done = () => setNotice(t('teams_link_copied')), fail = () => setNotice(t('tw_team_copy_hint'));
    if (!navigator.clipboard) { fail(); return; }
    navigator.clipboard.writeText(link).then(done, fail);
  };
  const conversation = snapshot.conversation;
  const chatLabel = conversation ? conversationLabel(conversation, snapshot.directory, uid, { bots: snapshot.bots, locale }) : undefined;
  const members = detail?.members ?? [];
  const invites = detail?.invites ?? [];
  const body = (): ReactElement => {
    if (current === 'bots') {
      return <div className="tw-embed">
        <BotDirectory key={`${uid}:${team.id}:${team.generation}:${snapshot.authority}`} api={workspace.api} uid={uid} team={team} members={snapshot.directory} bots={snapshot.bots} locale={locale} busy={snapshot.messageBusy} renderAvatar={renderAvatar}
          status={snapshot.botsUnverified ? t('teams_bots_unverified') : !snapshot.botsLoaded ? t('teams_bots_loading') : ''}
          attempt={workspace.attempt} onExample={(bot, example) => { void workspace.openBotExample(bot, example); }} onOpen={bot => workspace.startBotDm(bot)} onChanged={() => workspace.refreshBots()}
          onOpenChat={id => workspace.selectConversation(id)} />
        {snapshot.actionError ? <p role="alert" className="tw-error">{messageErrorText(snapshot.actionError, locale)}</p> : null}
      </div>;
    }
    if (!detail) {
      return snapshot.loadError && !snapshot.detailLoading
        ? <div className="tw-form"><p role="alert" className="tw-error">{teamsErrorText(snapshot.loadError, locale)}</p><button type="button" onClick={() => void workspace.refresh()}>{t('common_retry')}</button></div>
        : <p className="tw-empty-line" role="status">{t('teams_loading')}</p>;
    }
    if (current === 'members') {
      return <ul className="tw-people" aria-label={t('teams_members')}>
        {members.map(member => {
          const person = <>
            <Avatar identity={{ kind: 'user', id: member.uid, name: member.displayName }} size={40} render={renderAvatar} />
            <div className="tw-person-text"><b>{member.displayName}</b><small>{member.uid === uid ? `${t('teams_you')} · ` : ''}{role(member.role)}</small></div>
          </>;
          if (!(onAddFriend && member.uid !== uid) && !canManageMember(team.role, uid, member)) {
            return <li key={member.uid}><div className="tw-person-row">{person}</div></li>;
          }
          return <li key={member.uid}><details className="tw-member-actions">
            <summary className="tw-person-row" aria-label={`${member.displayName} · ${t('tw_more')}`}>
              {person}<span className="tw-person-more" title={t('tw_more')}><Icon name="more" /></span>
            </summary>
            <div className="tw-person-actions">
              {onAddFriend && member.uid !== uid ? <button type="button" onClick={() => onAddFriend(member)}>{t('friends_add')}</button> : null}
              {canManageMember(team.role, uid, member) ? <>
                {team.role === 'owner' ? <button type="button" disabled={snapshot.busy}
                  onClick={() => ask({ action: 'role', teamId: team.id, uid: member.uid, generation: member.generation, role: member.role === 'admin' ? 'member' : 'admin' },
                    member.role === 'admin' ? t('teams_confirm_demote_title_fmt', member.displayName) : t('teams_confirm_promote_title_fmt', member.displayName),
                    member.role === 'admin' ? t('teams_confirm_demote_body') : t('teams_confirm_promote_body'),
                    member.role === 'admin' ? t('teams_demote') : t('teams_promote'), false)}>
                  {member.role === 'admin' ? t('teams_demote') : t('teams_promote')}</button> : null}
                {team.role === 'owner' ? <button type="button" disabled={snapshot.busy}
                  onClick={() => ask({ action: 'handOver', teamId: team.id, uid: member.uid, generation: member.generation }, t('tw_team_owner_title_fmt', member.displayName, team.name),
                    t('tw_team_owner_body'),
                    t('teams_make_owner'), false)}>
                  {t('teams_make_owner')}</button> : null}
                <button type="button" className="tw-danger-btn" disabled={snapshot.busy}
                  onClick={() => ask({ action: 'remove', teamId: team.id, uid: member.uid, generation: member.generation }, t('teams_confirm_remove_title_fmt', member.displayName),
                    t('teams_confirm_remove_body'), t('teams_confirm_remove_button'))}>{t('teams_confirm_remove_button')}</button>
              </> : null}
            </div>
          </details></li>;
        })}
      </ul>;
    }
    return <div className="tw-invites">
      {onInviteFriends && canInviteFriends(team.role) ? <button type="button" onClick={onInviteFriends}>{t('friends_invite_action')}</button> : null}
      <form className="tw-form" onSubmit={event => { event.preventDefault(); void workspace.execute({ action: 'invite', teamId: team.id, requestId: crypto.randomUUID(), role: team.role === 'owner' ? inviteRole : 'member', maxUses: uses, ttlSeconds: ttl }); }}>
        <label>{t('tw_team_join_role')}
          <select value={team.role === 'owner' ? inviteRole : 'member'} disabled={team.role !== 'owner'} onChange={event => setInviteRole(event.target.value as 'member' | 'admin')}>
            <option value="member">{t('teams_member')}</option>{team.role === 'owner' ? <option value="admin">{t('teams_admin')}</option> : null}
          </select>
        </label>
        <div className="tw-form-row">
          <label>{t('teams_uses')}<input type="number" min={1} max={50} value={uses} onChange={event => setUses(Number(event.target.value))} required /></label>
          <label>{t('teams_expiry')}
            <select value={ttl} onChange={event => setTtl(Number(event.target.value))}>
              <option value={3600}>{t('teams_hour')}</option><option value={86400}>{t('teams_day')}</option><option value={604800}>{t('teams_week')}</option>
            </select>
          </label>
        </div>
        <button type="submit" className="tw-primary" disabled={snapshot.busy}>{t('teams_invite_create')}</button>
      </form>
      {snapshot.inviteLink ? <div className="tw-share">
        <label>{t('teams_invite_share')}<input readOnly value={snapshot.inviteLink} onFocus={event => event.target.select()} /></label>
        <button type="button" onClick={() => copy(snapshot.inviteLink)}><Icon name="copy" />{t('tw_copy')}</button>
      </div> : null}
      {notice ? <p role="status" className="tw-hint">{notice}</p> : null}
      <ul className="tw-invite-list" aria-label={t('tw_team_invitations')}>
        {invites.map(invite => <li key={invite.id}>
          <span>{invite.role === 'admin' ? t('teams_admin') : t('teams_member')} · {invite.uses}/{invite.maxUses} · {new Date(invite.expiresAt).toLocaleString(tag)}</span>
          {invite.revoked ? <small>{t('teams_revoked')}</small>
            : <button type="button" disabled={snapshot.busy || (team.role !== 'owner' && invite.role === 'admin')}
              onClick={() => ask({ action: 'revokeInvite', teamId: team.id, inviteId: invite.id }, t('teams_confirm_revoke_title'),
                t('teams_confirm_revoke_body'), t('teams_revoke'))}>{t('teams_revoke')}</button>}
        </li>)}
        {!invites.length ? <li className="tw-empty-line">{t('teams_invite_none')}</li> : null}
      </ul>
    </div>;
  };

  return <aside className="tw-scope tw-drawer" role="complementary" aria-label={t('teams_team_details')} style={{ width }} onKeyDown={escape}>
    <div className="tw-resize" role="separator" aria-orientation="vertical" aria-label={t('tw_team_resize')}
      aria-valuemin={DRAWER_MIN} aria-valuemax={DRAWER_MAX} aria-valuenow={width} tabIndex={0}
      onPointerDown={resizeDown} onPointerMove={resizeMove} onPointerUp={resizeUp} onPointerCancel={resizeUp} onKeyDown={resizeKey} />
    <header className="tw-drawer-head">
      <TeamMark id={team.id} name={team.name} size={56} />
      <div className="tw-drawer-title">
        <h2 ref={heading} tabIndex={-1}>{team.name}</h2>
        <p>{role(team.role)} · {t(team.memberCount === 1 ? 'teams_members_count_one_fmt' : 'teams_members_count_other_fmt', team.memberCount)}</p>
      </div>
      <button type="button" className="tw-icon-btn" aria-label={t('tw_team_close_details')} title={t('tw_team_close_details')} onClick={onClose}><Icon name="x" /></button>
    </header>
    {conversation && chatLabel ? <div className="tw-drawer-chat">
      <ConversationLead conversation={conversation} label={chatLabel} snapshot={snapshot} locale={locale} size={32} render={renderAvatar} />
      <div className="tw-drawer-title"><p>{t('teams_conversation_details')}</p><strong>{chatLabel}</strong></div>
    </div> : null}
    {conversation?.kind === 'channel' && workspace.api.datasets && !snapshot.offline && (
      <ChannelVisualizationSettings key={team.id + ':' + conversation.id} api={workspace.api} locale={locale}
        scope={{ teamId: team.id, generation: team.generation, conversationId: conversation.id }} />
    )}
    {snapshot.error ? <p role="alert" className="tw-error tw-drawer-error">{teamsErrorText(snapshot.error, locale)}</p> : null}
    <div className="tw-tabs" role="tablist" aria-label={t('teams_team_details')} onKeyDown={tabKey}>
      {tabs.map(value => <button key={value} type="button" role="tab" id={`${id}-tab-${value}`} aria-selected={value === current} aria-controls={`${id}-panel`} tabIndex={value === current ? 0 : -1}
        className="tw-tab" onClick={() => choose(value)}>{tabLabel(value)}</button>)}
    </div>
    <div className="tw-drawer-body" role="tabpanel" id={`${id}-panel`} aria-labelledby={`${id}-tab-${current}`} tabIndex={0}>{body()}</div>
    <footer className="tw-drawer-footer">
      <details className="tw-panel-disclosure">
        <summary><Icon name="lock" size={15} />{t('tw_team_platform_readable')}</summary>
        <p className="tw-hint">{platformNotice(locale)}</p>
      </details>
      <button type="button" className="tw-link tw-danger-btn" disabled={snapshot.busy} onClick={leaveOrDelete}>{team.role === 'owner' ? t('teams_delete') : t('teams_leave')}</button>
    </footer>
    {confirmation ? <Dialog role="alertdialog" title={confirmation.title} description={confirmation.body} onClose={() => setConfirmation(undefined)}>
      <div className="tw-form">
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setConfirmation(undefined)}>{t('common_cancel')}</button>
          <button type="button" className={confirmation.danger ? 'tw-danger-btn' : 'tw-primary'} disabled={snapshot.busy} onClick={() => void confirmNow()}>{confirmation.confirm}</button>
        </div>
      </div>
    </Dialog> : null}
  </aside>;
}
