import { useEffect, useRef, useState, type ReactNode } from 'react';
import { BotDirectory } from './BotDirectory';
import { TeamChannelAdmins } from './ChannelManagement';
import { ChatTopbarHeader, TopbarConnection } from './ChatTopbar';
import { messageErrorText, teamsErrorText } from './errors';
import { intlTag, useTeamsText } from './i18n';
import type { TeamCommand, TeamMember, TeamRole } from './index';
import { SettingsLayout, SettingsRole, type SettingsSection } from './SettingsLayout';
import { TeamMemberRemoval } from './TeamMemberRemoval';
import { TeamProfileSettings } from './TeamProfileSettings';
import { Avatar, Dialog, Icon, platformNotice, TeamMark, type RenderAvatar } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace';

export type TeamSettingsSection = 'general' | 'members' | 'invites' | 'channelAdmins' | 'bots';

/** Owners manage everyone but themselves and other owners; admins manage plain members. */
export const canManageMember = (viewerRole: TeamRole, viewerUid: string, member: Pick<TeamMember, 'uid' | 'role'>): boolean =>
  member.uid !== viewerUid && member.role !== 'owner' && (viewerRole === 'owner' || (viewerRole === 'admin' && member.role === 'member'));

export interface TeamSettingsPageProps {
  workspace: TeamsWorkspace;
  snapshot: TeamsSnapshot;
  locale: string;
  initialSection?: TeamSettingsSection;
  renderAvatar?: RenderAvatar;
  onClose(): void;
  onOpenChat(): void;
  onAddFriend?(member: TeamMember): void;
  onInviteFriends?(): void;
  status?: ReactNode;
}

interface Confirmation { command: TeamCommand; title: string; body: string; confirm: string; danger: boolean }

