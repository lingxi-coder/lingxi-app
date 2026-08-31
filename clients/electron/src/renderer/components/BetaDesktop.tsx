import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ClipboardEvent, type KeyboardEvent, type ReactNode } from 'react';
import type {
  ImageRefDto,
  ModelDetailsDto,
  ReasoningSelectionDto,
  SessionRowDto,
} from '@lingxi/bridge-client';

import type { UseBridge } from '../bridge/useBridge';
import { orderedTasks } from '../bridge/desktopState';
import { classifyDesktopError } from '../bridge/errors';
import { useT } from '../theme/ThemeContext';
import type { ThemeMode } from '../theme/tokens';
import {
  activeFileMention,
  promptWithFileMentions,
} from '../bridge/fileMentions';
import { imageFileToAttachment, type ImageAttachment } from '../bridge/imageInput';
import {
  activeSlashCommand,
  filterSlashCommands,
  moveSlashSelectionIndex,
  reconcileSlashSelectionIndex,
  slashCommandText,
  slashMenuLabel,
  slashNavigationDirection,
} from '../bridge/slashCommands';
import { groupModelReferences, modelReference } from '../bridge/modelCatalog';
import { persistProviderCredentialInput } from '../bridge/providerCredentials';
import { formatSessionMetadata } from '../bridge/sessionPresentation';
import { DESKTOP_COMMANDS } from '../bridge/desktopCommands';
import { resolveDesktopCommand, type DesktopCommandContext } from '../bridge/slashDispatch';
import { Icon } from './Icon';
import { PROVIDERS, providerById } from '../../shared/providers';
import { MAX_IMAGE_ATTACHMENTS } from '../../shared/imageInput';
import { PERM_MODES } from '../data';

function basename(path?: string): string {
  if (!path) return 'No project';
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function invoke(action: () => Promise<unknown>): void {
  void action().catch(() => undefined);
}

function Button({ children, onClick, disabled = false, primary = false, success = false, danger = false, title }: {
  children: ReactNode;
  onClick(): void;
  disabled?: boolean;
  primary?: boolean;
  success?: boolean;
  danger?: boolean;
  title?: string;
}) {
  const t = useT();
  return (
    <button
      type="button"
      title={title}
      disabled={disabled}
      onClick={onClick}
      style={{
        display: 'inline-flex', alignItems: 'center', justifyContent: 'center', gap: 7,
        minHeight: 32, padding: '6px 11px', borderRadius: 8,
        border: `0.5px solid ${primary ? t.accentBorder : success ? t.ok : t.border}`,
        background: primary ? t.accent : success ? t.ok : 'transparent',
        color: danger ? t.danger : primary || success ? '#fff' : t.text2,
        fontSize: 12.5, fontWeight: 600, cursor: disabled ? 'not-allowed' : 'pointer',
        opacity: disabled && !success ? 0.45 : 1,
      }}
    >
      {children}
    </button>
  );
}

function ConnectionDot({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  const status = bridge.connection.status;
  const color = status === 'connected' ? t.ok : status === 'error' || status === 'disconnected' ? t.danger : t.warn;
  const label = status === 'connected'
    ? 'Engine ready'
    : status === 'spawning'
      ? 'Starting engine'
      : status === 'restarting'
        ? 'Restarting engine'
        : status === 'connecting'
          ? 'Connecting'
          : status === 'error'
            ? 'Engine error'
            : status === 'disconnected'
              ? 'Disconnected'
              : 'Engine idle';
  return (
    <span role="status" aria-live="polite" title={label} style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 11.5, color: t.text3 }}>
      <span style={{ width: 7, height: 7, borderRadius: 99, background: color, boxShadow: `0 0 0 3px color-mix(in oklab, ${color} 18%, transparent)` }} />
      {label}
    </span>
  );
}

function SessionRow({ session, active, pinned, opening, status, onClick, onPin }: {
  session: SessionRowDto;
  active: boolean;
  pinned: boolean;
  opening: boolean;
  status?: ReturnType<UseBridge['sessionRuntimeStatus']>;
  onClick(): void;
  onPin(): void;
}) {
  const t = useT();
  const highlighted = active || opening;
  const attention = opening
    ? { label: 'Opening session', color: t.accent }
    : status?.connection.status === 'error' || status?.error
    ? { label: 'Session error', color: t.danger }
    : status?.pendingInteractions
      ? { label: 'Waiting for input', color: t.warn }
      : status?.turnActive
        ? { label: 'Running', color: t.ok }
        : undefined;
  return (
    <div className="sidebar-tree-row" style={{ position: 'relative' }}>
      <button
        type="button"
        onClick={onClick}
        disabled={opening}
        aria-busy={opening || undefined}
        aria-current={active ? 'page' : undefined}
        title={session.title || 'Untitled session'}
        style={{
          width: '100%', minHeight: 43, display: 'grid', gap: 1,
          padding: '6px 34px 6px 30px', borderRadius: 8, border: 0, textAlign: 'left',
          background: active ? t.accentBg : opening ? t.surface : 'transparent', color: highlighted ? t.text : t.text2,
          cursor: opening ? 'wait' : 'pointer',
          fontSize: 12.5, fontWeight: active ? 620 : 470,
        }}
      >
        <span style={{ display: 'flex', alignItems: 'center', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{session.title || 'Untitled session'}</span>
          {opening
            ? <span className="beta-spinner" role="status" aria-label="Opening session" title="Opening session" style={{ marginLeft: 7, color: t.accent }} />
            : attention ? <span aria-label={attention.label} title={attention.label} style={{ flexShrink: 0, width: 6, height: 6, marginLeft: 7, borderRadius: 99, background: attention.color }} /> : null}
        </span>
        <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 10.5 }}>
          {formatSessionMetadata(session.modified_rfc3339, session.message_count)}
        </span>
      </button>
      <button
        type="button"
        className="sidebar-row-action"
        data-visible={pinned ? 'true' : undefined}
        aria-label={`${pinned ? 'Unpin' : 'Pin'} ${session.title || 'Untitled session'}`}
        title={pinned ? 'Unpin session' : 'Pin session'}
        onClick={(event) => { event.stopPropagation(); onPin(); }}
        style={{
          position: 'absolute', right: 4, top: 4, width: 26, height: 26,
          display: 'grid', placeItems: 'center', border: 0, borderRadius: 6,
          background: active ? 'transparent' : t.sidebarBg, color: pinned ? t.accent : t.text3,
          cursor: 'pointer',
        }}
      >
        <Icon name="pin" size={13} stroke={1.8} />
      </button>
    </div>
  );
}

