import { useCallback, useEffect, useRef, useState, type ReactElement } from 'react';
import type { LocalPublishableBot, SharedHostBot, TeamTaskView } from './execution.js';
import { availabilityText, newServiceId, serviceCommand, serviceErrorText, serviceIdFrom, type HostedServicesApi, type PublishedService,
  type ServiceList, type ServiceMessage, type ServiceReportReason, type ServiceScope, type ServiceSession, type ServiceSync, type ServiceView } from './hosted-service.js';
import { Avatar, Dialog, type RenderMarkdown } from './ui';

/**
 * Bots offered outside Teams (hosted Bot services), for the person who uses one and, on a computer that hosts Bots, for
 * the person who offers one. A service is not a Team: nothing here reads a Team, and a caller sees one Bot, their own
 * sessions with it, and what its publisher said about cost, visibility and retention before they agreed to enter.
 */
export interface HostedServicesProps {
  api: HostedServicesApi; uid: string; zh: boolean;
  /** Renders the Bot's answers as Markdown, with the same renderer the Team chat uses. What a person types stays plain text. */
  renderMarkdown?: RenderMarkdown;
  /** A service to look up at once: the one a link named. */
  initialServiceId?: string;
  /** The link a publisher gives to people, for a service id. Without it the id itself is shown. */
  link?(serviceId: string): string;
  /** People the publisher can offer a service to by name (their friends). Without it a service is offered to anyone with the link. */
  people?: ReadonlyArray<{ uid: string; name: string }>;
  /** How often an open session is read again, in milliseconds: while the Bot is answering, and otherwise. */
  refreshMs?: { answering: number; idle: number };
}
const code = (error: unknown) => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
const LIVE = ['queued', 'running', 'waiting'];

export function HostedServices({ api, uid, zh, renderMarkdown, initialServiceId, link, people, refreshMs = { answering: 2000, idle: 10000 } }: HostedServicesProps): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [list, setList] = useState<ServiceList>(), [status, setStatus] = useState(''), [busy, setBusy] = useState(false);
  const [lookup, setLookup] = useState(''), [candidate, setCandidate] = useState<ServiceView>(), [open, setOpen] = useState<ServiceView>();
  const attempt = useCallback(async (action: () => Promise<unknown>) => {
    setBusy(true); setStatus('');
    try { await action(); } catch (error) { setStatus(serviceErrorText(code(error), zh)); } finally { setBusy(false); }
  }, [zh]);
  const refresh = useCallback(async () => setList(await api.services<ServiceList>({ action: 'listServices' })), [api]);
  const find = useCallback(async (serviceId: string) => {
    const view = await api.services<ServiceView | PublishedService>({ action: 'getService', serviceId });
    // The publisher is told what they set, not a caller's terms: they do not use their own service as a caller.
    if ('publisher' in view) { setCandidate(undefined); setOpen(undefined);
      setStatus(zh ? '这是你提供的服务。请在托管该 Bot 的电脑上管理它。' : 'This is a service you offer. You manage it on the computer that hosts its Bot.'); return; }
    // Looking a service up shows what it is. Nothing is entered and nothing is sent to its Bot.
    if (view.entered) { setCandidate(undefined); setOpen(view); } else { setOpen(undefined); setCandidate(view); }
  }, [api, zh]);
  useEffect(() => { void attempt(async () => { await refresh(); if (initialServiceId) await find(initialServiceId); }); }, [attempt, refresh, find, initialServiceId]);
  if (open) return <ServiceSessions api={api} uid={uid} zh={zh} service={open} renderMarkdown={renderMarkdown} refreshMs={refreshMs}
    onBack={() => { setOpen(undefined); void attempt(refresh); }} onLeft={() => { setOpen(undefined); void attempt(refresh); }} />;
  return <section className="tw-scope tw-embed hosted-services" aria-label={t('Bot services', 'Bot 服务')}>
    <h2>{t('Bot services', 'Bot 服务')}</h2>
    <p className="teams-hint">{t('A Bot someone offers outside their Teams. You talk to the Bot only: you do not join a Team and see nobody else who uses it.', '他人在团队之外提供的 Bot。你只与该 Bot 对话：不会加入任何团队，也看不到其他使用者。')}</p>
    {status ? <p role="alert">{status}</p> : null}
    <form className="tw-form" onSubmit={event => { event.preventDefault(); const serviceId = serviceIdFrom(lookup);
      if (!serviceId) { setStatus(t('That is not a service link.', '这不是服务链接。')); return; } void attempt(() => find(serviceId)); }}>
      <label>{t('Open a service by its link', '通过链接打开服务')}<input value={lookup} onChange={event => setLookup(event.target.value)} placeholder="https://…/services/s_…" autoComplete="off" spellCheck={false} /></label>
      <button disabled={busy || !lookup.trim()}>{t('Look it up', '查看')}</button>
    </form>
    {candidate ? <ServiceTerms service={candidate} zh={zh} busy={busy} onCancel={() => setCandidate(undefined)}
      onEnter={() => void attempt(async () => { const entered = await api.services<{ generation: number; service: ServiceView }>({ action: 'enterService', serviceId: candidate.serviceId, acknowledged: true });
        setCandidate(undefined); setOpen({ ...entered.service, entered: true, generation: entered.generation }); })} /> : null}
    <h3>{t('Services you use', '你使用的服务')}</h3>
    {list && !list.entered.length ? <p className="teams-hint">{t('None yet.', '暂无。')}</p> : null}
    <ul className="team-bot-grid">{list?.entered.map(service => <li key={service.serviceId} className="teams-card">
      <header><b>{service.name}</b><span className="team-bot-presence" data-online={service.availability === 'available'}>{availabilityText(service.availability, zh)}</span></header>
      <p>{service.description}</p><small>{t('Offered by', '提供者')}: {service.publisherName}</small>
      <div className="teams-row-actions"><button disabled={busy} onClick={() => setOpen(service)}>{t('Open', '打开')}</button></div>
    </li>)}</ul>
    {api.localBots && api.publishLocalBot ? <ServicePublisher api={api} zh={zh} busy={busy} attempt={attempt} published={list?.published ?? []} onChanged={refresh} link={link} people={people} /> : null}
  </section>;
}

