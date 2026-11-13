import { useState } from 'react';
import { executionCommand, type LocalPublishableBot, type SharedBotCard } from './execution.js';
import type { TeamDetail, TeamsApi, TeamSummary } from './index.js';
import { Dialog } from './ui';

/**
 * Handing a published assistant to another member, on its card. The maintainer offers it from the Desktop that publishes it, because
 * the role and files go from there. The member it is offered to takes it over on their own Desktop with one of their Bots, or declines
 * on any device. Until they have taken it over, nothing changes for the people who use the assistant. Only the two of them see any of this.
 */
export function BotHandover({ api, uid, team, members, bot, zh, busy, attempt, onChanged }: {
  api: TeamsApi; uid: string; team: TeamSummary; members: TeamDetail['members']; bot: SharedBotCard; zh: boolean; busy: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>; onChanged(): Promise<void>;
}) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [offering, setOffering] = useState(false), [to, setTo] = useState(''), [agreed, setAgreed] = useState(false);
  /** One id for one offer to one person: asking again after a lost answer is the same offer. */
  const [transferId, setTransferId] = useState(() => crypto.randomUUID());
  const [taking, setTaking] = useState(false), [local, setLocal] = useState<LocalPublishableBot[]>(), [instanceId, setInstanceId] = useState('');
  const name = (id: string) => members.find(member => member.uid === id)?.displayName ?? id;
  const scope = { teamId: team.id, generation: team.generation }, transfer = bot.transfer;
  const end = () => attempt(async () => { await api.messages(executionCommand(scope, 'endTransfer', { transferId: transfer!.transferId })); setTaking(false); await onChanged(); });
  const until = transfer ? new Date(transfer.expiresAt).toLocaleDateString(zh ? 'zh-CN' : 'en', { month: 'short', day: 'numeric' }) : '';

  if (transfer && transfer.to.uid === uid) {
    const from = name(transfer.from.uid), host = local?.find(value => value.instanceId === instanceId);
    return <div className="team-bot-handover" role="group" aria-label={t('Handover', '移交')}>
      <p>{t(`${from} wants to hand this assistant over to you. It stays with ${from} until you take it over. The offer ends on ${until}.`, `${from} 想把这个助手移交给你。在你接手之前，它仍由 ${from} 维护。此邀请于 ${until} 失效。`)}</p>
      {api.acceptSharedBot && api.localBots ? null : <p className="teams-hint">{t('To take it over, open LingXi Desktop on the Mac that will answer for it.', '要接手，请在将为它作答的 Mac 上打开灵犀 Desktop。')}</p>}
      <div className="teams-row-actions">
        {api.acceptSharedBot && api.localBots ? <button type="button" disabled={busy} onClick={() => void attempt(async () => { setLocal(await api.localBots!()); setAgreed(false); setTaking(true); })}>{t('Take over…', '接手…')}</button> : null}
        <button type="button" disabled={busy} onClick={() => void end()}>{t('Decline', '谢绝')}</button>
      </div>
      {taking && local ? <Dialog title={t(`Take over ${bot.name}`, `接手 ${bot.name}`)} onClose={() => setTaking(false)}><div className="tw-form">
        <p>{t(`You receive the role and the files ${bot.name} is published with. Nothing else of ${from}’s comes with it.`, `你会收到 ${bot.name} 发布时所用的角色和资料。${from} 的其他内容都不会随之移交。`)}</p>
        <label>{t('Answer with', '由此 Bot 作答')}<select value={instanceId} onChange={event => setInstanceId(event.target.value)}><option value="">{t('Choose one of your Bots', '选择你的一个 Bot')}</option>{local.map(value => <option key={value.instanceId} value={value.instanceId}>{value.name}{value.model ? ` · ${value.model}` : ''}</option>)}</select></label>
        {local.length ? <p className="teams-hint">{t('Its model answers. The published role is used in place of its own.', '由它的模型作答，并以已发布的角色代替它自己的角色。')}</p> : <p role="alert">{t('Create a Bot on this Mac first.', '请先在这台 Mac 上创建一个 Bot。')}</p>}
        <ul>
          <li>{t('This Mac answers for the assistant from now on, with your model account and at your cost.', '此后由这台 Mac 为助手作答，使用你的模型账号，费用由你承担。')}</li>
          <li>{t('You can read every new conversation with it. Earlier conversations stay with the people who had them, and you cannot read them.', '你可以阅读此后与它的所有新对话。此前的对话仍只属于当时的参与者，你无法阅读。')}</li>
          <li>{t('Who may use it, its daily limits and its shared list stay as they are.', '可使用它的成员、每日额度和共享清单保持不变。')}</li>
        </ul>
        <label className="team-party"><input type="checkbox" checked={agreed} onChange={event => setAgreed(event.target.checked)} />{t('I take over maintaining this assistant and its model costs.', '我接手维护此助手，并承担其模型费用。')}</label>
        <p className="teams-hint">{t('LingXi first checks that the model answers. This can take a moment.', '灵犀会先确认模型可以作答，这可能需要一点时间。')}</p>
        <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setTaking(false)}>{t('Cancel', '取消')}</button>
          <button type="button" disabled={busy || !host || !agreed} onClick={() => void attempt(async () => { await api.acceptSharedBot!({ ...scope, transferId: transfer.transferId, instanceId }); setTaking(false); await onChanged(); })}>{t('Take over', '接手')}</button></div>
      </div></Dialog> : null}
    </div>;
  }
  if (transfer && bot.maintainer.uid === uid) return <div className="team-bot-handover" role="group" aria-label={t('Handover', '移交')}>
    <p>{transfer.state === 'accepted' ? t(`${name(transfer.to.uid)} is taking this assistant over. You maintain it until their Mac answers for it.`, `${name(transfer.to.uid)} 正在接手此助手。在对方的 Mac 开始作答之前，仍由你维护。`)
      : t(`Offered to ${name(transfer.to.uid)}. You maintain it until they take it over. The offer ends on ${until}.`, `已邀请 ${name(transfer.to.uid)} 接手。在对方接手之前，仍由你维护。此邀请于 ${until} 失效。`)}</p>
    <div className="teams-row-actions"><button type="button" disabled={busy} onClick={() => void end()}>{t('Take back', '收回')}</button></div>
  </div>;

  // Someone who cannot use the assistant cannot maintain it: a handover never widens who sees its files.
  const policy = bot.usePolicy;
  const candidates = members.filter(member => member.uid !== uid && (policy.mode === 'all' || policy.members.some(value => value.uid === member.uid && value.generation === member.generation)));
  if (bot.maintainer.uid !== uid || bot.state !== 'published' || !api.offerSharedBot || !candidates.length) return null;
  const chosen = candidates.find(member => member.uid === to);
  return <>
    <div className="teams-row-actions"><button type="button" disabled={busy} onClick={() => { setAgreed(false); setOffering(true); }}>{t('Hand over…', '移交…')}</button></div>
    {offering ? <Dialog title={t(`Hand over ${bot.name}`, `移交 ${bot.name}`)} onClose={() => setOffering(false)}><div className="tw-form">
      <label>{t('New maintainer', '新的维护者')}<select value={to} onChange={event => { setTo(event.target.value); setTransferId(crypto.randomUUID()); }}><option value="">{t('Choose a member', '选择成员')}</option>{candidates.map(member => <option key={member.uid} value={member.uid}>{member.displayName}</option>)}</select></label>
      <ul>
        <li>{t('They receive the published role and the files it is shared with.', '对方会收到已发布的角色和随之共享的资料。')}</li>
        <li>{t('Your local Bot, your model account and everything else on this Mac stay with you.', '你的本机 Bot、模型账号以及这台 Mac 上的其他内容都留在你这里。')}</li>
        <li>{t('Conversations people already had with it stay with them and with you. The new maintainer cannot read them.', '成员此前与它的对话仍只属于他们和你，新的维护者无法阅读。')}</li>
        <li>{t('Once they take it over, their Mac answers and they pay for the model. You can no longer pause or publish it, and people start a new chat with it.', '对方接手后，由对方的 Mac 作答并承担模型费用。你将不能再暂停或发布它，成员需与它开始新的对话。')}</li>
      </ul>
      <label className="team-party"><input type="checkbox" checked={agreed} onChange={event => setAgreed(event.target.checked)} />{t('I may pass this role and these files on to the new maintainer.', '我有权把此角色和这些资料交给新的维护者。')}</label>
      <p className="teams-hint">{t('They have seven days to take it over. Until then you can take the offer back.', '对方有七天时间接手。在此之前你可以收回邀请。')}</p>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setOffering(false)}>{t('Cancel', '取消')}</button>
        <button type="button" disabled={busy || !chosen || !agreed} onClick={() => void attempt(async () => {
          await api.offerSharedBot!({ ...scope, transferId, sharedBotId: bot.sharedBotId, expectedRevision: bot.revision, to: { uid: chosen!.uid, generation: chosen!.generation } });
          setOffering(false); setTo(''); setTransferId(crypto.randomUUID()); await onChanged();
        })}>{t('Offer', '发出邀请')}</button></div>
    </div></Dialog> : null}
  </>;
}
