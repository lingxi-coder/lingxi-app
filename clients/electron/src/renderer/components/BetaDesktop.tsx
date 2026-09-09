import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type ClipboardEvent, type KeyboardEvent, type PointerEvent as ReactPointerEvent, type ReactNode } from 'react';
import type {
  ImageRefDto,
  ModelDetailsDto,
  ReasoningSelectionDto,
  SessionRowDto,
} from '@lingxi/bridge-client';

import { type UseBridge } from '../bridge/useBridge';
import type { ContextSummarySnapshot } from '../bridge/conversation';
import type { NativeAudioApi } from '../bridge/lingxi';
import { orderedTasks } from '../bridge/desktopState';
import { classifyDesktopError } from '../bridge/errors';
import { useT } from '../theme/ThemeContext';
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
  renderDesktopSlashHelp,
  slashCommandText,
  slashMenuLabel,
  slashNavigationDirection,
} from '../bridge/slashCommands';
import {
  filterModelGroups,
  filterVisibleModelReferences,
  groupModelReferences,
  modelBillingGroups,
  modelDisplayLabel,
  modelReference,
  resolveModelSelection,
} from '../bridge/modelCatalog';
import { formatSessionMetadata } from '../bridge/sessionPresentation';
import { ALL_DESKTOP_COMMANDS } from '../bridge/desktopCommands';
import {
  desktopCommandIsShadowed,
  resolveDesktopCommand,
  type DesktopCommandContext,
} from '../bridge/slashDispatch';
import {
  DEFAULT_VOICE_FLOW_STATE,
  VoiceFlowController,
  type VoiceFlowState,
} from '../audio/flow/controller';
import { sanitizeSpeakableText } from '../audio/flow/segmenter';
import { shouldAutoplayTrackedReply } from '../audio/autoplay';
import { ArchiveChatDialog } from './ArchiveChatDialog';
import type { ScheduledCronJob } from '../bridge/scheduledTaskDraft';
import { Icon } from './Icon';
import { commandPaletteIcon } from './commandPaletteIcons';
import { MarkdownContent } from './MarkdownContent';
import { VoiceFlowPanel } from './voice/VoiceFlowPanel';
import { providerById } from '../../shared/providers';
import { MAX_IMAGE_ATTACHMENTS } from '../../shared/imageInput';
import { defaultVoicePreferences, LANGUAGE_AUTO } from '../../shared/voicePreferences';
import type { NativeAudioOwner, NativeAudioResponse } from '../../shared/nativeAudio';
import { PERMISSION_MODE_OPTIONS } from '../model/permissionModes';
import type { RunItem } from '../model/runItem';

export const SIDEBAR_DEFAULT_WIDTH = 260;
export const SIDEBAR_MIN_WIDTH = 200;
export const SIDEBAR_MAX_WIDTH = 480;
const SIDEBAR_KEYBOARD_STEP = 16;

export function clampSidebarWidth(width: number): number {
  return Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, width));
}

function basename(path?: string): string {
  if (!path) return 'No project';
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

function invoke(action: () => Promise<unknown>): void {
  void action().catch(() => undefined);
}

const GOAL_CLEAR_TOKENS = new Set(['clear', 'stop', 'off', 'reset', 'none', 'cancel']);

/** Project active Goal state from the command lifecycle already visible in this conversation. */
export function composerGoalActive(items: readonly RunItem[]): boolean {
  let active = false;
  for (const item of items) {
    if (item.type === 'narration' && item.role === 'user') {
      const match = /^\s*\/goal(?:\s+([\s\S]*))?\s*$/i.exec(item.text);
      const args = match?.[1]?.trim();
      if (args) active = !GOAL_CLEAR_TOKENS.has(args.toLocaleLowerCase());
      continue;
    }
    if (item.type === 'command' && item.name.trim().split(/\s/, 1)[0]?.toLocaleLowerCase() === '/goal') {
      const output = item.output.trim().toLocaleLowerCase();
      if (output.startsWith('goal active:') || output.startsWith('goal set:')) active = true;
      else if (
        output.startsWith('no goal set')
        || output.startsWith('goal cleared:')
        || output.includes('only available in trusted workspaces')
        || output.includes("can't run while hooks are restricted")
        || output.startsWith('goal condition is limited')
      ) active = false;
      continue;
    }
    if (
      item.type === 'narration'
      && item.text.includes('Goal cleared after an unrecoverable error')
    ) active = false;
  }
  return active;
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

function SessionRow({ session, active, pinned, opening, status, onClick, onPin, onArchive }: {
  session: SessionRowDto;
  active: boolean;
  pinned: boolean;
  opening: boolean;
  status?: ReturnType<UseBridge['sessionRuntimeStatus']>;
  onClick(): void;
  onPin(): void;
  onArchive(): void;
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
          padding: '6px 62px 6px 30px', borderRadius: 8, border: 0, textAlign: 'left',
          background: active ? t.surfaceActive : opening ? t.surface : 'transparent', color: highlighted ? t.text : t.text2,
          cursor: opening ? 'wait' : 'pointer',
          fontSize: 13, fontWeight: active ? 600 : 500,
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
          position: 'absolute', right: 32, top: 4, width: 26, height: 26,
          display: 'grid', placeItems: 'center', border: 0, borderRadius: 6,
          background: active ? 'transparent' : t.sidebarBg, color: pinned ? t.accent : t.text3,
          cursor: 'pointer',
        }}
      >
        <Icon name="pin" size={13} stroke={1.8} />
      </button>
      <button type="button" className="sidebar-row-action" aria-label={`Archive ${session.title || 'Untitled chat'}`} title="Archive chat" disabled={opening} onClick={onArchive}
        style={{ position: 'absolute', right: 4, top: 4, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, color: t.text3, background: 'transparent', cursor: 'pointer' }}><Icon name="archive" size={13} /></button>
    </div>
  );
}