/** What a person is told before they enter: who offers the Bot, who pays, who can read the session, what is kept, how much they may ask. */
function ServiceTerms({ service, zh, busy, onEnter, onCancel }: { service: ServiceView; zh: boolean; busy: boolean; onEnter(): void; onCancel(): void }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en, who = service.publisherName;
  return <Dialog title={service.name} onClose={onCancel}><div className="tw-form">
    <p>{service.description}</p>
    <p>{t(`Offered by ${who}. This is not a Team: you will not see ${who}’s Teams, members or other Bots, and other people who use this Bot do not see you.`, `由 ${who} 提供。这不是团队：你看不到 ${who} 的团队、成员或其他 Bot，其他使用者也看不到你。`)}</p>
    <ul>
      <li>{t(`Who can read your sessions: you and ${who}. ${who} runs the Bot on their own computer and can read everything you write to it and everything it answers.`, `谁能阅读你的会话：你和 ${who}。${who} 在自己的电脑上运行该 Bot，可以阅读你发送的全部内容和 Bot 的全部回答。`)}</li>
      <li>{t(`Who pays: ${who} pays for the model. You are not charged.`, `谁付费：模型费用由 ${who} 承担，你无需付费。`)}</li>
      <li>{t('What is kept, as the publisher states it', '保留内容（发布者的说明）')}: {service.retention}</li>
      <li>{service.limits.userDaily !== undefined ? t(`You can send ${service.limits.userDaily} requests a day, ${service.limits.userConcurrent} at a time.`, `你每天可发送 ${service.limits.userDaily} 次请求，同时进行 ${service.limits.userConcurrent} 次。`)
        : t(`You can have ${service.limits.userConcurrent} requests under way at a time.`, `你可同时进行 ${service.limits.userConcurrent} 次请求。`)}</li>
      <li>{t('You can report this service to LingXi at any time, and leave it whenever you like.', '你可以随时向灵犀举报该服务，也可以随时退出。')}</li>
    </ul>
    <p className="teams-hint">{availabilityText(service.availability, zh)}</p>
    <div className="tw-dialog-actions"><button type="button" onClick={onCancel}>{t('Not now', '暂不')}</button>
      <button type="button" data-autofocus disabled={busy || service.availability === 'withdrawn'} onClick={onEnter}>{t('Agree and open', '同意并打开')}</button></div>
  </div></Dialog>;
}

