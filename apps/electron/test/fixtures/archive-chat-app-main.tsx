import * as React from 'react';
import { createRoot } from 'react-dom/client';
import { App } from '../../src/renderer/App';
import { LANGUAGE_STORAGE_KEY } from '../../src/renderer/i18n/locale';
import '../../src/renderer/global.css';

// Only unrelated surfaces and the renderer/Host transport are replaced. App's
// actual deep-link handler and BetaSidebar/ArchiveChatDialog are used intact.
export const LocalApps = () => null;
export const SideChatPanel = () => null;
export const BetaComposer = () => <input id="new-session-focus" aria-label="New session focus" />;
export const BetaTopBar = () => null;
export const ErrorBanner = () => null;
export const ComputerAccessPrompt = () => null;
export const AskUserQuestionPrompt = () => null;
export const PermissionPrompt = () => null;
export const SettingsScreen = ({ bridge }: { bridge: any }) => <button id="settings-open-b"
  onClick={() => { void bridge.openSession('/fixture', 'B'); }}>Open B from settings</button>;
export const ScheduledTasks = () => null;
export const Stage = () => null;
export const ModUiAbovePrompt = () => null;
export const ModUiParentSite = ({ children }: { children: React.ReactNode }) => children;
export const ModUiSurfaceClientIdProvider = ({ children }: { children: React.ReactNode }) => children;
export const CommandResultPanel = () => null;
export const RuntimeCenterInspector = () => null;
export const RuntimeCenterOverview = () => null;
export const GitWorkspaceProvider = ({ children }: { children: React.ReactNode }) => children;
export const TerminalPanel = () => null;
export const useTerminalPanel = () => ({ open: false, hasCommandReview: false, toggle() {}, async openLink() {} });
export const useSideChats = () => ({ open: false, selected: null, sides: [], create() {}, setOpen() {} });
export const SessionPicker = () => null;
export const restoreSessionPickerFocus = () => {};
export const CreateProjectDialog = () => null;

const projectPath = '/fixture';
const sessions = [
  { uuid: 'A', title: 'Chat A', modified_rfc3339: '', message_count: 0 },
  { uuid: 'B', title: 'Chat B', modified_rfc3339: '', message_count: 0 },
];
const noop = async () => {};
let releaseHostArchive: (() => void) | null = null;
let releaseArchiveDelivery: (() => void) | null = null;
let releaseNavigation: (() => void) | null = null;
let releasePreflight: (() => void) | null = null;
let holdArchive = false;
let holdNavigation = false;
let holdPreflight = false;
let archiveOutcome: 'failure' | 'success' = 'failure';
let preflightOutcome: 'failure' | 'success' = 'failure';
let archiveCalls = 0;
let completedArchiveDeliveries = 0;
let completedPreflights = 0;
let opened: string | null = null;
let queue: Promise<void> = Promise.resolve();
let pendingLink: unknown = null;
let linkListener: (() => void) | null = null;
let menuListener: ((action: string) => void) | null = null;
const acknowledgements: unknown[] = [];

(window as any).archiveAppFixture = {
  holdArchive(outcome: 'failure' | 'success') { holdArchive = true; archiveOutcome = outcome; },
  releaseHostArchive() { releaseHostArchive?.(); releaseHostArchive = null; },
  releaseArchiveDelivery() { releaseArchiveDelivery?.(); releaseArchiveDelivery = null; },
  holdNextNavigation() { holdNavigation = true; },
  releaseNavigation() { releaseNavigation?.(); releaseNavigation = null; },
  holdPreflight(outcome: 'failure' | 'success') { holdPreflight = true; preflightOutcome = outcome; },
  releasePreflight() { releasePreflight?.(); releasePreflight = null; },
  openSettings() { menuListener?.('settings'); },
  deliverSession(id: string) {
    pendingLink = { id: `link-${id}`, projectPath, target: { route: `session/${id}`, params: {} } };
    linkListener?.();
  },
  state() { return { hostArchiveHeld: !!releaseHostArchive, archiveDeliveryHeld: !!releaseArchiveDelivery,
    navigationHeld: !!releaseNavigation, preflightHeld: !!releasePreflight,
    archiveCalls, completedArchiveDeliveries, completedPreflights, opened, acknowledgements }; },
};
(window as any).lingxi = {
  onEvent: () => () => {}, onConnectionStateChanged: () => () => {},
  onApplicationMenu(callback: (action: string) => void) { menuListener = callback; return () => { menuListener = null; }; },
  deepLinks: {
    async next() { const value = pendingLink; pendingLink = null; return value; },
    async acknowledge(...args: unknown[]) { acknowledgements.push(args); },
    onAvailable(callback: () => void) { linkListener = callback; return () => { linkListener = null; }; },
  },
};
localStorage.setItem(LANGUAGE_STORAGE_KEY, 'en');