export function BetaSidebar({ bridge, onOpenSettings, scheduled = false, onOpenScheduled, onOpenChat }: {
  bridge: UseBridge;
  onOpenSettings(): void;
  scheduled?: boolean;
  onOpenScheduled?(): void;
  onOpenChat?(): void;
}) {
  const t = useT();
  const asideRef = useRef<HTMLElement>(null);
  const resizingPointerRef = useRef<number | null>(null);
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
  const [archiveTarget, setArchiveTarget] = useState<{projectPath: string; sessionId: string; title: string} | null>(null);
  const [archiveJobs, setArchiveJobs] = useState<ScheduledCronJob[]>([]);
  const [archiveLoading, setArchiveLoading] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const [archiveError, setArchiveError] = useState('');
  const archiveRequest = useRef(0);
  const closeArchive = () => { archiveRequest.current++; setArchiveTarget(null); };
  const prepareArchive = async (projectPath: string, sessionId: string, title: string) => {
    const version = ++archiveRequest.current;
    setArchiveTarget({ projectPath, sessionId, title });
    setArchiveJobs([]); setArchiveError(''); setArchiveLoading(true);
    try { const jobs = await bridge.preflightSessionArchive(projectPath, sessionId); if (version === archiveRequest.current) setArchiveJobs(jobs); }
    catch (cause) { if (version === archiveRequest.current) setArchiveError(cause instanceof Error ? cause.message : String(cause)); }
    finally { if (version === archiveRequest.current) setArchiveLoading(false); }
  };
  const confirmArchive = async () => {
    if (!archiveTarget || archiving) return;
    setArchiving(true); setArchiveError('');
    try { await bridge.archiveSession(archiveTarget.projectPath, archiveTarget.sessionId); closeArchive(); }
    catch (cause) { setArchiveError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setArchiving(false); }
  };
  const [sidebarWidth, setSidebarWidth] = useState(SIDEBAR_DEFAULT_WIDTH);
  const [resizingSidebar, setResizingSidebar] = useState(false);

  useEffect(() => {
    if (!resizingSidebar) return;
    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
    return () => {
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
    };
  }, [resizingSidebar]);

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
    onOpenChat?.();
    const key = `${projectPath}\0${sessionId}`;
    setOpeningSessionKey(key);
    void bridge.openSession(projectPath, sessionId)
      .catch(() => undefined)
      .finally(() => setOpeningSessionKey((current) => current === key ? null : current));
  }, [bridge.openSession, onOpenChat]);
  const editProject = useCallback((projectPath: string) => {
    onOpenChat?.();
    setExpandedProjects((current) => current.has(projectPath) ? current : new Set([...current, projectPath]));
    setEditingProject(projectPath);
    void bridge.newSession(projectPath)
      .catch(() => undefined)
      .finally(() => setEditingProject((current) => current === projectPath ? null : current));
  }, [bridge.newSession, onOpenChat]);
  const pinInput = (projectPath: string, sessionId: string, title: string) => ({ projectPath, sessionId, title });
  const startSidebarResize = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return;
    event.preventDefault();
    event.currentTarget.setPointerCapture(event.pointerId);
    resizingPointerRef.current = event.pointerId;
    setResizingSidebar(true);
  }, []);
  const resizeSidebar = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (resizingPointerRef.current !== event.pointerId) return;
    const sidebarLeft = asideRef.current?.getBoundingClientRect().left ?? 0;
    setSidebarWidth(clampSidebarWidth(event.clientX - sidebarLeft));
  }, []);
  const finishSidebarResize = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    resizingPointerRef.current = null;
    setResizingSidebar(false);
  }, []);
  const resizeSidebarWithKeyboard = useCallback((event: KeyboardEvent<HTMLDivElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    setSidebarWidth((current) => {
      if (event.key === 'Home') return SIDEBAR_MIN_WIDTH;
      if (event.key === 'End') return SIDEBAR_MAX_WIDTH;
      const delta = event.key === 'ArrowLeft' ? -SIDEBAR_KEYBOARD_STEP : SIDEBAR_KEYBOARD_STEP;
      return clampSidebarWidth(current + delta);
    });
  }, []);

  return (
    <>
    <aside
      className="desktop-sidebar"
      ref={asideRef}
      data-resizing={resizingSidebar || undefined}
      style={{ position: 'relative', width: sidebarWidth, flexShrink: 0, display: 'flex', flexDirection: 'column', background: t.sidebarBg, borderRight: `0.5px solid ${t.border}`, paddingTop: 38, boxShadow: t.dark ? '8px 0 30px rgba(0,0,0,.10)' : '8px 0 30px rgba(45,38,74,.035)' }}
    >
      <div className="drag-region desktop-sidebar-brand" style={{ minHeight: 48, padding: '7px 14px 6px', display: 'flex', alignItems: 'center', gap: 9 }}>
        <span className="desktop-brand-mark" style={{ color: t.accent, background: t.accentBg, boxShadow: `0 0 0 1px ${t.accentBorder}` }} aria-hidden="true"><Icon name="spark" size={13} stroke={1.7} /></span>
        <strong style={{ color: t.text, fontSize: 16, fontWeight: 600, letterSpacing: '-.02em' }}>LingXi</strong>
        <span className="desktop-brand-label" style={{ color: t.text4, borderColor: t.border }}>CODE</span>
      </div>

      <div style={{ padding: '2px 9px 10px' }}>
        <button
          className="sidebar-primary-action"
          type="button"
          disabled={bridge.sessionLoading || editingProject !== null}
          onClick={() => { onOpenChat?.(); selectedProject ? editProject(selectedProject) : invoke(bridge.addProject); }}
          style={{
            width: '100%', minHeight: 42, display: 'flex', alignItems: 'center', gap: 11,
            padding: '8px 11px', borderRadius: 11, border: 0, background: 'transparent',
            color: t.text, cursor: bridge.sessionLoading || editingProject !== null ? 'wait' : 'pointer', opacity: bridge.sessionLoading || editingProject !== null ? .5 : 1,
            textAlign: 'left', fontSize: 13.5, fontWeight: 500,
          }}
        >
          {editingProject
            ? <span className="beta-spinner" role="status" aria-label="Opening project draft" />
            : <Icon name="compose" size={20} color={t.text2} stroke={1.8} />}
          <span>{editingProject ? 'Opening draft…' : 'New chat'}</span>
        </button>
        <button
          type="button"
          className="sidebar-primary-action"
          aria-current={scheduled ? 'page' : undefined}
          onClick={onOpenScheduled}
          style={{ width: '100%', minHeight: 42, marginTop: 4, display: 'flex', alignItems: 'center', gap: 11,
            padding: '8px 11px', borderRadius: 11, border: 0, background: scheduled ? t.surfaceActive : 'transparent',
            color: t.text, cursor: 'pointer', textAlign: 'left', fontSize: 13.5, fontWeight: 500 }}
        >
          <Icon name="clock" size={20} stroke={1.8} />
          <span>Scheduled</span>
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
              const active = !scheduled && visibleSession?.projectPath === pinned.projectPath && visibleSession.sessionId === pinned.sessionId;
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
                      width: '100%', minHeight: 43, display: 'grid', gap: 1, padding: '6px 62px 6px 10px',
                      border: 0, borderRadius: 8, background: active ? t.surfaceActive : opening ? t.surface : 'transparent',
                      color: t.text2, textAlign: 'left', cursor: opening ? 'wait' : 'pointer',
                    }}
                  >
                    <span style={{ display: 'flex', alignItems: 'center', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 13, fontWeight: active ? 600 : 500 }}>
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
                    style={{ position: 'absolute', right: 32, top: 8, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: 'transparent', color: t.accent, cursor: 'pointer' }}
                  >
                    <Icon name="pin" size={13} stroke={1.8} />
                  </button>
                  <button type="button" className="sidebar-row-action" aria-label={`Archive ${title}`} title="Archive chat" onClick={() => void prepareArchive(pinned.projectPath, pinned.sessionId, title)}
                    style={{ position: 'absolute', right: 4, top: 8, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, color: t.text3, background: 'transparent', cursor: 'pointer' }}><Icon name="archive" size={13} /></button>
                </div>
              );
            })}
          </section>
        ) : null}

        <section aria-labelledby="projects-heading">
          <div style={{ minHeight: 31, padding: '2px 4px 4px 8px', display: 'flex', alignItems: 'center' }}>
            <h2 id="projects-heading" style={{ flex: 1, color: t.text4, fontSize: 11, fontWeight: 600, letterSpacing: '.04em' }}>Projects</h2>
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
                      onOpenChat?.();
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
                      background: 'transparent', color: active ? t.text : t.text2,
                      cursor: 'pointer', textAlign: 'left',
                    }}
                  >
                    <Icon name="folder" size={16} color={active ? t.text2 : t.text3} stroke={1.7} />
                    <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontSize: 13, fontWeight: active ? 600 : 500 }}>{basename(projectPath)}</span>
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
                          active={!scheduled && visibleSession?.projectPath === projectPath && visibleSession.sessionId === session.uuid}
                          pinned={pinned}
                          opening={opening}
                          status={bridge.sessionRuntimeStatus(session.uuid)}
                          onClick={() => openSidebarSession(projectPath, session.uuid)}
                          onArchive={() => void prepareArchive(projectPath, session.uuid, session.title || 'Untitled chat')}
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
        {!!settings?.archivedSessions?.length && <details style={{ marginTop: 16, padding: '0 8px', color: t.text3 }}>
          <summary style={{ cursor: 'pointer', fontSize: 12 }}>Archived chats ({settings.archivedSessions.length})</summary>
          {settings.archivedSessions.map((session) => <button key={`${session.projectPath}-${session.sessionId}`} type="button" title="Restore and open chat" onClick={() => openSidebarSession(session.projectPath, session.sessionId)}
            style={{ display: 'flex', width: '100%', gap: 8, padding: '10px 0', border: 0, background: 'transparent', color: t.text2, textAlign: 'left', cursor: 'pointer' }}>
            <Icon name="archive" size={14} /><span>{session.title || 'Archived chat'}</span>
          </button>)}
        </details>}
      </nav>

      <div className="desktop-sidebar-footer" style={{ padding: 10, borderTop: `0.5px solid ${t.border}`, display: 'flex', flexDirection: 'column', gap: 5 }}>
        <ConnectionDot bridge={bridge} />
        <button
          type="button"
          onClick={onOpenSettings}
          style={{ minHeight: 40, display: 'flex', alignItems: 'center', gap: 8, padding: '7px 5px', border: 0, borderRadius: 8, background: 'transparent', color: t.text2, cursor: 'pointer', fontSize: 12.5 }}
        >
          <Icon name="cog" size={15} /> Settings & diagnostics
        </button>
      </div>

      <div
        className="sidebar-resize-handle no-drag"
        role="separator"
        aria-label="Resize sidebar"
        aria-orientation="vertical"
        aria-valuenow={Math.round(sidebarWidth)}
        aria-valuemin={SIDEBAR_MIN_WIDTH}
        aria-valuemax={SIDEBAR_MAX_WIDTH}
        tabIndex={0}
        onPointerDown={startSidebarResize}
        onPointerMove={resizeSidebar}
        onPointerUp={finishSidebarResize}
        onPointerCancel={finishSidebarResize}
        onLostPointerCapture={() => {
          resizingPointerRef.current = null;
          setResizingSidebar(false);
        }}
        onKeyDown={resizeSidebarWithKeyboard}
        onDoubleClick={() => setSidebarWidth(SIDEBAR_DEFAULT_WIDTH)}
        style={{ color: t.accent }}
      />
    </aside>
    {archiveTarget && <ArchiveChatDialog title={archiveTarget.title} jobs={archiveJobs} loading={archiveLoading} busy={archiving} error={archiveError} onClose={closeArchive} onConfirm={() => void confirmArchive()} />}
    </>
  );
}