/** A caller's sessions with a service's Bot. Everything is read through the API on a timer: a service's records cannot be watched. */
function ServiceSessions({ api, uid, zh, service, renderMarkdown, refreshMs, onBack, onLeft }: {
  api: HostedServicesApi; uid: string; zh: boolean; service: ServiceView; renderMarkdown?: RenderMarkdown; refreshMs: { answering: number; idle: number }; onBack(): void; onLeft(): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const scope: ServiceScope = { serviceId: service.serviceId, generation: service.generation ?? 1 }, scopeKey = `${scope.serviceId}:${scope.generation}`;
  const [view, setView] = useState(service), [sessions, setSessions] = useState<ServiceSession[]>(), [sessionId, setSessionId] = useState<string>();
  const [messages, setMessages] = useState<ServiceMessage[]>([]), [tasks, setTasks] = useState<TeamTaskView[]>([]);
  const [text, setText] = useState(''), [status, setStatus] = useState(''), [busy, setBusy] = useState(false);
  const [reporting, setReporting] = useState(false), [leaving, setLeaving] = useState(false), [tick, setTick] = useState(0);
  const cursor = useRef({ key: '', after: 0, byId: new Map<string, ServiceMessage>() }), end = useRef<HTMLLIElement>(null);
  const attempt = async (action: () => Promise<unknown>) => {
    setBusy(true); setStatus('');
    try { await action(); return true; } catch (error) { setStatus(serviceErrorText(code(error), zh)); return false; } finally { setBusy(false); }
  };
  const loadSessions = useCallback(async () => {
    const found = (await api.serviceMessages<{ conversations: ServiceSession[] }>({ action: 'list', ...scope })).conversations;
    setSessions(found); return found;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, scopeKey]);
  useEffect(() => { void attempt(async () => { const found = await loadSessions(); setSessionId(old => old ?? found.at(-1)?.id); }); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [loadSessions]);
  // The open session: its messages by the change log, its tasks, and whether the service can answer.
  const live = tasks.some(task => LIVE.includes(task.state));
  useEffect(() => {
    if (!sessionId) { setMessages([]); setTasks([]); return; }
    let closed = false, running = false;
    const key = `${scopeKey}:${sessionId}`;
    if (cursor.current.key !== key) { cursor.current = { key, after: 0, byId: new Map() }; setMessages([]); setTasks([]); }
    const pass = async () => {
      if (closed || running || (typeof document !== 'undefined' && document.visibilityState === 'hidden')) return;
      running = true;
      try {
        const at = cursor.current;
        for (;;) {
          const page = await api.serviceMessages<ServiceSync>({ action: 'sync', ...scope, conversationId: sessionId, after: at.after });
          if (closed || cursor.current !== at) return;
          for (const message of page.messages) at.byId.set(message.id, message);
          at.after += page.changes.length;
          if (!page.hasMore || !page.changes.length) break;
        }
        const listed = await api.serviceMessages<{ tasks: TeamTaskView[] }>(serviceCommand(scope, 'listTasks', { conversationId: sessionId }));
        const now = await api.services<ServiceView>({ action: 'getService', serviceId: scope.serviceId });
        if (closed || cursor.current !== at) return;
        setMessages([...at.byId.values()].sort((a, b) => a.seq - b.seq)); setTasks(listed.tasks.sort((a, b) => a.createdAt - b.createdAt)); setView(now);
      } catch (error) { if (!closed) setStatus(serviceErrorText(code(error), zh)); } finally { running = false; }
    };
    void pass();
    // Quick while the Bot is answering, slow otherwise.
    const timer = setInterval(() => void pass(), live ? refreshMs.answering : refreshMs.idle);
    return () => { closed = true; clearInterval(timer); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, scopeKey, sessionId, live, zh, tick, refreshMs.answering, refreshMs.idle]);
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [messages.length, tasks.length]);
  const waiting = tasks.find(task => task.state === 'waiting' && task.wait?.recipient.uid === uid);
  const newSession = () => attempt(async () => {
    const conversationId = crypto.randomUUID();
    await api.serviceMessages(serviceCommand(scope, 'createConversation', { conversationId, kind: 'bot_dm', title: new Date().toLocaleString(zh ? 'zh-CN' : 'en'), participantUids: [uid], sharedBotIds: [service.sharedBotId] }));
    await loadSessions(); setSessionId(conversationId);
  });
  const send = () => attempt(async () => {
    const body = text.trim(); if (!body) return;
    let conversationId = sessionId;
    if (!conversationId) {
      conversationId = crypto.randomUUID();
      await api.serviceMessages(serviceCommand(scope, 'createConversation', { conversationId, kind: 'bot_dm', title: body.slice(0, 60), participantUids: [uid], sharedBotIds: [service.sharedBotId] }));
      // The session exists from here on, whatever becomes of the request: a refused request is sent again into it, not into another.
      await loadSessions(); setSessionId(conversationId);
    }
    const ids = { conversationId, messageId: crypto.randomUUID(), changeId: crypto.randomUUID(), text: body };
    // An answer to the Bot's question continues the waiting request; anything else is a new request.
    if (waiting?.wait?.kind === 'human_input' && conversationId === sessionId) await api.serviceMessages(serviceCommand(scope, 'answerWait', { ...ids, waitId: waiting.wait.waitId }));
    else await api.serviceMessages(serviceCommand(scope, 'submit', { ...ids, targets: [service.sharedBotId] }));
    setText('');
    // The request exists now: read it at once rather than at the next tick.
    setTick(value => value + 1);
  });
  const decide = (decision: 'approve' | 'deny') => attempt(async () => {
    if (!waiting?.wait || !sessionId) return;
    await api.serviceMessages(serviceCommand(scope, 'decideWait', { conversationId: sessionId, waitId: waiting.wait.waitId, messageId: crypto.randomUUID(), changeId: crypto.randomUUID(), decision }));
    setTick(value => value + 1);
  });
  const ended = (task: TeamTaskView) => ({ failed: t('The Bot could not answer this request.', 'Bot 未能回答此请求。'), expired: t('The request ran out of time.', '请求已超时。'),
    interrupted: t('The publisher’s computer stopped before it answered.', '发布者的电脑在回答前中断。'), revoked: t('The request was stopped because access changed.', '由于访问变更，请求已停止。'),
    cancelled: t('You cancelled this request.', '你已取消此请求。'), authorization_required: t('The publisher has to sign in to the Bot’s model again.', '发布者需要重新登录 Bot 的模型账号。'),
    needs_reconciliation: t('The publisher has to check what this request did.', '发布者需要核对此请求的执行结果。') } as Record<string, string>)[task.state];
  const off = view.availability !== 'available';
  return <section className="tw-scope tw-conv hosted-service" aria-label={view.name}>
    <header className="tw-conv-head">
      <button type="button" onClick={onBack}>{t('‹ Services', '‹ 服务')}</button>
      <div className="tw-conv-title"><h2>{view.name}</h2><p>{t('Offered by', '提供者')} {view.publisherName} · <span className="hosted-service-state" data-online={!off}>{availabilityText(view.availability, zh)}</span></p></div>
      <select aria-label={t('Session', '会话')} value={sessionId ?? ''} onChange={event => setSessionId(event.target.value || undefined)}>
        {!sessions?.length ? <option value="">{t('New session', '新会话')}</option> : null}
        {sessions?.map(session => <option key={session.id} value={session.id}>{session.title}</option>)}
      </select>
      <button type="button" disabled={busy} onClick={() => void newSession()}>{t('New session', '新会话')}</button>
      <button type="button" onClick={() => setReporting(true)}>{t('Report', '举报')}</button>
      <button type="button" onClick={() => setLeaving(true)}>{t('Leave', '退出')}</button>
    </header>
    <p className="hosted-service-note">{t(`${view.publisherName} can read this session and pays for the answers.`, `${view.publisherName} 可以阅读此会话，并承担回答的费用。`)}
      {view.limits.userDailyLeft !== undefined ? ` ${t(`Requests left today: ${view.limits.userDailyLeft}.`, `今日剩余请求：${view.limits.userDailyLeft}。`)}` : ''}</p>
    <div className="tw-stream-wrap"><div className="tw-stream" role="log" aria-label={t('Messages', '消息')} tabIndex={0}>
      {!messages.length && !tasks.length ? <p className="tw-empty-line" role="status">{t('Ask the Bot something to begin.', '向 Bot 提问以开始。')}</p> : null}
      <ol className="tw-messages">
      {messages.map(message => { const bot = message.author.kind === 'shared_bot';
        return <li key={message.id} className="tw-msg" data-grouped="false" data-mine={!bot} data-kind={bot ? 'bot' : 'user'} data-message-id={message.id}>
          <div className="tw-msg-lead"><Avatar identity={bot ? { kind: 'bot', id: view.sharedBotId, name: view.name } : { kind: 'user', id: uid, name: t('You', '你') }} size={32} /></div>
          <div className="tw-msg-main">
            <div className="tw-msg-head"><b className="tw-msg-author">{bot ? view.name : t('You', '你')}</b>{bot ? <span className="tw-tag">Bot</span> : null}
              {message.waitId && bot ? <span className="tw-tag">{t('Question', '提问')}</span> : null}
              {message.createdAt ? <time className="tw-msg-time" dateTime={message.createdAt}>{new Date(message.createdAt).toLocaleTimeString(zh ? 'zh-CN' : 'en', { hour: '2-digit', minute: '2-digit' })}</time> : null}</div>
            {bot && renderMarkdown && !message.recalled ? <div className="tw-msg-body tw-md" data-markdown="true">{renderMarkdown(message.text)}</div>
              : <p className="tw-msg-body">{message.recalled ? <em>{t('Message recalled', '消息已撤回')}</em> : message.text}</p>}
          </div></li>; })}
      {tasks.filter(task => task.state !== 'completed').map(task => <li key={task.taskId} className="tw-msg hosted-service-task" data-state={task.state} role="status">
        <div className="tw-msg-lead" /><div className="tw-msg-main"><p className="tw-msg-body hosted-service-note">
          {task.state === 'queued' ? t('Waiting for the publisher’s computer…', '正在等待发布者的电脑…') : task.state === 'running' ? t('The Bot is answering…', 'Bot 正在回答…')
            : task.state === 'waiting' ? (task.wait?.kind === 'tool_approval' ? t('The Bot asks for your approval.', 'Bot 请求你的批准。') : t('The Bot asked you a question. Answer below to continue.', 'Bot 向你提了一个问题，请在下方回答以继续。')) : ended(task)}
          {task.state === 'waiting' && task.wait?.kind === 'tool_approval' && task.wait.recipient.uid === uid ? <> <button type="button" disabled={busy} onClick={() => void decide('approve')}>{t('Approve', '批准')}</button> <button type="button" disabled={busy} onClick={() => void decide('deny')}>{t('Decline', '拒绝')}</button></> : null}
          {LIVE.includes(task.state) ? <> <button type="button" disabled={busy || !!task.cancelRequestedAt} onClick={() => void attempt(async () => { await api.serviceMessages(serviceCommand(scope, 'cancelTask', { taskId: task.taskId })); setTick(value => value + 1); })}>{t('Cancel', '取消')}</button></> : null}
        </p></div></li>)}
      <li ref={end} aria-hidden="true" />
      </ol></div></div>
    {status ? <p role="alert" className="hosted-service-note">{status}</p> : null}
    <form className="hosted-service-composer" onSubmit={event => { event.preventDefault(); void send(); }}>
      <textarea aria-label={t('Message', '消息')} value={text} maxLength={8000} disabled={view.availability === 'withdrawn'} placeholder={waiting?.wait?.kind === 'human_input' ? t('Answer the Bot’s question', '回答 Bot 的问题') : t(`Ask ${view.name}`, `向 ${view.name} 提问`)}
        onChange={event => setText(event.target.value)} onKeyDown={event => { if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) { event.preventDefault(); void send(); } }} />
      <button disabled={busy || !text.trim() || view.availability === 'withdrawn'}>{t('Send', '发送')}</button>
    </form>
    {off && view.availability !== 'withdrawn' ? <p className="hosted-service-note">{t('The service cannot answer right now. You can still read your sessions.', '该服务目前无法回答，你仍可阅读自己的会话。')}</p> : null}
    {reporting ? <ServiceReport api={api} zh={zh} serviceId={scope.serviceId} sessionId={sessionId} onClose={() => setReporting(false)} /> : null}
    {leaving ? <Dialog title={t(`Leave ${view.name}?`, `退出 ${view.name}？`)} role="alertdialog" onClose={() => setLeaving(false)}>
      <p>{t('Requests that have not started are stopped. You cannot read your sessions here while you are out; opening the service again brings them back.', '尚未开始的请求会被停止。退出期间无法阅读这里的会话；重新打开该服务后会恢复。')}</p>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setLeaving(false)}>{t('Cancel', '取消')}</button>
        <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void attempt(async () => { await api.services({ action: 'leaveService', serviceId: scope.serviceId }); onLeft(); })}>{t('Leave', '退出')}</button></div>
    </Dialog> : null}
  </section>;
}

