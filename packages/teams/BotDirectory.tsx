import { useEffect, useId, useRef, useState } from 'react';
import { executionCommand, sharedBotUsable, type LocalPublishableBot, type RequestQuotas, type SelectedSharedFile, type SharedBotCard, type UsePolicy, type PublishLocalBotInput, type PublishedKnowledge, type SharedList } from './execution.js';
import type { TeamDetail, TeamsApi, TeamSummary } from './index.js';
import { emptyBotScope, type PublishedBotScope } from './bot-scope.js';
import { Dialog } from './ui';
import { BotScopeSummary } from './BotScopeSummary';
import { SharedListDialog } from './SharedListDialog';
import { TriageLogDialog } from './TriageLogDialog';
import { HeldRepliesButton } from './HeldRepliesDialog';
import { maintainsBot } from './triage-log.js';
import { SharedHostPanel } from './SharedHostPanel';
import { BotHandover } from './BotHandover';
import { BotMemberDetails, botAvailabilityText } from './BotMemberDetails';
import { useTeamsText } from './i18n.js';

/**
 * Shared member directory and native publication wizard. Files are picked by the trusted host. With `friend` (the friend's name) it is
 * the Bots of a friendship instead (D21, D23): no audience or quota lines on the cards and an audience that is fixed to the friend in the
 * wizard (the quotas stay editable). Ending a Bot's availability is Stop sharing (U1, U8) in both, with the same dialog.
 */
