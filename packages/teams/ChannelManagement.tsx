import { useEffect, useId, useRef, useState } from 'react';
import { ChatTopbarHeader, TopbarConnection } from './ChatTopbar';
import { ChannelSettings as BotSettings } from './ChannelSettings';
import { channelCommand, channelInviteLink, DEFAULT_POSTING, type ChannelInvite, type ChannelManagement as Management, type ChannelPosting, type ChannelRequest } from './channel-settings';
import { teamsText, useTeamsText } from './i18n';
import type { TeamRole } from './index';
import type { TeamConversation } from './messages';
import { Avatar, ConversationLead, Dialog, Icon, type IconName, type RenderAvatar } from './ui';
import { SettingsLayout, SettingsRole } from './SettingsLayout';
import { conversationParticipants, type Identity, type TeamsSnapshot, type TeamsWorkspace } from './workspace';

type Context = { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; locale: string };
type ChannelResponse = Partial<Management> & { channels?: { id: string; title: string }[]; blockedBy?: { id: string; name: string }[]; conversationId?: string };
function channelError(error: unknown, locale: string) {
  const t = teamsText(locale), code = error instanceof Error ? error.message : String(error);
  if (code.includes('CHANNEL_TEAM_REQUIRED')) return t('tw_channel_team_required');
  if (code.includes('CHANNEL_INVITE_INVALID')) return t('tw_channel_invite_invalid');
  if (code.includes('CHANNEL_ADMIN_REQUIRED')) return t('tw_channel_admin_required');
  if (code.includes('CHANNEL_MEMBER_REQUIRED')) return t('tw_channel_member_required');
  if (code.includes('CHANNEL_INVITE_CHANGED')) return t('tw_channel_invite_changed');
  if (code.includes('CHANNEL_MEMBER_LIMIT')) return t('tw_channel_member_limit');
  return t('tw_channel_action_failed');
}

function makeRequest(workspace: TeamsWorkspace, teamId: string, generation: number, request: ChannelRequest) {
  return workspace.api.messages<ChannelResponse>(channelCommand({ teamId, generation }, request));
}

function ChannelMemberIdentity({ identity, admin, teamRole, locale, renderAvatar }: {
  identity: Identity; admin: boolean; teamRole?: TeamRole; locale: string; renderAvatar?: RenderAvatar;
}) {
  const t = useTeamsText(locale);
  return <>
    <Avatar identity={identity} size={32} render={renderAvatar} />
    <div className="tw-person-text"><b>{identity.name}</b><div className="tw-role-line">
      {identity.kind === 'bot' ? <SettingsRole>{t('teams_bot_tag')}</SettingsRole> : <>
        {teamRole === 'owner' ? <SettingsRole elevated>{t('tw_role_team_owner')}</SettingsRole> : null}
        {teamRole === 'admin' ? <SettingsRole elevated>{t('tw_role_team_admin')}</SettingsRole> : null}
        {admin ? <SettingsRole elevated>{t('tw_role_channel_admin')}</SettingsRole> : null}
        {!admin && teamRole !== 'owner' && teamRole !== 'admin' ? <SettingsRole>{t('teams_member')}</SettingsRole> : null}
      </>}
    </div></div>
  </>;
}

/** Shared by the channel's Roles page and the team's management entry. */
function AdministratorList({ management, snapshot, locale, busy, change, renderAvatar }: {
  management: Management; snapshot: TeamsSnapshot; locale: string; busy: boolean; change(uid: string, enabled: boolean): void; renderAvatar?: RenderAvatar;
}) {
  const t = useTeamsText(locale);
  return <ul className="tw-settings-fields tw-channel-roster" aria-label={t('tw_channel_roles')}>{management.members.map(uid => {
    const admin = management.admins.includes(uid);
    const member = snapshot.directory.find(person => person.uid === uid);
    const name = member?.displayName || uid;
    const action = t(admin ? 'tw_channel_dismiss_admin' : 'tw_channel_appoint_admin');
    return <li key={uid}><ChannelMemberIdentity identity={{ kind: 'user', id: uid, name }} admin={admin} teamRole={member?.role} locale={locale} renderAvatar={renderAvatar} />
      <button type="button" disabled={busy} aria-label={`${action} · ${name}`} onClick={() => change(uid, !admin)}>{action}</button></li>;
  })}</ul>;
}
type Section = 'general' | 'posting' | 'members' | 'invites' | 'admins' | 'bots';

