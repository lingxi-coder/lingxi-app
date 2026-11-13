import { LocalAppsSidebar } from './components/LocalAppsSidebar';
import { useLocalAppWorkspace } from './bridge/useLocalAppWorkspace';
import type { AppDevelopmentChat } from './bridge/localAppWorkspace';
import { tr } from './i18n/locale';
import { useI18n } from './i18n/useI18n';
import { LocalApps, LocalAppsTopBar } from './components/LocalApps';
import './components/LocalApps.css';
import { Bots } from './components/Bots';
import { BotProposalAction } from './components/BotProposalCard';
import { prepareBotCreationChat, type BotCreationProposal } from './bridge/botCreation';
import './components/Bots.css';
import { useBots } from './bridge/useBots';
import { useBotSessions } from './bridge/useBotSessions';
import { useSidebarMode } from './bridge/useSidebarMode';
import { modeForSettledChat, pageForMode, type AppPage } from './bridge/sidebarMode';
import type { SidebarMode } from '../shared/settings';
import { BotsSidebar } from './components/BotsSidebar';
import { TeamsSidebar, focusTeamsSidebarSoon } from './components/TeamsSidebar';
import { FriendsPanel } from './components/FriendsPanel';
// The Teams workspace's stylesheet (sidebar, page, panel and dialogs), imported once, here, as the package asks. Its rules are scoped to its own classes.
import '../../../../packages/teams/styles';
import { useTeams } from './bridge/useTeams';
import { useFriends } from './bridge/useFriends';
import { friendLink, type FriendLink } from '../../../../packages/teams/friends';
import { useTeamsDrawer } from './bridge/useTeamsDrawer';
import { teamsThemeVariables } from './theme/teamsTheme';
import type { DrawerTab } from '../../../../packages/teams/ui';
import { botScopesKey } from './bridge/botRoutines';
import { LocalAppAuthoringError, localAppChatTarget, type LocalAppDraft } from './bridge/localAppChat';
import { localAppAuthoringErrorPresentation, localAppErrorMessage } from './bridge/localAppErrors';
import type { ActionableError } from './bridge/errors';
import type { LocalAppEditRequest } from '../shared/localApps';
import type { TerminalScope } from '../shared/terminal';
import { settingsLinkRequest, runtimeLinkItem, type SettingsLinkRequest, type ScheduledLinkRequest } from './deepLinkRoutes';
import { SETTINGS_NAV } from './components/settings/nav';
import { useSideChats } from './bridge/useSideChats';
import { SideChatPanel } from './components/SideChatPanel';
import './components/SideChatPanel.css';
import { conversationPlans } from './bridge/planDocuments';
import { selectedGitScope } from './bridge/gitScope';
import {
  lazy,
  Suspense,
  useEffect,
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type CSSProperties,
} from 'react';

import { useBridge } from './bridge/useBridge.js';
import {
  BetaComposer,
  BetaSidebar,
  BetaTopBar,
  ErrorBanner,
} from './components/BetaDesktop';
import { ComputerAccessPrompt } from './components/ComputerAccessPrompt';
import { AskUserQuestionPrompt } from './components/AskUserQuestionPrompt';
import { PermissionPrompt } from './components/PermissionPrompt';
import { SettingsScreen } from './components/settings/SettingsScreen';
import { ScheduledTasks, ScheduledTasksTopBar } from './components/ScheduledTasks';
import { Stage } from './components/Stage';
import { ModUiAbovePrompt, ModUiParentSite, ModUiSurfaceClientIdProvider } from './components/modUiAbovePrompt';
import { askUserQuestionWithResponseProps, nativeAskUserQuestionProps } from './components/nativeUiSiteProps';
import { Theme } from './theme/ThemeContext';
import { tokens, watchThemePreference, type ThemeMode } from './theme/tokens';
import { RuntimeCenterInspector, RuntimeCenterOverview } from './components/RuntimeCenter';
import './components/RuntimeCenter.css';
import './components/GitReview.css';
import { GitChangesPage, GitWorkspaceProvider } from './components/GitReview';
import './components/TranscriptAgents.css';
import './components/TerminalPanel.css';
import { TerminalPanel, useTerminalPanel } from './components/TerminalPanel';
import { restoreSessionPickerFocus, SessionPicker } from './components/SessionPicker';
import './components/SessionPicker.css';
import { SESSION_PICKER_EVENT } from '../shared/sessionWorkspace';
import { CreateProjectDialog } from './components/CreateProjectDialog';
import { AccountGate } from './components/AccountGate';

interface SessionPickerRequest {
  id: number;
  projectPath?: string;
  query: string;
  opener: HTMLElement | null;
}

// The Teams page loads with its first visit: the conversation and the Bot cards in it are the weight. The sidebar and the workspace behind both are not lazy, so the rail can count unread messages from the start.
const TeamsMain = lazy(() => import('./components/TeamsMain').then(module => ({ default: module.TeamsMain })));
// Likewise the Friends page; its sidebar and workspace are not lazy, so the rail can badge requests and unread chats from the start.
const FriendsPage = lazy(() => import('./components/FriendsPage').then(module => ({ default: module.FriendsPage })));