/** A report goes to LingXi, not to the publisher. */
function ServiceReport({ api, zh, serviceId, sessionId, onClose }: { api: HostedServicesApi; zh: boolean; serviceId: string; sessionId?: string; onClose(): void }): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [reason, setReason] = useState<ServiceReportReason>('harmful'), [text, setText] = useState(''), [withSession, setWithSession] = useState(false);
  const [state, setState] = useState<{ busy?: boolean; error?: string; sent?: boolean }>({}), reportId = useRef(crypto.randomUUID());
  const send = async () => {
    setState({ busy: true });
    try { await api.services({ action: 'reportService', serviceId, reportId: reportId.current, reason, text: text.trim(), ...(withSession && sessionId ? { sessionId } : {}) }); setState({ sent: true }); }
    catch (error) { setState({ error: serviceErrorText(code(error), zh) }); }
  };
  return <Dialog title={t('Report this service', '举报该服务')} onClose={onClose}>{state.sent
    ? <div className="tw-form"><p role="status">{t('Your report was sent to LingXi. The publisher is not told who reported.', '你的举报已发送给灵犀。发布者不会得知举报人。')}</p><div className="tw-dialog-actions"><button type="button" data-autofocus onClick={onClose}>{t('Close', '关闭')}</button></div></div>
    : <form className="tw-form" onSubmit={event => { event.preventDefault(); void send(); }}>
      <p className="teams-hint">{t('The report goes to LingXi, not to the publisher.', '举报会发送给灵犀，而不是发布者。')}</p>
      <label>{t('What is wrong', '问题类型')}<select value={reason} onChange={event => setReason(event.target.value as ServiceReportReason)}>
        <option value="harmful">{t('Harmful or dangerous answers', '有害或危险的回答')}</option><option value="deceptive">{t('It deceives people about what it is', '冒充或欺骗')}</option>
        <option value="privacy">{t('It misuses personal information', '滥用个人信息')}</option><option value="spam">{t('Spam or advertising', '垃圾信息或广告')}</option><option value="other">{t('Something else', '其他')}</option></select></label>
      <label>{t('What happened (optional)', '发生了什么（可选）')}<textarea maxLength={1000} value={text} onChange={event => setText(event.target.value)} /></label>
      {sessionId ? <label className="team-party"><input type="checkbox" checked={withSession} onChange={event => setWithSession(event.target.checked)} />{t('Point LingXi to the session that is open now', '附上当前打开的会话')}</label> : null}
      {state.error ? <p role="alert">{state.error}</p> : null}
      <div className="tw-dialog-actions"><button type="button" onClick={onClose}>{t('Cancel', '取消')}</button><button disabled={state.busy}>{t('Send report', '发送举报')}</button></div>
    </form>}</Dialog>;
}