export function ChannelSettingsPage({ workspace, snapshot, locale, conversation, onClose, renderAvatar }: Context & { conversation: TeamConversation; onClose(): void; renderAvatar?: RenderAvatar }) {
  const t = useTeamsText(locale), team = snapshot.team!, scope = { teamId: team.id, generation: team.generation };
  const isTeamManager = team.role !== 'member';
  const canManage = isTeamManager || conversation.channelAdmins?.[`user:${snapshot.uid}`] === team.generation;
  const people = conversationParticipants(conversation, snapshot.directory, snapshot.bots, snapshot.uid, locale);
  const [section, setSection] = useState<Section>('general');
  const [profile, setProfile] = useState<{ title: string; description: string }>();
  const [posting, setPosting] = useState<ChannelPosting>();
  const [management, setManagement] = useState<Management>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [notice, setNotice] = useState('');
  const [confirmation, setConfirmation] = useState<{ title: string; action(): void }>();
  const [blocked, setBlocked] = useState<{ id: string; name: string }[]>([]);
  const heading = useRef<HTMLHeadingElement>(null);
  const page = useRef<HTMLElement>(null);
  const fieldsId = useId();
  useEffect(() => { heading.current?.focus({ preventScroll: true }); }, []);
  const sections: { id: Section; text: string; icon: IconName }[] = [
    { id: 'general', text: t('settings_group_general'), icon: 'sliders' },
    { id: 'posting', text: t('tw_channel_posting'), icon: 'send' },
    { id: 'members', text: t('teams_members'), icon: 'person' },
    ...(canManage ? [{ id: 'invites' as const, text: t('tw_channel_invite_links'), icon: 'link' as const }] : []),
    { id: 'admins', text: t('tw_channel_roles'), icon: 'lock' },
    ...(people.some(person => person.kind === 'bot') ? [{ id: 'bots' as const, text: t('remote_bots'), icon: 'bot' as const }] : []),
  ];
  const active = sections.find(item => item.id === section) ?? sections[0]!;
  useEffect(() => {
    if (!(active.id === 'invites' && canManage || active.id === 'admins' && isTeamManager)) return;
    let live = true;
    setManagement(undefined);
    makeRequest(workspace, team.id, team.generation, { op: 'get', conversationId: conversation.id }).then(result => {
      if (live) setManagement({ members: result.members ?? [], admins: result.admins ?? [], code: result.code ?? null });
    }).catch(e => { if (live) setError(channelError(e, locale)); });
    return () => { live = false; };
  }, [workspace, team.id, team.generation, conversation.id, active.id, canManage, isTeamManager, locale]);
  const run = async (request: ChannelRequest, done?: (result: ChannelResponse) => void) => {
    if (busy) return;
    setBusy(true); setError(''); setNotice('');
    try { done?.(await makeRequest(workspace, team.id, team.generation, request)); }
    catch (e) { setError(channelError(e, locale)); }
    finally { setBusy(false); }
  };
  const id = conversation.id, values = profile ?? { title: conversation.title, description: conversation.description ?? '' };
  const rule = posting ?? conversation.posting ?? DEFAULT_POSTING;
  const save = (disabled: boolean) => <footer><button type="submit" className="tw-primary" disabled={disabled || busy || snapshot.offline}>{t('common_save')}</button></footer>;
  const currentPrincipals = new Set(people.map(p => (p.kind === 'bot' ? 'shared_bot:' : 'user:') + p.id));
  const selected = rule.selected.filter(principal => currentPrincipals.has(principal));
  const knownAdmins = snapshot.directory.filter(member => conversation.channelAdmins?.[`user:${member.uid}`] === member.generation
    && conversation.participants.includes(`user:${member.uid}`));
  return <section ref={page} className="tw-scope tw-settings-page tw-channel-settings-page" aria-label={t('tw_channel_settings')} onKeyDown={event => {
    if (event.key !== 'Escape' || event.defaultPrevented || confirmation) return;
    event.preventDefault();
    event.stopPropagation();
    onClose();
  }}>
    <ChatTopbarHeader><header className="tw-conv-head">
      <button type="button" className="tw-icon-btn" onClick={onClose} aria-label={t('tw_channel_back')} title={t('tw_channel_back')}><Icon name="chevronLeft" /></button>
      <div className="tw-conv-identity">
        <ConversationLead conversation={conversation} label={conversation.title} snapshot={snapshot} locale={locale} size={24} />
        <div className="tw-conv-title"><h2>{conversation.title}</h2></div>
      </div>
      <TopbarConnection snapshot={snapshot} workspace={workspace} locale={locale} />
    </header></ChatTopbarHeader>
    <SettingsLayout label={t('tw_channel_settings')} sections={sections} active={active} heading={heading}
      onSelect={next => {
        setSection(next);
        setError('');
        setNotice('');
        if (page.current) page.current.scrollTop = 0;
      }}>
        {error ? <p role="alert" className="tw-error">{error}</p> : null}
        {notice ? <p role="status">{notice}</p> : null}
        {active.id === 'general' ? <form className="tw-form" onSubmit={event => {
          event.preventDefault();
          void run({ op: 'profile', conversationId: id, ...values }, () => { setProfile(undefined); setNotice(t('teams_bot_scope_saved')); });
        }}>
          <fieldset className="tw-settings-fields">
            <label>{t('tw_channel_name')}<input required maxLength={120} value={values.title} readOnly={!canManage} onChange={event => setProfile({ ...values, title: event.target.value })} /></label>
            <label>{t('tw_channel_description')}<textarea rows={4} maxLength={1000} value={values.description} readOnly={!canManage} onChange={event => setProfile({ ...values, description: event.target.value })} /></label>
          </fieldset>{canManage ? save(!profile || !values.title.trim()) : null}
        </form> : null}
        {active.id === 'posting' ? <form className="tw-form" onSubmit={event => {
          event.preventDefault();
          void run({ op: 'posting', conversationId: id, posting: { ...rule, selected } }, () => { setPosting(undefined); setNotice(t('teams_bot_scope_saved')); });
        }}>
          <fieldset className="tw-settings-fields" disabled={!canManage}>
            <div className="tw-channel-field"><label htmlFor={fieldsId + '-mode'}>{t('tw_channel_mode')}</label>
              <select id={fieldsId + '-mode'} value={rule.mode} onChange={event => setPosting({ ...rule, mode: event.target.value as ChannelPosting['mode'] })}>
              <option value="chat">{t('drawer_tab_chat')}</option><option value="posts">{t('tw_channel_posts')}</option>
            </select></div>
            <div className="tw-channel-field"><label htmlFor={fieldsId + '-senders'}>{rule.mode === 'posts' ? t('tw_channel_post_permission') : t('tw_channel_send_permission')}</label>
              <select id={fieldsId + '-senders'} value={rule.senders} onChange={event => setPosting({ ...rule, senders: event.target.value as ChannelPosting['senders'] })}><option value="all">{t('tw_channel_all_members')}</option><option value="selected">{t('tw_channel_selected_members')}</option></select>
            </div>
            {rule.senders === 'selected' ? <fieldset className="tw-channel-picks"><legend>{t('tw_channel_selected_members')}</legend>{people.map(person => {
              const principal = (person.kind === 'bot' ? 'shared_bot:' : 'user:') + person.id;
              return <label key={principal}><input type="checkbox" checked={selected.includes(principal)} onChange={event => setPosting({
                ...rule, selected: event.target.checked ? [...selected, principal] : selected.filter(value => value !== principal),
              })} />{person.name}{person.kind === 'bot' ? ` · ${t('teams_bot_tag')}` : ''}</label>;
            })}</fieldset> : null}
            {rule.mode === 'posts' ? <div className="tw-channel-field"><label htmlFor={fieldsId + '-replies'}>{t('tw_channel_reply_permission')}</label>
              <select id={fieldsId + '-replies'} value={rule.replies} onChange={event => setPosting({ ...rule, replies: event.target.value as ChannelPosting['replies'] })}>
                <option value="all">{t('tw_channel_all_members')}</option><option value="authors">{t('tw_channel_posters')}</option><option value="none">{t('tw_channel_replies_closed')}</option>
              </select>
            </div> : null}
          </fieldset>{canManage ? save(!posting || rule.senders === 'selected' && !selected.length) : null}
        </form> : null}
        {active.id === 'members' ? <div className="tw-settings-group"><ul className="tw-settings-fields tw-channel-roster" aria-label={t('teams_members')}>{people.map(person => {
          const member = snapshot.directory.find(m => m.uid === person.id);
          const bot = snapshot.bots.find(bot => bot.sharedBotId === person.id);
          return <li key={person.kind + ':' + person.id}>
            <ChannelMemberIdentity identity={person} admin={person.kind === 'user' && knownAdmins.some(m => m.uid === person.id)}
              teamRole={person.self ? team.role : member?.role} locale={locale} renderAvatar={renderAvatar} />
            {canManage && person.kind === 'user' && !person.self && member?.role === 'member' && !knownAdmins.some(m => m.uid === person.id)
              ? <button type="button" disabled={busy || snapshot.offline} onClick={() => setConfirmation({ title: t('tw_channel_remove_fmt', person.name),
                action: () => { setBlocked([]); void run({ op: 'remove', conversationId: id, uid: person.id }, result => {
                  setBlocked(result.blockedBy ?? []);
                  if (!result.blockedBy?.length) setNotice(t('tw_channel_member_removed'));
                }); } })}>{t('teams_confirm_remove_button')}</button> : null}
            {person.kind === 'bot' && (isTeamManager || bot?.maintainer.uid === snapshot.uid)
              ? <button type="button" disabled={busy || snapshot.offline} onClick={() => setConfirmation({ title: t('tw_channel_remove_fmt', person.name),
                action: () => void run({ op: 'detachBot', conversationId: id, botId: person.id }, () => {
                  setBlocked(previous => previous.filter(value => value.id !== person.id)); setNotice(t('tw_channel_bot_removed'));
                }) })}>{t('tw_channel_remove_bot')}</button> : null}
          </li>;
        })}</ul>{blocked.length ? <div role="alert"><p>{t('tw_channel_bot_dependency')}</p><ul>{blocked.map(bot => <li key={bot.id}>{bot.name}</li>)}</ul></div> : null}</div> : null}
        {active.id === 'invites' ? !management && !error ? <p role="status">{t('teams_loading')}</p> : management
          ? <div className="tw-settings-fields">
            <p>{t('tw_channel_invite_scope')}</p>
            {management.code ? <>
              <input aria-label={t('tw_channel_invite_link')} readOnly value={channelInviteLink({ teamId: team.id, conversationId: id, code: management.code })} />
              <div className="tw-channel-actions"><button type="button" onClick={() => {
                const link = channelInviteLink({ teamId: team.id, conversationId: id, code: management.code! });
                if (!navigator.clipboard) {
                  setError(t('tw_channel_invite_copy_manual'));
                  return;
                }
                void navigator.clipboard.writeText(link).then(() => setNotice(t('tw_copied')), e => setError(channelError(e, locale)));
              }}>{t('friends_link_copy')}</button>
              <button type="button" disabled={busy || snapshot.offline} onClick={() => setConfirmation({ title: t('tw_channel_invite_revoke_confirm'),
                action: () => void run({ op: 'revokeInvite', conversationId: id, code: management.code! }, () => setManagement({ ...management, code: null })) })}>{t('tw_channel_invite_revoke')}</button></div>
            </> : <button type="button" className="tw-primary" disabled={busy || snapshot.offline} onClick={() => void run({ op: 'createInvite', conversationId: id }, result => setManagement({ ...management, code: result.code ?? null }))}>{t('teams_invite_create')}</button>}
          </div> : null : null}
        {active.id === 'admins' ? <div className="tw-settings-group">
          <div className="tw-settings-fields tw-role-description">
            <SettingsRole elevated>{t('tw_role_channel_admin')}</SettingsRole>
            <p>{t('tw_channel_roles_hint')}</p>
            <p className="tw-hint">{t('tw_channel_roles_assignment')}</p>
          </div>
          {isTeamManager
            ? management ? <AdministratorList management={management} snapshot={snapshot} locale={locale} renderAvatar={renderAvatar}
              busy={busy || snapshot.offline} change={(uid, enabled) => {
                void run({ op: 'admin', conversationId: id, uid, enabled }, () => setManagement({
                  ...management, admins: enabled ? [...management.admins, uid] : management.admins.filter(previous => previous !== uid),
                }));
              }} /> : !error ? <p role="status">{t('teams_loading')}</p> : null
            : knownAdmins.length ? <ul className="tw-settings-fields tw-channel-roster" aria-label={t('tw_channel_admins')}>
              {knownAdmins.map(member => <li key={member.uid}><ChannelMemberIdentity
                identity={{ kind: 'user', id: member.uid, name: member.displayName }} admin teamRole={member.role} locale={locale} renderAvatar={renderAvatar} /></li>)}
            </ul> : <p className="tw-hint">{t('tw_channel_no_admins')}</p>}
        </div> : null}
        {active.id === 'bots' ? <BotSettings workspace={workspace} snapshot={snapshot} conversation={conversation} scope={scope} locale={locale} /> : null}
    </SettingsLayout>
    {confirmation ? <Dialog title={confirmation.title} onClose={() => setConfirmation(undefined)}>
      <div className="tw-dialog-actions"><button type="button" onClick={() => setConfirmation(undefined)}>{t('common_cancel')}</button>
        <button type="button" className="tw-danger-btn" onClick={() => { confirmation.action(); setConfirmation(undefined); }}>{t('tw_channel_confirm')}</button></div>
    </Dialog> : null}
  </section>;
}

