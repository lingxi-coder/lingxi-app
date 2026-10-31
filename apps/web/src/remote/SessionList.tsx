import type { RemoteSessionSummary } from '@lingxi/bridge-client/remote';
import type { Locale } from '../utils/locale';
import type { RemoteClient } from './remoteClient';
import { t, type StringKey } from './strings';
import { useStore } from './useStore';

const STATUS: Record<RemoteSessionSummary['status'], StringKey> = {
  running: 'statusRunning',
  requires_action: 'statusRequiresAction',
  idle: 'statusIdle',
  dormant: 'statusDormant',
};

const ACTION: Record<NonNullable<RemoteSessionSummary['pending_action']>, StringKey> = {
  permission: 'needsPermission',
  question: 'needsQuestion',
  computer_access: 'needsComputerAccess',
};

interface SessionListProps {
  locale: Locale;
  client: RemoteClient;
  onOpen(sessionId: string): void;
}

export function SessionList({ locale, client, onOpen }: SessionListProps) {
  const state = useStore(client)!;
  return (
    <section className="rc-panel">
      <h2>{t(locale, 'sessions')}</h2>
      {state.sessions === undefined ? <p className="rc-note" role="status">{t(locale, 'loading')}</p> : null}
      {state.sessions?.length === 0 ? (
        <div className="rc-empty">
          <h3>{t(locale, 'noSessions')}</h3>
          <p>{t(locale, 'noSessionsHelp')}</p>
        </div>
      ) : null}
      <ul className="rc-list">
        {(state.sessions ?? []).map((session) => (
          <SessionRow key={session.session_id} locale={locale} session={session} onOpen={onOpen} />
        ))}
      </ul>
    </section>
  );
}

/** One session in a list; a Bot's chats are listed with it too. */
export function SessionRow({ locale, session, onOpen }: { locale: Locale; session: RemoteSessionSummary; onOpen(sessionId: string): void }) {
  return (
    <li>
      <button type="button" className="rc-row rc-row-button" onClick={() => onOpen(session.session_id)}>
        <span className="rc-row-main">
          <strong>{session.title || t(locale, 'untitled')}</strong>
          <span className="rc-row-sub">
            {session.project_label}
            {session.model ? ` · ${session.model}` : ''}
          </span>
          {session.pending_action ? <span className="rc-pending-tag">{t(locale, ACTION[session.pending_action] ?? 'needsPermission')}</span> : null}
        </span>
        <span className={`rc-status rc-status-${session.status}`}>{t(locale, STATUS[session.status] ?? 'statusIdle')}</span>
      </button>
    </li>
  );
}
