import {noticeTargetFromSearch} from '../inbox/route';
import {TranscriptBuilder,type Transcript} from './sessionModel';
import { Files } from './Files';
import { SendFailureNotice } from '../../../../packages/chat-ui/SendFailureNotice';
import { ChatTopbarControl } from '../../../../packages/teams/ChatTopbar';
import type { FileReceipt, ImageGetParams } from '@lingxi/bridge-client/remote';
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { Locale } from '../utils/locale';
import { Composer } from './Composer';
import { Controls } from './Controls';
import { requestErrorStringKey } from './failureText';
import { PendingPrompts } from './Prompts';
import { isTeamApproval, knownBots, type RemoteClient, type SessionState } from './remoteClient';
import { TranscriptView } from './Transcript';
import { t } from './strings';
import { displayText } from './text';
import { useStore } from './useStore';
import type { ConnectionState } from './viewerConnection';

const RemoteRead = lazy(() => import('../inbox/RemoteRead').then(module => ({ default: module.RemoteRead })));

interface SessionViewProps {
  locale: Locale;
  client: RemoteClient;
  hostId?:string;
  sessionId: string;
  connection: ConnectionState;
  deviceName: string;
  onBack(): void;
}

function errorText(locale: Locale, state: SessionState): string {
  return t(locale, requestErrorStringKey(state.error?.code ?? ''));
}

