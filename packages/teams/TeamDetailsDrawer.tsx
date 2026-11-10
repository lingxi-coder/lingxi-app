import { useEffect, useId, useRef, useState, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactElement } from 'react';
import { messageErrorText, teamsErrorText } from './errors.js';
import type { TeamCommand, TeamMember, TeamRole } from './index.js';
import { BotDirectory } from './BotDirectory';
import { Avatar, Dialog, Icon, platformNotice, rovingIndex, type DrawerTab, type RenderAvatar } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

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
  zh: boolean;
  /** The shown tab, when the host keeps it; otherwise the drawer does. */
  tab?: DrawerTab;
  onTabChange?(tab: DrawerTab): void;
  /** The width in CSS pixels, when the host keeps it; otherwise the drawer does, starting at `defaultWidth`. */
  width?: number;
  defaultWidth?: number;
  onWidthChange?(width: number): void;
  onClose(): void;
  renderAvatar?: RenderAvatar;
  locale?: string;
  /** Without it members have no Add friend action (D15). The host looks the person up and shows the result. */
  onAddFriend?(member: TeamMember): void;
}

interface Confirmation { command: TeamCommand; title: string; confirm: string }

/**
 * The right column: the open team's members, invitations and Bots. Leaving or deleting the team is
 * at the top, behind a confirmation. The left edge is a resize handle (280 to 480 pixels); Escape closes.
 */