export function App() {
  const { locale, preference } = useI18n();
  useEffect(() => { void window.lingxi?.setInterfaceLocale?.(locale, preference === '').catch(() => undefined); }, [locale, preference]);
  const [messageTarget, setMessageTarget] = useState<string | null>(null);
  const [messageRequest, setMessageRequest] = useState<{ projectPath: string; sessionId: string; uuid: string } | null>(null);
  const messageCompletion = useRef<((found: boolean) => void) | null>(null);
  const [terminalLinkScope, setTerminalLinkScope] = useState<TerminalScope | null>(null);
  const [scheduledTarget, setScheduledTarget] = useState<ScheduledLinkRequest | undefined>();
  const [pageSidebar, setPageSidebar] = useState<HTMLDivElement | null>(null);
  const [activityView, setActivityView] = useState(false);
  const [localAppDraft, setLocalAppDraft] = useState<LocalAppDraft>();
  const [localAppInstallStatus, setLocalAppInstallStatus] = useState('');
  const [localAppNavigationError, setLocalAppNavigationErrorState] = useState('');
  const [localAppNavigationPresentation, setLocalAppNavigationPresentation] = useState<ActionableError>();
  const setLocalAppNavigationError = useCallback((message: string, presentation?: ActionableError) => {
    setLocalAppNavigationErrorState(message);
    setLocalAppNavigationPresentation(presentation);
  }, []);
  const clearLocalAppNavigationError = useCallback(() => setLocalAppNavigationError(''), [setLocalAppNavigationError]);
  const localAppNavigation = useRef(0);
  const localAppProjectTarget = useRef<string>();
  const localAppNavigationQueue = useRef<Promise<void>>(Promise.resolve());
  const [page, setPage] = useState<AppPage>('chat');
  const [teamInvite, setTeamInvite] = useState<string>();
  const handledTeamInvite = useCallback(() => setTeamInvite(undefined), []);
  // A friend link waits here, in memory only (D8), until the Friends workspace of a signed-in account can take it.
  const [friendTarget, setFriendTarget] = useState<FriendLink>();
  const handledFriendCode = useCallback(() => setFriendTarget(undefined), []);
  const [theme, setTheme] = useState<ThemeMode>('dark');
  const [settingsRoute, setSettingsRoute] = useState<SettingsRoute | null>(null);
  const [sessionPicker, setSessionPicker] = useState<SessionPickerRequest | null>(null);
  const sessionPickerRequest = useRef<SessionPickerRequest | null>(null);
  const sessionPickerSequence = useRef(0);
  const palette = useMemo(() => tokens(theme === 'dark'), [theme, locale]);
  const bridge = useBridge();
  const botsApi = window.lingxi?.bots;
  const bots = useBots(botsApi);
  const { setMode: setSidebarMode, mode: sidebarMode } = useSidebarMode({
    page, setPage, botsAvailable: bots.available, botsKnown: bots.state.availability !== undefined,
    settings: bridge.bootstrap?.settings, save: bridge.updateSidebarPreferences,
  });
  const appWorkspace = useLocalAppWorkspace(bridge);
  const localAppBridge = useRef(bridge);
  localAppBridge.current = bridge;
  // The Scheduled page lists the workspace of every Bot as a scope; ask for the scopes again when Bots are made, renamed or deleted.
  const botScopes = botScopesKey(bots.state.bots);
  const seenBotScopes = useRef<string>();
  const { refreshScheduledScopes } = bridge;
  useEffect(() => {
    const before = seenBotScopes.current;
    seenBotScopes.current = botScopes;
    if (before !== undefined && before !== botScopes) refreshScheduledScopes();
  }, [botScopes, refreshScheduledScopes]);
  const sessionNavigationBlocked = useCallback(() => localAppBridge.current.sessionLoading || localAppBridge.current.projectCreationOpen, []);
  const openLocalAppChat = useCallback((app?: LocalAppEditRequest) => {
    if (sessionNavigationBlocked()) return;
    const nonce = ++localAppNavigation.current;
    setLocalAppDraft(undefined);
    setLocalAppNavigationError('');
    const navigate = async () => {
      if (localAppBridge.current.bootstrap?.conversationSpaces.activeMode !== 'code') {
        const switched = await localAppBridge.current.switchConversationSpace('code');
        if (!switched || localAppNavigation.current !== nonce) return;
      }
      const current = localAppBridge.current;
      const target = await localAppChatTarget(app, {
        mode: 'code',
        requiresCodeMessage: tr('local_app_development_requires_code'),
        hasProject: path => Boolean(current.bootstrap?.settings.projects.includes(path)
          || current.bootstrap?.scheduledWorkspace === path),
        openSession: (projectPath, sessionId) => localAppBridge.current.openSession(projectPath, sessionId, undefined, 'code', app?.id, app?.authoringVersion),
        addProject: current.addProject,
        newSession: (projectPath, intent) => localAppBridge.current.newSession(projectPath, { kind: 'local-app', intent }),
        recoverSession: requestId => localAppBridge.current.newSession(undefined, { kind: 'recover-local-app', requestId }),
      }, () => localAppNavigation.current === nonce);
      if (!target || localAppNavigation.current !== nonce) return;
      localAppProjectTarget.current = target.projectPath;
      setSidebarMode('apps');
      setPage('chat');
      setActivityView(false);
      setLocalAppDraft({ id: app?.id ?? '', name: app?.name ?? '', nonce, target });
    };
    localAppNavigationQueue.current = localAppNavigationQueue.current.then(navigate, navigate).catch(error => {
      if (localAppNavigation.current === nonce) {
        if (error instanceof LocalAppAuthoringError) setLocalAppNavigationError(error.message, localAppAuthoringErrorPresentation(error));
        else setLocalAppNavigationError(localAppErrorMessage(error));
      }
    });
  }, [sessionNavigationBlocked, setLocalAppNavigationError, setSidebarMode]);
  const consumeLocalAppDraft = useCallback((nonce: number) => {
    setLocalAppDraft(current => current?.nonce === nonce ? undefined : current);
  }, []);
  const cancelLocalAppNavigation = useCallback(() => {
    localAppNavigation.current++;
    setLocalAppDraft(undefined);
    setLocalAppNavigationError('');
  }, [setLocalAppNavigationError]);
  const openPage = useCallback((next: AppPage) => {
    if (sessionNavigationBlocked()) return;
    if (next !== 'chat' && localAppBridge.current.handoffPending) void localAppBridge.current.cancelSessionHandoff();
    cancelLocalAppNavigation();
    setPage(next);
  }, [cancelLocalAppNavigation, sessionNavigationBlocked]);
  useEffect(() => () => { localAppNavigation.current++; }, []);
  const [visibleModToast, setVisibleModToast] = useState<typeof bridge.conversation.modToast>(null);
  useEffect(() => {
    const toast = bridge.conversation.modToast;
    setVisibleModToast(toast);
    if (!toast) return;
    const timer = window.setTimeout(() => setVisibleModToast(null), toast.timeoutMs);
    return () => window.clearTimeout(timer);
  }, [bridge.conversation.modToast]);
  const toggleActivityView = () => {
    if (sessionNavigationBlocked()) return;
    cancelLocalAppNavigation();
    setPage('chat');
    setActivityView(!activityView);
    bridge.setRuntimeCenterOverviewOpen(!activityView);
  };
  useEffect(() => {
    const toggle = (event: KeyboardEvent) => {
      if (sessionNavigationBlocked()) return;
      if ((event.metaKey || event.ctrlKey) && event.altKey && !event.shiftKey && event.key.toLowerCase() === 'u' && !settingsRoute && !sessionPicker) {
        event.preventDefault();
        cancelLocalAppNavigation();
        setPage('chat');
        setActivityView(!activityView);
        bridge.setRuntimeCenterOverviewOpen(!activityView);
      }
    };
    window.addEventListener('keydown', toggle);
    return () => window.removeEventListener('keydown', toggle);
  }, [settingsRoute, sessionPicker, activityView, bridge.setRuntimeCenterOverviewOpen, cancelLocalAppNavigation, sessionNavigationBlocked]);
  useEffect(() => window.lingxi?.onLocalAppEdit?.(openLocalAppChat), [openLocalAppChat]);
  useEffect(() => window.lingxi?.onConversationShown?.(() => {
    cancelLocalAppNavigation();
    setSettingsRoute(null);
    setPage('chat');
    setActivityView(false);
  }), [cancelLocalAppNavigation]);
  // The host made a chat of a Bot the displayed one on its own (a notification was clicked): bring the window to it.
  useEffect(() => botsApi?.onSessionShown?.((session) => {
    cancelLocalAppNavigation();
    setSettingsRoute(null);
    setPage('chat');
    setActivityView(false);
    void localAppBridge.current.showBotSession(session);
  }), [botsApi, cancelLocalAppNavigation]);
  const botRoutineRequests = useRef(0);
  /** Send the owner to the Scheduled page, to what a Bot's routines panel pointed at. */
  const openRoutine = useCallback((target: { kind: 'new'; scope: string } | { kind: 'job'; id: string; scope: string }) => {
    if (sessionNavigationBlocked()) return;
    cancelLocalAppNavigation();
    setScheduledTarget({ requestId: `bot-routine:${++botRoutineRequests.current}`, ...target });
    setPage('scheduled');
  }, [cancelLocalAppNavigation, sessionNavigationBlocked]);
  const workspace = bridge.bootstrap?.workspace;
  // A Bot's folder is not a project: with a Bot's chat on screen, project-bound parts of the window follow the selected project instead.
  const activeBotId = bridge.activeBotId;
  const botChat = activeBotId !== undefined;
  const activeBot = botChat ? bots.state.bots.find((bot) => bot.botId === activeBotId) : undefined;
  const botSessions = useBotSessions(botsApi);
  // Teams: one workspace for the window, which the sidebar, the page and the rail's unread count all read.
  const zh = locale.startsWith('zh');
  const teams = useTeams({}, bridge.setBotsKeepAwake);
  const friends = useFriends();
  const teamsDrawer = useTeamsDrawer({
    savedWidth: bridge.bootstrap?.settings.sidebar?.teamsDrawerWidth,
    save: (width) => { void bridge.updateSidebarPreferences({ teamsDrawerWidth: width }).catch(() => undefined); },
  });
  const { show: showTeamDetails, toggle: toggleTeamDetails } = teamsDrawer;
  const teamsTheme = useMemo(() => teamsThemeVariables(palette), [palette]);
  // The sidebar follows the chat on screen when that changes from a Bot's chat to a project's or back (a link, the picker, a notification),
  // so the chat is always listed beside it. Only settled changes count: a chat that is still opening says nothing yet.
  const appChat = !botChat && bridge.activeSession?.mode === 'code' && (Boolean(appWorkspace.activeGroup)
    || (localAppProjectTarget.current !== undefined && localAppProjectTarget.current === bridge.activeSession.projectId));
  const settledChatOwner = useRef(`${botChat}:${appChat}:${bridge.activeSession?.sessionId}`);
  useEffect(() => {
    const owner = `${botChat}:${appChat}:${bridge.activeSession?.sessionId}`;
    if (bridge.sessionLoading || settledChatOwner.current === owner) return;
    settledChatOwner.current = owner;
    const next = modeForSettledChat(sidebarMode, page, botChat, appChat);
    if (next !== sidebarMode) setSidebarMode(next);
  }, [botChat, appChat, bridge.activeSession?.sessionId, bridge.sessionLoading, page, sidebarMode, setSidebarMode]);
  // Whether the sidebar is hidden. The View menu toggles it here, and the sidebar's own button reports its change here too.
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  /** Run `go`, which takes the owner off the Bots page. A Bot there with edits that are not saved is asked about first, with the question of its own Back button; `go` waits for the answer. */
  const leaveBots = useCallback((go: () => void) => { if (page === 'bots') bots.guard.leave(go); else go(); }, [bots.guard, page]);
  /** The rail's Home, Apps, Bots and Teams: the sidebar lists what the mode is for, and the main area goes to the page that goes with it. `after` runs once it does. */
  const selectSidebarMode = useCallback((mode: SidebarMode, after?: () => void) => {
    if (sessionNavigationBlocked() || (mode === 'bots' && !bots.available)) return;
    const next = pageForMode(mode, { page, botChat, appChat });
    const go = () => { setSidebarMode(mode); openPage(next); after?.(); };
    // Staying on the page, or coming to it, leaves nothing behind.
    if (next === page) go(); else leaveBots(go);
  }, [bots.available, botChat, appChat, leaveBots, openPage, page, sessionNavigationBlocked, setSidebarMode]);
  /** Bring Teams forward when its sidebar is shown beside a utility page. */
  const showTeams = useCallback(() => { if (page !== 'teams') openPage('teams'); }, [openPage, page]);
  /** Likewise Friends. */
  const showFriends = useCallback(() => { if (page !== 'friends') openPage('friends'); }, [openPage, page]);
  // A link to the requests or to a friend's chat is carried out as soon as there is a workspace to carry it out in.
  const { workspace: friendsWorkspace } = friends;
  useEffect(() => {
    if (!friendsWorkspace || !friendTarget || friendTarget.kind === 'add') return;
    if (friendTarget.kind === 'chat') friendsWorkspace.openChat(friendTarget.uid); else friendsWorkspace.showRequests();
    setFriendTarget(undefined);
  }, [friendsWorkspace, friendTarget]);
  /** The team menu of the sidebar: the details panel beside the conversation, on a tab when one is asked for. */
  const openTeamDetails = useCallback((tab?: DrawerTab) => { showTeams(); showTeamDetails(tab); }, [showTeams, showTeamDetails]);
  const showChat = useCallback(() => {
    cancelLocalAppNavigation();
    setPage('chat');
    setActivityView(false);
  }, [cancelLocalAppNavigation]);
  const showAppLibrary = useCallback(() => { setSidebarMode('apps'); openPage('apps'); }, [openPage, setSidebarMode]);
  const openAppDevelopmentChat = useCallback(async (chat: AppDevelopmentChat) => {
    if (sessionNavigationBlocked()) return;
    cancelLocalAppNavigation();
    try {
      const opened = await localAppBridge.current.openSession(chat.projectPath, chat.sessionId, chat.selection, 'code');
      if (opened) { setSidebarMode('apps'); setPage('chat'); setActivityView(false); }
    } catch (cause) { setLocalAppNavigationError(localAppErrorMessage(cause)); }
  }, [cancelLocalAppNavigation, sessionNavigationBlocked, setLocalAppNavigationError, setSidebarMode]);
  const appAction = useCallback(async (id: string, edit: boolean) => {
    if (sessionNavigationBlocked()) return;
    try {
      if (edit && localAppBridge.current.bootstrap?.conversationSpaces.activeMode !== 'code') {
        if (!await localAppBridge.current.switchConversationSpace('code')) return;
      }
      const app = appWorkspace.groups.find(group => group.app?.id === id)?.app;
      if (edit && app?.marketOrigin && !app.authoring?.sourceDirectory) await window.lingxi?.localApps?.remix(id);
      else if (edit) await window.lingxi?.localApps?.edit(id);
      else await window.lingxi?.localApps?.open(id);
    } catch (cause) { setLocalAppNavigationError(localAppErrorMessage(cause)); }
  }, [appWorkspace.groups, sessionNavigationBlocked, setLocalAppNavigationError]);
  const botDeleteRequests = useRef(0);
  const { available: botsAvailable, view: botsView, setView: setBotsView, guard: botsGuard } = bots;
  /**
   * Show a Bot's page, from the sidebar or from the title of its chat; with `delete`, the page asks whether to delete the Bot.
   * The page of the Bot being edited keeps its edits. Another Bot's page replaces it, and them, so that asks about them first.
   */
  const openBot = useCallback((botId: string, intent?: 'delete') => {
    if (sessionNavigationBlocked() || !botsAvailable) return;
    const go = () => {
      setBotsView({ kind: 'detail', botId, ...(intent === 'delete' ? { deleteRequest: ++botDeleteRequests.current } : {}) });
      setSidebarMode('bots');
      openPage('bots');
    };
    if (botsView.kind === 'detail' && botsView.botId === botId) go(); else botsGuard.leave(go);
  }, [botsAvailable, botsGuard, botsView, openPage, sessionNavigationBlocked, setBotsView, setSidebarMode]);
  const createBot = useCallback(() => {
    if (sessionNavigationBlocked() || !botsAvailable) return;
    botsGuard.leave(() => {
      setBotsView({ kind: 'create' });
      setSidebarMode('bots');
      openPage('bots');
    });
  }, [botsAvailable, botsGuard, openPage, sessionNavigationBlocked, setBotsView, setSidebarMode]);
  const openBotCreationChat = useCallback(async (request: string) => {
    if (sessionNavigationBlocked() || !botsAvailable) throw new Error(tr('bots_assist_open_failed'));
    const nonce = ++localAppNavigation.current;
    setLocalAppDraft(undefined);
    const target = await prepareBotCreationChat(request, {
      newSession: purpose => localAppBridge.current.newSession(undefined, purpose),
      offerLinkDraft: draft => localAppBridge.current.offerLinkDraft(draft),
    }, () => localAppNavigation.current === nonce);
    if (localAppNavigation.current !== nonce) return;
    if (!target) throw new Error(tr('bots_assist_open_failed'));
    botsGuard.hold(false);
    setBotsView({ kind: 'list' });
    setSidebarMode('home');
    setPage('chat');
    setActivityView(false);
  }, [botsAvailable, botsGuard, sessionNavigationBlocked, setBotsView, setSidebarMode]);
  const reviewBotProposal = useCallback((proposal: BotCreationProposal) => {
    if (sessionNavigationBlocked() || !botsAvailable) return;
    botsGuard.leave(() => {
      setBotsView({ kind: 'create', proposal, requestId: ++localAppNavigation.current });
      setSidebarMode('bots');
      openPage('bots');
    });
  }, [botsAvailable, botsGuard, openPage, sessionNavigationBlocked, setBotsView, setSidebarMode]);
  const sessionPickerProject = (botChat ? undefined : bridge.activeSession?.projectPath) ?? bridge.bootstrap?.settings.activeProject ?? (botChat ? undefined : workspace?.path) ?? bridge.bootstrap?.settings.projects[0];
  const closeSessionPicker = useCallback(() => {
    const request = sessionPickerRequest.current;
    sessionPickerRequest.current = null;
    setSessionPicker(null);
    window.requestAnimationFrame(() => {
      if (sessionPickerRequest.current) return;
      restoreSessionPickerFocus(request?.opener ?? null, document.querySelector<HTMLElement>('.beta-rich-prompt'));
    });
  }, []);
  const openedPickerSession = useCallback(() => {
    cancelLocalAppNavigation();
    setPage('chat');
    setActivityView(false);
    setSettingsRoute(null);
  }, [cancelLocalAppNavigation]);
  useEffect(() => {
    const openPicker = (event: Event) => {
      if (sessionNavigationBlocked()) return;
      if (localAppBridge.current.handoffPending) void localAppBridge.current.cancelSessionHandoff();
      cancelLocalAppNavigation();
      const detail = (event as CustomEvent<{ query?: unknown }>).detail;
      const request: SessionPickerRequest = {
        id: ++sessionPickerSequence.current,
        projectPath: sessionPickerProject,
        query: typeof detail?.query === 'string' ? detail.query.trim() : '',
        opener: sessionPickerRequest.current?.opener ?? (document.activeElement instanceof HTMLElement ? document.activeElement : null),
      };
      sessionPickerRequest.current = request;
      setSessionPicker(request);
    };
    window.addEventListener(SESSION_PICKER_EVENT, openPicker);
    return () => window.removeEventListener(SESSION_PICKER_EVENT, openPicker);
  }, [sessionPickerProject, sessionNavigationBlocked, cancelLocalAppNavigation]);
  const planCalls = useMemo(() => conversationPlans(bridge.conversation.items, bridge.runtimeCenter.submittedPlanState.calls), [bridge.runtimeCenter.submittedPlanState.calls, bridge.conversation.items]);
  const planBridge = { ...bridge, runtimeCenter: { ...bridge.runtimeCenter, submittedPlan: planCalls.at(-1) ?? bridge.runtimeCenter.submittedPlan, submittedPlanState: { ...bridge.runtimeCenter.submittedPlanState, calls: planCalls } } };
  // Stage's agent-anchor cache compares this prop BY REFERENCE, so an inline
  // `Object.values(...)` (or a bare `[]` literal) would hand it a new identity on
  // every render, invalidating the anchor map and, through it, the transcript
  // `rows` memo — turning a memoized transcript into a full re-walk per render.
  const stageAgents = useMemo(
    () => bridge.sessionLoading ? [] : Object.values(bridge.runtimeCenter.agents),
    [bridge.sessionLoading, bridge.runtimeCenter.agents],
  );
  // `newSession` publishes a synthetic `pending-new:<opId>` id until the engine
  // answers. It is not a session id the main process will ever accept, so keying
  // the scope-bound panels on it empties the terminal tab strip and makes any
  // terminal opened in that window fail validation.
  const settledSession = bridge.activeSession?.sessionId?.startsWith('pending-new:') ? undefined : bridge.activeSession;
  const sideChats = useSideChats(settledSession?.sessionId);
  const gitScope = botChat ? null : selectedGitScope(bridge.bootstrap?.settings.activeProject, settledSession, workspace?.path);
  const terminalProject = terminalLinkScope?.projectPath ?? (botChat ? undefined : bridge.activeSession?.projectPath ?? workspace?.path);
  const terminalScope = bridge.loading || bridge.sessionLoading ? null
    : terminalLinkScope ?? (terminalProject ? { projectPath: terminalProject, sessionId: settledSession?.sessionId ?? '__draft__' } : null);
  const terminal = useTerminalPanel(terminalScope, page === 'chat' && settingsRoute === null);
  useEffect(() => { if (!terminal.hasCommandReview) setTerminalLinkScope(null); }, [settledSession?.sessionId, terminal.hasCommandReview]);
  const providerConfigured = Boolean(bridge.bootstrap?.providerAccountSnapshot?.modelAvailability?.some(model => model.eligibleAccountIds.length > 0));
  const ready = Boolean(
    !bridge.loading
    && bridge.hosted
    && workspace?.path
    && workspace.trusted
    && providerConfigured
    && bridge.connected
    && !bridge.sessionLoading,
  );
  useEffect(() => {
    // `'system'` isn't a third palette (see `ThemeMode`) — it's a preference
    // that resolves to one of the two, and keeps following the OS via the
    // media query's `change` event for as long as this effect is mounted.
    const preference = bridge.bootstrap?.settings.theme;
    return watchThemePreference(preference, window.matchMedia('(prefers-color-scheme: dark)'), setTheme);
  }, [bridge.bootstrap?.settings.theme]);
  const changeTheme = (value: ThemeMode) => {
    setTheme(value);
    void bridge.setThemePreference(value).catch(() => undefined);
  };
  const openSettings = useCallback((route?: Pick<SettingsRoute, 'pageId' | 'providerId' | 'pendingModelReference' | 'linkRequest'>) => {
    if (sessionNavigationBlocked()) return;
    if (localAppBridge.current.handoffPending) void localAppBridge.current.cancelSessionHandoff();
    cancelLocalAppNavigation();
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSettingsRoute({ ...route, restoreFocus: () => opener?.focus() });
  }, [cancelLocalAppNavigation, sessionNavigationBlocked]);
  useEffect(() => window.lingxi?.onApplicationMenu?.((action) => {
    if (sessionPicker || sessionNavigationBlocked()) return;
    if (action === 'settings') {
      // Preserve the original focus restoration target when already open.
      if (!settingsRoute) openSettings();
    } else if (action === 'activity' && !settingsRoute) {
      cancelLocalAppNavigation();
      setPage('chat');
      setActivityView(!activityView);
      bridge.setRuntimeCenterOverviewOpen(!activityView);
    } else if ((action === 'home' || action === 'bots' || action === 'teams' || action === 'friends' || action === 'apps') && !settingsRoute) {
      // Bots is ignored where Bots cannot run: the menu shows that item dimmed.
      // A key brought the person to Teams, so the keyboard comes along: to the open conversation in the sidebar, unless it is in Teams already.
      selectSidebarMode(action, action === 'teams' ? () => { if (!document.activeElement?.closest('.teams-sidebar, .teams-page')) focusTeamsSidebarSoon(); } : undefined);
    } else if (action === 'toggle-sidebar' && !settingsRoute) {
      setSidebarCollapsed((collapsed) => !collapsed);
    } else if (action === 'team-details' && !settingsRoute) {
      // The panel belongs to the Teams page: on any other page the menu item does nothing.
      if (page === 'teams' && teams.workspace) toggleTeamDetails();
    }
  }), [openSettings, settingsRoute, sessionPicker, activityView, bridge.setRuntimeCenterOverviewOpen, cancelLocalAppNavigation, sessionNavigationBlocked, selectSidebarMode, page, teams.workspace, toggleTeamDetails]);

  const linkBusy = useRef(false);
  const linkSurfacePending = useRef(false);
  useEffect(() => { linkSurfacePending.current = Boolean(settingsRoute || sessionPicker || bridge.projectCreationOpen || terminal.hasCommandReview); }, [settingsRoute, sessionPicker, bridge.projectCreationOpen, terminal.hasCommandReview]);
  const linkHandler = useRef<() => void>(() => {});
  linkHandler.current = () => {
    const api = window.lingxi?.deepLinks;
    if (!api || !bridge.bootstrap || bridge.sessionLoading || bridge.projectCreationOpen || settingsRoute || sessionPicker || terminal.hasCommandReview || linkSurfacePending.current || linkBusy.current) return;
    linkBusy.current = true;
    let delivered = false;
    void (async () => {
      const delivery = await api.next();
      if (!delivery) return;
      if (sessionNavigationBlocked()) {
        await api.acknowledge(delivery.id, tr('desktop_link_wait'));
        return;
      }
      cancelLocalAppNavigation();
      delivered = true;
      let error: string | undefined;
      try {
        const { route, params } = delivery.target;
        const [kind, id, detailId] = route.split('/');
        const settingsTarget = settingsLinkRequest(delivery.target, delivery.id);
        // Project/local settings are owned by the selected conversation's engine.
        // Merely changing activeProject leaves that engine in the previous cwd.
        if ((settingsTarget || kind === 'settings' || kind === 'provider') && delivery.projectPath && delivery.projectPath !== bridge.activeSession?.projectPath) {
          const catalog = await bridge.listProjectSessions(delivery.projectPath);
          const session = catalog?.sessions.find((row) => {
            const owner = bridge.bootstrap?.conversationSpaces.sessions.find((entry) => entry.sessionId === row.uuid);
            return row.mode === bridge.bootstrap?.conversationSpaces.activeMode && (!owner || owner.projectId === delivery.projectPath);
          });
          if (!session) throw new Error(tr('desktop_link_open_project_config'));
          if (!await bridge.openSession(delivery.projectPath, session.uuid, { scope: 'current', transcriptPath: session.path })) throw new Error(tr('desktop_link_project_conversation_error'));
        }
        const friend = friendLink(delivery.target);
        // The join dialog (and the friend dialog likewise) opens in the sidebar, which cannot be used while it is hidden.
        if (route === 'team/join' || route === 'team/inbox') { setTeamInvite(params.code); setPage('teams'); setSidebarCollapsed(false); }
        else if (friend) { setFriendTarget(friend); setPage('friends'); setSidebarCollapsed(false); }
        else if (settingsTarget) { linkSurfacePending.current = true; openSettings(settingsTarget); }
        else if (kind === 'settings') {
          if (!SETTINGS_NAV.some(page => page.id === id)) throw new Error(tr('desktop_link_settings_unavailable'));
          linkSurfacePending.current = true;
          openSettings({ pageId: id });
        } else if (kind === 'provider') { linkSurfacePending.current = true; openSettings({ pageId: 'provider-credentials', providerId: id }); }
        else if (kind === 'session' || kind === 'project') {
          setMessageTarget(null);
          if (!delivery.projectPath) throw new Error(tr('desktop_link_project_unregistered'));
          if (kind === 'session') {
            if (params.mode !== 'chat' && params.mode !== 'code') throw new Error(tr('desktop_link_conversation_error'));
            if (!await bridge.openSession(delivery.projectPath, id!, undefined, params.mode)) throw new Error(tr('desktop_link_conversation_error'));
          }
          if (kind === 'project') await bridge.activateProject(delivery.projectPath);
          setPage('chat');
          setActivityView(false);
          if (params.message) {
            await new Promise<void>((resolve, reject) => {
              messageCompletion.current = found => { messageCompletion.current = null; found ? resolve() : reject(new Error(tr('desktop_link_message_unavailable'))); };
              setMessageRequest({ projectPath: delivery.projectPath!, sessionId: id!, uuid: params.message! });
            });
          }
        } else if (kind === 'new') {
          const session = await bridge.newSession(delivery.projectPath);
          if (!session) throw new Error(tr('desktop_link_create_error'));
          if (typeof delivery.target.payload?.prompt === 'string') bridge.offerLinkDraft({ sessionId: session.sessionId, text: delivery.target.payload.prompt, requestId: delivery.id });
          setPage('chat');
        } else if (kind === 'terminal') {
          const projectPath = delivery.projectPath ?? bridge.activeSession?.projectPath ?? bridge.bootstrap?.settings.activeProject;
          if (!projectPath) throw new Error(tr('desktop_link_terminal_project'));
          if (projectPath !== bridge.activeSession?.projectPath) await bridge.activateProject(projectPath);
          setPage('chat');
          setActivityView(false);
          const scope = { projectPath, sessionId: params.scope ?? (bridge.activeSession?.projectPath === projectPath ? settledSession?.sessionId ?? '__draft__' : '__draft__') };
          setTerminalLinkScope(scope);
          if (typeof delivery.target.payload?.command === 'string') linkSurfacePending.current = true;
          await terminal.openLink(scope, id!, typeof delivery.target.payload?.command === 'string' ? delivery.target.payload.command : undefined);
        } else if (kind === 'market' && id === 'bot') {
          const botApi = window.lingxi?.bots;
          if (!botApi) throw new Error('Bot host is unavailable');
          const result = await botApi.market({ action: 'install', listingId: detailId!, ...(params.versionId ? {versionId:params.versionId} : {}), ...(params.intentId ? {intentId:params.intentId} : {}) });
          if (result && 'botId' in result && 'revision' in result) { bots.controller.put(result); bots.setView({kind:'detail',botId:result.botId}); setPage('bots'); }
        } else if (kind === 'market') {
          setPage('apps');
          setLocalAppNavigationError('');
          setLocalAppInstallStatus('Preparing local application…');
          try {
            const localApps = window.lingxi?.localApps;
            if (!localApps) throw new Error('Local application host is unavailable');
            const localApp = await localApps.marketInstallLink(id!);
            setLocalAppInstallStatus(localApp ? `${localApp.name} · Available in local apps` : '');
          } catch (error) { setLocalAppInstallStatus(''); throw error; }
        } else if (kind === 'cron') {
          setScheduledTarget({ requestId: delivery.id, kind: id as 'job' | 'run', id: detailId!, scope: delivery.projectPath ?? params.scope });
          setPage('scheduled');
        } else if (kind === 'runtime') {
          if (delivery.projectPath && delivery.projectPath !== bridge.activeSession?.projectPath) throw new Error(tr('desktop_link_runtime_project'));
          const item = runtimeLinkItem(id!, detailId!, bridge, planCalls);
          setPage('chat');
          if (item === 'activity') { setActivityView(true); bridge.setRuntimeCenterOverviewOpen(true); }
          else { setActivityView(false); bridge.openRuntimeItem(item); }
        } else throw new Error(tr('desktop_link_destination_unavailable'));
      } catch (cause) { linkSurfacePending.current = false; error = cause instanceof Error ? cause.message : tr('desktop_link_open_failed'); }
      await api.acknowledge(delivery.id, error);
    })().catch(() => undefined).finally(() => {
      linkBusy.current = false;
      if (delivered) queueMicrotask(() => linkHandler.current());
    });
  };
  useEffect(() => {
    const unsubscribe = window.lingxi?.deepLinks?.onAvailable(() => linkHandler.current());
    linkHandler.current();
    return unsubscribe;
  }, [bridge.bootstrap, bridge.sessionLoading, bridge.projectCreationOpen, settingsRoute, sessionPicker, terminal.hasCommandReview]);
  useEffect(() => {
    if (!messageRequest || bridge.sessionLoading) return;
    if (bridge.activeSession?.sessionId !== messageRequest.sessionId || bridge.activeSession?.projectPath !== messageRequest.projectPath) {
      messageCompletion.current?.(false);
      setMessageRequest(null);
      return;
    }
    if (bridge.conversation.sessionKey !== messageRequest.sessionId) return;
    setMessageTarget(messageRequest.uuid);
  }, [messageRequest, bridge.sessionLoading, bridge.activeSession?.sessionId, bridge.activeSession?.projectPath, bridge.conversation.sessionKey]);
  const onMessageLocated = useCallback((found: boolean) => {
    if (!messageCompletion.current) return;
    messageCompletion.current(found);
    setMessageRequest(null);
  }, []);

  const emptyMessage = bridge.sessionLoading
    ? tr('desktop_shell_loading_session')
    : !workspace?.path
    ? tr('desktop_shell_start_project')
    : bridge.activeSession
      ? tr('desktop_shell_empty_session')
      : tr('desktop_shell_create_session');
  const askQuestion = bridge.pendingAskUserQuestion;
  const sessionPickerSuspended = Boolean(sessionPicker && bridge.sessionLoading
    && (bridge.pendingPermission || bridge.pendingAskUserQuestion || bridge.pendingComputerAccess));

  return (
    <Theme.Provider value={palette}>
      <AccountGate>
      <ModUiSurfaceClientIdProvider clientId={bridge.uiSurfaceClientId}>
      <GitWorkspaceProvider scope={gitScope} onOpen={() => openPage('changes')} onTerminal={() => { openPage('chat'); if (!terminal.open) terminal.toggle(); }}>
      <div
        className="desktop-shell"
        data-screen-label="LingXi Code Desktop Beta"
        style={{ width: '100vw', height: '100vh', overflow: 'hidden', display: 'flex', position: 'relative', background: palette.sidebarBg, color: palette.text, colorScheme: theme, '--desktop-accent': palette.accent, '--rail-material': palette.sidebarBg, '--nav-panel-border': palette.border, ...teamsTheme } as CSSProperties}
      >
        <SettingsBackground active={bridge.projectCreationOpen || settingsRoute !== null || (sessionPicker !== null && !sessionPickerSuspended)}>
          <BetaSidebar
            bridge={bridge}
            pageSidebarRef={setPageSidebar}
            activityView={activityView}
            onToggleActivityView={toggleActivityView}
            apps={page === 'apps'}
            changes={page === 'changes'}
            onOpenApps={showAppLibrary}
            botsAvailable={bots.available}
            teamsUnread={teams.snapshot?.totalUnread ?? 0}
            friendsUnread={friends.snapshot?.badge ?? 0}
            sidebarMode={sidebarMode}
            localAppProjects={appWorkspace.projectPaths}
            onSelectSidebarMode={selectSidebarMode}
            modePanel={(mode, search) => mode === 'apps'
              ? <LocalAppsSidebar groups={appWorkspace.groups} activeSessionId={bridge.activeSession?.sessionId}
                  chatVisible={page === 'chat'} libraryVisible={page === 'apps'} query={search}
                  busy={sessionNavigationBlocked()} loading={appWorkspace.loading} error={appWorkspace.error} onRetry={() => { void appWorkspace.refresh(); }}
                  onLibrary={showAppLibrary} onCreate={() => openLocalAppChat()}
                  onNewChat={group => openLocalAppChat({ id: group.app?.id ?? '', name: group.name,
                    authoring: group.app?.authoring ?? { sourceDirectory: group.projectPath! },
                    authoringAction: 'createSession', authoringSetCurrent: Boolean(group.app) })}
                  onOpen={id => { void appAction(id, false); }} onEdit={id => { void appAction(id, true); }}
                  onChat={chat => { void openAppDevelopmentChat(chat); }} />
              : mode === 'bots'
              ? <BotsSidebar bots={bots} sessions={botSessions} bridge={bridge} query={search} chatVisible={page === 'chat'}
                  onOpenChat={showChat} onLeave={leaveBots} onOpenBot={openBot} onCreateBot={createBot} />
              : mode === 'friends'
              ? <FriendsPanel friends={friends} zh={zh} locale={locale} initialCode={friendTarget?.kind === 'add' ? friendTarget.code : undefined}
                  onCodeHandled={handledFriendCode} onShowFriends={showFriends} />
              : <TeamsSidebar teams={teams} zh={zh} query={search} initialInvite={teamInvite} onInviteHandled={handledTeamInvite}
                  onShowTeams={showTeams} onOpenSettings={openTeamDetails} />}
            sidebarCollapsed={sidebarCollapsed}
            onSidebarCollapsedChange={setSidebarCollapsed}
            scheduled={page === 'scheduled'}
            onOpenScheduled={() => openPage('scheduled')}
            onOpenChat={() => openPage('chat')}
            onOpenSettings={() => openSettings()}
            topbar={<>
              {page === 'scheduled' && <ScheduledTasksTopBar />}
              {page === 'apps' && <LocalAppsTopBar onOpenMarket={() => { void window.lingxi?.localApps?.marketBrowse().catch(error => setLocalAppNavigationError(error instanceof Error ? error.message : String(error))); }} />}
              <div style={{ display: page === 'chat' && (sidebarMode === 'home' || sidebarMode === 'apps') ? 'contents' : 'none' }}>
                <BetaTopBar
                  bridge={bridge}
                  app={appChat ? { name: appWorkspace.activeGroup?.name ?? bridge.activeSession!.projectPath.split(/[\\/]+/).at(-1) ?? '', onOpen: showAppLibrary } : undefined}
                  bot={activeBotId !== undefined ? { botId: activeBotId, name: activeBot?.name ?? '', onOpen: () => openBot(activeBotId) } : undefined}
                  sideChatOpen={sideChats.open && Boolean(sideChats.selected)}
                  sideChatAvailable={sideChats.sides.length > 0}
                  onToggleSideChat={() => { const next = !sideChats.open; sideChats.setOpen(next); if (!next) bridge.setRuntimeInspectorOpen(false); }}
                  terminalOpen={terminal.open}
                  terminalAvailable={Boolean(terminalProject && window.lingxi?.terminal)}
                  onToggleTerminal={terminal.toggle}
                  runtimeCenterOpen={bridge.runtimeCenter.overviewOpen}
                  onToggleRuntimeCenter={() => bridge.setRuntimeCenterOverviewOpen(!bridge.runtimeCenter.overviewOpen)}
                />
              </div>
            </>}
          >
          <div className="desktop-workspace">
          <div className="desktop-content" style={{ background: palette.stageBg }}>
          <div className="desktop-workspace-upper">
          <main className="desktop-main" style={{ position: 'relative', flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', background: palette.stageBg }}>
          {page === 'apps' && localAppInstallStatus && <p role="status" className="local-app-update">{localAppInstallStatus}</p>}
          <ErrorBanner bridge={{ error: localAppNavigationError, clearError: clearLocalAppNavigationError }} presentation={localAppNavigationPresentation} />
          <ErrorBanner bridge={bridge} />
          {page === 'teams' && <Suspense fallback={<p role="status" style={{ margin: 'auto' }}>{tr('desktop_teams_loading')}</p>}><TeamsMain teams={teams} friends={friends} zh={zh} locale={locale} drawer={teamsDrawer} /></Suspense>}
          {page === 'friends' && <Suspense fallback={<p role="status" style={{ margin: 'auto' }}>{tr('friends_loading')}</p>}><FriendsPage friends={friends} zh={zh} locale={locale} /></Suspense>}
          <div style={{ display: page === 'apps' ? 'contents' : 'none' }}><LocalApps sidebarTarget={pageSidebar} visible={page === 'apps'} onCreate={() => openLocalAppChat()} onDevelopmentBlocked={() => setLocalAppNavigationError(tr('local_app_development_requires_code'))} canDevelop={bridge.bootstrap?.conversationSpaces.activeMode === 'code' && !bridge.sessionLoading} /></div>
          {bots.available && botsApi && <div style={{ display: page === 'bots' ? 'contents' : 'none' }}>
            <Bots visible={page === 'bots'} bots={bots} api={botsApi} bridge={bridge}
              onAssist={openBotCreationChat}
              onOpenChat={showChat}
              onNewRoutine={(scope) => openRoutine({ kind: 'new', scope })}
              onOpenRoutine={(scope, id) => openRoutine({ kind: 'job', id, scope })} />
          </div>}
          <div style={{ display: page === 'scheduled' ? 'contents' : 'none' }}>
          {/* The run's chat is already open when this is called, as a Bot's chat is while it still reads as loading; the window must not wait for that to show it. */}
          <ScheduledTasks sidebarTarget={pageSidebar} initialTarget={scheduledTarget} bridge={bridge} visible={page === 'scheduled'} onOpenChat={() => { cancelLocalAppNavigation(); setPage('chat'); setActivityView(false); }} />
          </div>
          {page === 'changes' && <GitChangesPage />}
          <div style={{ display: page === 'chat' ? 'contents' : 'none' }}>
          <div className="desktop-chat-layout" data-runtime-inspector-open={bridge.runtimeCenter.inspectorOpen ? 'true' : undefined}>
            <div className="desktop-message-list-view">
                {/* The transcript keeps all remaining height; Todos live in Summary. */}
                <BotProposalAction.Provider value={botsAvailable ? reviewBotProposal : undefined}><Stage
                  messageTarget={!bridge.sessionLoading && (!messageRequest || bridge.conversation.sessionKey === messageRequest.sessionId) ? messageTarget : null}
                  onMessageLocated={onMessageLocated}
                  activityView={activityView}
                  onReviewFiles={(id, files, path) => bridge.openRuntimeItem({ kind: 'turn-review', id, files, path })}
                  submittedPlans={bridge.sessionLoading ? [] : planCalls}
                  onOpenPlan={(id) => bridge.openRuntimeItem({ kind: 'plan-document', id })}
                  liveItems={bridge.sessionLoading ? [] : bridge.conversation.items}
                  running={!bridge.sessionLoading && bridge.running}
                  apiRetry={bridge.sessionLoading ? null : bridge.desktop.apiRetry}
                  agents={stageAgents}
                  activeAgentId={bridge.runtimeCenter.inspectorOpen && bridge.runtimeCenter.activeItem?.kind === 'agent'
                    ? bridge.runtimeCenter.activeItem.id : undefined}
                  onOpenAgent={(agentId) => bridge.openRuntimeItem({ kind: 'agent', id: agentId })}
                  collapseThoughtsByDefault={bridge.bootstrap?.settings.collapseThoughtsByDefault ?? true}
                  emptyMessage={emptyMessage}
                  welcomeProject={botChat ? activeBot?.name ?? '' : bridge.activeSession?.projectId?.split(/[\\/]/).filter(Boolean).at(-1) ?? ''}
                  // Item ids restart at `i1` in every session; the Stage's
                  // collapse map is scoped by this and dropped when it changes.
                  sessionKey={bridge.conversation.sessionKey}
                  modUiSessionId={settledSession?.sessionId ?? ''}
                  foldedItemIds={bridge.sessionLoading ? [] : bridge.conversation.foldedItemIds}
                  onVisualizationFollowup={bridge.offerVisualizationFollowup}
                /></BotProposalAction.Provider>
                {/* Approval surfaces belong above the composer, never below the input. */}
                <ComputerAccessPrompt
                  request={bridge.pendingComputerAccess}
                  onSubmit={(requestId, response) => { void bridge.approveComputerAccess(requestId, response).catch(() => undefined); }}
                  onDeny={(requestId) => { void bridge.denyComputerAccess(requestId).catch(() => undefined); }}
                  onOpenSystemSettings={(pane) => { void bridge.openSystemSettings(pane).catch(() => undefined); }}
                />
                <PermissionPrompt
                  request={bridge.pendingPermission}
                  onApprove={(requestId, response) => { void bridge.approve(requestId, response).catch(() => undefined); }}
                  onDeny={(requestId) => { void bridge.deny(requestId).catch(() => undefined); }}
                />
                {askQuestion && <ModUiParentSite
                  sessionId={settledSession?.sessionId ?? ''}
                  surface="desktop"
                  component="AskUserQuestion"
                  instanceId={`ask-user-question:${askQuestion.request_id}`}
                  props={nativeAskUserQuestionProps(askQuestion)}
                  engineFallback={({ responseProps }) => <AskUserQuestionPrompt
                    request={askUserQuestionWithResponseProps(askQuestion, responseProps)}
                    onSubmit={(requestId, answers) => { void bridge.answerAskUserQuestion(requestId, answers).catch(() => undefined); }}
                    onCancel={(requestId) => { void bridge.cancelAskUserQuestion(requestId).catch(() => undefined); }}
                  />}
                >
                  <AskUserQuestionPrompt
                    request={askQuestion}
                    onSubmit={(requestId, answers) => { void bridge.answerAskUserQuestion(requestId, answers).catch(() => undefined); }}
                    onCancel={(requestId) => { void bridge.cancelAskUserQuestion(requestId).catch(() => undefined); }}
                  />
                </ModUiParentSite>}
                {(bridge.bootstrap?.runtimes ?? []).map((runtime) => <div
                  key={runtime.sessionId}
                  style={{ display: page === 'chat' && settingsRoute === null && !bridge.sessionLoading
                    && settledSession?.sessionId === runtime.sessionId ? 'contents' : 'none' }}
                >
                  <ModUiAbovePrompt sessionId={runtime.sessionId} enabled={runtime.connection.status === 'connected'} />
                </div>)}
                <BetaComposer
                  localAppDraft={localAppDraft}
                  onLocalAppDraftConsumed={consumeLocalAppDraft}
                  bridge={bridge}
                  ready={ready}
                  onCreateSide={sideChats.create}
                  onOpenSettings={() => {
                    // `/config` from the composer opens the same full-page
                    // surface the gear does, so it owes the same focus contract:
                    // capture the opener HERE, before `SettingsBackground`'s
                    // layout effect marks the tree `inert` and the browser has
                    // already moved focus to <body>. See the sidebar handler.
                    openSettings();
                  }}
                  onOpenSettingsPage={(pageId) => openSettings({ pageId })}
                  onSetTheme={changeTheme}
                  onOpenProviderSettings={(providerId, modelReference, restoreFocus) => {
                    if (!sessionNavigationBlocked()) setSettingsRoute({ pageId: 'provider-credentials', providerId, pendingModelReference: modelReference, restoreFocus });
                  }}
                />
                {!bridge.sessionLoading && visibleModToast && (
                  <div role="status" aria-live="polite" style={{ padding: '7px 16px',
                    color: palette.text, background: palette.stageBg, fontSize: 12 }}>
                    {visibleModToast.text}
                  </div>
                )}
                {!bridge.sessionLoading && bridge.conversation.modStatuses.map((status) => (
                  <div key={status.plugin} role="status" style={{ padding: '4px 16px',
                    color: palette.text, background: palette.stageBg, fontSize: 12 }}>
                    {status.text}
                  </div>
                ))}
            </div>
            {!bridge.sessionLoading && <RuntimeCenterOverview bridge={planBridge} sideChats={sideChats} onManageSources={() => openSettings({ pageId: 'mcp' })} />}
          </div>

          </div>

          </main>
          {page === 'chat' && !bridge.sessionLoading && <>
            <RuntimeCenterInspector bridge={planBridge} sideChats={sideChats} />
            {sideChats.open && sideChats.selected && <SideChatPanel controller={sideChats} />}
          </>}
          </div>
          <TerminalPanel controller={terminal} />
          </div>
          </div>
          </BetaSidebar>
        </SettingsBackground>

        {settingsRoute && (
          <SettingsBackground active={bridge.projectCreationOpen}>
            <SettingsScreen
              bridge={bridge}
              theme={theme}
              onTheme={changeTheme}
              linkRequest={settingsRoute.linkRequest}
              initialPageId={settingsRoute.pageId}
              initialProviderId={settingsRoute.providerId}
              pendingModelReference={settingsRoute.pendingModelReference}
              onClose={() => {
                const restoreFocus = settingsRoute.restoreFocus;
                setSettingsRoute(null);
                window.requestAnimationFrame(() => restoreFocus?.());
              }}
            />
          </SettingsBackground>
        )}
        {bridge.projectCreationOpen && <CreateProjectDialog localApp={bridge.projectCreationPurpose === 'local-app'}
          onDefaultFolder={bridge.defaultAppProjectFolder} onPickFolder={bridge.pickProjectFolder}
          onCreate={bridge.confirmProjectCreation} onClose={bridge.cancelProjectCreation} />}
        {sessionPicker && <SessionPicker key={sessionPicker.id} projectPath={sessionPicker.projectPath} mode={bridge.bootstrap?.conversationSpaces.activeMode ?? 'chat'} initialQuery={sessionPicker.query}
          suspended={sessionPickerSuspended}
          activeSessionId={bridge.activeSession?.sessionId} listProjectSessions={bridge.listProjectSessions} openSession={bridge.openSession}
          onOpened={openedPickerSession} onClose={closeSessionPicker} />}
      </div>
      </GitWorkspaceProvider>
      </ModUiSurfaceClientIdProvider>
      </AccountGate>
    </Theme.Provider>
  );
}

export function setSettingsBackgroundInert(
  element: Pick<HTMLElement, 'setAttribute' | 'removeAttribute'>,
  active: boolean,
): void {
  if (active) {
    element.setAttribute('inert', '');
    element.setAttribute('aria-hidden', 'true');
  } else {
    element.removeAttribute('inert');
    element.removeAttribute('aria-hidden');
  }
}

export function SettingsBackground({ active, children }: { active: boolean; children: ReactNode }) {
  const backgroundRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (backgroundRef.current) setSettingsBackgroundInert(backgroundRef.current, active);
  }, [active]);
  return <div ref={backgroundRef} aria-hidden={active ? 'true' : undefined} style={{ display: 'contents' }}>{children}</div>;
}

export interface SettingsRoute {
  pageId?: string;
  linkRequest?: SettingsLinkRequest;
  providerId?: string;
  pendingModelReference?: string;
  restoreFocus?: () => void;
}