/** The publisher's side, on the computer that hosts the Bot: offer a Bot, switch a service on and off, withdraw it. */
function ServicePublisher({ api, zh, busy, attempt, published, onChanged, link, people }: {
  api: HostedServicesApi; zh: boolean; busy: boolean; attempt(action: () => Promise<unknown>): Promise<void>; published: PublishedService[]; onChanged(): Promise<void>;
  link?(serviceId: string): string; people?: ReadonlyArray<{ uid: string; name: string }>;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const [local, setLocal] = useState<LocalPublishableBot[]>(), [instanceId, setInstanceId] = useState(''), [name, setName] = useState(''), [description, setDescription] = useState('');
  const [role, setRole] = useState(''), [everyone, setEveryone] = useState(!people?.length), [uids, setUids] = useState<string[]>([]);
  const [daily, setDaily] = useState({ user: 20, service: 200 }), [limits, setLimits] = useState({ userConcurrent: 1, serviceConcurrent: 4 });
  const [retention, setRetention] = useState(t('Sessions are stored by LingXi and are not deleted automatically.', '会话由灵犀保存，不会自动删除。'));
  const [confirmed, setConfirmed] = useState(false), [withdrawing, setWithdrawing] = useState<PublishedService>(), [hosts, setHosts] = useState<SharedHostBot[]>([]), [copied, setCopied] = useState('');
  const active = published.filter(service => service.state === 'active');
  useEffect(() => { if (!api.hostStatus || !active.length) return; let closed = false;
    const read = () => void api.hostStatus!().then(status => { if (!closed) setHosts(status.bots); }).catch(() => undefined);
    read(); const timer = setInterval(read, 5000); return () => { closed = true; clearInterval(timer); };
  }, [api, active.length]);
  const hostText = (state?: SharedHostBot['state']) => state === 'ready' ? t('This computer is answering', '此电脑正在应答') : state === 'not_ready' ? t('This computer cannot run the Bot now', '此电脑当前无法运行该 Bot')
    : state === 'unreachable' ? t('This computer cannot reach LingXi', '此电脑无法连接灵犀') : state === 'stopped' ? t('Not served from this computer', '未由此电脑提供') : t('Checking…', '检查中…');
  const offer = () => attempt(async () => {
    const bot = local?.find(value => value.instanceId === instanceId); if (!bot || !confirmed) throw new Error('PUBLICATION_CONFIRMATION_REQUIRED');
    const serviceId = newServiceId(), sharedBotId = crypto.randomUUID();
    await api.services({ action: 'createService', serviceId, sharedBotId, name: name.trim(), description: description.trim(),
      audience: everyone ? { mode: 'public' } : { mode: 'users', uids }, limits, retention: retention.trim() });
    try {
      await api.publishLocalBot!({ teamId: serviceId, generation: 1, publicationId: crypto.randomUUID(), sharedBotId, instanceId, expectedRevision: 0, usePolicy: { mode: 'all' }, fileTokens: [],
        quotas: { team: daily.service, member: daily.user, bot: daily.service }, role });
    } catch (error) {
      // A service with no Bot behind it can answer nobody: it is taken back, and the reason is shown.
      await api.services({ action: 'withdrawService', serviceId }).catch(() => undefined); await onChanged().catch(() => undefined); throw error;
    }
    setInstanceId(''); setName(''); setDescription(''); setRole(''); setConfirmed(false); setUids([]); await onChanged();
  });
  const number = (value: number, min: number, max: number, change: (next: number) => void, label: string) =>
    <label>{label}<input type="number" required min={min} max={max} value={value} onChange={event => change(Number(event.target.value))} /></label>;
  return <>
    <h3>{t('Services you offer', '你提供的服务')}</h3>
    {!active.length ? <p className="teams-hint">{t('None yet.', '暂无。')}</p> : null}
    <ul className="team-bot-grid">{active.map(service => { const host = hosts.find(bot => bot.teamId === service.serviceId), address = link?.(service.serviceId) ?? service.serviceId;
      return <li key={service.serviceId} className="teams-card">
        <header><b>{service.name}</b><span className="team-bot-presence" data-online={service.online && host?.state === 'ready'}>{service.online ? t('On', '已开启') : t('Off', '已关闭')}</span></header>
        <p>{service.description}</p>
        <p className="teams-hint">{hostText(host?.state)}{host?.running ? ` · ${t(`${host.running} answering now`, `${host.running} 个请求进行中`)}` : ''}</p>
        <p className="teams-hint">{service.audience.mode === 'public' ? t('Anyone with the link can use it.', '任何拥有链接的人都可以使用。') : t(`Offered to ${service.audience.uids.length} people you chose.`, `仅向你选择的 ${service.audience.uids.length} 人开放。`)}
          {' '}{t(`At once: ${service.limits.userConcurrent} per person, ${service.limits.serviceConcurrent} in all.`, `同时进行：每人 ${service.limits.userConcurrent} 个，总计 ${service.limits.serviceConcurrent} 个。`)}</p>
        <p className="teams-hint"><code>{address}</code></p>
        <div className="teams-row-actions">
          <button disabled={busy} onClick={() => void attempt(async () => { await api.services({ action: 'setServiceOnline', serviceId: service.serviceId, online: !service.online }); await onChanged(); })}>{service.online ? t('Turn off', '关闭') : t('Turn on', '开启')}</button>
          <button disabled={busy} onClick={() => void attempt(async () => { await navigator.clipboard.writeText(address); setCopied(service.serviceId); })}>{copied === service.serviceId ? t('Copied', '已复制') : t('Copy link', '复制链接')}</button>
          <button disabled={busy} className="tw-danger-btn" onClick={() => setWithdrawing(service)}>{t('Withdraw', '下线')}</button>
        </div></li>; })}</ul>
    <details><summary>{t('Offer a Bot outside your Teams', '在团队之外提供 Bot')}</summary>
      <button disabled={busy} onClick={() => void attempt(async () => setLocal(await api.localBots!()))}>{t('Load local Bots', '加载本机 Bot')}</button>
      {local ? <form className="tw-form" onSubmit={event => { event.preventDefault(); void offer(); }}>
        <label>{t('Local Bot and model', '本机 Bot 与模型')}<select required disabled={busy} value={instanceId} onChange={event => { const bot = local.find(value => value.instanceId === event.target.value);
          setInstanceId(event.target.value); setName(bot?.name ?? ''); setDescription(bot?.description ?? ''); setConfirmed(false); }}>
          <option value="">{t('Choose a Bot', '选择 Bot')}</option>{local.filter(bot => !bot.requiredResources?.length).map(bot => <option key={bot.instanceId} value={bot.instanceId}>{bot.name}{bot.model ? ` · ${bot.model}` : ''}</option>)}</select></label>
        {local.some(bot => bot.requiredResources?.length) ? <p className="teams-hint">{t('Bots that need files bound to them cannot be offered as a service yet.', '需要绑定文件的 Bot 暂不能作为服务提供。')}</p> : null}
        <label>{t('Name people see', '对外显示的名称')}<input required maxLength={80} value={name} onChange={event => setName(event.target.value)} /></label>
        <label>{t('What it does', '用途说明')}<textarea maxLength={500} value={description} onChange={event => setDescription(event.target.value)} /></label>
        <label>{t('Published role', '发布的角色')}<textarea required maxLength={12000} value={role} onChange={event => setRole(event.target.value)} /></label>
        <p className="teams-hint">{t('Only this role is used. The Bot’s private memory, its conversations with you and your files are not part of the service.', '仅使用此处填写的角色。Bot 的私人记忆、与你的会话以及你的文件都不属于服务内容。')}</p>
        {people?.length ? <><label>{t('Who can use it', '谁可以使用')}<select value={everyone ? 'public' : 'users'} onChange={event => setEveryone(event.target.value === 'public')}>
          <option value="users">{t('Only friends I choose', '仅我选择的好友')}</option><option value="public">{t('Anyone with the link', '任何拥有链接的人')}</option></select></label>
          {!everyone ? <fieldset><legend>{t('Friends', '好友')}</legend>{people.map(person => <label key={person.uid} className="team-party"><input type="checkbox" checked={uids.includes(person.uid)}
            onChange={event => setUids(old => event.target.checked ? [...old, person.uid] : old.filter(value => value !== person.uid))} />{person.name}</label>)}</fieldset> : null}</>
          : <p className="teams-hint">{t('Anyone with the link can use it.', '任何拥有链接的人都可以使用。')}</p>}
        <fieldset><legend>{t('Requests a day (UTC)', '每日请求数（UTC）')}</legend>
          {number(daily.user, 1, 10000, user => setDaily(old => ({ ...old, user })), t('Each person', '每人'))}{number(daily.service, 1, 10000, service => setDaily(old => ({ ...old, service })), t('Everyone together', '所有人合计'))}</fieldset>
        <fieldset><legend>{t('Requests at once', '同时进行的请求数')}</legend>
          {number(limits.userConcurrent, 1, 5, userConcurrent => setLimits(old => ({ ...old, userConcurrent })), t('Each person', '每人'))}{number(limits.serviceConcurrent, 1, 50, serviceConcurrent => setLimits(old => ({ ...old, serviceConcurrent })), t('Everyone together', '所有人合计'))}</fieldset>
        <label>{t('What you keep of people’s sessions, in your words', '你如何保留他人的会话（用你自己的话说明）')}<textarea required maxLength={500} value={retention} onChange={event => setRetention(event.target.value)} /></label>
        <p className="teams-hint">{t('People see this before they agree to use the service.', '人们在同意使用该服务前会看到这段说明。')}</p>
        <label className="team-party"><input type="checkbox" checked={confirmed} onChange={event => setConfirmed(event.target.checked)} />
          {t('This computer answers every request and I pay its model costs. I can read every session, and people are told so. The service stays off until I turn it on.', '此电脑回答所有请求，模型费用由我承担。我可以阅读所有会话，使用者会被告知这一点。在我开启之前，服务保持关闭。')}</label>
        <button disabled={busy || !instanceId || !name.trim() || !role.trim() || !retention.trim() || !confirmed || (!everyone && !uids.length)}>{t('Create the service', '创建服务')}</button>
      </form> : null}
    </details>
    {withdrawing ? <Dialog title={t(`Withdraw ${withdrawing.name}?`, `下线 ${withdrawing.name}？`)} role="alertdialog" onClose={() => setWithdrawing(undefined)}>
      <p>{t('This cannot be undone. Nobody can use the service or read its sessions afterwards, and its link stops working.', '此操作无法撤销。之后任何人都无法使用该服务或阅读其会话，链接也将失效。')}</p>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setWithdrawing(undefined)}>{t('Cancel', '取消')}</button>
        <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void attempt(async () => { await api.services({ action: 'withdrawService', serviceId: withdrawing.serviceId }); setWithdrawing(undefined); await onChanged(); })}>{t('Withdraw', '下线')}</button></div>
    </Dialog> : null}
  </>;
}
