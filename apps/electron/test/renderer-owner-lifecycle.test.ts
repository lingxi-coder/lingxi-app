import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { resolve } from 'node:path';
import { test } from 'node:test';
import { runInNewContext } from 'node:vm';
import React, { createElement } from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import ts from 'typescript';
import { SettingsScreen } from '../src/renderer/components/settings/SettingsScreen';
import * as layerFields from '../src/renderer/components/settings/layerFields';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';
import { imageFileToAttachment } from '../src/renderer/bridge/imageInput';
import {
  interruptConfigurationOperations, requestConfigurationOperation, settleConfigurationOperation,
  type PendingConfigurationOperations,
} from '../src/renderer/bridge/configurationOperations';
import type { ConfigurationOperationEvent } from '../src/renderer/bridge/bridgeTypes';

// Node's TSX loader uses the classic JSX transform for source imports.
(globalThis as unknown as { React: typeof React }).React = React;
const require = createRequire(import.meta.url);
function source(path: string): ts.SourceFile {
  return ts.createSourceFile(path, readFileSync(resolve(path), 'utf8'), ts.ScriptTarget.Latest, true,
    path.endsWith('.tsx') ? ts.ScriptKind.TSX : ts.ScriptKind.TS);
}
function findNode(file: ts.SourceFile, predicate: (node: ts.Node) => boolean): ts.Node {
  let found: ts.Node | undefined;
  const visit = (node: ts.Node): void => { if (!found && predicate(node)) found = node; ts.forEachChild(node, visit); };
  visit(file); assert.ok(found, `production node missing in ${file.fileName}`); return found;
}
function evaluate(code: string, context: Record<string, unknown>): void {
  runInNewContext(ts.transpileModule(code, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.ReactJSX,
  } }).outputText, context);
}
function settingsMarkup(snapshot: unknown): string {
  const bridge = {
    activeSession: { sessionId: 'A', projectPath: '/A' }, connected: true, sessionLoading: false,
    bootstrap: { settings: { projects: [] }, workspace: { path: '/A' } }, agentCatalog: [],
    settingsSnapshotEvent: snapshot, refreshSettingsSnapshot: async () => {},
  };
  return renderToStaticMarkup(createElement(Theme.Provider, { value: tokens('light') },
    createElement(SettingsScreen, { bridge: bridge as never, initialPageId: 'tools-agent', theme: 'light',
      onTheme: () => {}, onClose: () => {} })));
}
test('connected layered settings wait for a valid snapshot and offer retry after parse failure', () => {
  const loading = settingsMarkup(null);
  assert.match(loading, /settings-snapshot-loading/);
  assert.doesNotMatch(loading, /aria-label="enabledTools"/);
  const invalid = settingsMarkup({ type: 'settings_snapshot', effective_json: '{', provenance_json: '{}' });
  assert.match(invalid, /settings-snapshot-error/); assert.match(invalid, /重试/);
  assert.doesNotMatch(invalid, /aria-label="enabledTools"/);
  const ready = settingsMarkup({ type: 'settings_snapshot', effective_json: '{}', provenance_json: '{}',
    layers_json: JSON.stringify({ user: { enabledTools: ['Read'], outputStyle: 'Explanatory' } }) });
  assert.match(ready, /aria-label="enabledTools"[^>]*value="Read"/);
  assert.match(ready, /aria-label="outputStyle"[^>]*value="Explanatory"/);
});