export function TeamChannelAdmins({ workspace, snapshot, locale, renderAvatar }: Context & { renderAvatar?: RenderAvatar }) {
  const t = useTeamsText(locale), { id: teamId, generation } = snapshot.team!;
  const [channels, setChannels] = useState<ChannelResponse['channels']>();
  const [selected, select] = useState(''), [management, setManagement] = useState<Management>();
  const [error, setError] = useState(''), [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    makeRequest(workspace, teamId, generation, { op: 'list' }).then(result => {
      if (live) setChannels(result.channels ?? []);
    }).catch(e => { if (live) setError(channelError(e, locale)); });
    return () => { live = false; };
  }, [workspace, teamId, generation, locale]);
  useEffect(() => {
    if (!selected) return;
    let live = true;
    makeRequest(workspace, teamId, generation, { op: 'get', conversationId: selected }).then(result => {
      if (live) setManagement({ members: result.members ?? [], admins: result.admins ?? [], code: null });
    }).catch(e => { if (live) setError(channelError(e, locale)); });
    return () => { live = false; };
  }, [workspace, teamId, generation, locale, selected]);
  return <div className="tw-form">
    {error ? <p role="alert" className="tw-error">{error}</p> : null}
    {channels ? <div className="tw-settings-fields"><label>{t('teams_conversation_channel')}<select disabled={busy} value={selected} onChange={event => {
      select(event.currentTarget.value); setManagement(undefined); setError('');
    }}>
      <option value="">{t('tw_channel_choose')}</option>{channels.map(channel => <option key={channel.id} value={channel.id}>{channel.title}</option>)}
    </select></label><p className="tw-hint">{t('tw_channel_roles_assignment')}</p></div> : !error ? <p role="status">{t('teams_loading')}</p> : null}
    {selected && !management && !error ? <p role="status">{t('teams_loading')}</p> : null}
    {management ? <AdministratorList management={management} snapshot={snapshot} locale={locale} renderAvatar={renderAvatar} busy={busy || snapshot.offline} change={(uid, enabled) => {
      setBusy(true); setError('');
      void makeRequest(workspace, teamId, generation, { op: 'admin', conversationId: selected, uid, enabled }).then(() =>
        setManagement({ ...management, admins: enabled ? [...management.admins, uid] : management.admins.filter(value => value !== uid) })
      ).catch(e => setError(channelError(e, locale))).finally(() => setBusy(false));
    }} /> : null}
  </div>;
}

