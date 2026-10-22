import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import ts from 'typescript';
import {
  beginProjectCatalogRequest,
  displayedSession,
  isLatestOperation,
  isLatestProjectCatalogRequest,
  nextOperationId,
  pruneRuntimeMaps,
  shouldApplyBootstrapSnapshot,
} from '../src/renderer/bridge/bridgeRuntimeState';
import { ALL_DESKTOP_COMMANDS } from '../src/renderer/bridge/desktopCommands';
import { desktopCommandIsShadowed, resolveDesktopCommand } from '../src/renderer/bridge/slashDispatch';
import { CH_SESSION_WORKSPACE, SESSION_PICKER_EVENT } from '../src/shared/sessionWorkspace';
import { PermissionPrompt } from '../src/renderer/components/PermissionPrompt';
import { AskUserQuestionPrompt } from '../src/renderer/components/AskUserQuestionPrompt';
import { ComputerAccessPrompt } from '../src/renderer/components/ComputerAccessPrompt';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

function source(path: string) {
  return ts.createSourceFile(path, readFileSync(resolve(path), 'utf8'), ts.ScriptTarget.Latest, true,
    path.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
}
const bridgeSource = source('src/renderer/bridge/useBridge.ts');
const composerSource = source('src/renderer/components/BetaDesktop.tsx');
const preloadSource = source('src/preload/index.ts');
const appSource = source('src/renderer/App.tsx');

function initializer(file: ts.SourceFile, name: string, property = false): string {
  let found: ts.Expression | undefined;
  const walk = (node: ts.Node): void => {
    const declaration = property
      ? (ts.isPropertyAssignment(node) ? node : undefined)
      : (ts.isVariableDeclaration(node) ? node : undefined);
    const isApiProperty = !property || (node.parent && ts.isObjectLiteralExpression(node.parent)
      && ts.isVariableDeclaration(node.parent.parent) && node.parent.parent.name.getText(file) === 'api');
    if (!found && isApiProperty && declaration?.name.getText(file) === name) found = declaration.initializer;
    ts.forEachChild(node, walk);
  };
  walk(file);
  assert.ok(found, `${name} must remain callable in ${file.fileName}`);
  return found.getText(file);
}

function evaluate(code: string, context: Record<string, unknown>): void {
  context.Error = Error;
  runInNewContext(ts.transpileModule(code, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.React,
  } }).outputText, context);
}

function appApprovalJSX(component: string): string {
  let found: ts.Node | undefined;
  const visit = (node: ts.Node): void => {
    if (!found && ts.isJsxSelfClosingElement(node) && node.tagName.getText(appSource) === component) found = node;
    if (!found && component === 'AskUserQuestionPrompt' && ts.isJsxExpression(node)
      && node.expression && ts.isBinaryExpression(node.expression)
      && node.expression.left.getText(appSource) === 'askQuestion') found = node.expression;
    ts.forEachChild(node, visit);
  };
  visit(appSource);
  assert.ok(found, `${component} must remain connected in App`);
  return found.getText(appSource);
}

test('App exposes pending resume permission, question and computer approvals while the target session is still loading', () => {
  const permission = { request_id: 7, kind: { type: 'tool_use_confirm', tool_name: 'Bash',
    tool_input_json: '{"command":"echo deferred"}', default_allow: false } };
  const question = { request_id: 9, questions: [{ question: 'Resume mode?', header: 'Mode',
    options: [{ label: 'Safe', description: 'Keep safeguards' }], multi_select: false }] };
  const computer = { request_id: 8, reason: 'Resume deferred automation', apps: [{ label: 'Terminal' }],
    tier: 'full', clipboard_read: false, clipboard_write: false, system_key_combos: false };
  const context: any = { React, PermissionPrompt, AskUserQuestionPrompt, ComputerAccessPrompt,
    ModUiParentSite: ({ children }: { children: React.ReactNode }) => children,
    nativeAskUserQuestionProps: (request: unknown) => request,
    askUserQuestionWithResponseProps: (request: unknown) => request,
    settledSession: { projectPath: '/target', sessionId: 'B' },
    bridge: { sessionLoading: true, sessionReady: false, pendingPermission: permission,
      pendingAskUserQuestion: question, pendingComputerAccess: computer },
  };
  evaluate('globalThis.askQuestion = ' + initializer(appSource, 'askQuestion'), context);
  assert.equal(context.askQuestion, question);
  (globalThis as { React?: typeof React }).React = React;
  for (const [component, text] of [
    ['PermissionPrompt', 'echo deferred'], ['AskUserQuestionPrompt', 'Resume mode?'], ['ComputerAccessPrompt', 'Terminal'],
  ]) {
    evaluate('globalThis.element = ' + appApprovalJSX(component!), context);
    const html = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, context.element));
    assert.ok(html.includes(text!), `${component} must render its pending target request before resume completes`);
  }
  Object.assign(context.bridge, { pendingPermission: null, pendingAskUserQuestion: null, pendingComputerAccess: null });
  evaluate('globalThis.askQuestion = ' + initializer(appSource, 'askQuestion'), context);
  for (const component of ['PermissionPrompt', 'AskUserQuestionPrompt', 'ComputerAccessPrompt']) {
    evaluate('globalThis.element = ' + appApprovalJSX(component), context);
    assert.equal(renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, context.element)), '');
  }
});