export function SessionView({ locale, client, sessionId, hostId, connection, deviceName, onBack }: SessionViewProps) {
  const controller = useMemo(() => client.session(sessionId), [client, sessionId]);
  const state = useStore(controller)!;
  const imageEpoch = state.status === 'live' ? controller.imageEpoch : undefined;
  const fetchImage = useCallback((ref: string, size?: ImageGetParams['size'], signal?: AbortSignal) => {
    if (imageEpoch === undefined) return Promise.reject(new Error('image session unavailable'));
    return controller.fetchImage(ref, size, signal);
  }, [controller, imageEpoch]);
  const list = useStore(client)!;
  // A Bot's chat is in its Bot's list, not in the sessions': the page that opened it said which Bot that is.
  const botId = client.botOfThread(sessionId);
  const botThreads = useMemo(() => (botId === undefined ? null : client.botThreads(botId)), [client, botId]);
  const threads = useStore(botThreads);
  const summary = list.sessions?.find((session) => session.session_id === sessionId) ?? threads?.threads?.find((thread) => thread.session_id === sessionId);
  const botName = knownBots(list).find((bot) => bot.bot_id === botId)?.name;
  const scroller = useRef<HTMLDivElement>(null);
  const [attachments, setAttachments] = useState<FileReceipt[]>([]);
  const target=useMemo(()=>noticeTargetFromSearch(typeof location==='undefined'?'':location.search),[sessionId,hostId]);
  const [timeline,setTimeline]=useState(false);
  const preview=!!target && !timeline;
  const [sendFailure, setSendFailure] = useState<typeof controller | null>(null);
  const observedFailures = useRef<{ controller: typeof controller; ids: Set<string> } | null>(null);
  const stickToBottom = useRef(true);
  useEffect(() => setAttachments([]), [client, sessionId]);

  useEffect(() => {
    controller.activate();
    const report = (): void => controller.setForeground(!preview && document.visibilityState === 'visible');
    report();
    document.addEventListener('visibilitychange', report);
    return () => {
      document.removeEventListener('visibilitychange', report);
      controller.deactivate();
    };
  }, [controller,preview]);

  // Keeps the chat's title current while it is open: the Bot's list is asked for again as the Bot's activity changes.
  useEffect(() => {
    botThreads?.activate();
    return () => botThreads?.deactivate();
  }, [botThreads]);

  useEffect(() => {
    const ids = new Set(state.outgoing.filter((prompt) => prompt.status === 'failed').map((prompt) => prompt.id));
    const previous = observedFailures.current;
    observedFailures.current = { controller, ids };
    if (previous?.controller !== controller) {
      setSendFailure(null);
    } else if ([...ids].some((id) => !previous.ids.has(id))) {
      setSendFailure(controller);
    }
  }, [controller, state.outgoing]);

  // Follow new content while the reader is at the bottom; leave them alone once they scroll up.
  useLayoutEffect(() => {
    const element = scroller.current;
    if (element && stickToBottom.current) element.scrollTop = element.scrollHeight;
  }, [state.transcript, state.outgoing, state.pending]);

  const live = state.status === 'live' && connection.phase === 'ready';
  const approval = isTeamApproval(sessionId);
  const running = state.turn.running || state.transcript.conversation.running;

  if(preview && target)return <RemoteNoticePreview client={client} sessionId={sessionId} target={target} hostId={hostId} live={live} locale={locale} deviceName={deviceName} onReturn={()=>{
    const url=new URL(location.href);
    for(const key of ['through','preview','message','messageId','task','taskId','notificationId'])url.searchParams.delete(key);
    history.replaceState(history.state,'',url);setTimeline(true);
  }}/>;
  const retryPrompt = async (id: string) => {
    setSendFailure(null);
    try {
      await controller.retryPrompt(id);
      if (controller.getState().outgoing.some((entry) => entry.id === id && entry.status === 'failed')) {
        setSendFailure(controller);
      }
    } catch {
      setSendFailure(controller);
    }
  };

  return (
    <div className="rc-session">
      <SendFailureNotice
        message={sendFailure === controller ? t(locale, 'sendFailed') : null}
        dismissLabel={t(locale, 'dismissSendFailure')}
        onDismiss={() => setSendFailure(null)}
      />
      <ChatTopbarControl><header className="rc-session-head">
        <button type="button" className="rc-back" onClick={onBack} aria-label={botName === undefined ? t(locale, 'back') : t(locale, 'backToBot', { name: botName })}>
          ‹
        </button>
        <div className="rc-session-title">
          <h1>{displayText(summary?.title ?? '', 120) || t(locale, 'untitled')}</h1>
          {summary?.project_label || botName ? <p>{summary?.project_label || botName}</p> : null}
        </div>
        {hostId && (summary?.activityJson || new URLSearchParams(location.search).has('inbox'))?<Suspense fallback={null}><RemoteRead client={client} scroller={scroller} activityJson={summary?.activityJson} hostId={hostId} sessionId={sessionId} live={live} locale={locale}/></Suspense>:null}
        {connection.phase !== 'ready' || state.status === 'offline' ? (
          <span className="rc-topbar-status" role="status">{t(locale, connection.phase === 'ready' ? 'sessionOffline' : 'reconnecting')}</span>
        ) : <TransportBadge locale={locale} transport={connection.transport} />}
      </header></ChatTopbarControl>

      {!approval ? <Controls locale={locale} client={client} controller={controller} controls={state.controls} disabled={!live} /> : null}

      <div
        className="rc-scroll"
        data-session-image-scroll
        ref={scroller}
        onScroll={(event) => {
          const element = event.currentTarget;
          stickToBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < 80;
        }}
      >
        {state.status === 'loading' && state.transcript.conversation.items.length === 0 ? <p className="rc-note" role="status">{t(locale, 'sessionLoading')}</p> : null}
        {state.status === 'error' ? <p className="rc-note rc-danger" role="alert">{errorText(locale, state)}</p> : null}
        {state.status === 'closed' ? <p className="rc-note" role="status">{t(locale, 'sessionClosed')}</p> : null}
        {state.hasOlder ? (
          <button type="button" className="button secondary rc-older" disabled={!live || state.loadingOlder} onClick={() => void controller.loadOlder()}>
            {state.loadingOlder ? t(locale, 'loadingOlder') : t(locale, 'loadOlder')}
          </button>
        ) : null}
        {!approval ? <TranscriptView locale={locale} transcript={state.transcript} running={running} deviceName={deviceName} fetchImage={fetchImage} /> : null}
        {state.outgoing.map((prompt) => (
          <div key={prompt.id} className="rc-user">
            <div className={`rc-bubble rc-outgoing${prompt.status === 'failed' ? ' rc-failed' : ''}`}>
              {prompt.text ? <p className="rc-plain">{prompt.text}</p> : null}
              {prompt.images.length > 0 ? <p className="rc-note">{prompt.images.length} × {t(locale, 'imageAlt')}</p> : null}
              {(prompt.files?.length ?? 0) > 0 ? <p className="rc-note">{prompt.files!.length} × {locale === 'zh' ? '文件' : 'file'}</p> : null}
              <p className="rc-note">
                {prompt.status === 'sending' ? t(locale, 'sending') : t(locale, 'sendFailed')}
                {prompt.status === 'failed' ? (
                  <>
                    {' '}
                    <button type="button" className="rc-link" disabled={!live || (running && (prompt.images.length > 0 || (prompt.files?.length ?? 0) > 0))} onClick={() => void retryPrompt(prompt.id)}>{t(locale, 'sendRetry')}</button>
                    {' '}
                    <button type="button" className="rc-link" onClick={() => controller.dismissPrompt(prompt.id)}>{t(locale, 'dismiss')}</button>
                  </>
                ) : null}
              </p>
            </div>
          </div>
        ))}
      </div>

      <PendingPrompts locale={locale} controller={controller} pending={state.pending} answeredElsewhere={state.answeredElsewhere} disabled={!live} />
      {!approval ? <><Files key={sessionId} client={client} session={sessionId} locale={locale} disabled={!live || connection.transport !== 'webrtc'}
        attachmentLimitReached={attachments.length >= 4}
        onAttach={file => setAttachments(current => current.some(f => f.ref === file.ref)
          ? current.map(f => f.ref === file.ref ? file : f)
          : current.length < 4 ? [...current, file] : current)} />
      {attachments.length ? <ul>{attachments.map(file => <li key={file.ref}>{file.name}{' '}
        <button type="button" aria-label={locale === 'zh' ? '移除文件' : 'Remove file'} onClick={() => setAttachments(current => current.filter(f => f.ref !== file.ref))}>×</button>
      </li>)}</ul> : null}
      <Composer
        locale={locale}
        running={running}
        disabled={!live}
        hasFiles={attachments.length > 0}
        onSend={(text, images) => {
          setSendFailure(null);
          try {
            void controller.sendPrompt(text, images, running ? [] : attachments.map(f => f.ref));
            if (!running) setAttachments([]);
            return true;
          } catch {
            setSendFailure(controller);
            return false;
          }
        }}
        onStop={() => void controller.cancelTurn().catch(() => undefined)}
      /></> : null}
    </div>
  );
}

