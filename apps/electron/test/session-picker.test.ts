import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import ts from 'typescript';
import type { SessionCatalogRow } from '../src/shared/sessionWorkspace';
import {
  restoreSessionPickerFocus,
  resumeArgumentRow,
  resumeArgumentSessionId,
  SessionPicker,
  sessionPickerRows,
  sessionPickerShortcutScope,
} from '../src/renderer/components/SessionPicker';
import { dialogFocusTarget } from '../src/renderer/bridge/bridgeRuntimeState';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;
const firstId = '11111111-1111-4111-8111-111111111111';
const secondId = '22222222-2222-4222-8222-222222222222';
const row = (uuid: string, title: string, modified = '2026-10-21T00:00:00Z'): SessionCatalogRow => ({
  uuid, title, resume_title: title, modified_rfc3339: modified, message_count: 3, mode: 'code', path: `/saved/${uuid}.jsonl`,
});
const sessions = [row(firstId, 'Parser investigation', '2026-10-20T00:00:00Z'), row(secondId, 'Review tests')];

test('a resume argument selects a UUID or one normalized resume title and preserves ambiguous names for search', () => {
  assert.equal(resumeArgumentSessionId(` ${firstId} `), firstId);
  assert.equal(resumeArgumentSessionId('Parser investigation', sessions), firstId);
  assert.equal(resumeArgumentSessionId('  parser INVESTIGATION  ', sessions), firstId);
  assert.equal(resumeArgumentSessionId('Parser', sessions), undefined);
  assert.equal(resumeArgumentSessionId('Parser investigation', [...sessions, row(secondId, 'Parser investigation')]), undefined);
  assert.equal(resumeArgumentSessionId('', sessions), undefined);
});

test('display titles and untitled first prompts never substitute for the raw resume title', () => {
  const named = { ...row(firstId, 'Agent summary'), resume_title: '  Full custom TITLE  ' };
  const untitled = { ...row(secondId, 'Investigate the first prompt'), resume_title: undefined };
  assert.equal(resumeArgumentRow('full custom title', [named]), named);
  assert.equal(resumeArgumentSessionId('Agent summary', [named]), undefined);
  assert.equal(resumeArgumentSessionId('Investigate the first prompt', [untitled]), undefined);
  assert.equal(sessionPickerRows([untitled], 'Investigate the first prompt')[0], untitled);
});

test('picker search matches title, UUID and saved path and orders recent chats without mutating the catalog', () => {
  assert.deepEqual(sessionPickerRows(sessions, '').map(({ uuid }) => uuid), [secondId, firstId]);
  assert.equal(sessionPickerRows(sessions, 'PARSER investigation')[0]?.uuid, firstId);
  assert.equal(sessionPickerRows(sessions, secondId.slice(0, 8))[0]?.uuid, secondId);
  assert.equal(sessionPickerRows(sessions, `/saved/${firstId}`)[0]?.uuid, firstId);
  assert.deepEqual(sessionPickerRows(sessions, 'absent'), []);
  assert.equal(sessions[0]?.uuid, firstId);
});

test('Ctrl A returns from All to the previous local scope and Ctrl W has no inferred native behavior', () => {
  assert.equal(sessionPickerShortcutScope('w', 'current', 'current'), undefined);
  assert.equal(sessionPickerShortcutScope('w', 'repository', 'repository'), undefined);
  assert.equal(sessionPickerShortcutScope('a', 'repository', 'repository'), 'all');
  assert.equal(sessionPickerShortcutScope('a', 'all', 'repository'), 'repository');
  assert.equal(sessionPickerShortcutScope('a', 'all', 'current'), 'current');
  assert.equal(sessionPickerShortcutScope('x', 'current', 'current'), undefined);
});

