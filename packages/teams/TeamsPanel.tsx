import { useCallback, useEffect, useRef, useState } from 'react';
import { inviteCode, inviteLink, type TeamCommand, type TeamDetail, type TeamSummary, type TeamsApi } from './index';
import './teams.css';

export function TeamsPanel({ api, uid, zh, initialInvite, onInviteHandled }: {
  api: TeamsApi; uid: string; zh: boolean; initialInvite?: string; onInviteHandled?: () => void;
}) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [teams, setTeams] = useState<TeamSummary[]>([]);
  const [selected, setSelected] = useState('');
  const [detail, setDetail] = useState<TeamDetail | null>(null);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [name, setName] = useState('');
  const [code, setCode] = useState(initialInvite ?? '');
  const [link, setLink] = useState('');
  const [notice, setNotice] = useState('');
  const [uses, setUses] = useState(1);
  const [ttl, setTtl] = useState(86400);
  const [inviteRole, setInviteRole] = useState<'member' | 'admin'>('member');
  const [confirmation, setConfirmation] = useState<{ command: TeamCommand; title: string } | null>(null);
  const epoch = useRef(0), authority = useRef(0), selection = useRef('');
  const translateError = (error: unknown) => {
    const code = error instanceof Error ? error.message : '';
    const labels: Record<string, [string, string]> = {
      INVITE_UNAVAILABLE: ['This invite has expired, was revoked or has no uses left.', '邀请已过期、撤销或用尽。'],
      MEMBERSHIP_REQUIRED: ['Your membership has ended. Refresh or join with a new invite.', '你已不在此团队，请刷新或通过新邀请加入。'],
      STALE_MEMBERSHIP: ['Membership changed. Refresh before trying again.', '成员身份已变化，请刷新后重试。'],
      ROLE_REQUIRED: ['Your role cannot perform this action.', '当前角色无权执行此操作。'],
      TEAM_LIMIT: ['You can join up to 20 teams.', '最多可加入 20 个团队。'],
      MEMBER_LIMIT: ['This team has reached 200 members.', '团队已达到 200 人上限。'],
    };
    return labels[code]?.[zh ? 1 : 0] ?? t('Could not connect to Teams. Check your connection and try again.', '无法连接团队服务，请检查网络后重试。');
  };
  const refresh = useCallback(async () => {
    const request = ++epoch.current, teamId = selection.current;
    try {
      const [list, current] = await Promise.allSettled([api.list(), teamId ? api.detail(teamId) : Promise.resolve(null)]);
      if (request !== epoch.current) return;
      if (list.status === 'rejected') throw list.reason;
      setTeams(list.value.teams); setLoading(false);
      if (current.status === 'rejected') {
        selection.current = ''; setSelected(''); setDetail(null); setLink('');
        setError(translateError(current.reason));
      } else { setDetail(current.value); setError(''); }
    } catch (error) {
      if (request !== epoch.current) return;
      setDetail(null); setTeams([]); setLoading(false);
      setError(translateError(error));
    }
  // Translation is evaluated for the language captured by this callback.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, zh]);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    void refresh();
    const stop = api.onChanged(event => {
      if (event.invalidated) { epoch.current++; authority.current++; setDetail(null); setTeams([]); setLink(''); setConfirmation(null); }
      clearTimeout(timer); timer = setTimeout(() => void refresh(), 80);
    });
    return () => { epoch.current++; authority.current++; clearTimeout(timer); stop(); };
  }, [api, refresh]);
  useEffect(() => { if (initialInvite) { setCode(initialInvite); onInviteHandled?.(); } }, [initialInvite, onInviteHandled]);
  const select = (id: string) => {
    selection.current = id; setSelected(id); setDetail(null); setLink(''); setInviteRole('member'); setNotice(''); setLoading(true); void refresh();
  };
  const execute = async (command: TeamCommand) => {
    if (busy) return;
    const authorized = authority.current;
    setBusy(true); setError(''); setNotice('');
    try {
      const result = await api.execute(command);
      if (command.action === 'delete' || command.action === 'leave') { selection.current = ''; setSelected(''); setDetail(null); }
      if (authorized !== authority.current) { await refresh(); return; }
      if (result.code) setLink(inviteLink(result.code));
      if (command.action === 'join') { setCode(''); onInviteHandled?.(); }
      if (command.action === 'create') setName('');
      if (result.teamId && ['create', 'join'].includes(command.action)) {
        selection.current = result.teamId; setSelected(result.teamId);
      }
      await refresh();
    } catch (error) { setError(translateError(error)); }
    finally { setBusy(false); setConfirmation(null); }
  };
  const role = (value: string) => ({ owner: t('Owner', '所有者'), admin: t('Admin', '管理员'), member: t('Member', '成员') })[value];
  const team = detail?.team;
  return <section className="teams-panel" aria-labelledby="teams-title">
    <header className="teams-header"><div><span className="teams-eyebrow">LINGXI</span><h1 id="teams-title">{t('Teams', '团队')}</h1></div><button type="button" onClick={() => void refresh()} disabled={busy}>{t('Refresh', '刷新')}</button></header>
    <p className="teams-disclosure">{t('Team data and messages are readable by the platform. A Bot’s owner can read conversations with their Bot.', '团队资料与消息平台可读。与 Bot 的对话对该 Bot 的所有者可见。')}</p>
    {error ? <p role="alert" className="teams-error">{error}</p> : null}
    {notice ? <p role="status">{notice}</p> : null}
    <div className="teams-layout"><aside className="teams-list" aria-label={t('My teams', '我的团队')}>
      {teams.map(team => <button key={team.id} className="teams-card" type="button" aria-current={selected === team.id ? 'page' : undefined} onClick={() => select(team.id)}><b>{team.name}</b><span>{role(team.role)} · {team.memberCount} {t('members', '人')}</span></button>)}
      {!loading && teams.length === 0 ? <p>{t('Create a team or join with an invite.', '创建团队，或通过邀请加入。')}</p> : null}
      <form onSubmit={event => { event.preventDefault(); void execute({ action: 'create', teamId: crypto.randomUUID(), name: name.trim() }); }}>
        <label>{t('New team', '新建团队')}<input maxLength={80} value={name} onChange={event => setName(event.target.value)} required placeholder={t('Team name', '团队名称')} /></label>
        <button disabled={busy || !name.trim()}>{t('Create team', '创建团队')}</button>
      </form>
      <form onSubmit={event => { event.preventDefault(); const parsed = inviteCode(code); if (parsed) void execute({ action: 'join', code: parsed, requestId: crypto.randomUUID() }); }}>
        <label>{t('Join a team', '加入团队')}<input value={code} onChange={event => setCode(event.target.value)} maxLength={200} autoComplete="off" placeholder={t('Invite link or code', '邀请链接或邀请码')} /></label>
        <p className="teams-hint">{t('Join only teams you trust. Your name will be visible to their members.', '仅加入你信任的团队。加入后团队成员可见你的名称。')}</p>
        <button disabled={busy || !inviteCode(code)}>{t('Confirm and join', '确认加入')}</button>
      </form>
    </aside><div className="teams-detail" aria-busy={loading || busy}>
      {loading ? <p role="status">{t('Loading…', '正在加载…')}</p> : null}
      {team ? <><header className="teams-team-header"><div><h2>{team.name}</h2><span>{role(team.role)} · {team.memberCount} {t('members', '人')}</span></div>
        <button className="teams-danger" type="button" disabled={busy} onClick={() => setConfirmation({ title: team.role === 'owner' ? t(`Delete ${team.name} for everyone?`, `删除团队「${team.name}」？所有成员将失去访问权限。`) : t(`Leave ${team.name}?`, `退出团队「${team.name}」？`), command: team.role === 'owner' ? { action: 'delete', teamId: team.id } : { action: 'leave', teamId: team.id, generation: team.generation } })}>{team.role === 'owner' ? t('Delete team', '删除团队') : t('Leave team', '退出团队')}</button></header>
        <h3>{t('Members', '成员')}</h3><ul className="teams-members">{detail.members.map(member => <li key={member.uid}><div><b>{member.displayName}</b><small>{member.uid === uid ? t('You', '你') : ''} {role(member.role)}</small></div>
          {member.uid !== uid && member.role !== 'owner' && (team.role === 'owner' || (team.role === 'admin' && member.role === 'member')) ? <div className="teams-row-actions">
            {team.role === 'owner' ? <button disabled={busy} onClick={() => setConfirmation({ title: t(`Change ${member.displayName}’s role?`, `更改 ${member.displayName} 的角色？`), command: { action: 'role', teamId: team.id, uid: member.uid, generation: member.generation, role: member.role === 'admin' ? 'member' : 'admin' } })}>{member.role === 'admin' ? t('Make member', '设为成员') : t('Make admin', '设为管理员')}</button> : null}
            <button className="teams-danger" disabled={busy} onClick={() => setConfirmation({ title: t(`Remove ${member.displayName}?`, `移除 ${member.displayName}？`), command: { action: 'remove', teamId: team.id, uid: member.uid, generation: member.generation } })}>{t('Remove', '移除')}</button>
          </div> : null}</li>)}</ul>
        {team.role !== 'member' ? <section className="teams-invites"><h3>{t('Invites', '邀请')}</h3><form onSubmit={event => { event.preventDefault(); void execute({ action: 'invite', teamId: team.id, requestId: crypto.randomUUID(), role: team.role === 'owner' ? inviteRole : 'member', maxUses: uses, ttlSeconds: ttl }); }}>
          <div className="teams-invite-options"><label>{t('Role', '加入角色')}<select value={inviteRole} onChange={event => setInviteRole(event.target.value as 'member' | 'admin')}><option value="member">{t('Member', '成员')}</option>{team.role === 'owner' ? <option value="admin">{t('Admin', '管理员')}</option> : null}</select></label>
          <label>{t('Uses', '可用次数')}<input type="number" min={1} max={50} value={uses} onChange={event => setUses(Number(event.target.value))} required /></label>
          <label>{t('Expires in', '有效期')}<select value={ttl} onChange={event => setTtl(Number(event.target.value))}><option value={3600}>{t('1 hour', '1 小时')}</option><option value={86400}>{t('1 day', '1 天')}</option><option value={604800}>{t('7 days', '7 天')}</option></select></label></div>
          <button disabled={busy}>{t('Create invite link', '创建邀请链接')}</button>
        </form>{link ? <div className="teams-share"><label>{t('Share this invitation', '分享邀请')}<input readOnly value={link} onFocus={event => event.target.select()} /></label><button onClick={() => void navigator.clipboard.writeText(link).then(() => setNotice(t('Link copied', '链接已复制')), () => setNotice(t('Select and copy the link above.', '请选中上方链接并复制。')))}>{t('Copy', '复制')}</button></div> : null}
        <ul className="teams-invite-list">{detail.invites.map(invite => <li key={invite.id}><span>{role(invite.role)} · {invite.uses}/{invite.maxUses} · {new Date(invite.expiresAt).toLocaleString(zh ? 'zh-CN' : 'en-US')}</span>{invite.revoked ? <span>{t('Revoked', '已撤销')}</span> : <button disabled={busy || (team.role !== 'owner' && invite.role === 'admin')} onClick={() => void execute({ action: 'revokeInvite', teamId: team.id, inviteId: invite.id })}>{t('Revoke', '撤销')}</button>}</li>)}</ul></section> : null}
      </> : !loading ? <div className="teams-empty"><h2>{t('A shared place for your team', '与团队保持连接')}</h2><p>{t('Choose a team to see its members and manage invitations.', '选择团队查看成员并管理邀请。')}</p></div> : null}
    </div></div>
    {confirmation ? <div className="teams-modal"><section role="alertdialog" aria-modal="true" aria-labelledby="teams-confirm-title"><h2 id="teams-confirm-title">{confirmation.title}</h2><div><button autoFocus disabled={busy} onClick={() => setConfirmation(null)}>{t('Cancel', '取消')}</button><button className="teams-danger" disabled={busy} onClick={() => void execute(confirmation.command)}>{t('Confirm', '确认')}</button></div></section></div> : null}
  </section>;
}