export function ChannelJoinDialog({ workspace, snapshot, locale, invite, onClose, onJoined }: Context & {
  invite: ChannelInvite; onClose(): void; onJoined?(): void;
}) {
  const t = useTeamsText(locale);
  const [busy, setBusy] = useState(false), [error, setError] = useState('');
  const join = async () => {
    setBusy(true); setError('');
    try {
      const current = (await workspace.api.list()).teams.find(team => team.id === invite.teamId);
      if (!current) throw new Error('CHANNEL_TEAM_REQUIRED');
      await makeRequest(workspace, current.id, current.generation, { op: 'redeem', conversationId: invite.conversationId, code: invite.code });
      workspace.openConversation(invite.teamId, invite.conversationId);
      onJoined?.(); onClose();
    } catch (e) { setError(channelError(e, locale)); }
    finally { setBusy(false); }
  };
  return <div className="tw-scope"><Dialog title={t('tw_channel_join')} onClose={onClose}>
    {error ? <p role="alert">{error}</p> : <p>{t('tw_channel_join_scope')}</p>}
    <div className="tw-dialog-actions"><button type="button" onClick={onClose}>{t('common_cancel')}</button>
      <button type="button" className="tw-primary" disabled={busy || snapshot.loading} onClick={() => void join()}>{t('tw_channel_join')}</button></div>
  </Dialog></div>;
}