/** Persistent hook cells exercise the actual ToolsAgent, including its effect dependencies. */
function toolsHarness() {
  const cells: any[] = [], deps: unknown[][] = [];
  let cursor = 0, effectCursor = 0, effects: Array<() => void> = [], dirty = false;
  const hooks = {
    useState(initial: unknown) {
      const slot = cursor++; if (!(slot in cells)) cells[slot] = initial;
      return [cells[slot], (next: any) => {
        const value = typeof next === 'function' ? next(cells[slot]) : next;
        if (!Object.is(value, cells[slot])) { cells[slot] = value; dirty = true; }
      }];
    },
    useRef(initial: unknown) { const slot = cursor++; return cells[slot] ??= { current: initial }; },
    useEffect(effect: () => void, next: unknown[]) {
      const slot = effectCursor++;
      if (!deps[slot] || next.some((value, index) => !Object.is(value, deps[slot][index]))) {
        deps[slot] = next; effects.push(effect);
      }
    },
  };
  const module = { exports: {} as any };
  const imports: Record<string, unknown> = {
    react: hooks, 'react/jsx-runtime': require('react/jsx-runtime'), '../layerFields': layerFields,
    '../rows': { Card: 'Card', Row: 'Row', FieldProvenanceNotice: 'Notice' },
    '../../../theme/ThemeContext': { useT: () => tokens('light') }, '../primitives': { Toggle: 'Toggle' },
    './ghostButton': { ghostButtonStyle: () => ({}), inputStyle: () => ({}) },
  };
  evaluate(readFileSync(resolve('src/renderer/components/settings/pages/ToolsAgent.tsx'), 'utf8'),
    { module, exports: module.exports, require: (id: string) => { assert.ok(id in imports, id); return imports[id]; } });
  const saved: unknown[] = [];
  const bridge = { agentCatalog: [], updateEngineSettings: async (destination: string, patch: unknown) => saved.push({ destination, patch }) };
  return { saved, render(snapshot: unknown, editingLayer = 'user'): any {
    for (let attempt = 0; attempt < 5; attempt += 1) {
      cursor = effectCursor = 0; effects = []; dirty = false;
      const output = module.exports.ToolsAgent({ bridge, snapshot, editingLayer });
      effects.forEach((effect) => effect()); if (!dirty) return output;
    }
    throw new Error('component did not settle');
  } };
}
function nodes(node: any): any[] {
  if (Array.isArray(node)) return node.flatMap(nodes);
  return node && typeof node === 'object' ? [node, ...nodes(node.props?.children)] : [];
}
function field(tree: unknown, label: string): any {
  const node = nodes(tree).find((entry) => entry.type === 'input' && entry.props['aria-label'] === label);
  assert.ok(node, label); return node;
}
test('first real ToolsAgent snapshot initializes drafts without clobbering later unsaved input', () => {
  const harness = toolsHarness();
  let tree = harness.render(null);
  const empty = field(tree, 'enabledTools');
  const emptyParent = nodes(tree).find((node) => node.type === 'div' && nodes(node.props.children).includes(empty));
  assert.equal(nodes(emptyParent).find((node) => node.type === 'button').props.disabled, true);
  tree = harness.render({ layers: { user: { enabledTools: ['Read'], outputStyle: 'Explanatory' } } });
  assert.equal(field(tree, 'enabledTools').props.value, 'Read');
  assert.equal(field(tree, 'outputStyle').props.value, 'Explanatory');
  field(tree, 'enabledTools').props.onChange({ target: { value: 'Read, Edit' } });
  tree = harness.render({ layers: { user: { enabledTools: ['Bash'], outputStyle: 'Other' } } });
  assert.equal(field(tree, 'enabledTools').props.value, 'Read, Edit');
  const input = field(tree, 'enabledTools');
  const parent = nodes(tree).find((node) => node.type === 'div' && nodes(node.props.children).includes(input));
  nodes(parent).find((node) => node.type === 'button').props.onClick();
  assert.deepEqual(JSON.parse(JSON.stringify(harness.saved)), [{ destination: 'user', patch: { enabledTools: ['Read', 'Edit'] } }]);
  tree = harness.render({ layers: { project: { enabledTools: ['Glob'], outputStyle: 'Project' } } }, 'project');
  assert.equal(field(tree, 'enabledTools').props.value, 'Glob');
  assert.equal(field(tree, 'outputStyle').props.value, 'Project');
});

function imageHarness() {
  const file = source('src/renderer/components/BetaDesktop.tsx');
  const declaration = findNode(file, (node) => ts.isVariableDeclaration(node) && node.name.getText(file) === 'addImageFiles') as ts.VariableDeclaration;
  let release!: (bytes: ArrayBuffer) => void;
  const bytes = new Promise<ArrayBuffer>((resolve) => { release = resolve; });
  const pendingFile = { name: 'private-A.png', size: 8, arrayBuffer: () => bytes } as File;
  let current: any[] = [];
  const revoked: string[] = [], notices: unknown[] = [];
  const context: any = {
    ready: true, activeSessionId: 'A', imageAttachments: [], MAX_IMAGE_ATTACHMENTS: 4,
    draftSessionId: { current: 'A' }, imageDraftGeneration: { current: 1 }, composerDraftRevision: { current: 0 }, imageFileToAttachment,
    URL: { revokeObjectURL: (url: string) => revoked.push(url) },
    setImageNotice: (notice: unknown) => notices.push(notice),
    setImageAttachments: (update: (images: any[]) => any[]) => { current = update(current); },
  };
  evaluate(`globalThis.addImageFiles = ${declaration.initializer!.getText(file)};`, context);
  return { context, revoked, notices, pendingFile, setCurrent: (images: any[]) => { current = images; }, images: () => current,
    release: () => release(Uint8Array.from([137, 80, 78, 71, 13, 10, 26, 10]).buffer) };
}
test('an image read from A cannot append to B or a cleared/reopened A draft', async () => {
  for (const nextSession of ['B', 'A']) {
    const harness = imageHarness(); const pending = harness.context.addImageFiles([harness.pendingFile]);
    harness.context.draftSessionId.current = nextSession; harness.context.imageDraftGeneration.current += 1;
    harness.setCurrent([{ name: 'current.png' }]); harness.release(); await pending;
    assert.deepEqual(harness.images(), [{ name: 'current.png' }]);
    assert.equal(harness.revoked.length, 1); assert.deepEqual(harness.notices, []);
  }
});
test('an image read appends normally to the unchanged owner draft', async () => {
  const harness = imageHarness(); const pending = harness.context.addImageFiles([harness.pendingFile]);
  harness.release(); await pending;
  assert.deepEqual(Array.from(harness.images(), (image) => image.name), ['private-A.png']);
  assert.equal(harness.revoked.length, 0);
});
test('a queued image update releases its attachment if ownership changes before React invokes it', async () => {
  const harness = imageHarness(); let update!: (images: unknown[]) => unknown[];
  harness.context.setImageAttachments = (next: typeof update) => { update = next; };
  const pending = harness.context.addImageFiles([harness.pendingFile]); harness.release(); await pending;
  harness.context.draftSessionId.current = 'B'; harness.context.imageDraftGeneration.current += 1;
  const current = [{ name: 'B.png' }]; assert.equal(update(current), current); assert.equal(harness.revoked.length, 1);
});