test('App suspends the picker and releases its background only while a loading target needs approval', () => {
  let backgroundActive: ts.Expression | undefined;
  const visit = (node: ts.Node): void => {
    if (ts.isJsxOpeningElement(node) && node.tagName.getText(appSource) === 'SettingsBackground') {
      const attribute = node.attributes.properties.find(property => ts.isJsxAttribute(property)
        && property.name.getText(appSource) === 'active');
      if (attribute && ts.isJsxAttribute(attribute) && attribute.initializer && ts.isJsxExpression(attribute.initializer)) {
        backgroundActive = attribute.initializer.expression;
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(appSource); assert.ok(backgroundActive);
  const context: any = { sessionPicker: { id: 1 }, settingsRoute: null,
    bridge: { sessionLoading: true, pendingPermission: null, pendingAskUserQuestion: null, pendingComputerAccess: null } };
  for (const request of ['pendingPermission', 'pendingAskUserQuestion', 'pendingComputerAccess']) {
    context.bridge[request] = { request_id: 7 };
    evaluate('globalThis.sessionPickerSuspended = ' + initializer(appSource, 'sessionPickerSuspended'), context);
    evaluate('globalThis.backgroundActive = ' + backgroundActive.getText(appSource), context);
    assert.equal(context.sessionPickerSuspended, true, request);
    assert.equal(context.backgroundActive, false, request);
    context.bridge[request] = null;
  }
  evaluate('globalThis.sessionPickerSuspended = ' + initializer(appSource, 'sessionPickerSuspended'), context);
  evaluate('globalThis.backgroundActive = ' + backgroundActive.getText(appSource), context);
  assert.equal(context.sessionPickerSuspended, false);
  assert.equal(context.backgroundActive, true, 'the in-flight picker resumes after its request is answered');
  context.bridge.sessionLoading = false;
  context.bridge.pendingPermission = { request_id: 7 };
  evaluate('globalThis.sessionPickerSuspended = ' + initializer(appSource, 'sessionPickerSuspended'), context);
  assert.equal(context.sessionPickerSuspended, false, 'ordinary active-session cards do not dismiss an unrelated picker');
});

test('pending resume approval callbacks route all decisions to the target runtime before sessionReady', async () => {
  const previous = { projectPath: '/old', sessionId: 'A' }, pending = { projectPath: '/target', sessionId: 'B' };
  const permission = { request_id: 7 }, computer = { request_id: 8 }, question = { request_id: 9 };
  const calls: unknown[] = [], acknowledged: unknown[] = [];
  const context: any = {
    useCallback: (fn: unknown) => fn,
    activeSessionId: displayedSession(previous, pending)!.sessionId, activeSessionIdRef: { current: 'B' },
    sessionReady: false, sessionLoading: true,
    permissionQueue: [permission], computerAccessQueue: [computer], askUserQuestionQueue: [question],
    runtimeStatesRef: { current: new Map([['B', { permissionQueue: [permission], computerAccessQueue: [computer], askUserQuestionQueue: [question] }]]) },
    removedRuntimeIds: { current: new Set() },
    acknowledgeInteraction: (...args: unknown[]) => acknowledged.push(args), updateRuntime() {},
    capture: (error: unknown) => { throw error; }, isPermissionRequestGone: () => false,
    host: Object.fromEntries(['approve', 'deny', 'approveComputerAccess', 'denyComputerAccess', 'answerAskUserQuestion', 'cancelAskUserQuestion']
      .map(name => [name, async (...args: unknown[]) => { calls.push([name, ...args]); }])),
  };
  const computerResponse = { granted_apps: ['Terminal'], clipboard_read: false, clipboard_write: false, system_key_combos: false };
  const answers = { 'Resume mode?': 'Safe' };
  for (const [name, args] of [
    ['approve', [7]], ['deny', [7]], ['approveComputerAccess', [8, computerResponse]],
    ['denyComputerAccess', [8]], ['answerAskUserQuestion', [9, answers]], ['cancelAskUserQuestion', [9]],
  ] as Array<[string, unknown[]]>) {
    evaluate('globalThis.decide = ' + initializer(bridgeSource, name), context);
    await context.decide(...args);
    assert.deepEqual(calls.at(-1), [name, 'B', ...args, ...(name === 'approve' ? [undefined] : [])]);
    assert.equal((acknowledged.at(-1) as unknown[])[0], 'B');
  }
  assert.equal(calls.length, 6);
  context.removedRuntimeIds.current.add('B');
  await context.decide(9);
  assert.equal(calls.length, 6, 'retired target callbacks cannot resolve a different runtime');
});

test('all hook navigation entry points retain the pending resume owner until its acknowledgement', async () => {
  const response = deferred<unknown>(), calls: unknown[] = [], snapshots: unknown[] = [];
  let visible: unknown;
  const context: any = {
    useCallback: (fn: unknown) => fn, nextOperationId,
    navigationOperationRef: { current: 0 }, pendingSessionRef: { current: null }, sessionLoadingRef: { current: false },
    removedRuntimeIds: { current: new Set() }, setSessionReady() {}, setError() {},
    setPendingSession: (session: unknown) => { visible = session; },
    bootstrap: { settings: { activeProject: '/old' } }, desktop: {},
    patchBootstrap() {}, applyBootstrap: (snapshot: unknown) => snapshots.push(snapshot),
    isCurrentNavigationOperation: (operation: number) => operation === context.navigationOperationRef.current,
    capture: (error: unknown) => { throw error; },
    host: { openSession: (...args: unknown[]) => { calls.push(['open', ...args]); return response.promise; },
      pickWorkspace: () => { calls.push('pick'); }, setWorkspace: () => { calls.push('activate'); },
      removeProject: () => { calls.push('remove'); }, archiveSession: () => { calls.push('archive'); },
      newSession: () => { calls.push('new'); }, bootstrap() {}, settings() {} },
  };
  evaluate('globalThis.beginNavigationOperation = ' + initializer(bridgeSource, 'beginNavigationOperation'), context);
  for (const callback of ['openSession', 'addProject', 'activateProject', 'removeProject', 'archiveSession', 'newSession']) {
    evaluate(`globalThis.${callback} = ` + initializer(bridgeSource, callback), context);
  }
  const opening = context.openSession('/target', 'A');
  const owner = context.pendingSessionRef.current;
  assert.equal(owner.sessionId, 'A'); assert.equal(visible, owner);
  for (const [callback, args] of [
    ['openSession', ['/other', 'B']], ['addProject', []], ['activateProject', ['/other']],
    ['removeProject', ['/target']], ['archiveSession', ['/target', 'A']],
  ] as Array<[string, string[]]>) {
    await assert.rejects(context[callback](...args), /finish opening before navigating/, callback);
    assert.equal(context.pendingSessionRef.current, owner, callback);
    assert.equal(visible, owner, callback);
    assert.equal(context.sessionLoadingRef.current, true, callback);
    assert.equal(context.navigationOperationRef.current, 1, callback);
  }
  assert.equal(await context.newSession('/other'), undefined);
  assert.deepEqual(calls, [['open', '/target', 'A']]);
  response.resolve({ revision: 5 }); await opening;
  assert.equal(context.pendingSessionRef.current, null);
  assert.equal(context.sessionLoadingRef.current, false);
  assert.deepEqual(snapshots, [{ revision: 5 }]);
  await context.openSession('/other', 'B');
  assert.deepEqual(calls, [['open', '/target', 'A'], ['open', '/other', 'B']]);
});

test('App page, settings, local app and picker navigation use the latest loading guard', () => {
  const calls: unknown[] = [];
  const context: any = {
    useCallback: (fn: unknown) => fn, localAppBridge: { current: { sessionLoading: false } },
    cancelLocalAppNavigation: () => calls.push('cancel-local'), setPage: (page: string) => calls.push(['page', page]),
    setSettingsRoute: (route: unknown) => calls.push(['settings', route]),
  };
  evaluate('globalThis.sessionNavigationBlocked = ' + initializer(appSource, 'sessionNavigationBlocked'), context);
  for (const callback of ['openPage', 'openSettings', 'openLocalAppChat', 'openPicker']) {
    evaluate(`globalThis.${callback} = ` + initializer(appSource, callback), context);
  }
  // Keep the callback closures from the earlier render; the current bridge
  // ref changes before a deferred host action can navigate the old page.
  context.localAppBridge.current = { sessionLoading: true };
  context.openPage('apps'); context.openPage('scheduled'); context.openPage('chat');
  context.openSettings(); context.openLocalAppChat(); context.openPicker({ detail: { query: 'B' } });
  assert.deepEqual(calls, []);
  context.localAppBridge.current = { sessionLoading: false };
  context.openPage('apps');
  assert.deepEqual(calls, ['cancel-local', ['page', 'apps']]);
});

test('sidebar capture and direct session or project callbacks cannot navigate during a resume', () => {
  const calls: unknown[] = [];
  const context: any = { useCallback: (fn: unknown) => fn,
    bridge: { sessionLoading: true, openSession: () => calls.push('open'), newSession: () => calls.push('new') },
    onOpenChat: () => calls.push('chat'), setOpeningSessionKey: () => calls.push('opening'),
    setExpandedProjects: () => calls.push('expand'), setEditingProject: () => calls.push('editing'),
  };
  for (const callback of ['blockNavigation', 'openSidebarSession', 'editProject']) {
    evaluate(`globalThis.${callback} = ` + initializer(composerSource, callback), context);
  }
  const event = { preventDefault: () => calls.push('prevent'), stopPropagation: () => calls.push('stop') };
  context.blockNavigation(event);
  context.openSidebarSession('/other', 'B'); context.editProject('/other');
  assert.deepEqual(calls, ['prevent', 'stop']);
  context.bridge.sessionLoading = false;
  context.blockNavigation(event);
  assert.deepEqual(calls, ['prevent', 'stop']);
});

test('sidebar navigation groups become inert during resume without locking the conversation and draft', () => {
  let effect: ts.Expression | undefined;
  const visit = (node: ts.Node): void => {
    if (ts.isCallExpression(node) && node.expression.getText(composerSource) === 'useLayoutEffect'
      && node.arguments[0]?.getText(composerSource).includes("asideRef.current?.toggleAttribute('inert'")) effect = node.arguments[0];
    ts.forEachChild(node, visit);
  };
  visit(composerSource); assert.ok(effect);
  const flags = new Map<string, boolean>();
  const ref = (name: string) => ({ current: { toggleAttribute: (attribute: string, active: boolean) => {
    assert.equal(attribute, 'inert'); flags.set(name, active);
  } } });
  const context: any = { bridge: { sessionLoading: true }, sidebarCollapsed: false,
    asideRef: ref('sidebar'), railRef: ref('rail'), toolbarRef: ref('toolbar') };
  evaluate('globalThis.lockNavigation = ' + effect.getText(composerSource), context);
  context.lockNavigation();
  assert.deepEqual([...flags], [['sidebar', true], ['rail', true], ['toolbar', true]]);
  context.bridge.sessionLoading = false; context.lockNavigation();
  assert.deepEqual([...flags], [['sidebar', false], ['rail', false], ['toolbar', false]]);
  context.sidebarCollapsed = true; context.lockNavigation();
  assert.deepEqual([...flags], [['sidebar', true], ['rail', false], ['toolbar', false]]);
});

test('a deep link fetched before resume admission cannot navigate after the target becomes pending', async () => {
  let handler: ts.Expression | undefined;
  const visit = (node: ts.Node): void => {
    if (ts.isBinaryExpression(node) && node.left.getText(appSource) === 'linkHandler.current') handler = node.right;
    ts.forEachChild(node, visit);
  };
  visit(appSource); assert.ok(handler);
  const delivery = deferred<unknown>(), calls: unknown[] = [];
  let blocked = false;
  const context: any = { bridge: { bootstrap: {}, sessionLoading: false },
    settingsRoute: null, sessionPicker: null, terminal: { hasCommandReview: false },
    linkSurfacePending: { current: false }, linkBusy: { current: false }, linkHandler: { current() {} },
    sessionNavigationBlocked: () => blocked, cancelLocalAppNavigation: () => calls.push('cancel-local'),
    window: { lingxi: { deepLinks: { next: () => { calls.push('next'); return delivery.promise; },
      acknowledge: async (...args: unknown[]) => calls.push(['acknowledge', ...args]) } } },
  };
  evaluate('globalThis.navigate = ' + handler.getText(appSource), context);
  context.navigate(); assert.deepEqual(calls, ['next']);
  blocked = true; context.bridge.sessionLoading = true;
  delivery.resolve({ id: 'linked-B', target: { route: 'session/B', params: {} }, projectPath: '/other' });
  await new Promise<void>(resolve => setImmediate(resolve));
  assert.deepEqual(calls, ['next', ['acknowledge', 'linked-B', 'Wait for the session to finish opening before navigating.']]);
  assert.equal(context.linkBusy.current, false);
  context.navigate();
  assert.equal(calls.length, 2, 'links arriving during pending resume remain queued instead of requesting a delivery');
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: Error) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function workspaceHarness() {
  const response = deferred<unknown>();
  const applied: unknown[] = [], errors: unknown[] = [], cleared: unknown[] = [], calls: unknown[] = [];
  const context: any = {
    useCallback: (fn: unknown) => fn,
    host: { updateSessionWorkspace: (...args: unknown[]) => { calls.push(args); return response.promise; } },
    activeSessionIdRef: { current: 'A' }, sessionLoadingRef: { current: false },
    navigationOperationRef: { current: 0 }, nextOperationId,
    isCurrentNavigationOperation: (id: number) => isLatestOperation(id, context.navigationOperationRef.current),
    applyBootstrap: (snapshot: unknown) => applied.push(snapshot),
    setError: (value: unknown) => cleared.push(value),
    capture: (cause: unknown) => { errors.push(cause); throw cause; },
  };
  evaluate('globalThis.update = ' + initializer(bridgeSource, 'updateSessionWorkspace'), context);
  return { context, response, applied, errors, cleared, calls };
}

test('the preload forwards workspace operations and catalog scope on their IPC channels', async () => {
  const calls: unknown[] = [];
  const context: any = {
    CH_SESSION_WORKSPACE, CH_PROJECT_SESSIONS_LIST: 'lingxi:project-sessions:list',
    ipcRenderer: { invoke: async (...args: unknown[]) => { calls.push(args); return null; } },
  };
  evaluate('globalThis.update = ' + initializer(preloadSource, 'updateSessionWorkspace', true), context);
  evaluate('globalThis.list = ' + initializer(preloadSource, 'listProjectSessions', true), context);
  const operation = { action: 'cd', path: '../next' };
  assert.equal(await context.update('A', operation), null);
  await context.list('/current', 'repository');
  assert.deepEqual(calls, [
    [CH_SESSION_WORKSPACE, 'A', operation],
    ['lingxi:project-sessions:list', '/current', 'repository'],
  ]);
});

test('preload preserves ordinary two-argument opens and transports exact picker selections unchanged', async () => {
  const calls: unknown[] = [];
  const context: any = { CH_SESSION_OPEN: 'lingxi:session:open',
    ipcRenderer: { invoke: async (...args: unknown[]) => { calls.push(args); return {}; } },
  };
  evaluate('globalThis.open = ' + initializer(preloadSource, 'openSession', true), context);
  await context.open('/current', 'A');
  const selection = { scope: 'repository', transcriptPath: '/other/selected.jsonl' };
  await context.open('/current', 'A', selection);
  assert.deepEqual(calls, [
    ['lingxi:session:open', '/current', 'A'], ['lingxi:session:open', '/current', 'A', selection],
  ]);
});

test('useBridge transports a picker selection while retaining the standard openSession behavior', async () => {
  const calls: unknown[] = [], snapshots: unknown[] = [];
  const snapshot = { revision: 1 };
  const context: any = {
    useCallback: (fn: unknown) => fn,
    host: { openSession: async (...args: unknown[]) => { calls.push(args); return snapshot; } },
    beginNavigationOperation: () => 1, pendingSessionRef: { current: null }, sessionLoadingRef: { current: false },
    removedRuntimeIds: { current: new Set() }, setSessionReady() {}, setPendingSession() {}, setError() {},
    isCurrentNavigationOperation: () => true, applyBootstrap: (value: unknown) => snapshots.push(value),
    capture: (cause: unknown) => { throw cause; },
  };
  evaluate('globalThis.open = ' + initializer(bridgeSource, 'openSession'), context);
  const selection = { scope: 'all', transcriptPath: '/other/chosen-copy.jsonl' };
  assert.equal(await context.open('/current', 'A', selection), true);
  assert.equal(await context.open('/current', 'B'), true);
  assert.deepEqual(calls, [['/current', 'A', selection], ['/current', 'B']]);
  assert.deepEqual(snapshots, [snapshot, snapshot]);
});

test('the ordinary sidebar opens the exact transcript from its displayed Current catalog', async () => {
  const response = deferred<boolean>(), calls: unknown[] = [], opening: unknown[] = [];
  let openedChat = 0, openingKey: string | null = null;
  const context: any = {
    useCallback: (fn: unknown) => fn,
    onOpenChat: () => { openedChat++; },
    setOpeningSessionKey: (next: string | null | ((previous: string | null) => string | null)) => {
      openingKey = typeof next === 'function' ? next(openingKey) : next; opening.push(openingKey);
    },
    bridge: { bootstrap: { projectCatalogs: {
      '/current-A': { sessions: [{ uuid: 'same-uuid', path: '/aliases/newer copy B/session.jsonl' }] },
      '/other': { sessions: [{ uuid: 'same-uuid', path: '/other/wrong-copy.jsonl' }] },
    } }, openSession: (...args: unknown[]) => { calls.push(structuredClone(args)); return response.promise; } },
  };
  evaluate('globalThis.open = ' + initializer(composerSource, 'openSidebarSession'), context);
  context.open('/current-A', 'same-uuid');
  assert.equal(openedChat, 1);
  assert.deepEqual(calls, [['/current-A', 'same-uuid', { scope: 'current', transcriptPath: '/aliases/newer copy B/session.jsonl' }]]);
  assert.equal(openingKey, '/current-A\0same-uuid');
  response.resolve(true); await new Promise<void>(resolve => setImmediate(resolve));
  assert.deepEqual(opening, ['/current-A\0same-uuid', null]);
});

test('pinned or archived sidebar rows absent from the Current catalog retain the exact two-argument UUID fallback', async () => {
  for (const bootstrap of [undefined, { projectCatalogs: {} }, { projectCatalogs: { '/current': { sessions: [{ uuid: 'another-id', path: '/another.jsonl' }] } } }]) {
    const calls: unknown[] = [];
    const context: any = {
      useCallback: (fn: unknown) => fn, onOpenChat: undefined, setOpeningSessionKey() {},
      bridge: { bootstrap, openSession: async (...args: unknown[]) => { calls.push(args); return true; } },
    };
    evaluate('globalThis.open = ' + initializer(composerSource, 'openSidebarSession'), context);
    context.open('/current', 'absent-id');
    await new Promise<void>(resolve => setImmediate(resolve));
    assert.deepEqual(calls, [['/current', 'absent-id']]);
  }
});

test('a refreshed Current catalog replaces the sidebar callback selection instead of retaining its previous copy', async () => {
  let cached: unknown, dependencies: unknown[] | undefined;
  const calls: unknown[] = [];
  const useCallback = (callback: unknown, nextDependencies: unknown[]) => {
    if (!dependencies || dependencies.some((previous, index) => !Object.is(previous, nextDependencies[index]))) {
      cached = callback; dependencies = nextDependencies;
    }
    return cached;
  };
  const openSession = async (...args: unknown[]) => { calls.push(structuredClone(args)); return true; };
  const onOpenChat = () => {};
  for (const path of ['/saved/old-A.jsonl', '/saved/fresh-B.jsonl']) {
    // Each render gets its own closure, while React's dependency cache persists.
    const context: any = { useCallback, onOpenChat, setOpeningSessionKey() {},
      bridge: { openSession, bootstrap: { projectCatalogs: { '/current': { sessions: [{ uuid: 'same-id', path }] } } } } };
    evaluate('globalThis.open = ' + initializer(composerSource, 'openSidebarSession'), context);
    context.open('/current', 'same-id');
    await new Promise<void>(resolve => setImmediate(resolve));
  }
  assert.deepEqual(calls, [
    ['/current', 'same-id', { scope: 'current', transcriptPath: '/saved/old-A.jsonl' }],
    ['/current', 'same-id', { scope: 'current', transcriptPath: '/saved/fresh-B.jsonl' }],
  ]);
});

test('canceling a native workspace picker preserves the existing UI and error', async () => {
  const h = workspaceHarness();
  const pending = h.context.update('A', { action: 'cd' });
  h.response.resolve(null);
  assert.equal(await pending, null);
  assert.deepEqual(h.applied, []);
  assert.deepEqual(h.cleared, []);
  assert.deepEqual(h.errors, []);
});

test('a successful workspace operation applies its authoritative bootstrap', async () => {
  const h = workspaceHarness(), snapshot = { revision: 7 };
  const operation = { action: 'add-dir', path: '/other' };
  const pending = h.context.update('A', operation);
  h.response.resolve(snapshot);
  assert.equal(await pending, snapshot);
  assert.deepEqual(h.calls, [['A', operation]]);
  assert.deepEqual(h.applied, [snapshot]);
  assert.deepEqual(h.cleared, [null]);
});

test('workspace results cannot replace a later navigation or a different active session', async () => {
  for (const move of ['navigation', 'session']) {
    const h = workspaceHarness();
    const pending = h.context.update('A', { action: 'cd', path: '/other' });
    if (move === 'navigation') h.context.navigationOperationRef.current += 1;
    else h.context.activeSessionIdRef.current = 'B';
    h.response.resolve({ revision: 8 });
    await pending;
    assert.deepEqual(h.applied, []);
    assert.deepEqual(h.cleared, []);
  }
});

test('workspace failures reject their caller and only capture the originating active session', async () => {
  for (const moved of [false, true]) {
    const h = workspaceHarness(), failure = new Error('Directory does not exist.');
    const pending = h.context.update('A', { action: 'cd', path: '/missing' });
    if (moved) h.context.activeSessionIdRef.current = 'B';
    h.response.reject(failure);
    await assert.rejects(pending, /Directory does not exist/);
    assert.deepEqual(h.errors, moved ? [] : [failure]);
    assert.deepEqual(h.applied, []);
  }
});

test('workspace changes require the current session and reject while navigation is loading', async () => {
  const missing = workspaceHarness();
  missing.context.activeSessionIdRef.current = null;
  await assert.rejects(missing.context.update('', { action: 'cd' }), /active session is required/);
  assert.deepEqual(missing.calls, []);
  const stale = workspaceHarness();
  await assert.rejects(stale.context.update('B', { action: 'add-dir' }), /active session changed/);
  assert.deepEqual(stale.calls, []);
  const loading = workspaceHarness();
  loading.context.sessionLoadingRef.current = true;
  await assert.rejects(loading.context.update('A', { action: 'cd' }), /finish opening/);
  assert.deepEqual(loading.calls, []);
});

test('expanded session scopes do not overwrite or supersede the default sidebar catalog', async () => {
  const current = deferred<unknown>(), all = deferred<unknown>();
  const calls: unknown[] = [];
  let bootstrap: any = { projectCatalogs: { '/project': { sessions: [{ uuid: 'old' }] } } };
  const context: any = {
    useCallback: (fn: unknown) => fn,
    host: { listProjectSessions: (path: string, scope: string) => {
      calls.push([path, scope]);
      return scope === 'current' ? current.promise : all.promise;
    } },
    catalogRequestGenerations: { current: new Map() },
    latestBootstrapRevisionRef: { current: 1 },
    beginProjectCatalogRequest, isLatestProjectCatalogRequest,
    setBootstrap: (update: (previous: unknown) => unknown) => { bootstrap = update(bootstrap); },
  };
  evaluate('globalThis.list = ' + initializer(bridgeSource, 'listProjectSessions'), context);
  const defaultList = context.list('/project'), expandedList = context.list('/project', 'all');
  all.resolve({ projectPath: '/project', sessions: [{ uuid: 'outside' }] });
  assert.equal((await expandedList).sessions[0].uuid, 'outside');
  assert.equal(bootstrap.projectCatalogs['/project'].sessions[0].uuid, 'old');
  current.resolve({ projectPath: '/project', sessions: [{ uuid: 'inside' }] });
  assert.equal((await defaultList).projectPath, '/project');
  assert.equal(bootstrap.projectCatalogs['/project'].sessions[0].uuid, 'inside');
  assert.deepEqual(calls, [['/project', 'current'], ['/project', 'all']]);
});

function catalogNavigationHarness() {
  const mutation = deferred<unknown>();
  const requests: Array<{ projectPath: string; scope: string; response: ReturnType<typeof deferred<any>> }> = [];
  const initial = { revision: 1, settings: { projects: ['/A', '/B'] }, activeSession: { projectPath: '/A', sessionId: 'chat-A' },
    projectCatalogs: { '/A': { sessions: [{ uuid: 'chat-A' }] }, '/B': { sessions: [] } } };
  let bootstrap: any = null, runtimes = new Map(), deferCatalogUpdates = false;
  const queuedCatalogUpdates: Array<(previous: unknown) => unknown> = [];
  const context: any = {
    useCallback: (fn: unknown) => fn,
    host: {
      listProjectSessions: (projectPath: string, scope: string) => {
        const response = deferred<any>(); requests.push({ projectPath, scope, response }); return response.promise;
      },
      updateSessionWorkspace: () => mutation.promise, removeProject: () => mutation.promise,
    },
    shouldApplyBootstrapSnapshot, pruneRuntimeMaps, beginProjectCatalogRequest, isLatestProjectCatalogRequest, nextOperationId,
    latestBootstrapRevisionRef: { current: null }, pendingSessionRef: { current: null }, sessionLoadingRef: { current: false },
    activeSessionIdRef: { current: 'chat-A' }, navigationOperationRef: { current: 0 },
    catalogRequestGenerations: { current: new Map() }, removedRuntimeIds: { current: new Set() },
    isCurrentNavigationOperation: (id: number) => isLatestOperation(id, context.navigationOperationRef.current),
    completeTrackedSpeech() {}, setSessionReady() {}, setPendingSession() {}, setError() {},
    capture: (cause: unknown) => { throw cause; },
    setRuntimeStates: (update: (previous: unknown) => Map<unknown, unknown>) => { runtimes = update(runtimes); },
    setBootstrap: (update: unknown) => {
      if (typeof update === 'function' && deferCatalogUpdates) { queuedCatalogUpdates.push(update as (previous: unknown) => unknown); return; }
      bootstrap = typeof update === 'function' ? update(bootstrap) : update;
    },
  };
  for (const name of ['turnActiveRefs', 'engineTurnActiveRefs', 'slashPendingRefs', 'pendingFusionDispatches',
    'sideQuestionTurns', 'cancellingRefs', 'cancellationTasks', 'pendingTrackedTurns', 'activeTrackedTurns', 'sharedDesktopCacheRef']) {
    context[name] = { current: new Map() };
  }
  for (const name of ['applyBootstrap', 'beginNavigationOperation', 'listProjectSessions', 'updateSessionWorkspace', 'removeProject']) {
    evaluate(`globalThis.${name} = ` + initializer(bridgeSource, name), context);
  }
  context.applyBootstrap(initial);
  return { context, requests, mutation, initial, bootstrap: () => bootstrap,
    deferCatalogUpdates: () => { deferCatalogUpdates = true; },
    flushCatalogUpdates: () => { for (const update of queuedCatalogUpdates.splice(0)) bootstrap = update(bootstrap); },
  };
}

for (const action of ['cd', 'remove-project']) {
  test(`an old Current response cannot re-add a catalog after an authoritative ${action} snapshot`, async () => {
    const h = catalogNavigationHarness();
    const oldCurrent = h.context.listProjectSessions('/A');
    const navigation = action === 'cd'
      ? h.context.updateSessionWorkspace('chat-A', { action: 'cd', path: '/B' })
      : h.context.removeProject('/A');
    const authoritative = { revision: 2, settings: { projects: action === 'cd' ? ['/A', '/B'] : ['/B'] },
      activeSession: { projectPath: '/B', sessionId: action === 'cd' ? 'chat-A' : 'chat-B' },
      projectCatalogs: { '/B': { sessions: [{ uuid: 'authoritative-B' }] } } };
    h.mutation.resolve(authoritative); await navigation;
    assert.equal(h.bootstrap(), authoritative);
    const freshCurrent = h.context.listProjectSessions('/B');
    h.requests[1]!.response.resolve({ projectPath: '/B', sessions: [{ uuid: 'fresh-B' }] });
    assert.equal((await freshCurrent).sessions[0].uuid, 'fresh-B');
    h.requests[0]!.response.resolve({ projectPath: '/A', sessions: [{ uuid: 'chat-A' }] });
    assert.equal(await oldCurrent, undefined);
    assert.equal(Object.hasOwn(h.bootstrap().projectCatalogs, '/A'), false);
    assert.equal(h.bootstrap().projectCatalogs['/B'].sessions[0].uuid, 'fresh-B');
  });
}

test('ordinary concurrent Current catalog results do not invalidate each other across projects', async () => {
  const h = catalogNavigationHarness(), first = h.context.listProjectSessions('/A'), second = h.context.listProjectSessions('/B');
  h.requests[0]!.response.resolve({ projectPath: '/A', sessions: [{ uuid: 'fresh-A' }] });
  assert.equal((await first).sessions[0].uuid, 'fresh-A');
  h.requests[1]!.response.resolve({ projectPath: '/B', sessions: [{ uuid: 'fresh-B' }] });
  assert.equal((await second).sessions[0].uuid, 'fresh-B');
  assert.equal(h.bootstrap().projectCatalogs['/A'].sessions[0].uuid, 'fresh-A');
  assert.equal(h.bootstrap().projectCatalogs['/B'].sessions[0].uuid, 'fresh-B');
  assert.equal(h.context.latestBootstrapRevisionRef.current, 1);
});

test('an ignored stale bootstrap and a canceled directory picker leave an in-flight Current request valid', async () => {
  const h = catalogNavigationHarness(), current = h.context.listProjectSessions('/A');
  h.context.applyBootstrap({ ...h.initial, revision: 0 });
  h.context.applyBootstrap({ ...h.initial, revision: 1 });
  const change = h.context.updateSessionWorkspace('chat-A', { action: 'cd' });
  h.mutation.resolve(null); assert.equal(await change, null);
  h.requests[0]!.response.resolve({ projectPath: '/A', sessions: [{ uuid: 'still-current-A' }] });
  assert.equal((await current).sessions[0].uuid, 'still-current-A');
  assert.equal(h.bootstrap().projectCatalogs['/A'].sessions[0].uuid, 'still-current-A');
});

test('a queued React catalog updater cannot merge old rows after an accepted bootstrap replaces the sidebar', async () => {
  const h = catalogNavigationHarness();
  h.deferCatalogUpdates();
  const current = h.context.listProjectSessions('/A');
  h.requests[0]!.response.resolve({ projectPath: '/A', sessions: [{ uuid: 'old-A' }] });
  assert.equal((await current).sessions[0].uuid, 'old-A');
  const authoritative = { revision: 2, settings: { projects: ['/B'] }, projectCatalogs: { '/B': { sessions: [{ uuid: 'new-B' }] } } };
  h.context.applyBootstrap(authoritative);
  h.flushCatalogUpdates();
  assert.equal(h.bootstrap(), authoritative);
  assert.equal(Object.hasOwn(h.bootstrap().projectCatalogs, '/A'), false);
});

function commandContextHarness(sessionId: string | null = 'A') {
  const calls: unknown[] = [], events: any[] = [];
  const context: any = {
    useMemo: (fn: () => unknown) => fn(), activeSessionId: sessionId,
    onOpenSettings: () => {}, onOpenSettingsPage: () => {}, onSetTheme: () => {},
    bridge: { updateSessionWorkspace: async (...args: unknown[]) => { calls.push(args); } },
    SESSION_PICKER_EVENT,
    window: { dispatchEvent: (event: unknown) => { events.push(event); } },
    CustomEvent: class { constructor(readonly type: string, readonly options: unknown) {} },
  };
  evaluate('globalThis.commandContext = ' + initializer(composerSource, 'commandContext'), context);
  return { context, calls, events };
}

test('Desktop workspace commands target the active session and reject a missing one', async () => {
  const h = commandContextHarness();
  await h.context.commandContext.updateSessionWorkspace({ action: 'add-dir', path: '/other' });
  assert.equal((h.calls[0] as any[])[0], 'A');
  const missing = commandContextHarness(null);
  for (const action of ['cd', 'add-dir']) {
    await assert.rejects(missing.context.commandContext.updateSessionWorkspace({ action }), new RegExp(`/${action} requires an active session`));
  }
  assert.deepEqual(missing.calls, []);
});

test('/resume opens the picker with its search query and starts no command on the current engine', () => {
  const h = commandContextHarness();
  h.context.commandContext.resumeSession('parser investigation');
  h.context.commandContext.resumeSession();
  assert.equal(h.events[0].type, SESSION_PICKER_EVENT);
  assert.equal(h.events[0].options.detail.query, 'parser investigation');
  assert.equal(h.events[1].options.detail.query, '');
  assert.deepEqual(h.calls, []);
});

test('the composer reports a local workspace failure against its original session', async () => {
  const output: unknown[] = [], notices: unknown[] = [];
  const context: any = {
    input: { current: {} }, text: '/cd /missing', selectedFiles: [], imageAttachments: [],
    richPromptSnapshot: () => ({ text: '/cd /missing', files: [] }),
    promptWithFileMentions: (value: string) => value,
    ready: true, flowModeRef: { current: false }, onCreateSide: undefined,
    sideCreationInput: () => null, fusionInvocation: () => null,
    activeSessionId: 'A', draftSessionId: { current: 'A' }, voiceState: 'idle',
    ALL_DESKTOP_COMMANDS, resolveDesktopCommand, desktopCommandIsShadowed,
    commandContext: { updateSessionWorkspace: async () => {
      context.draftSessionId.current = 'B';
      throw new Error('Directory does not exist.');
    } },
    clearComposer: () => {}, setImageNotice: (value: unknown) => notices.push(value),
    bridge: { running: false, desktop: { slashCommands: [] },
      beginLocalCommand: async () => {},
      emitCommandOutput: (...args: unknown[]) => output.push(args),
    },
  };
  evaluate('globalThis.submit = ' + initializer(composerSource, 'submit'), context);
  await context.submit();
  assert.deepEqual(output, [['Directory does not exist.', true, 'A']]);
  assert.deepEqual(notices, []);
});

test('a delayed local resume is canceled when its originating session changes before admission finishes', async () => {
  for (const nextSession of ['A', 'B', null]) {
    const admission = deferred<unknown>(), resumed: unknown[] = [], output: unknown[] = [];
    const context: any = {
      input: { current: {} }, text: '/resume parser', selectedFiles: [], imageAttachments: [],
      richPromptSnapshot: () => ({ text: '/resume parser', files: [] }), promptWithFileMentions: (value: string) => value,
      ready: true, flowModeRef: { current: false }, onCreateSide: undefined,
      sideCreationInput: () => null, fusionInvocation: () => null,
      activeSessionId: 'A', draftSessionId: { current: 'A' }, voiceState: 'idle',
      ALL_DESKTOP_COMMANDS, resolveDesktopCommand, desktopCommandIsShadowed,
      commandContext: { resumeSession: (query: string) => resumed.push(query) },
      clearComposer() {}, setImageNotice() {},
      bridge: { running: false, desktop: { slashCommands: [] }, beginLocalCommand: () => admission.promise,
        emitCommandOutput: (...args: unknown[]) => output.push(args) },
    };
    evaluate('globalThis.submit = ' + initializer(composerSource, 'submit'), context);
    const pending = context.submit();
    assert.deepEqual(resumed, []);
    context.activeSessionId = nextSession;
    context.draftSessionId.current = nextSession;
    admission.resolve(undefined);
    await pending;
    assert.deepEqual(resumed, nextSession === 'A' ? ['parser'] : []);
    assert.deepEqual(output, []);
  }
});

test('the composer keeps a loading-session draft and sends no prompt or local command when Enter submits it', async () => {
  const calls: unknown[] = [];
  for (const text of ['Draft while the resume needs approval', '/resume parser']) {
    const context: any = {
      input: { current: {} }, text, selectedFiles: [], imageAttachments: [], ready: false,
      richPromptSnapshot: () => ({ text, files: [] }), promptWithFileMentions: (value: string) => value,
      flowModeRef: { current: false }, clearComposer: () => calls.push('cleared'),
      bridge: { sendPrompt: (...args: unknown[]) => calls.push(['prompt', ...args]),
        beginLocalCommand: (...args: unknown[]) => calls.push(['command', ...args]) },
    };
    evaluate('globalThis.submit = ' + initializer(composerSource, 'submit'), context);
    await context.submit();
    assert.equal(context.text, text);
    assert.deepEqual(calls, []);
  }
});