export function TransportBadge({ locale, transport }: { locale: Locale; transport: ConnectionState['transport'] }) {
  if (transport === undefined) return null;
  const direct = transport === 'webrtc';
  return (
    <span className={`rc-badge ${direct ? 'rc-badge-direct' : 'rc-badge-tunnel'}`} title={t(locale, direct ? 'directHelp' : 'tunnelHelp')}>
      {t(locale, direct ? 'directBadge' : 'tunnelBadge')}
    </span>
  );
}

function RemoteNoticePreview({client,sessionId,target,hostId,live,locale,deviceName,onReturn}:{
  client:RemoteClient;sessionId:string;target:import('../../../../packages/inbox/contracts').InboxNoticeTarget;hostId?:string;live:boolean;locale:Locale;deviceName:string;onReturn():void;
}){
  const [body,setBody]=useState<Transcript|null>();
  useEffect(()=>{
    let mounted=true;setBody(undefined);
    if(!live || target.through<1 || !target.key || !hostId){if(live)setBody(null);return;}
    const key=target.key;
    async function load(){
      const [{auth},{webInboxRequest}]=await Promise.all([import('../app/firebase'),import('../inbox/session')]);
      const user=auth.currentUser;if(!user)throw new Error('SIGN_IN_REQUIRED');
      const {item}=await webInboxRequest(user)<{item:import('../../../../packages/inbox/contracts').InboxItem}>({action:'get',key,generation:target.generation});
      if(!mounted || item.ref.kind!=='assistant' || item.ref.conversationId!==sessionId || item.remoteHostId!==hostId || auth.currentUser?.uid!==user.uid)throw new Error('SOURCE_ACCESS_REQUIRED');
      return client.activityPreview(sessionId,target.through);
    }
    void load().then(result=>{
      if(mounted)setBody(result.available && result.messages.length>0?new TranscriptBuilder(result.messages).snapshot():null);
    }).catch(()=>{if(mounted)setBody(null);});
    return ()=>{mounted=false;};
  },[client,sessionId,target.through,target.key,target.generation,hostId,live]);
  return <section className="rc-session"><header className="rc-session-head"><h1>{locale==='zh'?'通知预览':'Notification preview'}</h1></header>
    {body?<TranscriptView locale={locale} transcript={body} running={false} deviceName={deviceName} fetchImage={client.session(sessionId).fetchImage}/>:
     body===null?<p role="status">{locale==='zh'?'此通知的内容已不可用。':'The content of this notification is no longer available.'}</p>:null}
    <button type="button" onClick={onReturn}>{locale==='zh'?'返回会话':'Return to conversation'}</button>
  </section>;
}
