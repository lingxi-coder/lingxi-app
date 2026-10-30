import { useEffect, useReducer, useRef, useState, type ReactElement, type ReactNode } from 'react';
import { canRecoverBot, isQuestionRecipient, teamBotTriggerId, teamBotUsable, type LocalTeamBot, type TeamBotCard, type TeamBotState } from './bots.js';
import type { TeamDetail, TeamsApi, TeamSummary } from './index.js';
import { indexedDbMessageStorage } from './message-storage.js';
import { recoveryKey, TeamMessaging, type TeamMessage } from './messages.js';

export function MessagesPanel({ api, uid, team, members, zh, onClose }: {
  api: TeamsApi; uid: string; team: TeamSummary; members: TeamDetail['members']; zh: boolean; onClose(): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const submitting = useRef(false);
  const [, render] = useReducer(value => value + 1, 0);
  const [model, setModel] = useState<TeamMessaging>();
  const [text, setText] = useState(''), [title, setTitle] = useState('');
  const [kind, setKind] = useState<'dm' | 'group' | 'channel'>('dm');
  const [parties, setParties] = useState<string[]>([]), [botParties, setBotParties] = useState<string[]>([]), [targets, setTargets] = useState<string[]>([]);
  const [bots, setBots] = useState<TeamBotCard[]>([]);
  const [botStatus, setBotStatus] = useState('');
  const checkedRuns = useRef(new Map<string, TeamBotState | null>());
  const [runStates, setRunStates] = useState<{ conversationId: string; values: Record<string, TeamBotState> }>({ conversationId: '', values: {} });
  const [editing, setEditing] = useState<TeamMessage>(), [reply, setReply] = useState<TeamMessage>();
  const [recall, setRecall] = useState<TeamMessage>();
  const [error, setError] = useState(''), [busy, setBusy] = useState(false), [shown, setShown] = useState(100);
  useEffect(() => {
    const current = new TeamMessaging(api, indexedDbMessageStorage(), uid, { teamId: team.id, generation: team.generation });
    const stop = current.onChanged(render); setModel(current);
    void current.start().catch(() => setError(t('Local message storage is unavailable. Retry after freeing space.', '本地消息存储不可用，请检查空间后重试。')));
    const online = () => void current.refresh(); window.addEventListener('online', online);
    return () => { stop(); current.close(); window.removeEventListener('online', online); };
  // The controller owns one account and one membership generation.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, uid, team.id, team.generation]);
  function errorText(code: string): string {
    if (['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(code)) {
      return t('Access changed. Return to members and refresh before continuing.', '访问权限已变化，请返回成员页并刷新。');
    }
    if (/STORAGE|Quota|disk/iu.test(code)) {
      return t('Local message storage is unavailable. Check free space and try again.', '本地消息存储不可用，请检查空间后重试。');
    }
    switch (code) {
      case 'BOT_USE_REQUIRED':
      case 'BOT_PARTICIPANT_REQUIRED':
      case 'INVALID_BOT_TARGET':
        return t('Bot access changed. Sync and check the selected Bots before sending again.', 'Bot 权限已变化，请同步并检查所选 Bot 后重试。');
      case 'QUESTION_RECIPIENT_REQUIRED':
      case 'ANSWER_CONFLICT':
      case 'INVALID_ANSWER':
        return t('This answer cannot be accepted. Check the question and original requester.', '此答案无法接收，请检查问题与原始请求人身份。');
      case 'TEAM_MODEL_UNSUPPORTED':
        return t('This model configuration cannot be used for Team Bots. Check the provider settings on Desktop or choose another model.', '当前模型配置不能用于团队 Bot，请检查 Desktop 的供应商配置或选择其他模型。');
      case 'TEAM_MODEL_UNAVAILABLE':
        return t('This Bot’s model or API key is unavailable. Check its model and provider settings on Desktop.', '此 Bot 的模型或 API 密钥不可用，请检查 Desktop 的模型与供应商设置。');
      case 'TEAM_RUNTIME_UNAVAILABLE':
        return t('The isolated Teams runtime is unavailable on this Desktop. Install a Desktop build that includes it.', '此 Desktop 缺少团队隔离运行时，请安装包含此运行时的 Desktop 版本。');
      case 'TEAM_HOST_MACOS_REQUIRED':
        return t('Sharing a Team Bot currently requires macOS Desktop.', '共享团队 Bot 当前需要 macOS Desktop。');
      case 'TEAM_HOST_UNAVAILABLE':
        return t('The Desktop Bot host is unavailable. Check the Desktop service and try again.', 'Desktop Bot 主机不可用，请检查 Desktop 服务后重试。');
      case 'RECOVERY_NOT_REQUIRED':
        return t('The run is not eligible for a new settlement. Refresh and review its current status.', '此运行当前无法创建新的处理结果，请刷新并检查状态。');
      case 'BOT_RESULT_CONFLICT':
        return t('A different settlement is already saved for this request. Retry the saved result unchanged.', '此请求已保存不同的处理结果，请重试原始结果。');
      case 'BOT_REVISION_CONFLICT':
        return t('Bot settings changed. Refresh before saving again.', 'Bot 设置已变化，请刷新后重试。');
      case 'MESSAGE_RECALLED':
      case 'MESSAGE_ID_CONFLICT':
      case 'MESSAGE_AUTHOR_REQUIRED':
      case 'PARTICIPANT_REQUIRED':
      case 'CONVERSATION_REJECTED':
      case 'INVALID_INPUT':
        return t('This pending change was rejected. Discard it and try again.', '此待发操作未被接受，请丢弃后重试。');
      default:
        return t('Could not sync messages. Check your connection and try again.', '消息同步未完成，请检查网络后重试。');
    }
  }
  const selected = model?.selected ?? '';
  useEffect(() => { setEditing(undefined); setReply(undefined); setRecall(undefined); setText(''); setShown(100); setTargets([]); }, [selected]);
  const attempt = async (action: () => Promise<unknown>) => {
    if (submitting.current) return; submitting.current = true;
    setBusy(true); setError('');
    try { await action(); } catch (error) { setError(error instanceof Error ? error.message : t('Try again.', '请重试。')); }
    finally { submitting.current = false; setBusy(false); }
  };
  const conversation = model?.state.conversations.find(value => value.id === selected);
  const botDmPermitted = conversation?.kind !== 'bot_dm' || bots.some(bot => teamBotUsable(bot) && conversation.participants.includes(bot.key));
  const messages = model?.state.threads[selected]?.messages ?? [];
  const pending = model?.state.outbox.filter(value => value.conversationId === selected) ?? [];
  const states = runStates.conversationId === selected ? runStates.values : {};
  const triggerSources = JSON.stringify(messages.slice(-shown).flatMap<{ messageId: string; triggerId?: string; botKey?: string }>(message => {
    if (message.triggerId) return [{ messageId: message.id, triggerId: message.triggerId }];
    if (message.author.kind !== 'user' || message.recalled) return [];
    const botKeys = conversation?.kind === 'bot_dm'
      ? conversation.participants.filter(value => value.startsWith('bot:'))
      : message.mentions ?? [];
    return botKeys.map(botKey => ({ messageId: message.id, botKey }));
  }));
  const [triggerMap, setTriggerMap] = useState<{ conversationId: string; values: Record<string, string[]> }>({ conversationId: '', values: {} });
  useEffect(() => {
    let closed = false;
    void (async () => {
      const values: Record<string, string[]> = {};
      const sources = JSON.parse(triggerSources) as { messageId: string; triggerId?: string; botKey?: string }[];
      for (const source of sources) {
        const id = source.triggerId ?? await teamBotTriggerId(source.messageId, source.botKey!);
        (values[source.messageId] ??= []).push(id);
      }
      if (!closed) setTriggerMap({ conversationId: selected, values });
    })().catch(() => { if (!closed) setTriggerMap({ conversationId: selected, values: {} }); });
    return () => { closed = true; };
  }, [selected, triggerSources]);
  const messageTriggers = triggerMap.conversationId === selected ? triggerMap.values : {};
  const triggers = [...new Set(Object.values(messageTriggers).flat())].sort().join(',');
  useEffect(() => {
    let closed = false, running = false;
    const refresh = async () => {
      if (running) return; running = true;
      try {
        const result = await api.messages<{ bots: TeamBotCard[] }>({ action: 'bots', teamId: team.id, generation: team.generation });
        if (!closed) { setBots(result.bots); setBotStatus(''); }
      } catch { if (!closed) { setBots([]); setBotStatus(t('Bot availability could not be verified.', '未能确认 Bot 可用状态。')); } }
      finally { running = false; }
    };
    void refresh(); const timer = setInterval(() => void refresh(), 15000);
    return () => { closed = true; clearInterval(timer); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, team.id, team.generation]);
  useEffect(() => {
    let closed = false, running = false;
    const refresh = async () => {
      if (!selected || !triggers || running) return; running = true;
      const ids = triggers.split(','), values: Record<string, TeamBotState> = {};
      // Bound request concurrency and retain settled states; old history never polls forever.
      for (let offset = 0; offset < ids.length && !closed; offset += 4) {
        await Promise.all(ids.slice(offset, offset + 4).map(async triggerId => {
          const key = JSON.stringify([uid, team.id, team.generation, selected, triggerId]);
          if (checkedRuns.current.has(key)) { const cached = checkedRuns.current.get(key); if (cached) values[triggerId] = cached; return; }
          try {
            const state = await api.messages<TeamBotState>({ action: 'botState', teamId: team.id, generation: team.generation, conversationId: selected, triggerId });
            if (closed) return;
            values[triggerId] = state;
            if (['completed', 'failed', 'missed', 'cancelled', 'recovered', 'revoked'].includes(state.state)) checkedRuns.current.set(key, state);
          } catch (error) { if (!closed && error instanceof Error && error.message === 'TRIGGER_REQUIRED') checkedRuns.current.set(key, null); }
        }));
      }
      if (!closed) setRunStates({ conversationId: selected, values });
      running = false;
    };
    void refresh(); const timer = setInterval(() => void refresh(), 5000);
    return () => { closed = true; clearInterval(timer); };
  }, [api, uid, selected, triggers, team.id, team.generation]);
  const name = (id: string) => members.find(member => member.uid === id)?.displayName ?? (id === uid ? t('You', '你') : t('Member', '成员'));
  const submit = () => attempt(async () => {
    if (!model) return;
    if (!botDmPermitted || (!editing && targets.some(key => !bots.some(bot => bot.key === key && teamBotUsable(bot))))) throw new Error('BOT_USE_REQUIRED');
    await model.queue(editing ? 'edit' : 'create', text, editing?.id, reply?.id, editing ? [] : targets);
    setTargets([]);
    setText(value => value === text ? '' : value); setEditing(value => value?.id === editing?.id ? undefined : value); setReply(value => value?.id === reply?.id ? undefined : value);
  });
  return <section className="team-messaging" aria-label={t('Team conversations', '团队会话')}>
    <header className="teams-team-header"><div><h2>{team.name}</h2><p>{t('Conversations · platform readable', '会话 · 平台可读')}</p></div><button onClick={onClose}>{t('Back to members', '返回成员')}</button></header>
    <p role="status" className="teams-hint">{model?.offline ? t('Saved on this device. Reconnect to confirm access and send pending messages.', '当前显示本机缓存。联网后确认权限并发送待发消息。') : t('Connected. Message history is saved on this device.', '已连接。消息历史保存在本机。')}</p>
    {error || model?.error ? <p role="alert" className="teams-error">{errorText(error || model?.error || "")}</p> : null}
    {model?.state.creation ? <p role="status">{t("Creating conversation…", "正在创建会话…")} {model.state.creation.title} {model.state.creation.error ? errorText(model.state.creation.error) : ""} <button disabled={Boolean(model.state.creation.attempted && !model.state.creation.error) || (model.state.creation.leaseUntil ?? 0) > Date.now()} onClick={() => void attempt(() => model.discardCreation())}>{t("Discard", "丢弃")}</button></p> : null}
    <BotDirectory api={api} uid={uid} team={team} members={members} bots={bots} zh={zh} busy={busy} status={botStatus} attempt={attempt} onOpen={bot => teamBotUsable(bot) ? model?.create('bot_dm', bot.name, [uid], [bot.key]) ?? Promise.resolve() : Promise.reject(new Error('BOT_USE_REQUIRED'))} onChanged={async () => { const result = await api.messages<{ bots: TeamBotCard[] }>({ action: 'bots', teamId: team.id, generation: team.generation }); setBots(result.bots); }} />
    <div className="team-message-layout"><aside>
      <button disabled={busy} onClick={() => void model?.refresh()}>{t('Sync messages', '同步消息')}</button>
      <nav aria-label={t('Conversations', '会话')}>{model?.state.conversations.map(value => <button key={value.id} className="team-thread" aria-current={value.id === selected ? 'page' : undefined} onClick={() => model.select(value.id)}><b>{value.title}</b><span>{({ dm: t('Direct', '私聊'), group: t('Group', '群聊'), channel: t('Channel', '频道'), bot_dm: 'Bot' })[value.kind]} · {Math.max(0, value.lastSeq - value.lastReadSeq)} {t('unread', '未读')}</span></button>)}</nav>
      <details><summary>{t('New conversation', '新建会话')}</summary><form onSubmit={event => { event.preventDefault(); void attempt(async () => { if (botParties.some(key => !bots.some(bot => bot.key === key && teamBotUsable(bot)))) throw new Error('BOT_USE_REQUIRED'); await model?.create(kind, title.trim(), [uid, ...parties], botParties); setTitle(''); setParties([]); setBotParties([]); }); }}>
        <label>{t('Title', '标题')}<input value={title} onChange={event => setTitle(event.target.value)} maxLength={120} required /></label>
        <label>{t('Type', '类型')}<select value={kind} onChange={event => { setKind(event.target.value as typeof kind); setParties([]); setBotParties([]); }}><option value="dm">{t('Direct message', '私聊')}</option><option value="group">{t('Group', '群聊')}</option><option value="channel">{t('Channel', '频道')}</option></select></label>
        <fieldset><legend>{t('Participants', '参与成员')}</legend>{members.filter(member => member.uid !== uid).map(member => <label key={member.uid} className="team-party"><input type={kind === 'dm' ? 'radio' : 'checkbox'} name="participants" checked={parties.includes(member.uid)} onChange={event => {
          if (kind === 'dm') setParties([member.uid]);
          else if (event.target.checked) setParties([...parties, member.uid]);
          else setParties(parties.filter(id => id !== member.uid));
        }} />{member.displayName}</label>)}</fieldset>
        {kind !== 'dm' ? <fieldset><legend>{t('Participating Bots', '参与的 Bot')}</legend>{bots.filter(teamBotUsable).map(bot => <label key={bot.key} className="team-party"><input type="checkbox" checked={botParties.includes(bot.key)} disabled={!botParties.includes(bot.key) && botParties.length >= 8} onChange={event => setBotParties(event.target.checked ? [...botParties, bot.key] : botParties.filter(key => key !== bot.key))} />{bot.name} · {name(bot.ownerUid)}</label>)}</fieldset> : null}
        <button disabled={busy || !title.trim() || (parties.length === 0 && botParties.length === 0) || Boolean(model?.offline)}>{t('Create conversation', '创建会话')}</button>
      </form></details>
    </aside><div className="team-message-detail">
      {conversation ? <><header className="teams-team-header"><h3>{conversation.title}</h3><button disabled={busy || model?.offline} onClick={() => void attempt(() => model!.markRead())}>{t('Mark read', '标为已读')}</button></header>
        {messages.length > shown ? <button onClick={() => setShown(value => value + 100)}>{t('Show earlier messages', '显示更早消息')}</button> : null}
        <ol className="team-messages" aria-label={t('Messages', '消息')}>{messages.slice(-shown).map(message => <li key={message.id} data-message-id={message.id} data-message-seq={message.seq}>
          <header><b>{message.author.kind === 'bot' ? bots.find(bot => bot.key === message.author.botKey)?.name ?? 'Bot' : name(message.author.uid)}</b>{message.author.kind === 'bot' ? <small>{t('Owner', '所有者')}: {name(message.author.uid)}</small> : null}<time>{message.createdAt ? new Date(message.createdAt).toLocaleString() : ''}</time>{message.editedAt ? <small>{t('Edited', '已编辑')}</small> : null}</header>
          {message.replyTo ? <blockquote>{messages.find(value => value.id === message.replyTo)?.recalled ? t('Recalled message', '已撤回消息') : messages.find(value => value.id === message.replyTo)?.text ?? t('Reply to an earlier message', '回复早前的消息')}</blockquote> : null}
          <p>{message.recalled ? <em>{t('Message recalled', '消息已撤回')}</em> : message.text}</p>
          {message.question && !message.recalled ? <BotQuestion message={message} uid={uid} generation={team.generation} state={message.triggerId ? states[message.triggerId] : undefined} model={model!} busy={busy} zh={zh} attempt={attempt} /> : null}
          {(messageTriggers[message.id] ?? []).filter(triggerId => states[triggerId] && !messages.slice(messages.indexOf(message) + 1).some(value => messageTriggers[value.id]?.includes(triggerId))).map(triggerId => <BotRun key={triggerId} state={states[triggerId]!} triggerId={triggerId} conversationId={selected} model={model!} uid={uid} zh={zh} busy={busy} attempt={attempt} />)}
          {!message.recalled ? <div className="teams-row-actions"><button onClick={() => { setReply(message); setEditing(undefined); }}>{t('Reply', '回复')}</button>{message.author.uid === uid && message.author.kind === 'user' ? <><button onClick={() => { setEditing(message); setReply(undefined); setText(message.text); }}>{t('Edit', '编辑')}</button><button className="teams-danger" onClick={() => setRecall(message)}>{t('Recall', '撤回')}</button></> : null}</div> : null}
        </li>)}</ol>
        {pending.map(intent => <div key={intent.changeId} className="team-pending"><small>{(intent.error ? errorText(intent.error) : '') || (intent.attempted ? t('Waiting for server confirmation. Retries keep the same message identity.', '等待服务器确认，重试会沿用同一条消息。') : t('Pending · sends when connected', '待发送 · 联网后发送'))}</small><p>{intent.operation === 'recall' ? t('Recall message', '撤回消息') : intent.text}</p><button disabled={Boolean(intent.attempted && !intent.error) || (intent.leaseUntil ?? 0) > Date.now()} onClick={() => void attempt(() => model!.discard(intent.changeId))}>{t('Discard', '丢弃')}</button></div>)}
        <form className="team-composer" onSubmit={event => { event.preventDefault(); void submit(); }}>
          {editing || reply ? <div>{editing ? t('Editing message', '正在编辑消息') : t('Replying', '正在回复')}<button type="button" onClick={() => { setEditing(undefined); setReply(undefined); setText(''); }}>{t('Cancel', '取消')}</button></div> : null}
          {!editing && conversation.kind !== 'dm' && conversation.kind !== 'bot_dm' ? <fieldset><legend>{t('Ask Bots explicitly', '明确请求 Bot')}</legend><p className="teams-hint">{t('Only checked Bots run. Typing @name does not run a Bot.', '仅勾选的 Bot 会执行。输入 @名称 不会执行 Bot。')}</p>{bots.filter(bot => teamBotUsable(bot) && conversation.participants.includes(bot.key)).map(bot => <label key={bot.key} className="team-party"><input type="checkbox" checked={targets.includes(bot.key)} disabled={!targets.includes(bot.key) && targets.length >= 8} onChange={event => setTargets(event.target.checked ? [...targets, bot.key] : targets.filter(key => key !== bot.key))} />{bot.name} · {bot.online ? t('Online', '在线') : t('Offline · queued', '离线 · 将排队')}</label>)}</fieldset> : null}
          {conversation.kind === 'bot_dm' ? <p className="teams-hint">{t('Messages request this Bot automatically. Its owner can read this conversation.', '发送消息会请求此 Bot。其所有者可阅读此会话。')}</p> : null}
          <label>{t('Message', '消息')}<textarea disabled={busy || !botDmPermitted} value={text} onChange={event => setText(event.target.value)} rows={3} maxLength={8192} /></label>
          <button disabled={busy || !botDmPermitted || !text.trim() || new TextEncoder().encode(text).length > 8192}>{editing ? t('Save edit', '保存修改') : t('Send', '发送')}</button>
        </form>
      </> : <p>{t('Choose a conversation or start one with team members.', '选择会话，或与团队成员新建会话。')}</p>}
    </div></div>
    {recall ? <div className="teams-modal"><section role="alertdialog" aria-modal="true" aria-label={t('Recall message?', '撤回消息？')}><h2>{t('Recall this message for everyone?', '为所有人撤回此消息？')}</h2><div><button autoFocus onClick={() => setRecall(undefined)}>{t('Cancel', '取消')}</button><button disabled={busy} className="teams-danger" onClick={() => void attempt(async () => { await model?.queue('recall', '', recall.id); setRecall(undefined); })}>{t('Recall', '撤回')}</button></div></section></div> : null}
  </section>;
}

function BotDirectory({ api, uid, team, members, bots, zh, busy, status, attempt, onOpen, onChanged }: {
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

function BotQuestion({ message, uid, generation, state, model, busy, zh, attempt }: {
  message: TeamMessage; uid: string; generation: number; state?: TeamBotState; model: TeamMessaging; busy: boolean; zh: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [answers, setAnswers] = useState<Record<string, string>>({});
  const pending = model.state.answerOutbox?.find(value => value.conversationId === model.selected && value.questionId === message.question!.id);
  const answered = state?.questionId === message.question!.id && state.answered || model.state.answerReceipts?.includes(JSON.stringify([model.selected, message.question!.id]));
  const recipient = isQuestionRecipient(message, uid, generation);
  const active = state?.state === 'running' && state.questionId === message.question!.id && state.origin.uid === uid && state.origin.generation === generation && (state.leaseUntil ?? 0) > Date.now();
  let content: ReactNode;
  if (answered) {
    content = <p role="status">{t('Answer received.', '答案已接收。')}</p>;
  } else if (pending) {
    content = <>
      <p role="status">{pending.error ? t('Answer was rejected. It was not changed or resent with a new identity.', '答案被拒绝，未更改内容或使用新身份重发。') : t('Answer saved. Waiting for confirmation; reconnect retries this exact answer.', '答案已保存，等待确认；重新连接后将重试相同答案。')}</p>
      {message.question!.questions.map(question => <p key={question}><b>{question}</b><br />{pending.answers[question]}</p>)}
      {pending.error ? <button disabled={busy} onClick={() => void attempt(() => model.discardAnswer(pending.answerId))}>{t('Discard rejected answer', '丢弃被拒绝的答案')}</button> : null}
    </>;
  } else if (recipient) {
    content = <form onSubmit={event => { event.preventDefault(); void attempt(() => model.answer(message, answers)); }}>
      {message.question!.questions.map(question => <label key={question}>{question}<textarea maxLength={2000} required disabled={busy || !active} value={answers[question] ?? ''} onChange={event => setAnswers(old => ({ ...old, [question]: event.target.value }))} /></label>)}
      <button disabled={busy || !active || message.question!.questions.some(question => !answers[question]?.trim())}>{t('Send answer', '发送答案')}</button>
      {!active ? <p className="teams-hint">{state ? t('This question is no longer active.', '此问题已无法回答。') : t('Waiting to verify this question is still active.', '正在确认此问题是否仍可回答。')}</p> : null}
    </form>;
  } else {
    content = <>
      <ul>{message.question!.questions.map(question => <li key={question}>{question}</li>)}</ul>
      <p className="teams-hint">{t('Only the original requester in the same membership can answer.', '仅同一次成员身份的原始请求人可回答。')}</p>
    </>;
  }
  return <section className="team-bot-question"><h4>{t('Bot question', 'Bot 提问')}</h4>{content}</section>;
}

function BotRun({ state, triggerId, conversationId, model, uid, zh, busy, attempt }: {
  state: TeamBotState; triggerId: string; conversationId: string; model: TeamMessaging; uid: string; zh: boolean; busy: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [text, setText] = useState(''), [confirmed, setConfirmed] = useState(false);
  const key = recoveryKey(conversationId, triggerId);
  const pending = model.state.recoveryOutbox?.find(item => recoveryKey(item.conversationId, item.triggerId) === key);
  const done = model.state.recoveryReceipts?.includes(key);
  const labels: Record<string, [string, string]> = { queued: ['Queued', '已排队'], pending: ['Queued', '已排队'], claimed: ['Preparing', '准备中'], running: ['Processing', '处理中'], recovery: ['Owner recovery required', '需要所有者恢复'], recovered: ['Recovered by owner', '所有者已恢复'], completed: ['Completed', '已完成'], failed: ['Failed', '失败'], missed: ['Expired while offline', '离线期间已过期'], cancelled: ['Cancelled', '已取消'], revoked: ['Access revoked', '授权已撤销'] };
  const hostLabels: Record<NonNullable<TeamBotState['hostStatus']>, [string, string]> = {
    retrying: ['Desktop is reconnecting. The saved request will be retried.', 'Desktop 正在重新连接，将重试已保存的请求。'],
    result_pending: ['The exact result is saved on Desktop. Reconnecting retries delivery without running tools again.', '原始结果已保存在 Desktop，重新连接后将重试交付，不会再次运行工具。'],
    recovery_required: ['Desktop stopped this interrupted run. Review it before settling the request.', 'Desktop 已停止此中断运行，请检查后结束请求。'],
    model_unavailable: ['The configured model or credential is unavailable on Desktop. Check provider settings.', 'Desktop 上配置的模型或凭证不可用，请检查供应商设置。'],
    runtime_unavailable: ['The isolated Teams runtime is unavailable on Desktop. Check the installed Desktop build.', 'Desktop 的团队隔离运行时不可用，请检查已安装版本。'],
    storage_error: ['Desktop could not save the request state. Check local storage before retrying.', 'Desktop 无法保存请求状态，请检查本地存储后重试。'],
  };
  let recovery: ReactNode = null;
  if (pending) {
    recovery = <div className="team-pending">
      <p role="status">{pending.error ? t('Settlement was rejected. Review the run before creating a new settlement.', '处理结果被拒绝，请检查运行状态后再创建新的处理结果。') : t('Settlement saved. Reconnecting retries this exact result without running tools.', '处理结果已保存，重新连接后将重试交付原始结果，不会运行工具。')}</p>
      <p>{pending.text}</p>
      {pending.error ? <button disabled={busy} onClick={() => void attempt(() => model.discardRejectedRecovery(conversationId, triggerId))}>{t('Discard rejected settlement', '丢弃被拒绝的处理结果')}</button> : null}
    </div>;
  } else if (!done && canRecoverBot(state, uid)) {
    recovery = <details>
      <summary>{t('Resolve interrupted request', '处理已中断请求')}</summary>
      <p>{t('This settles the request without running its tools again. Check any external effects before writing the result.', '此操作会结束该请求，不会重新执行工具。请先检查外部操作结果。')}</p>
      <form onSubmit={event => { event.preventDefault(); void attempt(() => model.recover(conversationId, triggerId, text)); }}>
        <label>{t('Result for the conversation', '发送到会话的处理结果')}<textarea required value={text} maxLength={8192} onChange={event => setText(event.target.value)} /></label>
        <label className="team-party"><input type="checkbox" checked={confirmed} onChange={event => setConfirmed(event.target.checked)} />{t('I checked the interrupted request and want to settle it.', '我已检查中断请求并确认结束。')}</label>
        <button disabled={busy || !confirmed || !text.trim() || new TextEncoder().encode(text).length > 8192}>{t('Settle request', '结束请求')}</button>
      </form>
    </details>;
  }
  return <div className="team-bot-run"><p role="status">Bot · {done ? t('Recovered by owner', '所有者已恢复') : labels[state.state]?.[zh ? 1 : 0] ?? state.state}</p>{uid === state.ownerUid && state.hostStatus ? <p role="status" className="teams-hint">{hostLabels[state.hostStatus][zh ? 1 : 0]}</p> : null}{recovery}</div>;
}
