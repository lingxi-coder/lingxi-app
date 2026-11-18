import { createContext, useContext, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { messageErrorText, teamsErrorText } from './errors.js';
import { teamsText, useTeamsText } from './i18n.js';
import { Icon } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

/** Undefined uses the view's inline header. Null waits for the host's topbar to mount. */
export const ChatTopbarTarget = createContext<HTMLElement | null | undefined>(undefined);

export function ChatTopbarHeader({ children }: { children: ReactNode }) {
  const target = useContext(ChatTopbarTarget);
  if (target === undefined) return <>{children}</>;
  return target ? createPortal(<div className="tw-scope tw-outer-chat">{children}</div>, target) : null;
}

/** A host control, such as Web's compact back button, shares the same outer row. */
export function ChatTopbarControl({ children }: { children: ReactNode }) {
  const target = useContext(ChatTopbarTarget);
  if (target === undefined) return <>{children}</>;
  return target ? createPortal(children, target) : null;
}

export interface ConnectionStatus { kind: 'ok' | 'connecting' | 'offline' | 'error'; label: string; detail: string; action?: 'refresh' | 'reconnect' }
/**
 * A failed refetch of the team list is an error with a retry.
 * Initial server confirmation is shown as connecting, then cached-only data is offline.
 * Nothing while the first list is loading or when there is no team to connect to.
 */
export function connectionStatus(snapshot: Pick<TeamsSnapshot, 'loading' | 'selectedTeamId' | 'loadError' | 'offline' | 'messagingConnecting' | 'messagingError'>, locale: string): ConnectionStatus | undefined {
  const t = teamsText(locale);
  if (snapshot.loading) return undefined;
  if (snapshot.loadError) return { kind: 'error', label: t('tw_sidebar_refresh_failed'), detail: teamsErrorText(snapshot.loadError, locale), action: 'refresh' };
  if (!snapshot.selectedTeamId) return undefined;
  if (snapshot.messagingConnecting && !snapshot.messagingError) return {
    kind: 'connecting', label: t('tw_connecting'),
    detail: t('tw_sidebar_waiting_server'),
  };
  if (snapshot.offline) {
    return {
      kind: 'offline', label: t('teams_offline_title'), action: 'reconnect',
      detail: snapshot.messagingError ? messageErrorText(snapshot.messagingError, locale) : t('tw_sidebar_cached'),
    };
  }
  return { kind: 'ok', label: t('tw_sidebar_connected'), detail: t('tw_sidebar_connected_hint') };
}

export function TopbarConnection({ snapshot, workspace, locale, friend }: {
  snapshot: TeamsSnapshot; workspace: TeamsWorkspace; locale: string; friend?: boolean;
}) {
  const t = useTeamsText(locale);
  let status = connectionStatus(snapshot, locale);
  if (status && !snapshot.loadError && snapshot.messagingError) {
    status = { kind: 'error', label: t('teams_load_failed'), detail: messageErrorText(snapshot.messagingError, locale), action: 'reconnect' };
  } else if (status?.kind === 'offline') {
    status = { ...status, detail: t('teams_messages_offline') };
  } else if (friend && status?.kind === 'ok') {
    // A personal chat still uses the pair workspace, but its status belongs to the chat.
    return null;
  }
  if (!status) return null;
  const { kind, label, detail, action } = status;
  return <details className="tw-top-connection" data-kind={kind}
    onBlur={event => { if (!event.currentTarget.contains(event.relatedTarget)) event.currentTarget.open = false; }}
    onKeyDown={event => {
      if (event.key !== 'Escape' || !event.currentTarget.open) return;
      event.stopPropagation();
      event.currentTarget.open = false;
      event.currentTarget.querySelector('summary')?.focus();
    }}>
    <summary aria-label={label} title={detail}>
      <span role="status" className="tw-connection-label"><span className="tw-dot" aria-hidden="true" /><span>{label}</span></span>
      <Icon name="chevronDown" size={12} />
    </summary>
    <div className="tw-connection-detail">
      <p>{detail}</p>
      {action ? <button type="button" className="tw-link" onClick={() => action === 'refresh' ? void workspace.refresh() : workspace.reconnect()}>
        {action === 'refresh' ? t('common_retry') : t('teams_reconnect')}
      </button> : null}
    </div>
  </details>;
}