const pickerSource = ts.createSourceFile('SessionPicker.tsx', readFileSync(resolve('src/renderer/components/SessionPicker.tsx'), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const appSource = ts.createSourceFile('App.tsx', readFileSync(resolve('src/renderer/App.tsx'), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const dialogSource = ts.createSourceFile('DesktopDialog.tsx', readFileSync(resolve('src/renderer/components/DesktopDialog.tsx'), 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);

function initializer(file: ts.SourceFile, name: string): string {
  let found: ts.Expression | undefined;
  const walk = (node: ts.Node): void => {
    if (!found && ts.isVariableDeclaration(node) && node.name.getText(file) === name) found = node.initializer;
    ts.forEachChild(node, walk);
  };
  walk(file);
  assert.ok(found, name);
  return found.getText(file);
}
function evaluate(code: string, context: Record<string, unknown>): void {
  context.Error = Error;
  runInNewContext(ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText, context);
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}
function pickerHarness(query = '') {
  const state: any = { error: '', loading: true, loaded: false, sessions: [], opened: 0, closed: 0 };
  const listed: unknown[] = [], resumed: unknown[] = [];
  const context: any = {
    useCallback: (fn: unknown) => fn, mounted: { current: true }, opening: { current: false },
    requestGeneration: { current: 0 }, firstRequest: { current: true }, initialArgument: { current: query },
    projectPath: '/current', scope: 'repository', resumeArgumentSessionId, resumeArgumentRow,
    listProjectSessions: async (...args: unknown[]) => { listed.push(args); return { projectPath: '/current', sessions }; },
    openSession: async (...args: unknown[]) => { resumed.push(structuredClone(args)); return true; },
    onOpened: () => { state.opened++; }, onClose: () => { state.closed++; },
  };
  for (const key of ['error', 'loading', 'loaded', 'sessions', 'selectedPath', 'openingUuid']) {
    context[`set${key[0]!.toUpperCase()}${key.slice(1)}`] = (value: unknown) => { state[key] = value; };
  }
  evaluate('globalThis.openChat = ' + initializer(pickerSource, 'openChat'), context);
  evaluate('globalThis.loadSessions = ' + initializer(pickerSource, 'loadSessions'), context);
  return { context, state, listed, resumed };
}

test('UUID resume reads Repository metadata first and opens the newest matching transcript copy', async () => {
  const h = pickerHarness(firstId), response = deferred<unknown>();
  const old = { ...row(firstId, 'Old copy', '2026-10-20T00:00:00Z'), path: '/old/session.jsonl' };
  const newest = { ...row(firstId, 'New copy'), path: '/new/session.jsonl' };
  h.context.listProjectSessions = async (...args: unknown[]) => { h.listed.push(args); return response.promise; };
  const pending = h.context.loadSessions();
  assert.deepEqual(h.resumed, [], 'UUID arguments must not use the CLI-flag opening path before metadata resolves');
  response.resolve({ sessions: [old, newest] }); await pending;
  assert.deepEqual(h.listed, [['/current', 'repository']]);
  assert.deepEqual(h.resumed, [['/current', firstId, { scope: 'repository', transcriptPath: newest.path }]]);
  assert.equal(h.state.closed, 1);
});

test('a UUID absent from Repository metadata uses the explicit slash fallback without a transcript path', async () => {
  const h = pickerHarness(firstId);
  h.context.listProjectSessions = async (...args: unknown[]) => { h.listed.push(args); return { sessions: [] }; };
  await h.context.loadSessions();
  assert.deepEqual(h.listed, [['/current', 'repository']]);
  assert.deepEqual(h.resumed, [['/current', firstId, { scope: 'repository' }]]);
});

test('a failed metadata read does not fall back to opening a different copy by UUID', async () => {
  const h = pickerHarness(firstId);
  h.context.listProjectSessions = async () => ({ sessions: [], error: 'Repository metadata unavailable.' });
  await h.context.loadSessions();
  assert.deepEqual(h.resumed, []);
  assert.equal(h.state.error, 'Repository metadata unavailable.');
});

test('a unique title auto-opens while ambiguous titles remain in the picker', async () => {
  const exact = pickerHarness('  parser INVESTIGATION  ');
  await exact.context.loadSessions();
  assert.deepEqual(exact.resumed, [['/current', firstId, { scope: 'repository', transcriptPath: sessions[0]!.path }]]);
  const ambiguous = pickerHarness('Parser investigation');
  ambiguous.context.listProjectSessions = async () => ({ sessions: [sessions[0], row(secondId, 'Parser investigation')] });
  await ambiguous.context.loadSessions();
  assert.deepEqual(ambiguous.resumed, []);
  assert.equal(ambiguous.state.sessions.length, 2);
  assert.equal(ambiguous.state.closed, 0);
});

test('a selected All-scope row preserves its exact transcript path even when UUIDs are duplicated', async () => {
  const h = pickerHarness();
  h.context.scope = 'all';
  const exact = { ...sessions[0]!, path: '/all/chosen-copy.jsonl' };
  await h.context.openChat(firstId, exact);
  assert.deepEqual(h.resumed, [['/current', firstId, { scope: 'all', transcriptPath: exact.path }]]);
});

test('failed UUID opens retain their explicit error and allow another attempt', async () => {
  const h = pickerHarness(firstId);
  h.context.openSession = async () => { throw new Error('Session not found in local history.'); };
  await h.context.loadSessions();
  assert.equal(h.state.error, 'Session not found in local history.');
  assert.equal(h.state.closed, 0);
  assert.equal(h.context.opening.current, false);
  h.context.openSession = async () => true;
  await h.context.openChat(secondId);
  assert.equal(h.state.closed, 1);
});

test('a superseded catalog response is retried and stale scope results never replace the current list', async () => {
  const retry = pickerHarness();
  let count = 0;
  retry.context.listProjectSessions = async () => ++count === 1 ? undefined : { sessions };
  await retry.context.loadSessions();
  assert.equal(count, 2);
  assert.equal(retry.state.sessions, sessions);
  const h = pickerHarness(), old = deferred<unknown>(), current = deferred<unknown>();
  h.context.listProjectSessions = (_path: string, scope: string) => scope === 'repository' ? old.promise : current.promise;
  const first = h.context.loadSessions();
  h.context.scope = 'all'; h.context.firstRequest.current = false;
  const second = h.context.loadSessions();
  current.resolve({ sessions: [sessions[1]] }); await second;
  old.resolve({ sessions: [sessions[0]] }); await first;
  assert.equal(h.state.sessions[0]?.uuid, secondId);
});

test('a repeated mount does not lose exact-name resolution to the abandoned first read', async () => {
  const h = pickerHarness('Parser investigation'), abandoned = deferred<unknown>();
  let count = 0;
  h.context.listProjectSessions = async () => ++count === 1 ? abandoned.promise : { sessions };
  const first = h.context.loadSessions();
  h.context.requestGeneration.current++;
  await h.context.loadSessions();
  abandoned.resolve({ sessions }); await first;
  assert.deepEqual(h.resumed, [['/current', firstId, { scope: 'repository', transcriptPath: sessions[0]!.path }]]);
});

test('editing the initial search while loading prevents an automatic resume of the old name', async () => {
  const h = pickerHarness('Parser investigation'), response = deferred<unknown>();
  h.context.listProjectSessions = () => response.promise;
  const pending = h.context.loadSessions();
  h.context.firstRequest.current = false;
  response.resolve({ sessions }); await pending;
  assert.deepEqual(h.resumed, []);
  assert.equal(h.state.sessions, sessions);
});

test('keyboard input changes scopes, moves selection, resumes on Enter and traps Tab', async () => {
  const focused: string[] = [], changed: unknown[] = [], selected: unknown[] = [], opened: unknown[] = [];
  const input = { focus: () => focused.push('input') }, last = { focus: () => focused.push('last') };
  const context: any = {
    input: { current: input }, opening: { current: false }, firstRequest: { current: true },
    previousLocalScope: { current: 'repository' }, scope: 'repository',
    setScope: (value: string) => { context.scope = value; changed.push(value); },
    sessionPickerShortcutScope, rows: sessions, selectedIndex: 0, selected: sessions[1], resumeId: secondId, loading: false,
    setSelectedPath: (value: string) => selected.push(value), openChat: async (...args: unknown[]) => opened.push(args),
    dialogFocusTarget, document: { activeElement: last },
    dialog: { current: { querySelectorAll: () => [input, last] } },
  };
  evaluate('globalThis.changeScope = ' + initializer(pickerSource, 'changeScope'), context);
  evaluate('globalThis.keyDown = ' + initializer(pickerSource, 'handleKeyDown'), context);
  const press = (key: string, ctrlKey = false) => context.keyDown({
    key, ctrlKey, metaKey: false, altKey: false, shiftKey: false, nativeEvent: {}, target: input,
    preventDefault() {}, stopPropagation() {},
  });
  press('a', true); press('a', true); press('w', true);
  assert.deepEqual(changed, ['all', 'repository']);
  press('ArrowDown'); assert.deepEqual(selected, [sessions[1]!.path]);
  press('Enter'); assert.deepEqual(opened, [[secondId, sessions[1]]]);
  focused.length = 0; press('Tab'); assert.deepEqual(focused, ['input']);
});

test('focus returns to the visible opener and falls back when that opener was removed or hidden', () => {
  const focused: string[] = [];
  const fallback = { focus: () => focused.push('composer') } as unknown as HTMLElement;
  const opener = { isConnected: true, closest: () => null, getClientRects: () => [1], focus: () => focused.push('opener') };
  restoreSessionPickerFocus(opener as unknown as HTMLElement, fallback);
  opener.isConnected = false;
  restoreSessionPickerFocus(opener as unknown as HTMLElement, fallback);
  opener.isConnected = true; opener.getClientRects = () => [];
  restoreSessionPickerFocus(opener as unknown as HTMLElement, fallback);
  assert.deepEqual(focused, ['opener', 'composer', 'composer']);
});

test('Escape closes the picker while idle and keeps an in-flight resume visible', () => {
  let handler: ts.Expression | undefined;
  const walk = (node: ts.Node): void => {
    if (ts.isJsxAttribute(node) && node.name.getText(pickerSource) === 'onEscape' && node.initializer && ts.isJsxExpression(node.initializer)) handler = node.initializer.expression;
    ts.forEachChild(node, walk);
  };
  walk(pickerSource); assert.ok(handler);
  let closed = 0;
  const context: any = { opening: { current: false }, onClose: () => { closed++; } };
  evaluate('globalThis.escape = ' + handler.getText(pickerSource), context);
  context.escape(); assert.equal(closed, 1);
  context.opening.current = true;
  context.escape(); assert.equal(closed, 1);
});

test('the shared dialog leaves IME Escape unconsumed and preserves ordinary Escape and Tab delegation', () => {
  let handler: ts.Expression | undefined;
  const walk = (node: ts.Node): void => {
    if (ts.isJsxAttribute(node) && node.name.getText(dialogSource) === 'onKeyDown' && node.initializer && ts.isJsxExpression(node.initializer)) handler = node.initializer.expression;
    ts.forEachChild(node, walk);
  };
  walk(dialogSource); assert.ok(handler);
  let closed = 0, prevented = 0, stopped = 0;
  const delegated: unknown[] = [];
  const context = { onEscape: () => { closed++; }, onKeyDown: (event: unknown) => delegated.push(event) };
  evaluate('globalThis.keyDown = ' + handler.getText(dialogSource), context);
  const press = (key: string, isComposing: boolean, keyCode: number) => {
    const event = { key, nativeEvent: { isComposing, keyCode }, preventDefault: () => { prevented++; }, stopPropagation: () => { stopped++; } };
    (context as any).keyDown(event);
    return event;
  };
  press('Escape', true, 27); press('Escape', false, 229);
  assert.equal(closed, 0); assert.equal(prevented, 0); assert.equal(stopped, 0); assert.deepEqual(delegated, []);
  press('Escape', false, 27);
  assert.equal(closed, 1); assert.equal(prevented, 1); assert.equal(stopped, 1);
  const tab = press('Tab', false, 9);
  assert.deepEqual(delegated, [tab]);
  assert.equal(stopped, 1);
});

test('App picker events retain the current project path, query and original focus owner', () => {
  class Element {}
  const opener = new Element();
  const context: any = {
    sessionNavigationBlocked: () => false,
    cancelLocalAppNavigation() {},
    sessionPickerProject: '/current', sessionPickerSequence: { current: 0 }, sessionPickerRequest: { current: null },
    document: { activeElement: opener }, HTMLElement: Element, setSessionPicker: () => {},
  };
  evaluate('globalThis.openPicker = ' + initializer(appSource, 'openPicker'), context);
  context.openPicker({ detail: { query: ' Parser investigation ' } });
  assert.equal(context.sessionPickerRequest.current.projectPath, '/current');
  assert.equal(context.sessionPickerRequest.current.query, 'Parser investigation');
  assert.equal(context.sessionPickerRequest.current.opener, opener);
  context.document.activeElement = new Element();
  context.openPicker({ detail: { query: firstId } });
  assert.equal(context.sessionPickerRequest.current.opener, opener);
});

test('picker exposes scoped search, keyboard hints and an honest loading state in both themes', () => {
  for (const dark of [false, true]) {
    const markup = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(dark) }, React.createElement(SessionPicker, {
      projectPath: '/current', listProjectSessions: async () => undefined, openSession: async () => false, onOpened() {}, onClose() {},
    })));
    assert.match(markup, /role="dialog"/);
    assert.match(markup, /role="combobox"/);
    assert.match(markup, /role="listbox"/);
    assert.match(markup, /Repository/);
    assert.match(markup, /aria-pressed="true"[^>]*>Repository/);
    assert.doesNotMatch(markup, /Control\+W|Ctrl W/);
    assert.match(markup, /Control\+A/);
    assert.match(markup, /Loading chats/);
    assert.match(markup, /Close chat picker/);
  }
});

test('a suspended picker renders no modal over the target approval controls', () => {
  const markup = renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(true) }, React.createElement(SessionPicker, {
    projectPath: '/current', initialQuery: 'Parser investigation', suspended: true,
    listProjectSessions: async () => undefined, openSession: async () => false, onOpened() {}, onClose() {},
  })));
  assert.equal(markup, '');
});