export function BetaSidebar({ bridge, onOpenSettings }: { bridge: UseBridge; onOpenSettings(): void }) {
  const t = useT();
  const settings = bridge.bootstrap?.settings;
  const projects = settings?.projects ?? [];
  const pinnedSessions = settings?.pinnedSessions ?? [];
  const selectedProject = bridge.activeSession?.projectPath ?? settings?.activeProject ?? bridge.bootstrap?.workspace.path;
  const visibleSession = bridge.activeSession ?? bridge.bootstrap?.activeSession ?? settings?.activeSession;
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(
    () => new Set(selectedProject ? [selectedProject] : []),
  );
  const [showAllSessions, setShowAllSessions] = useState<Record<string, boolean>>({});
  const [menuProject, setMenuProject] = useState<string | null>(null);
  const [openingSessionKey, setOpeningSessionKey] = useState<string | null>(null);
  const [editingProject, setEditingProject] = useState<string | null>(null);

  useEffect(() => {
    setMenuProject(null);
    if (selectedProject) {
      setExpandedProjects((current) => current.has(selectedProject) ? current : new Set([...current, selectedProject]));
    }
  }, [selectedProject]);

  useEffect(() => {
    for (const projectPath of expandedProjects) {
      if (!bridge.bootstrap?.projectCatalogs?.[projectPath]) void bridge.listProjectSessions(projectPath).catch(() => undefined);
    }
  }, [bridge.bootstrap?.projectCatalogs, bridge.listProjectSessions, expandedProjects]);

  useEffect(() => {
    const pinnedProjects = new Set(pinnedSessions.map((session) => session.projectPath));
    for (const projectPath of pinnedProjects) {
      if (!bridge.bootstrap?.projectCatalogs?.[projectPath]) void bridge.listProjectSessions(projectPath).catch(() => undefined);
    }
  }, [bridge.bootstrap?.projectCatalogs, bridge.listProjectSessions, pinnedSessions]);

  const pinnedKeys = useMemo(
    () => new Set(pinnedSessions.map((session) => `${session.projectPath}\0${session.sessionId}`)),
    [pinnedSessions],
  );
  const openSidebarSession = useCallback((projectPath: string, sessionId: string) => {
    const key = `${projectPath}\0${sessionId}`;
    setOpeningSessionKey(key);
    void bridge.openSession(projectPath, sessionId)
      .catch(() => undefined)
      .finally(() => setOpeningSessionKey((current) => current === key ? null : current));
  }, [bridge.openSession]);
  const editProject = useCallback((projectPath: string) => {
    setExpandedProjects((current) => current.has(projectPath) ? current : new Set([...current, projectPath]));
    setEditingProject(projectPath);
    void bridge.newSession(projectPath)
      .catch(() => undefined)
      .finally(() => setEditingProject((current) => current === projectPath ? null : current));
  }, [bridge.newSession]);
  const pinInput = (projectPath: string, sessionId: string, title: string) => ({ projectPath, sessionId, title });

  return (
    <aside style={{ width: 260, flexShrink: 0, display: 'flex', flexDirection: 'column', background: t.sidebarBg, borderRight: `0.5px solid ${t.border}`, paddingTop: 38 }}>
      <div className="drag-region" style={{ minHeight: 46, padding: '7px 14px 6px', display: 'flex', alignItems: 'center' }}>
        <strong style={{ color: t.text, fontSize: 16.5, fontWeight: 650, letterSpacing: '-.02em' }}>LingXi</strong>
      </div>

      <div style={{ padding: '2px 9px 10px' }}>
        <button
          className="sidebar-primary-action"
          type="button"
          disabled={bridge.sessionLoading || editingProject !== null}
          onClick={() => selectedProject ? editProject(selectedProject) : invoke(bridge.addProject)}
          style={{
            width: '100%', minHeight: 38, display: 'flex', alignItems: 'center', gap: 11,
            padding: '8px 10px', borderRadius: 8, border: 0, background: 'transparent',
            color: t.text, cursor: bridge.sessionLoading || editingProject !== null ? 'wait' : 'pointer', opacity: bridge.sessionLoading || editingProject !== null ? .5 : 1,
            textAlign: 'left', fontSize: 14, fontWeight: 550,
          }}
        >
          {editingProject
            ? <span className="beta-spinner" role="status" aria-label="Opening project draft" />
            : <Icon name="pencil" size={16} color={t.text2} stroke={1.8} />}
          <span>{editingProject ? 'Opening draft…' : selectedProject ? 'New session' : 'Add your first project'}</span>
        </button>
      </div>

      <nav aria-label="Projects and sessions" style={{ flex: 1, minHeight: 0, overflowY: 'auto', padding: '0 8px 12px' }}>
        {pinnedSessions.length > 0 ? (
          <section aria-labelledby="pinned-sessions-heading" style={{ marginBottom: 16 }}>
            <h2 id="pinned-sessions-heading" style={{ padding: '7px 8px 6px', color: t.text4, fontSize: 12.5, fontWeight: 600, letterSpacing: '.01em' }}>Pinned</h2>
            {pinnedSessions.map((pinned) => {
              const pinnedCatalog = bridge.bootstrap?.projectCatalogs?.[pinned.projectPath];
              const current = pinnedCatalog?.sessions.find((session) => session.uuid === pinned.sessionId);
              const title = current?.title || pinned.title || 'Untitled session';
              const metadata = !pinnedCatalog
                ? 'Loading…'
                : current
                  ? formatSessionMetadata(current.modified_rfc3339, current.message_count)
                  : 'Unavailable';
              const active = visibleSession?.projectPath === pinned.projectPath && visibleSession.sessionId === pinned.sessionId;
              const opening = openingSessionKey === `${pinned.projectPath}\0${pinned.sessionId}`;
              const status = bridge.sessionRuntimeStatus(pinned.sessionId);
              const attention = status?.connection.status === 'error' || status?.error
                ? { label: 'Session error', color: t.danger }
                : status?.pendingInteractions
                  ? { label: 'Waiting for input', color: t.warn }
                  : status?.turnActive
                    ? { label: 'Running', color: t.ok }
                    : undefined;
              return (
                <div className="sidebar-tree-row" key={`${pinned.projectPath}-${pinned.sessionId}`} style={{ position: 'relative' }}>
                  <button
                    type="button"
                    aria-current={active ? 'page' : undefined}
                    aria-busy={opening || undefined}
                    disabled={opening}
                    onClick={() => openSidebarSession(pinned.projectPath, pinned.sessionId)}
                    title={`${title}\n${pinned.projectPath}`}
                    style={{
                      width: '100%', minHeight: 43, display: 'grid', gap: 1, padding: '6px 34px 6px 10px',
                      border: 0, borderRadius: 8, background: active ? t.accentBg : opening ? t.surface : 'transparent',
                      color: t.text2, textAlign: 'left', cursor: opening ? 'wait' : 'pointer',
                    }}
                  >
                    <span style={{ display: 'flex', alignItems: 'center', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 12.5, fontWeight: 520 }}>
                      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{title}</span>
                      {opening
                        ? <span className="beta-spinner" role="status" aria-label="Opening session" title="Opening session" style={{ marginLeft: 7, color: t.accent }} />
                        : attention ? <span aria-label={attention.label} title={attention.label} style={{ flexShrink: 0, width: 6, height: 6, marginLeft: 7, borderRadius: 99, background: attention.color }} /> : null}
                    </span>
                    <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 10.5 }}>
                      {basename(pinned.projectPath)} · {metadata}
                    </span>
                  </button>
                  <button
                    type="button"
                    className="sidebar-row-action"
                    data-visible="true"
                    aria-label={`Unpin ${title}`}
                    title="Unpin session"
                    onClick={() => invoke(() => bridge.setSessionPinned(pinInput(pinned.projectPath, pinned.sessionId, title), false))}
                    style={{ position: 'absolute', right: 4, top: 8, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: 'transparent', color: t.accent, cursor: 'pointer' }}
                  >
                    <Icon name="pin" size={13} stroke={1.8} />
                  </button>
                </div>
              );
            })}
          </section>
        ) : null}

        <section aria-labelledby="projects-heading">
          <div style={{ minHeight: 31, padding: '2px 4px 4px 8px', display: 'flex', alignItems: 'center' }}>
            <h2 id="projects-heading" style={{ flex: 1, color: t.text4, fontSize: 12.5, fontWeight: 600, letterSpacing: '.01em' }}>Projects</h2>
            <button
              type="button"
              className="sidebar-header-action"
              aria-label="Add project"
              title="Add project"
              onClick={() => invoke(bridge.addProject)}
              style={{ width: 28, height: 28, display: 'grid', placeItems: 'center', border: 0, borderRadius: 7, background: 'transparent', color: t.text3, cursor: 'pointer' }}
            >
              <Icon name="plus" size={14} stroke={1.9} />
            </button>
          </div>

          {projects.length === 0 ? (
            <div style={{ padding: '12px 9px', color: t.text4, fontSize: 11.5, lineHeight: 1.5 }}>
              Add a project folder to start a session.
            </div>
          ) : projects.map((projectPath) => {
            const active = projectPath === selectedProject;
            const open = expandedProjects.has(projectPath);
            const catalog = bridge.bootstrap?.projectCatalogs?.[projectPath];
            const allSessions = catalog?.sessions ?? [];
            const visibleSessions = showAllSessions[projectPath] ? allSessions : allSessions.slice(0, 5);
            const projectHasActiveWork = (bridge.bootstrap?.runtimes ?? []).some((runtime) => {
              if (runtime.projectPath !== projectPath) return false;
              const status = bridge.sessionRuntimeStatus(runtime.sessionId);
              return Boolean(status?.turnActive || status?.pendingInteractions);
            });
            return (
              <div key={projectPath} style={{ position: 'relative', marginBottom: 2 }}>
                <div className="sidebar-tree-row" style={{ position: 'relative' }}>
                  <button
                    type="button"
                    aria-expanded={open}
                    aria-controls={open ? `project-sessions-${encodeURIComponent(projectPath)}` : undefined}
                    title={projectPath}
                    onClick={() => {
                      setExpandedProjects((current) => {
                        const next = new Set(current);
                        if (next.has(projectPath)) next.delete(projectPath);
                        else next.add(projectPath);
                        return next;
                      });
                      if (!active) invoke(() => bridge.activateProject(projectPath));
                    }}
                    style={{
                      width: '100%', minHeight: 37, display: 'flex', alignItems: 'center', gap: 9,
                      padding: '7px 62px 7px 8px', border: 0, borderRadius: 8,
                      background: active ? t.surface : 'transparent', color: active ? t.text : t.text2,
                      cursor: 'pointer', textAlign: 'left',
                    }}
                  >
                    <Icon name="folder" size={16} color={active ? t.text2 : t.text3} stroke={1.7} />
                    <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 13.5, fontWeight: active ? 600 : 500 }}>{basename(projectPath)}</span>
                  </button>
                  <button
                    type="button"
                    className="sidebar-row-action"
                    data-visible={editingProject === projectPath ? 'true' : undefined}
                    disabled={editingProject !== null}
                    aria-label={`Edit ${basename(projectPath)}`}
                    aria-busy={editingProject === projectPath || undefined}
                    title="Open project draft"
                    onClick={() => editProject(projectPath)}
                    style={{ position: 'absolute', right: 32, top: 5, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: active ? t.surface : t.sidebarBg, color: t.text3, cursor: editingProject !== null ? 'wait' : 'pointer' }}
                  >
                    {editingProject === projectPath
                      ? <span className="beta-spinner" role="status" aria-label="Opening project draft" />
                      : <Icon name="pencil" size={13} stroke={1.8} />}
                  </button>
                  <button
                    type="button"
                    className="sidebar-row-action"
                    data-visible={menuProject === projectPath ? 'true' : undefined}
                    disabled={projectHasActiveWork}
                    aria-label={`Project actions for ${basename(projectPath)}`}
                    aria-expanded={menuProject === projectPath}
                    onClick={() => setMenuProject((current) => current === projectPath ? null : projectPath)}
                    style={{ position: 'absolute', right: 4, top: 5, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: active ? t.surface : t.sidebarBg, color: t.text3, cursor: projectHasActiveWork ? 'not-allowed' : 'pointer' }}
                  >
                    <Icon name="more" size={14} stroke={1.9} />
                  </button>
                </div>

                {menuProject === projectPath ? (
                  <div className="sidebar-project-menu" role="menu" style={{ margin: '2px 4px 5px 24px', padding: 4, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.windowBg, boxShadow: '0 8px 24px rgba(0,0,0,.18)' }}>
                    <button type="button" role="menuitem" disabled={projectHasActiveWork} onClick={() => { setMenuProject(null); invoke(() => bridge.removeProject(projectPath)); }} style={{ width: '100%', minHeight: 30, display: 'flex', alignItems: 'center', gap: 8, padding: '5px 7px', border: 0, borderRadius: 6, background: 'transparent', color: t.danger, cursor: projectHasActiveWork ? 'not-allowed' : 'pointer', textAlign: 'left', fontSize: 11.5 }}>
                      <Icon name="x" size={13} /> Remove from sidebar
                    </button>
                  </div>
                ) : null}

                {open ? (
                  <div id={`project-sessions-${encodeURIComponent(projectPath)}`}>
                    {catalog?.error ? (
                      <div style={{ padding: '8px 10px 8px 30px', color: t.danger, fontSize: 10.5 }}>{catalog.error}</div>
                    ) : !catalog ? (
                      <div style={{ padding: '8px 10px 8px 30px', color: t.text4, fontSize: 10.5 }}>Loading sessions…</div>
                    ) : allSessions.length === 0 ? (
                      <div style={{ padding: '8px 10px 8px 30px', color: t.text4, fontSize: 10.5 }}>No saved sessions yet.</div>
                    ) : visibleSessions.map((session) => {
                      const pinned = pinnedKeys.has(`${projectPath}\0${session.uuid}`);
                      const opening = openingSessionKey === `${projectPath}\0${session.uuid}`;
                      return (
                        <SessionRow
                          key={session.uuid}
                          session={session}
                          active={visibleSession?.projectPath === projectPath && visibleSession.sessionId === session.uuid}
                          pinned={pinned}
                          opening={opening}
                          status={bridge.sessionRuntimeStatus(session.uuid)}
                          onClick={() => openSidebarSession(projectPath, session.uuid)}
                          onPin={() => invoke(() => bridge.setSessionPinned(pinInput(projectPath, session.uuid, session.title || 'Untitled session'), !pinned))}
                        />
                      );
                    })}
                    {allSessions.length > 5 ? (
                      <button type="button" onClick={() => setShowAllSessions((current) => ({ ...current, [projectPath]: !current[projectPath] }))} style={{ minHeight: 32, marginLeft: 30, padding: '5px 8px', border: 0, borderRadius: 7, background: 'transparent', color: t.text4, cursor: 'pointer', fontSize: 11.5 }}>
                        {showAllSessions[projectPath] ? 'Show less' : `Show more (${allSessions.length - 5})`}
                      </button>
                    ) : null}
                  </div>
                ) : null}
              </div>
            );
          })}
        </section>
      </nav>

      <div style={{ padding: 10, borderTop: `0.5px solid ${t.border}`, display: 'flex', flexDirection: 'column', gap: 8 }}>
        <ConnectionDot bridge={bridge} />
        <button
          type="button"
          onClick={onOpenSettings}
          style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '7px 4px', border: 0, background: 'transparent', color: t.text2, cursor: 'pointer', fontSize: 12.5 }}
        >
          <Icon name="cog" size={15} /> Settings & diagnostics
        </button>
      </div>
    </aside>
  );
}

