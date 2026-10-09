import { composerGoalState } from './goalPresentation';
import { GoalStatus } from './GoalStatus';
import { createPortal } from 'react-dom';
import { DragDropProvider, DragOverlay, type DragEndEvent } from '@dnd-kit/react';
import { isSortable, useSortable } from '@dnd-kit/react/sortable';
import { PointerActivationConstraints, PointerSensor } from '@dnd-kit/dom';
import { ContextWindow } from './ContextWindow';
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ClipboardEvent,
  type KeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from 'react';
import type {
  ImageRefDto,
  ModelDetailsDto,
  ReasoningSelectionDto,
  SessionRowDto,
} from '@lingxi/bridge-client';

import type { UseBridge } from '../bridge/bridgeTypes.js';
import { isSideQuestionCommand } from '../bridge/sideQuestion';
import type { ContextSummarySnapshot } from '../bridge/conversation';
import { matchesComposerSubmission, type ComposerSubmissionSnapshot } from '../bridge/composerSubmission';
import type { NativeAudioApi } from '../bridge/lingxi';
import { orderedTasks } from '../bridge/desktopState';
import { runningSubagentIds } from '../bridge/runtimeCenterState';
import { useT } from '../theme/ThemeContext';
import { settingsLabel } from '../settingsLabel';
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
import { RenameSessionDialog } from './RenameSessionDialog';
import type { ScheduledCronJob } from '../bridge/scheduledTaskDraft';
import { Icon } from './Icon';
import { commandPaletteColor } from './commandPaletteIcons';
import { CommandIcon } from './CommandIdentity';
import { parseSlashCommandPrefix } from './slashCommandMessage';
import { MentionMenu, commandMenuStyle } from './MentionMenu';
import { FileMentionPreview } from './FileMentionPreview';
import {
  contextMentionHref, contextMentionMarkdown, mentionCatalog, mentionMenuEntries,
  OPEN_CONTEXT_MENTION_EVENT, parseContextMentionHref,
  type ContextMention, type MentionMenuEntry,
} from '../bridge/composerMentions';
import { MarkdownContent } from './MarkdownContent';
import { VoiceFlowPanel } from './voice/VoiceFlowPanel';
import { providerById } from '../../shared/providers';
import { MAX_IMAGE_ATTACHMENTS } from '../../shared/imageInput';
import {
  audioConfigurationDefaults,
  resolveAudioLanguage,
} from '../../shared/generatedAudioConfiguration';
import type { AudioOperationResultDto } from '@lingxi/bridge-client';
import type { NativeAudioOwner } from '../../shared/nativeAudio';
import {
  modelPickerSubmenuPlacement,
  MODEL_PICKER_MENU_WIDTH,
  MODEL_PICKER_SUBMENU_GAP,
  type ModelPickerSubmenuBounds,
  type ModelPickerSubmenuPlacement,
} from './modelPickerPlacement';
import { PERMISSION_MODE_OPTIONS } from '../model/permissionModes';
import type { RunItem, VisualizationContextChip } from '../model/runItem';
import { VisualizationContextBadge } from './VisualizationCard';
import type { SidebarPreferences } from '../../shared/settings';

export const SIDEBAR_DEFAULT_WIDTH = 308;
export const SIDEBAR_MIN_WIDTH = 240;
export const SIDEBAR_MAX_WIDTH = 480;
const SIDEBAR_KEYBOARD_STEP = 16;
const PROJECT_ACTIONS_HIDE_DELAY = 700;
const SESSION_ACTIONS_LONG_PRESS_DELAY = 500;
const SESSION_POINTER_SENSOR = PointerSensor.configure({
  activationConstraints: [new PointerActivationConstraints.Distance({ value: 8 })],
});

type SidebarOrganization = 'project' | 'list';
type ChatSort = 'priority' | 'updated' | 'manual';

function sidebarSessionSortId(kind: 'sortable' | 'pinned' | 'static', projectPath: string, sessionId: string): string {
  return `sidebar-session:${kind}:${encodeURIComponent(projectPath)}:${encodeURIComponent(sessionId)}`;
}

function sidebarSessionTimestamp(session: SessionRowDto): number {
  const value = Date.parse(session.modified_rfc3339);
  return Number.isNaN(value) ? 0 : value;
}

/** Renderer-created or first-message rows have not received a durable path yet. */
function compareNewSidebarSessions(left: SessionRowDto, right: SessionRowDto): number {
  const leftIsNew = left.path === '';
  const rightIsNew = right.path === '';
  if (leftIsNew === rightIsNew) return 0;
  return leftIsNew ? -1 : 1;
}

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

