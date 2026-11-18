import {SourceNoticePreview} from '../inbox/SourceNoticePreview';
import type {InboxNoticeTarget} from '../inbox/contracts';
import { observeTranscript, VisibleReadFrontier } from '../inbox/visibility.js';
import { useCallback, useContext, useEffect, useRef, useState, type ReactElement } from 'react';
import { ComposerEditor, ComposerSurface, ComposerToolbar } from '../chat-ui/ComposerSurface';
import { ChatConnectionStatus, ChatTopbarHeader, ChatTopbarTarget } from './ChatTopbar';
import type { LocalPublishableBot, SharedHostBot, TeamTaskView } from './execution.js';
import { availabilityText, newServiceId, serviceCommand, serviceErrorText, serviceIdFrom, type HostedServicesApi, type PublishedService,
  type ServiceList, type ServiceMessage, type ServiceReportReason, type ServiceScope, type ServiceSession, type ServiceSync, type ServiceView } from './hosted-service.js';
import { Avatar, Dialog, Icon, PopupMenu, type RenderMarkdown } from './ui';
import { intlTag, useTeamsText } from './i18n.js';

/**
 * Bots offered outside Teams (hosted Bot services), for the person who uses one and, on a computer that hosts Bots, for
 * the person who offers one. A service is not a Team: nothing here reads a Team, and a caller sees one Bot, their own
 * sessions with it, and what its publisher said about cost, visibility and retention before they agreed to enter.
 */
export interface HostedServicesProps {
  notificationTarget?:InboxNoticeTarget;
  api: HostedServicesApi; uid: string; locale: string;
  /** Renders the Bot's answers as Markdown, with the same renderer the Team chat uses. What a person types stays plain text. */
  renderMarkdown?: RenderMarkdown;
  /** A service to look up at once: the one a link named. */
  initialServiceId?: string;
  initialSessionId?: string;
  /** The link a publisher gives to people, for a service id. Without it the id itself is shown. */
  link?(serviceId: string): string;
  /** People the publisher can offer a service to by name (their friends). Without it a service is offered to anyone with the link. */
  people?: ReadonlyArray<{ uid: string; name: string }>;
  onBackToTeams?(): void;
  /** How often an open session is read again, in milliseconds: while the Bot is answering, and otherwise. */
  refreshMs?: { answering: number; idle: number };
}
const code = (error: unknown) => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';
const LIVE = ['queued', 'running', 'waiting'];