export function BetaTopBar({ bridge, tasksOpen, onToggleTasks, theme, onTheme }: {
  bridge: UseBridge;
  tasksOpen: boolean;
  onToggleTasks(): void;
  theme: ThemeMode;
  onTheme(value: ThemeMode): void;
}) {
  const t = useT();
  return (
    <header className="drag-region" style={{ height: 52, flexShrink: 0, display: 'flex', alignItems: 'center', gap: 12, padding: '0 14px', borderBottom: `0.5px solid ${t.border}`, background: t.windowBg }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ color: t.text, fontSize: 13, fontWeight: 620, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{basename(bridge.activeSession?.projectPath ?? bridge.bootstrap?.workspace.path)}</div>
        <div className="mono" style={{ color: t.text4, fontSize: 10, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{bridge.activeSession?.projectPath ?? bridge.bootstrap?.workspace.path ?? 'Add a project to begin'}</div>
      </div>
      {bridge.usage && (
        <span className="mono" style={{ color: t.text4, fontSize: 9.5 }} title="Input + output tokens">
          {(bridge.usage.inputTokens + bridge.usage.outputTokens).toLocaleString()} tok
        </span>
      )}
      <button className="no-drag" type="button" aria-label="Toggle theme" onClick={() => onTheme(theme === 'dark' ? 'light' : 'dark')} style={{ width: 30, height: 30, display: 'grid', placeItems: 'center', borderRadius: 7, border: `0.5px solid ${t.border}`, background: 'transparent', color: t.text3, cursor: 'pointer' }}>
        <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={14} />
      </button>
      <button className="no-drag" type="button" aria-label="Toggle background tasks" aria-pressed={tasksOpen} onClick={onToggleTasks} style={{ width: 30, height: 30, display: 'grid', placeItems: 'center', borderRadius: 7, border: `0.5px solid ${tasksOpen ? t.accentBorder : t.border}`, background: tasksOpen ? t.accentBg : 'transparent', color: tasksOpen ? t.accent : t.text3, cursor: 'pointer' }}>
        <Icon name="tasks" size={15} />
      </button>
    </header>
  );
}

type SpeechRecognitionResultLike = {
  isFinal: boolean;
  [index: number]: { transcript: string };
};

type SpeechRecognitionEventLike = Event & {
  resultIndex: number;
  results: { length: number; [index: number]: SpeechRecognitionResultLike };
};

type SpeechRecognitionLike = {
  continuous: boolean;
  interimResults: boolean;
  lang: string;
  start(): void;
  stop(): void;
  onresult: ((event: SpeechRecognitionEventLike) => void) | null;
  onerror: ((event: Event & { error?: string }) => void) | null;
  onend: (() => void) | null;
};

type SpeechRecognitionConstructor = new () => SpeechRecognitionLike;

function speechRecognitionConstructor(): SpeechRecognitionConstructor | null {
  if (typeof window === 'undefined') return null;
  const browserWindow = window as Window & {
    SpeechRecognition?: SpeechRecognitionConstructor;
    webkitSpeechRecognition?: SpeechRecognitionConstructor;
  };
  return browserWindow.SpeechRecognition ?? browserWindow.webkitSpeechRecognition ?? null;
}

function modelLabel(model?: string | null): string {
  if (!model) return 'Select model';
  return modelReference(model).label;
}

function reasoningSelectionKey(selection: ReasoningSelectionDto): string {
  return JSON.stringify(selection);
}

function reasoningSelectionLabel(selection?: ReasoningSelectionDto): string {
  if (!selection) return 'Auto';
  switch (selection.type) {
    case 'automatic': return 'Auto';
    case 'disabled': return 'Off';
    case 'enabled': return 'On';
    case 'level': {
      const levelLabels: Record<string, string> = { xhigh: 'Extra High', extra_high: 'Extra High' };
      return levelLabels[selection.id] ?? selection.id.replace(/[-_]/g, ' ').replace(/\b\w/g, (letter) => letter.toUpperCase());
    }
    case 'token_budget': return `${selection.tokens.toLocaleString()} tokens`;
    default: return 'Auto';
  }
}

function modelSupportsFastMode(detail?: ModelDetailsDto): boolean {
  if (detail?.supports_fast_mode !== true) return false;
  const modelId = detail.model_id.toLowerCase();
  return modelId.includes('claude-opus-4-7')
    || modelId.includes('claude-opus-4-8')
    || modelId.includes('claude-opus-5');
}

type FilePickerState = {
  source: 'mention' | 'button';
  query: string;
};

type ModelPickerSubmenu = 'model' | 'effort' | 'speed' | null;
type ModelPickerSection = Exclude<ModelPickerSubmenu, null>;

type RichPromptSnapshot = {
  text: string;
  files: string[];
};

type ComposerDraft = RichPromptSnapshot & {
  html: string;
  images: ImageAttachment[];
};

const FILE_MENTION_SELECTOR = '[data-file-mention]';
const ZERO_WIDTH_SPACE = '\u200b';

function richPromptText(node: Node): string {
  if (node.nodeType === Node.TEXT_NODE) return (node.nodeValue ?? '').split(ZERO_WIDTH_SPACE).join('');
  if (node instanceof HTMLElement && node.matches(FILE_MENTION_SELECTOR)) return '';
  if (node instanceof HTMLBRElement) return '\n';

  const parts: string[] = [];
  let hasText = false;
  let endsWithNewline = false;
  for (const child of node.childNodes) {
    const next = richPromptText(child);
    if (
      child instanceof HTMLElement
      && /^(DIV|P)$/.test(child.tagName)
      && hasText
      && !endsWithNewline
    ) {
      parts.push('\n');
      endsWithNewline = true;
    }
    parts.push(next);
    if (next.length > 0) {
      hasText = true;
      endsWithNewline = next.endsWith('\n');
    }
  }
  return parts.join('');
}

function richPromptSnapshot(editor: HTMLElement): RichPromptSnapshot {
  const files = [...editor.querySelectorAll<HTMLElement>(FILE_MENTION_SELECTOR)]
    .map((token) => token.dataset.fileMention)
    .filter((path): path is string => Boolean(path));
  return { text: richPromptText(editor), files: [...new Set(files)] };
}

function editorSelection(editor: HTMLElement): Range {
  const selection = window.getSelection();
  if (selection?.rangeCount) {
    const current = selection.getRangeAt(0);
    if (editor.contains(current.commonAncestorContainer)) return current.cloneRange();
  }
  const end = document.createRange();
  end.selectNodeContents(editor);
  end.collapse(false);
  return end;
}

function applyEditorSelection(range: Range): void {
  const selection = window.getSelection();
  selection?.removeAllRanges();
  selection?.addRange(range);
}

function createFileMention(path: string, color: string): HTMLElement {
  const token = document.createElement('span');
  token.className = 'beta-file-mention';
  token.dataset.fileMention = path;
  token.contentEditable = 'false';
  token.title = path;
  token.setAttribute('aria-label', `File mention: ${path}`);
  token.style.color = color;

  const icon = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  icon.setAttribute('viewBox', '0 0 18 18');
  icon.setAttribute('width', '18');
  icon.setAttribute('height', '18');
  icon.setAttribute('fill', 'none');
  icon.setAttribute('stroke', 'currentColor');
  icon.setAttribute('stroke-width', '1.45');
  icon.setAttribute('stroke-linecap', 'round');
  icon.setAttribute('stroke-linejoin', 'round');
  icon.setAttribute('aria-hidden', 'true');
  const circle = document.createElementNS('http://www.w3.org/2000/svg', 'circle');
  circle.setAttribute('cx', '9');
  circle.setAttribute('cy', '9');
  circle.setAttribute('r', '7');
  const file = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  file.setAttribute('d', 'M6.5 5.25h4l2.25 2.25v5.25H6.5zM10.5 5.25V7.5h2.25');
  icon.append(circle, file);

  const label = document.createElement('span');
  label.textContent = basename(path);
  token.append(icon, label);
  return token;
}

export function BetaComposer({ bridge, ready, onOpenSettings, onSetTheme }: {
  bridge: UseBridge;
  ready: boolean;
  onOpenSettings(): void;
  onSetTheme(theme: 'dark' | 'light'): void;
}) {
  const t = useT();
  const [text, setText] = useState('');
  const [modelOpen, setModelOpen] = useState(false);
  const [modelSubmenu, setModelSubmenu] = useState<ModelPickerSubmenu>(null);
  const [permissionOpen, setPermissionOpen] = useState(false);
  const [slashQuery, setSlashQuery] = useState<string | null>(null);
  const [filePicker, setFilePicker] = useState<FilePickerState | null>(null);
  const [fileResults, setFileResults] = useState<string[]>([]);
  const [fileResultsTruncated, setFileResultsTruncated] = useState(false);
  const [fileSearchStatus, setFileSearchStatus] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [fileResultIndex, setFileResultIndex] = useState(0);
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [imageAttachments, setImageAttachments] = useState<ImageAttachment[]>([]);
  const [imageNotice, setImageNotice] = useState<string | null>(null);
  const [imageDragActive, setImageDragActive] = useState(false);
  const [goalMode, setGoalMode] = useState(false);
  const [voiceState, setVoiceState] = useState<'idle' | 'listening' | 'unsupported' | 'denied'>('idle');
  const [flowMode, setFlowMode] = useState(false);
  const [slashResultIndex, setSlashResultIndex] = useState(0);
  const input = useRef<HTMLDivElement>(null);
  const fileControl = useRef<HTMLDivElement>(null);
  const fileSearchInput = useRef<HTMLInputElement>(null);
  const imageFileInput = useRef<HTMLInputElement>(null);
  const permissionControl = useRef<HTMLDivElement>(null);
  const permissionButton = useRef<HTMLButtonElement>(null);
  const modelControl = useRef<HTMLDivElement>(null);
  const slashControl = useRef<HTMLDivElement>(null);
  const recognition = useRef<SpeechRecognitionLike | null>(null);
  const voiceBase = useRef('');
  const fileSearchRequest = useRef(0);
  const savedEditorSelection = useRef<Range | null>(null);
  const activeMentionRange = useRef<Range | null>(null);
  const activeSlashRange = useRef<Range | null>(null);
  const activeSlashQuery = useRef<string | null>(null);
  const slashDismissed = useRef(false);
  const imageAttachmentsRef = useRef<ImageAttachment[]>([]);
  const draftsBySession = useRef(new Map<string, ComposerDraft>());
  const draftSessionId = useRef<string | null>(null);
  imageAttachmentsRef.current = imageAttachments;
  const activeSessionId = bridge.activeSession?.sessionId ?? null;

  const slashCommands = useMemo(
    () => filterSlashCommands(bridge.desktop.slashCommands, slashQuery ?? ''),
    [bridge.desktop.slashCommands, slashQuery],
  );
  const modelGroups = useMemo(
    () => groupModelReferences(bridge.desktop.models),
    [bridge.desktop.models],
  );
  const modelDetailsByReference = useMemo(
    () => new Map(bridge.desktop.modelDetails.map((detail) => [detail.reference, detail])),
    [bridge.desktop.modelDetails],
  );
  const currentModelDetail = modelDetailsByReference.get(bridge.desktop.currentModel ?? '');
  const currentModelProvider = modelReference(bridge.desktop.currentModel ?? '').providerId;
  const fastModeAvailable = currentModelProvider === 'anthropic'
    && modelSupportsFastMode(currentModelDetail);
  const pickerSubmenus: readonly ModelPickerSection[] = fastModeAvailable
    ? ['model', 'effort', 'speed']
    : ['model', 'effort'];
  const reasoningControls = bridge.desktop.conversationControls?.reasoning;
  const reasoningOptions = reasoningControls?.spec.options ?? [];
  const selectedReasoning = reasoningControls?.effective ?? reasoningControls?.requested;
  const providerCredentials = bridge.bootstrap?.providerCredentials;

  const commandContext: DesktopCommandContext = useMemo(() => ({
    setModel: (model) => bridge.setModel(model),
    knownModel: (model) => bridge.desktop.models.includes(model),
    setPermissionMode: (mode) => bridge.setPermissionMode(mode),
    setReasoningLevel: (id) => bridge.setReasoningSelection({ type: 'level', id }),
    setReasoningAutomatic: () => bridge.setReasoningSelection({ type: 'automatic' }),
    setReasoningDisabled: () => bridge.setReasoningSelection({ type: 'disabled' }),
    setFastMode: (enabled) => bridge.setFastMode(enabled),
    fastMode: () => bridge.desktop.fastMode,
    setTheme: (theme) => onSetTheme(theme),
    openModelPicker: (section) => { setModelOpen(true); setModelSubmenu(section); },
    openPermissionPicker: () => setPermissionOpen(true),
    openSettings: onOpenSettings,
    emit: (output, isError) => bridge.emitCommandOutput(output, isError === true),
  }), [bridge, onOpenSettings, onSetTheme]);

  const slashMenuOpen = slashQuery !== null && ready;

  const fileMenuOpen = Boolean(filePicker && ready);

  useEffect(() => {
    if (ready) return;
    setPermissionOpen(false);
    setModelOpen(false);
    setModelSubmenu(null);
  }, [ready]);

  useEffect(() => {
    if (!modelOpen) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (modelControl.current?.contains(event.target as Node)) return;
      setModelOpen(false);
      setModelSubmenu(null);
    };
    document.addEventListener('pointerdown', closeOnOutsidePointer);
    return () => document.removeEventListener('pointerdown', closeOnOutsidePointer);
  }, [modelOpen]);

  useEffect(() => {
    setSlashResultIndex((index) => reconcileSlashSelectionIndex(
      index,
      slashQuery,
      slashQuery ?? '',
      slashCommands.length,
    ));
  }, [slashCommands.length, slashQuery]);

  useEffect(() => {
    if (!slashMenuOpen) return;
    slashControl.current
      ?.querySelector<HTMLElement>(`[data-slash-index="${slashResultIndex}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [slashMenuOpen, slashResultIndex]);

  useEffect(() => {
    if (!fileMenuOpen || !filePicker) {
      fileSearchRequest.current += 1;
      setFileSearchStatus('idle');
      setFileResults([]);
      setFileResultsTruncated(false);
      setFileResultIndex(0);
      return;
    }
    const request = ++fileSearchRequest.current;
    setFileSearchStatus('loading');
    const timer = window.setTimeout(() => {
      void bridge.searchWorkspaceFiles(filePicker.query)
        .then((result) => {
          if (fileSearchRequest.current !== request) return;
          setFileResults(result.files);
          setFileResultsTruncated(result.truncated);
          setFileResultIndex(0);
          setFileSearchStatus('ready');
        })
        .catch(() => {
          if (fileSearchRequest.current !== request) return;
          setFileResults([]);
          setFileResultsTruncated(false);
          setFileResultIndex(0);
          setFileSearchStatus('error');
        });
    }, 90);
    return () => window.clearTimeout(timer);
  }, [bridge.searchWorkspaceFiles, fileMenuOpen, filePicker?.query]);

  useEffect(() => {
    if (!fileMenuOpen) return;
    const pointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !fileControl.current?.contains(event.target)) {
        setFilePicker(null);
      }
    };
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setFilePicker(null);
      if (document.activeElement === fileSearchInput.current) input.current?.focus();
    };
    document.addEventListener('pointerdown', pointerDown);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', pointerDown);
      document.removeEventListener('keydown', escape);
    };
  }, [fileMenuOpen]);

  useEffect(() => {
    if (fileMenuOpen && filePicker?.source === 'button') fileSearchInput.current?.focus();
  }, [fileMenuOpen, filePicker?.source]);

  useEffect(() => () => {
    recognition.current?.stop();
    recognition.current = null;
  }, []);

  useEffect(() => () => {
    const previewUrls = new Set(imageAttachmentsRef.current.map((attachment) => attachment.previewUrl));
    for (const draft of draftsBySession.current.values()) {
      for (const attachment of draft.images) previewUrls.add(attachment.previewUrl);
    }
    previewUrls.forEach((previewUrl) => URL.revokeObjectURL(previewUrl));
    draftsBySession.current.clear();
  }, []);

  useEffect(() => {
    if (!permissionOpen) return;
    const pointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !permissionControl.current?.contains(event.target)) {
        setPermissionOpen(false);
      }
    };
    const keyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      setPermissionOpen(false);
      permissionButton.current?.focus();
    };
    document.addEventListener('pointerdown', pointerDown);
    document.addEventListener('keydown', keyDown);
    return () => {
      document.removeEventListener('pointerdown', pointerDown);
      document.removeEventListener('keydown', keyDown);
    };
  }, [permissionOpen]);

  useEffect(() => {
    if (!slashMenuOpen) return;
    const pointerDown = (event: PointerEvent) => {
      if (
        event.target instanceof Node
        && !slashControl.current?.contains(event.target)
        && !input.current?.contains(event.target)
      ) {
        slashDismissed.current = true;
        setSlashQuery(null);
        activeSlashRange.current = null;
        activeSlashQuery.current = null;
      }
    };
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      slashDismissed.current = true;
      setSlashQuery(null);
      activeSlashRange.current = null;
      activeSlashQuery.current = null;
      input.current?.focus();
    };
    document.addEventListener('pointerdown', pointerDown);
    document.addEventListener('keydown', escape);
    return () => {
      document.removeEventListener('pointerdown', pointerDown);
      document.removeEventListener('keydown', escape);
    };
  }, [slashMenuOpen]);

  const syncPromptState = () => {
    const editor = input.current;
    if (!editor) return { text: '', files: [] };
    const snapshot = richPromptSnapshot(editor);
    setText(snapshot.text);
    setSelectedFiles((current) => (
      current.length === snapshot.files.length && current.every((path, index) => path === snapshot.files[index])
        ? current
        : snapshot.files
    ));
    return snapshot;
  };

  useLayoutEffect(() => {
    const editor = input.current;
    if (!editor) return;

    const previousSessionId = draftSessionId.current;
    if (previousSessionId) {
      const snapshot = richPromptSnapshot(editor);
      const images = imageAttachmentsRef.current;
      if (snapshot.text || snapshot.files.length || images.length) {
        draftsBySession.current.set(previousSessionId, {
          ...snapshot,
          html: editor.innerHTML,
          images: [...images],
        });
      } else {
        draftsBySession.current.delete(previousSessionId);
      }
    }

    draftSessionId.current = activeSessionId;
    const draft = activeSessionId ? draftsBySession.current.get(activeSessionId) : undefined;
    editor.innerHTML = draft?.html ?? '';
    setText(draft?.text ?? '');
    setSelectedFiles(draft ? [...draft.files] : []);
    const images = draft ? [...draft.images] : [];
    imageAttachmentsRef.current = images;
    setImageAttachments(images);
    setImageNotice(null);
    setFilePicker(null);
    setSlashQuery(null);
    activeMentionRange.current = null;
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    slashDismissed.current = false;
  }, [activeSessionId]);

  const savePromptSelection = () => {
    const editor = input.current;
    if (!editor) return;
    savedEditorSelection.current = editorSelection(editor);
  };

  const updateActiveCompletions = () => {
    const editor = input.current;
    const selection = window.getSelection();
    if (!editor || !selection?.rangeCount || !editor.contains(selection.focusNode)) return;
    savedEditorSelection.current = selection.getRangeAt(0).cloneRange();

    const node = selection.focusNode;
    const mention = node?.nodeType === Node.TEXT_NODE
      ? activeFileMention(node.nodeValue ?? '', selection.focusOffset)
      : null;
    if (mention && node) {
      const range = document.createRange();
      range.setStart(node, mention.start);
      range.setEnd(node, mention.end);
      activeMentionRange.current = range;
      setFilePicker((current) => (
        current?.source === 'mention' && current.query === mention.query
          ? current
          : { source: 'mention', query: mention.query }
      ));
      setSlashQuery(null);
      activeSlashRange.current = null;
      activeSlashQuery.current = null;
      return;
    }
    activeMentionRange.current = null;
    setFilePicker((current) => current?.source === 'mention' ? null : current);

    const slash = node?.nodeType === Node.TEXT_NODE
      ? activeSlashCommand(node.nodeValue ?? '', selection.focusOffset)
      : null;
    if (slash && node) {
      if (slashDismissed.current) return;
      const range = document.createRange();
      range.setStart(node, slash.start);
      range.setEnd(node, slash.end);
      activeSlashRange.current = range;
      const previousQuery = activeSlashQuery.current;
      activeSlashQuery.current = slash.query;
      setSlashQuery((current) => current === slash.query ? current : slash.query);
      setSlashResultIndex((index) => reconcileSlashSelectionIndex(
        index,
        previousQuery,
        slash.query,
        slashCommands.length,
      ));
      setFilePicker(null);
      setModelOpen(false);
      setModelSubmenu(null);
      setPermissionOpen(false);
      return;
    }
    slashDismissed.current = false;
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    setSlashQuery(null);
  };

  const replaceVoiceText = (nextText: string) => {
    const editor = input.current;
    if (!editor) return;
    const mentions = [...editor.querySelectorAll<HTMLElement>(FILE_MENTION_SELECTOR)];
    editor.replaceChildren();
    for (const mention of mentions) editor.append(mention, document.createTextNode(ZERO_WIDTH_SPACE));
    if (nextText) editor.append(document.createTextNode(nextText));
    const range = editorSelection(editor);
    range.selectNodeContents(editor);
    range.collapse(false);
    savedEditorSelection.current = range;
    setText(nextText);
  };

  const stopVoice = () => {
    recognition.current?.stop();
    recognition.current = null;
    setVoiceState('idle');
  };

  const toggleVoice = () => {
    if (voiceState === 'listening') {
      stopVoice();
      return;
    }
    const SpeechRecognition = speechRecognitionConstructor();
    if (!SpeechRecognition) {
      setVoiceState('unsupported');
      return;
    }
    const next = new SpeechRecognition();
    voiceBase.current = text.trimEnd();
    next.continuous = true;
    next.interimResults = true;
    next.lang = typeof navigator !== 'undefined' && navigator.language ? navigator.language : 'en-US';
    next.onresult = (event) => {
      const transcript = Array.from(event.results, (result) => result[0]?.transcript ?? '').join('');
      const prefix = voiceBase.current;
      replaceVoiceText(`${prefix}${prefix && transcript ? ' ' : ''}${transcript}`);
    };
    next.onerror = (event) => {
      setVoiceState(event.error === 'not-allowed' || event.error === 'service-not-allowed' ? 'denied' : 'idle');
      recognition.current = null;
    };
    next.onend = () => {
      recognition.current = null;
      setVoiceState((current) => current === 'listening' ? 'idle' : current);
    };
    recognition.current = next;
    setVoiceState('listening');
    try {
      next.start();
    } catch {
      recognition.current = null;
      setVoiceState('idle');
    }
  };

  const toggleStandardVoice = () => {
    if (flowMode) setFlowMode(false);
    toggleVoice();
  };

  const toggleFlowMode = () => {
    if (flowMode) {
      stopVoice();
      setFlowMode(false);
      return;
    }
    setFlowMode(true);
    if (voiceState !== 'listening') toggleVoice();
  };

  const addImageFiles = async (files: File[]) => {
    if (!ready) return;
    const remaining = MAX_IMAGE_ATTACHMENTS - imageAttachments.length;
    if (remaining <= 0) {
      setImageNotice(`最多添加 ${MAX_IMAGE_ATTACHMENTS} 张图片。`);
      return;
    }
    const candidates = files.slice(0, remaining);
    const results = await Promise.all(candidates.map(async (file) => {
      try {
        return { attachment: await imageFileToAttachment(file) };
      } catch (error) {
        return { error: error instanceof Error ? error.message : '无法读取图片。' };
      }
    }));
    const attachments = results.flatMap((result) => result.attachment ? [result.attachment] : []);
    const firstError = results.find((result) => result.error)?.error;
    if (files.length > candidates.length) {
      setImageNotice(`最多添加 ${MAX_IMAGE_ATTACHMENTS} 张图片。`);
    } else if (firstError) {
      setImageNotice(firstError);
    } else if (attachments.length > 0) {
      setImageNotice(null);
    }
    if (!attachments.length) return;
    setImageAttachments((current) => {
      const available = Math.max(0, MAX_IMAGE_ATTACHMENTS - current.length);
      const accepted = attachments.slice(0, available);
      attachments.slice(available).forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
      return [...current, ...accepted];
    });
  };

  const removeImage = (id: string) => {
    setImageAttachments((current) => {
      const removed = current.find((attachment) => attachment.id === id);
      if (removed) URL.revokeObjectURL(removed.previewUrl);
      return current.filter((attachment) => attachment.id !== id);
    });
  };

  const clearComposer = (sessionId = draftSessionId.current) => {
    const draft = sessionId ? draftsBySession.current.get(sessionId) : undefined;
    if (sessionId) draftsBySession.current.delete(sessionId);
    if (sessionId !== draftSessionId.current) {
      draft?.images.forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
      return;
    }
    input.current?.replaceChildren();
    setText('');
    setSelectedFiles([]);
    setImageAttachments((current) => {
      current.forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
      return [];
    });
    setImageNotice(null);
    setFilePicker(null);
    setSlashQuery(null);
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    slashDismissed.current = false;
  };

  const submit = async () => {
    const snapshot = input.current ? richPromptSnapshot(input.current) : { text, files: selectedFiles };
    const value = promptWithFileMentions(snapshot.text, snapshot.files);
    if (!ready) return;
    if (!value) {
      if (imageAttachments.length) setImageNotice('请先输入问题，再发送图片。');
      return;
    }
    const slashCommand = snapshot.files.length === 0 ? snapshot.text.trim() : '';
    const isSlashCommand = /^\/[^\s/]+(?:\s|$)/.test(slashCommand);
    if (bridge.running && isSlashCommand) {
      setImageNotice('当前任务完成后才能运行 / 命令。');
      return;
    }
    if (bridge.running && imageAttachments.length) {
      setImageNotice('Pending message 暂不支持图片，请等待当前任务完成后发送。');
      return;
    }
    if (isSlashCommand && imageAttachments.length) {
      setImageNotice('图片附件不能和 / 命令一起发送，请先输入普通问题。');
      return;
    }
    if (voiceState === 'listening') stopVoice();
    if (isSlashCommand) {
      clearComposer();
      const resolved = resolveDesktopCommand(slashCommand, DESKTOP_COMMANDS);
      if (resolved) {
        bridge.beginLocalCommand(slashCommand);
        void Promise.resolve(resolved.command.run(resolved.args, commandContext)).catch(() => undefined);
        return;
      }
      invoke(() => bridge.runSlashCommand(slashCommand));
      return;
    }
    const images: ImageRefDto[] = imageAttachments.map(({ media_type, base64 }) => ({ media_type, base64 }));
    const submittingSessionId = draftSessionId.current;
    try {
      await bridge.sendPrompt(value, images);
      clearComposer(submittingSessionId);
    } catch {
      setImageNotice('发送失败，图片附件已保留，可以重试。');
    }
  };

  const chooseSlashCommand = (name: string) => {
    const editor = input.current;
    if (!editor) {
      setSlashQuery(null);
      return;
    }
    const insertion = activeSlashRange.current?.cloneRange() ?? editorSelection(editor);
    insertion.deleteContents();
    const command = document.createTextNode(slashCommandText(name));
    insertion.insertNode(command);
    const caret = document.createRange();
    caret.setStart(command, command.length);
    caret.collapse(true);
    applyEditorSelection(caret);
    savedEditorSelection.current = caret.cloneRange();
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    setSlashQuery(null);
    // Keep the completed token closed through the matching keyup; typing any
    // new character clears this guard in onInput.
    slashDismissed.current = true;
    setSlashResultIndex(0);
    syncPromptState();
    focusPrompt(caret);
  };

  const focusPrompt = (range?: Range | null) => {
    window.requestAnimationFrame(() => {
      const editor = input.current;
      if (!editor) return;
      editor.focus();
      applyEditorSelection(range ?? savedEditorSelection.current ?? editorSelection(editor));
    });
  };

  const chooseFile = (path: string) => {
    const editor = input.current;
    if (!filePicker || !editor) return;
    const range = filePicker.source === 'mention'
      ? activeMentionRange.current
      : savedEditorSelection.current;
    const insertion = range?.cloneRange() ?? editorSelection(editor);
    if (filePicker.source === 'mention') insertion.deleteContents();

    const duplicate = [...editor.querySelectorAll<HTMLElement>(FILE_MENTION_SELECTOR)]
      .some((token) => token.dataset.fileMention === path);
    let caret = insertion;
    if (!duplicate) {
      const token = createFileMention(path, t.accent);
      const cursorNode = document.createTextNode(ZERO_WIDTH_SPACE);
      const fragment = document.createDocumentFragment();
      fragment.append(token, cursorNode);
      insertion.insertNode(fragment);
      caret = document.createRange();
      caret.setStart(cursorNode, 1);
      caret.collapse(true);
    } else {
      caret.collapse(true);
    }
    applyEditorSelection(caret);
    savedEditorSelection.current = caret.cloneRange();
    activeMentionRange.current = null;
    syncPromptState();
    setFilePicker(null);
    setFileResults([]);
    focusPrompt(caret);
  };

  const openFileMenu = () => {
    if (filePicker?.source === 'button') {
      setFilePicker(null);
      input.current?.focus();
      return;
    }
    setSlashQuery(null);
    activeSlashRange.current = null;
    setFilePicker({ source: 'button', query: '' });
    setModelOpen(false);
    setModelSubmenu(null);
    setPermissionOpen(false);
  };

  const filePickerKeyDown = (event: KeyboardEvent<HTMLInputElement | HTMLDivElement>) => {
    if (fileMenuOpen) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        setFileResultIndex((index) => fileResults.length ? (index + 1) % fileResults.length : 0);
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        setFileResultIndex((index) => fileResults.length ? (index - 1 + fileResults.length) % fileResults.length : 0);
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        setFilePicker(null);
        input.current?.focus();
        return;
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault();
        const selected = fileResults[fileResultIndex];
        if (selected) chooseFile(selected);
        else setFilePicker(null);
        return;
      }
    }
  };
  const slashPickerKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (!slashMenuOpen) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      slashDismissed.current = true;
      setSlashQuery(null);
      activeSlashRange.current = null;
      activeSlashQuery.current = null;
      return;
    }
    const direction = slashNavigationDirection(event.key);
    if (direction) {
      event.preventDefault();
      setSlashResultIndex((index) => moveSlashSelectionIndex(index, direction, slashCommands.length));
      return;
    }
    if (event.key === 'Enter' || event.key === 'Tab') {
      event.preventDefault();
      const selected = slashCommands[slashResultIndex];
      if (selected) chooseSlashCommand(selected.name);
      else {
        setSlashQuery(null);
        activeSlashRange.current = null;
      }
    }
  };
  const keyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    filePickerKeyDown(event);
    if (slashMenuOpen) {
      slashPickerKeyDown(event);
      if (event.defaultPrevented) return;
    }
    if (event.defaultPrevented) return;
    if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void submit();
    }
  };
  const keyUp = (event: KeyboardEvent<HTMLDivElement>) => {
    // Arrow navigation changes only the active palette row. Re-running caret
    // detection on the matching keyup can reconcile against stale query state
    // and overwrite the index selected during keydown.
    if (slashMenuOpen && slashNavigationDirection(event.key)) return;
    updateActiveCompletions();
  };

  const pastePlainText = (event: ClipboardEvent<HTMLDivElement>) => {
    const clipboardImages = [...event.clipboardData.items]
      .filter((item) => item.kind === 'file' && (item.type.startsWith('image/') || item.type === ''))
      .map((item) => item.getAsFile())
      .filter((file): file is File => Boolean(file));
    if (clipboardImages.length) {
      event.preventDefault();
      void addImageFiles(clipboardImages);
      return;
    }
    event.preventDefault();
    const editor = input.current;
    if (!editor) return;
    const range = editorSelection(editor);
    range.deleteContents();
    const value = event.clipboardData.getData('text/plain');
    const node = document.createTextNode(value);
    range.insertNode(node);
    range.setStart(node, node.length);
    range.collapse(true);
    applyEditorSelection(range);
    savedEditorSelection.current = range.cloneRange();
    syncPromptState();
    updateActiveCompletions();
  };
  const permissionMode = PERM_MODES.find((mode) => mode.id === bridge.desktop.permissionMode) ?? PERM_MODES[0]!;
  const workspace = bridge.bootstrap?.workspace;
  const providerConfigured = bridge.bootstrap?.providerCredentials?.some((entry) => entry.configured) ?? false;
  const promptPlaceholder = !ready
    ? !workspace?.path || workspace.recovery
      ? 'Add or select an available project to start coding…'
      : !providerConfigured
        ? 'Connect a provider in Settings to start coding…'
        : 'Waiting for the local engine…'
    : goalMode
        ? 'Describe the goal you want LingXi to accomplish'
        : bridge.running
          ? 'LingXi is working — draft your next message…'
          : 'Do anything';
  const hasPrompt = Boolean(text.trim() || selectedFiles.length);
  return (
    <div style={{ flexShrink: 0, padding: '10px 18px 18px', background: t.stageBg }}>
      {flowMode && (
        <div
          role="status"
          aria-label="Flow mode is listening"
          style={{
            position: 'relative',
            maxWidth: 980,
            height: 156,
            margin: '0 auto 10px',
            overflow: 'hidden',
            borderRadius: 22,
            border: `1px solid ${t.accentBorder}`,
            background: `radial-gradient(circle at 50% 48%, ${t.accentBg} 0%, ${t.surface} 72%)`,
            boxShadow: '0 14px 36px rgba(0,0,0,.10)',
          }}
        >
          <div style={{ position: 'absolute', left: 16, top: 13, display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ width: 7, height: 7, borderRadius: 99, background: t.accent, boxShadow: `0 0 12px ${t.accent}` }} />
            <span style={{ color: t.text, fontSize: 12.5, fontWeight: 650 }}>心流模式</span>
          </div>
          <button
            type="button"
            aria-label="关闭心流模式"
            title="关闭心流模式"
            onClick={toggleFlowMode}
            style={{ ...composerIconStyle(t), position: 'absolute', right: 12, top: 9, width: 32, height: 32, background: t.surfaceHover }}
          >
            <Icon name="x" size={13} color={t.text3} stroke={1.9} />
          </button>
          <svg
            aria-hidden="true"
            viewBox="0 0 180 64"
            style={{ position: 'absolute', left: '50%', top: '48%', width: 210, height: 74, transform: 'translate(-50%, -50%)', color: t.accent }}
          >
            {[12, 23, 34, 48, 34, 23, 12].map((height, index) => (
              <rect key={index} x={27 + index * 20} y={(64 - height) / 2} width="7" height={height} rx="3.5" fill="currentColor" opacity={0.5 + index * 0.06}>
                <animate attributeName="height" values={`${height};${Math.max(12, 58 - Math.abs(3 - index) * 8)};${height}`} dur={`${1.05 + index * 0.09}s`} repeatCount="indefinite" />
                <animate attributeName="y" values={`${(64 - height) / 2};${(64 - Math.max(12, 58 - Math.abs(3 - index) * 8)) / 2};${(64 - height) / 2}`} dur={`${1.05 + index * 0.09}s`} repeatCount="indefinite" />
              </rect>
            ))}
          </svg>
          <div style={{ position: 'absolute', left: 0, right: 0, bottom: 13, textAlign: 'center', color: t.text3, fontSize: 11.5 }}>
            {voiceState === 'listening' ? '正在聆听 · 可继续使用下方输入框' : '轻点波形按钮继续聆听'}
          </div>
        </div>
      )}
      <div
        className="beta-composer"
        onDragOver={(event) => {
          if (!ready || !event.dataTransfer.types.includes('Files')) return;
          event.preventDefault();
          event.dataTransfer.dropEffect = 'copy';
          setImageDragActive(true);
        }}
        onDragLeave={(event) => {
          if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setImageDragActive(false);
        }}
        onDrop={(event) => {
          event.preventDefault();
          setImageDragActive(false);
          void addImageFiles([...event.dataTransfer.files]);
        }}
        style={{ position: 'relative', maxWidth: 980, margin: '0 auto', borderRadius: 26, border: `1px solid ${imageDragActive ? t.accent : ready ? t.borderStrong : t.border}`, background: imageDragActive ? t.accentBg : t.surface, boxShadow: '0 12px 34px rgba(0,0,0,.10)', overflow: 'visible', transition: 'border-color 0.16s ease, background-color 0.16s ease' }}
      >
        {imageAttachments.length > 0 && (
          <div aria-label="Image attachments" style={{ display: 'flex', flexWrap: 'wrap', gap: 8, padding: '12px 18px 2px' }}>
            {imageAttachments.map((attachment) => (
              <div key={attachment.id} title={attachment.name} style={{ position: 'relative', width: 74, height: 62, overflow: 'hidden', borderRadius: 10, background: t.surfaceHover, outline: `1px solid color-mix(in oklab, ${t.borderStrong} 55%, transparent)` }}>
                <img src={attachment.previewUrl} alt={attachment.name} style={{ display: 'block', width: '100%', height: '100%', objectFit: 'cover', outline: `1px solid color-mix(in oklab, ${t.text} 10%, transparent)`, outlineOffset: -1 }} />
                <button type="button" aria-label={`Remove ${attachment.name}`} title="Remove image" onClick={() => removeImage(attachment.id)} style={{ position: 'absolute', top: 3, right: 3, width: 24, height: 24, display: 'grid', placeItems: 'center', padding: 0, border: 0, borderRadius: 99, background: 'rgba(0,0,0,.62)', color: '#fff', cursor: 'pointer' }}><Icon name="x" size={11} stroke={2} /></button>
              </div>
            ))}
          </div>
        )}
        {imageNotice && <div role="status" style={{ padding: '0 18px 9px', color: t.warn, fontSize: 10.5 }}>{imageNotice}</div>}
        <div
          ref={input}
          className="beta-rich-prompt"
          role="textbox"
          contentEditable={ready}
          suppressContentEditableWarning
          spellCheck
          data-placeholder={promptPlaceholder}
          data-empty={!hasPrompt ? 'true' : 'false'}
          onInput={() => { slashDismissed.current = false; setImageNotice(null); syncPromptState(); updateActiveCompletions(); }}
          onFocus={() => { slashDismissed.current = false; savePromptSelection(); updateActiveCompletions(); }}
          onBlur={savePromptSelection}
          onKeyUp={keyUp}
          onMouseUp={updateActiveCompletions}
          onKeyDown={keyDown}
          onPaste={pastePlainText}
          aria-label="Prompt"
          aria-multiline="true"
          aria-disabled={!ready}
          aria-autocomplete="list"
          aria-controls={slashMenuOpen ? 'slash-command-results' : fileMenuOpen && filePicker?.source === 'mention' ? 'workspace-file-results' : undefined}
          aria-expanded={slashMenuOpen || (fileMenuOpen && filePicker?.source === 'mention')}
          aria-activedescendant={slashMenuOpen && slashCommands[slashResultIndex]
            ? `slash-command-result-${slashResultIndex}`
            : fileMenuOpen && filePicker?.source === 'mention' && fileResults[fileResultIndex]
              ? `workspace-file-result-${fileResultIndex}`
              : undefined}
          style={{ display: 'block', width: '100%', minHeight: 86, maxHeight: 180, overflowY: 'auto', border: 0, outline: 0, background: 'transparent', color: t.text, lineHeight: 1.5, fontSize: 16.5, padding: '18px 22px 4px', fontWeight: 450, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', cursor: ready ? 'text' : 'default', opacity: ready ? 1 : .68 }}
        />
        {slashMenuOpen && (
          <div ref={slashControl} id="slash-command-results" role="listbox" aria-label="Slash commands" style={{ ...composerMenuStyle(t, 'left'), width: 600, maxWidth: 'min(600px, calc(100vw - 44px))', maxHeight: 300, overflowY: 'auto', padding: 7 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '4px 9px 7px', borderBottom: `0.5px solid ${t.border}`, color: t.text3, fontSize: 10.5 }}>
              <strong style={{ color: t.text2, fontWeight: 700, letterSpacing: '.08em', textTransform: 'uppercase' }}>Commands</strong>
              <span className="mono" style={{ color: t.accent }}>/{slashQuery}</span>
              <span style={{ marginLeft: 'auto', color: t.text4 }}>{slashCommands.length} match{slashCommands.length === 1 ? '' : 'es'}</span>
            </div>
            {slashCommands.length === 0 && (
              <div role="status" style={{ padding: '16px 10px', color: t.text3, fontSize: 11.5 }}>
                No matching commands. Press Esc to keep the text as a prompt.
              </div>
            )}
            {slashCommands.map((entry, index) => {
              const selected = index === slashResultIndex;
              return (
                <button
                  id={`slash-command-result-${index}`}
                  data-slash-index={index}
                  key={entry.name}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseEnter={() => setSlashResultIndex(index)}
                  onClick={() => chooseSlashCommand(entry.name)}
                  style={{ width: '100%', display: 'grid', gridTemplateColumns: 'minmax(92px, auto) minmax(0, 1fr) auto', gap: 10, alignItems: 'center', padding: '8px 9px', border: 0, borderRadius: 7, background: selected ? t.accentBg : 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit' }}
                >
                  <span
                    className="mono"
                    style={{ color: t.accent, fontWeight: 650, borderRadius: 6, padding: '2px 0', fontSize: 11.5 }}
                  >
                    /{entry.name}
                    {entry.argument_hint ? <span style={{ color: t.text4, fontWeight: 400 }}> {entry.argument_hint}</span> : null}
                  </span>
                  <span style={{ color: t.text2, fontSize: 12.5 }}>{slashMenuLabel(entry)}</span>
                  <span style={{ color: t.text4, fontSize: 10, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{entry.source}</span>
                </button>
              );
            })}
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 26, padding: '5px 9px 2px', borderTop: `0.5px solid ${t.border}`, color: t.text4, fontSize: 9.5 }}>
              <span>↑↓ Navigate</span><span>Enter / Tab Complete</span><span>Esc Close</span>
            </div>
          </div>
        )}
        <div style={{ display: 'flex', alignItems: 'center', gap: 6, minHeight: 54, padding: '0 10px 10px 14px' }}>
          <input ref={imageFileInput} type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple onChange={(event) => { void addImageFiles(event.target.files ? [...event.target.files] : []); event.currentTarget.value = ''; }} style={{ display: 'none' }} />
          <button type="button" disabled={!ready} aria-label="Attach image" title="Attach image" onClick={() => imageFileInput.current?.click()} style={{ ...composerIconStyle(t), width: 34, height: 34 }}><Icon name="image" size={19} color={t.text2} stroke={1.7} /></button>
          <div ref={fileControl}>
            <button type="button" disabled={!ready} aria-label="Search workspace files" aria-expanded={fileMenuOpen} title="Add file context (@)" onMouseDown={savePromptSelection} onClick={openFileMenu} style={{ ...composerIconStyle(t), width: 34, height: 34 }}><Icon name="plus" size={21} color={t.text2} stroke={1.7} /></button>
            {fileMenuOpen && (
              <div role="dialog" aria-label="Search workspace files" style={{ ...composerMenuStyle(t, 'left'), width: 560, maxWidth: 'min(560px, calc(100vw - 44px))', padding: 7, overflow: 'hidden' }}>
                <div style={{ display: 'flex', alignItems: 'center', gap: 7, padding: '3px 4px 7px', borderBottom: `0.5px solid ${t.border}` }}>
                  <Icon name="search" size={14} color={t.text3} />
                  <input
                    ref={fileSearchInput}
                    type="text"
                    role="searchbox"
                    value={filePicker?.query ?? ''}
                    onChange={(event) => setFilePicker((current) => current ? { ...current, query: event.target.value } : current)}
                    onKeyDown={filePickerKeyDown}
                    placeholder="Search workspace files"
                    aria-label="File search query"
                    aria-controls="workspace-file-results"
                    aria-activedescendant={fileResults[fileResultIndex] ? `workspace-file-result-${fileResultIndex}` : undefined}
                    style={{ minWidth: 0, flex: 1, height: 30, padding: '0 3px', border: 0, outline: 0, background: 'transparent', color: t.text, font: 'inherit', fontSize: 12.5 }}
                  />
                  <span className="mono" style={{ color: t.text4, fontSize: 9.5 }}>@ file</span>
                  {filePicker?.query && <button type="button" aria-label="Clear file search" title="Clear search" onClick={() => { setFilePicker((current) => current ? { ...current, query: '' } : current); fileSearchInput.current?.focus(); }} style={{ width: 26, height: 26, display: 'grid', placeItems: 'center', padding: 0, border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={11} /></button>}
                  <button type="button" aria-label="Close file search" title="Close" onClick={() => { setFilePicker(null); input.current?.focus(); }} style={{ width: 26, height: 26, display: 'grid', placeItems: 'center', padding: 0, border: 0, borderRadius: 6, background: t.surfaceHover, color: t.text2, cursor: 'pointer' }}><Icon name="x" size={13} /></button>
                </div>
                <div id="workspace-file-results" role="listbox" aria-label="Workspace files" style={{ maxHeight: 310, overflowY: 'auto', padding: '5px 0' }}>
                  {fileSearchStatus === 'loading' && <div role="status" style={{ padding: '14px 10px', color: t.text3, fontSize: 11.5 }}>Searching workspace…</div>}
                  {fileSearchStatus === 'error' && <div role="alert" style={{ padding: '14px 10px', color: t.danger, fontSize: 11.5 }}>Could not search this workspace.</div>}
                  {fileSearchStatus === 'ready' && fileResults.length === 0 && <div role="status" style={{ padding: '14px 10px', color: t.text3, fontSize: 11.5 }}>No matching files.</div>}
                  {fileResults.map((path, index) => {
                    const slash = path.lastIndexOf('/');
                    const directory = slash >= 0 ? path.slice(0, slash) : 'workspace root';
                    const selected = index === fileResultIndex;
                    return (
                      <button
                        id={`workspace-file-result-${index}`}
                        key={path}
                        type="button"
                        role="option"
                        aria-selected={selected}
                        onMouseDown={(event) => event.preventDefault()}
                        onMouseEnter={() => setFileResultIndex(index)}
                        onClick={() => chooseFile(path)}
                        style={{ width: '100%', display: 'grid', gridTemplateColumns: '24px minmax(0, 1fr)', gap: 8, alignItems: 'center', padding: '7px 9px', border: 0, borderRadius: 7, background: selected ? t.accentBg : 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit' }}
                      >
                        <Icon name="file" size={15} color={selected ? t.accent : t.text3} />
                        <span style={{ minWidth: 0 }}>
                          <span style={{ display: 'block', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 12.5, fontWeight: 570 }}>{basename(path)}</span>
                          <span className="mono" style={{ display: 'block', marginTop: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 9.5 }}>{directory}</span>
                        </span>
                      </button>
                    );
                  })}
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 30, padding: '5px 9px 2px', borderTop: `0.5px solid ${t.border}`, color: t.text4, fontSize: 9.5 }}>
                  <span>↑↓ Navigate</span><span>Enter / Tab Add</span><span>Esc Close</span>
                  {fileResultsTruncated && <span style={{ marginLeft: 'auto' }}>More matches available — keep typing</span>}
                </div>
              </div>
            )}
          </div>
          <div ref={permissionControl} style={{ position: 'relative' }}>
            <button
              ref={permissionButton}
              type="button"
              disabled={!ready || bridge.running}
              aria-haspopup="menu"
              aria-expanded={permissionOpen}
              aria-label={`Permission mode: ${permissionMode.label}`}
              title="Change permission mode"
              onMouseDown={() => { setSlashQuery(null); activeSlashRange.current = null; }}
              onClick={() => { setPermissionOpen((open) => !open); setModelOpen(false); setModelSubmenu(null); }}
              style={{
                ...composerPillStyle(t, permissionOpen),
                color: permissionMode.danger ? t.danger : permissionOpen ? t.accent : t.text2,
                background: permissionOpen ? t.accentBg : permissionMode.danger ? `color-mix(in oklab, ${t.danger} 9%, transparent)` : 'transparent',
              }}
            >
              <Icon name={permissionMode.icon} size={18} color="currentColor" stroke={1.65} />
              <span>{permissionMode.shortLabel}</span>
              <Icon name="chevron" size={12} color="currentColor" stroke={1.8} />
            </button>
            {ready && permissionOpen && (
              <div
                style={{ ...composerMenuStyle(t, 'left'), width: 480, maxWidth: 'min(480px, calc(100vw - 44px))', maxHeight: 'min(470px, calc(100vh - 150px))', overflowY: 'auto', padding: 9 }}
                role="menu"
                aria-label="Permission modes"
              >
                <div style={{ padding: '4px 9px 8px', display: 'flex', alignItems: 'baseline', gap: 9 }}>
                  <strong style={{ color: t.text, fontSize: 12.5, fontWeight: 650 }}>How should LingXi actions be approved?</strong>
                  <span style={{ marginLeft: 'auto', color: t.text4, fontSize: 10.5 }}>Current session</span>
                </div>
                {PERM_MODES.map((mode) => {
                  const selected = mode.id === bridge.desktop.permissionMode;
                  const color = mode.danger ? t.danger : selected ? t.accent : t.text2;
                  return (
                    <button
                      key={mode.id}
                      type="button"
                      role="menuitemradio"
                      aria-checked={selected}
                      onClick={() => {
                        if (!selected) invoke(() => bridge.setPermissionMode(mode.id));
                        setPermissionOpen(false);
                      }}
                      style={{
                        width: '100%', display: 'grid', gridTemplateColumns: '28px minmax(0, 1fr) 18px',
                        alignItems: 'center', gap: 9, padding: '8px 9px', border: 0, borderRadius: 9,
                        background: selected ? t.accentBg : 'transparent', color, textAlign: 'left',
                        cursor: 'pointer', font: 'inherit',
                      }}
                    >
                      <span style={{ width: 28, height: 28, display: 'grid', placeItems: 'center', color }}>
                        <Icon name={mode.icon} size={19} color="currentColor" stroke={1.65} />
                      </span>
                      <span style={{ minWidth: 0 }}>
                        <span style={{ display: 'block', color, fontSize: 13, fontWeight: 570, lineHeight: 1.25 }}>{mode.label}</span>
                        <span style={{ display: 'block', marginTop: 2, color: mode.danger ? t.danger : t.text3, fontSize: 11, lineHeight: 1.35 }}>{mode.description}</span>
                      </span>
                      {selected && <Icon name="check" size={16} color={color} stroke={2.2} />}
                    </button>
                  );
                })}
              </div>
            )}
          </div>
          <span aria-hidden="true" style={{ width: 1, height: 24, background: t.border, margin: '0 4px' }} />
          <button type="button" disabled={!ready} aria-pressed={goalMode} aria-label="Toggle goal mode" onClick={() => setGoalMode((enabled) => !enabled)} style={{ ...composerPillStyle(t, goalMode), color: goalMode ? t.accent : t.text2 }}><Icon name="goal" size={18} color={goalMode ? t.accent : t.text3} stroke={1.6} /><span>Goal</span></button>

          <div style={{ flex: 1 }} />

          <div ref={modelControl} style={{ position: 'relative' }}>
            <button
              type="button"
              disabled={!ready || bridge.running || bridge.desktop.models.length === 0}
              aria-haspopup="menu"
              aria-expanded={modelOpen}
              aria-label={`${fastModeAvailable && bridge.desktop.fastMode ? 'Fast mode, ' : ''}Model: ${modelLabel(bridge.desktop.currentModel)}, reasoning ${reasoningSelectionLabel(selectedReasoning)}`}
              onMouseDown={() => { setSlashQuery(null); activeSlashRange.current = null; }}
              onClick={() => { setModelOpen((open) => !open); setModelSubmenu(null); setPermissionOpen(false); }}
              style={{ ...composerPillStyle(t, modelOpen), maxWidth: 340, color: t.text }}
            >
              {fastModeAvailable && <Icon name="bolt" size={18} color={t.text} stroke={2.1} />}
              {fastModeAvailable && bridge.desktop.fastMode && <span style={{ color: t.accent, fontSize: 11.5, fontWeight: 700 }}>Fast</span>}
              {fastModeAvailable && bridge.desktop.fastMode && <span style={{ color: t.text4 }}>·</span>}
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{modelLabel(bridge.desktop.currentModel)}</span>
              <span style={{ color: t.text4 }}>·</span>
              <span style={{ color: t.text3, fontSize: 11.5 }}>{reasoningSelectionLabel(selectedReasoning)}</span>
              <Icon name="chevron" size={14} color={t.text3} />
            </button>
            {ready && modelOpen && (
              <div
                style={{
                  ...composerMenuStyle(t, 'right'),
                  width: 340,
                  maxWidth: 'min(340px, calc(100vw - 44px))',
                  padding: 8,
                  overflow: 'visible',
                }}
                role="menu"
                aria-label="Model settings"
              >
                {pickerSubmenus.map((submenu) => {
                  const labels = { model: 'Model', effort: 'Effort', speed: 'Speed' };
                  const values = {
                    model: modelLabel(bridge.desktop.currentModel),
                    effort: reasoningSelectionLabel(selectedReasoning),
                    speed: fastModeAvailable && bridge.desktop.fastMode ? 'Fast' : 'Standard',
                  };
                  const disabled = submenu === 'effort' && !reasoningControls;
                  return (
                    <button
                      key={submenu}
                      type="button"
                      role="menuitem"
                      aria-haspopup="menu"
                      aria-expanded={modelSubmenu === submenu}
                      disabled={disabled}
                      onClick={() => setModelSubmenu((current) => current === submenu ? null : submenu)}
                      style={{
                        display: 'flex', alignItems: 'center', gap: 12, width: '100%', minHeight: 42,
                        padding: '7px 9px', border: 0, borderRadius: 8, background: modelSubmenu === submenu ? t.surfaceHover : 'transparent',
                        color: disabled ? t.text4 : t.text, textAlign: 'left', cursor: disabled ? 'not-allowed' : 'pointer', font: 'inherit',
                      }}
                      onMouseEnter={(event) => { if (modelSubmenu !== submenu && !disabled) event.currentTarget.style.background = t.surfaceHover; }}
                      onMouseLeave={(event) => { if (modelSubmenu !== submenu) event.currentTarget.style.background = 'transparent'; }}
                    >
                      <span style={{ flex: 1, minWidth: 0, fontSize: 13, fontWeight: 520 }}>{labels[submenu]}</span>
                      <span style={{ maxWidth: 180, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: disabled ? t.text4 : t.text3, fontSize: 12.5 }}>{values[submenu]}</span>
                      <Icon name="chevronR" size={14} color={disabled ? t.text4 : t.text3} stroke={1.8} />
                    </button>
                  );
                })}

                <div aria-hidden="true" style={{ height: 1, margin: '7px 9px', background: t.border }} />
                <button
                  type="button"
                  role="menuitem"
                  disabled={!ready || bridge.running}
                  onClick={() => {
                    invoke(() => bridge.setReasoningSelection({ type: 'automatic' }));
                    invoke(() => bridge.setFastMode(false));
                    setModelSubmenu(null);
                  }}
                  style={{ display: 'flex', alignItems: 'center', gap: 12, width: '100%', minHeight: 42, padding: '7px 9px', border: 0, borderRadius: 8, background: 'transparent', color: !ready || bridge.running ? t.text4 : t.text3, textAlign: 'left', cursor: !ready || bridge.running ? 'not-allowed' : 'pointer', font: 'inherit', fontSize: 13, opacity: !ready || bridge.running ? .6 : 1 }}
                  onMouseEnter={(event) => { if (ready && !bridge.running) event.currentTarget.style.background = t.surfaceHover; }}
                  onMouseLeave={(event) => { event.currentTarget.style.background = 'transparent'; }}
                >
                  <span style={{ flex: 1 }}>Reset to default</span>
                  <span aria-hidden="true" style={{ color: t.text3, fontSize: 22, fontWeight: 300, lineHeight: 1 }}>↻</span>
                </button>

                {modelSubmenu === 'model' && (
                  <div style={{ ...modelPickerSubmenuStyle(t), width: 390, maxWidth: 'min(390px, calc(100vw - 44px))', maxHeight: 'min(500px, calc(100vh - 140px))', overflow: 'hidden', display: 'flex', flexDirection: 'column' }} role="menu" aria-label="Available models">
                    <div style={{ flexShrink: 0, padding: '5px 10px 8px', color: t.text3, fontSize: 12, fontWeight: 600 }}>Model</div>
                    <div style={{ flex: '1 1 auto', minHeight: 0, overflowY: 'auto', padding: '0 3px 3px', scrollbarGutter: 'stable' }}>
                      {modelGroups.map((group, groupIndex) => {
                        const statusProviderId = group.providerId === 'builtin' ? 'anthropic' : group.providerId;
                        const metadata = statusProviderId
                          ? providerCredentials?.find((entry) => entry.providerId === statusProviderId)
                          : undefined;
                        const connectionLabel = group.providerId
                          ? providerCredentials
                            ? metadata?.configured ? 'Connected' : 'Not connected'
                            : 'Checking…'
                          : undefined;
                        const connected = metadata?.configured === true;
                        return (
                          <section key={group.providerId ?? 'unqualified'} aria-labelledby={`desktop-model-provider-${group.providerId ?? 'other'}`} style={groupIndex === 0 ? undefined : { marginTop: 5, paddingTop: 5, borderTop: `0.5px solid ${t.border}` }}>
                            <div id={`desktop-model-provider-${group.providerId ?? 'other'}`} style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '7px 10px 5px', color: t.text3, fontSize: 10.5, fontWeight: 700, letterSpacing: '.08em', textTransform: 'uppercase' }}>
                              <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{group.providerLabel}</span>
                              {connectionLabel && <span title={`${group.providerLabel}: ${connectionLabel}`} style={{ display: 'inline-flex', alignItems: 'center', gap: 5, flexShrink: 0, color: connected ? t.ok : connectionLabel === 'Checking…' ? t.text4 : t.warn, fontSize: 9.5, fontWeight: 600, letterSpacing: 0, textTransform: 'none' }}><span aria-hidden="true" style={{ width: 6, height: 6, borderRadius: 99, background: 'currentColor' }} />{connectionLabel}</span>}
                            </div>
                            {group.models.map((entry) => {
                              const active = entry.reference === bridge.desktop.currentModel;
                              const entryDetail = modelDetailsByReference.get(entry.reference);
                              const entrySupportsFastMode = modelReference(entry.reference).providerId === 'anthropic'
                                && modelSupportsFastMode(entryDetail);
                              return (
                                <button key={entry.reference} type="button" role="menuitemradio" aria-checked={active} onClick={() => { invoke(() => bridge.setModel(entry.reference)); setModelOpen(false); setModelSubmenu(null); }} style={{ display: 'flex', alignItems: 'center', gap: 9, width: '100%', minHeight: 40, padding: '7px 10px', border: 0, borderRadius: 7, background: active ? t.accentBg : 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit', fontSize: 12.5 }} onMouseEnter={(event) => { if (!active) event.currentTarget.style.background = t.surfaceHover; }} onMouseLeave={(event) => { if (!active) event.currentTarget.style.background = 'transparent'; }}>
                                  {entrySupportsFastMode && <Icon name="bolt" size={14} color={active ? t.accent : t.text3} />}
                                  <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontWeight: active ? 650 : 500 }}>{entry.label}</span>
                                  {active && <Icon name="check" size={14} color={t.accent} stroke={2.2} />}
                                </button>
                              );
                            })}
                          </section>
                        );
                      })}
                    </div>
                  </div>
                )}

                {modelSubmenu === 'effort' && (
                  <div style={{ ...modelPickerSubmenuStyle(t), width: 300, maxWidth: 'min(300px, calc(100vw - 44px))', maxHeight: 'min(430px, calc(100vh - 140px))', overflow: 'hidden' }} role="menu" aria-label="Reasoning effort">
                    <div style={{ padding: '5px 10px 8px', color: t.text3, fontSize: 12, fontWeight: 600 }}>Effort</div>
                    {reasoningOptions.length > 0 ? reasoningOptions.map((option) => {
                      const active = reasoningSelectionKey(option.selection) === reasoningSelectionKey(selectedReasoning ?? { type: 'automatic' });
                      const disabled = reasoningControls?.spec.editable === false;
                      return (
                        <button key={reasoningSelectionKey(option.selection)} type="button" role="menuitemradio" aria-checked={active} disabled={disabled} onClick={() => invoke(() => bridge.setReasoningSelection(option.selection))} style={{ display: 'flex', alignItems: 'center', gap: 9, width: '100%', minHeight: 40, padding: '7px 10px', border: 0, borderRadius: 7, background: active ? t.accentBg : 'transparent', color: disabled ? t.text4 : t.text, textAlign: 'left', cursor: disabled ? 'not-allowed' : 'pointer', font: 'inherit', fontSize: 12.5, opacity: disabled ? .65 : 1 }} onMouseEnter={(event) => { if (!active && !disabled) event.currentTarget.style.background = t.surfaceHover; }} onMouseLeave={(event) => { if (!active && !disabled) event.currentTarget.style.background = 'transparent'; }}>
                          <span style={{ flex: 1 }}>{reasoningSelectionLabel(option.selection)}</span>
                          {active && <Icon name="check" size={14} color={t.accent} stroke={2.2} />}
                        </button>
                      );
                    }) : <div style={{ padding: '5px 10px 10px', color: t.text4, fontSize: 11.5 }}>Effort is not configurable for this model.</div>}
                  </div>
                )}

                {modelSubmenu === 'speed' && fastModeAvailable && (
                  <div style={{ ...modelPickerSubmenuStyle(t), width: 300, maxWidth: 'min(300px, calc(100vw - 44px))', maxHeight: 'min(300px, calc(100vh - 140px))', overflow: 'hidden' }} role="menu" aria-label="Speed">
                    <div style={{ padding: '5px 10px 8px', color: t.text3, fontSize: 12, fontWeight: 600 }}>Speed</div>
                    <button type="button" role="menuitemradio" aria-checked={!bridge.desktop.fastMode || !fastModeAvailable} disabled={!ready || bridge.running} onClick={() => invoke(() => bridge.setFastMode(false))} style={speedOptionStyle(t, !bridge.desktop.fastMode || !fastModeAvailable, !ready || bridge.running)}>
                      <span style={{ flex: 1 }}><span style={{ display: 'block', fontSize: 13, fontWeight: 540 }}>Standard</span><span style={{ display: 'block', marginTop: 2, color: t.text3, fontSize: 11.5 }}>Default speed</span></span>
                      {(!bridge.desktop.fastMode || !fastModeAvailable) && <Icon name="check" size={16} color={t.accent} stroke={2.2} />}
                    </button>
                    {fastModeAvailable && <button type="button" role="menuitemradio" aria-checked={bridge.desktop.fastMode} disabled={!ready || bridge.running} onClick={() => invoke(() => bridge.setFastMode(true))} style={speedOptionStyle(t, bridge.desktop.fastMode, !ready || bridge.running)}>
                      <span style={{ flex: 1 }}><span style={{ display: 'block', fontSize: 13, fontWeight: 540 }}>Fast</span><span style={{ display: 'block', marginTop: 2, color: t.text3, fontSize: 11.5 }}>1.5x speed, more usage</span></span>
                      {bridge.desktop.fastMode && <Icon name="check" size={16} color={t.accent} stroke={2.2} />}
                    </button>}
                  </div>
                )}
              </div>
            )}
          </div>
          <button type="button" disabled={!ready} aria-label={voiceState === 'listening' && !flowMode ? 'Stop ordinary recording' : 'Start ordinary recording'} title={voiceState === 'unsupported' ? 'Voice input is unavailable in this environment' : voiceState === 'denied' ? 'Microphone permission was denied' : '普通录音'} onClick={toggleStandardVoice} style={{ ...composerIconStyle(t), width: 36, height: 36, color: voiceState === 'listening' && !flowMode ? t.accent : voiceState === 'denied' ? t.danger : t.text }}><Icon name="mic" size={20} color="currentColor" stroke={voiceState === 'listening' && !flowMode ? 2.1 : 1.7} /></button>
          <button
            type="button"
            disabled={!ready}
            aria-label={flowMode ? '关闭心流模式' : '开启心流模式'}
            aria-pressed={flowMode}
            title={flowMode ? '关闭心流模式' : '开启心流模式'}
            onClick={toggleFlowMode}
            style={{ ...composerIconStyle(t), width: 42, height: 42, background: flowMode ? t.accent : t.text, color: t.windowBg, boxShadow: flowMode ? `0 0 0 4px ${t.accentBg}` : 'none' }}
          >
            <Icon name="waveform" size={21} color={flowMode ? '#fff' : t.windowBg} stroke={2.15} />
          </button>
          {bridge.running && (
            <button
              type="button"
              disabled={bridge.isCancelling}
              onClick={() => invoke(() => bridge.cancel())}
              aria-label={bridge.isCancelling ? 'Stopping current turn' : 'Stop current turn'}
              title={bridge.isCancelling ? 'Stopping…' : 'Stop'}
              style={{ ...composerSendStyle(t, true), background: t.danger, cursor: bridge.isCancelling ? 'wait' : 'pointer', opacity: bridge.isCancelling ? .7 : 1 }}
            ><Icon name="stop" size={15} color="#fff" /></button>
          )}
          <button
            type="button"
            disabled={!ready || !hasPrompt}
            onClick={() => { void submit(); }}
            aria-label={bridge.running ? 'Send pending message' : 'Send prompt'}
            title={bridge.running ? 'Send as pending message' : 'Send prompt'}
            style={composerSendStyle(t, Boolean(ready && hasPrompt))}
          ><Icon name="arrowU" size={19} color={ready && hasPrompt ? '#fff' : t.text4} /></button>
        </div>
        {(voiceState === 'unsupported' || voiceState === 'denied') && <div style={{ position: 'relative' }}>
          {voiceState === 'unsupported' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.text3, fontSize: 10.5 }}>Voice input is unavailable here</span>}
          {voiceState === 'denied' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.danger, fontSize: 10.5 }}>Microphone permission denied</span>}
        </div>}
      </div>
      <div style={{ maxWidth: 980, margin: '5px auto 0', padding: '0 3px', display: 'flex', justifyContent: 'space-between', color: t.text4, fontSize: 10 }}>
        <span>Enter to send · Shift+Enter for a new line · @ files · / commands</span>
        <span>{goalMode ? 'Goal mode enabled' : permissionMode.description}</span>
      </div>
    </div>
  );
}

function composerIconStyle(t: ReturnType<typeof useT>): CSSProperties {
  return { display: 'grid', placeItems: 'center', border: 0, borderRadius: 99, background: 'transparent', color: t.text2, cursor: 'pointer', opacity: 1 };
}

function composerPillStyle(t: ReturnType<typeof useT>, active: boolean): CSSProperties {
  return { display: 'inline-flex', alignItems: 'center', gap: 7, minHeight: 34, padding: '0 9px', border: 0, borderRadius: 9, background: active ? t.surfaceHover : 'transparent', cursor: 'pointer', font: 'inherit', fontSize: 13.5, fontWeight: 500 };
}

function composerSendStyle(t: ReturnType<typeof useT>, enabled: boolean): CSSProperties {
  return { width: 38, height: 38, borderRadius: 99, border: 0, background: enabled ? t.accent : t.surfaceActive, color: enabled ? '#fff' : t.text4, display: 'grid', placeItems: 'center', cursor: enabled ? 'pointer' : 'not-allowed', opacity: enabled ? 1 : .82 };
}

function composerMenuStyle(t: ReturnType<typeof useT>, side: 'left' | 'right'): CSSProperties {
  return { position: 'absolute', bottom: 'calc(100% + 9px)', [side]: 0, zIndex: 20, width: 286, padding: 7, borderRadius: 12, border: `0.5px solid ${t.borderStrong}`, background: t.surface, boxShadow: '0 16px 40px rgba(0,0,0,.22)', animation: 'fade-in .15s ease' };
}

function modelPickerSubmenuStyle(t: ReturnType<typeof useT>): CSSProperties {
  return { position: 'absolute', right: 'calc(100% + 12px)', bottom: 12, zIndex: 21, padding: 8, borderRadius: 12, border: `0.5px solid ${t.borderStrong}`, background: t.surface, boxShadow: '0 16px 40px rgba(0,0,0,.22)', animation: 'fade-in .15s ease' };
}

function speedOptionStyle(t: ReturnType<typeof useT>, active: boolean, disabled: boolean): CSSProperties {
  return { display: 'flex', alignItems: 'center', gap: 9, width: '100%', minHeight: 54, padding: '7px 10px', border: 0, borderRadius: 7, background: active ? t.accentBg : 'transparent', color: disabled ? t.text4 : t.text, textAlign: 'left', cursor: disabled ? 'not-allowed' : 'pointer', font: 'inherit', opacity: disabled ? .65 : 1 };
}

export function BetaTasks({ bridge, onClose }: { bridge: UseBridge; onClose(): void }) {
  const t = useT();
  const tasks = orderedTasks(bridge.desktop);
  const [selected, setSelected] = useState<string | null>(null);
  useEffect(() => { invoke(bridge.refreshTasks); }, [bridge.refreshTasks]);
  const output = selected ? bridge.desktop.taskOutput[selected] : undefined;
  return (
    <aside style={{ width: 330, flexShrink: 0, borderLeft: `0.5px solid ${t.border}`, background: t.sidebarBg, display: 'flex', flexDirection: 'column' }}>
      <div style={{ height: 52, display: 'flex', alignItems: 'center', padding: '0 12px 0 15px', borderBottom: `0.5px solid ${t.border}` }}>
        <span style={{ flex: 1, fontSize: 12.5, fontWeight: 650, color: t.text }}>Background tasks</span>
        <Button onClick={() => invoke(bridge.refreshTasks)} title="Refresh tasks"><Icon name="git" size={13} /></Button>
        <button type="button" onClick={onClose} aria-label="Close tasks" style={{ marginLeft: 6, border: 0, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={14} /></button>
      </div>
      <div style={{ flex: 1, overflow: 'auto', padding: 9 }}>
        {tasks.length === 0 ? <div style={{ padding: 18, color: t.text4, fontSize: 11.5 }}>No background tasks reported.</div> : tasks.map((task) => {
          const running = task.status.type === 'running';
          const selectedTask = selected === task.task_id;
          const color = task.status.type === 'completed'
            ? t.ok
            : task.status.type === 'failed'
              ? t.danger
              : running ? t.accent : t.text3;
          return (
            <div
              key={task.task_id}
              className="background-task-row"
              data-status={task.status.type}
              data-selected={selectedTask ? 'true' : undefined}
              style={{
                '--task-label-color': t.text,
                '--task-description-color': t.text3,
                '--task-focus-color': color === t.danger ? t.danger : t.accent,
                '--sweep-base': t.text3,
                '--sweep-highlight': t.accent,
              } as CSSProperties}
            >
              <button type="button" className="background-task-row-main" onClick={() => { setSelected(task.task_id); invoke(() => bridge.taskOutput(task.task_id)); }}>
                <span className="background-task-row-heading">
                  <span aria-hidden="true" style={{ width: 7, height: 7, borderRadius: 99, background: color, flexShrink: 0 }} />
                  <strong style={{ fontSize: 11.5 }}>{task.task_type}</strong>
                  <span className={running ? 'running-sweep' : undefined} style={{ marginLeft: 'auto', color, fontSize: 10 }}>{task.status.type}</span>
                </span>
                <span className={running ? 'background-task-row-description running-sweep' : 'background-task-row-description'}>{task.description}</span>
              </button>
              {running && <button type="button" className="background-task-stop" onClick={() => invoke(() => bridge.stopTask(task.task_id))}>Stop task</button>}
            </div>
          );
        })}
        {output && <pre className="mono" style={{ marginTop: 10, padding: 10, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.windowBg, color: t.text2, whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', fontSize: 10.5, lineHeight: 1.55 }}>{output.content || '(No output yet)'}{output.truncated ? `\n\n…output truncated (${output.totalLines} total lines)` : ''}</pre>}
      </div>
    </aside>
  );
}

export function BetaSettings({ bridge, theme, onTheme, onClose }: { bridge: UseBridge; theme: ThemeMode; onTheme(value: ThemeMode): void; onClose(): void }) {
  const t = useT();
  const snapshot = bridge.bootstrap;
  const [key, setKey] = useState('');
  const [selectedProviderId, setSelectedProviderId] = useState('anthropic');
  const selectedProvider = providerById(selectedProviderId) ?? PROVIDERS[0];
  const selectedMetadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === selectedProvider.id);
  const panelRef = useRef<HTMLElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => { invoke(bridge.refreshDiagnostics); }, [bridge.refreshDiagnostics]);
  useEffect(() => {
    const previouslyFocused = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    const keyDown = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        onCloseRef.current();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = [...(panelRef.current?.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled])') ?? [])];
      const first = focusable[0];
      const last = focusable.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    document.addEventListener('keydown', keyDown);
    return () => { document.removeEventListener('keydown', keyDown); previouslyFocused?.focus(); };
  }, []);
  const save = () => {
    const submitted = key;
    invoke(async () => {
      const saved = await persistProviderCredentialInput(selectedProvider.id, submitted, bridge.setProviderCredential);
      if (saved) setKey((current) => current === submitted ? '' : current);
    });
  };
  return (
    <div role="dialog" aria-modal="true" aria-labelledby="lingxi-settings-title" style={{ position: 'absolute', inset: 0, zIndex: 50, display: 'grid', placeItems: 'center', background: 'rgba(0,0,0,.42)', padding: 24 }}>
      <section ref={panelRef} style={{ width: 'min(720px, 100%)', maxHeight: 'min(720px, 92vh)', overflow: 'auto', borderRadius: 15, border: `0.5px solid ${t.border}`, background: t.windowBg, boxShadow: '0 24px 70px rgba(0,0,0,.36)' }}>
        <header style={{ position: 'sticky', top: 0, zIndex: 1, display: 'flex', alignItems: 'center', padding: '14px 17px', borderBottom: `0.5px solid ${t.border}`, background: t.windowBg }}><strong id="lingxi-settings-title" style={{ flex: 1, color: t.text, fontSize: 14 }}>Settings & diagnostics</strong><button ref={closeRef} type="button" onClick={onClose} aria-label="Close settings" style={{ border: 0, background: 'transparent', color: t.text3, cursor: 'pointer' }}><Icon name="x" size={16} /></button></header>
        <div style={{ padding: 18, display: 'grid', gap: 18 }}>
          <SettingsSection title="Appearance"><div style={{ display: 'flex', gap: 8 }}><Button primary={theme === 'dark'} onClick={() => onTheme('dark')}><Icon name="moon" size={13} /> Dark</Button><Button primary={theme === 'light'} onClick={() => onTheme('light')}><Icon name="sun" size={13} /> Light</Button></div></SettingsSection>
          <SettingsSection title="Providers">
            <p>Desktop, CLI, and TUI share one credential store. It uses the macOS login Keychain when available and an owner-only local fallback otherwise.</p>
            <div style={{ width: '100%', display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(145px, 1fr))', gap: 7 }}>
              {PROVIDERS.map((provider) => {
                const metadata = snapshot?.providerCredentials?.find((entry) => entry.providerId === provider.id);
                const selected = provider.id === selectedProvider.id;
                return <button key={provider.id} type="button" onClick={() => { setSelectedProviderId(provider.id); setKey(''); }} style={{ padding: '8px 9px', borderRadius: 8, border: `0.5px solid ${selected ? t.accentBorder : t.border}`, background: selected ? t.accentBg : t.surface, color: t.text, textAlign: 'left', cursor: 'pointer', opacity: provider.available ? 1 : .55 }}><span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 11.5, fontWeight: 650 }}>{metadata?.configured && <Icon name="check" size={12} color={t.ok} stroke={2.5} />}{provider.label}</span><span style={{ display: 'block', color: t.text4, fontSize: 10, marginTop: 2 }}>{provider.available ? provider.description : 'CLI/TUI sign-in'}</span></button>;
              })}
            </div>
            {selectedMetadata?.runtimeOnly && <p style={{ color: t.ok }}>The running engine received this credential from an external runtime source. LingXi has not stored it.</p>}
            {selectedMetadata?.configured && selectedMetadata.encryptionAvailable && !selectedMetadata.runtimeOnly && <p style={{ color: t.ok }}>Persisted securely on this Mac. Generic API keys are intentionally not listed in the Passwords app.</p>}
            {selectedMetadata?.configured && !selectedMetadata.encryptionAvailable && !selectedMetadata.runtimeOnly && <p style={{ color: t.warn }}>macOS Keychain is unavailable. This credential is in the shared owner-only local fallback used by Desktop, CLI, and TUI.</p>}
            {!selectedMetadata?.configured && selectedMetadata?.encryptionAvailable === false && <p style={{ color: t.warn }}>macOS Keychain is unavailable. Connecting will use the shared owner-only local fallback used by Desktop, CLI, and TUI.</p>}
            {selectedProvider.available ? <>
              <label htmlFor="settings-provider-credential" style={{ color: t.text2, fontSize: 11 }}>{selectedProvider.keyLabel}</label>
              <div style={{ display: 'flex', gap: 7, width: '100%' }}><input id="settings-provider-credential" type="password" autoComplete="off" disabled={bridge.running} value={key} onChange={(event) => setKey(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') save(); }} placeholder={selectedMetadata?.configured ? 'Enter a replacement key' : selectedProvider.keyPlaceholder} aria-label={`${selectedProvider.keyLabel} for settings`} style={{ flex: 1, height: 33, borderRadius: 8, border: `0.5px solid ${t.border}`, background: t.surface, color: t.text, padding: '0 9px' }} /><Button disabled={!key.trim() || bridge.running} onClick={save}>{selectedMetadata?.runtimeOnly ? 'Use entered key' : selectedMetadata?.configured ? 'Replace' : 'Connect'}</Button>{selectedMetadata?.configured && !selectedMetadata.runtimeOnly && <Button disabled={bridge.running} danger onClick={() => invoke(() => bridge.clearProviderCredential(selectedProvider.id))}>Delete</Button>}</div>
            </> : <p style={{ color: t.warn }}>{selectedProvider.label} sign-in is currently available from the CLI/TUI connect flow.</p>}
          </SettingsSection>
          <SettingsSection title="Engine">
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}><ConnectionDot bridge={bridge} /><Button onClick={() => invoke(bridge.restartBridge)}>Restart engine</Button><Button onClick={() => invoke(bridge.refresh)}>Refresh all</Button></div>
            {bridge.desktop.status && <div className="mono" style={{ color: t.text3, fontSize: 10.5 }}>session {bridge.desktop.status.session_id} · {bridge.desktop.status.model} · {bridge.desktop.status.n_messages} messages</div>}
            {bridge.desktop.doctor && <div style={{ color: bridge.desktop.doctor.summary.failed > 0 ? t.danger : bridge.desktop.doctor.summary.warnings > 0 ? t.warn : t.ok, fontSize: 10.5 }}>Doctor: {bridge.desktop.doctor.summary.passed} passed, {bridge.desktop.doctor.summary.warnings} warnings, {bridge.desktop.doctor.summary.failed} failed</div>}
          </SettingsSection>
          <SettingsSection title="Diagnostics"><p>Sanitized lifecycle messages only. Prompts, tool payloads and credential values are excluded.</p><div style={{ display: 'flex', gap: 7 }}><Button onClick={() => invoke(bridge.copyDiagnostics)}>Copy report</Button><Button onClick={() => invoke(bridge.exportDiagnostics)}>Export JSON…</Button><Button onClick={() => invoke(bridge.refreshDiagnostics)}>Refresh</Button></div><div className="mono" style={{ maxHeight: 170, overflow: 'auto', padding: 9, borderRadius: 8, background: t.surface, border: `0.5px solid ${t.border}`, color: t.text3, fontSize: 9.5, lineHeight: 1.55 }}>{snapshot?.diagnostics.length ? snapshot.diagnostics.map((entry, index) => <div key={`${entry.timestamp}-${index}`}><span style={{ color: entry.level === 'error' ? t.danger : entry.level === 'warn' ? t.warn : t.text4 }}>{entry.timestamp} [{entry.source}/{entry.level}]</span> {entry.message}</div>) : 'No diagnostic entries.'}</div></SettingsSection>
          <SettingsSection title="About"><p>LingXi Code Desktop · Internal Beta</p><p>Local engine, explicit project trust, shared credential storage, manual signed updates.</p></SettingsSection>
        </div>
      </section>
    </div>
  );
}

function SettingsSection({ title, children }: { title: string; children: ReactNode }) {
  const t = useT();
  return <section style={{ display: 'grid', gridTemplateColumns: '145px 1fr', gap: 18, paddingBottom: 18, borderBottom: `0.5px solid ${t.border}` }}><h2 style={{ color: t.text, fontSize: 12.5 }}>{title}</h2><div style={{ minWidth: 0, display: 'flex', flexDirection: 'column', alignItems: 'flex-start', gap: 9, color: t.text3, fontSize: 11.5, lineHeight: 1.5 }}>{children}</div></section>;
}

export function ErrorBanner({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  if (!bridge.error) return null;
  const error = classifyDesktopError(bridge.error);
  return (
    <div role="alert" style={{ display: 'flex', alignItems: 'center', gap: 9, padding: '8px 12px', background: `color-mix(in oklab, ${t.danger} 12%, ${t.windowBg})`, borderBottom: `0.5px solid color-mix(in oklab, ${t.danger} 35%, transparent)`, color: t.danger, fontSize: 11.5 }}>
      <Icon name="circle" size={13} color={t.danger} />
      <span title={`${error.title}. ${error.detail}`} style={{ flex: 1, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}><strong>{error.title}.</strong> {error.detail}</span>
      <Button onClick={bridge.clearError}>Dismiss</Button>
    </div>
  );
}
