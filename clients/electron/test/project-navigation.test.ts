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
  clampSidebarWidth,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_MIN_WIDTH,
} from '../src/renderer/components/BetaDesktop';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { formatRelativeSessionTime, formatSessionMetadata } from '../src/renderer/bridge/sessionPresentation';

const projectPath = '/Users/tester/Projects/LingXi-Next';
const pinnedProjectPath = '/Users/tester/Projects/MLPlatform';

function bridgeFixture() {
  const sessions = Array.from({ length: 6 }, (_, index) => ({
    uuid: `00000000-0000-4000-8000-${String(index + 1).padStart(12, '0')}`,
    title: `Session ${index + 1}`,
    modified_rfc3339: `2026-08-${String(index + 1).padStart(2, '0')}T00:00:00Z`,
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

test('project rows omit the disclosure arrow and the active session uses the accent background', () => {
  const markup = renderToStaticMarkup(React.createElement(
    Theme.Provider,
    { value: tokens(true) },
    React.createElement(BetaSidebar, { bridge: bridgeFixture() as any, onOpenSettings: () => undefined }),
  ));
  const activeSessionTag = openingTag(markup, 'aria-current="page" title="Session 1"');
  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');

  assert.doesNotMatch(source, /\{active \? <Icon name="chevron"[^\n]+rotate/);
  assert.match(activeSessionTag, /background:oklch\(72% 0\.18 268 \/ 0\.14\)/);
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
  for (const marker of ['aria-label="Attach image"', 'aria-label="Search workspace files"', 'aria-label="Toggle goal mode"', 'aria-label="Start ordinary recording"']) {
    assert.doesNotMatch(openingTag(markup, marker), /\bdisabled\b/, `${marker} must remain interactive`);
  }
  assert.doesNotMatch(openingTag(markup, 'aria-label="Stop current turn"'), /\bdisabled\b/);
  assert.match(markup, /aria-label="Send pending message"/);

  const source = readFileSync(join(process.cwd(), 'src/renderer/components/BetaDesktop.tsx'), 'utf8');
  const submitStart = source.indexOf('const submit = async');
  const submitEnd = source.indexOf('const chooseSlashCommand', submitStart);
  assert.ok(submitStart >= 0 && submitEnd > submitStart);
  assert.doesNotMatch(source.slice(submitStart, submitEnd), /if \(!ready \|\| bridge\.running\) return/);
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

  assert.match(source, /\{ready && permissionOpen && \(/);
  assert.match(source, /\{ready && modelOpen && \(/);
  assert.match(
    source,
    /if \(ready\) return;\s+setPermissionOpen\(false\);\s+setModelOpen\(false\);\s+setModelSubmenu\(null\);/,
  );
});

test('session rows expose deterministic relative activity metadata', () => {
  const now = Date.parse('2026-08-26T12:00:00.000Z');
  assert.equal(formatRelativeSessionTime('2026-08-26T11:58:00.000Z', now), '2 minutes ago');
  assert.equal(formatRelativeSessionTime('not-a-date', now), 'just now');
  assert.equal(formatRelativeSessionTime('2026-08-26T12:05:00.000Z', now), 'just now');
  assert.equal(formatSessionMetadata('2026-08-26T11:58:00.000Z', 2, now), '2 minutes ago · 2 messages');
  assert.equal(formatSessionMetadata('2026-08-26T11:59:00.000Z', 1, now), '1 minute ago · 1 message');
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