export function useBridge(): any {
  const [settledSession, setSettledSession] = React.useState({ projectPath, sessionId: 'A' });
  const [pendingSession, setPendingSession] = React.useState<{ projectPath: string; sessionId: string } | null>(null);
  const [visibleSessions, setVisibleSessions] = React.useState(sessions);
  const activeSession = pendingSession ?? settledSession;
  return {
    activeSession, sessionLoading: pendingSession !== null,
    bootstrap: { settings: { projects: [projectPath], activeProject: projectPath, pinnedSessions: [], theme: 'light' },
      activeSession: settledSession, workspace: { path: projectPath, trusted: true }, scheduledWorkspace: '/scheduled',
      projectCatalogs: { [projectPath]: { sessions: visibleSessions }, '/scheduled': { sessions: [] } }, providerCredentials: [] },
    hosted: true, loading: false, connected: true, projectCreationOpen: false,
    conversation: { sessionKey: activeSession.sessionId, items: [], foldedItemIds: [], modStatuses: [] },
    runtimeCenter: { submittedPlanState: { calls: [] }, agents: {}, overviewOpen: false, inspectorOpen: false },
    desktop: { sessions: [], tasks: {}, models: [] },
    sessionRuntimeStatus: () => undefined,
    preflightSessionArchive: async () => {
      if (!holdPreflight) return [];
      holdPreflight = false;
      await new Promise<void>(resolve => { releasePreflight = resolve; });
      completedPreflights += 1;
      if (preflightOutcome === 'failure') throw new Error('Delayed preflight A failed');
      return [{ id: 'old-job', cron: '0 9 * * 1', prompt: 'Old task', recurring: true }];
    },
    archiveSession: async (_project: string, id: string) => {
      archiveCalls += 1;
      if (!holdArchive) { setVisibleSessions(current => current.filter(session => session.uuid !== id)); return; }
      holdArchive = false;
      const hostResult = queue.then(async () => {
        await new Promise<void>(resolve => { releaseHostArchive = resolve; });
        if (archiveOutcome === 'failure') throw new Error('Delayed archive A failed');
      });
      // The real Host serializes navigation even if the previous request fails.
      queue = hostResult.then(() => undefined, () => undefined);
      let failure: unknown;
      try { await hostResult; } catch (error) { failure = error; }
      // Delivery to the old renderer caller may lag the next queued navigation.
      await new Promise<void>(resolve => { releaseArchiveDelivery = resolve; });
      completedArchiveDeliveries += 1;
      if (failure) throw failure;
    },
    openSession: async (project: string, id: string) => {
      setPendingSession({ projectPath: project, sessionId: id });
      await queue;
      if (holdNavigation) {
        holdNavigation = false;
        await new Promise<void>(resolve => { releaseNavigation = resolve; });
      }
      setSettledSession({ projectPath: project, sessionId: id });
      setPendingSession(null);
      opened = id;
      return { projectPath: project, sessionId: id };
    },
    setRuntimeCenterOverviewOpen: () => {}, openRuntimeItem: () => {}, offerLinkDraft: () => {},
    listProjectSessions: async () => ({ sessions: visibleSessions }), setThemePreference: noop,
  };
}

createRoot(document.getElementById('root')!).render(<App />);