export function TeamSettingsPage({ workspace, snapshot, locale, initialSection, renderAvatar, onClose, onOpenChat, onAddFriend, onInviteFriends, status }: TeamSettingsPageProps) {
  const t = useTeamsText(locale), tag = intlTag(locale);
  const [section, setSection] = useState<TeamSettingsSection>(initialSection ?? 'general');
  const [confirmation, setConfirmation] = useState<Confirmation>();
  const [removing, setRemoving] = useState<Pick<TeamMember, 'uid' | 'generation'>>();
  const [uses, setUses] = useState(1);
  const [ttl, setTtl] = useState(86400);
  const [inviteRole, setInviteRole] = useState<'member' | 'admin'>('member');
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const heading = useRef<HTMLHeadingElement>(null);
  const page = useRef<HTMLElement>(null);
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  useEffect(() => { setConfirmation(undefined); setRemoving(undefined); setError(''); }, [snapshot.authority, snapshot.team?.role]);

  const { team, detail, uid } = snapshot;
  if (!team) return null;
  const canManage = team.role !== 'member';
  const busy = snapshot.busy || snapshot.offline;
  const sections: SettingsSection<TeamSettingsSection>[] = [
    { id: 'general', text: t('settings_group_general'), icon: 'sliders' },
    { id: 'members', text: t('tw_team_members_roles'), icon: 'person' },
    ...(canManage ? [
      { id: 'invites' as const, text: t('teams_invites'), icon: 'link' as const },
      { id: 'channelAdmins' as const, text: t('tw_channel_manage_admins'), icon: 'lock' as const },
    ] : []),
    { id: 'bots', text: t('remote_bots'), icon: 'bot' },
  ];
  const active = sections.find(item => item.id === section) ?? sections[0]!;
  const selectedMember = removing && detail?.members.find(member => member.uid === removing.uid && member.generation === removing.generation
    && canManageMember(team.role, uid, member));
  const role = (value: TeamRole) => ({ owner: t('teams_owner'), admin: t('teams_admin'), member: t('teams_member') })[value];
  const execute = async (command: TeamCommand) => {
    setNotice('');
    setError('');
    const ok = await workspace.execute(command);
    if (!ok) setError(workspace.getSnapshot().error || 'SERVICE_UNAVAILABLE');
    return ok;
  };
  const ask = (command: TeamCommand, title: string, body: string, confirm: string, danger = true) => setConfirmation({ command, title, body, confirm, danger });
  const leaveOrDelete = () => team.role === 'owner'
    ? ask({ action: 'delete', teamId: team.id }, t('teams_confirm_delete_title_fmt', team.name), t('teams_confirm_delete_body'), t('teams_confirm_delete_button'))
    : ask({ action: 'leave', teamId: team.id, generation: team.generation }, t('teams_confirm_leave_title_fmt', team.name), t('teams_confirm_leave_body'), t('teams_confirm_leave_button'));
  const copy = async (link: string) => {
    try { await navigator.clipboard.writeText(link); setNotice(t('teams_link_copied')); }
    catch { setNotice(t('tw_team_copy_hint')); }
  };
  const loading = snapshot.loadError && !snapshot.detailLoading
    ? <div className="tw-form"><p role="alert" className="tw-error">{teamsErrorText(snapshot.loadError, locale)}</p>
      <button type="button" onClick={() => void workspace.refresh()}>{t('common_retry')}</button></div>
    : <p role="status">{t('teams_loading')}</p>;
  const count = t(team.memberCount === 1 ? 'teams_members_count_one_fmt' : 'teams_members_count_other_fmt', team.memberCount);
  const label = t('tw_team_settings');
  const back = <button type="button" className="tw-icon-btn" aria-label={t('tw_team_settings_back')} title={t('tw_team_settings_back')} onClick={onClose}>
    <Icon name="chevronLeft" />
  </button>;

  return <section ref={page} className="tw-scope tw-settings-page tw-team-settings-page" aria-label={label} onKeyDown={event => {
    if (event.key !== 'Escape' || event.defaultPrevented || confirmation || selectedMember
      || (event.target as HTMLElement).closest?.('dialog')) return;
    event.preventDefault();
    event.stopPropagation();
    onClose();
  }}>
    <ChatTopbarHeader><header className="tw-conv-head">
      {back}
      <div className="tw-conv-identity">
        <TeamMark id={team.id} name={team.name} avatar={detail?.profile?.avatar} />
        <div className="tw-conv-title"><h2>{team.name}</h2></div>
      </div>
      <TopbarConnection snapshot={snapshot} workspace={workspace} locale={locale} />
    </header></ChatTopbarHeader>
    {status}
    <SettingsLayout label={label} sections={sections} active={active} heading={heading} onSelect={next => {
      setSection(next);
      setNotice('');
      setError('');
      if (page.current) page.current.scrollTop = 0;
    }}>
      {error && !confirmation ? <p role="alert" className="tw-error">{teamsErrorText(error, locale)}</p> : null}
      <div hidden={active.id !== 'general'}>
        {active.id === 'general' && !detail ? loading : null}
        <TeamProfileSettings workspace={workspace} snapshot={snapshot} team={team} locale={locale} />
        <section className="tw-team-settings-membership" aria-label={t('teams_team_details')}>
          <div className="tw-role-line"><SettingsRole elevated={canManage}>{role(team.role)}</SettingsRole><span>{count}</span></div>
          <details className="tw-panel-disclosure">
            <summary><Icon name="lock" size={15} />{t('tw_team_platform_readable')}</summary>
            <p className="tw-hint">{platformNotice(locale)}</p>
          </details>
          <button type="button" className="tw-link tw-danger-btn" disabled={busy} onClick={leaveOrDelete}>{team.role === 'owner' ? t('teams_delete') : t('teams_leave')}</button>
        </section>
      </div>
      {active.id === 'members' ? detail ? <div className="tw-settings-group">
        <p className="tw-hint">{t('tw_team_roles_hint')}</p>
        <ul className="tw-people tw-settings-fields" aria-label={t('teams_members')}>
          {detail.members.map(member => {
            const identity = <>
              <Avatar identity={{ kind: 'user', id: member.uid, name: member.displayName }} size={40} render={renderAvatar} />
              <div className="tw-person-text"><b>{member.displayName}</b>
                <div className="tw-role-line"><SettingsRole elevated={member.role !== 'member'}>{role(member.role)}</SettingsRole>
                  {member.uid === uid ? <small>{t('teams_you')}</small> : null}</div>
              </div>
            </>;
            const friend = onAddFriend && member.uid !== uid;
            const manage = canManageMember(team.role, uid, member);
            return <li key={member.uid}>
              {friend || manage ? <details className="tw-member-actions">
                <summary className="tw-person-row" aria-label={`${member.displayName} · ${t('tw_more')}`}>
                  {identity}<span className="tw-person-more"><Icon name="more" /></span>
                </summary>
                <div className="tw-person-actions">
                  {friend ? <button type="button" onClick={() => onAddFriend(member)}>{t('friends_add')}</button> : null}
                  {manage && team.role === 'owner' ? <>
                    <button type="button" disabled={busy} onClick={() => ask(
                      { action: 'role', teamId: team.id, uid: member.uid, generation: member.generation, role: member.role === 'admin' ? 'member' : 'admin' },
                      t(member.role === 'admin' ? 'teams_confirm_demote_title_fmt' : 'teams_confirm_promote_title_fmt', member.displayName),
                      t(member.role === 'admin' ? 'teams_confirm_demote_body' : 'teams_confirm_promote_body'),
                      t(member.role === 'admin' ? 'teams_demote' : 'teams_promote'), false)}>
                      {t(member.role === 'admin' ? 'teams_demote' : 'teams_promote')}
                    </button>
                    <button type="button" disabled={busy} onClick={() => ask(
                      { action: 'handOver', teamId: team.id, uid: member.uid, generation: member.generation },
                      t('tw_team_owner_title_fmt', member.displayName, team.name), t('tw_team_owner_body'), t('teams_make_owner'), false)}>
                      {t('teams_make_owner')}
                    </button>
                  </> : null}
                  {manage ? <button type="button" className="tw-danger-btn" disabled={busy}
                    onClick={() => { setError(''); setRemoving(member); }}>{t('teams_confirm_remove_button')}</button> : null}
                </div>
              </details> : <div className="tw-person-row">{identity}</div>}
            </li>;
          })}
        </ul>
      </div> : loading : null}
      {active.id === 'invites' ? detail ? <div className="tw-invites">
        {onInviteFriends ? <button type="button" disabled={busy} onClick={onInviteFriends}>{t('friends_invite_action')}</button> : null}
        <form className="tw-form tw-settings-fields" onSubmit={event => {
          event.preventDefault();
          void execute({ action: 'invite', teamId: team.id, requestId: crypto.randomUUID(), role: team.role === 'owner' ? inviteRole : 'member', maxUses: uses, ttlSeconds: ttl });
        }}>
          <label>{t('tw_team_join_role')}
            <select value={team.role === 'owner' ? inviteRole : 'member'} disabled={team.role !== 'owner'} onChange={event => setInviteRole(event.currentTarget.value as 'member' | 'admin')}>
              <option value="member">{t('teams_member')}</option>{team.role === 'owner' ? <option value="admin">{t('teams_admin')}</option> : null}
            </select>
          </label>
          <div className="tw-form-row">
            <label>{t('teams_uses')}<input type="number" min={1} max={50} value={uses} onChange={event => setUses(Number(event.currentTarget.value))} required /></label>
            <label>{t('teams_expiry')}<select value={ttl} onChange={event => setTtl(Number(event.currentTarget.value))}>
              <option value={3600}>{t('teams_hour')}</option><option value={86400}>{t('teams_day')}</option><option value={604800}>{t('teams_week')}</option>
            </select></label>
          </div>
          <div><button type="submit" className="tw-primary" disabled={busy}>{t('teams_invite_create')}</button></div>
        </form>
        {snapshot.inviteLink ? <div className="tw-share">
          <label>{t('teams_invite_share')}<input readOnly value={snapshot.inviteLink} onFocus={event => event.currentTarget.select()} /></label>
          <button type="button" onClick={() => void copy(snapshot.inviteLink)}><Icon name="copy" />{t('tw_copy')}</button>
        </div> : null}
        {notice ? <p role="status" className="tw-hint">{notice}</p> : null}
        <ul className="tw-invite-list" aria-label={t('tw_team_invitations')}>
          {detail.invites.map(invite => <li key={invite.id}>
            <span>{role(invite.role)} · {invite.uses}/{invite.maxUses} · {new Date(invite.expiresAt).toLocaleString(tag)}</span>
            {invite.revoked ? <small>{t('teams_revoked')}</small>
              : <button type="button" disabled={busy || (team.role !== 'owner' && invite.role === 'admin')}
                onClick={() => ask({ action: 'revokeInvite', teamId: team.id, inviteId: invite.id }, t('teams_confirm_revoke_title'),
                  t('teams_confirm_revoke_body'), t('teams_revoke'))}>{t('teams_revoke')}</button>}
          </li>)}
          {!detail.invites.length ? <li className="tw-empty-line">{t('teams_invite_none')}</li> : null}
        </ul>
      </div> : loading : null}
      {active.id === 'channelAdmins' ? <TeamChannelAdmins workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderAvatar} /> : null}
      {active.id === 'bots' ? <div className="tw-embed">
        <BotDirectory key={snapshot.authority} api={workspace.api} uid={uid} team={team} members={snapshot.directory} bots={snapshot.bots} locale={locale}
          busy={snapshot.messageBusy} renderAvatar={renderAvatar}
          status={snapshot.botsUnverified ? t('teams_bots_unverified') : !snapshot.botsLoaded ? t('teams_bots_loading') : ''}
          attempt={workspace.attempt} onChanged={workspace.refreshBots}
          onOpen={async bot => { await workspace.startBotDm(bot); onOpenChat(); }}
          onExample={async (bot, example) => { if (await workspace.openBotExample(bot, example)) onOpenChat(); }}
          onOpenChat={id => { workspace.selectConversation(id); onOpenChat(); }} />
        {snapshot.actionError ? <p role="alert" className="tw-error">{messageErrorText(snapshot.actionError, locale)}</p> : null}
      </div> : null}
    </SettingsLayout>
    {confirmation ? <Dialog role="alertdialog" title={confirmation.title} description={confirmation.body} onClose={() => setConfirmation(undefined)}>
      {error ? <p role="alert" className="tw-error">{teamsErrorText(error, locale)}</p> : null}
      <div className="tw-dialog-actions">
        <button type="button" data-autofocus onClick={() => setConfirmation(undefined)}>{t('common_cancel')}</button>
        <button type="button" className={confirmation.danger ? 'tw-danger-btn' : 'tw-primary'} disabled={busy} onClick={() => {
          void execute(confirmation.command).then(ok => { if (ok) setConfirmation(undefined); });
        }}>{confirmation.confirm}</button>
      </div>
    </Dialog> : null}
    {selectedMember ? <TeamMemberRemoval key={selectedMember.uid} workspace={workspace} snapshot={snapshot} team={team} member={selectedMember} locale={locale}
      onClose={() => setRemoving(undefined)} /> : null}
  </section>;
}