function event(status: ConfigurationOperationEvent['status'] = 'succeeded'): ConfigurationOperationEvent {
  return { type: 'configuration_operation', domain: 'hook', operation_id: 7, status, effect: 'applied' };
}
test('configuration RPCs distinguish identical ids in different owners and accept out-of-order terminals', async () => {
  const pending: PendingConfigurationOperations = new Map();
  const a = requestConfigurationOperation(pending, 'A', 'hook', 7, async () => {}, 1_000);
  const b = requestConfigurationOperation(pending, 'B', 'hook', 7, async () => {}, 1_000);
  settleConfigurationOperation(pending, 'A', event('progress')); assert.equal(pending.size, 2);
  settleConfigurationOperation(pending, 'B', event()); assert.equal((await b).status, 'succeeded'); assert.equal(pending.size, 1);
  settleConfigurationOperation(pending, 'A', event()); assert.equal((await a).status, 'succeeded'); assert.equal(pending.size, 0);
});
test('the real useBridge event handler settles A after the UI navigates to B', async () => {
  const pending: PendingConfigurationOperations = new Map();
  const wait = requestConfigurationOperation(pending, 'A', 'hook', 7, async () => {}, 1_000);
  const file = source('src/renderer/bridge/useBridge.ts');
  const call = findNode(file, (node) => ts.isCallExpression(node) && node.expression.getText(file) === 'host.onEvent') as ts.CallExpression;
  const context: any = {
    removedRuntimeIds: { current: new Set() }, bootstrapRef: { current: { runtimes: [] } }, pendingSessionRef: { current: null },
    sideQuestionTurns: { current: new Map() }, pendingFusionDispatches: { current: new Map() }, isPendingFusionSessionRestore: () => false,
    updateRuntime: () => {}, activeSessionIdRef: { current: 'B' }, pendingConfigurationOperations: { current: pending }, settleConfigurationOperation,
    setConfigurationOperations: () => { throw new Error('A result cannot paint B UI'); },
  };
  evaluate(`globalThis.handleEvent = ${call.arguments[0].getText(file)};`, context);
  context.handleEvent({ sessionId: 'A', sequence: 1, event: event() });
  assert.equal((await wait).status, 'succeeded'); assert.equal(pending.size, 0);
});
test('a replaced connection interrupts only its owner; late success cannot revive the request', async () => {
  const pending: PendingConfigurationOperations = new Map();
  const a = requestConfigurationOperation(pending, 'A', 'hook', 7, async () => {}, 1_000);
  const rejected = assert.rejects(a, /interrupted/);
  const b = requestConfigurationOperation(pending, 'B', 'hook', 7, async () => {}, 1_000);
  interruptConfigurationOperations(pending, 'A'); settleConfigurationOperation(pending, 'A', event()); await rejected;
  assert.equal(pending.size, 1); settleConfigurationOperation(pending, 'B', event()); await b;
});
test('configuration failure, dispatch rejection and timeout each release their waiter', async () => {
  const pending: PendingConfigurationOperations = new Map();
  const failed = requestConfigurationOperation(pending, 'A', 'hook', 7, async () => {}, 1_000);
  settleConfigurationOperation(pending, 'A', { ...event('failed'), message: 'revision changed' });
  await assert.rejects(failed, /revision changed/);
  await assert.rejects(requestConfigurationOperation(pending, 'A', 'hook', 8, async () => { throw new Error('dispatch failed'); }, 1_000), /dispatch failed/);
  await assert.rejects(requestConfigurationOperation(pending, 'A', 'hook', 9, async () => {}, 5), /Timed out/);
  assert.equal(pending.size, 0);
});


