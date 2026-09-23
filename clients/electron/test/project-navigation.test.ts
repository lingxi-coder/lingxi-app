import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import assert from 'node:assert/strict';
import { test } from 'node:test';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';

(globalThis as { React?: typeof React }).React = React;

import { canResumePendingSession } from '../src/renderer/bridge/useBridge';
import {
  BetaComposer,
  BetaSidebar,
  BetaTopBar,
  ContextSummaryPanel,
  DictationRecorderBar,
  clampSidebarWidth,
  composerGoalActive,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
} from '../src/renderer/components/BetaDesktop';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { formatClockTime, formatRelativeSessionTime, formatSessionMetadata } from '../src/renderer/bridge/sessionPresentation';

const projectPath = '/Users/tester/Projects/LingXi-Next';
const pinnedProjectPath = '/Users/tester/Projects/MLPlatform';

function bridgeFixture() {
  // Keep the active session in the first five updated-sorted rows while
  // retaining a newer Session 5 than Session 4 for manual-order assertions.
  const sessionDays = [31, 2, 3, 4, 5, 1];
  const sessions = Array.from({ length: 6 }, (_, index) => ({
    uuid: `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
    title: `Session ${index + 1}`,
    modified_rfc3339: `2026-08-${String(sessionDays[index]).padStart(2, '0')}T00:00:00Z`,
    message_count: index + 1,
    path: `/tmp/session-${index + 1}.jsonl`,
  }));
  return {
    bootstrap: {
      settings: {
        version: 1,
        activeProject: projectPath,
        activeSession: { projectPath, sessionId: sessions[0]!.uuid },
        projects: [projectPath, pinnedProjectPath],
        pinnedSessions: [{
          projectPath: pinnedProjectPath,
          sessionId: '11111111-1111-4111-8111-111111111111',
          title: 'Pinned work',
          pinnedAt: '2026-08-26T00:00:00Z',
        }],
      },
      workspace: { path: projectPath, trusted: true },
      activeSession: { projectPath, sessionId: sessions[0]!.uuid },
      runtimes: [{ projectPath, sessionId: sessions[0]!.uuid, connection: { status: 'connected' }, turnActive: true, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
      projectCatalogs: {
        [projectPath]: { sessions },
        [pinnedProjectPath]: { sessions: [] },
      },
      connection: { status: 'connected' },
      diagnostics: [],
    },
    // Deliberately disagree with the legacy desktop snapshot: the sidebar must
    // use bootstrap.activeSession as the visible transcript identity.
    desktop: { sessions, activeSessionId: sessions[1]?.uuid },
    connection: { status: 'connected' },
    connected: true,
    running: false,
    newSession: async () => undefined,
    addProject: async () => null,
    activateProject: async () => ({ path: projectPath, trusted: true }),
    removeProject: async () => undefined,
    openSession: async () => undefined,
    setSessionPinned: async () => undefined,
    listProjectSessions: async () => undefined,
    sessionRuntimeStatus: (sessionId: string) => sessionId === sessions[0]!.uuid
      ? { connection: { status: 'connected' }, turnActive: true, pendingInteractions: 0, pendingAskUserQuestions: 0 }
      : undefined,
  };
}

function openingTag(markup: string, marker: string): string {
  const markerIndex = markup.indexOf(marker);
  assert.ok(markerIndex >= 0, `missing markup marker: ${marker}`);
  const start = markup.lastIndexOf('<button', markerIndex);
  const end = markup.indexOf('>', markerIndex);
  assert.ok(start >= 0 && end > markerIndex, `missing button for marker: ${marker}`);
  return markup.slice(start, end + 1);
}

function composerBridgeFixture() {
  const bridge = bridgeFixture();
  return {
    ...bridge,
    activeSession: bridge.bootstrap.activeSession,
    desktop: {
      ...bridge.desktop,
      models: [],
      modelDetails: [],
      currentModel: null,
      conversationControls: null,
      fastMode: false,
      permissionMode: 'default',
      slashCommands: [],
      tasks: {},
      taskOutput: {},
    },
    running: true,
    isCancelling: false,
    searchWorkspaceFiles: async () => ({ files: [], truncated: false }),
    setModel: async () => undefined,
    setReasoningSelection: async () => undefined,
    setFastMode: async () => undefined,
    setPermissionMode: async () => undefined,
    emitCommandOutput: () => undefined,
    beginLocalCommand: () => undefined,
    runSlashCommand: async () => undefined,
    sendPrompt: async () => undefined,
    cancel: async () => undefined,
  };
}

test('dictation recording bar exposes live amplitude with separate cancel and finish actions', () => {
  const audio = {
    request: async () => ({ type: 'cancelled', snapshot: {} }),
    onEvent: () => () => undefined,
    executeEngineRequest: async () => ({ type: 'ok' }),
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(DictationRecorderBar, {
      audio: audio as any,
      onCancel: () => undefined,
      onFinish: () => undefined,
    }),
  ));

  assert.match(markup, /aria-label="Cancel dictation"/);
  assert.match(markup, /aria-label="Live microphone amplitude"/);
  assert.match(markup, /aria-label="Stop dictation"/);
  assert.match(openingTag(markup, 'aria-label="Cancel dictation"'), /width:40px/);
  assert.match(openingTag(markup, 'aria-label="Stop dictation"'), /width:40px/);
});

test('desktop topbar keeps command and engine controls out of the chrome', () => {
  const bridge = {
    ...bridgeFixture(),
    usage: null,
    conversation: { sessionKey: 'session-a', summaries: [] },
    runtimeCenter: { inspectorOpen: false },
  };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaTopBar, {
      bridge: bridge as any,
      runtimeCenterOpen: false,
      onToggleRuntimeCenter: () => undefined,
    }),
  ));

  assert.doesNotMatch(markup, /More chat actions|desktop-topbar-more|git-topbar/);
  assert.match(markup, /aria-label="Toggle pinned summary"/);
  assert.match(markup, /aria-label="Toggle right panel"/);
  assert.doesNotMatch(markup, /aria-label="Toggle theme"|aria-label="Open context summaries"/);
  assert.doesNotMatch(markup, /Open command palette|Open session status|Engine ready|>Commands</);
  assert.doesNotMatch(openingTag(markup, 'aria-label="Toggle pinned summary"'), /background:/);

  const css = readFileSync(join(import.meta.dirname, '../src/renderer/global.css'), 'utf8');
  assert.match(css, /\.desktop-topbar-action\s*\{[^}]*background:\s*transparent/s);
  assert.match(css, /\.desktop-topbar-action:focus-visible\s*\{[^}]*background:/s);
});

test('context summary panel selects one saved item and expands its full detail', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(ContextSummaryPanel, {
      summaries: [
        { id: 'summary-1', content: '## Provider routing\n\nKeep the OpenRouter key bound to the session.', messagesBefore: 24, messagesAfter: 6, bytesSaved: 8192 },
        { id: 'summary-2', content: '## Topbar polish\n\nUse transparent icon controls.', messagesBefore: 18, messagesAfter: 4, bytesSaved: 4096 },
      ],
      selectedId: 'summary-1',
      onSelect: () => undefined,
      onClose: () => undefined,
    }),
  ));

  assert.match(markup, /role="listbox"/);
  assert.equal((markup.match(/role="option"/g) ?? []).length, 2);
  assert.match(markup, /aria-selected="true"/);
  assert.match(markup, /Context summary/);
  assert.match(markup, /Provider routing/);
  assert.match(markup, /Keep the OpenRouter key bound to the session/);
});

test('pending sessions resume only after the matching project is trusted and connected', () => {
  const pending = { projectPath, sessionId: '11111111-1111-4111-8111-111111111111' };
  assert.equal(canResumePendingSession(pending, { path: projectPath, trusted: true }, { status: 'connected' }), true);
  assert.equal(canResumePendingSession(pending, { path: projectPath, trusted: false }, { status: 'connected' }), false);
  assert.equal(canResumePendingSession(pending, { path: '/different', trusted: true }, { status: 'connected' }), false);
  assert.equal(canResumePendingSession(pending, { path: projectPath, trusted: true }, { status: 'restarting' }), false);
});

test('sidebar renders global pins before projects and limits each project sessions to five', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridgeFixture() as any, onOpenSettings: () => undefined }),
  ));

  assert.ok(markup.indexOf('Pinned work') < markup.indexOf('id="projects-heading"'));
  const footer = markup.slice(markup.indexOf('class="desktop-sidebar-footer"'), markup.indexOf('class="sidebar-resize-handle'));
  assert.match(footer, /> Settings<|>Settings<|> <!-- -->Settings</);
  assert.doesNotMatch(footer, /Engine|diagnostics|role="status"/);
  assert.match(markup, /MLPlatform/);
  assert.match(markup, /LingXi-Next/);
  assert.match(markup, /Session 5/);
  assert.doesNotMatch(markup, /Session 6/);
  assert.match(markup, /Show more \(1\)/);
  assert.match(markup, /aria-label="Unpin Pinned work"/);
  assert.match(markup, /aria-expanded="true"/);
  assert.match(markup, /aria-label="Running"/);
  assert.match(markup, /aria-current="page" title="Session 1"/);
  assert.match(markup, /disabled/); // project actions remain independently guarded by project activity
});

test('sidebar exposes a bounded drag handle for resizing', () => {
  assert.equal(clampSidebarWidth(SIDEBAR_MIN_WIDTH - 40), SIDEBAR_MIN_WIDTH);
  assert.equal(clampSidebarWidth(318), 318);
  assert.equal(clampSidebarWidth(SIDEBAR_MAX_WIDTH + 40), SIDEBAR_MAX_WIDTH);

  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridgeFixture() as any, onOpenSettings: () => undefined }),
  ));
  assert.match(markup, /role="separator"/);
  assert.match(markup, /aria-label="Resize sidebar"/);
  assert.match(markup, /aria-orientation="vertical"/);
  assert.match(markup, new RegExp(`aria-valuenow="${SIDEBAR_DEFAULT_WIDTH}"`));
  assert.match(markup, new RegExp(`aria-valuemin="${SIDEBAR_MIN_WIDTH}"`));
  assert.match(markup, new RegExp(`aria-valuemax="${SIDEBAR_MAX_WIDTH}"`));

  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  assert.match(source, /setPointerCapture\(event\.pointerId\)/);
  assert.match(source, /onPointerMove=\{resizeSidebar\}/);
});

test('project rows omit the disclosure arrow and the active session matches Settings selection styling', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridgeFixture() as any, onOpenSettings: () => undefined }),
  ));
  const activeSessionTag = openingTag(markup, 'aria-current="page" title="Session 1"');
  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');

  assert.doesNotMatch(source, /\{active \? <Icon name="chevron"[^\n]+rotate/);
  assert.ok(activeSessionTag.includes(`background:${tokens(true).surfaceActive}`));
});

test('a running session does not lock global project and session navigation', () => {
  const bridge = bridgeFixture();
  bridge.running = true;

  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridge as any, onOpenSettings: () => undefined }),
  ));

  for (const marker of [
    'class="sidebar-primary-action"',
    'title="Pinned work\n/Users/tester/Projects/MLPlatform"',
    'aria-label="Add project"',
    'title="Session 1"',
  ]) {
    assert.doesNotMatch(openingTag(markup, marker), /\bdisabled\b/, `${marker} must remain interactive`);
  }
});

test('a running turn keeps drafting and local composer controls interactive', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaComposer, {
      bridge: composerBridgeFixture() as any,
      ready: true,
      onOpenSettings: () => undefined,
      onSetTheme: () => undefined,
      onOpenProviderSettings: () => undefined,
    }),
  ));
  const promptMarker = 'aria-label="Prompt"';
  const markerIndex = markup.indexOf(promptMarker);
  const promptStart = markup.lastIndexOf('<div', markerIndex);
  const promptEnd = markup.indexOf('>', markerIndex);
  assert.ok(markerIndex >= 0 && promptStart >= 0 && promptEnd > markerIndex);
  const promptTag = markup.slice(promptStart, promptEnd + 1);

  assert.match(promptTag, /contentEditable="true"/i);
  assert.match(promptTag, /aria-disabled="false"/);
  for (const marker of ['aria-label="Attach files"', 'aria-label="Search workspace files"', 'aria-label="Start ordinary recording"']) {
    assert.doesNotMatch(openingTag(markup, marker), /\bdisabled\b/, `${marker} must remain interactive`);
  }
  assert.doesNotMatch(markup, /aria-label="Goal active"/);
  assert.doesNotMatch(openingTag(markup, 'aria-label="Stop current turn"'), /\bdisabled\b/);
  assert.match(markup, /class="composer-send-presence" data-visible="false" aria-hidden="true"/);
  assert.match(openingTag(markup, 'aria-label="Send pending message"'), /\bdisabled\b/);
  assert.match(openingTag(markup, 'aria-label="Send pending message"'), /tabindex="-1"/i);

  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  const submitStart = source.indexOf('const submit = async');
  const submitEnd = source.indexOf('const chooseSlashCommand', submitStart);
  assert.ok(submitStart >= 0 && submitEnd > submitStart);
  assert.doesNotMatch(source.slice(submitStart, submitEnd), /if \(!ready \|\| bridge\.running\) return/);
});

test('composer keeps its idle input compact and aligns the primary action controls', () => {
  const bridge = composerBridgeFixture();
  bridge.running = false;
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaComposer, {
      bridge: bridge as any,
      ready: true,
      onOpenSettings: () => undefined,
      onSetTheme: () => undefined,
      onOpenProviderSettings: () => undefined,
    }),
  ));
  const promptMarker = 'aria-label="Prompt"';
  const promptIndex = markup.indexOf(promptMarker);
  const promptStart = markup.lastIndexOf('<div', promptIndex);
  const promptEnd = markup.indexOf('>', promptIndex);
  assert.ok(promptIndex >= 0 && promptStart >= 0 && promptEnd > promptIndex);
  assert.match(markup.slice(promptStart, promptEnd + 1), /min-height:56px/);

  for (const marker of ['aria-label="Start ordinary recording"', 'aria-label="开启心流模式"', 'aria-label="Send prompt"']) {
    const tag = openingTag(markup, marker);
    assert.match(tag, /width:40px/);
    assert.match(tag, /height:40px/);
    assert.match(tag, /border-radius:50%/);
  }

  const microphoneTag = openingTag(markup, 'aria-label="Start ordinary recording"');
  const flowTag = openingTag(markup, 'aria-label="开启心流模式"');
  assert.match(microphoneTag, /background:transparent/);
  assert.match(flowTag, /background:transparent/, 'the flow icon must use the same transparent treatment as the microphone');
  assert.doesNotMatch(flowTag, /box-shadow/, 'the flow icon must not restore the old circular halo');

  assert.doesNotMatch(markup, /aria-label="Goal active"/);
  assert.doesNotMatch(markup, /Enter to send/);
  assert.doesNotMatch(markup, /Goal mode enabled/);
});

test('composer hides an inactive goal and shows only an active goal status', () => {
  const userGoal = { type: 'narration', id: 'g1', role: 'user', text: '/goal ship the desktop' } as const;
  const cleared = { type: 'command', id: 'g2', name: '/goal', output: 'Goal cleared: ship the desktop', isError: false } as const;
  assert.equal(composerGoalActive([]), false);
  assert.equal(composerGoalActive([userGoal]), true);
  assert.equal(composerGoalActive([userGoal, cleared]), false);

  const bridge = composerBridgeFixture();
  bridge.running = false;
  (bridge as any).conversation = { items: [userGoal] };
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaComposer, {
      bridge: bridge as any,
      ready: true,
      onOpenSettings: () => undefined,
      onSetTheme: () => undefined,
      onOpenProviderSettings: () => undefined,
    }),
  ));
  assert.match(markup, /role="status" aria-label="Goal active"/);
  assert.match(markup, />Goal<\/span>/);
  assert.doesNotMatch(markup, /Toggle goal mode/);
});

test('each project row exposes a focused edit action routed to that project draft', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridgeFixture() as any, onOpenSettings: () => undefined }),
  ));

  assert.match(markup, /aria-label="Edit LingXi-Next"/);
  assert.match(markup, /aria-label="Edit MLPlatform"/);

  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  assert.match(source, /bridge\.newSession\(projectPath\)/);
});

test('session navigation exposes immediate opening feedback instead of failing silently', () => {
  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  assert.match(source, /Opening session/);
  assert.match(source, /aria-busy=\{opening/);
  assert.match(source, /setOpeningSessionKey/);
});

test('opening a session preserves its cached renderer runtime until the host responds', () => {
  const source = readFileSync(join(process.cwd(), 'src/renderer/bridge/useBridge.ts'), 'utf8');
  const appSource = readFileSync(join(process.cwd(), 'src/renderer/App.tsx'), 'utf8');
  const start = source.indexOf('const openSession = useCallback');
  const end = source.indexOf('const sessionRuntimeStatus', start);
  assert.ok(start >= 0 && end > start);
  const openSessionSource = source.slice(start, end);

  assert.doesNotMatch(openSessionSource, /setRuntimeStates/);
  assert.doesNotMatch(openSessionSource, /turnActiveRefs\.current\.set/);
  assert.doesNotMatch(openSessionSource, /removeRuntimeFromMaps\(sessionId, cancellingRefs/);
  assert.match(appSource, /: bridge\.activeSession\s+\? 'This session has no messages yet/);
});

test('session menus are hidden and closed while the active session is loading', () => {
  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');

  const useBridgeSource = readFileSync(join(process.cwd(), 'src/renderer/bridge/useBridge.ts'), 'utf8');

  assert.match(source, /\{ready && permissionOpen && \(/);
  assert.match(source, /if \(ready\) return;\s+setPermissionOpen\(false\);/);

  // The model control is NOT on `ready` — `ready` also demands a configured
  // provider, and this control is the way to configure one. It still must not
  // outlive the session it commands, and `bridge.connected` is what carries
  // that: `useBridge` defines it to exclude a session that is still loading, so
  // the last assertion is the half of the chain this file cannot see.
  assert.match(
    source,
    /const modelControlReady = Boolean\(bridge\.hosted && !bridge\.loading && bridge\.connected && activeSessionId\);/,
  );
  assert.match(source, /\{modelControlReady && modelOpen && \(/);
  assert.match(
    source,
    /if \(modelControlReady\) return;\s+setModelOpen\(false\);\s+setModelSubmenu\(null\);/,
  );
  assert.match(useBridgeSource, /connected: !sessionLoading && connection\.status === 'connected',/);
});

test('session rows expose deterministic relative activity metadata', () => {
  const now = Date.parse('2026-08-26T12:00:00.000Z');
  assert.equal(formatRelativeSessionTime('2026-08-26T11:58:00.000Z', now), '2 minutes ago');
  assert.equal(formatRelativeSessionTime('not-a-date', now), 'just now');
  assert.equal(formatRelativeSessionTime('2026-08-26T12:05:00.000Z', now), 'just now');
  assert.equal(formatSessionMetadata('2026-08-26T11:58:00.000Z', 2, now), '2 minutes ago · 2 messages');
  assert.equal(formatSessionMetadata('2026-08-26T11:59:00.000Z', 1, now), '1 minute ago · 1 message');
});

test('message clocks render the 12-hour time of the local send instant', () => {
  // Built from LOCAL parts on purpose: the clock is a local wall time, so a
  // UTC literal would assert a different string in every timezone.
  const at = (hours: number, minutes: number) => new Date(2026, 7, 26, hours, minutes).getTime();
  assert.equal(formatClockTime(at(23, 35)), '11:35 PM');
  assert.equal(formatClockTime(at(0, 5)), '12:05 AM');
  assert.equal(formatClockTime(at(12, 0)), '12:00 PM');
  assert.equal(formatClockTime(at(9, 7)), '9:07 AM');
  assert.equal(formatClockTime(undefined), '');
  assert.equal(formatClockTime(Number.NaN), '');
});

test('sidebar renders activity metadata for normal and pinned sessions', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridgeFixture() as any, onOpenSettings: () => undefined }),
  ));
  assert.match(markup, /message/);
  assert.match(markup, /Pinned work/);
  assert.match(markup, /MLPlatform · Unavailable/);
  assert.doesNotMatch(markup, /MLPlatform · just now · 0 messages/);
  assert.match(readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8'), /formatSessionMetadata/);
});

test('sidebar sorting keeps new sessions first for every policy', () => {
  const bridge = bridgeFixture();
  const sessions = bridge.bootstrap.projectCatalogs[projectPath]!.sessions;
  const savedOrder = [sessions[1]!.uuid, sessions[0]!.uuid, sessions[2]!.uuid];
  const newSession = { ...sessions[4]!, path: '', modified_rfc3339: '2026-08-01T00:00:00Z' };
  const renderIds = (testBridge: typeof bridge) => {
    const markup = renderToStaticMarkup(React.createElement(
      Theme.Provider, { value: tokens(true) },
      React.createElement(BetaSidebar, { bridge: testBridge as any, onOpenSettings: () => undefined }),
    ));
    return [...markup.matchAll(/data-session-id="([^"]+)"/g)].map((match) => match[1]);
  };
  const makeBridge = (
    chatSort: 'priority' | 'updated' | 'manual',
    rows: typeof sessions,
    organization: 'project' | 'list' = 'project',
  ) => {
    const testBridge = bridgeFixture();
    Object.assign(testBridge.bootstrap.settings, {
      pinnedSessions: [],
      sidebar: { organization, chatSort, manualSessionOrder: { [projectPath]: savedOrder } },
    });
    testBridge.bootstrap.projectCatalogs[projectPath]!.sessions = rows;
    return testBridge;
  };
  for (const chatSort of ['priority', 'updated', 'manual'] as const) {
    const testBridge = makeBridge(chatSort, [newSession, ...sessions.slice(0, 3)]);
    assert.equal(renderIds(testBridge)[0], newSession.uuid, `${chatSort} should put a new session first`);
  }
  assert.equal(
    renderIds(makeBridge('updated', [newSession, ...sessions.slice(0, 3)], 'list'))[0],
    newSession.uuid,
    'the all-projects view should also put a new session first',
  );

  const manualBridge = makeBridge('manual', sessions.slice(0, 3));
  assert.deepEqual(renderIds(manualBridge), savedOrder);

  // Catalog refreshes need not arrive in activity order; both additions precede
  // the saved rows, newest first, even though an older saved row is active.
  manualBridge.bootstrap.projectCatalogs[projectPath]!.sessions.push(sessions[3]!, sessions[4]!);
  const refreshedMarkup = renderToStaticMarkup(React.createElement(
    Theme.Provider, { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: manualBridge as any, onOpenSettings: () => undefined }),
  ));
  assert.deepEqual(
    [...refreshedMarkup.matchAll(/data-session-id="([^"]+)"/g)].map((match) => match[1]),
    [sessions[4]!.uuid, sessions[3]!.uuid, ...savedOrder],
  );
  assert.deepEqual(savedOrder, [sessions[1]!.uuid, sessions[0]!.uuid, sessions[2]!.uuid]);
});

test('pinned metadata distinguishes loading from an unavailable catalog row', () => {
  const bridge = bridgeFixture();
  delete (bridge.bootstrap.projectCatalogs as Record<string, unknown>)[pinnedProjectPath];
  const loading = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridge as any, onOpenSettings: () => undefined }),
  ));
  assert.match(loading, /MLPlatform · Loading…/);

  (bridge.bootstrap.projectCatalogs as Record<string, unknown>)[pinnedProjectPath] = { sessions: [], error: 'missing' };
  const unavailable = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridge as any, onOpenSettings: () => undefined }),
  ));
  assert.match(unavailable, /MLPlatform · Unavailable/);
});

test('setup stays out of the transcript while project and provider controls remain available', () => {
  const desktopSource = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  const appSource = readFileSync(join(process.cwd(), 'src/renderer/App.tsx'), 'utf8');
  assert.doesNotMatch(desktopSource, /<SettingsSection title="Workspace">/);
  assert.doesNotMatch(desktopSource, /export function SetupCard/);
  assert.doesNotMatch(desktopSource, /Set up LingXi Code Beta/);
  assert.doesNotMatch(desktopSource, /Trust this project/);
  assert.doesNotMatch(desktopSource, /Complete setup/);
  assert.doesNotMatch(appSource, /<SetupCard/);
  assert.doesNotMatch(desktopSource, /setWorkspaceTrusted/);
  assert.doesNotMatch(readFileSync(join(process.cwd(), 'src/preload/index.ts'), 'utf8'), /lingxi:trust:set|setWorkspaceTrusted/);
  assert.doesNotMatch(readFileSync(join(process.cwd(), 'src/main/host.ts'), 'utf8'), /CH_TRUST_SET|lingxi:trust:set/);
  assert.match(desktopSource, /aria-label="Add project"/);
  // Provider controls moved out of `BetaDesktop.tsx`'s old settings modal
  // (retired in Task 20, cutting `<SettingsSection title="Providers">` from
  // this file entirely) into `ProviderCredentials.tsx`, reached through
  // `SettingsScreen`. The regression this test guards against — provider
  // controls disappearing along with `SetupCard` — is checked at their
  // current address instead of their old one.
  assert.match(
    readFileSync(join(process.cwd(), 'src/renderer/components/settings/pages/ProviderCredentials.tsx'), 'utf8'),
    /<Card title="Provider">/,
  );
});

test('session interaction prompts are scoped to the main panel instead of covering global navigation', () => {
  const source = readFileSync(join(process.cwd(), 'src/renderer/App.tsx'), 'utf8');
  const mainStart = source.indexOf('<main ');
  const mainEnd = source.indexOf('</main>');
  assert.ok(mainStart >= 0 && mainEnd > mainStart);
  for (const prompt of ['<PermissionPrompt', '<ComputerAccessPrompt', '<AskUserQuestionPrompt']) {
    const promptIndex = source.indexOf(prompt);
    assert.ok(promptIndex > mainStart && promptIndex < mainEnd, `${prompt} must stay inside the session panel`);
  }
});

test('sidebar uses indeterminate progress for main turns and background agents', () => {
  for (const turnActive of [true, false]) {
    const bridge = bridgeFixture();
    bridge.sessionRuntimeStatus = () => ({ connection: { status: 'connected' }, turnActive, backgroundAgentsRunning: !turnActive, pendingInteractions: 0, pendingAskUserQuestions: 0 });
    const markup = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(BetaSidebar, { bridge: bridge as any, onOpenSettings() {} })));
    assert.match(markup, /class="sidebar-session-progress" role="progressbar" aria-label="Running"/);
    assert.doesNotMatch(markup, /role="progressbar"[^>]*aria-valuenow/);
    assert.doesNotMatch(markup, /aria-label="Running" title="Running"/);
  }
});

test('topbar shows session totals instead of the latest usage update and restores saved totals', () => {
  const render = (cost: unknown, status: unknown = null, loading = false) => renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(BetaTopBar, {
    bridge: { ...bridgeFixture(), sessionLoading: loading, cost, desktop: { ...bridgeFixture().desktop, status }, usage: { inputTokens: 10, outputTokens: 5 }, conversation: { sessionKey: 'session-a', summaries: [] }, runtimeCenter: { inspectorOpen: false } } as any,
    runtimeCenterOpen: false, onToggleRuntimeCenter: () => undefined,
  })));
  const snapshot = { input_tokens: 10_000, output_tokens: 2_500 };
  assert.match(render(snapshot), new RegExp(`${(12_500).toLocaleString()} tok`));
  assert.doesNotMatch(render(snapshot), />15 tok/);
  assert.match(render(null, snapshot), new RegExp(`${(12_500).toLocaleString()} tok`));
  assert.match(render(snapshot, { input_tokens: 12_000, output_tokens: 3_000 }), new RegExp(`${(15_000).toLocaleString()} tok`));
  assert.doesNotMatch(render(null), /desktop-topbar-usage/);
  assert.doesNotMatch(render(snapshot, null, true), /desktop-topbar-usage/);
  assert.match(render(snapshot), /Session total: input \+ output tokens/);
});
