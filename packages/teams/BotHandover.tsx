import { useState } from 'react';
import { executionCommand, type LocalPublishableBot, type SharedBotCard } from './execution.js';
import type { TeamDetail, TeamsApi, TeamSummary } from './index.js';
import { Dialog } from './ui';
import { intlTag, useTeamsText } from './i18n.js';

/**
 * Handing a published Bot to another member, on its card. The maintainer offers it from the Desktop that publishes it, because
 * the role and files go from there. The member it is offered to takes it over on their own Desktop with one of their Bots, or declines
 * on any device. Until they have taken it over, nothing changes for the people who use the Bot. Only the two of them see any of this.
 */
export function BotHandover({ api, uid, team, members, bot, locale, busy, attempt, onChanged }: {
  api: TeamsApi; uid: string; team: TeamSummary; members: TeamDetail['members']; bot: SharedBotCard; locale: string; busy: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>; onChanged(): Promise<void>;
}) {
  const t = useTeamsText(locale);
  const [offering, setOffering] = useState(false), [to, setTo] = useState(''), [agreed, setAgreed] = useState(false);
  /** One id for one offer to one person: asking again after a lost answer is the same offer. */
  const [transferId, setTransferId] = useState(() => crypto.randomUUID());
  const [taking, setTaking] = useState(false), [local, setLocal] = useState<LocalPublishableBot[]>(), [instanceId, setInstanceId] = useState('');
  const name = (id: string) => members.find(member => member.uid === id)?.displayName ?? id;
  const scope = { teamId: team.id, generation: team.generation }, transfer = bot.transfer;
  const end = () => attempt(async () => { await api.messages(executionCommand(scope, 'endTransfer', { transferId: transfer!.transferId })); setTaking(false); await onChanged(); });
  const until = transfer ? new Date(transfer.expiresAt).toLocaleDateString(intlTag(locale), { month: 'short', day: 'numeric' }) : '';

  if (transfer && transfer.to.uid === uid) {
    const from = name(transfer.from.uid), host = local?.find(value => value.instanceId === instanceId);
    return <div className="team-bot-handover" role="group" aria-label={t('tw_handover')}>
      <p>{t('tw_handover_offer_fmt', from, from, until)}</p>
      {api.acceptSharedBot && api.localBots ? null : <p className="teams-hint">{t('tw_handover_take_hint')}</p>}
      <div className="teams-row-actions">
        {api.acceptSharedBot && api.localBots ? <button type="button" disabled={busy} onClick={() => void attempt(async () => { setLocal(await api.localBots!()); setAgreed(false); setTaking(true); })}>{t('tw_handover_take')}</button> : null}
        <button type="button" disabled={busy} onClick={() => void end()}>{t('friends_request_decline')}</button>
      </div>
      {taking && local ? <Dialog title={t('tw_handover_take_fmt', bot.name)} onClose={() => setTaking(false)}><div className="tw-form">
        <p>{t('tw_handover_receive_fmt', bot.name, from)}</p>
        <label>{t('tw_handover_answer_with')}<select value={instanceId} onChange={event => setInstanceId(event.target.value)}><option value="">{t('tw_handover_choose_bot')}</option>{local.map(value => <option key={value.instanceId} value={value.instanceId}>{value.name}{value.model ? ` · ${value.model}` : ''}</option>)}</select></label>
        {local.length ? <p className="teams-hint">{t('tw_handover_model_hint')}</p> : <p role="alert">{t('tw_handover_no_bot')}</p>}
        <ul>
          <li>{t('tw_handover_mac_answers')}</li>
          <li>{t('tw_handover_new_chats')}</li>
          <li>{t('tw_handover_unchanged')}</li>
        </ul>
        <label className="team-party"><input type="checkbox" checked={agreed} onChange={event => setAgreed(event.target.checked)} />{t('tw_handover_consent')}</label>
        <p className="teams-hint">{t('tw_handover_check_hint')}</p>
        <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setTaking(false)}>{t('common_cancel')}</button>
          <button type="button" disabled={busy || !host || !agreed} onClick={() => void attempt(async () => { await api.acceptSharedBot!({ ...scope, transferId: transfer.transferId, instanceId }); setTaking(false); await onChanged(); })}>{t('tw_handover_take_button')}</button></div>
      </div></Dialog> : null}
    </div>;
  }
  if (transfer && bot.maintainer.uid === uid) return <div className="team-bot-handover" role="group" aria-label={t('tw_handover')}>
    <p>{transfer.state === 'accepted' ? t('tw_handover_taking_fmt', name(transfer.to.uid))
      : t('tw_handover_offered_fmt', name(transfer.to.uid), until)}</p>
    <div className="teams-row-actions"><button type="button" disabled={busy} onClick={() => void end()}>{t('tw_handover_take_back')}</button></div>
  </div>;

  // Someone who cannot use the Bot cannot maintain it: a handover never widens who sees its files.
  const policy = bot.usePolicy;
  const candidates = members.filter(member => member.uid !== uid && (policy.mode === 'all' || policy.members.some(value => value.uid === member.uid && value.generation === member.generation)));
  if (bot.maintainer.uid !== uid || bot.state !== 'published' || !api.offerSharedBot || !candidates.length) return null;
  const chosen = candidates.find(member => member.uid === to);
  return <>
    <div className="teams-row-actions"><button type="button" disabled={busy} onClick={() => { setAgreed(false); setOffering(true); }}>{t('tw_handover_give')}</button></div>
    {offering ? <Dialog title={t('tw_handover_give_fmt', bot.name)} onClose={() => setOffering(false)}><div className="tw-form">
      <label>{t('tw_handover_new_maintainer')}<select value={to} onChange={event => { setTo(event.target.value); setTransferId(crypto.randomUUID()); }}><option value="">{t('tw_handover_choose_member')}</option>{candidates.map(member => <option key={member.uid} value={member.uid}>{member.displayName}</option>)}</select></label>
      <ul>
        <li>{t('tw_handover_they_receive')}</li>
        <li>{t('tw_handover_stays_with_you')}</li>
        <li>{t('tw_handover_old_chats')}</li>
        <li>{t('tw_handover_after')}</li>
      </ul>
      <label className="team-party"><input type="checkbox" checked={agreed} onChange={event => setAgreed(event.target.checked)} />{t('tw_handover_give_consent')}</label>
      <p className="teams-hint">{t('tw_handover_seven_days')}</p>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setOffering(false)}>{t('common_cancel')}</button>
        <button type="button" disabled={busy || !chosen || !agreed} onClick={() => void attempt(async () => {
          await api.offerSharedBot!({ ...scope, transferId, sharedBotId: bot.sharedBotId, expectedRevision: bot.revision, to: { uid: chosen!.uid, generation: chosen!.generation } });
          setOffering(false); setTo(''); setTransferId(crypto.randomUUID()); await onChanged();
        })}>{t('tw_handover_offer')}</button></div>
    </div></Dialog> : null}
  </>;
}
