import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import {
  buildNativeUiClientPressRequest,
  ModUiClientTree,
  parseModUiClientTree,
  type ModUiClientPressEvent,
  type ModUiSurfaceNode,
} from '../src/renderer/components/modUiClientTree';
import {
  collectModUiClientElements,
  gridSizeFromPixels,
  heldHandleForElement,
  invalidationTargetsRenderSite,
  isClientControlAddressable,
} from '../src/renderer/components/modUiAbovePrompt';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

const site = { component: 'AbovePrompt', instanceId: 'parent-render-1' };
const client = { plugin: 'status-plugin', key: 'status', module: 'surface/status.tsx' };
const noOp = () => undefined;

function render(tree: unknown, interactionsEnabled = true): string {
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(ModUiClientTree, {
    tree,
    site,
    client,
    onPress: noOp,
    interactionsEnabled,
  })));
}

test('renders the eight Native Client surface node kinds as inert JSON-backed React nodes', () => {
  const tree: ModUiSurfaceNode = {
    type: 'Box',
    props: { flexDirection: 'column', columnGap: 8 },
    children: [
      { type: 'Text', props: { bold: true }, children: ['Status'] },
      { type: 'Button', props: { key: 'refresh', label: 'Refresh', variant: 'primary', hotkey: 'r' }, press: { plugin: 'status-plugin', handle: 1 } },
      { type: 'Input', props: { key: 'filter', label: 'Filter', placeholder: 'name', value: '' }, press: { plugin: 'status-plugin', handle: 2 } },
      { type: 'Select', props: { key: 'period', label: 'Period', options: [{ value: 'day', label: 'Today' }, { value: 'week' }], value: 'day' }, press: { plugin: 'status-plugin', handle: 3 } },
      { type: 'Link', props: { href: 'https://example.com', label: 'Details' } },
      { type: 'Code', props: { source: 'const ready = true;', language: 'typescript', path: 'status.ts' } },
      { type: 'Markdown', props: { text: '**All good**', dimColor: true, pressableLinks: [] }, press: { plugin: 'status-plugin', handle: 4 } },
    ],
  };

  const html = render(tree);
  assert.match(html, /class="mod-ui-client-box"/);
  assert.match(html, /class="mod-ui-client-text"[^>]*><strong>Status|class="mod-ui-client-text"[^>]*>Status/);
  assert.match(html, /class="mod-ui-client-button"[^>]*>Refresh/);
  assert.match(html, /class="mod-ui-client-input"[^>]*placeholder="name"/);
  assert.match(html, /class="mod-ui-client-select"/);
  assert.match(html, /<option value="day" selected="">Today<\/option>/);
  assert.match(html, /class="mod-ui-client-link"[^>]*href="https:\/\/example.com\/?"/);
  assert.match(html, /data-language="typescript"/);
  assert.match(html, /ready = /);
  assert.match(html, /class="mod-ui-client-markdown"[^>]*[\s\S]*All good/);
  assert.doesNotMatch(html, /<Client|plugin source/);
});

test('keeps links inert unless they use a supported external URL scheme', () => {
  const html = render({
    type: 'Box',
    children: [
      { type: 'Link', props: { href: 'javascript:alert(1)', label: 'blocked' } },
      { type: 'Link', props: { href: 'mailto:support@example.com', label: 'email' } },
      { type: 'Link', props: { href: 'https://example.com/path', label: 'web' } },
      { type: 'Link', props: { href: 'http://localhost:3000/health', label: 'local' } },
      { type: 'Link', props: { href: 'https://user:pass@example.com/', label: 'credentials' } },
      { type: 'Link', props: { href: 'https://example.com/@team', label: 'raw at' } },
    ],
  });
  assert.match(html, /class="mod-ui-client-link-disabled"[^>]*>blocked/);
  assert.match(html, /class="mod-ui-client-link-disabled"[^>]*>email/);
  assert.doesNotMatch(html, /href="javascript:/);
  assert.match(html, /href="https:\/\/example.com\/path"/);
  assert.match(html, /href="http:\/\/localhost:3000\/health"/);
  assert.doesNotMatch(html, /href="https:\/\/user:pass/);
  assert.doesNotMatch(html, /href="https:\/\/example.com\/@team/);
});

test('rejects unknown element names and nested Client instances before React can create tags', () => {
  assert.throws(() => parseModUiClientTree({ type: 'script', props: { children: 'alert(1)' } }), /node type is unsupported/);
  assert.throws(() => parseModUiClientTree({ type: 'Box', children: [{ type: 'Client', props: { key: 'nested', module: 'nested.tsx' } }] }), /node type is unsupported/);
});

test('rejects leaf children, repeated Select values, and malformed held handles', () => {
  assert.throws(() => parseModUiClientTree({
    type: 'Button',
    props: { key: 'ok', label: 'Okay' },
    press: { plugin: 'status-plugin', handle: 1 },
    children: ['unexpected'],
  }), /is a leaf/);
  assert.throws(() => parseModUiClientTree({
    type: 'Select',
    props: { key: 'choice', options: [{ value: 'x' }, { value: 'x' }] },
    press: { plugin: 'status-plugin', handle: 1 },
  }), /must be unique/);
  assert.throws(() => parseModUiClientTree({
    type: 'Button', props: { key: 'ok', label: 'Okay' }, press: { plugin: 'status-plugin', handle: 0 },
  }), /positive safe integer/);
  assert.throws(() => parseModUiClientTree({ type: 'Box', props: { flexDirection: 'diagonal' } }), /flexDirection is invalid/);
  assert.throws(() => parseModUiClientTree({ type: 'Box', props: { top: 1.5 } }), /top is invalid/);
  assert.throws(() => parseModUiClientTree({ type: 'Text', props: { wrap: 'unknown' } }), /wrap is invalid/);
  assert.throws(() => parseModUiClientTree({ type: 'Button', props: { key: 'ok', label: 'Okay' }, press: { plugin: 'other', handle: 1 } }, 'status-plugin'), /owning Client plugin/);
});

test('builds source-shaped press/input/select control bodies with the parent render identity', () => {
  const events: ModUiClientPressEvent[] = [
    { type: 'press' },
    { type: 'input', kind: 'change', value: 'hello' },
    { type: 'input', kind: 'submit', value: 'hello' },
    { type: 'select', value: 'week' },
  ];
  assert.deepEqual(events.map((event) => buildNativeUiClientPressRequest(site, client, 'control', event)), [
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[0] },
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[1] },
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[2] },
    { subtype: 'ui_client_press', plugin: 'status-plugin', component: 'AbovePrompt', instance_id: 'parent-render-1', client: 'status', module: 'surface/status.tsx', element: 'control', event: events[3] },
  ]);
  assert.throws(() => buildNativeUiClientPressRequest(site, client, 'control', { type: 'input', kind: 'change', value: 'x'.repeat(16_385) }), /input event value is invalid/);
  assert.throws(() => buildNativeUiClientPressRequest(site, { ...client, module: 'm'.repeat(257) }, 'control', { type: 'press' }), /client.module is invalid/);
});