export function composerGoalActive(items: readonly RunItem[]): boolean {
  return composerGoalState(items).active;
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

function SidebarSessionProgress({ label }: { label: string }) {
  const t = useT();
  return <span className="sidebar-session-progress" role="progressbar" aria-label={label}
    style={{ color: t.text3 }}><span /></span>;
}

function SessionRow({ projectPath, session, active, pinned, opening, status, metadata, pinnedSection = false, onClick, onPin, onArchive, onRename, reorderable = false, sortableIndex = 0 }: {
  projectPath: string;
  session: Pick<SessionRowDto, 'uuid' | 'title' | 'modified_rfc3339' | 'message_count'>;
  metadata?: string;
  pinnedSection?: boolean;
  active: boolean;
  pinned: boolean;
  opening: boolean;
  status?: ReturnType<UseBridge['sessionRuntimeStatus']>;
  onClick(): void;
  onPin(): void;
  onArchive(): void;
  onRename(): void;
  reorderable?: boolean;
  sortableIndex?: number;
}) {
  const t = useT();
  const [actionsPosition, setActionsPosition] = useState<{ left: number; top: number } | null>(null);
  const actionsMenuRef = useRef<HTMLDivElement>(null);
  const longPressTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pointerGesture = useRef<{ pointerId: number; x: number; y: number } | null>(null);
  const suppressClick = useRef(false);
  const title = session.title || 'Untitled session';
  const sessionMetadata = metadata ?? formatSessionMetadata(session.modified_rfc3339, session.message_count);
  const sortable = useSortable({
    id: sidebarSessionSortId(reorderable ? 'sortable' : pinnedSection ? 'pinned' : 'static', projectPath, session.uuid),
    index: sortableIndex,
    group: reorderable ? projectPath : undefined,
    disabled: !reorderable || actionsPosition !== null,
    data: {
      projectPath,
      sessionId: session.uuid,
      sessionTitle: title,
      sessionMetadata,
      pinnedSection,
    },
  });
  const highlighted = active || opening;
  const attention = opening
    ? { label: 'Opening session', color: t.accent }
    : status?.connection.status === 'error' || status?.error
    ? { label: 'Session error', color: t.danger }
    : status?.pendingInteractions
      ? { label: 'Waiting for input', color: t.warn }
      : (status?.turnActive || status?.backgroundAgentsRunning)
        ? { label: 'Running', color: t.ok }
        : undefined;
  const clearLongPress = useCallback(() => {
    if (longPressTimer.current) clearTimeout(longPressTimer.current);
    longPressTimer.current = null;
  }, []);
  const closeActions = useCallback(() => setActionsPosition(null), []);
  useEffect(() => () => clearLongPress(), [clearLongPress]);
  useEffect(() => {
    if (!actionsPosition) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      if (!actionsMenuRef.current?.contains(event.target as Node)) closeActions();
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); closeActions(); }
    };
    document.addEventListener('pointerdown', closeOnOutsidePointer, true);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOnOutsidePointer, true);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [actionsPosition, closeActions]);
  const openActions = useCallback((left: number, top: number) => {
    suppressClick.current = true;
    setActionsPosition({ left, top });
  }, []);
  return (
    <div
      ref={sortable.ref}
      className="sidebar-tree-row"
      data-session-reorderable={reorderable || undefined}
      data-dragging={sortable.isDragging || undefined}
      data-drag-target={sortable.isDropTarget || undefined}
      style={{ position: 'relative' }}
    >
      <button
        type="button"
        ref={reorderable ? sortable.handleRef : undefined}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          clearLongPress();
          suppressClick.current = false;
          const { clientX, clientY } = event;
          event.currentTarget.setPointerCapture(event.pointerId);
          pointerGesture.current = { pointerId: event.pointerId, x: event.clientX, y: event.clientY };
          longPressTimer.current = setTimeout(() => {
            pointerGesture.current = null;
            openActions(clientX, clientY);
          }, SESSION_ACTIONS_LONG_PRESS_DELAY);
        }}
        onPointerMove={(event) => {
          const gesture = pointerGesture.current;
          if (!gesture || gesture.pointerId !== event.pointerId) return;
          if (Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) > 8) {
            clearLongPress();
            if (reorderable && actionsPosition === null) suppressClick.current = true;
          }
        }}
        onPointerUp={(event) => {
          const gesture = pointerGesture.current;
          clearLongPress();
          if (gesture?.pointerId === event.pointerId) {
            pointerGesture.current = null;
          }
          if (event.currentTarget.hasPointerCapture(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId);
        }}
        onPointerCancel={(event) => {
          const gesture = pointerGesture.current;
          clearLongPress();
          if (gesture?.pointerId === event.pointerId) pointerGesture.current = null;
        }}
        onLostPointerCapture={() => {
          clearLongPress();
          pointerGesture.current = null;
        }}
        onContextMenu={(event) => {
          event.preventDefault();
          clearLongPress();
          openActions(event.clientX, event.clientY);
        }}
        onClick={(event) => {
          if (suppressClick.current) { event.preventDefault(); suppressClick.current = false; return; }
          onClick();
        }}
        disabled={opening}
        aria-busy={opening || undefined}
        data-session-id={session.uuid}
        aria-current={active ? 'page' : undefined}
        title={pinnedSection ? `${title}\n${projectPath}` : title}
        aria-label={`${title}, ${sessionMetadata}`}
        data-session-project-path={projectPath}
        style={{
          width: '100%', minHeight: 34, display: 'flex', alignItems: 'center', gap: pinnedSection ? 5 : 10,
          padding: pinnedSection ? '5px 64px 5px 10px' : '5px 64px 5px 30px', borderRadius: 9, border: 0, textAlign: 'left',
          touchAction: 'none', userSelect: 'none',
          background: active ? t.surfaceActive : opening ? t.surface : 'transparent', color: highlighted ? t.text : t.text2,
          cursor: opening ? 'wait' : 'pointer',
          fontSize: 13, fontWeight: active ? 600 : 400,
        }}
      >
        {pinnedSection && <Icon name="chat" size={15} stroke={1.7} />}
        <span style={{ display: 'flex', alignItems: 'center', minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{title}</span>
          {opening
            ? <SidebarSessionProgress label="Opening session" />
            // A session ERROR shows as a ringed mark at this row's trailing edge
            // instead (below), so it is not repeated inline here.
            : attention?.label === 'Running' ? <SidebarSessionProgress label="Running" />
            : attention && (pinnedSection || attention.label !== 'Session error') ? <span aria-label={attention.label} title={attention.label} style={{ flexShrink: 0, width: 6, height: 6, marginLeft: 7, borderRadius: 99, background: attention.color }} /> : null}
        </span>
        <span className="sidebar-session-metadata" style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 10.5 }}>
          {sessionMetadata}
        </span>
      </button>
      <button type="button" className="sidebar-row-action" aria-label={`${pinned ? 'Unpin' : 'Pin'} ${session.title || 'Untitled session'}`}
        title={pinned ? 'Unpin session' : 'Pin session'} onClick={onPin} disabled={opening}
        style={{ position: 'absolute', right: 32, top: 8, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: 'transparent', color: pinned ? t.accent : t.text3, cursor: 'pointer' }}>
        <Icon name="pin" size={13} stroke={1.8} />
      </button>
      <button type="button" className="sidebar-row-action" aria-label={`Archive ${session.title || 'Untitled session'}`}
        title="Archive chat" onClick={onArchive} disabled={opening}
        style={{ position: 'absolute', right: 4, top: 8, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: 'pointer' }}>
        <Icon name="archive" size={13} />
      </button>
      {!pinnedSection && attention?.label === 'Session error' && (
        // Sits where the pin/archive actions sit, and yields to them: the row
        // reveals those on hover/focus, and two marks in one slot would collide.
        <span
          className="sidebar-session-error"
          role="img"
          aria-label={attention.label}
          title={attention.label}
          style={{ position: 'absolute', right: 8, top: 4, width: 26, height: 26, display: 'grid', placeItems: 'center', color: attention.color, pointerEvents: 'none' }}
        >
          <Icon name="circleAlert" size={15} />
        </span>
      )}
      {actionsPosition && createPortal(
        <div ref={actionsMenuRef} className="session-actions-menu" role="menu" aria-label={`Actions for ${session.title || 'Untitled session'}`}
          style={{ left: Math.max(12, Math.min(actionsPosition.left, window.innerWidth - 220)), top: Math.max(12, Math.min(actionsPosition.top, window.innerHeight - 150)) }}>
          <button type="button" role="menuitem" disabled={opening} onClick={() => { closeActions(); onRename(); }}><Icon name="pencil" size={16} /> Rename</button>
          <button type="button" role="menuitem" onClick={() => { closeActions(); onPin(); }}><Icon name="pin" size={16} /> {pinned ? 'Unpin' : 'Pin'}</button>
          <button type="button" role="menuitem" disabled={opening} onClick={() => { closeActions(); onArchive(); }}><Icon name="archive" size={16} /> Archive</button>
        </div>,
        document.body,
      )}
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
  const scheduledWorkspace = bridge.bootstrap?.scheduledWorkspace;
  const projects = useMemo(
    () => (settings?.projects ?? []).filter((path) => path !== scheduledWorkspace),
    [scheduledWorkspace, settings?.projects],
  );
  const generalCatalog = scheduledWorkspace ? bridge.bootstrap?.projectCatalogs?.[scheduledWorkspace] : undefined;
  const pinnedSessions = settings?.pinnedSessions ?? [];
  const selectedProject = bridge.activeSession?.projectPath ?? settings?.activeProject ?? bridge.bootstrap?.workspace.path;
  const visibleSession = bridge.activeSession ?? bridge.bootstrap?.activeSession ?? settings?.activeSession;
  const [expandedProjects, setExpandedProjects] = useState<Set<string>>(
    () => new Set(selectedProject ? [selectedProject] : []),
  );
  const [showAllSessions, setShowAllSessions] = useState<Record<string, boolean>>({});
  const [pinnedExpanded, setPinnedExpanded] = useState(true);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const searchInputRef = useRef<HTMLInputElement>(null);
  const projectsHeadingRef = useRef<HTMLElement>(null);
  const normalizedSearch = searchQuery.trim().toLocaleLowerCase();
  const matchesSearch = (value: string) => !normalizedSearch || value.toLocaleLowerCase().includes(normalizedSearch);
  useEffect(() => { asideRef.current?.toggleAttribute('inert', sidebarCollapsed); }, [sidebarCollapsed]);
  const activeCatalogRows = visibleSession ? bridge.bootstrap?.projectCatalogs?.[visibleSession.projectPath]?.sessions : undefined;
  const activeRowIndex = activeCatalogRows?.findIndex((row) => row.uuid === visibleSession?.sessionId) ?? -1;
  const revealedSessionKey = !scheduled && visibleSession && activeRowIndex >= 0
    ? `${visibleSession.projectPath}\0${visibleSession.sessionId}` : null;
  const focusedSessionRef = useRef<string | null>(null);
  useEffect(() => {
    if (!revealedSessionKey || !visibleSession) return;
    const projectPath = visibleSession.projectPath;
    setExpandedProjects((current) => current.has(projectPath) ? current : new Set([...current, projectPath]));
    if (activeRowIndex >= 5) setShowAllSessions((current) => current[projectPath] ? current : { ...current, [projectPath]: true });
  }, [revealedSessionKey, activeRowIndex, visibleSession?.projectPath]);
  useEffect(() => {
    if (!revealedSessionKey) { focusedSessionRef.current = null; return; }
    if (focusedSessionRef.current === revealedSessionKey) return;
    const row = asideRef.current?.querySelector<HTMLButtonElement>('[data-session-id][aria-current="page"]');
    if (!row) return;
    focusedSessionRef.current = revealedSessionKey;
    row.focus({ preventScroll: true });
    row.scrollIntoView({ block: 'nearest' });
  }, [revealedSessionKey, expandedProjects, showAllSessions]);
  const [menuProject, setMenuProject] = useState<string | null>(null);
  const [projectsMenu, setProjectsMenu] = useState<'root' | 'organize' | 'sort' | null>(null);
  const [projectsMenuPosition, setProjectsMenuPosition] = useState<{ left: number; top: number } | null>(null);
  const [sidebarOrganization, setSidebarOrganization] = useState<SidebarOrganization>(() => settings?.sidebar?.organization ?? 'project');
  const [chatSort, setChatSort] = useState<ChatSort>(() => settings?.sidebar?.chatSort ?? 'updated');
  const [projectActionsVisible, setProjectActionsVisible] = useState(false);
  const projectsMenuRef = useRef<HTMLDivElement>(null);
  const projectsMenuTriggerRef = useRef<HTMLButtonElement>(null);
  const projectActionsHideTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [openingSessionKey, setOpeningSessionKey] = useState<string | null>(null);
  const [editingProject, setEditingProject] = useState<string | null>(null);
  const [archiveTarget, setArchiveTarget] = useState<{projectPath: string; sessionId: string; title: string} | null>(null);
  const [archiveJobs, setArchiveJobs] = useState<ScheduledCronJob[]>([]);
  const [archiveLoading, setArchiveLoading] = useState(false);
  const [archiving, setArchiving] = useState(false);
  const [archiveError, setArchiveError] = useState('');
  const [renameTarget, setRenameTarget] = useState<{ projectPath: string; sessionId: string; title: string } | null>(null);
  const [renamingSession, setRenamingSession] = useState(false);
  const [renameError, setRenameError] = useState('');
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
  const prepareRename = (projectPath: string, sessionId: string, title: string) => {
    setRenameTarget({ projectPath, sessionId, title });
    setRenameError('');
  };
  const confirmRename = async (title: string) => {
    if (!renameTarget || renamingSession) return;
    setRenamingSession(true); setRenameError('');
    try {
      await bridge.renameSession(renameTarget.projectPath, renameTarget.sessionId, title);
      setRenameTarget(null);
    } catch (cause) {
      setRenameError(cause instanceof Error ? cause.message : String(cause));
    } finally { setRenamingSession(false); }
  };
  const [sidebarWidth, setSidebarWidth] = useState(SIDEBAR_DEFAULT_WIDTH);
  const [resizingSidebar, setResizingSidebar] = useState(false);

  const revealProjectActions = useCallback(() => {
    if (projectActionsHideTimer.current) clearTimeout(projectActionsHideTimer.current);
    setProjectActionsVisible(true);
  }, []);
  const scheduleProjectActionsHide = useCallback(() => {
    if (projectActionsHideTimer.current) clearTimeout(projectActionsHideTimer.current);
    projectActionsHideTimer.current = setTimeout(() => {
      setProjectActionsVisible(false);
      projectActionsHideTimer.current = null;
    }, PROJECT_ACTIONS_HIDE_DELAY);
  }, []);
  const openProjectsMenu = useCallback((trigger: HTMLButtonElement) => {
    const rect = trigger.getBoundingClientRect();
    revealProjectActions();
    setProjectsMenuPosition({ left: rect.left, top: rect.bottom + 6 });
    setProjectsMenu((current) => current === 'root' ? null : 'root');
  }, [revealProjectActions]);
  const closeProjectsMenu = useCallback((restoreFocus = false) => {
    setProjectsMenu(null);
    if (restoreFocus) projectsMenuTriggerRef.current?.focus();
  }, []);

  useEffect(() => () => {
    if (projectActionsHideTimer.current) clearTimeout(projectActionsHideTimer.current);
  }, []);
  useEffect(() => {
    if (!settings?.sidebar) return;
    setSidebarOrganization(settings.sidebar.organization);
    setChatSort(settings.sidebar.chatSort);
  }, [settings?.sidebar]);
  useEffect(() => {
    if (!projectsMenu) return;
    const closeOnOutsidePointer = (event: PointerEvent) => {
      const target = event.target as Node;
      if (projectsMenuRef.current?.contains(target) || projectsMenuTriggerRef.current?.contains(target)) return;
      closeProjectsMenu();
    };
    const closeOnEscape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      closeProjectsMenu(true);
    };
    document.addEventListener('pointerdown', closeOnOutsidePointer, true);
    document.addEventListener('keydown', closeOnEscape);
    return () => {
      document.removeEventListener('pointerdown', closeOnOutsidePointer, true);
      document.removeEventListener('keydown', closeOnEscape);
    };
  }, [closeProjectsMenu, projectsMenu]);

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
    const catalogsToLoad = sidebarOrganization === 'list' || normalizedSearch ? projects : expandedProjects;
    for (const projectPath of catalogsToLoad) {
      if (!bridge.bootstrap?.projectCatalogs?.[projectPath]) void bridge.listProjectSessions(projectPath).catch(() => undefined);
    }
  }, [bridge.bootstrap?.projectCatalogs, bridge.listProjectSessions, expandedProjects, normalizedSearch, projects, sidebarOrganization]);

  useEffect(() => {
    if (scheduledWorkspace && !generalCatalog) void bridge.listProjectSessions(scheduledWorkspace).catch(() => undefined);
  }, [scheduledWorkspace, generalCatalog, bridge.listProjectSessions]);

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
  const manualSessionOrder = settings?.sidebar?.manualSessionOrder ?? {};
  const sortedSessions = useCallback((projectPath: string, sessions: readonly SessionRowDto[]) => {
    if (chatSort === 'manual') {
      const order = new Map((manualSessionOrder[projectPath] ?? []).map((sessionId, index) => [sessionId, index]));
      return [...sessions].sort((left, right) => {
        const newSessionOrder = compareNewSidebarSessions(left, right);
        if (newSessionOrder !== 0) return newSessionOrder;
        const leftIndex = order.get(left.uuid);
        const rightIndex = order.get(right.uuid);
        // New sessions precede the saved drag order, newest first.
        if (leftIndex === undefined && rightIndex === undefined) {
          return sidebarSessionTimestamp(right) - sidebarSessionTimestamp(left);
        }
        return (leftIndex ?? -1) - (rightIndex ?? -1);
      });
    }
    return [...sessions].sort((left, right) => {
      const newSessionOrder = compareNewSidebarSessions(left, right);
      if (newSessionOrder !== 0) return newSessionOrder;
      if (chatSort === 'priority') {
        const leftActive = visibleSession?.projectPath === projectPath && visibleSession.sessionId === left.uuid;
        const rightActive = visibleSession?.projectPath === projectPath && visibleSession.sessionId === right.uuid;
        if (leftActive !== rightActive) return leftActive ? -1 : 1;
      }
      return sidebarSessionTimestamp(right) - sidebarSessionTimestamp(left);
    });
  }, [chatSort, manualSessionOrder, visibleSession]);
  const allProjectSessions = useMemo(() => projects.flatMap((projectPath) => (
    sortedSessions(projectPath, bridge.bootstrap?.projectCatalogs?.[projectPath]?.sessions ?? [])
      .map((session) => ({ projectPath, session }))
  )).sort((left, right) => {
    const newSessionOrder = compareNewSidebarSessions(left.session, right.session);
    if (newSessionOrder !== 0) return newSessionOrder;
    if (chatSort === 'manual') return 0;
    if (chatSort === 'priority') {
      const leftActive = visibleSession?.projectPath === left.projectPath && visibleSession.sessionId === left.session.uuid;
      const rightActive = visibleSession?.projectPath === right.projectPath && visibleSession.sessionId === right.session.uuid;
      if (leftActive !== rightActive) return leftActive ? -1 : 1;
    }
    return sidebarSessionTimestamp(right.session) - sidebarSessionTimestamp(left.session);
  }), [bridge.bootstrap?.projectCatalogs, chatSort, projects, sortedSessions, visibleSession]);
  const searchedProjectSessions = normalizedSearch
    ? allProjectSessions.filter(({ projectPath, session }) => matchesSearch(basename(projectPath)) || matchesSearch(session.title || 'Untitled session'))
    : allProjectSessions;
  const visibleProjects = normalizedSearch
    ? projects.filter((path) => matchesSearch(basename(path)) ||
      bridge.bootstrap?.projectCatalogs?.[path]?.sessions.some((session) => matchesSearch(session.title || 'Untitled session')))
    : projects;
  const projectSessionIndices = useMemo(() => {
    const nextIndexByProject = new Map<string, number>();
    const indexBySession = new Map<string, number>();
    for (const { projectPath, session } of allProjectSessions) {
      const key = `${projectPath}\0${session.uuid}`;
      const index = nextIndexByProject.get(projectPath) ?? 0;
      indexBySession.set(key, index);
      nextIndexByProject.set(projectPath, index + 1);
    }
    return indexBySession;
  }, [allProjectSessions]);
  const persistSidebarPreferences = useCallback((next: Partial<SidebarPreferences>) => {
    void bridge.updateSidebarPreferences({
      organization: next.organization ?? sidebarOrganization,
      chatSort: next.chatSort ?? chatSort,
      manualSessionOrder: next.manualSessionOrder ?? manualSessionOrder,
    });
  }, [bridge, chatSort, manualSessionOrder, sidebarOrganization]);
  const chooseSidebarOrganization = useCallback((organization: SidebarOrganization) => {
    setSidebarOrganization(organization);
    persistSidebarPreferences({ organization });
    closeProjectsMenu(true);
  }, [closeProjectsMenu, persistSidebarPreferences]);
  const chooseChatSort = useCallback((sort: ChatSort) => {
    setChatSort(sort);
    persistSidebarPreferences({ chatSort: sort });
    closeProjectsMenu(true);
  }, [closeProjectsMenu, persistSidebarPreferences]);
  const moveSessionToIndex = useCallback((projectPath: string, sessionId: string, targetIndex: number) => {
    const sessions = sortedSessions(projectPath, bridge.bootstrap?.projectCatalogs?.[projectPath]?.sessions ?? []);
    const order = sessions.map((session) => session.uuid).filter((id) => id !== sessionId);
    const sourceIndex = sessions.findIndex((session) => session.uuid === sessionId);
    if (sourceIndex < 0) return;
    const destinationIndex = Math.max(0, Math.min(targetIndex, order.length));
    if (sourceIndex === destinationIndex) return;
    order.splice(destinationIndex, 0, sessionId);
    setChatSort('manual');
    persistSidebarPreferences({ chatSort: 'manual', manualSessionOrder: { ...manualSessionOrder, [projectPath]: order } });
    void bridge.touchSession(projectPath, sessionId);
  }, [bridge, manualSessionOrder, persistSidebarPreferences, sortedSessions]);
  const handleSessionDragEnd = useCallback((event: DragEndEvent) => {
    const { source, target } = event.operation;
    if (event.canceled || !target || !isSortable(source)) return;
    const data = source.data as { projectPath?: unknown; sessionId?: unknown };
    if (typeof data.projectPath !== 'string' || typeof data.sessionId !== 'string') return;
    moveSessionToIndex(data.projectPath, data.sessionId, source.index);
  }, [moveSessionToIndex]);
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
    <div className="desktop-navigation" data-collapsed={sidebarCollapsed || undefined} style={{ '--rail-material': t.appBg, '--sidebar-material': t.sidebarBg, '--desktop-accent': t.accent, '--text': t.text, '--text3': t.text3, '--nav-panel-border': t.border } as CSSProperties}>
      <nav className="desktop-nav-rail no-drag" aria-label="Main navigation">
        <button type="button" className="desktop-rail-button desktop-rail-toggle" aria-label={sidebarCollapsed ? 'Show sidebar' : 'Hide sidebar'} aria-controls="desktop-session-sidebar" aria-expanded={!sidebarCollapsed} title={sidebarCollapsed ? 'Show sidebar' : 'Hide sidebar'} onClick={() => setSidebarCollapsed((value) => !value)}><Icon name="sidebar" size={19} stroke={1.65} /></button>
        <button type="button" className="desktop-rail-button" data-active={!scheduled || undefined} aria-label="Home" title="Home" onClick={onOpenChat}><Icon name="home" size={21} stroke={1.8} /></button>
        <button type="button" className="desktop-rail-button" aria-label="Projects" title="Projects" onClick={() => { setSidebarCollapsed(false); window.requestAnimationFrame(() => projectsHeadingRef.current?.scrollIntoView({ block: 'start' })); }}><Icon name="copy" size={20} stroke={1.7} /></button>
        <button type="button" className="desktop-rail-button" data-active={scheduled || undefined} aria-label="Scheduled tasks" title="Scheduled tasks" onClick={onOpenScheduled}><Icon name="clock" size={21} stroke={1.7} /></button>
        <button type="button" className="desktop-rail-button" aria-label="Activity" title="Activity" onClick={() => { onOpenChat?.(); bridge.setRuntimeCenterOverviewOpen(true); }}><Icon name="goal" size={21} stroke={1.7} /></button>
        <button type="button" className="desktop-rail-button" aria-label="Settings" title="Settings" onClick={onOpenSettings}><Icon name="more" size={21} stroke={1.8} /></button>
        <span className="desktop-rail-divider" aria-hidden="true" />
        <button type="button" className="desktop-rail-button" aria-label="Review changes" title="Review changes" onClick={() => { onOpenChat?.(); bridge.openRuntimeItem({ kind: 'section', id: 'review' }); }}><Icon name="branch" size={21} stroke={1.7} /></button>
      </nav>
    <aside
      id="desktop-session-sidebar"
      className="desktop-sidebar"
      ref={asideRef}
      aria-hidden={sidebarCollapsed || undefined}
      data-resizing={resizingSidebar || undefined}
      style={{ position: 'relative', width: sidebarWidth, flexShrink: 0, display: 'flex', flexDirection: 'column', border: `0.5px solid ${t.border}`, borderBottom: 0, marginTop: 38, '--sidebar-material': t.sidebarBg, '--desktop-accent': t.accent } as CSSProperties}
    >
      <div className="desktop-sidebar-brand" style={{ minHeight: 48, padding: '5px 16px 4px', display: 'flex', alignItems: 'center', gap: 6 }}>
        <strong style={{ color: t.text, fontSize: 17, fontWeight: 650, letterSpacing: '-.035em', flex: 1 }}>LingXi</strong>
        <button type="button" className="sidebar-header-icon" aria-label="Activity" title="Activity" onClick={() => { onOpenChat?.(); bridge.setRuntimeCenterOverviewOpen(true); }} style={{ color: t.text3 }}><Icon name="bell" size={18} stroke={1.65} /></button>
        <button type="button" className="sidebar-header-icon" aria-label="Search chats" title="Search chats" aria-expanded={searchOpen} onClick={() => { setSidebarCollapsed(false); setSearchOpen(true); window.requestAnimationFrame(() => searchInputRef.current?.focus()); }} style={{ color: t.text3 }}><Icon name="search" size={18} stroke={1.65} /></button>
      </div>

      {searchOpen && <div className="sidebar-search"><Icon name="search" size={15} stroke={1.7} /><input ref={searchInputRef} type="search" aria-label="Search chats and projects" placeholder="Search chats and projects" value={searchQuery} onChange={(event) => setSearchQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Escape') { setSearchQuery(''); setSearchOpen(false); } }} /><button type="button" aria-label="Close search" onClick={() => { setSearchQuery(''); setSearchOpen(false); }}><Icon name="x" size={15} /></button></div>}

      <div style={{ padding: '0 8px 8px' }}>
        <button
          className="sidebar-primary-action"
          type="button"
          disabled={bridge.sessionLoading || editingProject !== null}
          onClick={() => { onOpenChat?.(); selectedProject ? editProject(selectedProject) : invoke(bridge.addProject); }}
          style={{
            width: '100%', minHeight: 36, display: 'flex', alignItems: 'center', gap: 9,
            padding: '6px 5px', borderRadius: 10, border: 0, background: 'transparent',
            color: t.text, cursor: bridge.sessionLoading || editingProject !== null ? 'wait' : 'pointer', opacity: bridge.sessionLoading || editingProject !== null ? .5 : 1,
            textAlign: 'left', fontSize: 14, fontWeight: 500,
          }}
        >
          {editingProject
            ? <span className="beta-spinner" role="status" aria-label="Opening project draft" />
            : <Icon name="compose" size={18} color={t.text2} stroke={1.8} />}
          <span>{editingProject ? 'Opening draft…' : 'New chat'}</span>
        </button>
        <button
          type="button"
          className="sidebar-primary-action"
          aria-current={scheduled ? 'page' : undefined}
          onClick={onOpenScheduled}
          style={{ width: '100%', minHeight: 36, marginTop: 0, display: 'flex', alignItems: 'center', gap: 9,
            padding: '6px 5px', borderRadius: 10, border: 0, background: scheduled ? t.surfaceActive : 'transparent',
            color: t.text, cursor: 'pointer', textAlign: 'left', fontSize: 14, fontWeight: 500 }}
        >
          <Icon name="clock" size={18} stroke={1.8} />
          <span>Scheduled</span>
        </button>
      </div>

      {/* Electron drag regions discard pointer events. Keep every session control in
          an explicit no-drag region even if the title-bar geometry changes. */}
      <DragDropProvider
        sensors={(defaults) => defaults.map((sensor) => sensor === PointerSensor ? SESSION_POINTER_SENSOR : sensor)}
        onDragEnd={handleSessionDragEnd}
      >
      <nav className="no-drag desktop-sidebar-list" aria-label="Projects and sessions" style={{ flex: 1, minHeight: 0, overflowY: 'auto', padding: '0 10px 12px' }}>
        {pinnedSessions.length > 0 ? (
          <section aria-labelledby="pinned-sessions-heading" style={{ marginBottom: 24 }}>
            <h2 id="pinned-sessions-heading" className="sidebar-section-heading"><button type="button" aria-expanded={pinnedExpanded} onClick={() => setPinnedExpanded((value) => !value)} style={{ color: t.text4 }}>Pinned <Icon name="chevron" size={13} stroke={1.7} style={{ transform: pinnedExpanded ? 'none' : 'rotate(-90deg)' }} /></button></h2>
            {pinnedExpanded && pinnedSessions.filter((pinned) => {
              const current = bridge.bootstrap?.projectCatalogs?.[pinned.projectPath]?.sessions.find((session) => session.uuid === pinned.sessionId);
              return matchesSearch(current?.title || pinned.title || 'Untitled session') || matchesSearch(basename(pinned.projectPath));
            }).map((pinned) => {
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
              return (
                <SessionRow key={`${pinned.projectPath}-${pinned.sessionId}`} projectPath={pinned.projectPath}
                  session={current ?? { uuid: pinned.sessionId, title, modified_rfc3339: '', message_count: 0 }}
                  active={active} pinned pinnedSection opening={opening} status={bridge.sessionRuntimeStatus(pinned.sessionId)}
                  metadata={`${basename(pinned.projectPath)} · ${metadata}`}
                  onClick={() => openSidebarSession(pinned.projectPath, pinned.sessionId)}
                  onRename={() => prepareRename(pinned.projectPath, pinned.sessionId, title)}
                  onPin={() => invoke(() => bridge.setSessionPinned(pinInput(pinned.projectPath, pinned.sessionId, title), false))}
                  onArchive={() => void prepareArchive(pinned.projectPath, pinned.sessionId, title)} />
              );
            })}
          </section>
        ) : null}

        {scheduledWorkspace && generalCatalog && generalCatalog.sessions.length > 0 && <section aria-labelledby="general-chats-heading">
          <h2 id="general-chats-heading" style={{ padding: '8px', color: t.text4, fontSize: 11, fontWeight: 600, letterSpacing: '.04em' }}>Chats</h2>
          {(normalizedSearch ? generalCatalog.sessions.filter((session) => matchesSearch(session.title || 'Untitled session')) : showAllSessions[scheduledWorkspace] ? generalCatalog.sessions : generalCatalog.sessions.slice(0, 5)).map((session) => {
            const pinned = pinnedKeys.has(`${scheduledWorkspace}\0${session.uuid}`);
            return <SessionRow key={session.uuid} projectPath={scheduledWorkspace} session={session}
              active={!scheduled && visibleSession?.projectPath === scheduledWorkspace && visibleSession?.sessionId === session.uuid}
              pinned={pinned} opening={openingSessionKey === `${scheduledWorkspace}\0${session.uuid}`}
              status={bridge.sessionRuntimeStatus(session.uuid)}
              onClick={() => openSidebarSession(scheduledWorkspace, session.uuid)}
              onArchive={() => void prepareArchive(scheduledWorkspace, session.uuid, session.title || 'Untitled chat')}
              onRename={() => prepareRename(scheduledWorkspace, session.uuid, session.title || 'Untitled chat')}
              onPin={() => invoke(() => bridge.setSessionPinned(pinInput(scheduledWorkspace, session.uuid, session.title || 'Untitled session'), !pinned))} />;
          })}
          {!normalizedSearch && generalCatalog.sessions.length > 5 && <button type="button" onClick={() => setShowAllSessions((current) => ({ ...current, [scheduledWorkspace]: !current[scheduledWorkspace] }))} style={{ minHeight: 32, marginLeft: 30, padding: '5px 8px', border: 0, borderRadius: 7, background: 'transparent', color: t.text4, cursor: 'pointer', fontSize: 11.5 }}>
            {showAllSessions[scheduledWorkspace] ? 'Show less' : `Show more (${generalCatalog.sessions.length - 5})`}
          </button>}
        </section>}

        <section
          ref={projectsHeadingRef}
          className="projects-sidebar-section"
          aria-labelledby="projects-heading"
          tabIndex={0}
          data-project-actions-visible={projectActionsVisible || projectsMenu !== null || undefined}
          onPointerEnter={revealProjectActions}
          onPointerLeave={scheduleProjectActionsHide}
          onFocusCapture={revealProjectActions}
          onBlurCapture={(event) => {
            const section = event.currentTarget;
            window.setTimeout(() => {
              if (!section.contains(document.activeElement)) scheduleProjectActionsHide();
            }, 0);
          }}
        >
          <div className="projects-sidebar-heading" style={{ minHeight: 35, padding: '3px 4px 5px 8px', display: 'flex', alignItems: 'center' }}>
            <h2 id="projects-heading" style={{ flex: 1, color: t.text4, fontSize: 12.5, fontWeight: 500, letterSpacing: '.01em' }}>Projects</h2>
            <div className="projects-sidebar-actions">
              <button
                ref={projectsMenuTriggerRef}
                type="button"
                className="sidebar-header-action projects-sidebar-action"
                tabIndex={projectActionsVisible || projectsMenu !== null ? 0 : -1}
                aria-label="Project sidebar options"
                aria-haspopup="menu"
                aria-expanded={projectsMenu !== null}
                title="Project sidebar options"
                onClick={(event) => openProjectsMenu(event.currentTarget)}
                style={{ width: 28, height: 28, display: 'grid', placeItems: 'center', border: 0, borderRadius: 7, background: 'transparent', color: t.text3, cursor: 'pointer' }}
              >
                <Icon name="more" size={16} stroke={2} />
              </button>
            <button
              type="button"
              className="sidebar-header-action projects-sidebar-action"
              tabIndex={projectActionsVisible || projectsMenu !== null ? 0 : -1}
              aria-label="Add project"
              title="Add project"
              onClick={() => invoke(bridge.addProject)}
              style={{ width: 28, height: 28, display: 'grid', placeItems: 'center', border: 0, borderRadius: 7, background: 'transparent', color: t.text3, cursor: 'pointer' }}
            >
              <Icon name="plus" size={17} stroke={1.8} />
            </button>
            </div>
          </div>

          {projectsMenu && projectsMenuPosition && createPortal(
            <div
              ref={projectsMenuRef}
              className="projects-sidebar-menu-layer"
              style={{ left: projectsMenuPosition.left, top: projectsMenuPosition.top }}
              onPointerEnter={revealProjectActions}
            >
              <div className="projects-sidebar-menu" role="menu" aria-label="Project sidebar options">
                <button type="button" role="menuitem" className="projects-sidebar-menu-item" data-active={projectsMenu === 'organize' || undefined} onPointerEnter={() => setProjectsMenu('organize')} onClick={() => setProjectsMenu('organize')}>
                  <span>Organize sidebar</span><Icon name="chevronR" size={16} stroke={2.1} />
                </button>
                <button type="button" role="menuitem" className="projects-sidebar-menu-item" data-active={projectsMenu === 'sort' || undefined} onPointerEnter={() => setProjectsMenu('sort')} onClick={() => setProjectsMenu('sort')}>
                  <span>Sort chats by</span><Icon name="chevronR" size={16} stroke={2.1} />
                </button>
              </div>
              {projectsMenu === 'organize' ? (
                <div className="projects-sidebar-menu projects-sidebar-submenu" role="menu" aria-label="Organize sidebar">
                  <button type="button" role="menuitemradio" aria-checked={sidebarOrganization === 'project'} className="projects-sidebar-menu-item" onClick={() => chooseSidebarOrganization('project')}>
                    <span className="projects-sidebar-menu-check">{sidebarOrganization === 'project' ? <Icon name="check" size={17} stroke={2.5} /> : null}</span><span>By project</span>
                  </button>
                  <button type="button" role="menuitemradio" aria-checked={sidebarOrganization === 'list'} className="projects-sidebar-menu-item" onClick={() => chooseSidebarOrganization('list')}>
                    <span className="projects-sidebar-menu-check">{sidebarOrganization === 'list' ? <Icon name="check" size={17} stroke={2.5} /> : null}</span><span>In one list</span>
                  </button>
                </div>
              ) : null}
              {projectsMenu === 'sort' ? (
                <div className="projects-sidebar-menu projects-sidebar-submenu" role="menu" aria-label="Sort chats by">
                  {([['priority', 'Priority'], ['updated', 'Last updated'], ['manual', 'Manual order']] as const).map(([value, label]) => (
                    <button key={value} type="button" role="menuitemradio" aria-checked={chatSort === value} className="projects-sidebar-menu-item" onClick={() => chooseChatSort(value)}>
                      <span className="projects-sidebar-menu-check">{chatSort === value ? <Icon name="check" size={17} stroke={2.5} /> : null}</span><span>{label}</span>
                    </button>
                  ))}
                </div>
              ) : null}
            </div>,
            document.body,
          )}

          {projects.length === 0 ? (
            <div style={{ padding: '12px 9px', color: t.text4, fontSize: 11.5, lineHeight: 1.5 }}>
              Add a project folder to start a session.
            </div>
          ) : sidebarOrganization === 'list' ? (
            <div className="projects-one-list" aria-label="All project chats">
              {searchedProjectSessions.length === 0 ? (
                <div style={{ padding: '12px 9px', color: t.text4, fontSize: 11.5, lineHeight: 1.5 }}>{normalizedSearch ? 'No matching chats.' : 'Loading project chats…'}</div>
              ) : searchedProjectSessions.map(({ projectPath, session }) => {
                const pinned = pinnedKeys.has(`${projectPath}\0${session.uuid}`);
                const opening = openingSessionKey === `${projectPath}\0${session.uuid}`;
              return <SessionRow key={`${projectPath}\0${session.uuid}`} projectPath={projectPath} session={session}
                active={!scheduled && visibleSession?.projectPath === projectPath && visibleSession.sessionId === session.uuid}
                pinned={pinned} opening={opening} status={bridge.sessionRuntimeStatus(session.uuid)}
                  onClick={() => openSidebarSession(projectPath, session.uuid)}
                onArchive={() => void prepareArchive(projectPath, session.uuid, session.title || 'Untitled chat')}
                onRename={() => prepareRename(projectPath, session.uuid, session.title || 'Untitled chat')}
                onPin={() => invoke(() => bridge.setSessionPinned(pinInput(projectPath, session.uuid, session.title || 'Untitled session'), !pinned))}
                reorderable={!opening}
                sortableIndex={projectSessionIndices.get(`${projectPath}\0${session.uuid}`) ?? 0} />;
              })}
            </div>
          ) : visibleProjects.length === 0 ? (
            <div style={{ padding: '12px 9px', color: t.text4, fontSize: 11.5, lineHeight: 1.5 }}>No matching projects or chats.</div>
          ) : visibleProjects.map((projectPath) => {
            const active = projectPath === selectedProject;
            const open = Boolean(normalizedSearch) || expandedProjects.has(projectPath);
            const catalog = bridge.bootstrap?.projectCatalogs?.[projectPath];
            const allSessions = sortedSessions(projectPath, catalog?.sessions ?? []);
            const visibleSessions = normalizedSearch
              ? allSessions.filter((session) => matchesSearch(basename(projectPath)) || matchesSearch(session.title || 'Untitled session'))
              : showAllSessions[projectPath] ? allSessions : allSessions.slice(0, 5);
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
                    style={{ position: 'absolute', right: 32, top: 5, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: editingProject !== null ? 'wait' : 'pointer' }}
                  >
                    <Icon name="pencil" size={13} stroke={1.8} />
                  </button>
                  <button
                    type="button"
                    className="sidebar-row-action"
                    data-visible={menuProject === projectPath ? 'true' : undefined}
                    disabled={projectHasActiveWork}
                    aria-label={`Project actions for ${basename(projectPath)}`}
                    aria-expanded={menuProject === projectPath}
                    onClick={() => setMenuProject((current) => current === projectPath ? null : projectPath)}
                    style={{ position: 'absolute', right: 4, top: 5, width: 26, height: 26, display: 'grid', placeItems: 'center', border: 0, borderRadius: 6, background: 'transparent', color: t.text3, cursor: projectHasActiveWork ? 'not-allowed' : 'pointer' }}
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
                    {catalog?.error && allSessions.length === 0 ? (
                      <div style={{ padding: '8px 10px 8px 30px', color: t.danger, fontSize: 10.5 }}>{catalog.error}</div>
                    ) : !catalog ? (
                      <div style={{ padding: '8px 10px 8px 30px', color: t.text4, fontSize: 10.5 }}>Loading sessions…</div>
                    ) : allSessions.length === 0 ? (
                      <div style={{ padding: '8px 10px 8px 30px', color: t.text4, fontSize: 10.5 }}>No saved sessions yet.</div>
                    ) : visibleSessions.map((session, sortableIndex) => {
                      const pinned = pinnedKeys.has(`${projectPath}\0${session.uuid}`);
                      const opening = openingSessionKey === `${projectPath}\0${session.uuid}`;
                      return (
                        <SessionRow
                          key={session.uuid}
                          projectPath={projectPath}
                          session={session}
                          active={!scheduled && visibleSession?.projectPath === projectPath && visibleSession.sessionId === session.uuid}
                          pinned={pinned}
                          opening={opening}
                          status={bridge.sessionRuntimeStatus(session.uuid)}
                          onClick={() => openSidebarSession(projectPath, session.uuid)}
                          onArchive={() => void prepareArchive(projectPath, session.uuid, session.title || 'Untitled chat')}
                          onRename={() => prepareRename(projectPath, session.uuid, session.title || 'Untitled chat')}
                          onPin={() => invoke(() => bridge.setSessionPinned(pinInput(projectPath, session.uuid, session.title || 'Untitled session'), !pinned))}
                          reorderable={!opening}
                          sortableIndex={sortableIndex}
                        />
                      );
                    })}
                    {!normalizedSearch && allSessions.length > 5 ? (
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
      <DragOverlay
        className="sidebar-session-drag-overlay"
        dropAnimation={null}
        style={{ pointerEvents: 'none' }}
      >
        {(source) => {
          const data = source.data as { sessionTitle?: string; sessionMetadata?: string };
          const previewWidth = source.element?.getBoundingClientRect().width ?? sidebarWidth - 16;
          return (
            <div
              className="sidebar-session-drag-preview"
              aria-hidden="true"
              style={{
                width: previewWidth, minHeight: 43, display: 'grid', gap: 1,
                padding: '6px 64px 6px 30px', border: `1px solid ${t.border}`, borderRadius: 8,
                background: t.surfaceActive, color: t.text, boxShadow: '0 8px 22px rgba(0,0,0,.24)',
                fontSize: 13, fontWeight: 600, boxSizing: 'border-box',
              }}
            >
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{data.sessionTitle || 'Untitled session'}</span>
              <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap', color: t.text4, fontSize: 10.5, fontWeight: 400 }}>{data.sessionMetadata}</span>
            </div>
          );
        }}
      </DragOverlay>
      </DragDropProvider>

      <div className="desktop-sidebar-footer" style={{ padding: 10, borderTop: `0.5px solid ${t.border}`, display: 'flex', flexDirection: 'column', gap: 5 }}>
        <button
          type="button"
          onClick={onOpenSettings}
          style={{ minHeight: 40, display: 'flex', alignItems: 'center', gap: 8, padding: '7px 5px', border: 0, borderRadius: 8, background: 'transparent', color: t.text2, cursor: 'pointer', fontSize: 12.5 }}
        >
          <Icon name="cog" size={15} /> {settingsLabel()}
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
    </div>
    {archiveTarget && <ArchiveChatDialog title={archiveTarget.title} jobs={archiveJobs} loading={archiveLoading} busy={archiving} error={archiveError} onClose={closeArchive} onConfirm={() => void confirmArchive()} onRetry={() => void prepareArchive(archiveTarget.projectPath, archiveTarget.sessionId, archiveTarget.title)} />}
    {renameTarget && <RenameSessionDialog title={renameTarget.title} busy={renamingSession} error={renameError} onClose={() => { if (!renamingSession) setRenameTarget(null); }} onConfirm={(title) => void confirmRename(title)} />}
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

/** Context utilities live with the pinned summary instead of the topbar. */
export function SummaryContextActions({ bridge }: { bridge: UseBridge }) {
  const t = useT();
  const [summaryOpen, setSummaryOpen] = useState(false);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  const summaries = bridge.conversation.summaries ?? [];
  const compacting = bridge.conversation.activeCompactionId != null;
  const close = () => { setSummaryOpen(false); triggerRef.current?.focus(); };

  useEffect(() => {
    setSummaryOpen(false);
    setSelectedId(null);
  }, [bridge.conversation.sessionKey]);

  useEffect(() => {
    if (!summaryOpen) return;
    panelRef.current?.querySelector<HTMLButtonElement>('.context-summary-close')?.focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !panelRef.current?.contains(event.target) && !triggerRef.current?.contains(event.target)) setSummaryOpen(false);
    };
    document.addEventListener('pointerdown', outside, true);
    return () => document.removeEventListener('pointerdown', outside, true);
  }, [summaryOpen]);

  return <section className="runtime-summary-section runtime-context-actions" aria-label="Context"
    style={{ '--context-action-hover': t.surfaceHover, '--context-action-ring': t.borderStrong } as CSSProperties}>
    <h2>Context</h2>
    <button ref={triggerRef} type="button" aria-label="Open context summaries" aria-expanded={summaryOpen} aria-controls="context-summary-panel"
      onClick={() => { setSelectedId(summaries.at(-1)?.id ?? null); setSummaryOpen(true); }}>
      <Icon name="summary" size={16} /><span>Context summaries</span>
      <span className="runtime-context-count">{summaries.length}</span>
    </button>
    <button type="button" aria-label="Compact conversation" disabled={!bridge.activeSession || bridge.sessionLoading || compacting}
      onClick={() => { invoke(() => bridge.forceCompact()); }}>
      <Icon name="summary-list" size={16} /><span>{compacting ? 'Compacting…' : 'Compact'}</span>
    </button>
    {summaryOpen && createPortal(<div ref={panelRef} className="runtime-context-detail" data-runtime-summary-owned="true"
      onKeyDown={(event) => { if (event.key === 'Escape') { event.preventDefault(); event.stopPropagation(); close(); } }}>
      <ContextSummaryPanel summaries={summaries} selectedId={selectedId} onSelect={setSelectedId} onClose={close} />
    </div>, document.body)}
  </section>;
}

export function BetaTopBar({ bridge, runtimeCenterOpen, onToggleRuntimeCenter, terminalOpen = false, terminalAvailable = false, onToggleTerminal }: {
  terminalOpen?: boolean;
  terminalAvailable?: boolean;
  onToggleTerminal?(): void;
  bridge: UseBridge;
  runtimeCenterOpen: boolean;
  onToggleRuntimeCenter(): void;
}) {
  const t = useT();
  const inspectorOpen = bridge.runtimeCenter.inspectorOpen;
  // Cost events include all completed calls; status seeds a resumed session
  // before its next turn. usage_update only describes the latest API response.
  // A status refresh after turn completion can arrive just after cost_update,
  // so keep the greatest cumulative total visible while the two converge.
  const costTokens = bridge.cost
    ? bridge.cost.input_tokens + bridge.cost.output_tokens : null;
  const statusTokens = bridge.desktop?.status
    ? bridge.desktop.status.input_tokens + bridge.desktop.status.output_tokens : null;
  const sessionTokens = !bridge.sessionLoading && (costTokens !== null || statusTokens !== null)
    ? Math.max(costTokens ?? 0, statusTokens ?? 0) : null;

  const topbarActionTokens = {
    '--topbar-action-focus': t.surfaceHover,
    '--topbar-action-active': t.surfaceHover,
    '--topbar-action-ring': t.borderStrong,
    '--topbar-action-color': t.text3,
    '--topbar-action-hover-color': t.text,
    '--topbar-action-active-color': t.text,
  } as CSSProperties;
  return (
    <header className="drag-region desktop-topbar" style={{ height: 56, position: 'relative', flexShrink: 0, display: 'flex', alignItems: 'center', gap: 4, padding: '0 12px 0 18px', borderBottom: `0.5px solid ${t.border}`, '--toolbar-material': t.windowBg } as CSSProperties}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div style={{ color: t.text, fontSize: 13, fontWeight: 600, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{basename(bridge.activeSession?.projectPath ?? bridge.bootstrap?.workspace.path)}</div>
        <div className="mono" style={{ color: t.text4, fontSize: 10, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{bridge.activeSession?.projectPath ?? bridge.bootstrap?.workspace.path ?? 'Add a project to begin'}</div>
      </div>
      {sessionTokens !== null && (
        <span className="mono desktop-topbar-usage" style={{ color: t.text4, fontSize: 9.5 }} title="Session total: input + output tokens">
          {sessionTokens.toLocaleString()} tok
        </span>
      )}
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
      ><Icon name="topbar-summary" size={20} stroke={1.75} /></button>
      <button type="button" className="no-drag desktop-topbar-action" aria-label="Toggle terminal" title="Toggle terminal (Ctrl+`)" aria-controls="desktop-terminal" aria-expanded={terminalOpen} aria-pressed={terminalOpen} disabled={!terminalAvailable} data-active={terminalOpen ? 'true' : undefined} onClick={onToggleTerminal} style={topbarActionTokens}><Icon name="topbar-terminal" size={20} stroke={1.75} /></button>
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
          // The inspector is still sliding in; focusing must not scroll its
          // clipped container and jump the panel straight to its resting position.
          if (!inspectorOpen) window.requestAnimationFrame(() => {
            (document.querySelector<HTMLElement>('[data-runtime-inspector-active="true"]')
              ?? document.querySelector<HTMLElement>('.runtime-inspector-landing button')
              ?? document.querySelector<HTMLElement>('.runtime-panel-hide'))?.focus({ preventScroll: true });
          });
        }}
        style={topbarActionTokens}
      ><Icon name={inspectorOpen ? 'topbar-inspector-open' : 'topbar-inspector-closed'} size={20} stroke={1.75} /></button>
    </header>
  );
}

function nativeAudioApi(): NativeAudioApi | undefined {
  return typeof window === 'undefined' ? undefined : window.lingxi?.audio;
}

const DICTATION_WAVEFORM_SAMPLES = 160;

export function DictationRecorderBar({ audio, onCancel, onFinish, turnAction }: {
  audio: NativeAudioApi | undefined;
  onCancel(): void;
  onFinish(): void;
  turnAction?: ReactNode;
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
      if (event.type !== 'input_level' || event.owner.kind !== 'ui') return;
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
  }, [audio, drawWaveform]);

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
      {turnAction}
    </div>
  );
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
  view: 'all' | 'files';
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
  if (node instanceof HTMLElement && node.dataset.contextMention) {
    const mention = parseContextMentionHref(node.dataset.contextMention);
    return mention ? contextMentionMarkdown(mention) : node.textContent ?? '';
  }
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
  const token = document.createElement('a');
  token.href = contextMentionHref({ kind: path.endsWith('/') ? 'folder' : 'file', target: path, name: basename(path.replace(/\/$/, '')) });
  token.className = 'beta-file-mention';
  token.dataset.fileMention = path;
  token.contentEditable = 'false';
  token.title = path;
  token.setAttribute('aria-label', `${path.endsWith('/') ? 'Folder' : 'File'} mention: ${path}`);
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
  label.textContent = basename(path.replace(/\/$/, ''));
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
  const [promptComposing, setPromptComposing] = useState(false);
  const [modelOpen, setModelOpen] = useState(false);
  const [modelSubmenu, setModelSubmenu] = useState<ModelPickerSubmenu>(null);
  const [modelQuery, setModelQuery] = useState('');
  const [submenuBounds, setSubmenuBounds] = useState<ModelPickerSubmenuBounds | null>(null);
  const [permissionOpen, setPermissionOpen] = useState(false);
  const [slashQuery, setSlashQuery] = useState<string | null>(null);
  const [filePicker, setFilePicker] = useState<FilePickerState | null>(null);
  const [fileResults, setFileResults] = useState<string[]>([]);
  const [fileResultsTruncated, setFileResultsTruncated] = useState(false);
  const [fileSearchStatus, setFileSearchStatus] = useState<'idle' | 'loading' | 'ready' | 'error'>('idle');
  const [fileResultIndex, setFileResultIndex] = useState(0);
  const [previewPath, setPreviewPath] = useState<string | null>(null);
  const [selectedFiles, setSelectedFiles] = useState<string[]>([]);
  const [imageAttachments, setImageAttachments] = useState<ImageAttachment[]>([]);
  const [imageNotice, setImageNotice] = useState<string | null>(null);
  const [imageDragActive, setImageDragActive] = useState(false);
  const [voiceState, setVoiceState] = useState<'idle' | 'listening' | 'unsupported' | 'denied'>('idle');
  const [flowMode, setFlowMode] = useState(false);
  const [flowState, setFlowState] = useState<VoiceFlowState>(DEFAULT_VOICE_FLOW_STATE);
  const [slashResultIndex, setSlashResultIndex] = useState(0);
  /** The widget the next prompt follows up on; cleared on send or session switch. */
  const [visualizationChip, setVisualizationChip] = useState<VisualizationContextChip | null>(null);
  const input = useRef<HTMLDivElement>(null);
  const commandMirror = useRef<HTMLDivElement>(null);
  const fileControl = useRef<HTMLDivElement>(null);
  const fileButton = useRef<HTMLButtonElement>(null);
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
  const mentionDismissed = useRef(false);
  const activeSlashRange = useRef<Range | null>(null);
  const activeSlashQuery = useRef<string | null>(null);
  const slashDismissed = useRef(false);
  const imageAttachmentsRef = useRef<ImageAttachment[]>([]);
  const draftsBySession = useRef(new Map<string, ComposerDraft>());
  const draftSessionId = useRef<string | null>(null);
  const imageDraftGeneration = useRef(0);
  const composerDraftRevision = useRef(0);
  imageAttachmentsRef.current = imageAttachments;
  const activeSessionId = bridge.activeSession?.sessionId ?? null;
  const audio = nativeAudioApi();
  const voicePrefs = bridge.bootstrap?.settings.voice ?? audioConfigurationDefaults();
  const voicePrefsRef = useRef({ configuration: voicePrefs, revision: bridge.bootstrap?.settings.voiceRevision ?? 0 });
  voicePrefsRef.current = { configuration: voicePrefs, revision: bridge.bootstrap?.settings.voiceRevision ?? 0 };
  const modelPickerVisibility = bridge.bootstrap?.settings.modelPickerVisibility;
  const dictationOwner = useRef<NativeAudioOwner | null>(null);
  const autoplayOwner = useRef<NativeAudioOwner | null>(null);
  const autoplaySubscription = useRef<(() => void) | null>(null);
  const activeAudioSessionId = useRef(activeSessionId);
  activeAudioSessionId.current = activeSessionId;
  const flowModeRef = useRef(flowMode);
  flowModeRef.current = flowMode;
  const standardListeningRef = useRef(false);
  const dictationGeneration = useRef(0);

  const slashCommands = useMemo(
    () => filterSlashCommands(bridge.desktop.slashCommands, slashQuery ?? ''),
    [bridge.desktop.slashCommands, slashQuery],
  );
  const contextCatalog = useMemo(() => mentionCatalog({
    skills: bridge.skillsEvent?.skills,
    skillCatalog: bridge.skillCatalogEvent?.catalog_json,
    pluginCatalog: bridge.pluginCatalogEvent?.catalog_json,
    effectiveSettings: bridge.settingsSnapshotEvent?.effective_json,
  }), [bridge.skillsEvent, bridge.skillCatalogEvent, bridge.pluginCatalogEvent, bridge.settingsSnapshotEvent]);
  const mentionEntries = useMemo(() => mentionMenuEntries({
    query: filePicker?.query ?? '', filesOnly: filePicker?.view === 'files', files: fileResults,
    catalog: contextCatalog, running: bridge.running, planActive: bridge.desktop.permissionMode === 'plan',
    goalAvailable: bridge.desktop.slashCommands.some((entry) => entry.name === 'goal' && !entry.hidden),
    goalHasAttachments: selectedFiles.length > 0 || imageAttachments.length > 0,
  }), [filePicker?.query, filePicker?.view, fileResults, contextCatalog, bridge.running, bridge.desktop.permissionMode, bridge.desktop.slashCommands, selectedFiles.length, imageAttachments.length]);
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
  // Both submenu sizes are resolved here so the panels and their headings agree
  // on whether they are beside the menu or drilled down over it.
  const modelListPlacement = modelPickerSubmenuPlacement(390, submenuBounds);
  const optionListPlacement = modelPickerSubmenuPlacement(300, submenuBounds);
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
    clearSession: (name) => bridge.clearSession(name),
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

  /**
   * The model control is deliberately NOT gated on `ready`.
   *
   * `ready` is the gate for SENDING, and it also demands a trusted workspace
   * and a CONFIGURED PROVIDER. The model control is how the user reaches those:
   * its rows offer "Connect in Settings" for a provider that has none. Gating
   * it on `ready` made the way out of a state unavailable inside that state —
   * and a turn in flight, or a catalog that has not arrived, are not reasons to
   * refuse it either.
   *
   * What is left is the one refusal that is not a lie: nothing is up to take
   * the change. Before the bootstrap there is no session id, while a session is
   * being opened `useBridge`'s `command()` drops whatever is sent, and a
   * disconnected engine rejects it — a pill that looked live and then lost the
   * choice would be worse than a disabled one. So: enabled exactly when a
   * switch would actually land.
   */
  const modelControlReady = Boolean(bridge.hosted && !bridge.loading && bridge.connected && activeSessionId);

  useEffect(() => {
    if (ready) return;
    setPermissionOpen(false);
  }, [ready]);

  useEffect(() => {
    if (modelControlReady) return;
    setModelOpen(false);
    setModelSubmenu(null);
  }, [modelControlReady]);

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

  // How much room a submenu has to fly out into. The composer dock spans the
  // clipping `.desktop-workspace-upper` box exactly, so its left edge IS the
  // boundary; falling back to the window keeps a dock-less host (the fixtures)
  // measuring something real rather than nothing.
  useLayoutEffect(() => {
    if (!modelOpen || !modelSubmenu) {
      setSubmenuBounds(null);
      return;
    }
    const measure = () => {
      const control = modelControl.current;
      if (!control) return;
      const dock = control.closest('.desktop-composer-dock');
      setSubmenuBounds({
        anchorRight: control.getBoundingClientRect().right,
        boundaryLeft: dock ? dock.getBoundingClientRect().left : 0,
      });
    };
    measure();
    window.addEventListener('resize', measure);
    return () => window.removeEventListener('resize', measure);
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

  // The same self-healing the slash menu has, over everything this picker
  // shows. The catalog and the reasoning controls are otherwise asked for
  // exactly once per connection, so a reply lost to a restart (Codex activation
  // restarts the engine mid-connection) left the picker empty, and its Effort
  // row dead, with nothing able to ask again.
  useEffect(() => {
    if (!modelOpen) return;
    void bridge.refreshModelPicker().catch(() => undefined);
  }, [bridge.refreshModelPicker, modelOpen]);

  useEffect(() => {
    if (!slashMenuOpen) return;
    slashControl.current
      ?.querySelector<HTMLElement>(`[data-slash-index="${slashResultIndex}"]`)
      ?.scrollIntoView({ block: 'nearest' });
  }, [slashMenuOpen, slashResultIndex]);

  useEffect(() => {
    if (!fileMenuOpen || !filePicker || (filePicker.view === 'all' && !filePicker.query.trim())) {
      fileSearchRequest.current += 1;
      setFileSearchStatus('idle');
      setFileResults([]);
      setFileResultsTruncated(false);
      setFileResultIndex(0);
      return;
    }
    const request = ++fileSearchRequest.current;
    setFileSearchStatus('loading');
    setFileResults([]);
    setFileResultsTruncated(false);
    const timer = window.setTimeout(() => {
      void bridge.searchWorkspaceFiles(filePicker.query)
        .then((result) => {
          if (fileSearchRequest.current !== request) return;
          setFileResults([...result.files, ...(result.directories ?? []).map((path) => `${path}/`)]);
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
    return () => { window.clearTimeout(timer); fileSearchRequest.current += 1; };
  }, [bridge.searchWorkspaceFiles, activeSessionId, fileMenuOpen, filePicker?.query, filePicker?.view]);

  useEffect(() => {
    setFileResultIndex(0);
  }, [filePicker?.query, filePicker?.view]);

  useEffect(() => {
    setFileResultIndex((index) => Math.min(index, Math.max(0, mentionEntries.length - 1)));
  }, [mentionEntries.length]);

  useEffect(() => {
    if (!fileMenuOpen) return;
    void Promise.allSettled([
      bridge.refreshSkills?.(), bridge.skillAdmin?.({ action: 'get_catalog' }),
      bridge.pluginAdmin?.({ action: 'get_catalog' }), bridge.refreshSlashCommands(),
    ]);
  }, [fileMenuOpen, activeSessionId, bridge.refreshSkills, bridge.skillAdmin, bridge.pluginAdmin, bridge.refreshSlashCommands]);

  useEffect(() => {
    const open = (event: Event) => {
      const mention = (event as CustomEvent<ContextMention>).detail;
      if (mention?.kind === 'skill' || mention?.kind === 'plugin') onOpenSettingsPage(mention.kind === 'skill' ? 'skills' : 'plugins');
      else if (mention?.kind === 'file' || mention?.kind === 'folder') { setPreviewPath(mention.target); setFilePicker(null); setSlashQuery(null); }
    };
    window.addEventListener(OPEN_CONTEXT_MENTION_EVENT, open);
    return () => window.removeEventListener(OPEN_CONTEXT_MENTION_EVENT, open);
  }, [onOpenSettingsPage]);

  useEffect(() => {
    if (!fileMenuOpen) return;
    const pointerDown = (event: PointerEvent) => {
      if (event.target instanceof Node && !fileControl.current?.contains(event.target) && !fileButton.current?.contains(event.target) && !input.current?.contains(event.target)) {
        mentionDismissed.current = true;
        setFilePicker(null);
      }
    };
    const escape = (event: globalThis.KeyboardEvent) => {
      if (event.key !== 'Escape') return;
      event.preventDefault();
      mentionDismissed.current = true;
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
    imageDraftGeneration.current += 1;
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
    composerDraftRevision.current += 1;
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
    imageDraftGeneration.current += 1;
    const editor = input.current;
    if (!editor) return;

    const previousSessionId = draftSessionId.current;
    // Keep text drafted during startup when the temporary session becomes real.
    const carryDraft = activeSessionId && !bridge.error
      && (previousSessionId === null || previousSessionId.startsWith('pending-new:'))
      && !activeSessionId.startsWith('pending-new:');
    if (carryDraft) {
      const snapshot = richPromptSnapshot(editor);
      draftsBySession.current.set(activeSessionId, {
        ...snapshot, html: editor.innerHTML, images: [...imageAttachmentsRef.current],
      });
    }
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
    setPreviewPath(null);
    mentionDismissed.current = false;
    savedEditorSelection.current = null;
    setSlashQuery(null);
    activeMentionRange.current = null;
    activeSlashRange.current = null;
    activeSlashQuery.current = null;
    slashDismissed.current = false;
    setVisualizationChip(null);
  }, [activeSessionId]);

  // A widget drafted a follow-up: put the question in the editor (after any
  // text already typed) and attach the widget as this prompt's context.
  const visualizationFollowup = bridge.visualizationFollowup;
  const clearVisualizationFollowup = bridge.clearVisualizationFollowup;
  useEffect(() => {
    const editor = input.current;
    if (!visualizationFollowup || !editor) return;
    clearVisualizationFollowup();
    const current = richPromptSnapshot(editor).text.trimEnd();
    if (!current) editor.textContent = visualizationFollowup.text;
    else editor.append(document.createTextNode(` ${visualizationFollowup.text}`));
    syncPromptState();
    setVisualizationChip({
      id: visualizationFollowup.reference.id,
      revision: visualizationFollowup.reference.revision,
      title: visualizationFollowup.title,
    });
    editor.focus();
    const range = document.createRange();
    range.selectNodeContents(editor);
    range.collapse(false);
    const selection = window.getSelection();
    selection?.removeAllRanges();
    selection?.addRange(range);
  }, [visualizationFollowup, clearVisualizationFollowup]);

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
    const mention = selection.isCollapsed && node?.nodeType === Node.TEXT_NODE
      && !node.parentElement?.closest('[contenteditable="false"]')
      ? activeFileMention(node.nodeValue ?? '', selection.focusOffset)
      : null;
    if (mention && node) {
      if (mentionDismissed.current) return;
      const range = document.createRange();
      range.setStart(node, mention.start);
      range.setEnd(node, mention.end);
      activeMentionRange.current = range;
      setFilePicker((current) => (
        current?.source === 'mention' && current.query === mention.query
          ? current
          : { source: 'mention', query: mention.query, view: current?.source === 'mention' ? current.view : 'all' }
      ));
      setModelOpen(false);
      setModelSubmenu(null);
      setPermissionOpen(false);
      setPreviewPath(null);
      setSlashQuery(null);
      activeSlashRange.current = null;
      activeSlashQuery.current = null;
      return;
    }
    activeMentionRange.current = null;
    mentionDismissed.current = false;
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

  const reportNativeAudioFailure = useCallback((result: AudioOperationResultDto): boolean => {
    if (result.type !== 'failed') return false;
    if (result.error.kind === 'permission_denied') setVoiceState('denied');
    else if (result.error.kind === 'unavailable' || result.error.kind === 'unsupported') setVoiceState('unsupported');
    else setVoiceState('idle');
    if (result.error.kind !== 'cancelled') setImageNotice(`语音操作失败：${result.error.message}`);
    return true;
  }, []);

  const cancelStandardListening = useCallback(async () => {
    dictationGeneration.current += 1;
    const owner = dictationOwner.current;
    standardListeningRef.current = false;
    dictationOwner.current = null;
    dictationInsertionRange.current = null;
    setVoiceState('idle');
    if (!audio || !owner) return;
    try { await bridge.audioCancel(); } catch {}
  }, [audio, bridge.audioCancel]);

  const cancelAutoplay = useCallback(async () => {
    autoplaySubscription.current?.();
    autoplaySubscription.current = null;
    const owner = autoplayOwner.current;
    autoplayOwner.current = null;
    if (!audio || !owner) return;
    try { await bridge.audioCancel(); } catch {}
  }, [audio, bridge.audioCancel]);

  const startStandardListening = async () => {
    if (!audio || typeof bridge.audioExecute !== 'function') {
      setVoiceState('unsupported');
      return;
    }
    const editor = input.current;
    dictationInsertionRange.current = editor ? editorSelection(editor).cloneRange() : savedEditorSelection.current?.cloneRange() ?? null;
    const owner = audioOwner('dictation');
    dictationOwner.current = owner;
    const generation = ++dictationGeneration.current;
    standardListeningRef.current = true;
    setVoiceState('listening');
    setImageNotice(null);
    let preserveFailureState = false;
    try {
      const response = await bridge.audioExecute({
        type: 'listen',
        language: resolveAudioLanguage(voicePrefs.language, typeof navigator !== 'undefined' ? navigator.language : 'en-US'),
      }, bridge.bootstrap?.settings.voiceRevision ?? 0);
      if (generation !== dictationGeneration.current) return;
      if (reportNativeAudioFailure(response.result)) {
        preserveFailureState = true;
        return;
      }
      if (response.result.type === 'transcript') {
        const transcript = response.result.text.trim();
        if (transcript) insertVoiceTextAtSelection(transcript);
      }
    } catch (cause) {
      if (generation === dictationGeneration.current) setImageNotice(cause instanceof Error ? cause.message : '本机语音识别失败。');
    } finally {
      if (generation === dictationGeneration.current) {
        standardListeningRef.current = false;
        dictationOwner.current = null;
        dictationInsertionRange.current = null;
        if (!preserveFailureState) setVoiceState('idle');
      }
    }
  };

  const finishStandardListening = async () => {
    if (!standardListeningRef.current) return;
    try {
      await bridge.audioFinishListen();
    } catch (cause) {
      setImageNotice(cause instanceof Error ? cause.message : '停止语音识别失败。');
    }
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
        execute: (operation, revision) => bridge.audioExecute(operation, revision),
        cancel: bridge.audioCancel,
        finishListen: bridge.audioFinishListen,
      },
      bridge: {
        sendTrackedPrompt: bridge.sendTrackedPrompt,
        subscribeTrackedSpeech: bridge.subscribeTrackedSpeech,
        cancelTrackedPrompt: bridge.cancelTrackedPrompt,
      },
      getPreferences: () => voicePrefsRef.current,
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
  }, [audio, bridge.audioCancel, bridge.audioExecute, bridge.audioFinishListen, bridge.cancelTrackedPrompt, bridge.sendTrackedPrompt, bridge.subscribeTrackedSpeech]);

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
    return undefined;
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
    const ownerSessionId = activeSessionId;
    const ownerGeneration = imageDraftGeneration.current;
    const ownsDraft = () => ownerSessionId === draftSessionId.current
      && ownerGeneration === imageDraftGeneration.current;
    const remaining = MAX_IMAGE_ATTACHMENTS - imageAttachments.length;
    if (remaining <= 0) {
      setImageNotice(`最多添加 ${MAX_IMAGE_ATTACHMENTS} 张图片。`);
      return;
    }
    const candidates = files.slice(0, remaining);
    if (candidates.length) composerDraftRevision.current += 1;
    const results = await Promise.all(candidates.map(async (file) => {
      try {
        return { attachment: await imageFileToAttachment(file) };
      } catch (error) {
        return { error: error instanceof Error ? error.message : '无法读取图片。' };
      }
    }));
    const attachments = results.flatMap((result) => result.attachment ? [result.attachment] : []);
    if (!ownsDraft()) {
      attachments.forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
      return;
    }
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
      if (!ownsDraft()) {
        attachments.forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
        return current;
      }
      const available = Math.max(0, MAX_IMAGE_ATTACHMENTS - current.length);
      const accepted = attachments.slice(0, available);
      attachments.slice(available).forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
      return [...current, ...accepted];
    });
  };

  const addFiles = async (files: File[]) => {
    if (!ready) return;
    const ownerSessionId = activeSessionId;
    const ownerGeneration = imageDraftGeneration.current;
    const images: File[] = [];
    let notice: string | null = null;
    for (const file of files) {
      if (/^image\/(png|jpeg|gif|webp)$/.test(file.type) || /\.(png|jpe?g|gif|webp)$/i.test(file.name)) {
        images.push(file);
        continue;
      }
      try {
        const path = window.lingxi?.getPathForFile(file);
        if (!path) throw new Error(`无法获取“${file.name}”的本地路径，请先保存文件后再添加。`);
        chooseFile(path, true);
      } catch (error) {
        notice = error instanceof Error ? error.message : `无法添加“${file.name}”。`;
      }
    }
    if (images.length) await addImageFiles(images);
    if (ownerSessionId !== draftSessionId.current || ownerGeneration !== imageDraftGeneration.current) return;
    if (notice || !images.length) setImageNotice(notice);
  };

  const removeImage = (id: string) => {
    composerDraftRevision.current += 1;
    setImageAttachments((current) => {
      const removed = current.find((attachment) => attachment.id === id);
      if (removed) URL.revokeObjectURL(removed.previewUrl);
      return current.filter((attachment) => attachment.id !== id);
    });
  };

  const clearComposer = (sessionId = draftSessionId.current, submittedImageIds?: readonly string[]) => {
    const draft = sessionId ? draftsBySession.current.get(sessionId) : undefined;
    if (sessionId) draftsBySession.current.delete(sessionId);
    if (sessionId !== draftSessionId.current) {
      draft?.images.forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
      return;
    }
    imageDraftGeneration.current += 1;
    composerDraftRevision.current += 1;
    input.current?.replaceChildren();
    savedEditorSelection.current = null;
    activeMentionRange.current = null;
    setText('');
    setSelectedFiles([]);
    setImageAttachments((current) => {
      const removed = submittedImageIds ? new Set(submittedImageIds) : null;
      current.filter((attachment) => !removed || removed.has(attachment.id))
        .forEach((attachment) => URL.revokeObjectURL(attachment.previewUrl));
      return removed ? current.filter((attachment) => !removed.has(attachment.id)) : [];
    });
    setImageNotice(null);
    setFilePicker(null);
    setSlashQuery(null);
    setVisualizationChip(null);
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
    if (bridge.running && isSlashCommand && !isSideQuestionCommand(slashCommand, bridge.desktop.slashCommands)) {
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
    const submittedRevision = composerDraftRevision.current;
    const submitted: ComposerSubmissionSnapshot = {
      sessionId: submittingSessionId, generation: imageDraftGeneration.current,
      html: input.current?.innerHTML, text: snapshot.text, files: [...snapshot.files],
      imageIds: imageAttachments.map((attachment) => attachment.id),
    };
    const ownsVisibleDraft = () => draftSessionId.current === submitted.sessionId
      && imageDraftGeneration.current === submitted.generation && composerDraftRevision.current === submittedRevision;
    let submittedAutoplaySubscription: (() => void) | null = null;
    let submittedAutoplayOwner: NativeAudioOwner | null = null;
    try {
      const supportsTrackedSend = typeof bridge.sendTrackedPrompt === 'function';
      // Start retiring old playback before dispatch, but register this turn's
      // listener without awaiting native cleanup: a fast reply can end meanwhile.
      const previousAutoplayStopped = voicePrefs.autoPlayReplies && audio && supportsTrackedSend
        ? cancelAutoplay() : Promise.resolve();
      const tracked = supportsTrackedSend
        ? bridge.sendTrackedPrompt(
            value,
            images,
            imageAttachments.map((attachment) => attachment.name),
            snapshot.files,
            visualizationChip ? { visualizationContext: visualizationChip } : undefined,
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
        const autoplayTokenOwner = { kind: 'autoplay', id: tracked.token.clientTurnId } satisfies NativeAudioOwner;
        submittedAutoplayOwner = autoplayTokenOwner;
        autoplayOwner.current = autoplayTokenOwner;
        const offTrackedSpeech = bridge.subscribeTrackedSpeech(tracked.token, (event) => {
          if (event.type !== 'completion') return;
          offTrackedSpeech();
          if (autoplaySubscription.current === offTrackedSpeech) autoplaySubscription.current = null;
          void previousAutoplayStopped.then(async () => {
            if (autoplayOwner.current?.id !== autoplayTokenOwner.id) return;
            if (event.terminal === 'stale' || !shouldAutoplayTrackedReply(activeAudioSessionId.current, tracked.token.sessionId, document.hidden)) return;
            const spoken = sanitizeSpeakableText(event.text);
            if (!spoken) return;
            await bridge.audioExecute({
              type: 'speak',
              text: spoken,
              language: resolveAudioLanguage(voicePrefs.language, typeof navigator !== 'undefined' ? navigator.language : 'en-US'),
              rate: voicePrefs.rate,
              ...(voicePrefs.speech.voice ? {
                voice: voicePrefs.speech.voice.source === 'offline'
                  ? `sherpa:${voicePrefs.speech.voice.modelId ?? voicePrefs.speech.offlineModelId ?? ''}:${voicePrefs.speech.voice.id}`
                  : `${voicePrefs.speech.voice.source}:${voicePrefs.speech.voice.id}`,
              } : {}),
            }, bridge.bootstrap?.settings.voiceRevision ?? 0);
          }).catch(() => undefined).finally(() => {
            if (autoplayOwner.current?.id === autoplayTokenOwner.id) autoplayOwner.current = null;
          });
        });
        submittedAutoplaySubscription = offTrackedSpeech;
        autoplaySubscription.current = offTrackedSpeech;
      }
      await queued;
      if (ownsVisibleDraft()) {
        const latest = input.current ? richPromptSnapshot(input.current) : { text, files: selectedFiles };
        if (matchesComposerSubmission(submitted, { ...latest, html: input.current?.innerHTML, images: imageAttachmentsRef.current })) {
          clearComposer(submittingSessionId, submitted.imageIds);
        }
      } else if (submittingSessionId !== draftSessionId.current && submittingSessionId) {
        const saved = draftsBySession.current.get(submittingSessionId);
        if (saved && matchesComposerSubmission(submitted, saved)) clearComposer(submittingSessionId, submitted.imageIds);
      }
    } catch {
      submittedAutoplaySubscription?.();
      if (autoplaySubscription.current === submittedAutoplaySubscription) autoplaySubscription.current = null;
      if (submittedAutoplayOwner && autoplayOwner.current?.id === submittedAutoplayOwner.id) autoplayOwner.current = null;
      if (ownsVisibleDraft()) setImageNotice('发送失败，图片附件已保留，可以重试。');
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

  const chooseFile = (path: string, attachment = false) => {
    const editor = input.current;
    if ((!filePicker && !attachment) || !editor) return;
    const range = !attachment && filePicker?.source === 'mention'
      ? activeMentionRange.current
      : savedEditorSelection.current;
    const insertion = range?.cloneRange() ?? editorSelection(editor);
    if (!attachment && filePicker?.source === 'mention') insertion.deleteContents();

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
    mentionDismissed.current = true;
    syncPromptState();
    setFilePicker(null);
    setFileResults([]);
    focusPrompt(caret);
  };

  const dismissMentions = () => {
    mentionDismissed.current = true;
    setFilePicker(null);
    focusPrompt();
  };

  const consumeMentionQuery = (): Range | null => {
    const editor = input.current;
    if (!editor) return null;
    const range = (filePicker?.source === 'mention' ? activeMentionRange.current : savedEditorSelection.current)?.cloneRange()
      ?? editorSelection(editor);
    if (filePicker?.source === 'mention') range.deleteContents();
    range.collapse(true);
    savedEditorSelection.current = range.cloneRange();
    activeMentionRange.current = null;
    mentionDismissed.current = true;
    setFilePicker(null);
    return range;
  };

  const chooseMention = (entry: MentionMenuEntry) => {
    if (entry.disabled) return;
    if (entry.path !== undefined) { chooseFile(entry.path); return; }
    if (entry.action === 'files') {
      setFilePicker((current) => current ? { ...current, query: '', view: 'files' } : current);
      window.requestAnimationFrame(() => fileSearchInput.current?.focus());
      return;
    }
    const caret = consumeMentionQuery();
    if (!caret) return;
    if (entry.mention && input.current) {
      const href = contextMentionHref(entry.mention);
      const duplicate = [...input.current.querySelectorAll<HTMLElement>('[data-context-mention]')]
        .some((token) => token.dataset.contextMention === href);
      if (!duplicate) {
        const token = createFileMention(entry.mention.target, t.accent);
        delete token.dataset.fileMention;
        token.dataset.contextMention = href;
        token.setAttribute('href', href);
        token.setAttribute('aria-label', `${entry.mention.kind}: ${entry.mention.name}`);
        token.querySelector('svg')?.replaceWith(document.createTextNode('@'));
        token.querySelector('span')!.textContent = entry.mention.name;
        const spacer = document.createTextNode(ZERO_WIDTH_SPACE);
        const fragment = document.createDocumentFragment();
        fragment.append(token, spacer);
        caret.insertNode(fragment);
        caret.setStart(spacer, 1);
        caret.collapse(true);
      }
    } else if (entry.action === 'goal' && input.current) {
      // Goal has arguments; prepare the command and leave submission to the user.
      // Keep any existing draft, attachments and references intact.
      const start = document.createRange();
      start.selectNodeContents(input.current);
      start.collapse(true);
      const prefix = document.createTextNode('/goal ');
      start.insertNode(prefix);
      caret.selectNodeContents(input.current);
      caret.collapse(false);
      slashDismissed.current = true;
    } else if (entry.action === 'plan') {
      void bridge.setPermissionMode('plan').catch(() => setImageNotice('Could not enable Plan mode.'));
    } else if (entry.action === 'attach') {
      imageFileInput.current?.click();
    }
    savedEditorSelection.current = caret.cloneRange();
    syncPromptState();
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
    mentionDismissed.current = false;
    setPreviewPath(null);
    setFilePicker({ source: 'button', query: '', view: 'all' });
    setModelOpen(false);
    setModelSubmenu(null);
    setPermissionOpen(false);
  };

  const filePickerKeyDown = (event: KeyboardEvent<HTMLInputElement | HTMLDivElement>) => {
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (fileMenuOpen) {
      if (event.key === 'ArrowDown') {
        event.preventDefault();
        setFileResultIndex((index) => moveSlashSelectionIndex(index, 'next', mentionEntries.length));
        return;
      }
      if (event.key === 'ArrowUp') {
        event.preventDefault();
        setFileResultIndex((index) => moveSlashSelectionIndex(index, 'previous', mentionEntries.length));
        return;
      }
      if (event.key === 'Escape') {
        event.preventDefault();
        dismissMentions();
        return;
      }
      if (event.key === 'Enter' || event.key === 'Tab') {
        event.preventDefault();
        const selected = mentionEntries[fileResultIndex];
        if (selected) chooseMention(selected);
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
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    // Enter on a focused reference follows the link instead of submitting the draft.
    const reference = event.target instanceof Element
      ? event.target.closest<HTMLElement>('[data-context-mention], [data-file-mention]') : null;
    if (reference) {
      if (event.key === 'Enter') { event.preventDefault(); reference.click(); }
      return;
    }
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
    if (fileMenuOpen && ['ArrowDown', 'ArrowUp', 'Enter', 'Tab', 'Escape'].includes(event.key)) return;
    updateActiveCompletions();
  };

  const copyPromptReferences = (event: ClipboardEvent<HTMLDivElement>) => {
    const editor = input.current;
    const selection = window.getSelection();
    if (!editor || !selection?.rangeCount || selection.isCollapsed) return;
    const range = selection.getRangeAt(0);
    if (!editor.contains(range.commonAncestorContainer)) return;
    const fragment = document.createElement('div');
    fragment.append(range.cloneContents());
    if (!fragment.querySelector('[data-context-mention], [data-file-mention]')) return;
    const snapshot = richPromptSnapshot(fragment);
    event.preventDefault();
    event.clipboardData.setData('text/plain', promptWithFileMentions(snapshot.text, snapshot.files));
    if (event.type === 'cut') {
      range.deleteContents();
      syncPromptState();
      updateActiveCompletions();
    }
  };

  const pastePlainText = (event: ClipboardEvent<HTMLDivElement>) => {
    const clipboardImages = [...event.clipboardData.items]
      .filter((item) => item.kind === 'file')
      .map((item) => item.getAsFile())
      .filter((file): file is File => Boolean(file));
    if (clipboardImages.length) {
      event.preventDefault();
      void addFiles(clipboardImages);
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
  const goal = composerGoalState(bridge.conversation?.items ?? []);
  const goalActive = goal.active;
  const promptPlaceholder = !ready
    ? !workspace?.path || workspace.recovery
      ? 'Add or select an available project to start coding…'
      : !providerConfigured
        ? 'Connect a provider in Settings to start coding…'
        : 'Waiting for the local engine…'
    : goalActive
        ? 'Add guidance for this goal…'
        : bridge.running
          ? 'LingXi is working — draft your next message…'
          : 'Do anything';
  const hasPrompt = Boolean(text.trim() || selectedFiles.length);
  const draftCommand = parseSlashCommandPrefix(text);
  const hasMentionTokens = selectedFiles.length > 0 || Boolean(input.current?.querySelector('[data-context-mention]'));
  const decorateCommand = Boolean(draftCommand && !promptComposing && !hasMentionTokens);
  const promptTextStyle: CSSProperties = {
    display: 'block', width: '100%', minHeight: 56, maxHeight: 160, overflowY: 'auto',
    border: 0, outline: 0, background: 'transparent', lineHeight: 1.5, fontSize: 15,
    padding: draftCommand ? '16px 18px 8px 56px' : '16px 18px 8px',
    fontWeight: 400, letterSpacing: 'normal', whiteSpace: 'pre-wrap', overflowWrap: 'anywhere',
  };
  const copyDecoratedPrompt = (event: ClipboardEvent<HTMLDivElement>) => {
    const selection = window.getSelection();
    if (!selection?.rangeCount || selection.isCollapsed
      || !event.currentTarget.contains(selection.getRangeAt(0).commonAncestorContainer)) return;
    // The editor's ink is hidden behind a visual layer. Copy plain source text
    // so a rich-text destination never inherits that transparent styling.
    event.clipboardData.setData('text/plain', selection.toString());
    event.preventDefault();
    if (event.type === 'cut') document.execCommand('delete');
  };
  useLayoutEffect(() => {
    if (commandMirror.current && input.current) {
      commandMirror.current.scrollTop = input.current.scrollTop;
      commandMirror.current.scrollLeft = input.current.scrollLeft;
    }
  }, [text, decorateCommand]);
  const canStop = bridge.running || runningSubagentIds(bridge.runtimeCenter).length > 0;
  const stopTurnButton = (
    <button
      type="button"
      className="composer-submit-button"
      disabled={!canStop || hasPrompt || bridge.isCancelling}
      tabIndex={canStop && !hasPrompt ? 0 : -1}
      onClick={() => invoke(() => bridge.cancel())}
      aria-label={bridge.isCancelling ? (bridge.running ? 'Stopping current turn' : 'Stopping background agents') : (bridge.running ? 'Stop current turn' : 'Stop background agents')}
      title={bridge.isCancelling ? 'Stopping…' : 'Stop'}
      style={{ ...composerSendStyle(t, true), background: t.danger, cursor: bridge.isCancelling ? 'wait' : 'pointer', opacity: bridge.isCancelling ? .7 : 1 }}
    ><Icon name="stop" size={15} color="#fff" /></button>
  );
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
      {goalActive && <GoalStatus key={`${activeSessionId}:${goal.objective}`} objective={goal.objective} disabled={!ready || bridge.sessionLoading} running={bridge.running} cancelling={bridge.isCancelling} onClear={() => bridge.runSlashCommand('/goal clear')} onPause={() => bridge.cancel()} onResume={() => bridge.sendPrompt('Continue pursuing the current goal.')} />}
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
          void addFiles([...event.dataTransfer.files]);
        }}
        style={{ position: 'relative', maxWidth: 'var(--conversation-width)', margin: '0 auto', borderRadius: 16, border: `1px solid ${imageDragActive ? t.accent : t.border}`, background: imageDragActive ? t.accentBg : t.surface, boxShadow: '0 2px 8px rgba(0,0,0,.06)', overflow: 'visible', transition: 'border-color 0.16s ease, background-color 0.16s ease, box-shadow 0.16s ease' }}
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
        {visualizationChip && (
          <div style={{ padding: '10px 18px 0' }}>
            <VisualizationContextBadge chip={visualizationChip} onDismiss={() => setVisualizationChip(null)} />
          </div>
        )}
        <div className="composer-input-view">
          <div
            ref={input}
            className="beta-rich-prompt"
            role="textbox"
            contentEditable
            suppressContentEditableWarning
            spellCheck
            data-placeholder={promptPlaceholder}
            data-empty={!hasPrompt ? 'true' : 'false'}
            data-command-decorated={decorateCommand || undefined}
            onInput={() => { mentionDismissed.current = false; slashDismissed.current = false; setImageNotice(null); syncPromptState(); updateActiveCompletions(); }}
            onFocus={() => { slashDismissed.current = false; savePromptSelection(); updateActiveCompletions(); }}
            onBlur={savePromptSelection}
            onKeyUp={keyUp}
            onMouseUp={updateActiveCompletions}
            onKeyDown={keyDown}
            onPaste={pastePlainText}
            onCopy={(event) => { copyPromptReferences(event); if (!event.defaultPrevented && decorateCommand) copyDecoratedPrompt(event); }}
            onCut={(event) => { copyPromptReferences(event); if (!event.defaultPrevented && decorateCommand) copyDecoratedPrompt(event); }}
            onClick={(event) => {
              const token = (event.target as HTMLElement).closest<HTMLElement>('[data-context-mention], [data-file-mention]');
              if (!token) return;
              event.preventDefault();
              const mention = parseContextMentionHref(token.dataset.contextMention ?? '');
              if (mention) onOpenSettingsPage(mention.kind === 'skill' ? 'skills' : 'plugins');
              else if (token.dataset.fileMention) {
                setFilePicker(null);
                setSlashQuery(null);
                setPreviewPath(token.dataset.fileMention);
              }
            }}
            onCompositionStart={() => setPromptComposing(true)}
            onCompositionEnd={() => { setPromptComposing(false); syncPromptState(); updateActiveCompletions(); }}
            onScroll={(event) => {
              if (!commandMirror.current) return;
              commandMirror.current.scrollTop = event.currentTarget.scrollTop;
              commandMirror.current.scrollLeft = event.currentTarget.scrollLeft;
            }}
            aria-label="Prompt"
            aria-multiline="true"
            aria-disabled={false}
            aria-autocomplete="list"
            aria-controls={slashMenuOpen ? 'slash-command-results' : fileMenuOpen && filePicker?.source === 'mention' ? 'mention-results' : undefined}
            aria-describedby={slashMenuOpen ? 'slash-command-hint' : fileMenuOpen ? 'mention-hint' : undefined}
            aria-expanded={slashMenuOpen || (fileMenuOpen && filePicker?.source === 'mention')}
            aria-activedescendant={slashMenuOpen && slashCommands[slashResultIndex]
              ? `slash-command-result-${slashResultIndex}`
              : fileMenuOpen && filePicker?.source === 'mention' && mentionEntries[fileResultIndex]
                ? `mention-result-${fileResultIndex}`
                : undefined}
            style={{ ...promptTextStyle, color: decorateCommand ? 'transparent' : t.text, caretColor: t.text, cursor: 'text' }}
          />
          {draftCommand && (
            <span className="composer-command-marker"><CommandIcon command={draftCommand.name} size={28} /></span>
          )}
          {draftCommand && decorateCommand && (
            <div ref={commandMirror} className="composer-command-mirror" aria-hidden="true" style={{
              ...promptTextStyle, color: t.text,
              '--command-identity-color': commandPaletteColor(draftCommand.name, t.dark),
            } as CSSProperties}>
              <span className="composer-command-prefix">{draftCommand.prefix}</span>{draftCommand.rest}
            </div>
          )}
        </div>
        {previewPath && <FileMentionPreview key={`${activeSessionId}:${previewPath}`} path={previewPath} bridge={bridge} onClose={() => { setPreviewPath(null); focusPrompt(); }} />}
        {slashMenuOpen && (
          <div ref={slashControl} className="slash-command-menu" style={commandMenuStyle(t)}>
            <div className="slash-command-header">
              <span className="slash-command-header-mark" aria-hidden="true">/</span>
              <strong>Commands</strong>
              {slashQuery && <span className="slash-command-query mono">/{slashQuery}</span>}
              <span className="slash-command-count">{slashCommands.length} match{slashCommands.length === 1 ? '' : 'es'}</span>
            </div>
            <div id="slash-command-results" className="slash-command-list" role="listbox" aria-label="Slash commands">
              {slashCommands.length === 0 && (
                <div className="slash-command-empty" role="status">
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
                    title={`${slashCommandText(entry.name)}${entry.argument_hint ? ` ${entry.argument_hint}` : ''}\n${description}`}
                    onMouseDown={(event) => event.preventDefault()}
                    onMouseEnter={() => setSlashResultIndex(index)}
                    onFocus={() => setSlashResultIndex(index)}
                    onClick={() => chooseSlashCommand(entry.name)}
                  >
                    <CommandIcon command={entry.name} size={32} />
                    <span className="slash-command-copy">
                      <span className="slash-command-heading">
                        <span className="slash-command-name">{entry.name}</span>
                        {entry.argument_hint ? <span className="slash-command-argument mono">{entry.argument_hint}</span> : null}
                      </span>
                      <span className="slash-command-description">{description}</span>
                    </span>
                    <kbd className="slash-command-complete" aria-hidden="true">↵</kbd>
                  </button>
                );
              })}
            </div>
            <div id="slash-command-hint" className="slash-command-footer">
              <span><kbd>↑</kbd><kbd>↓</kbd> Navigate</span>
              <span><kbd>↵</kbd><kbd>Tab</kbd> Complete</span>
              <span><kbd>esc</kbd> Close</span>
            </div>
          </div>
        )}
        <div ref={fileControl}>
          {fileMenuOpen && <MentionMenu
            entries={mentionEntries} selectedIndex={fileResultIndex} query={filePicker?.query ?? ''}
            filesOnly={filePicker?.view === 'files'} searchInput={fileSearchInput}
            status={fileSearchStatus} truncated={fileResultsTruncated}
            onQuery={(query) => setFilePicker((current) => current ? { ...current, query } : current)}
            onSelectIndex={setFileResultIndex} onChoose={chooseMention} onKeyDown={filePickerKeyDown}
            onClose={dismissMentions} onBack={() => setFilePicker((current) => current ? { ...current, view: 'all', query: '' } : current)}
          />}
        </div>
        {voiceState === 'listening' && !flowMode ? (
          <DictationRecorderBar
            audio={audio}
            turnAction={canStop ? stopTurnButton : undefined}
            onCancel={() => { void cancelStandardListening(); }}
            onFinish={() => { void finishStandardListening(); }}
          />
        ) : (
        <div className="composer-toolbar" style={{ display: 'flex', alignItems: 'center', gap: 4, minHeight: 48, padding: '0 8px 8px' }}>
          <input ref={imageFileInput} type="file" multiple onChange={(event) => { void addFiles(event.target.files ? [...event.target.files] : []); event.currentTarget.value = ''; }} style={{ display: 'none' }} />
          <button type="button" disabled={!ready} className="composer-icon-action" aria-label="Attach files" title="Attach files or images" onClick={() => imageFileInput.current?.click()} style={{ ...composerIconStyle(t), width: 40, height: 40 }}><Icon name="image" size={19} color={t.text2} stroke={1.7} /></button>
          <button ref={fileButton} type="button" disabled={!ready} className="composer-icon-action" aria-label="Add context" aria-expanded={fileMenuOpen} title="Add context (@)" onMouseDown={savePromptSelection} onClick={openFileMenu} style={{ ...composerIconStyle(t), width: 40, height: 40 }}><Icon name="plus" size={21} color={t.text2} stroke={1.7} /></button>
          <div ref={permissionControl} style={{ position: 'relative' }}>
            <button
              ref={permissionButton}
              type="button"
              disabled={!ready}
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


          <div style={{ flex: 1 }} />

          <ContextWindow key={activeSessionId ?? 'new-session'}
            usage={bridge.sessionLoading ? null : bridge.usage}
            capacity={currentModelDetail?.context_window_tokens} />
          <div ref={modelControl} className="composer-model-control" style={{ position: 'relative', minWidth: 0 }}>
            <button
              ref={modelTrigger}
              type="button"
              // NOT disabled on an empty catalog. Empty means "the list has not
              // arrived", not "there is nothing to pick" — the engine always
              // lists at least the current model — and disabling on it turned a
              // lost reply into a control the user could never reach again. The
              // menu below asks for the catalog when it opens, so the empty case
              // is a panel that fills in rather than a dead pill.
              disabled={!modelControlReady}
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
            {modelControlReady && modelOpen && (
              <>
              <div
                style={{
                  ...composerMenuStyle(t, 'right'),
                  width: MODEL_PICKER_MENU_WIDTH,
                  maxWidth: `min(${MODEL_PICKER_MENU_WIDTH}px, calc(100vw - 44px))`,
                  padding: 8,
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
              </div>

              {modelSubmenu === 'model' && (
                <div style={{ ...modelPickerSubmenuStyle(t, modelListPlacement), maxHeight: 'min(500px, calc(100vh - 140px))', overflow: 'hidden', display: 'flex', flexDirection: 'column' }} role="menu" aria-label="Available models">
                  {modelPickerSubmenuHeading(t, 'Model', '5px 10px 7px', modelListPlacement, () => setModelSubmenu(null))}
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
                <div style={{ ...modelPickerSubmenuStyle(t, optionListPlacement), maxHeight: 'min(430px, calc(100vh - 140px))', overflow: 'hidden' }} role="menu" aria-label="Reasoning effort">
                  {modelPickerSubmenuHeading(t, 'Effort', '5px 10px 8px', optionListPlacement, () => setModelSubmenu(null))}
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
                <div style={{ ...modelPickerSubmenuStyle(t, optionListPlacement), maxHeight: 'min(300px, calc(100vh - 140px))', overflow: 'hidden' }} role="menu" aria-label="Speed">
                  {modelPickerSubmenuHeading(t, 'Speed', '5px 10px 8px', optionListPlacement, () => setModelSubmenu(null))}
                  <button type="button" role="menuitemradio" aria-checked={!bridge.desktop.fastMode || !fastModeAvailable} disabled={!modelControlReady} onClick={() => invoke(() => bridge.setFastMode(false))} style={speedOptionStyle(t, !bridge.desktop.fastMode || !fastModeAvailable, !modelControlReady)}>
                    <span style={{ flex: 1 }}><span style={{ display: 'block', fontSize: 13, fontWeight: 540 }}>Standard</span><span style={{ display: 'block', marginTop: 2, color: t.text3, fontSize: 11.5 }}>Default speed</span></span>
                    {(!bridge.desktop.fastMode || !fastModeAvailable) && <Icon name="check" size={16} color={t.accent} stroke={2.2} />}
                  </button>
                  {fastModeAvailable && <button type="button" role="menuitemradio" aria-checked={bridge.desktop.fastMode} disabled={!modelControlReady} onClick={() => invoke(() => bridge.setFastMode(true))} style={speedOptionStyle(t, bridge.desktop.fastMode, !modelControlReady)}>
                    <span style={{ flex: 1 }}><span style={{ display: 'block', fontSize: 13, fontWeight: 540 }}>Fast</span><span style={{ display: 'block', marginTop: 2, color: t.text3, fontSize: 11.5 }}>1.5x speed, more usage</span></span>
                    {bridge.desktop.fastMode && <Icon name="check" size={16} color={t.accent} stroke={2.2} />}
                  </button>}
                </div>
              )}
              </>
            )}
          </div>
          <button type="button" disabled={!ready || flowMode} className="composer-icon-action" aria-label={voiceState === 'listening' && !flowMode ? 'Stop ordinary recording' : 'Start ordinary recording'} title={voiceState === 'unsupported' ? 'Voice input is unavailable in this environment' : voiceState === 'denied' ? 'Microphone permission was denied' : '普通录音'} onClick={toggleStandardVoice} style={{ ...composerPrimaryActionStyle(t, ready && !flowMode), color: voiceState === 'listening' && !flowMode ? t.accent : voiceState === 'denied' ? t.danger : t.text }}><Icon name="mic" size={18} color="currentColor" stroke={voiceState === 'listening' && !flowMode ? 2.1 : 1.8} /></button>
          {!canStop && !hasPrompt && <button
            type="button"
            disabled={!ready}
            className="composer-icon-action"
            aria-label={flowMode ? '关闭心流模式' : '开启心流模式'}
            aria-pressed={flowMode}
            title={flowMode ? '关闭心流模式' : '开启心流模式'}
            onClick={toggleFlowMode}
            style={{ ...composerPrimaryActionStyle(t, ready), color: flowMode ? t.accent : t.text }}
          >
            <Icon name="waveform" size={18} color="currentColor" stroke={2.15} />
          </button>}
          <div className="composer-submit-actions" style={!canStop && !hasPrompt ? { display: 'none' } : undefined}>
            <span className="composer-stop-presence" data-visible={canStop && !hasPrompt} aria-hidden={!canStop || hasPrompt}>
              {stopTurnButton}
            </span>
            <span className="composer-send-presence" data-visible={hasPrompt} aria-hidden={!hasPrompt}>
              <button
                type="button"
                className="composer-submit-button"
                disabled={!ready || !hasPrompt || flowMode}
                tabIndex={hasPrompt ? 0 : -1}
                onClick={() => { void submit(); }}
                aria-label={bridge.running ? 'Send pending message' : 'Send prompt'}
                title={bridge.running ? 'Send as pending message' : 'Send prompt'}
                style={composerSendStyle(t, Boolean(ready && hasPrompt && !flowMode))}
              ><Icon name="arrowU" size={18} color="currentColor" /></button>
            </span>
          </div>
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
  return { ...composerPrimaryActionStyle(t, enabled), background: enabled ? `color-mix(in srgb, ${t.accent} 85%, #000)` : t.surfaceActive, color: enabled ? '#fff' : t.text4, opacity: enabled ? 1 : .82 };
}

/** How far a composer menu floats above the control it belongs to. */
const COMPOSER_MENU_LIFT = 9;

function composerMenuStyle(t: ReturnType<typeof useT>, side: 'left' | 'right'): CSSProperties {
  return { position: 'absolute', bottom: `calc(100% + ${COMPOSER_MENU_LIFT}px)`, [side]: 0, zIndex: 20, width: 286, padding: 7, borderRadius: 12, border: `0.5px solid ${t.borderStrong}`, background: t.surface, boxShadow: '0 16px 40px rgba(0,0,0,.22)', animation: 'fade-in .15s ease' };
}

/**
 * A submenu's section title.
 *
 * Drilled down over the menu, the title is the only way BACK to it, so there it
 * is a control; beside the menu the rows it would return to are already on
 * screen and a back button is noise.
 */
function modelPickerSubmenuHeading(
  t: ReturnType<typeof useT>,
  label: string,
  padding: string,
  placement: ModelPickerSubmenuPlacement,
  onBack: () => void,
): ReactNode {
  if (placement.right !== 0) {
    return <div style={{ flexShrink: 0, padding, color: t.text3, fontSize: 12, fontWeight: 600 }}>{label}</div>;
  }
  return (
    <button
      type="button"
      onClick={onBack}
      aria-label={`Back to model settings from ${label}`}
      style={{ flexShrink: 0, display: 'flex', alignItems: 'center', gap: 5, width: '100%', padding, border: 0, background: 'transparent', color: t.text3, font: 'inherit', fontSize: 12, fontWeight: 600, textAlign: 'left', cursor: 'pointer' }}
    >
      <Icon name="chevronL" size={13} color="currentColor" stroke={2} />
      <span>{label}</span>
    </button>
  );
}

/**
 * A model-picker submenu, positioned against the model CONTROL rather than
 * against the menu it belongs to. Both are right-aligned to the same control,
 * so the offsets below are plain border-box arithmetic — nesting the submenu
 * inside the menu would measure them from its padding box instead, which is
 * how {@link modelPickerSubmenuPlacement}'s width budget and the rendered gap
 * would drift apart.
 */
function modelPickerSubmenuStyle(t: ReturnType<typeof useT>, placement: ModelPickerSubmenuPlacement): CSSProperties {
  const bottom = placement.right === 0
    // Drilled down over the menu: share its bottom edge so it cannot peek out.
    ? COMPOSER_MENU_LIFT
    : COMPOSER_MENU_LIFT + MODEL_PICKER_SUBMENU_GAP;
  return { position: 'absolute', right: placement.right, bottom: `calc(100% + ${bottom}px)`, zIndex: 21, width: placement.width, padding: 8, borderRadius: 12, border: `0.5px solid ${t.borderStrong}`, background: t.surface, boxShadow: '0 16px 40px rgba(0,0,0,.22)', animation: 'fade-in .15s ease' };
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

export { ErrorBanner } from './ErrorBanner';