test('a delayed dispatch rejection cannot reject a newer request reusing the completed owner/id', async () => {
  const pending: PendingConfigurationOperations = new Map();
  let failSend!: (error: Error) => void;
  const send = new Promise<void>((_resolve, reject) => { failSend = reject; });
  const first = requestConfigurationOperation(pending, 'A', 'hook', 7, () => send, 1_000);
  settleConfigurationOperation(pending, 'A', event()); await first;
  const next = requestConfigurationOperation(pending, 'A', 'hook', 7, async () => {}, 1_000);
  failSend(new Error('old send failed after its terminal')); await Promise.resolve();
  assert.equal(pending.size, 1);
  settleConfigurationOperation(pending, 'A', event()); await next;
});

test('the real connection-disposal handler rejects its pending configuration operation immediately', async () => {
  const pending: PendingConfigurationOperations = new Map();
  const wait = requestConfigurationOperation(pending, 'A', 'hook', 7, async () => {}, 1_000);
  const rejected = assert.rejects(wait, /interrupted/);
  const file = source('src/renderer/bridge/useBridge.ts');
  const call = findNode(file, (node) => ts.isCallExpression(node)
    && node.expression.getText(file) === 'host.onConnectionStateChanged') as ts.CallExpression;
  const context: any = {
    pendingConfigurationOperations: { current: pending }, interruptConfigurationOperations,
    removedRuntimeIds: { current: new Set() }, isRuntimeRemovedState: () => true,
    setBootstrap: () => {}, setRuntimeStates: () => {}, removeRuntimeFromMaps: () => {}, completeTrackedSpeech: () => {},
  };
  for (const name of ['turnActiveRefs', 'engineTurnActiveRefs', 'slashPendingRefs', 'pendingFusionDispatches', 'sideQuestionTurns', 'cancellingRefs', 'cancellationTasks']) {
    context[name] = { current: new Map() };
  }
  evaluate(`globalThis.handleState = ${call.arguments[0].getText(file)};`, context);
  context.handleState({ sessionId: 'A', event: { status: 'disconnected', reason: 'session runtime disposed' } });
  await rejected; assert.equal(pending.size, 0);
});


test('a failed configuration dispatch retains A ownership without surfacing its error in the newly active B UI', async () => {
  const pending: PendingConfigurationOperations = new Map();
  let failSend!: (error: Error) => void;
  const send = new Promise<void>((_resolve, reject) => { failSend = reject; });
  const owners: string[] = [];
  let globalErrors = 0;
  const file = source('src/renderer/bridge/useBridge.ts');
  const dispatch = findNode(file, (node) => ts.isVariableDeclaration(node)
    && node.name.getText(file) === 'runConfigurationAdmin') as ts.VariableDeclaration;
  const initializer = dispatch.initializer as ts.CallExpression;
  const operationId = findNode(file, (node) => ts.isFunctionDeclaration(node)
    && node.name?.getText(file) === 'configurationOperationId');
  const context: any = {
    pendingConfigurationOperations: { current: pending }, requestConfigurationOperation,
    sessionLoadingRef: { current: false }, activeSessionIdRef: { current: 'A' },
    CONFIGURATION_OPERATION_TIMEOUT_MS: 1_000,
    host: { command: (sessionId: string) => { owners.push(sessionId); return send; } },
    capture: (cause: unknown) => { globalErrors += 1; throw cause; },
  };
  evaluate(`${operationId.getText(file)}; globalThis.runAdmin = ${initializer.arguments[0].getText(file)};`, context);
  const wait = context.runAdmin('hook', { type: 'hook_admin', command: { action: 'save_document', operation_id: 7 } });
  const rejected = assert.rejects(wait, /A dispatch failed/);
  context.activeSessionIdRef.current = 'B'; failSend(new Error('A dispatch failed')); await rejected;
  assert.deepEqual(owners, ['A']); assert.equal(globalErrors, 0); assert.equal(pending.size, 0);
});


test('mixed-file attachment errors cannot paint another session after an image read completes', async () => {
  const harness = imageHarness();
  const file = source('src/renderer/components/BetaDesktop.tsx');
  const declaration = findNode(file, (node) => ts.isVariableDeclaration(node)
    && node.name.getText(file) === 'addFiles') as ts.VariableDeclaration;
  harness.context.window = { lingxi: { getPathForFile: () => { throw new Error('private-A.txt could not be read'); } } };
  harness.context.chooseFile = () => {};
  evaluate(`globalThis.addFiles = ${declaration.initializer!.getText(file)};`, harness.context);
  const pending = harness.context.addFiles([harness.pendingFile, { name: 'private-A.txt', type: 'text/plain' }]);
  harness.context.draftSessionId.current = 'B'; harness.context.imageDraftGeneration.current += 1;
  harness.release(); await pending;
  assert.deepEqual(harness.notices, []);
});