export function HostedServices({ notificationTarget,api, uid, locale, renderMarkdown, initialServiceId, initialSessionId, link, people, onBackToTeams, refreshMs = { answering: 2000, idle: 10000 } }: HostedServicesProps): ReactElement {
  const t = useTeamsText(locale);
  const hasTopbar = useContext(ChatTopbarTarget) !== undefined;
  const [list, setList] = useState<ServiceList>(), [status, setStatus] = useState(''), [busy, setBusy] = useState(false);
  const [lookup, setLookup] = useState(''), [candidate, setCandidate] = useState<ServiceView>(), [open, setOpen] = useState<ServiceView>();
  const attempt = useCallback(async (action: () => Promise<unknown>) => {
    setBusy(true); setStatus('');
    try { await action(); } catch (error) { setStatus(serviceErrorText(code(error), locale)); } finally { setBusy(false); }
  }, [locale]);
  const refresh = useCallback(async () => setList(await api.services<ServiceList>({ action: 'listServices' })), [api]);
  const find = useCallback(async (serviceId: string) => {
    const view = await api.services<ServiceView | PublishedService>({ action: 'getService', serviceId });
    // The publisher is told what they set, not a caller's terms: they do not use their own service as a caller.
    if ('publisher' in view) { setCandidate(undefined); setOpen(undefined);
      setStatus(t('services_own')); return; }
    // Looking a service up shows what it is. Nothing is entered and nothing is sent to its Bot.
    if (view.entered) { setCandidate(undefined); setOpen(view); } else { setOpen(undefined); setCandidate(view); }
  }, [api, locale]);
  useEffect(() => { void attempt(async () => { await refresh(); if (initialServiceId) await find(initialServiceId); }); }, [attempt, refresh, find, initialServiceId]);
  const [dismissed,setDismissed]=useState('');
  const token=notificationTarget?JSON.stringify([notificationTarget.key,notificationTarget.generation,notificationTarget.through,notificationTarget.messageId,notificationTarget.taskId]):'';
  if(open && initialSessionId && open.serviceId===initialServiceId && notificationTarget && token!==dismissed)return <SourceNoticePreview api={api} kind="service" sourceId={open.serviceId} generation={open.generation??1} conversationId={initialSessionId} locale={locale} target={notificationTarget} onReturn={()=>setDismissed(token)}/>;
  if (open) return <ServiceSessions key={open.serviceId} api={api} uid={uid} locale={locale} service={open} initialSessionId={initialServiceId === open.serviceId ? initialSessionId : undefined} renderMarkdown={renderMarkdown} refreshMs={refreshMs}
    onBack={() => { setOpen(undefined); void attempt(refresh); }} onLeft={() => { setOpen(undefined); void attempt(refresh); }} />;
  return <section className="tw-scope tw-embed hosted-services" aria-label={t('services_title')}>
    {onBackToTeams ? <ChatTopbarHeader><header className="tw-conv-head">
      <button type="button" className="tw-icon-btn" aria-label={t('tw_back_team')} title={t('tw_back_team')} onClick={onBackToTeams}><Icon name="chevronLeft" /></button>
      <div className="tw-conv-title"><h2>{t('services_title')}</h2></div>
    </header></ChatTopbarHeader> : !hasTopbar ? <h2>{t('services_title')}</h2> : null}
    <p className="teams-hint">{t('services_intro')}</p>
    {status ? <p role="alert">{status}</p> : null}
    <form className="tw-form" onSubmit={event => { event.preventDefault(); const serviceId = serviceIdFrom(lookup);
      if (!serviceId) { setStatus(t('services_lookup_invalid')); return; } void attempt(() => find(serviceId)); }}>
      <label>{t('services_lookup_label')}<input value={lookup} onChange={event => setLookup(event.target.value)} placeholder="https://…/services/s_…" autoComplete="off" spellCheck={false} /></label>
      <button disabled={busy || !lookup.trim()}>{t('services_lookup_action')}</button>
    </form>
    {candidate ? <ServiceTerms service={candidate} locale={locale} busy={busy} onCancel={() => setCandidate(undefined)}
      onEnter={() => void attempt(async () => { const entered = await api.services<{ generation: number; service: ServiceView }>({ action: 'enterService', serviceId: candidate.serviceId, acknowledged: true });
        setCandidate(undefined); setOpen({ ...entered.service, entered: true, generation: entered.generation }); })} /> : null}
    <h3>{t('services_used')}</h3>
    {list && !list.entered.length ? <p className="teams-hint">{t('services_none')}</p> : null}
    <ul className="team-bot-grid">{list?.entered.map(service => <li key={service.serviceId} className="teams-card">
      <header><b>{service.name}</b><span className="team-bot-presence" data-online={service.availability === 'available'}>{availabilityText(service.availability, locale)}</span></header>
      <p>{service.description}</p><small>{t('tw_service_offered_by')}: {service.publisherName}</small>
      <div className="teams-row-actions"><button disabled={busy} onClick={() => setOpen(service)}>{t('common_open')}</button></div>
    </li>)}</ul>
    {api.localBots && api.publishLocalBot ? <ServicePublisher api={api} locale={locale} busy={busy} attempt={attempt} published={list?.published ?? []} onChanged={refresh} link={link} people={people} /> : null}
  </section>;
}

/** What a person is told before they enter: who offers the Bot, who pays, who can read the session, what is kept, how much they may ask. */
function ServiceTerms({ service, locale, busy, onEnter, onCancel }: { service: ServiceView; locale: string; busy: boolean; onEnter(): void; onCancel(): void }): ReactElement {
  const t = useTeamsText(locale), who = service.publisherName;
  return <Dialog title={service.name} onClose={onCancel}><div className="tw-form">
    <p>{service.description}</p>
    <p>{t('tw_service_scope_fmt', who, who)}</p>
    <ul>
      <li>{t('tw_service_readers_fmt', who, who)}</li>
      <li>{t('services_terms_cost_fmt', who)}</li>
      <li>{t('services_terms_retention')}: {service.retention}</li>
      <li>{service.limits.userDaily !== undefined ? t('services_terms_daily_fmt', service.limits.userDaily, service.limits.userConcurrent)
        : t('services_terms_concurrent_fmt', service.limits.userConcurrent)}</li>
      <li>{t('services_terms_reach')}</li>
    </ul>
    <p className="teams-hint">{availabilityText(service.availability, locale)}</p>
    <div className="tw-dialog-actions"><button type="button" onClick={onCancel}>{t('services_terms_decline')}</button>
      <button type="button" data-autofocus disabled={busy || service.availability === 'withdrawn'} onClick={onEnter}>{t('services_terms_agree')}</button></div>
  </div></Dialog>;
}

