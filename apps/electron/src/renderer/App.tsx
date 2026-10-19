import type { TerminalScope } from '../shared/terminal';
import { settingsLinkRequest, runtimeLinkItem, type SettingsLinkRequest, type ScheduledLinkRequest } from './deepLinkRoutes';
import { SETTINGS_NAV } from './components/settings/nav';
import { useSideChats } from './bridge/useSideChats';
import { SideChatPanel } from './components/SideChatPanel';
import { conversationPlans } from './bridge/planDocuments';
import { selectedGitScope } from './bridge/gitScope';
import {
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
import { ScheduledTasks } from './components/ScheduledTasks';
import { Stage } from './components/Stage';
import { ModUiAbovePrompt, ModUiParentSite, ModUiSurfaceClientIdProvider } from './components/modUiAbovePrompt';
import { askUserQuestionWithResponseProps, nativeAskUserQuestionProps } from './components/nativeUiSiteProps';
import { CommandResultPanel } from './components/CommandResultPanel';
import './components/CommandResultPanel.css';
import { Theme } from './theme/ThemeContext';
import { tokens, watchThemePreference, type ThemeMode } from './theme/tokens';
import { RuntimeCenterInspector, RuntimeCenterOverview } from './components/RuntimeCenter';
import './components/RuntimeCenter.css';
import './components/GitReview.css';
import { GitWorkspaceProvider } from './components/GitReview';
import './components/TranscriptAgents.css';
import './components/TerminalPanel.css';
import { TerminalPanel, useTerminalPanel } from './components/TerminalPanel';

export function App() {
  const [messageTarget, setMessageTarget] = useState<string | null>(null);
  const [messageRequest, setMessageRequest] = useState<{ projectPath: string; sessionId: string; uuid: string } | null>(null);
  const messageCompletion = useRef<((found: boolean) => void) | null>(null);
  const [terminalLinkScope, setTerminalLinkScope] = useState<TerminalScope | null>(null);
  const [scheduledTarget, setScheduledTarget] = useState<ScheduledLinkRequest | undefined>();
  const [activityView, setActivityView] = useState(false);
  const [page, setPage] = useState<'chat' | 'scheduled'>('chat');
  const [theme, setTheme] = useState<ThemeMode>('dark');
  const [settingsRoute, setSettingsRoute] = useState<SettingsRoute | null>(null);
  const palette = useMemo(() => tokens(theme === 'dark'), [theme]);
  const bridge = useBridge();
  const [visibleModToast, setVisibleModToast] = useState<typeof bridge.conversation.modToast>(null);
  useEffect(() => {
    const toast = bridge.conversation.modToast;
    setVisibleModToast(toast);
    if (!toast) return;
    const timer = window.setTimeout(() => setVisibleModToast(null), toast.timeoutMs);
    return () => window.clearTimeout(timer);
  }, [bridge.conversation.modToast]);
  const toggleActivityView = () => {
    setPage('chat');
    setActivityView(!activityView);
    bridge.setRuntimeCenterOverviewOpen(!activityView);
  };
  useEffect(() => {
    const toggle = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.altKey && !event.shiftKey && event.key.toLowerCase() === 'u' && !settingsRoute) {
        event.preventDefault();
        setPage('chat');
        setActivityView(!activityView);
        bridge.setRuntimeCenterOverviewOpen(!activityView);
      }
    };
    window.addEventListener('keydown', toggle);
    return () => window.removeEventListener('keydown', toggle);
  }, [settingsRoute, activityView, bridge.setRuntimeCenterOverviewOpen]);
  const workspace = bridge.bootstrap?.workspace;
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
  const gitScope = selectedGitScope(bridge.bootstrap?.settings.activeProject, settledSession, workspace?.path);
  const terminalProject = terminalLinkScope?.projectPath ?? bridge.activeSession?.projectPath ?? workspace?.path;
  const terminal = useTerminalPanel(terminalLinkScope ?? (terminalProject ? { projectPath: terminalProject, sessionId: settledSession?.sessionId ?? '__draft__' } : null), page === 'chat' && settingsRoute === null);
  useEffect(() => { if (!terminal.hasCommandReview) setTerminalLinkScope(null); }, [settledSession?.sessionId, terminal.hasCommandReview]);
  const providerConfigured = Boolean(bridge.bootstrap?.providerCredentials?.some((entry) => entry.configured));
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
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setSettingsRoute({ ...route, restoreFocus: () => opener?.focus() });
  }, []);
  useEffect(() => window.lingxi?.onApplicationMenu?.((action) => {
    if (action === 'copy-session-link') {
      const active = bridge.activeSession;
      if (active) void window.lingxi?.deepLinks.copySessionLink(active.projectPath, active.sessionId).catch(() => undefined);
    } else if (action === 'settings') {
      // Preserve the original focus restoration target when already open.
      if (!settingsRoute) openSettings();
    } else if (action === 'activity' && !settingsRoute) {
      setPage('chat');
      setActivityView(!activityView);
      bridge.setRuntimeCenterOverviewOpen(!activityView);
    }
  }), [openSettings, settingsRoute, activityView, bridge.setRuntimeCenterOverviewOpen, bridge.activeSession]);

  const linkBusy = useRef(false);
  const linkSurfacePending = useRef(false);
  useEffect(() => { linkSurfacePending.current = Boolean(settingsRoute || terminal.hasCommandReview); }, [settingsRoute, terminal.hasCommandReview]);
  const linkHandler = useRef<() => void>(() => {});
  linkHandler.current = () => {
    const api = window.lingxi?.deepLinks;
    if (!api || !bridge.bootstrap || bridge.sessionLoading || settingsRoute || terminal.hasCommandReview || linkSurfacePending.current || linkBusy.current) return;
    linkBusy.current = true;
    let delivered = false;
    void (async () => {
      const delivery = await api.next();
      if (!delivery) return;
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
          const session = catalog?.sessions[0];
          if (!session) throw new Error('Open a local conversation in this project before opening its configuration.');
          if (!await bridge.openSession(delivery.projectPath, session.uuid)) throw new Error('Unable to open the linked project’s local conversation.');
        }
        if (settingsTarget) { linkSurfacePending.current = true; openSettings(settingsTarget); }
        else if (kind === 'settings') {
          if (!SETTINGS_NAV.some(page => page.id === id)) throw new Error('This settings page is unavailable.');
          linkSurfacePending.current = true;
          openSettings({ pageId: id });
        } else if (kind === 'provider') { linkSurfacePending.current = true; openSettings({ pageId: 'provider-credentials', providerId: id }); }
        else if (kind === 'session' || kind === 'project') {
          setMessageTarget(null);
          if (!delivery.projectPath) throw new Error('This project is not registered on this device.');
          if (kind === 'session' && !await bridge.openSession(delivery.projectPath, id!)) throw new Error('Unable to open this local conversation.');
          if (kind === 'project') await bridge.activateProject(delivery.projectPath);
          setPage('chat');
          setActivityView(false);
          if (params.message) {
            await new Promise<void>((resolve, reject) => {
              messageCompletion.current = found => { messageCompletion.current = null; found ? resolve() : reject(new Error('This message is unavailable in the local conversation.')); };
              setMessageRequest({ projectPath: delivery.projectPath!, sessionId: id!, uuid: params.message! });
            });
          }
        } else if (kind === 'new') {
          const project = delivery.projectPath ?? bridge.bootstrap?.settings.activeProject;
          if (!project) throw new Error('Select a local project before creating a conversation.');
          const session = await bridge.newSession(project);
          if (!session) throw new Error('Unable to create conversation.');
          if (typeof delivery.target.payload?.prompt === 'string') bridge.offerLinkDraft({ sessionId: session.sessionId, text: delivery.target.payload.prompt, requestId: delivery.id });
          setPage('chat');
        } else if (kind === 'terminal') {
          const projectPath = delivery.projectPath ?? bridge.activeSession?.projectPath ?? bridge.bootstrap?.settings.activeProject;
          if (!projectPath) throw new Error('Select a local project before opening a terminal.');
          if (projectPath !== bridge.activeSession?.projectPath) await bridge.activateProject(projectPath);
          setPage('chat');
          setActivityView(false);
          const scope = { projectPath, sessionId: params.scope ?? (bridge.activeSession?.projectPath === projectPath ? settledSession?.sessionId ?? '__draft__' : '__draft__') };
          setTerminalLinkScope(scope);
          if (typeof delivery.target.payload?.command === 'string') linkSurfacePending.current = true;
          await terminal.openLink(scope, id!, typeof delivery.target.payload?.command === 'string' ? delivery.target.payload.command : undefined);
        } else if (kind === 'cron') {
          setScheduledTarget({ requestId: delivery.id, kind: id as 'job' | 'run', id: detailId!, scope: delivery.projectPath ?? params.scope });
          setPage('scheduled');
        } else if (kind === 'runtime') {
          if (delivery.projectPath && delivery.projectPath !== bridge.activeSession?.projectPath) throw new Error('Open this local project’s conversation before opening its runtime details.');
          const item = runtimeLinkItem(id!, detailId!, bridge, planCalls);
          setPage('chat');
          if (item === 'activity') { setActivityView(true); bridge.setRuntimeCenterOverviewOpen(true); }
          else { setActivityView(false); bridge.openRuntimeItem(item); }
        } else throw new Error('This destination is not available in Desktop.');
      } catch (cause) { linkSurfacePending.current = false; error = cause instanceof Error ? cause.message : 'Unable to open destination'; }
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
  }, [bridge.bootstrap, bridge.sessionLoading, settingsRoute, terminal.hasCommandReview]);
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
    ? 'Loading session…'
    : !workspace?.path
    ? 'Add a project from the sidebar to start a session.'
    : bridge.activeSession
      ? 'This session has no messages yet. Ask LingXi to inspect the project.'
      : 'Create a session and ask LingXi to inspect, explain, or change this project.';
  const askQuestion = bridge.sessionLoading ? null : bridge.pendingAskUserQuestion;

  return (
    <Theme.Provider value={palette}>
      <ModUiSurfaceClientIdProvider clientId={bridge.uiSurfaceClientId}>
      <GitWorkspaceProvider scope={gitScope} onOpen={() => bridge.openRuntimeItem({ kind: 'section', id: 'review' })} onTerminal={() => { if (!terminal.open) terminal.toggle(); }}>
      <div
        className="desktop-shell"
        data-screen-label="LingXi Code Desktop Beta"
        style={{ width: '100vw', height: '100vh', overflow: 'hidden', display: 'flex', position: 'relative', background: palette.sidebarBg, color: palette.text, colorScheme: theme, '--desktop-accent': palette.accent, '--rail-material': palette.sidebarBg, '--nav-panel-border': palette.border } as CSSProperties}
      >
        <SettingsBackground active={settingsRoute !== null}>
          <BetaSidebar
            bridge={bridge}
            activityView={activityView}
            onToggleActivityView={toggleActivityView}
            scheduled={page === 'scheduled'}
            onOpenScheduled={() => setPage('scheduled')}
            onOpenChat={() => setPage('chat')}
            onOpenSettings={() => openSettings()}
            topbar={
              <div style={{ display: page === 'chat' ? 'contents' : 'none' }}>
                <BetaTopBar
                  bridge={bridge}
                  sideChatOpen={sideChats.open && Boolean(sideChats.selected)}
                  sideChatAvailable={sideChats.sides.length > 0}
                  onToggleSideChat={() => sideChats.setOpen(!sideChats.open)}
                  terminalOpen={terminal.open}
                  terminalAvailable={Boolean(terminalProject && window.lingxi?.terminal)}
                  onToggleTerminal={terminal.toggle}
                  runtimeCenterOpen={bridge.runtimeCenter.overviewOpen}
                  onToggleRuntimeCenter={() => bridge.setRuntimeCenterOverviewOpen(!bridge.runtimeCenter.overviewOpen)}
                />
              </div>
            }
          >
          <div className="desktop-workspace">
          <div className="desktop-content" style={{ background: palette.stageBg }}>
          <div className="desktop-workspace-upper">
          <main className="desktop-main" style={{ position: 'relative', flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', background: palette.stageBg }}>
          <div style={{ display: page === 'scheduled' ? 'contents' : 'none' }}>
          <ScheduledTasks initialTarget={scheduledTarget} bridge={bridge} visible={page === 'scheduled'} onOpenChat={() => setPage('chat')} />
          </div>
          <div style={{ display: page === 'chat' ? 'contents' : 'none' }}>
          <ErrorBanner bridge={bridge} />
          <div className="desktop-chat-layout" data-runtime-inspector-open={bridge.runtimeCenter.inspectorOpen ? 'true' : undefined}>
            <div className="desktop-message-list-view">
                {/* The transcript keeps all remaining height; Todos live in Summary. */}
                <Stage
                  messageTarget={!bridge.sessionLoading && (!messageRequest || bridge.conversation.sessionKey === messageRequest.sessionId) ? messageTarget : null}
                  onMessageLocated={onMessageLocated}
                  onCopyMessage={(uuid) => {
                    const active = bridge.activeSession;
                    if (active) void window.lingxi?.deepLinks.copySessionLink(active.projectPath, active.sessionId, uuid).catch(() => undefined);
                  }}
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
                  welcomeProject={(bridge.activeSession?.projectPath ?? bridge.bootstrap?.settings.activeProject ?? workspace?.path)?.split(/[\\/]/).filter(Boolean).at(-1) ?? ''}
                  // Item ids restart at `i1` in every session; the Stage's
                  // collapse map is scoped by this and dropped when it changes.
                  sessionKey={bridge.conversation.sessionKey}
                  modUiSessionId={settledSession?.sessionId ?? ''}
                  foldedItemIds={bridge.sessionLoading ? [] : bridge.conversation.foldedItemIds}
                  onVisualizationFollowup={bridge.offerVisualizationFollowup}
                />
                <PermissionPrompt
                  request={bridge.sessionLoading ? null : bridge.pendingPermission}
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
                <ModUiAbovePrompt
                  sessionId={settledSession?.sessionId ?? ''}
                  enabled={page === 'chat' && settingsRoute === null && bridge.connected && !bridge.sessionLoading}
                />
                <BetaComposer
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
                  onOpenProviderSettings={(providerId, modelReference, restoreFocus) => setSettingsRoute({ pageId: 'provider-credentials', providerId, pendingModelReference: modelReference, restoreFocus })}
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
            {!bridge.sessionLoading && <RuntimeCenterOverview bridge={planBridge} />}
          </div>

          </div>

          {page === 'chat' && !settingsRoute && !bridge.sessionLoading && bridge.conversation.commandResult && (
            <CommandResultPanel
              key={`${bridge.conversation.sessionKey}:${bridge.conversation.commandResult.id}`}
              item={bridge.conversation.commandResult}
              onClose={bridge.dismissCommandResult}
            />
          )}
          <ComputerAccessPrompt
            request={bridge.sessionLoading ? null : bridge.pendingComputerAccess}
            onSubmit={(requestId, response) => { void bridge.approveComputerAccess(requestId, response).catch(() => undefined); }}
            onDeny={(requestId) => { void bridge.denyComputerAccess(requestId).catch(() => undefined); }}
            onOpenSystemSettings={(pane) => { void bridge.openSystemSettings(pane).catch(() => undefined); }}
          />

          </main>
          {page === 'chat' && !bridge.sessionLoading && (sideChats.open && sideChats.selected ? <SideChatPanel controller={sideChats} /> : <RuntimeCenterInspector bridge={planBridge} />)}
          </div>
          <TerminalPanel controller={terminal} />
          </div>
          </div>
          </BetaSidebar>
        </SettingsBackground>

        {settingsRoute && (
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
        )}
      </div>
      </GitWorkspaceProvider>
      </ModUiSurfaceClientIdProvider>
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