function contextSummaryPreview(content: string): string {
  const line = content
    .split('\n')
    .map((part) => part.trim().replace(/^#{1,6}\s+/, '').replace(/^summary\s*:\s*/i, ''))
    .find(Boolean);
  if (!line) return 'Context preserved by compaction';
  return line.length > 76 ? `${line.slice(0, 73).trimEnd()}…` : line;
}

function contextSummaryStats(summary: ContextSummarySnapshot): string {
  const messages = summary.messagesBefore > 0
    ? summary.messagesAfter > 0
      ? `${summary.messagesBefore} → ${summary.messagesAfter} messages`
      : `${summary.messagesBefore} messages summarized`
    : 'Compacted context';
  if (!summary.bytesSaved) return messages;
  const bytes = summary.bytesSaved < 1024
    ? `${summary.bytesSaved} B`
    : summary.bytesSaved < 1024 * 1024
      ? `${Math.round(summary.bytesSaved / 1024)} KB`
      : `${(summary.bytesSaved / (1024 * 1024)).toFixed(1)} MB`;
  return `${messages} · ${bytes} freed`;
}

export function ContextSummaryPanel({ summaries, selectedId, onSelect, onClose }: {
  summaries: readonly ContextSummarySnapshot[];
  selectedId: string | null;
  onSelect(id: string): void;
  onClose(): void;
}) {
  const t = useT();
  const selected = summaries.find((summary) => summary.id === selectedId) ?? summaries.at(-1);
  const selectedIndex = selected ? summaries.findIndex((summary) => summary.id === selected.id) : -1;

  return (
    <section
      id="context-summary-panel"
      className="no-drag context-summary-panel"
      role="dialog"
      aria-label="Context summaries"
      style={{
        '--summary-panel-bg': t.surface,
        '--summary-panel-subtle': t.surfaceHover,
        '--summary-panel-active': t.accentBg,
        '--summary-panel-ring': t.borderStrong,
        '--summary-panel-divider': t.border,
        '--summary-panel-text': t.text,
        '--summary-panel-secondary': t.text3,
        '--summary-panel-muted': t.text4,
        '--summary-panel-accent': t.accent,
      } as CSSProperties}
    >
      <div className="context-summary-sidebar">
        <div className="context-summary-heading">
          <div>
            <strong>Context summaries</strong>
            <span>{summaries.length ? `${summaries.length} saved` : 'No saved summary'}</span>
          </div>
        </div>
        <div className="context-summary-list" role="listbox" aria-label="Saved context summaries">
          {[...summaries].reverse().map((summary, reverseIndex) => {
            const originalIndex = summaries.length - reverseIndex - 1;
            const current = selected?.id === summary.id;
            return (
              <button
                key={summary.id}
                type="button"
                role="option"
                aria-selected={current}
                className="context-summary-item"
                data-active={current ? 'true' : undefined}
                onClick={() => onSelect(summary.id)}
              >
                <span className="context-summary-item-icon" aria-hidden="true"><Icon name="summary" size={15} stroke={1.75} /></span>
                <span className="context-summary-item-copy">
                  <span className="context-summary-item-title">
                    Summary {originalIndex + 1}
                    {reverseIndex === 0 ? <em>Current</em> : null}
                  </span>
                  <span className="context-summary-item-preview">{contextSummaryPreview(summary.content)}</span>
                  <span className="context-summary-item-meta">{contextSummaryStats(summary)}</span>
                </span>
                <Icon name="chevronR" size={13} stroke={1.8} />
              </button>
            );
          })}
          {summaries.length === 0 ? (
            <div className="context-summary-empty-list">
              <span aria-hidden="true"><Icon name="summary" size={19} stroke={1.6} /></span>
              <p>Summaries appear after the conversation is compacted.</p>
            </div>
          ) : null}
        </div>
      </div>

      <article className="context-summary-detail">
        <div className="context-summary-detail-heading">
          <div>
            <span>Context summary</span>
            <strong>{selected ? `Summary ${selectedIndex + 1}` : 'No summary yet'}</strong>
          </div>
          <button type="button" className="context-summary-close" aria-label="Close context summaries" onClick={onClose}>
            <Icon name="x" size={14} stroke={1.8} />
          </button>
        </div>
        {selected ? (
          <>
            <div className="context-summary-detail-meta">{contextSummaryStats(selected)}</div>
            <div className="context-summary-markdown"><MarkdownContent text={selected.content} /></div>
          </>
        ) : (
          <div className="context-summary-empty-detail">
            <span aria-hidden="true"><Icon name="summary" size={24} stroke={1.45} /></span>
            <strong>No compacted context</strong>
            <p>When LingXi compacts this session, the preserved context will be available here.</p>
          </div>
        )}
      </article>
    </section>
  );
}

export function BetaTopBar({ bridge, runtimeCenterOpen, onToggleRuntimeCenter }: {
  bridge: UseBridge;
  runtimeCenterOpen: boolean;
  onToggleRuntimeCenter(): void;
}) {
  const t = useT();
  const [moreOpen, setMoreOpen] = useState(false);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const [selectedSummaryId, setSelectedSummaryId] = useState<string | null>(null);
  const moreTriggerRef = useRef<HTMLButtonElement>(null);
  const moreMenuRef = useRef<HTMLDivElement>(null);
  const summaryPanelRef = useRef<HTMLDivElement>(null);
  const summaries = bridge.conversation.summaries;
  const inspectorOpen = bridge.runtimeCenter.inspectorOpen;

  useEffect(() => {
    if (!moreOpen && !summaryOpen) return;
    const closeOnPointerDown = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (moreTriggerRef.current?.contains(target) || moreMenuRef.current?.contains(target) || summaryPanelRef.current?.contains(target)) return;
      setMoreOpen(false);
      setSummaryOpen(false);
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.stopPropagation();
      setMoreOpen(false);
      setSummaryOpen(false);
      moreTriggerRef.current?.focus();
    };
    document.addEventListener('pointerdown', closeOnPointerDown, true);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOnPointerDown, true);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [moreOpen, summaryOpen]);

  useEffect(() => {
    setMoreOpen(false);
    setSummaryOpen(false);
    setSelectedSummaryId(null);
  }, [bridge.conversation.sessionKey]);

  useEffect(() => {
    if (moreOpen) moreMenuRef.current?.querySelector<HTMLButtonElement>('button')?.focus();
  }, [moreOpen]);

  useEffect(() => {
    if (summaryOpen) summaryPanelRef.current?.querySelector<HTMLButtonElement>('.context-summary-close')?.focus();
  }, [summaryOpen]);

  const topbarActionTokens = {
    '--topbar-action-focus': t.surfaceHover,
    '--topbar-action-active': t.surfaceHover,
    '--topbar-action-ring': t.borderStrong,
    '--topbar-action-color': t.text3,
    '--topbar-action-hover-color': t.text,
    '--topbar-action-active-color': t.text,
  } as CSSProperties;
  return (
    <header className="drag-region desktop-topbar" style={{ height: 56, position: 'relative', flexShrink: 0, display: 'flex', alignItems: 'center', gap: 4, padding: '0 12px 0 18px', borderBottom: `0.5px solid ${t.border}`, background: t.windowBg }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ color: t.text, fontSize: 13, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{basename(bridge.activeSession?.projectPath ?? bridge.bootstrap?.workspace.path)}</div>
        <div className="mono" style={{ color: t.text4, fontSize: 10, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{bridge.activeSession?.projectPath ?? bridge.bootstrap?.workspace.path ?? 'Add a project to begin'}</div>
      </div>
      {bridge.usage && (
        <span className="mono desktop-topbar-usage" style={{ color: t.text4, fontSize: 9.5 }} title="Input + output tokens">
          {(bridge.usage.inputTokens + bridge.usage.outputTokens).toLocaleString()} tok
        </span>
      )}
      <button
        ref={moreTriggerRef}
        className="no-drag desktop-topbar-action"
        type="button"
        aria-label="More chat actions"
        title="More chat actions"
        aria-expanded={moreOpen}
        aria-controls="desktop-topbar-more"
        data-active={moreOpen || summaryOpen ? 'true' : undefined}
        onClick={() => { setMoreOpen((open) => !open); setSummaryOpen(false); }}
        style={topbarActionTokens}
      ><Icon name="more" size={18} /></button>
      <button
        className="no-drag desktop-topbar-action"
        type="button"
        data-runtime-center-trigger="true"
        aria-label="Toggle pinned summary"
        title="Toggle pinned summary"
        aria-controls="runtime-center-overview"
        aria-expanded={runtimeCenterOpen}
        aria-pressed={runtimeCenterOpen}
        data-active={runtimeCenterOpen ? 'true' : undefined}
        onClick={onToggleRuntimeCenter}
        style={topbarActionTokens}
      ><Icon name="summary-list" size={20} /></button>
      <button
        className="no-drag desktop-topbar-action desktop-inspector-trigger"
        type="button"
        data-runtime-inspector-trigger="true"
        aria-label="Toggle right panel"
        title="Toggle right panel"
        aria-controls="runtime-inspector"
        aria-expanded={inspectorOpen}
        aria-pressed={inspectorOpen}
        data-active={inspectorOpen ? 'true' : undefined}
        onClick={() => {
          bridge.setRuntimeInspectorOpen(!inspectorOpen);
          if (!inspectorOpen) window.requestAnimationFrame(() => {
            (document.querySelector<HTMLElement>('[data-runtime-inspector-active="true"]')
              ?? document.querySelector<HTMLElement>('.runtime-inspector-landing button')
              ?? document.querySelector<HTMLElement>('.runtime-panel-hide'))?.focus();
          });
        }}
        style={topbarActionTokens}
      ><Icon name={inspectorOpen ? 'panel-right' : 'panel-right-hidden'} size={20} /></button>
      {moreOpen && (
        <div ref={moreMenuRef} id="desktop-topbar-more" className="no-drag desktop-topbar-more" style={{ background: t.surface, color: t.text, borderColor: t.border }}>
          <button type="button" aria-label="Open context summaries" onClick={() => {
            setMoreOpen(false);
            setSelectedSummaryId(summaries.at(-1)?.id ?? null);
            setSummaryOpen(true);
          }}><Icon name="summary" size={16} /><span>Context summaries</span></button>
        </div>
      )}
      {summaryOpen && (
        <div ref={summaryPanelRef} style={{ display: 'contents' }}>
          <ContextSummaryPanel
            summaries={summaries}
            selectedId={selectedSummaryId}
            onSelect={setSelectedSummaryId}
            onClose={() => { setSummaryOpen(false); moreTriggerRef.current?.focus(); }}
          />
        </div>
      )}
    </header>
  );
}

function nativeAudioApi(): NativeAudioApi | undefined {
  return typeof window === 'undefined' ? undefined : window.lingxi?.audio;
}

const DICTATION_WAVEFORM_SAMPLES = 160;

export function DictationRecorderBar({ audio, owner, onCancel, onFinish }: {
  audio: NativeAudioApi | undefined;
  owner: NativeAudioOwner;
  onCancel(): void;
  onFinish(): void;
}) {
  const t = useT();
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const levelsRef = useRef<Float32Array>(new Float32Array(DICTATION_WAVEFORM_SAMPLES));
  const frameRef = useRef<number | null>(null);

  const drawWaveform = useCallback(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const width = Math.max(1, Math.floor(canvas.clientWidth));
    const height = Math.max(1, Math.floor(canvas.clientHeight));
    const pixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    const renderWidth = Math.floor(width * pixelRatio);
    const renderHeight = Math.floor(height * pixelRatio);
    if (canvas.width !== renderWidth || canvas.height !== renderHeight) {
      canvas.width = renderWidth;
      canvas.height = renderHeight;
    }
    const context = canvas.getContext('2d');
    if (!context) return;
    context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
    context.clearRect(0, 0, width, height);
    context.lineWidth = 2.5;
    context.lineCap = 'round';

    const spacing = 7;
    const count = Math.max(1, Math.min(levelsRef.current.length, Math.floor(width / spacing)));
    const startX = (width - ((count - 1) * spacing)) / 2;
    const offset = levelsRef.current.length - count;
    for (let index = 0; index < count; index += 1) {
      const rawLevel = levelsRef.current[offset + index] ?? 0;
      const visibleLevel = rawLevel <= 0.004
        ? 0
        : Math.min(1, Math.sqrt((rawLevel - 0.004) / 0.12));
      const barHeight = 2.5 + (visibleLevel * Math.max(0, height - 8));
      const x = startX + (index * spacing);
      context.strokeStyle = visibleLevel > 0.02 ? t.text3 : t.text4;
      context.globalAlpha = visibleLevel > 0.02 ? 0.82 : 0.48;
      context.beginPath();
      context.moveTo(x, (height - barHeight) / 2);
      context.lineTo(x, (height + barHeight) / 2);
      context.stroke();
    }
    context.globalAlpha = 1;
  }, [t.text3, t.text4]);

  useEffect(() => {
    drawWaveform();
    const canvas = canvasRef.current;
    if (!canvas || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(drawWaveform);
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [drawWaveform]);

  useEffect(() => {
    if (!audio) return;
    const unsubscribe = audio.onEvent((event) => {
      if (event.type !== 'input_level' || event.owner.id !== owner.id || event.owner.kind !== owner.kind) return;
      const levels = levelsRef.current;
      levels.copyWithin(0, 1);
      levels[levels.length - 1] = event.level;
      if (frameRef.current !== null) return;
      frameRef.current = window.requestAnimationFrame(() => {
        frameRef.current = null;
        drawWaveform();
      });
    });
    return () => {
      unsubscribe();
      if (frameRef.current !== null) window.cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    };
  }, [audio, drawWaveform, owner.id, owner.kind]);

  const actionStyle: CSSProperties = {
    width: 40,
    height: 40,
    flex: '0 0 40px',
    display: 'grid',
    placeItems: 'center',
    padding: 0,
    border: 0,
    borderRadius: '50%',
    background: t.surfaceHover,
    color: t.text,
    cursor: 'pointer',
  };

  return (
    <div role="group" aria-label="Audio dictation controls" style={{ display: 'flex', alignItems: 'center', gap: 12, minHeight: 48, padding: '0 8px 8px' }}>
      <button className="dictation-recorder-action" type="button" aria-label="Cancel dictation" title="Cancel dictation" onClick={onCancel} style={actionStyle}>
        <Icon name="x" size={16} color="currentColor" stroke={1.8} />
      </button>
      <canvas
        ref={canvasRef}
        role="img"
        aria-label="Live microphone amplitude"
        width={640}
        height={36}
        style={{ display: 'block', flex: '1 1 auto', width: '100%', minWidth: 0, height: 36 }}
      />
      <button className="dictation-recorder-action" type="button" aria-label="Stop dictation" title="Stop dictation" onClick={onFinish} style={actionStyle}>
        <span aria-hidden="true" style={{ width: 14, height: 14, borderRadius: 2, background: 'currentColor' }} />
      </button>
    </div>
  );
}

function resolvedVoiceLanguage(configured: string): string {
  if (configured === LANGUAGE_AUTO) {
    return typeof navigator !== 'undefined' && navigator.language ? navigator.language : 'en-US';
  }
  return configured;
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

export function BetaComposer({ bridge, ready, onOpenSettings, onOpenSettingsPage, onSetTheme, onOpenProviderSettings }: {
  bridge: UseBridge;
  ready: boolean;
  onOpenSettings(): void;
  onOpenSettingsPage(pageId: string): void;
  onSetTheme(theme: 'dark' | 'light'): void;
  onOpenProviderSettings(providerId: string, modelReference: string, restoreFocus: () => void): void;
}) {
  const t = useT();
  const [text, setText] = useState('');
  const [modelOpen, setModelOpen] = useState(false);
  const [modelSubmenu, setModelSubmenu] = useState<ModelPickerSubmenu>(null);
  const [modelQuery, setModelQuery] = useState('');
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
  const [voiceState, setVoiceState] = useState<'idle' | 'listening' | 'unsupported' | 'denied'>('idle');
  const [flowMode, setFlowMode] = useState(false);
  const [flowState, setFlowState] = useState<VoiceFlowState>(DEFAULT_VOICE_FLOW_STATE);
  const [slashResultIndex, setSlashResultIndex] = useState(0);
  const input = useRef<HTMLDivElement>(null);
  const fileControl = useRef<HTMLDivElement>(null);
  const fileSearchInput = useRef<HTMLInputElement>(null);
  const imageFileInput = useRef<HTMLInputElement>(null);
  const permissionControl = useRef<HTMLDivElement>(null);
  const permissionButton = useRef<HTMLButtonElement>(null);
  const modelControl = useRef<HTMLDivElement>(null);
  const modelTrigger = useRef<HTMLButtonElement>(null);
  const modelSearchInput = useRef<HTMLInputElement>(null);
  const slashControl = useRef<HTMLDivElement>(null);
  const dictationInsertionRange = useRef<Range | null>(null);
  const flowControllerRef = useRef<VoiceFlowController | null>(null);
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
  const audio = nativeAudioApi();
  const voicePrefs = bridge.bootstrap?.settings.voice ?? defaultVoicePreferences();
  const voicePrefsRef = useRef(voicePrefs);
  voicePrefsRef.current = voicePrefs;
  const modelPickerVisibility = bridge.bootstrap?.settings.modelPickerVisibility;
  const dictationOwner = useRef<NativeAudioOwner | null>(null);
  const autoplayOwner = useRef<NativeAudioOwner | null>(null);
  const autoplaySubscription = useRef<(() => void) | null>(null);
  const activeAudioSessionId = useRef(activeSessionId);
  activeAudioSessionId.current = activeSessionId;
  const flowModeRef = useRef(flowMode);
  flowModeRef.current = flowMode;
  const standardListeningRef = useRef(false);

  const slashCommands = useMemo(
    () => filterSlashCommands(bridge.desktop.slashCommands, slashQuery ?? ''),
    [bridge.desktop.slashCommands, slashQuery],
  );
  const visibleModelReferences = useMemo(
    () => filterVisibleModelReferences(
      bridge.desktop.models,
      bridge.desktop.providerModelCatalog,
      modelPickerVisibility,
    ),
    [bridge.desktop.models, bridge.desktop.providerModelCatalog, modelPickerVisibility],
  );
  const modelGroups = useMemo(
    () => groupModelReferences(visibleModelReferences),
    [visibleModelReferences],
  );
  const filteredModelGroups = useMemo(
    () => filterModelGroups(modelGroups, modelQuery, bridge.desktop.modelDetails),
    [bridge.desktop.modelDetails, modelGroups, modelQuery],
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
    openSettingsPage: onOpenSettingsPage,
    addWorkspaceDirectory: (path) => bridge.updateWorkspaceDirectories('project', [path], []),
    chooseProject: async () => { await bridge.addProject(); },
    activateProject: async (path) => { await bridge.activateProject(path); },
    clearSession: async () => {
      if (window.confirm('Clear the current session and start a new draft?')) await bridge.clearSession();
    },
    forceCompact: (instructions) => bridge.forceCompact(instructions),
    copyLastResponse: async () => {
      const item = [...bridge.conversation.items].reverse().find((entry) => (
        entry.type === 'narration' && entry.role === 'assistant' && entry.text.trim().length > 0
      ));
      if (!item || item.type !== 'narration') return false;
      await bridge.copyText(item.text);
      return true;
    },
    login: () => bridge.login(),
    logout: () => bridge.logout(),
    reloadPlugins: () => bridge.restartBridge(),
    openTasks: async () => {
      bridge.setRuntimeCenterOverviewOpen(true);
      await bridge.refreshTasks();
    },
    showHelp: () => bridge.emitCommandOutput(renderDesktopSlashHelp(bridge.desktop.slashCommands), false),
    emit: (output, isError) => bridge.emitCommandOutput(output, isError === true),
  }), [bridge, onOpenSettings, onOpenSettingsPage, onSetTheme]);

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
    if (modelOpen && modelSubmenu === 'model') {
      modelSearchInput.current?.focus();
      return;
    }
    setModelQuery('');
  }, [modelOpen, modelSubmenu]);

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
    void bridge.refreshSlashCommands().catch(() => undefined);
  }, [bridge.refreshSlashCommands, slashMenuOpen]);

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

  const insertVoiceTextAtSelection = (transcript: string, selection = dictationInsertionRange.current) => {
    const editor = input.current;
    if (!editor || transcript.length === 0) return;
    const insertion = selection?.cloneRange() ?? savedEditorSelection.current?.cloneRange() ?? editorSelection(editor);
    insertion.deleteContents();
    const node = document.createTextNode(transcript);
    insertion.insertNode(node);
    const caret = document.createRange();
    caret.setStart(node, node.length);
    caret.collapse(true);
    applyEditorSelection(caret);
    savedEditorSelection.current = caret.cloneRange();
    dictationInsertionRange.current = caret.cloneRange();
    syncPromptState();
    updateActiveCompletions();
  };

  const audioOwner = useCallback((kind: NativeAudioOwner['kind']): NativeAudioOwner => ({
    kind,
    id: `${activeSessionId ?? 'desktop'}:${kind}`,
  }), [activeSessionId]);

  const reportNativeAudioFailure = useCallback((response: NativeAudioResponse): boolean => {
    if (response.type !== 'error') return false;
    if (response.error.code === 'permission') setVoiceState('denied');
    else if (response.error.code === 'unavailable') setVoiceState('unsupported');
    else setVoiceState('idle');
    return true;
  }, []);

  const cancelStandardListening = useCallback(async () => {
    const owner = dictationOwner.current;
    standardListeningRef.current = false;
    dictationOwner.current = null;
    dictationInsertionRange.current = null;
    setVoiceState('idle');
    if (!audio || !owner) return;
    try {
      await audio.request({ type: 'cancel', owner });
    } catch {}
  }, [audio]);

  const cancelAutoplay = useCallback(async () => {
    autoplaySubscription.current?.();
    autoplaySubscription.current = null;
    const owner = autoplayOwner.current;
    autoplayOwner.current = null;
    if (!audio || !owner) return;
    try {
      await audio.request({ type: 'stop_speaking', owner });
    } catch {}
  }, [audio]);

  const startStandardListening = async () => {
    if (!audio) {
      setVoiceState('unsupported');
      return;
    }
    const editor = input.current;
    dictationInsertionRange.current = editor ? editorSelection(editor).cloneRange() : savedEditorSelection.current?.cloneRange() ?? null;
    const owner = audioOwner('dictation');
    dictationOwner.current = owner;
    const permissions = await audio.request({ type: 'request_authorization', permissions: ['microphone', 'speech'] });
    if (reportNativeAudioFailure(permissions)) {
      dictationOwner.current = null;
      dictationInsertionRange.current = null;
      return;
    }
    const response = await audio.request({
      type: 'start_listening',
      owner,
      recognitionMode: voicePrefs.recognitionMode,
      language: resolvedVoiceLanguage(voicePrefs.language),
    });
    if (reportNativeAudioFailure(response)) {
      dictationOwner.current = null;
      dictationInsertionRange.current = null;
      return;
    }
    standardListeningRef.current = true;
    setVoiceState('listening');
  };

  const finishStandardListening = async () => {
    if (!audio || !dictationOwner.current) return;
    const owner = dictationOwner.current;
    standardListeningRef.current = false;
    dictationOwner.current = null;
    const response = await audio.request({ type: 'finish_listening', owner });
    if (reportNativeAudioFailure(response)) {
      dictationInsertionRange.current = null;
      return;
    }
    setVoiceState('idle');
    if (response.type === 'listening_finished') {
      const transcript = response.transcript?.text.trim() ?? '';
      if (transcript) insertVoiceTextAtSelection(transcript);
    }
    dictationInsertionRange.current = null;
  };

  const stopFlowMode = useCallback(async () => {
    setFlowMode(false);
    await flowControllerRef.current?.stop();
  }, []);

  const retryFlowMode = useCallback(() => {
    if (!flowModeRef.current) setFlowMode(true);
    void flowControllerRef.current?.retry();
  }, []);

  useEffect(() => {
    if (!audio) {
      flowControllerRef.current = null;
      setVoiceState('unsupported');
      setFlowState({
        ...DEFAULT_VOICE_FLOW_STATE,
        phase: 'configurationRequired',
        detail: 'Native audio is unavailable in this environment.',
      });
      return;
    }
    const controller = new VoiceFlowController({
      audio: {
        request: (command) => audio.request(command),
        onEvent: (listener) => audio.onEvent(listener),
      },
      bridge: {
        sendTrackedPrompt: bridge.sendTrackedPrompt,
        subscribeTrackedSpeech: bridge.subscribeTrackedSpeech,
        cancel: bridge.cancel,
      },
      getPreferences: () => voicePrefsRef.current,
      createOwner: audioOwner,
      timers: {
        setTimeout: (callback, delayMs) => window.setTimeout(callback, delayMs),
        clearTimeout: (handle) => window.clearTimeout(handle as number),
      },
      onStateChange: (state) => setFlowState(state),
    });
    flowControllerRef.current = controller;
    setFlowState(controller.getState());
    return () => {
      if (flowControllerRef.current === controller) flowControllerRef.current = null;
      controller.dispose();
    };
  }, [audio, audioOwner, bridge.cancel, bridge.sendTrackedPrompt, bridge.subscribeTrackedSpeech]);

  useEffect(() => {
    const controller = flowControllerRef.current;
    if (!controller) return;
    if (flowMode) {
      void controller.start();
      return;
    }
    void controller.stop();
  }, [flowMode]);

  useEffect(() => {
    if (!audio) {
      setVoiceState('unsupported');
      return;
    }
    return audio.onEvent((event) => {
      if (event.type === 'speech_state'
          && autoplayOwner.current?.id === event.owner.id
          && (event.state === 'finished' || event.state === 'interrupted')) {
        autoplayOwner.current = null;
      }
      if (event.type === 'error' && autoplayOwner.current?.id === event.owner?.id) {
        autoplayOwner.current = null;
      }
      if (event.type === 'error' && event.owner && dictationOwner.current && event.owner.id === dictationOwner.current.id) {
        standardListeningRef.current = false;
        dictationOwner.current = null;
        dictationInsertionRange.current = null;
        setVoiceState(event.error.code === 'permission' ? 'denied' : 'idle');
      }
    });
  }, [audio]);

  const previousAudioSessionId = useRef(activeSessionId);
  useEffect(() => {
    if (previousAudioSessionId.current === activeSessionId) return;
    previousAudioSessionId.current = activeSessionId;
    if (standardListeningRef.current) void cancelStandardListening();
    void cancelAutoplay();
    if (flowModeRef.current) setFlowMode(false);
  }, [activeSessionId, cancelAutoplay, cancelStandardListening]);

  useEffect(() => {
    const handleHidden = () => {
      if (!document.hidden) return;
      if (standardListeningRef.current) void cancelStandardListening();
      void cancelAutoplay();
      if (flowModeRef.current) setFlowMode(false);
    };
    document.addEventListener('visibilitychange', handleHidden);
    return () => document.removeEventListener('visibilitychange', handleHidden);
  }, [cancelAutoplay, cancelStandardListening]);

  useEffect(() => () => {
    if (standardListeningRef.current) void cancelStandardListening();
    void cancelAutoplay();
    flowControllerRef.current?.dispose();
  }, [cancelAutoplay, cancelStandardListening]);

  const stopVoice = () => {
    if (flowModeRef.current) {
      void stopFlowMode();
      return;
    }
    if (standardListeningRef.current) {
      void finishStandardListening();
      return;
    }
    setVoiceState('idle');
  };

  const toggleStandardVoice = () => {
    if (flowModeRef.current) {
      void stopFlowMode();
      return;
    }
    if (voiceState === 'denied') {
      void bridge.openSystemSettings('microphone');
      return;
    }
    if (standardListeningRef.current) {
      void finishStandardListening();
      return;
    }
    void startStandardListening();
  };

  const toggleFlowMode = () => {
    if (flowModeRef.current) {
      void stopFlowMode();
      return;
    }
    setFlowMode(true);
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
    if (!ready || flowModeRef.current) return;
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
      const resolved = resolveDesktopCommand(slashCommand, ALL_DESKTOP_COMMANDS);
      if (resolved && !desktopCommandIsShadowed(slashCommand, bridge.desktop.slashCommands)) {
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
      const supportsTrackedSend = typeof bridge.sendTrackedPrompt === 'function';
      const tracked = supportsTrackedSend
        ? bridge.sendTrackedPrompt(
            value,
            images,
            imageAttachments.map((attachment) => attachment.name),
            snapshot.files,
          )
        : null;
      if (supportsTrackedSend && !tracked) return;
      const queued = tracked?.queued ?? bridge.sendPrompt(
        value,
        images,
        imageAttachments.map((attachment) => attachment.name),
        snapshot.files,
      );
      if (voicePrefs.autoPlayReplies && audio && tracked) {
        await cancelAutoplay();
        const autoplayTokenOwner = { kind: 'autoplay', id: tracked.token.clientTurnId } satisfies NativeAudioOwner;
        autoplayOwner.current = autoplayTokenOwner;
        const offTrackedSpeech = bridge.subscribeTrackedSpeech(tracked.token, (event) => {
          if (event.type !== 'completion') return;
          offTrackedSpeech();
          if (autoplaySubscription.current === offTrackedSpeech) autoplaySubscription.current = null;
          if (!shouldAutoplayTrackedReply(activeAudioSessionId.current, tracked.token.sessionId, document.hidden)) {
            autoplayOwner.current = null;
            return;
          }
          const spoken = sanitizeSpeakableText(event.text);
          if (!spoken) {
            autoplayOwner.current = null;
            return;
          }
          void audio.request({
            type: 'speak',
            owner: autoplayTokenOwner,
            text: spoken,
            voiceId: voicePrefs.voiceSelection,
            rate: voicePrefs.rate,
          }).then((response) => {
            if (response.type === 'error' && autoplayOwner.current?.id === autoplayTokenOwner.id) {
              autoplayOwner.current = null;
            }
          }).catch(() => {
            if (autoplayOwner.current?.id === autoplayTokenOwner.id) autoplayOwner.current = null;
          });
        });
        autoplaySubscription.current = offTrackedSpeech;
      }
      await queued;
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
  const permissionMode = PERMISSION_MODE_OPTIONS.find((mode) => mode.id === bridge.desktop.permissionMode) ?? PERMISSION_MODE_OPTIONS[0]!;
  const workspace = bridge.bootstrap?.workspace;
  const providerConfigured = bridge.bootstrap?.providerCredentials?.some((entry) => entry.configured) ?? false;
  const goalActive = composerGoalActive(bridge.conversation?.items ?? []);
  const promptPlaceholder = !ready
    ? !workspace?.path || workspace.recovery
      ? 'Add or select an available project to start coding…'
      : !providerConfigured
        ? 'Connect a provider in Settings to start coding…'
        : 'Waiting for the local engine…'
    : goalActive
        ? 'Describe the goal you want LingXi to accomplish'
        : bridge.running
          ? 'LingXi is working — draft your next message…'
          : 'Do anything';
  const hasPrompt = Boolean(text.trim() || selectedFiles.length);
  return (
    <div className="desktop-composer-dock" style={{ flexShrink: 0, padding: '12px var(--conversation-gutter) 20px', background: t.stageBg }}>
      {flowMode && (
        <VoiceFlowPanel
          state={flowState}
          onOrb={() => { void flowControllerRef.current?.orb(); }}
          onRetry={retryFlowMode}
          onOpenSettings={() => onOpenSettingsPage('voice')}
          onClose={() => { void stopFlowMode(); }}
        />
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
        style={{ position: 'relative', maxWidth: 'var(--conversation-width)', margin: '0 auto', borderRadius: 20, border: `1px solid ${imageDragActive ? t.accent : t.border}`, background: imageDragActive ? t.accentBg : t.surface, boxShadow: t.dark ? '0 8px 24px rgba(0,0,0,.20), 0 1px 3px rgba(0,0,0,.16)' : '0 8px 24px rgba(24,28,36,.06), 0 1px 3px rgba(24,28,36,.04)', overflow: 'visible', transition: 'border-color 0.16s ease, background-color 0.16s ease, box-shadow 0.16s ease' }}
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
          style={{ display: 'block', width: '100%', minHeight: 56, maxHeight: 160, overflowY: 'auto', border: 0, outline: 0, background: 'transparent', color: t.text, lineHeight: 1.5, fontSize: 15, padding: '16px 18px 8px', fontWeight: 400, letterSpacing: 'normal', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere', cursor: ready ? 'text' : 'default', opacity: ready ? 1 : .68 }}
        />
        {slashMenuOpen && (
          <div ref={slashControl} id="slash-command-results" role="listbox" aria-label="Slash commands" style={{ ...composerMenuStyle(t, 'left'), width: 820, maxWidth: 'min(820px, calc(100vw - 44px))', maxHeight: 'min(420px, calc(100vh - 190px))', overflowY: 'auto', padding: 7 }}>
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
              const description = slashMenuLabel(entry);
              return (
                <button
                  id={`slash-command-result-${index}`}
                  data-slash-index={index}
                  className="slash-command-row"
                  key={entry.name}
                  type="button"
                  role="option"
                  aria-selected={selected}
                  title={description}
                  onMouseDown={(event) => event.preventDefault()}
                  onMouseEnter={() => setSlashResultIndex(index)}
                  onFocus={() => setSlashResultIndex(index)}
                  onClick={() => chooseSlashCommand(entry.name)}
                  style={{ width: '100%', minHeight: 44, display: 'grid', gridTemplateColumns: '26px minmax(0, 1fr)', gap: 10, alignItems: 'center', padding: '7px 10px', border: 0, borderRadius: 9, background: selected ? t.surfaceHover : 'transparent', color: t.text, textAlign: 'left', cursor: 'pointer', font: 'inherit' }}
                >
                  <span aria-hidden="true" style={{ width: 26, height: 26, display: 'grid', placeItems: 'center', color: t.text2 }}>
                    <Icon name={commandPaletteIcon(entry.name)} size={18} stroke={1.7} />
                  </span>
                  <span style={{ minWidth: 0, display: 'flex', alignItems: 'baseline', gap: 10, overflow: 'hidden', whiteSpace: 'nowrap' }}>
                    <span className="slash-command-name" style={{ flexShrink: 0, color: t.text, fontWeight: 500, fontSize: 14.5, letterSpacing: '-.01em' }}>
                      {entry.name}
                    </span>
                    {entry.argument_hint ? <span style={{ flexShrink: 0, color: t.text4, fontSize: 12.5 }}>{entry.argument_hint}</span> : null}
                    <span className="slash-command-description" style={{ minWidth: 0, overflow: 'hidden', color: t.text3, fontSize: 13.5, lineHeight: 1.35, textOverflow: 'ellipsis' }}>{description}</span>
                  </span>
                </button>
              );
            })}
            <div style={{ display: 'flex', alignItems: 'center', gap: 10, minHeight: 26, padding: '5px 9px 2px', borderTop: `0.5px solid ${t.border}`, color: t.text4, fontSize: 9.5 }}>
              <span>↑↓ Navigate</span><span>Enter / Tab Complete</span><span>Esc Close</span>
            </div>
          </div>
        )}
        {voiceState === 'listening' && !flowMode ? (
          <DictationRecorderBar
            audio={audio}
            owner={dictationOwner.current ?? audioOwner('dictation')}
            onCancel={() => { void cancelStandardListening(); }}
            onFinish={() => { void finishStandardListening(); }}
          />
        ) : (
        <div className="composer-toolbar" style={{ display: 'flex', alignItems: 'center', gap: 4, minHeight: 48, padding: '0 8px 8px' }}>
          <input ref={imageFileInput} type="file" accept="image/png,image/jpeg,image/gif,image/webp" multiple onChange={(event) => { void addImageFiles(event.target.files ? [...event.target.files] : []); event.currentTarget.value = ''; }} style={{ display: 'none' }} />
          <button type="button" disabled={!ready} aria-label="Attach image" title="Attach image" onClick={() => imageFileInput.current?.click()} style={{ ...composerIconStyle(t), width: 40, height: 40 }}><Icon name="image" size={19} color={t.text2} stroke={1.7} /></button>
          <div ref={fileControl}>
            <button type="button" disabled={!ready} aria-label="Search workspace files" aria-expanded={fileMenuOpen} title="Add file context (@)" onMouseDown={savePromptSelection} onClick={openFileMenu} style={{ ...composerIconStyle(t), width: 40, height: 40 }}><Icon name="plus" size={21} color={t.text2} stroke={1.7} /></button>
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
                {PERMISSION_MODE_OPTIONS.map((mode) => {
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
          {goalActive && (
            <>
              <span aria-hidden="true" style={{ width: 1, height: 24, background: t.border, margin: '0 4px' }} />
              <div role="status" aria-label="Goal active" title="Session goal is active. Use /goal clear to stop it." style={{ ...composerPillStyle(t, true), minWidth: 40, minHeight: 40, justifyContent: 'center', padding: '0 10px', borderRadius: 12, color: t.accent }}>
                <Icon name="goal" size={18} color={t.accent} stroke={1.6} />
                <span>Goal</span>
              </div>
            </>
          )}

          <div style={{ flex: 1 }} />

          <div ref={modelControl} className="composer-model-control" style={{ position: 'relative', minWidth: 0 }}>
            <button
              ref={modelTrigger}
              type="button"
              disabled={!ready || bridge.running || bridge.desktop.models.length === 0}
              aria-haspopup="menu"
              aria-expanded={modelOpen}
              aria-label={`${fastModeAvailable && bridge.desktop.fastMode ? 'Fast mode, ' : ''}Model: ${modelLabel(bridge.desktop.currentModel)}, reasoning ${reasoningSelectionLabel(selectedReasoning)}`}
              onMouseDown={() => { setSlashQuery(null); activeSlashRange.current = null; }}
              onClick={() => { setModelOpen((open) => !open); setModelSubmenu(null); setPermissionOpen(false); }}
              style={{ ...composerPillStyle(t, modelOpen), maxWidth: '100%', width: '100%', color: t.text }}
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

                {modelSubmenu === 'model' && (
                  <div className="model-picker-model-submenu" style={{ ...modelPickerSubmenuStyle(t), width: 390, maxWidth: 'min(390px, calc(100vw - 44px))', maxHeight: 'min(500px, calc(100vh - 140px))', overflow: 'hidden', display: 'flex', flexDirection: 'column' }} role="menu" aria-label="Available models">
                    <div style={{ flexShrink: 0, padding: '5px 10px 7px', color: t.text3, fontSize: 12, fontWeight: 600 }}>Model</div>
                    <div style={{ position: 'relative', flexShrink: 0, margin: '0 3px 7px' }}>
                      <span aria-hidden="true" style={{ position: 'absolute', left: 10, top: '50%', display: 'grid', placeItems: 'center', transform: 'translateY(-50%)', pointerEvents: 'none' }}>
                        <Icon name="search" size={14} color={t.text4} stroke={1.8} />
                      </span>
                      <input
                        ref={modelSearchInput}
                        type="text"
                        value={modelQuery}
                        aria-label="Search models"
                        placeholder="Search models"
                        autoComplete="off"
                        spellCheck={false}
                        onChange={(event) => setModelQuery(event.currentTarget.value)}
                        onKeyDown={(event) => {
                          if (event.key !== 'Escape') return;
                          event.preventDefault();
                          event.stopPropagation();
                          if (modelQuery) setModelQuery('');
                          else setModelSubmenu(null);
                        }}
                        style={{ width: '100%', height: 34, padding: '0 32px 0 31px', border: `0.5px solid ${t.borderStrong}`, borderRadius: 8, outline: 'none', background: t.surfaceActive, color: t.text, font: 'inherit', fontSize: 12.5 }}
                        onFocus={(event) => { event.currentTarget.style.borderColor = t.accent; }}
                        onBlur={(event) => { event.currentTarget.style.borderColor = t.borderStrong; }}
                      />
                      {modelQuery && (
                        <button type="button" aria-label="Clear model search" onClick={() => { setModelQuery(''); modelSearchInput.current?.focus(); }} style={{ position: 'absolute', right: 5, top: '50%', display: 'grid', width: 24, height: 24, padding: 0, placeItems: 'center', transform: 'translateY(-50%)', border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' }}>
                          <Icon name="x" size={12} color={t.text3} stroke={1.9} />
                        </button>
                      )}
                    </div>
                    <div style={{ flex: '1 1 auto', minHeight: 0, overflowY: 'auto', padding: '0 3px 3px', scrollbarGutter: 'stable' }}>
                      {filteredModelGroups.map((group, groupIndex) => {
                        const statusProviderId = group.providerId === 'builtin' ? 'anthropic' : group.providerId;
                        const metadata = statusProviderId
                          ? providerCredentials?.find((entry) => entry.providerId === statusProviderId)
                          : undefined;
                        const knownProvider = Boolean(statusProviderId && providerById(statusProviderId));
                        const connectionLabel = knownProvider
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
                            {modelBillingGroups(group, bridge.desktop.modelDetails).map((billingGroup, billingIndex) => (
                              <div key={billingGroup.label ?? 'all'} style={billingIndex === 0 ? undefined : { marginTop: 4, paddingTop: 4, borderTop: `0.5px solid ${t.border}` }}>
                                {billingGroup.label && <div style={{ padding: '5px 10px 3px', color: t.text4, fontSize: 9.5, fontWeight: 700, letterSpacing: '.06em', textTransform: 'uppercase' }}>{billingGroup.label}</div>}
                                {billingGroup.models.map((entry) => {
                                  const active = entry.reference === bridge.desktop.currentModel;
                                  const entryDetail = modelDetailsByReference.get(entry.reference);
                                  const entrySupportsFastMode = modelReference(entry.reference).providerId === 'anthropic'
                                    && modelSupportsFastMode(entryDetail);
                                  const selection = resolveModelSelection(entry.reference, providerCredentials);
                                  const unavailable = selection.kind === 'loading';
                                  const requiresConnection = selection.kind === 'connect';
                                  return (
                                    <button key={entry.reference} type="button" role="menuitemradio" aria-checked={active} disabled={unavailable} aria-disabled={unavailable} title={unavailable ? 'Checking provider connection…' : requiresConnection ? `Connect ${providerById(selection.providerId)?.label ?? selection.providerId} in Settings to use this model` : entry.requestModel} onClick={() => {
                                      if (selection.kind === 'loading') return;
                                      setModelOpen(false);
                                      setModelSubmenu(null);
                                      if (selection.kind === 'connect') onOpenProviderSettings(selection.providerId, entry.reference, () => modelTrigger.current?.focus());
                                      else invoke(() => bridge.setModel(entry.reference));
                                    }} style={{ display: 'flex', alignItems: 'center', gap: 9, width: '100%', minHeight: 40, padding: '7px 10px', border: 0, borderRadius: 7, background: active ? t.accentBg : 'transparent', color: unavailable ? t.text4 : requiresConnection ? t.text2 : t.text, textAlign: 'left', cursor: unavailable ? 'wait' : 'pointer', font: 'inherit', fontSize: 12.5, opacity: unavailable ? .68 : 1 }} onMouseEnter={(event) => { if (!active && !unavailable) event.currentTarget.style.background = t.surfaceHover; }} onMouseLeave={(event) => { if (!active && !unavailable) event.currentTarget.style.background = 'transparent'; }}>
                                      {unavailable ? <span className="beta-spinner" aria-hidden="true" style={{ width: 11, height: 11, borderWidth: 1.5, color: t.text4 }} /> : requiresConnection ? <Icon name="lock" size={14} color={t.warn} /> : entrySupportsFastMode && <Icon name="bolt" size={14} color={active ? t.accent : t.text3} />}
                                      <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', fontWeight: active ? 650 : 500 }}>{modelDisplayLabel(entry, bridge.desktop.modelDetails)}</span>
                                      {requiresConnection && <span style={{ flexShrink: 0, color: t.warn, fontSize: 10, fontWeight: 600 }}>Connect in Settings</span>}
                                      {active && <Icon name="check" size={14} color={t.accent} stroke={2.2} />}
                                    </button>
                                  );
                                })}
                              </div>
                            ))}
                          </section>
                        );
                      })}
                      {filteredModelGroups.length === 0 && (
                        <div role="status" style={{ padding: '28px 16px 30px', color: t.text4, fontSize: 12, textAlign: 'center' }}>
                          {modelQuery.trim()
                            ? `No models match “${modelQuery.trim()}”`
                            : visibleModelReferences.length === 0
                              ? 'No models are visible in this picker'
                              : 'No models match your current filters'}
                        </div>
                      )}
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
          <button type="button" disabled={!ready || flowMode} aria-label={voiceState === 'listening' && !flowMode ? 'Stop ordinary recording' : 'Start ordinary recording'} title={voiceState === 'unsupported' ? 'Voice input is unavailable in this environment' : voiceState === 'denied' ? 'Microphone permission was denied' : '普通录音'} onClick={toggleStandardVoice} style={{ ...composerPrimaryActionStyle(t, ready && !flowMode), color: voiceState === 'listening' && !flowMode ? t.accent : voiceState === 'denied' ? t.danger : t.text }}><Icon name="mic" size={18} color="currentColor" stroke={voiceState === 'listening' && !flowMode ? 2.1 : 1.8} /></button>
          <button
            type="button"
            disabled={!ready}
            aria-label={flowMode ? '关闭心流模式' : '开启心流模式'}
            aria-pressed={flowMode}
            title={flowMode ? '关闭心流模式' : '开启心流模式'}
            onClick={toggleFlowMode}
            style={{ ...composerPrimaryActionStyle(t, ready), color: flowMode ? t.accent : t.text }}
          >
            <Icon name="waveform" size={18} color="currentColor" stroke={2.15} />
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
            disabled={!ready || !hasPrompt || flowMode}
            onClick={() => { void submit(); }}
            aria-label={bridge.running ? 'Send pending message' : 'Send prompt'}
            title={bridge.running ? 'Send as pending message' : 'Send prompt'}
            style={composerSendStyle(t, Boolean(ready && hasPrompt && !flowMode))}
          ><Icon name="arrowU" size={18} color={ready && hasPrompt && !flowMode ? '#fff' : t.text4} /></button>
        </div>
        )}
        {(voiceState === 'unsupported' || voiceState === 'denied') && <div style={{ position: 'relative' }}>
          {voiceState === 'unsupported' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.text3, fontSize: 10.5 }}>Voice input is unavailable here</span>}
          {voiceState === 'denied' && <span role="status" style={{ position: 'absolute', right: 52, bottom: 9, padding: '5px 8px', borderRadius: 7, background: t.surfaceHover, color: t.danger, fontSize: 10.5 }}>Microphone permission denied</span>}
        </div>}
      </div>
    </div>
  );
}

function composerIconStyle(t: ReturnType<typeof useT>): CSSProperties {
  return { display: 'grid', placeItems: 'center', border: 0, borderRadius: 99, background: 'transparent', color: t.text2, cursor: 'pointer', opacity: 1 };
}

function composerPrimaryActionStyle(t: ReturnType<typeof useT>, enabled: boolean): CSSProperties {
  return { width: 40, height: 40, flex: '0 0 40px', borderRadius: '50%', border: 0, background: 'transparent', color: t.text2, display: 'grid', placeItems: 'center', cursor: enabled ? 'pointer' : 'not-allowed', opacity: enabled ? 1 : .62 };
}

function composerPillStyle(t: ReturnType<typeof useT>, active: boolean): CSSProperties {
  return { display: 'inline-flex', alignItems: 'center', gap: 6, minHeight: 40, padding: '0 9px', border: 0, borderRadius: 9, background: active ? t.surfaceHover : 'transparent', cursor: 'pointer', font: 'inherit', fontSize: 12.5, fontWeight: 500 };
}

function composerSendStyle(t: ReturnType<typeof useT>, enabled: boolean): CSSProperties {
  return { ...composerPrimaryActionStyle(t, enabled), background: enabled ? t.accent : t.surfaceActive, color: enabled ? '#fff' : t.text4, opacity: enabled ? 1 : .82 };
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