test('extracts only unique, leaf Client identities from a parent ui.render tree', () => {
  assert.deepEqual(collectModUiClientElements({
    type: 'Box',
    children: [{ type: 'Client', props: { key: 'status', module: 'surface.tsx', props: { value: ['ready'] }, width: '80%' }, client: { plugin: 'status-plugin' } }],
  }), [{ plugin: 'status-plugin', key: 'status', module: 'surface.tsx', props: { value: ['ready'] }, width: '80%' }]);
  assert.throws(() => collectModUiClientElements({
    type: 'Client', props: { key: 'status', module: 'surface.tsx' }, client: { plugin: 'status-plugin' }, children: 'not a leaf',
  }), /leaves/);
  assert.throws(() => collectModUiClientElements({
    type: 'Box', children: [
      { type: 'Client', props: { key: 'status', module: 'one.tsx' }, client: { plugin: 'status-plugin' } },
      { type: 'Client', props: { key: 'status', module: 'two.tsx' }, client: { plugin: 'status-plugin' } },
    ],
  }), /more than once/);
  assert.throws(() => collectModUiClientElements({
    type: 'Client', props: { key: 'status', module: 'surface.tsx', props: { unsafe: Number.NaN } }, client: { plugin: 'status-plugin' },
  }), /not finite/);
  const longIdentity = collectModUiClientElements({
    type: 'Box', children: [
      { type: 'Client', props: { key: 'k'.repeat(257), module: 'm'.repeat(257) }, client: { plugin: 'status-plugin' } },
      { type: 'Client', props: { key: 'sibling', module: 'surface.tsx' }, client: { plugin: 'status-plugin' } },
    ],
  });
  assert.equal(longIdentity.length, 2);
  assert.equal(isClientControlAddressable(longIdentity[0]), false);
  assert.equal(isClientControlAddressable(longIdentity[1]), true);
});

test('maps hook-adjusted elements to the current held handle and scopes invalidations by site', () => {
  const frame = {
    type: 'Box', children: [
      { type: 'Button', props: { key: 'run', label: 'Run' }, press: { plugin: 'status-plugin', handle: 7 } },
    ],
  } as unknown as ModUiSurfaceNode;
  assert.equal(heldHandleForElement(frame as unknown as never, 'run', 'status-plugin'), 7);
  assert.equal(heldHandleForElement(frame as unknown as never, 'run', 'other-plugin'), null);
  assert.equal(heldHandleForElement(frame as unknown as never, 'missing', 'status-plugin'), null);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's1', uuid: 'u1' }, 's1', 'above-prompt-x'), true);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's2', uuid: 'u1' }, 's1', 'above-prompt-x'), false);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's1', uuid: 'u1', instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: 'other' }] }, 's1', 'above-prompt-x'), false);
  assert.equal(invalidationTargetsRenderSite({ sessionId: 's1', uuid: 'u1', instances: [{ surface: 'desktop', component: 'AbovePrompt', instance_id: 'above-prompt-x' }] }, 's1', 'above-prompt-x'), true);
});

test('keeps press controls disabled when the owning parent address is not representable', () => {
  const longClient = { ...client, key: 'k'.repeat(257) };
  assert.equal(isClientControlAddressable(longClient), false);
  const html = render({
    type: 'Box',
    children: [
      { type: 'Button', props: { key: 'run', label: 'Run' }, press: { plugin: 'status-plugin', handle: 1 } },
      { type: 'Input', props: { key: 'filter', value: '' }, press: { plugin: 'status-plugin', handle: 2 } },
      { type: 'Select', props: { key: 'period', options: [{ value: 'day' }] }, press: { plugin: 'status-plugin', handle: 3 } },
    ],
  }, isClientControlAddressable(longClient));
  assert.match(html, /<button[^>]*disabled=""[^>]*>Run/);
  assert.match(html, /<input[^>]*disabled=""/);
  assert.match(html, /<select[^>]*disabled=""/);
});

test('calculates VM character grids from measured pixels without a guessed cell size', () => {
  assert.deepEqual(gridSizeFromPixels(101, 38, 10, 19), { columns: 10, rows: 2 });
  assert.equal(gridSizeFromPixels(0, 38, 10, 19), null);
  assert.equal(gridSizeFromPixels(100, 38, Number.NaN, 19), null);
});
