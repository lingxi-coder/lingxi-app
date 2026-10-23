import { Files } from './Files';
import type { FileReceipt } from '@lingxi/bridge-client/remote';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { Locale } from '../utils/locale';
import { Composer } from './Composer';
import { Controls } from './Controls';
import { PendingPrompts } from './Prompts';
import type { RemoteClient, SessionState } from './remoteClient';
import { TranscriptView } from './Transcript';
import { t } from './strings';
import { displayText } from './text';
import { useStore } from './useRemote';
import type { ConnectionState } from './viewerConnection';

interface SessionViewProps {
  locale: Locale;
  client: RemoteClient;
  sessionId: string;
  connection: ConnectionState;
  deviceName: string;
  onBack(): void;
}

const ERROR_KEYS = {
  unknown_session: 'errUnknownSession',
  not_remote_enabled: 'errNotRemoteEnabled',
  capacity: 'errCapacity',
  forbidden: 'errForbidden',
} as const;

function errorText(locale: Locale, state: SessionState): string {
  const key = ERROR_KEYS[(state.error?.code ?? '') as keyof typeof ERROR_KEYS];
  return t(locale, key ?? 'errGeneric');
}

export function SessionView({ locale, client, sessionId, connection, deviceName, onBack }: SessionViewProps) {
  const controller = useMemo(() => client.session(sessionId), [client, sessionId]);
  const state = useStore(controller)!;
  const list = useStore(client)!;
  const summary = list.sessions?.find((session) => session.session_id === sessionId);
  const scroller = useRef<HTMLDivElement>(null);
  const [attachments, setAttachments] = useState<FileReceipt[]>([]);
  const stickToBottom = useRef(true);
  useEffect(() => setAttachments([]), [sessionId]);

  useEffect(() => {
    controller.activate();
    const report = (): void => controller.setForeground(document.visibilityState === 'visible');
    report();
    document.addEventListener('visibilitychange', report);
    return () => {
      document.removeEventListener('visibilitychange', report);
      controller.deactivate();
    };
  }, [controller]);

  // Follow new content while the reader is at the bottom; leave them alone once they scroll up.
  useLayoutEffect(() => {
    const element = scroller.current;
    if (element && stickToBottom.current) element.scrollTop = element.scrollHeight;
  }, [state.transcript, state.outgoing, state.pending]);

  const live = state.status === 'live' && connection.phase === 'ready';
  const running = state.turn.running || state.transcript.conversation.running;

  return (
    <div className="rc-session">
      <header className="rc-session-head">
        <button type="button" className="rc-back" onClick={onBack} aria-label={t(locale, 'back')}>
          ‹
        </button>
        <div className="rc-session-title">
          <h1>{displayText(summary?.title ?? '', 120) || t(locale, 'untitled')}</h1>
          {summary?.project_label ? <p>{summary.project_label}</p> : null}
        </div>
        <TransportBadge locale={locale} transport={connection.transport} />
      </header>

      {connection.phase !== 'ready' ? <p className="rc-banner" role="status">{t(locale, 'reconnecting')}</p> : null}
      {state.status === 'offline' && connection.phase === 'ready' ? <p className="rc-banner" role="status">{t(locale, 'sessionOffline')}</p> : null}

      <Controls locale={locale} client={client} controller={controller} controls={state.controls} disabled={!live} />

      <div
        className="rc-scroll"
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
          <button type="button" className="button secondary rc-older" disabled={state.loadingOlder} onClick={() => void controller.loadOlder()}>
            {state.loadingOlder ? t(locale, 'loadingOlder') : t(locale, 'loadOlder')}
          </button>
        ) : null}
        <TranscriptView locale={locale} transcript={state.transcript} running={running} deviceName={deviceName} fetchImage={(ref) => controller.fetchImage(ref)} />
        {state.outgoing.map((prompt) => (
          <div key={prompt.id} className="rc-user">
            <div className={`rc-bubble rc-outgoing${prompt.status === 'failed' ? ' rc-failed' : ''}`}>
              {prompt.text ? <p className="rc-plain">{prompt.text}</p> : null}
              {prompt.images.length > 0 ? <p className="rc-note">{prompt.images.length} × {t(locale, 'imageAlt')}</p> : null}
              <p className="rc-note">
                {prompt.status === 'sending' ? t(locale, 'sending') : t(locale, 'sendFailed')}
                {prompt.status === 'failed' ? (
                  <>
                    {' '}
                    <button type="button" className="rc-link" onClick={() => void controller.retryPrompt(prompt.id)}>{t(locale, 'sendRetry')}</button>
                    {' '}
                    <button type="button" className="rc-link" onClick={() => controller.dismissPrompt(prompt.id)}>{t(locale, 'dismiss')}</button>
                  </>
                ) : null}
              </p>
            </div>
          </div>
        ))}
      </div>

      <PendingPrompts locale={locale} controller={controller} pending={state.pending} answeredElsewhere={state.answeredElsewhere} />
      <Files key={sessionId} client={client} session={sessionId} locale={locale} disabled={!live || connection.transport !== 'webrtc'}
        onAttach={file => setAttachments(current => [...current.filter(f => f.ref !== file.ref), file].slice(-4))} />
      {attachments.length ? <ul>{attachments.map(file => <li key={file.ref}>{file.name}{' '}
        <button type="button" aria-label={locale === 'zh' ? '移除文件' : 'Remove file'} onClick={() => setAttachments(current => current.filter(f => f.ref !== file.ref))}>×</button>
      </li>)}</ul> : null}
      <Composer
        locale={locale}
        running={running}
        disabled={!live}
        hasFiles={attachments.length > 0}
        onSend={(text, images) => { void controller.sendPrompt(text, images, attachments.map(f => f.ref)); setAttachments([]); }}
        onStop={() => void controller.cancelTurn().catch(() => undefined)}
      />
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
