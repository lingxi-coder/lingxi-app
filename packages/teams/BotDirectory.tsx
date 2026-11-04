import { useState, type ReactElement } from 'react';
import { teamBotUsable, type LocalTeamBot, type TeamBotCard } from './bots.js';
import type { TeamDetail, TeamsApi, TeamSummary } from './index.js';

/**
 * The team's Bots: who may use each, a button to message it, unbinding, and (on a Desktop) sharing a local Bot or
 * changing its audience. Rendered inside the details drawer's Bots tab, so it keeps the markup the `.tw-embed` rules style.
 */
export function BotDirectory({ api, uid, team, members, bots, zh, busy, status, attempt, onOpen, onChanged }: {
  api: TeamsApi; uid: string; team: TeamSummary; members: TeamDetail['members']; bots: TeamBotCard[]; zh: boolean; busy: boolean; status: string;
  attempt(action: () => Promise<unknown>): Promise<void>; onOpen(bot: TeamBotCard): Promise<void>; onChanged(): Promise<void>;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [local, setLocal] = useState<LocalTeamBot[]>([]), [loaded, setLoaded] = useState(false);
  const [botId, setBotId] = useState(''), [use, setUse] = useState<'all' | 'listed'>('listed'), [uids, setUids] = useState<string[]>([]);
  const [removing, setRemoving] = useState<TeamBotCard>();
  const [draftRevision, setDraftRevision] = useState(0);
  const owner = (id: string) => members.find(member => member.uid === id)?.displayName ?? id;
  const existing = bots.find(bot => bot.ownerUid === uid && bot.botId === botId);
  let bindingLabel: string;
  if (existing?.enabled === false) bindingLabel = t('Rebind Bot', '重新绑定 Bot');
  else if (existing) bindingLabel = t('Save permissions', '保存权限');
  else bindingLabel = t('Bind Bot', '绑定 Bot');
  return <section className="team-bots" aria-label={t('Team Bots', '团队 Bot')}><h3>{t('Team Bots', '团队 Bot')}</h3>
    <p className="teams-hint">{t('Bots run on their owner’s Desktop. The owner can read every conversation containing their Bot. Team members cannot approve its tools.', 'Bot 在所有者的 Desktop 上运行。所有者可阅读包含其 Bot 的所有会话。团队成员无法批准其工具操作。')}</p>
    {status ? <p role="status">{status}</p> : null}
    <ul className="team-bot-grid">{bots.map(bot => {
      let presenceLabel: string;
      let audienceLabel: string;
      let messageLabel: string;
      if (bot.enabled === false) {
        presenceLabel = t('Disabled', '已停用');
        audienceLabel = t('Not shared. Rebind on Desktop to enable new requests.', '尚未共享。在 Desktop 重新绑定后可接收新请求。');
        messageLabel = t('Disabled', '已停用');
      } else {
        presenceLabel = bot.online ? t('Online', '在线') : t('Offline', '离线');
        audienceLabel = bot.use === 'all' ? t('All team members may use', '全体团队成员可用') : t('Only owner and listed members may use', '仅所有者与指定成员可用');
        messageLabel = bot.permitted ? t('Message Bot', '与 Bot 对话') : t('Not permitted', '无使用权限');
      }
      return <li key={bot.key} className="teams-card">
        <header><b>{bot.name}</b><span className="team-bot-presence" data-online={bot.enabled !== false && bot.online}>{presenceLabel}</span></header>
        <p>{bot.description}</p><small>{t('Owner', '所有者')}: {owner(bot.ownerUid)}</small>
        <p className="teams-hint">{audienceLabel}{bot.use === 'listed' && bot.uids.length ? ` · ${bot.uids.map(owner).join(', ')}` : ''}</p>
        <div className="teams-row-actions">
          <button disabled={busy || !teamBotUsable(bot)} onClick={() => void attempt(() => onOpen(bot))}>{messageLabel}</button>
          {bot.ownerUid === uid && bot.enabled !== false ? <button disabled={busy} className="teams-danger" onClick={() => setRemoving(bot)}>{t('Unbind', '解除绑定')}</button> : null}
        </div>
      </li>;
    })}</ul>
    {!bots.length && !status ? <p>{t('No Bots are shared with this team yet.', '此团队尚未共享 Bot。')}</p> : null}
    {api.localBots && api.bindLocalBot ? <details><summary>{t('Share a Desktop Bot / change permissions', '共享 Desktop Bot / 修改权限')}</summary><button disabled={busy} onClick={() => void attempt(async () => { const result = await api.localBots!(); setLocal(result); setLoaded(true); })}>{t('Load local Bots', '加载本机 Bot')}</button>{loaded ? <form onSubmit={event => { event.preventDefault(); void attempt(async () => { await api.bindLocalBot!({ teamId: team.id, generation: team.generation, botId, use, uids: use === 'listed' ? uids : [], expectedRevision: draftRevision }); setBotId(''); setDraftRevision(0); setUids([]); await onChanged(); }); }}><label>{t('Local Bot', '本机 Bot')}<select required value={botId} onChange={event => { const id = event.target.value, card = bots.find(bot => bot.ownerUid === uid && bot.botId === id); setBotId(id); setDraftRevision(card?.revision ?? 0); setUse(card?.use ?? 'listed'); setUids([...(card?.uids ?? [])]); }}><option value="">{t('Choose a Bot', '选择 Bot')}</option>{local.map(bot => <option key={bot.botId} value={bot.botId}>{bot.name}{bot.model ? ` · ${bot.model}` : ''}</option>)}</select></label>{!local.length ? <p>{t('Create a Bot on this Desktop first.', '请先在此 Desktop 创建 Bot。')}</p> : null}{botId && draftRevision !== (existing?.revision ?? 0) ? <div role="status"><p>{t('Permissions changed in another window. Reload the current audience before editing again.', '其他窗口已更改权限，请重新加载当前使用范围后再编辑。')}</p><button type="button" disabled={busy} onClick={() => { setDraftRevision(existing?.revision ?? 0); setUse(existing?.use ?? 'listed'); setUids([...(existing?.uids ?? [])]); }}>{t('Reload current permissions', '重新加载当前权限')}</button></div> : null}<label>{t('Who may use this Bot?', '谁可使用此 Bot？')}<select value={use} onChange={event => setUse(event.target.value as typeof use)}><option value="listed">{t('Owner and selected members', '所有者与指定成员')}</option><option value="all">{t('All team members', '全体团队成员')}</option></select></label>{use === 'listed' ? <fieldset><legend>{t('Allowed members', '允许的成员')}</legend>{members.filter(member => member.uid !== uid).map(member => <label key={member.uid} className="team-party"><input type="checkbox" checked={uids.includes(member.uid)} onChange={event => setUids(event.target.checked ? [...uids, member.uid] : uids.filter(id => id !== member.uid))} />{member.displayName}</label>)}</fieldset> : null}<button disabled={busy || !botId}>{bindingLabel}</button></form> : null}</details> : <p className="teams-hint">{t('Open Teams in LingXi Desktop to share a local Bot or change its permissions.', '在灵犀 Desktop 的团队页面共享本机 Bot 或修改使用权限。')}</p>}
    {removing ? <div className="teams-modal"><section role="alertdialog" aria-modal="true" aria-label={t('Unbind Bot?', '解除 Bot 绑定？')}><h2>{t(`Unbind ${removing.name}?`, `解除 ${removing.name} 的绑定？`)}</h2><p>{t('New Bot requests will stop. Existing conversation history remains.', '将停止新的 Bot 请求，现有会话历史保留。')}</p><div><button autoFocus disabled={busy} onClick={() => setRemoving(undefined)}>{t('Cancel', '取消')}</button><button disabled={busy} className="teams-danger" onClick={() => void attempt(async () => { await api.messages({ action: 'unbindBot', teamId: team.id, generation: team.generation, botKey: removing.key, expectedRevision: removing.revision }); setRemoving(undefined); await onChanged(); })}>{t('Unbind', '解除绑定')}</button></div></section></div> : null}
  </section>;
}
