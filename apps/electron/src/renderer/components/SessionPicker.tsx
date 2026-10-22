import { useCallback, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import type { SessionCatalogRow, SessionCatalogScope } from '../../shared/sessionWorkspace';
import type { UseBridge } from '../bridge/bridgeTypes';
import { dialogFocusTarget } from '../bridge/bridgeRuntimeState';
import { formatSessionMetadata } from '../bridge/sessionPresentation';
import { DesktopDialog, DesktopDialogButton } from './DesktopDialog';
import { Icon } from './Icon';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const SCOPES: readonly { value: SessionCatalogScope; label: string }[] = [
  { value: 'current', label: 'Current' },
  { value: 'repository', label: 'Repository' },
  { value: 'all', label: 'All' },
];

/** Resolve UUIDs against picker metadata before using the host's slash fallback. */
export function resumeArgumentRow(query: string, sessions: readonly SessionCatalogRow[]): SessionCatalogRow | undefined {
  const argument = query.trim();
  if (UUID.test(argument)) return sessionPickerRows(sessions, '').find((session) => session.uuid.toLowerCase() === argument.toLowerCase());
  if (!argument) return undefined;
  const matches = sessions.filter((session) => session.resume_title?.toLowerCase().trim() === argument.toLowerCase());
  return matches.length === 1 ? matches[0] : undefined;
}

export function resumeArgumentSessionId(query: string, sessions: readonly SessionCatalogRow[] = []): string | undefined {
  const argument = query.trim();
  if (UUID.test(argument)) return argument;
  return resumeArgumentRow(argument, sessions)?.uuid;
}

export function sessionPickerRows(sessions: readonly SessionCatalogRow[], query: string): SessionCatalogRow[] {
  const terms = query.trim().toLocaleLowerCase().split(/\s+/).filter(Boolean);
  return sessions.filter((session) => {
    const text = `${session.title}\n${session.resume_title ?? ''}\n${session.uuid}\n${session.path}`.toLocaleLowerCase();
    return terms.every((term) => text.includes(term));
  }).sort((left, right) => (Date.parse(right.modified_rfc3339) || 0) - (Date.parse(left.modified_rfc3339) || 0));
}

export function sessionPickerShortcutScope(
  key: string,
  scope: SessionCatalogScope,
  previousLocalScope: 'current' | 'repository',
): SessionCatalogScope | undefined {
  if (key.toLowerCase() === 'a') return scope === 'all' ? previousLocalScope : 'all';
  return undefined;
}

export function restoreSessionPickerFocus(opener: HTMLElement | null, fallback: HTMLElement | null): void {
  const target = opener?.isConnected && !opener.closest('[inert]') && opener.getClientRects().length ? opener : fallback;
  target?.focus({ preventScroll: true });
}

export interface SessionPickerProps {
  projectPath?: string;
  initialQuery?: string;
  activeSessionId?: string;
  suspended?: boolean;
  listProjectSessions: UseBridge['listProjectSessions'];
  openSession: UseBridge['openSession'];
  onOpened(): void;
  onClose(): void;
}

export function SessionPicker({ projectPath, initialQuery = '', activeSessionId, suspended = false, listProjectSessions, openSession, onOpened, onClose }: SessionPickerProps) {
  const dialog = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const mounted = useRef(false);
  const requestGeneration = useRef(0);
  const opening = useRef(false);
  const firstRequest = useRef(true);
  const initialArgument = useRef(initialQuery.trim());
  const previousLocalScope = useRef<'current' | 'repository'>('repository');
  const [scope, setScope] = useState<SessionCatalogScope>('repository');
  const [query, setQuery] = useState(initialQuery);
  const [sessions, setSessions] = useState<SessionCatalogRow[]>([]);
  const [selectedPath, setSelectedPath] = useState<string>();
  const [loading, setLoading] = useState(true);
  const [loaded, setLoaded] = useState(false);
  const [openingUuid, setOpeningUuid] = useState<string>();
  const [error, setError] = useState('');
  const listId = useId();
  const titleId = useId();
  const descriptionId = useId();
  const rows = useMemo(() => sessionPickerRows(sessions, query), [sessions, query]);
  const selectedIndex = Math.max(0, rows.findIndex((session) => session.path === selectedPath));
  const selected = rows[selectedIndex];
  const selectedOptionId = selected ? `${listId}-${selectedIndex}` : undefined;
  const resumeId = selected?.uuid ?? resumeArgumentSessionId(query);

  useEffect(() => {
    mounted.current = true;
    dialog.current?.setAttribute('aria-modal', 'true');
    input.current?.focus({ preventScroll: true });
    input.current?.select();
    return () => { mounted.current = false; requestGeneration.current++; };
  }, []);

  const openChat = useCallback(async (sessionId: string, row?: SessionCatalogRow) => {
    if (!mounted.current || opening.current) return;
    if (!projectPath) { setError('Select a project before resuming a chat.'); return; }
    opening.current = true;
    setOpeningUuid(sessionId);
    setError('');
    try {
      const opened = await openSession(projectPath, sessionId, row
        ? { scope, transcriptPath: row.path }
        : { scope: 'repository' });
      if (!mounted.current) return;
      if (!opened) throw new Error('Unable to open this chat. Retry or choose another chat.');
      onOpened();
      onClose();
    } catch (cause) {
      if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      opening.current = false;
      if (mounted.current) setOpeningUuid(undefined);
    }
  }, [onClose, onOpened, openSession, projectPath, scope]);

  const loadSessions = useCallback(async () => {
    if (opening.current) return;
    const generation = ++requestGeneration.current;
    const isCurrent = () => mounted.current && generation === requestGeneration.current;
    const automatic = firstRequest.current;
    setError('');
    if (!projectPath) { setLoading(false); setError('Select a project before resuming a chat.'); return; }
    setLoading(true);
    setLoaded(false);
    setSessions([]);
    try {
      let catalog = await listProjectSessions(projectPath, scope);
      if (!isCurrent()) return;
      // A sidebar refresh can supersede the current-scope request. Retry once
      // rather than presenting a canceled response as an empty catalog.
      if (!catalog) catalog = await listProjectSessions(projectPath, scope);
      if (!isCurrent()) return;
      if (!catalog) throw new Error('The chat list changed while loading. Refresh to try again.');
      const autoResume = automatic && firstRequest.current;
      firstRequest.current = false;
      setSessions(catalog.sessions);
      setLoaded(true);
      setSelectedPath(undefined);
      if (catalog.error) { setError(catalog.error); return; }
      const exactRow = autoResume ? resumeArgumentRow(initialArgument.current, catalog.sessions) : undefined;
      if (exactRow) await openChat(exactRow.uuid, exactRow);
      else if (autoResume) {
        const fallbackId = resumeArgumentSessionId(initialArgument.current);
        if (fallbackId) await openChat(fallbackId);
      }
    } catch (cause) {
      if (isCurrent()) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (isCurrent()) setLoading(false);
    }
  }, [listProjectSessions, openChat, projectPath, scope]);

  useEffect(() => {
    void loadSessions();
    return () => { requestGeneration.current++; };
  }, [loadSessions]);

  useEffect(() => {
    if (suspended) {
      document.querySelector<HTMLElement>('.permission-prompt-inline button')?.focus({ preventScroll: true });
      return;
    }
    dialog.current?.setAttribute('aria-modal', 'true');
    if (openingUuid) dialog.current?.focus({ preventScroll: true });
    else input.current?.focus({ preventScroll: true });
  }, [openingUuid, suspended]);

  useEffect(() => {
    if (selectedOptionId) document.getElementById(selectedOptionId)?.scrollIntoView({ block: 'nearest' });
  }, [selectedOptionId]);

  const changeScope = (next: SessionCatalogScope) => {
    if (opening.current) return;
    firstRequest.current = false;
    if (next !== 'all') previousLocalScope.current = next;
    setScope(next);
    input.current?.focus({ preventScroll: true });
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Tab') {
      const controls = Array.from(dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)') ?? []);
      const target = dialogFocusTarget(controls, document.activeElement as HTMLElement | null, event.shiftKey);
      if (target || !controls.length) {
        event.preventDefault();
        (target ?? dialog.current)?.focus({ preventScroll: true });
      }
      return;
    }
    if (event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey) {
      const next = sessionPickerShortcutScope(event.key, scope, previousLocalScope.current);
      if (next) { event.preventDefault(); event.stopPropagation(); changeScope(next); return; }
    }
    if (event.target !== input.current || opening.current) return;
    if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
      event.preventDefault();
      if (rows.length) setSelectedPath(rows[Math.max(0, Math.min(rows.length - 1, selectedIndex + (event.key === 'ArrowDown' ? 1 : -1)))]!.path);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      if (!loading && resumeId) void openChat(resumeId, selected);
    }
  };

  const description = scope === 'all' ? 'Saved chats from every directory.'
    : scope === 'repository' ? 'This repository and its linked worktrees.'
    : projectPath ?? 'No project selected';

  if (suspended) return null;

  return <DesktopDialog ref={dialog} tabIndex={-1} title="Resume chat" titleId={titleId} summary={description} summaryId={descriptionId}
    ariaLabelledBy={titleId} ariaDescribedBy={descriptionId} icon="chat" size="wide" className="session-picker" zIndex={100}
    onEscape={() => { if (!opening.current) onClose(); }} onKeyDown={handleKeyDown}
    footer={<div className="session-picker-footer">
      <span className="session-picker-shortcuts"><span><kbd>↑</kbd><kbd>↓</kbd> Select</span><span><kbd>Ctrl A</kbd> All</span></span>
      <DesktopDialogButton variant="primary" disabled={loading || Boolean(openingUuid) || !resumeId} onClick={() => { if (resumeId) void openChat(resumeId, selected); }}>
        {openingUuid ? 'Opening…' : 'Resume'}
      </DesktopDialogButton>
    </div>}>
    <button type="button" className="session-picker-close" aria-label="Close chat picker" disabled={Boolean(openingUuid)} onClick={onClose}><Icon name="x" size={18} /></button>
    <div className="session-picker-controls">
      <div className="session-picker-scopes" role="group" aria-label="Chat directory scope">
        {SCOPES.map(({ value, label }) => <button key={value} type="button" aria-pressed={scope === value} disabled={Boolean(openingUuid)}
          aria-keyshortcuts={value === 'all' ? 'Control+A' : undefined}
          onClick={() => changeScope(value)}>{label}</button>)}
      </div>
      <div className="session-picker-search">
        <Icon name="search" size={17} />
        <input ref={input} type="search" role="combobox" aria-label="Search chats" aria-expanded="true" aria-controls={listId}
          aria-autocomplete="list" aria-activedescendant={selectedOptionId} value={query} disabled={Boolean(openingUuid)}
          placeholder="Search chats by name or session ID" spellCheck={false}
          onChange={(event) => { firstRequest.current = false; setQuery(event.target.value); setSelectedPath(undefined); setError(''); }} />
      </div>
    </div>
    {error && <div className="session-picker-error" role="alert"><span>{error}</span><button type="button" disabled={Boolean(openingUuid)} onClick={() => { void loadSessions(); }}>Refresh</button></div>}
    <div id={listId} role="listbox" aria-label="Saved chats" aria-busy={loading || Boolean(openingUuid)} className="session-picker-list">
      {loading ? <p role="status" className="session-picker-empty">Loading chats…</p>
        : rows.length ? rows.map((session, index) => <div key={session.path} id={`${listId}-${index}`} role="option" aria-selected={selected?.path === session.path}
          aria-disabled={Boolean(openingUuid)} className="session-picker-row" title={session.path}
          onPointerMove={() => { if (!opening.current) setSelectedPath(session.path); }}
          onPointerDown={(event) => event.preventDefault()} onClick={() => { void openChat(session.uuid, session); }}>
          <span className="session-picker-row-icon" aria-hidden="true"><Icon name="chat" size={18} /></span>
          <span className="session-picker-row-copy"><strong>{session.title || 'Untitled chat'}</strong>
            <span>{formatSessionMetadata(session.modified_rfc3339, session.message_count)}<code>{session.uuid.slice(0, 8)}</code></span>
          </span>
          {activeSessionId === session.uuid && <span className="session-picker-current">Current</span>}
        </div>)
          : <p role="status" className="session-picker-empty">{loaded ? query.trim() ? 'No chats match this search.' : 'No saved chats in this scope.' : error ? 'Change the search or refresh to browse chats.' : 'Choose a chat to resume.'}</p>}
    </div>
    <div className="session-picker-count" aria-live="polite">{!loading && loaded ? `${rows.length} ${rows.length === 1 ? 'chat' : 'chats'}` : openingUuid ? 'Opening saved chat…' : ''}</div>
  </DesktopDialog>;
}