/** A caller's sessions with a service's Bot. Everything is read through the API on a timer: a service's records cannot be watched. */
function ServiceSessions({ api, uid, locale, service, initialSessionId, renderMarkdown, refreshMs, onBack, onLeft }: {
  api: HostedServicesApi; uid: string; locale: string; service: ServiceView; initialSessionId?:string; renderMarkdown?: RenderMarkdown; refreshMs: { answering: number; idle: number }; onBack(): void; onLeft(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const scope: ServiceScope = { serviceId: service.serviceId, generation: service.generation ?? 1 }, scopeKey = `${scope.serviceId}:${scope.generation}`;
  const [view, setView] = useState(service), [sessions, setSessions] = useState<ServiceSession[]>(), [sessionId, setSessionId] = useState<string|undefined>(initialSessionId);
  const [messages, setMessages] = useState<ServiceMessage[]>([]), [tasks, setTasks] = useState<TeamTaskView[]>([]);
  const [text, setText] = useState(''), [status, setStatus] = useState(''), [busy, setBusy] = useState(false);
  const [syncError, setSyncError] = useState('');
  const [reporting, setReporting] = useState(false), [leaving, setLeaving] = useState(false), [tick, setTick] = useState(0);
  const [menu, setMenu] = useState(false), menuRef = useRef<HTMLButtonElement>(null);
  const scroller=useRef<HTMLDivElement>(null);
  const [older,setOlder]=useState<number|null>(null);
  const cursor = useRef({ historical:false, initialized:false,verifiedAt:0,key: '', after: 0, byId: new Map<string, ServiceMessage>() }), end = useRef<HTMLLIElement>(null);
  const attempt = async (action: () => Promise<unknown>) => {
    setBusy(true); setStatus('');
    try { await action(); return true; } catch (error) { setStatus(serviceErrorText(code(error), locale)); return false; } finally { setBusy(false); }
  };
  async function loadOlder(){
    const at=cursor.current;
    if(!sessionId || !older || busy)return;
    await attempt(async()=>{
      const page=await api.serviceMessages<{messages:ServiceMessage[];next:number|null}>({action:'history',...scope,conversationId:sessionId,before:older});
      if(cursor.current!==at)return;
      const byId=new Map([...page.messages,...at.byId.values()].map(m=>[m.id,m]));
      const values=[...byId.values()].sort((a,b)=>a.seq-b.seq);
      if(values.length>200)at.historical=true;
      const visible=values.slice(0,200);at.byId=new Map(visible.map(m=>[m.id,m]));
      setMessages(visible);setOlder(page.next);
    });
  }
  const currentRead=sessions?.find(s=>s.id===sessionId)?.lastReadSeq;
  useEffect(()=>{
    if(!sessionId || currentRead===undefined || !scroller.current)return;
    const frontier=new VisibleReadFrontier(currentRead,0,(through,revision)=>{
      const read=api.readVisible?.(scope,sessionId,through,revision) ?? api.serviceMessages({action:'read',...scope,conversationId:sessionId,lastReadSeq:through});
      void read.catch(e=>setStatus(serviceErrorText(code(e),locale)));
    });
    return observeTranscript(scroller.current,frontier);
  },[api,scopeKey,sessionId,currentRead]);
  const loadSessions = useCallback(async () => {
    const found = (await api.serviceMessages<{ conversations: ServiceSession[] }>({ action: 'list', ...scope })).conversations;
    setSessions(found); return found;
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, scopeKey]);
  useEffect(() => { void attempt(async () => { const found = await loadSessions(); setSessionId(old => old ?? found.at(-1)?.id); }); /* eslint-disable-next-line react-hooks/exhaustive-deps */ }, [loadSessions]);
  // The open session: its messages by the change log, its tasks, and whether the service can answer.
  const live = tasks.some(task => LIVE.includes(task.state));
  useEffect(() => {
    if (!sessionId) { setMessages([]); setTasks([]); setSyncError(''); return; }
    let closed = false, running = false;
    const key = `${scopeKey}:${sessionId}`;
    if (cursor.current.key !== key) { cursor.current = { key, historical:false, initialized:false,verifiedAt:0,after: 0, byId: new Map() };setOlder(null); setMessages([]); setTasks([]); setSyncError(''); }
    const pass = async () => {
      if (closed || running || (typeof document !== 'undefined' && document.visibilityState === 'hidden')) return;
      running = true;
      try {
        const at = cursor.current;
        async function history() {
          const previous=[...at.byId.values()].sort((a,b)=>a.seq-b.seq);
          const recent=at.historical && previous.length ? previous[previous.length-1]!.seq+1 : undefined;
          let before=recent,remaining=Math.max(1,Math.ceil(previous.length/50)), high:number|undefined;
          const rebased=new Map<string,ServiceMessage>();
          do {
            const page=await api.serviceMessages<{messages:ServiceMessage[];next:number|null;lastChangeSeq:number}>({action:'history',...scope,conversationId:sessionId,...(before?{before}:{})});
            if(closed || at!==cursor.current)return;
            high??=page.lastChangeSeq;
            for(const m of page.messages)rebased.set(m.id,m);
            before=page.next??undefined;
          }while(before && --remaining>0);
          at.byId=rebased;at.after=high??0;at.initialized=true;at.verifiedAt=Date.now();setOlder(before??null);
        }
        if(!at.initialized || Date.now()-at.verifiedAt>30*86400000) await history();
        for (;;) {
          if(closed || cursor.current!==at)return;
          const page = await api.serviceMessages<ServiceSync>({ action: 'sync', ...scope, conversationId: sessionId, after: at.after,limit:50 });
          if (closed || cursor.current !== at) return;
          if(page.conversation.lastChangeSeq-at.after>1000 || page.changes.some((c,i)=>c.seq!==at.after+i+1)) {await history();break;}
          const tail=Math.max(0,...[...at.byId.values()].map(m=>m.seq));
          for (const message of page.messages) if(!at.historical || at.byId.has(message.id) || message.seq<=tail && message.seq>=(messages[0]?.seq??0)) at.byId.set(message.id, message);
          const sorted=[...at.byId.values()].sort((a,b)=>a.seq-b.seq);
          if(sorted.length>200){at.byId=new Map(sorted.slice(-200).map(m=>[m.id,m]));setOlder(sorted.at(-200)!.seq);}
          at.after += page.changes.length;at.verifiedAt=Date.now();
          if (!page.hasMore || !page.changes.length) break;
        }
        const listed = await api.serviceMessages<{ tasks: TeamTaskView[] }>(serviceCommand(scope, 'listTasks', { conversationId: sessionId }));
        const now = await api.services<ServiceView>({ action: 'getService', serviceId: scope.serviceId });
        if (closed || cursor.current !== at) return;
        setMessages([...at.byId.values()].sort((a, b) => a.seq - b.seq)); setTasks(listed.tasks.sort((a, b) => a.createdAt - b.createdAt)); setView(now);
        setSyncError('');
      } catch (error) { if (!closed) setSyncError(serviceErrorText(code(error), locale)); } finally { running = false; }
    };
    void pass();
    // Quick while the Bot is answering, slow otherwise.
    const timer = setInterval(() => void pass(), live ? refreshMs.answering : refreshMs.idle);
    return () => { closed = true; clearInterval(timer); };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [api, scopeKey, sessionId, live, locale, tick, refreshMs.answering, refreshMs.idle]);
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [messages.length, tasks.length]);
  const waiting = tasks.find(task => task.state === 'waiting' && task.wait?.recipient.uid === uid);
  const newSession = () => attempt(async () => {
    const conversationId = crypto.randomUUID();
    await api.serviceMessages(serviceCommand(scope, 'createConversation', { conversationId, kind: 'bot_dm', title: new Date().toLocaleString(intlTag(locale)), participantUids: [uid], sharedBotIds: [service.sharedBotId] }));
    await loadSessions(); setSessionId(conversationId);
  });
  const send = () => attempt(async () => {
    const body = text.trim(); if (!body || busy || view.availability === 'withdrawn') return;
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
  const ended = (task: TeamTaskView) => ({ failed: t('services_task_failed'), expired: t('services_task_expired'),
    interrupted: t('services_task_interrupted'), revoked: t('services_task_revoked'),
    cancelled: t('services_task_cancelled'), authorization_required: t('services_task_authorization_required'),
    needs_reconciliation: t('services_task_needs_reconciliation') } as Record<string, string>)[task.state];
  const off = view.availability !== 'available';
  const availability = availabilityText(view.availability, locale);
  return <section className="tw-scope tw-conv hosted-service" aria-label={view.name}>
    <ChatTopbarHeader><header className="tw-conv-head hosted-service-header">
      <button type="button" className="tw-icon-btn" aria-label={t('tw_services_back')} title={t('tw_services_back')} onClick={onBack}><Icon name="chevronLeft" /></button>
      <div className="tw-conv-identity">
        <Avatar identity={{ kind: 'bot', id: view.sharedBotId, name: view.name }} size={32} />
        <div className="tw-conv-title"><h2>{view.name}</h2><p>{t('tw_service_offered_by')} {view.publisherName}</p></div>
      </div>
      <div className="tw-head-actions">
        <ChatConnectionStatus kind={syncError ? 'error' : off ? 'offline' : 'ok'}
          label={syncError ? t('teams_load_failed') : availability} detail={syncError || (off && view.availability !== 'withdrawn' ? t('services_cannot_answer') : availability)}
          recovery={syncError && sessionId ? { label: t('common_retry'), run: () => setTick(value => value + 1) } : undefined} />
        <select aria-label={t('tw_service_session')} value={sessionId ?? ''} onChange={event => setSessionId(event.target.value || undefined)}>
          {!sessions?.length ? <option value="">{t('services_new_session')}</option> : null}
          {sessions?.map(session => <option key={session.id} value={session.id}>{session.title}</option>)}
        </select>
        <button type="button" className="tw-icon-btn" aria-label={t('services_new_session')} title={t('services_new_session')} disabled={busy} onClick={() => void newSession()}><Icon name="plus" /></button>
        <div className="tw-switcher-wrap tw-friends-more">
          <button ref={menuRef} type="button" className="tw-icon-btn" aria-label={t('tw_more')} title={t('tw_more')} aria-haspopup="menu" aria-expanded={menu} onClick={() => setMenu(open => !open)}><Icon name="more" /></button>
          {menu ? <PopupMenu label={view.name} items={[
            { id: 'report', label: t('services_report'), onSelect: () => setReporting(true) },
            { id: 'leave', label: t('services_leave'), danger: true, separated: true, onSelect: () => setLeaving(true) },
          ]} anchor={menuRef} align="end" onClose={restore => { setMenu(false); if (restore) menuRef.current?.focus(); }} /> : null}
        </div>
      </div>
    </header></ChatTopbarHeader>
    <p className="hosted-service-note">{t('services_note_fmt', view.publisherName)}
      {view.limits.userDailyLeft !== undefined ? ` ${t('services_left_today_fmt', view.limits.userDailyLeft)}` : ''}</p>
    <div className="tw-stream-wrap tw-chat-thread"><div ref={scroller} className="tw-stream" role="log" aria-label={t('tw_messages')} tabIndex={0}>
      {older?<button type="button" disabled={busy} onClick={()=>void loadOlder()}>{t('teams_messages_earlier')}</button>:null}
      {cursor.current.historical?<button type="button" onClick={()=>{cursor.current.key='';setTick(n=>n+1);}}>{t('tw_conv_latest')}</button>:null}
      {!messages.length && !tasks.length ? <p className="tw-empty-line" role="status">{t('services_empty')}</p> : null}
      <ol className="tw-messages">
      {messages.map(message => { const bot = message.author.kind === 'shared_bot';
        return <li key={message.id} className="tw-msg" data-grouped="false" data-mine={!bot} data-kind={bot ? 'bot' : 'user'} data-message-id={message.id} data-message-seq={message.seq}>
          {bot ? <div className="tw-msg-lead"><Avatar identity={{ kind: 'bot', id: view.sharedBotId, name: view.name }} size={32} /></div> : null}
          <div className={bot ? 'tw-msg-main' : 'tw-msg-main tw-msg-bubble'}>
            <div className="tw-msg-head"><b className="tw-msg-author">{bot ? view.name : t('teams_you')}</b>{bot ? <span className="tw-tag">{t('teams_bot_tag')}</span> : null}
              {message.waitId && bot ? <span className="tw-tag">{t('services_question')}</span> : null}
              {message.createdAt ? <time className="tw-msg-time" dateTime={message.createdAt}>{new Date(message.createdAt).toLocaleTimeString(intlTag(locale), { hour: '2-digit', minute: '2-digit' })}</time> : null}</div>
            {bot && renderMarkdown && !message.recalled ? <div className="tw-msg-body tw-md" data-markdown="true">{renderMarkdown(message.text)}</div>
              : <p className="tw-msg-body">{message.recalled ? <em>{t('teams_message_recalled')}</em> : message.text}</p>}
          </div></li>; })}
      {tasks.filter(task => task.state !== 'completed').map(task => <li key={task.taskId} className="tw-msg hosted-service-task" data-state={task.state} role="status">
        <div className="tw-msg-lead" /><div className="tw-msg-main"><p className="tw-msg-body hosted-service-note">
          {task.state === 'queued' ? t('services_task_queued') : task.state === 'running' ? t('services_task_running')
            : task.state === 'waiting' ? (task.wait?.kind === 'tool_approval' ? t('services_task_approval') : t('services_task_question')) : ended(task)}
          {task.state === 'waiting' && task.wait?.kind === 'tool_approval' && task.wait.recipient.uid === uid ? <> <button type="button" disabled={busy} onClick={() => void decide('approve')}>{t('teams_approval_approve')}</button> <button type="button" disabled={busy} onClick={() => void decide('deny')}>{t('teams_approval_decline')}</button></> : null}
          {LIVE.includes(task.state) ? <> <button type="button" disabled={busy || !!task.cancelRequestedAt} onClick={() => void attempt(async () => { await api.serviceMessages(serviceCommand(scope, 'cancelTask', { taskId: task.taskId })); setTick(value => value + 1); })}>{t('common_cancel')}</button></> : null}
        </p></div></li>)}
      <li ref={end} aria-hidden="true" />
      </ol></div></div>
    {status ? <p role="alert" className="hosted-service-note">{status}</p> : null}
    <form className="tw-composer hosted-service-composer" onSubmit={event => { event.preventDefault(); void send(); }}>
      <ComposerSurface className="tw-compose-box">
        <ComposerEditor><textarea className="composer-editor-control tw-message-box" aria-label={t('tw_composer_message')} value={text} maxLength={8000} disabled={view.availability === 'withdrawn'} placeholder={waiting?.wait?.kind === 'human_input' ? t('services_answer_hint') : t('services_ask_hint_fmt', view.name)}
          onChange={event => setText(event.target.value)} onKeyDown={event => {
            if (event.key !== 'Enter' || event.shiftKey || event.nativeEvent.isComposing || event.keyCode === 229) return;
            event.preventDefault(); void send();
          }} /></ComposerEditor>
        <ComposerToolbar><button className="tw-primary tw-send" type="submit" aria-label={t('teams_message_send')} disabled={busy || !text.trim() || view.availability === 'withdrawn'}><Icon name="arrowUp" /></button></ComposerToolbar>
      </ComposerSurface>
    </form>
    {reporting ? <ServiceReport api={api} locale={locale} serviceId={scope.serviceId} sessionId={sessionId} onClose={() => setReporting(false)} /> : null}
    {leaving ? <Dialog title={t('services_leave_title_fmt', view.name)} role="alertdialog" onClose={() => setLeaving(false)}>
      <p>{t('services_leave_body')}</p>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setLeaving(false)}>{t('common_cancel')}</button>
        <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void attempt(async () => { await api.services({ action: 'leaveService', serviceId: scope.serviceId }); onLeft(); })}>{t('services_leave')}</button></div>
    </Dialog> : null}
  </section>;
}

/** A report goes to LingXi, not to the publisher. */
function ServiceReport({ api, locale, serviceId, sessionId, onClose }: { api: HostedServicesApi; locale: string; serviceId: string; sessionId?: string; onClose(): void }): ReactElement {
  const t = useTeamsText(locale);
  const [reason, setReason] = useState<ServiceReportReason>('harmful'), [text, setText] = useState(''), [withSession, setWithSession] = useState(false);
  const [state, setState] = useState<{ busy?: boolean; error?: string; sent?: boolean }>({}), reportId = useRef(crypto.randomUUID());
  const send = async () => {
    setState({ busy: true });
    try { await api.services({ action: 'reportService', serviceId, reportId: reportId.current, reason, text: text.trim(), ...(withSession && sessionId ? { sessionId } : {}) }); setState({ sent: true }); }
    catch (error) { setState({ error: serviceErrorText(code(error), locale) }); }
  };
  return <Dialog title={t('services_report_title')} onClose={onClose}>{state.sent
    ? <div className="tw-form"><p role="status">{t('services_report_sent')}</p><div className="tw-dialog-actions"><button type="button" data-autofocus onClick={onClose}>{t('common_close')}</button></div></div>
    : <form className="tw-form" onSubmit={event => { event.preventDefault(); void send(); }}>
      <p className="teams-hint">{t('services_report_hint')}</p>
      <label>{t('services_report_reason')}<select value={reason} onChange={event => setReason(event.target.value as ServiceReportReason)}>
        <option value="harmful">{t('services_report_harmful')}</option><option value="deceptive">{t('services_report_deceptive')}</option>
        <option value="privacy">{t('services_report_privacy')}</option><option value="spam">{t('services_report_spam')}</option><option value="other">{t('services_report_other')}</option></select></label>
      <label>{t('services_report_text')}<textarea maxLength={1000} value={text} onChange={event => setText(event.target.value)} /></label>
      {sessionId ? <label className="team-party"><input type="checkbox" checked={withSession} onChange={event => setWithSession(event.target.checked)} />{t('services_report_session')}</label> : null}
      {state.error ? <p role="alert">{state.error}</p> : null}
      <div className="tw-dialog-actions"><button type="button" onClick={onClose}>{t('common_cancel')}</button><button disabled={state.busy}>{t('services_report_send')}</button></div>
    </form>}</Dialog>;
}

/** The publisher's side, on the computer that hosts the Bot: offer a Bot, switch a service on and off, withdraw it. */
function ServicePublisher({ api, locale, busy, attempt, published, onChanged, link, people }: {
  api: HostedServicesApi; locale: string; busy: boolean; attempt(action: () => Promise<unknown>): Promise<void>; published: PublishedService[]; onChanged(): Promise<void>;
  link?(serviceId: string): string; people?: ReadonlyArray<{ uid: string; name: string }>;
}): ReactElement {
  const t = useTeamsText(locale);
  const [local, setLocal] = useState<LocalPublishableBot[]>(), [instanceId, setInstanceId] = useState(''), [name, setName] = useState(''), [description, setDescription] = useState('');
  const [role, setRole] = useState(''), [everyone, setEveryone] = useState(!people?.length), [uids, setUids] = useState<string[]>([]);
  const [daily, setDaily] = useState({ user: 20, service: 200 }), [limits, setLimits] = useState({ userConcurrent: 1, serviceConcurrent: 4 });
  const [retention, setRetention] = useState(t('tw_service_sessions_note'));
  const [confirmed, setConfirmed] = useState(false), [withdrawing, setWithdrawing] = useState<PublishedService>(), [hosts, setHosts] = useState<SharedHostBot[]>([]), [copied, setCopied] = useState('');
  const active = published.filter(service => service.state === 'active');
  useEffect(() => { if (!api.hostStatus || !active.length) return; let closed = false;
    const read = () => void api.hostStatus!().then(status => { if (!closed) setHosts(status.bots); }).catch(() => undefined);
    read(); const timer = setInterval(read, 5000); return () => { closed = true; clearInterval(timer); };
  }, [api, active.length]);
  const hostText = (state?: SharedHostBot['state']) => state === 'ready' ? t('tw_service_state_answering') : state === 'not_ready' ? t('tw_service_state_cannot_run')
    : state === 'unreachable' ? t('tw_service_state_offline') : state === 'stopped' ? t('tw_service_state_not_served') : t('tw_checking');
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
    <h3>{t('tw_service_yours')}</h3>
    {!active.length ? <p className="teams-hint">{t('services_none')}</p> : null}
    <ul className="team-bot-grid">{active.map(service => { const host = hosts.find(bot => bot.teamId === service.serviceId), address = link?.(service.serviceId) ?? service.serviceId;
      return <li key={service.serviceId} className="teams-card">
        <header><b>{service.name}</b><span className="team-bot-presence" data-online={service.online && host?.state === 'ready'}>{service.online ? t('tw_on') : t('tw_off')}</span></header>
        <p>{service.description}</p>
        <p className="teams-hint">{hostText(host?.state)}{host?.running ? ` · ${t('tw_service_answering_fmt', host.running)}` : ''}</p>
        <p className="teams-hint">{service.audience.mode === 'public' ? t('tw_service_anyone') : t('tw_service_chosen_fmt', service.audience.uids.length)}
          {' '}{t('tw_service_concurrency_fmt', service.limits.userConcurrent, service.limits.serviceConcurrent)}</p>
        <p className="teams-hint"><code>{address}</code></p>
        <div className="teams-row-actions">
          <button disabled={busy} onClick={() => void attempt(async () => { await api.services({ action: 'setServiceOnline', serviceId: service.serviceId, online: !service.online }); await onChanged(); })}>{service.online ? t('tw_turn_off') : t('tw_turn_on')}</button>
          <button disabled={busy} onClick={() => void attempt(async () => { await navigator.clipboard.writeText(address); setCopied(service.serviceId); })}>{copied === service.serviceId ? t('tw_copied') : t('friends_link_copy')}</button>
          <button disabled={busy} className="tw-danger-btn" onClick={() => setWithdrawing(service)}>{t('tw_service_withdraw')}</button>
        </div></li>; })}</ul>
    <details><summary>{t('tw_service_offer_title')}</summary>
      <button disabled={busy} onClick={() => void attempt(async () => setLocal(await api.localBots!()))}>{t('tw_botdir_load_local')}</button>
      {local ? <form className="tw-form" onSubmit={event => { event.preventDefault(); void offer(); }}>
        <label>{t('tw_botdir_local_bot_model')}<select required disabled={busy} value={instanceId} onChange={event => { const bot = local.find(value => value.instanceId === event.target.value);
          setInstanceId(event.target.value); setName(bot?.name ?? ''); setDescription(bot?.description ?? ''); setConfirmed(false); }}>
          <option value="">{t('tw_botdir_choose_bot')}</option>{local.filter(bot => !bot.requiredResources?.length).map(bot => <option key={bot.instanceId} value={bot.instanceId}>{bot.name}{bot.model ? ` · ${bot.model}` : ''}</option>)}</select></label>
        {local.some(bot => bot.requiredResources?.length) ? <p className="teams-hint">{t('tw_service_no_files')}</p> : null}
        <label>{t('tw_service_public_name')}<input required maxLength={80} value={name} onChange={event => setName(event.target.value)} /></label>
        <label>{t('tw_service_what')}<textarea maxLength={500} value={description} onChange={event => setDescription(event.target.value)} /></label>
        <label>{t('tw_botdir_published_role')}<textarea required maxLength={12000} value={role} onChange={event => setRole(event.target.value)} /></label>
        <p className="teams-hint">{t('tw_service_role_hint')}</p>
        {people?.length ? <><label>{t('tw_service_who')}<select value={everyone ? 'public' : 'users'} onChange={event => setEveryone(event.target.value === 'public')}>
          <option value="users">{t('tw_service_friends')}</option><option value="public">{t('tw_service_link')}</option></select></label>
          {!everyone ? <fieldset><legend>{t('friends_title')}</legend>{people.map(person => <label key={person.uid} className="team-party"><input type="checkbox" checked={uids.includes(person.uid)}
            onChange={event => setUids(old => event.target.checked ? [...old, person.uid] : old.filter(value => value !== person.uid))} />{person.name}</label>)}</fieldset> : null}</>
          : <p className="teams-hint">{t('tw_service_anyone')}</p>}
        <fieldset><legend>{t('tw_service_daily')}</legend>
          {number(daily.user, 1, 10000, user => setDaily(old => ({ ...old, user })), t('tw_service_each'))}{number(daily.service, 1, 10000, service => setDaily(old => ({ ...old, service })), t('tw_service_all'))}</fieldset>
        <fieldset><legend>{t('tw_service_at_once')}</legend>
          {number(limits.userConcurrent, 1, 5, userConcurrent => setLimits(old => ({ ...old, userConcurrent })), t('tw_service_each'))}{number(limits.serviceConcurrent, 1, 50, serviceConcurrent => setLimits(old => ({ ...old, serviceConcurrent })), t('tw_service_all'))}</fieldset>
        <label>{t('tw_service_retention')}<textarea required maxLength={500} value={retention} onChange={event => setRetention(event.target.value)} /></label>
        <p className="teams-hint">{t('tw_service_retention_hint')}</p>
        <label className="team-party"><input type="checkbox" checked={confirmed} onChange={event => setConfirmed(event.target.checked)} />
          {t('tw_service_consent')}</label>
        <button disabled={busy || !instanceId || !name.trim() || !role.trim() || !retention.trim() || !confirmed || (!everyone && !uids.length)}>{t('tw_service_create')}</button>
      </form> : null}
    </details>
    {withdrawing ? <Dialog title={t('tw_service_withdraw_title_fmt', withdrawing.name)} role="alertdialog" onClose={() => setWithdrawing(undefined)}>
      <p>{t('tw_service_withdraw_body')}</p>
      <div className="tw-dialog-actions"><button type="button" data-autofocus onClick={() => setWithdrawing(undefined)}>{t('common_cancel')}</button>
        <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => void attempt(async () => { await api.services({ action: 'withdrawService', serviceId: withdrawing.serviceId }); setWithdrawing(undefined); await onChanged(); })}>{t('tw_service_withdraw')}</button></div>
    </Dialog> : null}
  </>;
}