export function TeamDetailsDrawer({ workspace, snapshot, zh, tab, onTabChange, width: controlledWidth, defaultWidth, onWidthChange, onClose, renderAvatar, locale, onAddFriend }: TeamDetailsDrawerProps): ReactElement | null {
  const t = (en: string, cn: string) => zh ? cn : en;
  const tag = locale ?? (zh ? 'zh-CN' : 'en-US');
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
  const role = (value: TeamRole) => ({ owner: t('Owner', '所有者'), admin: t('Admin', '管理员'), member: t('Member', '成员') })[value];
  const tabLabel = (value: DrawerTab) => ({ members: t('Members', '成员'), invites: t('Invites', '邀请'), bots: 'Bots' })[value];

  const ask = (command: TeamCommand, title: string, confirm: string) => setConfirmation({ command, title, confirm });
  const confirmNow = async () => {
    const pending = confirmation;
    if (!pending) return;
    const ok = await workspace.execute(pending.command);
    setConfirmation(undefined);
    // The workspace moves on to another team after this one is left or deleted; the drawer was about the old one.
    if (ok && (pending.command.action === 'leave' || pending.command.action === 'delete')) onClose();
  };
  const leaveOrDelete = team.role === 'owner'
    ? () => ask({ action: 'delete', teamId: team.id }, t(`Delete ${team.name} for everyone?`, `删除团队「${team.name}」？所有成员将失去访问权限。`), t('Delete team', '删除团队'))
    : () => ask({ action: 'leave', teamId: team.id, generation: team.generation }, t(`Leave ${team.name}?`, `退出团队「${team.name}」？`), t('Leave team', '退出团队'));

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
    const done = () => setNotice(t('Link copied', '链接已复制')), fail = () => setNotice(t('Select and copy the link above.', '请选中上方链接并复制。'));
    if (!navigator.clipboard) { fail(); return; }
    navigator.clipboard.writeText(link).then(done, fail);
  };
  const members = detail?.members ?? [];
  const invites = detail?.invites ?? [];
  const body = (): ReactElement => {
    if (current === 'bots') {
      return <div className="tw-embed">
        <BotDirectory key={`${uid}:${team.id}:${team.generation}:${snapshot.authority}`} api={workspace.api} uid={uid} team={team} members={snapshot.directory} bots={snapshot.bots} zh={zh} busy={snapshot.messageBusy}
          status={snapshot.botsUnverified ? t('Bot availability could not be verified.', '未能确认 Bot 可用状态。') : !snapshot.botsLoaded ? t('Loading Bots…', '正在加载 Bot…') : ''}
          attempt={workspace.attempt} onOpen={bot => workspace.startBotDm(bot)} onChanged={() => workspace.refreshBots()} />
        {snapshot.actionError ? <p role="alert" className="tw-error">{messageErrorText(snapshot.actionError, zh)}</p> : null}
      </div>;
    }
    if (!detail) {
      return snapshot.loadError && !snapshot.detailLoading
        ? <div className="tw-form"><p role="alert" className="tw-error">{teamsErrorText(snapshot.loadError, zh)}</p><button type="button" onClick={() => void workspace.refresh()}>{t('Retry', '重试')}</button></div>
        : <p className="tw-empty-line" role="status">{t('Loading…', '正在加载…')}</p>;
    }
    if (current === 'members') {
      return <ul className="tw-people" aria-label={t('Members', '成员')}>
        {members.map(member => <li key={member.uid}>
          <Avatar identity={{ kind: 'user', id: member.uid, name: member.displayName }} size={32} render={renderAvatar} />
          <div className="tw-person-text"><b>{member.displayName}</b><small>{member.uid === uid ? `${t('You', '你')} · ` : ''}{role(member.role)}</small></div>
          {onAddFriend && member.uid !== uid || canManageMember(team.role, uid, member) ? <div className="tw-person-actions">
            {onAddFriend && member.uid !== uid ? <button type="button" onClick={() => onAddFriend(member)}>{t('Add friend', '添加好友')}</button> : null}
            {canManageMember(team.role, uid, member) ? <>
              {team.role === 'owner' ? <button type="button" disabled={snapshot.busy}
                onClick={() => ask({ action: 'role', teamId: team.id, uid: member.uid, generation: member.generation, role: member.role === 'admin' ? 'member' : 'admin' }, t(`Change ${member.displayName}’s role?`, `更改 ${member.displayName} 的角色？`), t('Change role', '更改角色'))}>
                {member.role === 'admin' ? t('Make member', '设为成员') : t('Make admin', '设为管理员')}</button> : null}
              <button type="button" className="tw-danger-btn" disabled={snapshot.busy}
                onClick={() => ask({ action: 'remove', teamId: team.id, uid: member.uid, generation: member.generation }, t(`Remove ${member.displayName}?`, `移除 ${member.displayName}？`), t('Remove', '移除'))}>{t('Remove', '移除')}</button>
            </> : null}
          </div> : null}
        </li>)}
      </ul>;
    }
    return <div className="tw-invites">
      <form className="tw-form" onSubmit={event => { event.preventDefault(); void workspace.execute({ action: 'invite', teamId: team.id, requestId: crypto.randomUUID(), role: team.role === 'owner' ? inviteRole : 'member', maxUses: uses, ttlSeconds: ttl }); }}>
        <label>{t('Role', '加入角色')}
          <select value={team.role === 'owner' ? inviteRole : 'member'} disabled={team.role !== 'owner'} onChange={event => setInviteRole(event.target.value as 'member' | 'admin')}>
            <option value="member">{t('Member', '成员')}</option>{team.role === 'owner' ? <option value="admin">{t('Admin', '管理员')}</option> : null}
          </select>
        </label>
        <div className="tw-form-row">
          <label>{t('Uses', '可用次数')}<input type="number" min={1} max={50} value={uses} onChange={event => setUses(Number(event.target.value))} required /></label>
          <label>{t('Expires in', '有效期')}
            <select value={ttl} onChange={event => setTtl(Number(event.target.value))}>
              <option value={3600}>{t('1 hour', '1 小时')}</option><option value={86400}>{t('1 day', '1 天')}</option><option value={604800}>{t('7 days', '7 天')}</option>
            </select>
          </label>
        </div>
        <button type="submit" className="tw-primary" disabled={snapshot.busy}>{t('Create invite link', '创建邀请链接')}</button>
      </form>
      {snapshot.inviteLink ? <div className="tw-share">
        <label>{t('Share this invitation', '分享邀请')}<input readOnly value={snapshot.inviteLink} onFocus={event => event.target.select()} /></label>
        <button type="button" onClick={() => copy(snapshot.inviteLink)}><Icon name="copy" />{t('Copy', '复制')}</button>
      </div> : null}
      {notice ? <p role="status" className="tw-hint">{notice}</p> : null}
      <ul className="tw-invite-list" aria-label={t('Invitations', '邀请')}>
        {invites.map(invite => <li key={invite.id}>
          <span>{invite.role === 'admin' ? t('Admin', '管理员') : t('Member', '成员')} · {invite.uses}/{invite.maxUses} · {new Date(invite.expiresAt).toLocaleString(tag)}</span>
          {invite.revoked ? <small>{t('Revoked', '已撤销')}</small>
            : <button type="button" disabled={snapshot.busy || (team.role !== 'owner' && invite.role === 'admin')} onClick={() => void workspace.execute({ action: 'revokeInvite', teamId: team.id, inviteId: invite.id })}>{t('Revoke', '撤销')}</button>}
        </li>)}
        {!invites.length ? <li className="tw-empty-line">{t('No invitations yet.', '还没有邀请。')}</li> : null}
      </ul>
    </div>;
  };

  return <aside className="tw-scope tw-drawer" role="complementary" aria-label={t('Team details', '团队详情')} style={{ width }} onKeyDown={escape}>
    <div className="tw-resize" role="separator" aria-orientation="vertical" aria-label={t('Resize details panel', '调整详情面板宽度')}
      aria-valuemin={DRAWER_MIN} aria-valuemax={DRAWER_MAX} aria-valuenow={width} tabIndex={0}
      onPointerDown={resizeDown} onPointerMove={resizeMove} onPointerUp={resizeUp} onPointerCancel={resizeUp} onKeyDown={resizeKey} />
    <header className="tw-drawer-head">
      <div className="tw-drawer-title">
        <h2 ref={heading} tabIndex={-1}>{team.name}</h2>
        <p>{role(team.role)} · {team.memberCount} {t('members', '位成员')} · {t('Platform readable', '平台可读')}</p>
      </div>
      <button type="button" className="tw-icon-btn" aria-label={t('Close details', '关闭详情')} title={t('Close details', '关闭详情')} onClick={onClose}><Icon name="x" /></button>
    </header>
    <div className="tw-drawer-danger">
      <button type="button" className="tw-danger-btn" disabled={snapshot.busy} onClick={leaveOrDelete}>{team.role === 'owner' ? t('Delete team', '删除团队') : t('Leave team', '退出团队')}</button>
      <p className="tw-hint">{platformNotice(zh)}</p>
    </div>
    {snapshot.error ? <p role="alert" className="tw-error tw-drawer-error">{teamsErrorText(snapshot.error, zh)}</p> : null}
    <div className="tw-tabs" role="tablist" aria-label={t('Team details', '团队详情')} onKeyDown={tabKey}>
      {tabs.map(value => <button key={value} type="button" role="tab" id={`${id}-tab-${value}`} aria-selected={value === current} aria-controls={`${id}-panel`} tabIndex={value === current ? 0 : -1}
        className="tw-tab" onClick={() => choose(value)}>{tabLabel(value)}</button>)}
    </div>
    <div className="tw-drawer-body" role="tabpanel" id={`${id}-panel`} aria-labelledby={`${id}-tab-${current}`} tabIndex={0}>{body()}</div>
    {confirmation ? <Dialog role="alertdialog" title={confirmation.title} onClose={() => setConfirmation(undefined)}>
      <div className="tw-form">
        {confirmation.command.action === 'delete' ? <p className="tw-hint">{t('This cannot be undone.', '此操作无法撤销。')}</p> : null}
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus disabled={snapshot.busy} onClick={() => setConfirmation(undefined)}>{t('Cancel', '取消')}</button>
          <button type="button" className="tw-danger-btn" disabled={snapshot.busy} onClick={() => void confirmNow()}>{confirmation.confirm}</button>
        </div>
      </div>
    </Dialog> : null}
  </aside>;
}