export function BotDirectory({ api, uid, team, members, bots, locale, busy, status, attempt, onOpen, onChanged, friend, onExample, onOpenChat }: {
  api: TeamsApi; uid: string; team: TeamSummary; members: TeamDetail['members']; bots: SharedBotCard[]; locale: string; busy: boolean; status: string;
  attempt(action: () => Promise<unknown>): Promise<void>; onOpen(bot: SharedBotCard): Promise<void>; onChanged(): Promise<void>; friend?: string; onExample?(bot: SharedBotCard, example: string): void;
  /** Take the person to a Chat, from the replies of their Bot that wait there. Without it the list of those replies does not offer the Chat. */
  onOpenChat?(conversationId: string): void;
}) {
  const t = useTeamsText(locale);
  const note = t('friends_bots_note');
  const exampleId = useId();
  const request = useRef(0), live = useRef(true);
  useEffect(() => { live.current = true; return () => { live.current = false; request.current++; }; }, [uid, team.id, team.generation]);
  const [examples, setExamples] = useState<string[]>(['', '', '']);
  const exampleInput = examples.map(value => value.trim()).filter(Boolean).slice(0, 3);
  const examplesInvalid = exampleInput.some(value => new TextEncoder().encode(value).length > 2000 || value.includes('\0'));
  const [local, setLocal] = useState<LocalPublishableBot[]>(), [instanceId, setInstanceId] = useState('');
  const [role, setRole] = useState(''), [files, setFiles] = useState<SelectedSharedFile[]>([]);
  const [resourceBindings, setResourceBindings] = useState<Record<string, string>>({});
  /** What the chosen Bot is published with from this Desktop, the files that will stop being shared, and the newly picked files that replace current ones. */
  const [current, setCurrent] = useState<PublishedKnowledge>(), [dropped, setDropped] = useState<string[]>([]), [swaps, setSwaps] = useState<Record<string, SelectedSharedFile>>({});
  const [mode, setMode] = useState<'all' | 'listed'>('listed'), [uids, setUids] = useState<string[]>([]);
  const [quotas, setQuotas] = useState<RequestQuotas>({ team: 100, member: 20, bot: 100 });
  const [review, setReview] = useState<PublishLocalBotInput>(), [confirmed, setConfirmed] = useState(false), [pausing, setPausing] = useState<SharedBotCard>();
  const [draft, setDraft] = useState<{ sharedBotId: string; revision: number }>();
  /** The Bot whose auto-reply log is open, and the Bots the service said this person does not maintain after all: their entry is not offered again. */
  const [triageOf, setTriageOf] = useState<SharedBotCard>(), [denied, setDenied] = useState<string[]>([]);
  /** The wizard's shared list, and the Bot whose list is open. */
  const [listOn, setListOn] = useState(false), [listTitle, setListTitle] = useState(''), [listAbout, setListAbout] = useState(''), [listOf, setListOf] = useState<SharedBotCard>(), [listAsk, setListAsk] = useState(false);
  /** What of this Desktop the Bot is published with: the Local Apps ticked, and the datasets ticked with what the Bot may do in them. Nothing is ticked until the maintainer does. */
  const [scopeApps, setScopeApps] = useState<string[]>([]), [scopeData, setScopeData] = useState<Record<string, { write: boolean; reportReview: boolean }>>({});
  const selected = local?.find(bot => bot.instanceId === instanceId);
  const supportsList = selected?.capabilities?.writeOperations === true;
  useEffect(() => { if (!supportsList) { setListOn(false); setListTitle(''); setListAbout(''); setListAsk(false); setReview(undefined); setConfirmed(false); } }, [instanceId, supportsList]);
  const listInput = listOn && supportsList ? { sharedList: { title: listTitle.trim(), description: listAbout.trim(), ...(listAsk ? { approval: 'requester' as const } : {}) } } : {}, listMissing = listOn && (!supportsList || !listTitle.trim() || !listAbout.trim());
  const existing = bots.find(bot => bot.maintainer.uid === uid && bot.instanceId === instanceId);
  // The most the local Bot's own scope allows, and what is ticked of it. A write or a report review is offered only where the Bot has it.
  const bound = selected?.scope ?? emptyBotScope();
  const shared: PublishedBotScope = { localApps: bound.localApps.filter(app => scopeApps.includes(app.id)),
    datasets: bound.datasets.filter(row => scopeData[row.dataset]).map(({ source, dataset, write, reportReview }) => ({ source, dataset, write: write && scopeData[dataset]!.write, reportReview: reportReview && scopeData[dataset]!.reportReview })) };
  const scoped = shared.localApps.length > 0 || shared.datasets.length > 0, scopeInput = scoped ? { scope: shared } : {};
  /** What is published now that the Bot's own scope no longer has: publishing again stops sharing it. */
  const outside = [...(existing?.scope?.localApps ?? []).filter(app => !bound.localApps.some(own => own.id === app.id)).map(app => app.name),
    ...(existing?.scope?.datasets ?? []).filter(row => !bound.datasets.some(own => own.dataset === row.dataset)).map(row => row.dataset)];
  const scopeShown = !!selected && (bound.localApps.length > 0 || bound.datasets.length > 0 || outside.length > 0);
  /** Answering unasked messages, and whether this Desktop has the Decision service that judges them (unknown until the local Bots load). */
  const [autoOn, setAutoOn] = useState(false), [autoDaily, setAutoDaily] = useState(20), [decisionReady, setDecisionReady] = useState<boolean>();
  // A friendship's Bots answer only when asked: the friend chat itself has no Bots in it.
  const autoInput = autoOn && decisionReady && !friend ? { autoReply: { daily: autoDaily } } : {};
  const audience: UsePolicy = friend || mode === 'all' ? { mode: 'all' } : { mode: 'listed', members: members.filter(member => member.uid !== uid && uids.includes(member.uid)).map(({ uid: memberUid, generation }) => ({ uid: memberUid, generation })) };
  const requiredResources = selected?.requiredResources ?? [];
  const carried = current?.documents ?? [];
  // A current file stays shared unless it is dropped; a replaced one keeps its place and its package binding.
  const knowledgeInput = current ? { keptResources: carried.filter(doc => !dropped.includes(doc.resourceId) && !swaps[doc.resourceId]).map(doc => doc.resourceId),
    replacedResources: Object.fromEntries(carried.filter(doc => !dropped.includes(doc.resourceId) && swaps[doc.resourceId]).map(doc => [doc.resourceId, swaps[doc.resourceId]!.token])) } : {};
  const boundNow = (id: string) => carried.some(doc => !dropped.includes(doc.resourceId) && doc.name.startsWith(`${id}: `));
  const missingBindings = requiredResources.some(id => !boundNow(id) && !files.some(file => file.token === resourceBindings[id]));
  const bindingInput = requiredResources.length ? { resourceBindings } : {};
  const owner = (id: string) => members.find(member => member.uid === id)?.displayName ?? id;
  const changed = draft && draft.revision !== (existing?.revision ?? 0);
  const native = api.localBots && api.selectSharedFiles && api.publishLocalBot;
  /** Whether this computer can isolate the Bots it publishes. Unknown until the host has said; a computer that cannot has no Bot to publish from, and says why. */
  const [isolation, setIsolation] = useState<boolean>();
  useEffect(() => {
    if (!native || !api.hostStatus) return undefined;
    let current = true;
    void api.hostStatus().then(value => { if (current) setIsolation(value.isolationAvailable); }, () => undefined);
    return () => { current = false; };
  }, [api, native, uid, team.id, team.generation]);
  const reviewCurrent = !!review && JSON.stringify({ role, examples: exampleInput, fileTokens: files.map(file => file.token), usePolicy: audience, quotas, ...bindingInput, ...listInput, ...knowledgeInput, ...autoInput, ...scopeInput }) === JSON.stringify({ role: review.role, examples: review.examples ?? [], fileTokens: review.fileTokens, usePolicy: review.usePolicy, quotas: review.quotas, ...(review.resourceBindings ? { resourceBindings: review.resourceBindings } : {}), ...(review.sharedList ? { sharedList: review.sharedList } : {}), ...(review.keptResources ? { keptResources: review.keptResources } : {}), ...(review.replacedResources ? { replacedResources: review.replacedResources } : {}), ...(review.autoReply ? { autoReply: review.autoReply } : {}), ...(review.scope ? { scope: review.scope } : {}) });
  const publish = async () => {
    if (!review || !draft || !confirmed || !selected || changed || !reviewCurrent || missingBindings || listMissing || examplesInvalid) throw new Error('PUBLICATION_CONFIRMATION_REQUIRED');
    await api.publishLocalBot!(review);
    setReview(undefined); setConfirmed(false); setInstanceId(''); setRole(''); setExamples(['', '', '']); setFiles([]); setResourceBindings({}); setCurrent(undefined); setDropped([]); setSwaps({}); setDraft(undefined); setListOn(false); setListTitle(''); setListAbout(''); setListAsk(false); setAutoOn(false); setAutoDaily(20); setScopeApps([]); setScopeData({}); await onChanged();
  };
  return <section className="team-bots" aria-label={friend ? t('friends_bots_title') : t('teams_bots_title')}>
    {friend ? null : <><h3>{t('teams_bots_title')}</h3>
    <p className="teams-hint">{t('tw_botdir_intro')}</p></>}
    {status ? <p role="status">{status}</p> : null}
    <ul className="team-bot-grid">{bots.map(bot => <li key={bot.sharedBotId} className="teams-card"><header><b>{bot.name}</b><span className="team-bot-presence" data-online={bot.ready}>{botAvailabilityText(bot, locale)}</span></header><p>{bot.description}</p><small>{t('tw_maintainer')}: {owner(bot.maintainer.uid)}</small><BotMemberDetails bot={bot} locale={locale} busy={busy} showAvailability={false} onExample={onExample ? example => onExample(bot, example) : undefined} />{bot.autoReply ? <p className="teams-hint">{t('tw_auto_reply_card')}</p> : null}
      {friend ? null : <><p className="teams-hint">{bot.usePolicy.mode === 'all' ? t('tw_audience_all') : t('tw_audience_selected')}</p><p className="teams-hint">{t('tw_botdir_quota_line')} {bot.quotas.team} / {bot.quotas.member} / {bot.quotas.bot}</p></>}
      <div className="teams-row-actions"><button disabled={busy || !sharedBotUsable(bot)} onClick={() => void attempt(() => onOpen(bot))}>{bot.permitted ? t('teams_bot_message') : t('teams_bot_not_permitted')}</button>{bot.sharedList && (bot.permitted || bot.maintainer.uid === uid) ? <button disabled={busy} onClick={() => setListOf(bot)}>{t('tw_list')}: {bot.sharedList.title}</button> : null}{maintainsBot(bot, uid, team.generation) && !!bot.autoReply && !denied.includes(bot.sharedBotId) ? <button disabled={busy} onClick={() => setTriageOf(bot)}>{t('teams_triage_title')}</button> : null}{maintainsBot(bot, uid, team.generation) ? <HeldRepliesButton api={api} scope={{ teamId: team.id, generation: team.generation }} bot={bot} locale={locale} busy={busy} onOpenChat={onOpenChat} /> : null}{bot.maintainer.uid === uid && bot.state === 'published' ? <button disabled={busy} onClick={() => setPausing(bot)}>{t('friends_share_bot_stop')}</button> : null}</div>{friend ? null : <BotHandover api={api} uid={uid} team={team} members={members} bot={bot} locale={locale} busy={busy} attempt={attempt} onChanged={onChanged} />}</li>)}</ul>
    {!bots.length && !status ? <p>{friend ? t('friends_bots_none') : t('tw_botdir_empty')}</p> : null}
    {friend ? <p className="teams-hint">{note}</p> : null}
    {api.openBotMarket && !friend ? <button type="button" disabled={busy} onClick={() => void attempt(() => api.openBotMarket!())}>{t('tw_botdir_market')}</button> : null}
    {api.hostStatus ? <SharedHostPanel api={api} teamId={team.id} bots={bots.filter(bot => bot.maintainer.uid === uid && bot.state === 'published')} locale={locale} busy={busy} attempt={attempt} /> : null}
    {native ? <details><summary>{friend ? t('friends_share_bot_with_fmt', friend) : t('tw_botdir_publish_title')}</summary><button disabled={busy || isolation === false} aria-describedby={isolation === false ? `${exampleId}-isolation` : undefined} onClick={() => void attempt(async () => { const token = ++request.current; const bots = await api.localBots!(); const ready = (await api.hostStatus?.().catch(() => undefined))?.decisionConfigured === true; if (live.current && request.current === token) { setLocal(bots); setDecisionReady(ready); } })}>{t('tw_botdir_load_local')}</button>{isolation === false ? <p id={`${exampleId}-isolation`} className="teams-hint">{t('tw_error_isolation')}</p> : null}{local ? <form className="tw-form" onSubmit={event => { event.preventDefault(); setConfirmed(false); if (draft && !examplesInvalid && (!listOn || supportsList)) setReview({ teamId: team.id, generation: team.generation, publicationId: crypto.randomUUID(), sharedBotId: draft.sharedBotId, instanceId, expectedRevision: draft.revision, role, examples: exampleInput, fileTokens: files.map(file => file.token), usePolicy: audience, quotas, ...bindingInput, ...listInput, ...knowledgeInput, ...autoInput, ...scopeInput }); }}>
      <label>{t('tw_botdir_local_bot_model')}<select required disabled={busy} value={instanceId} onChange={event => { const token = ++request.current; const id = event.target.value, bot = bots.find(value => value.maintainer.uid === uid && value.instanceId === id); setInstanceId(id); setExamples(Array.from({ length: 3 }, (_, index) => (bot?.examples ?? local.find(value => value.instanceId === id)?.examples ?? [])[index] ?? '')); setReview(undefined); setConfirmed(false); setFiles([]); setResourceBindings({}); setRole(''); setCurrent(undefined); setDropped([]); setSwaps({}); setMode(bot?.usePolicy.mode ?? 'listed'); setUids(bot?.usePolicy.mode === 'listed' ? bot.usePolicy.members.map(member => member.uid) : []); setQuotas(bot?.quotas ?? { team: 100, member: 20, bot: 100 }); setDraft({ sharedBotId: bot?.sharedBotId ?? crypto.randomUUID(), revision: bot?.revision ?? 0 });
        // A list the Bot already has is carried into the new publication unless its maintainer switches it off.
        setAutoOn(!!bot?.autoReply); setAutoDaily(bot?.autoReply?.daily ?? 20);
        // What the Bot is published with of this Desktop is ticked again; what its own scope no longer has is not offered, and is not sent.
        setScopeApps((bot?.scope?.localApps ?? []).map(app => app.id));
        setScopeData(Object.fromEntries((bot?.scope?.datasets ?? []).map(({ dataset, write, reportReview }) => [dataset, { write, reportReview }])));
        setListOn(!!bot?.sharedList && local.find(value => value.instanceId === id)?.capabilities?.writeOperations === true); setListTitle(bot?.sharedList?.title ?? ''); setListAbout(''); setListAsk(bot?.sharedList?.approval === 'requester');
        if (bot) void attempt(async () => {
          if (bot.sharedList && local.find(value => value.instanceId === id)?.capabilities?.writeOperations === true) { const list = await api.messages<SharedList>(executionCommand({ teamId: team.id, generation: team.generation }, 'readList', { sharedBotId: bot.sharedBotId })); if (!live.current || request.current !== token) return; setListAbout(list.description); }
          // The role and files this Desktop published the Bot with are carried into the form. A publication this Desktop does not hold is entered again.
          const known = await api.publishedKnowledge?.(team.id, bot.sharedBotId).catch(() => undefined);
          if (live.current && request.current === token && known && known.revision === bot.revision) { setCurrent(known); setRole(old => old || known.role); }
        }); }}><option value="">{t('tw_botdir_choose_bot')}</option>{local.map(bot => <option key={bot.instanceId} value={bot.instanceId}>{bot.name}{bot.model ? ` · ${bot.model}` : ''}</option>)}</select></label>
      <label>{t('tw_botdir_published_role')}<textarea required maxLength={12000} value={role} onChange={event => setRole(event.target.value)} /></label><p className="teams-hint">{t('tw_botdir_role_hint')}</p>
      <fieldset><legend>{t('tw_botdir_examples_legend')}</legend><p id={`${exampleId}-hint`} className="teams-hint">{t('tw_botdir_examples_hint')}</p>{examples.map((example, index) => <label key={index} htmlFor={`${exampleId}-${index}`}>{t('tw_botdir_example_fmt', index + 1)}<input type="text" id={`${exampleId}-${index}`} name={`example-${index + 1}`} aria-describedby={`${exampleId}-hint`} maxLength={2000} value={example} onChange={event => setExamples(old => old.map((value, at) => at === index ? event.target.value : value))} /></label>)}</fieldset>
      {current && carried.length ? <fieldset><legend>{t('tw_botdir_shared_now')}</legend>
        <p className="teams-hint">{t('tw_botdir_shared_hint')}</p>
        <ul>{carried.map(doc => { const off = dropped.includes(doc.resourceId), swap = swaps[doc.resourceId]; return <li key={doc.resourceId}>
          {off ? <s>{doc.name}</s> : doc.name} · {doc.kind} · {t('tw_botdir_revision_fmt', doc.revision)}{off ? ` · ${t('tw_botdir_will_stop')}` : swap ? ` · ${t('tw_botdir_will_replace_fmt', swap.name)}` : ''}
          {off ? <button type="button" aria-label={t('tw_botdir_keep_sharing_fmt', doc.name)} onClick={() => setDropped(old => old.filter(id => id !== doc.resourceId))}>{t('tw_botdir_keep_sharing')}</button> : <>
            <button type="button" disabled={busy} aria-label={t('tw_botdir_replace_fmt', doc.name)} onClick={() => void attempt(async () => { const [file] = await api.selectSharedFiles!(instanceId); if (!file) return; if (file.kind !== doc.kind) throw new Error('SHARED_RESOURCE_KIND'); setSwaps(old => ({ ...old, [doc.resourceId]: file })); })}>{t('tw_botdir_replace')}</button>
            {swap ? <button type="button" aria-label={t('tw_botdir_keep_as_is_fmt', doc.name)} onClick={() => setSwaps(old => Object.fromEntries(Object.entries(old).filter(([id]) => id !== doc.resourceId)))}>{t('tw_botdir_undo_replace')}</button> : null}
            <button type="button" aria-label={t('tw_botdir_stop_sharing_fmt', doc.name)} onClick={() => setDropped(old => [...old, doc.resourceId])}>{t('friends_share_bot_stop')}</button></>}
        </li>; })}</ul></fieldset> : null}
      <button type="button" disabled={busy || !instanceId} onClick={() => void attempt(async () => { const selected = await api.selectSharedFiles!(instanceId); setFiles(old => [...old, ...selected.filter(file => !old.some(value => value.token === file.token))]); })}>{t('tw_botdir_select_files')}</button><ul>{files.map(file => <li key={file.token}>{file.name} · {file.kind} · {file.bytes} bytes<button type="button" onClick={() => setFiles(old => old.filter(value => value.token !== file.token))}>{t('teams_list_remove')}</button></li>)}</ul>
      {requiredResources.length ? <fieldset><legend>{t('tw_botdir_bind_resources')}</legend><p className="teams-hint">{t('tw_botdir_import_hint')}</p>{requiredResources.map(id => <label key={id}>{id}<select required={!boundNow(id)} value={resourceBindings[id] ?? ''} onChange={event => setResourceBindings(old => ({ ...old, [id]: event.target.value }))}><option value="">{t('tw_botdir_choose_resource')}</option>{files.map(file => <option key={file.token} value={file.token}>{file.name}</option>)}</select></label>)}</fieldset> : null}
      {friend ? null : <><label>{t('tw_botdir_audience_question')}<select value={mode} onChange={event => setMode(event.target.value as typeof mode)}><option value="listed">{t('tw_audience_selected_members')}</option><option value="all">{t('tw_audience_all')}</option></select></label>{mode === 'listed' ? <fieldset><legend>{t('tw_botdir_allowed_memberships')}</legend>{members.filter(member => member.uid !== uid).map(member => <label key={`${member.uid}:${member.generation}`} className="team-party"><input type="checkbox" checked={uids.includes(member.uid)} onChange={event => setUids(old => event.target.checked ? [...old, member.uid] : old.filter(value => value !== member.uid))} />{member.displayName}</label>)}</fieldset> : null}</>}
      <fieldset><legend>{t('tw_botdir_quotas')}</legend>{(['team', 'member', 'bot'] as const).map(key => <label key={key}>{({ team: t('tw_team'), member: t('teams_member'), bot: t('teams_bot_tag') })[key]}<input type="number" min={1} max={10000} required value={quotas[key]} onChange={event => setQuotas(old => ({ ...old, [key]: Number(event.target.value) }))} /></label>)}</fieldset>
      {friend ? null : <><label className="team-party"><input type="checkbox" checked={autoOn && !!decisionReady} disabled={!decisionReady} onChange={event => setAutoOn(event.target.checked)} />{t('tw_auto_reply_option')}</label>
      {!decisionReady ? <p className="teams-hint">{t('tw_auto_reply_needs_decision')}</p>
        : autoOn ? <><p className="teams-hint">{t('tw_auto_reply_cost_hint')}</p>
          <label>{t('tw_auto_reply_daily')}<input type="number" min={1} max={10000} required value={autoDaily} onChange={event => setAutoDaily(Number(event.target.value))} /></label></>
        : existing?.autoReply ? <p className="teams-hint">{t('tw_auto_reply_unpublish_hint')}</p> : null}</>}
      <label className="team-party"><input type="checkbox" checked={listOn} disabled={!supportsList} onChange={event => { if (supportsList) setListOn(event.target.checked); }} />{t('tw_botdir_list_title')}</label>
      {!supportsList && selected ? <p className="teams-hint">{t('tw_botdir_list_unsupported')}</p> : null}
      {listOn ? <><p className="teams-hint">{t('tw_botdir_list_hint')}</p>
        <label>{t('tw_botdir_list_name')}<input required maxLength={120} value={listTitle} onChange={event => setListTitle(event.target.value)} /></label>
        <label>{t('tw_botdir_list_purpose')}<textarea required maxLength={2000} value={listAbout} onChange={event => setListAbout(event.target.value)} /></label>
        <p className="teams-hint">{t('tw_botdir_list_purpose_hint')}</p>
        <label className="team-party"><input type="checkbox" checked={listAsk} onChange={event => setListAsk(event.target.checked)} />{t('tw_botdir_list_ask')}</label>
        {listAsk ? <p className="teams-hint">{t('tw_botdir_list_ask_hint')}</p> : null}</>
        : existing?.sharedList ? <p className="teams-hint">{t('tw_botdir_list_unpublish_hint')}</p> : null}
      {scopeShown ? <fieldset><legend>{friend ? t('tw_botdir_scope_title_friend_fmt', friend) : t('tw_botdir_scope_title_fmt', team.name)}</legend>
        <p className="teams-hint">{friend ? t('tw_botdir_scope_hint_friend_fmt', friend) : t('tw_botdir_scope_hint')}</p>
        {bound.localApps.length ? <fieldset><legend>{t('teams_bot_scope_apps')}</legend>{bound.localApps.map(app => <label key={app.id} className="team-party"><input type="checkbox" checked={scopeApps.includes(app.id)}
          onChange={event => setScopeApps(old => event.target.checked ? [...old.filter(id => id !== app.id), app.id] : old.filter(id => id !== app.id))} />{app.name}</label>)}</fieldset> : null}
        {bound.datasets.length ? <fieldset><legend>{t('teams_bot_scope_datasets')}</legend>{bound.datasets.map(row => { const chosen = scopeData[row.dataset]; return <div key={row.dataset}>
          <label className="team-party"><input type="checkbox" checked={!!chosen}
            onChange={event => setScopeData(old => event.target.checked ? { ...old, [row.dataset]: { write: false, reportReview: false } } : Object.fromEntries(Object.entries(old).filter(([id]) => id !== row.dataset)))} />{row.dataset}</label>
          {chosen && (row.write || row.reportReview) ? <div className="tw-bot-scope-flags" role="group" aria-label={row.dataset}>
            {row.write ? <label className="team-party"><input type="checkbox" checked={chosen.write}
              onChange={event => setScopeData(old => ({ ...old, [row.dataset]: { write: event.target.checked, reportReview: event.target.checked && chosen.reportReview } }))} />{t('teams_bot_scope_edit')}</label> : null}
            {row.reportReview ? <label className="team-party"><input type="checkbox" checked={chosen.reportReview}
              onChange={event => setScopeData(old => ({ ...old, [row.dataset]: { write: event.target.checked && row.write ? true : chosen.write, reportReview: event.target.checked } }))} />{t('tw_botdir_scope_review')}</label> : null}
          </div> : null}</div>; })}</fieldset> : null}
        {outside.length ? <p className="teams-hint">{t('tw_botdir_scope_lost_fmt', outside.join(', '))}</p> : null}</fieldset> : null}
      <p className="teams-hint">{t(scoped ? 'tw_botdir_host_hint_scope' : 'tw_botdir_host_hint')}</p>
      {changed ? <p role="alert">{t('tw_botdir_changed_elsewhere')}</p> : null}<button disabled={busy || !instanceId || !role.trim() || !!changed || missingBindings || listMissing || examplesInvalid}>{t('tw_botdir_review')}</button>
    </form> : null}</details> : <p className="teams-hint">{friend ? t('friends_bots_share_hint') : t('tw_botdir_open_desktop')}</p>}
    {review && selected ? <Dialog title={t('tw_botdir_confirm')} onClose={() => setReview(undefined)}><div className="tw-form"><h3>{selected.name}</h3><p>{t('tw_botdir_host_this')} · {selected.model ?? t('tw_botdir_model')}</p><blockquote>{review.role}</blockquote>{review.examples?.length ? <><h4>{t('tw_botdir_examples_title')}</h4><ul>{review.examples.map((example, index) => <li key={index}>{example}</li>)}</ul></> : null}<ul>{carried.map(doc => { const swap = swaps[doc.resourceId]; return <li key={doc.resourceId}>{dropped.includes(doc.resourceId) ? `${doc.name} · ${t('tw_botdir_will_stop')}`
          : swap ? `${doc.name} → ${swap.name} · ${doc.kind} · ${t('tw_botdir_revision_fmt', doc.revision + 1)} · ${swap.sha256}` : `${doc.name} · ${doc.kind} · ${t('tw_unchanged')}`}</li>; })}{files.map(file => <li key={file.token}>{file.name} · {file.kind} · {file.sha256}</li>)}</ul><p>{friend ? t('tw_botdir_audience_fmt', friend) : mode === 'all' ? t('tw_botdir_audience_all') : `${t('tw_botdir_audience')}: ${[owner(uid), ...uids.map(owner)].join(', ')}`}</p>{friend ? <p>{note}</p> : null}{requiredResources.length ? <ul>{requiredResources.map(id => <li key={id}>{id}: {files.find(file => file.token === resourceBindings[id])?.name}</li>)}</ul> : null}<p>{t('tw_botdir_quotas_summary')}: {quotas.team} / {quotas.member} / {quotas.bot}</p>{review.autoReply ? <p>{t('tw_auto_reply_review_fmt', review.autoReply.daily)}</p> : null}{review.scope || existing?.scope ? <BotScopeSummary scope={review.scope ?? emptyBotScope()} locale={locale} /> : null}{review.sharedList ? <p>{t('teams_list_open')}: {review.sharedList.title}{review.sharedList.approval === 'requester' ? ` · ${t('tw_botdir_asks_before')}` : ''}</p> : null}<label className="team-party"><input type="checkbox" checked={confirmed} onChange={event => setConfirmed(event.target.checked)} />{t(review.scope ? 'tw_botdir_consent_scope' : 'tw_botdir_consent')}</label>{!reviewCurrent ? <p role="alert">{t('tw_botdir_inputs_changed')}</p> : null}<button disabled={busy || !confirmed || !!changed || !reviewCurrent || missingBindings || examplesInvalid} onClick={() => void attempt(publish)}>{t('tw_botdir_publish')}</button></div></Dialog> : null}
    {listOf ? <SharedListDialog api={api} scope={{ teamId: team.id, generation: team.generation }} bot={listOf} uid={uid} members={members} locale={locale} onClose={() => setListOf(undefined)} /> : null}
    {triageOf ? <TriageLogDialog api={api} scope={{ teamId: team.id, generation: team.generation }} bots={[triageOf]} locale={locale} onClose={() => setTriageOf(undefined)} onDenied={id => { setDenied(old => [...old, id]); setTriageOf(undefined); }} /> : null}
    {pausing ? <Dialog title={t('teams_bot_stop_title_fmt', pausing.name)} role="alertdialog"
      description={t('teams_bot_stop_detail')} onClose={() => setPausing(undefined)}>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setPausing(undefined)}>{t('common_cancel')}</button>
        <button disabled={busy} className="tw-danger-btn" onClick={() => void attempt(async () => { await api.messages(executionCommand({ teamId: team.id, generation: team.generation }, 'pauseSharedBot', { sharedBotId: pausing.sharedBotId, expectedRevision: pausing.revision })); setPausing(undefined); await onChanged(); })}>{t('friends_share_bot_stop')}</button></div></Dialog> : null}
  </section>;
}
