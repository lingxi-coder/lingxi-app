import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import type { UiJsonValue } from '@lingxi/bridge-client';
import {
  buildNativeUiParentInputRequest,
  buildNativeUiParentPressRequest,
  buildNativeUiParentSelectRequest,
  ModUiParentTree,
  parseModUiParentTree,
  type ModUiParentTreeProps,
} from '../src/renderer/components/modUiParentTree';
import { Theme } from '../src/renderer/theme/ThemeContext';
import { tokens } from '../src/renderer/theme/tokens';

(globalThis as { React?: typeof React }).React = React;

const site = { surface: 'desktop', component: 'ToolUse', instanceId: 'tool-row-7' } as const;
const requestProps = { original: 'tool input' };
const responseProps = { rewritten: 'response input' };
const press = { plugin: 'fixture-plugin', handle: 31 };

function render(tree: unknown, overrides: Partial<ModUiParentTreeProps> = {}): string {
  const props: ModUiParentTreeProps = {
    tree: tree as UiJsonValue | null,
    site,
    requestProps,
    responseProps,
    renderClient: (descriptor, reactKey) => React.createElement('div', {
      key: reactKey,
      'data-client-plugin': descriptor.plugin,
      'data-client-key': descriptor.key,
      'data-client-module': descriptor.module,
    }, descriptor.props.label ?? 'Client'),
    onParentControl: () => undefined,
    ...overrides,
  };
  return renderToStaticMarkup(React.createElement(Theme.Provider, { value: tokens(false) }, React.createElement(ModUiParentTree, props)));
}

test('renders the Native Parent tree node families and keeps engine/Client rows distinct', () => {
  const html = render({
    type: 'Box',
    props: { key: 'scope', flexDirection: 'column', padding: 8 },
    children: [
      { type: 'Text', hover: { scope: 'scope', color: '#123456' }, group: { plugin: 'fixture-plugin' }, children: ['Parent status'] },
      { type: 'div', props: { style: 'display:flex;color:#222;background-image:url(https://invalid.test/x)', id: 'legacy-node', class: 'from-native' }, children: ['Legacy div'] },
      { type: 'span', props: { style: 'font-weight:700' }, children: ['Legacy span'] },
      { type: 'b', props: { style: 'text-decoration:underline' }, children: ['Legacy bold'] },
      { type: 'Button', props: { key: 'run', label: 'Run', variant: 'primary' }, press },
      { type: 'Input', props: { key: 'filter', label: 'Filter', value: '' }, press: { ...press, handle: 32 } },
      { type: 'Select', props: { key: 'period', label: 'Period', options: [{ value: 'day' }, { value: 'week' }], value: 'week' }, press: { ...press, handle: 33 } },
      { type: 'Link', props: { href: 'https://example.com/docs', label: 'Docs' } },
      { type: 'Code', props: { source: 'const ready = true;', language: 'typescript', path: 'ready.ts', startLine: 4, wrap: 'truncate-end' } },
      { type: 'Markdown', props: { key: 'summary', text: '**Ready**', pressableLinks: ['https://example.com'] }, press: { ...press, handle: 34 } },
      { type: 'Client', props: { key: 'local', module: 'surface/local.tsx', props: { label: 'Client row' }, width: '50%' }, client: { plugin: 'fixture-plugin' } },
      { type: 'Svg', props: { source: '<svg xmlns="http://www.w3.org/2000/svg"><circle cx="1" cy="1" r="1"/></svg>', alt: 'Diagram', width: 24, height: 18 } },
      { type: 'Svg', props: { source: '<svg xmlns="http://www.w3.org/2000/svg"><script>parent.postMessage("x","*")</script></svg>', alt: 'Interactive diagram', isInteractive: true } },
      { type: 'engine', ref: 17 },
    ],
  }, {
    engineFallback: ({ ref, requestProps: original, responseProps: rewritten }) => React.createElement('div', {
      'data-engine-ref': ref,
      'data-engine-original': original.original,
    }, rewritten.rewritten),
  });

  assert.match(html, /class="mod-ui-parent-box"/);
  assert.match(html, /data-mod-ui-hover-plugin="fixture-plugin"/);
  assert.match(html, /data-mod-ui-hover-scope="scope"/);
  assert.match(html, /id="legacy-node"/);
  assert.match(html, /class="mod-ui-parent-html mod-ui-parent-html-div from-native"/);
  assert.match(html, /style="display:flex;color:#222"/);
  assert.doesNotMatch(html, /background-image:url/);
  assert.match(html, /class="mod-ui-parent-button"[^>]*>Run<\/button>/);
  assert.match(html, /class="mod-ui-parent-input"/);
  assert.match(html, /class="mod-ui-parent-select-label"/);
  assert.match(html, /href="https:\/\/example.com\/docs"/);
  assert.match(html, /data-path="ready.ts" data-start-line="4" data-wrap="truncate-end"/);
  assert.match(html, /class="mod-ui-parent-markdown"[^>]*data-pressable-links="https:\/\/example.com"/);
  assert.match(html, /data-client-plugin="fixture-plugin" data-client-key="local" data-client-module="surface\/local.tsx"/);
  assert.match(html, /<img[^>]*alt="Diagram"/);
  assert.match(html, /<iframe[^>]*title="Interactive diagram"[^>]*sandbox="allow-scripts"/);
  assert.match(html, /data-engine-ref="17" data-engine-original="tool input">response input/);
  assert.doesNotMatch(html, /<script>parent\.postMessage/);
});

test('uses the engine fallback for invalid trees without turning ordinary renderer input into a Client fault', () => {
  const html = render({ type: 'Raster', props: { source: 'unused' } }, {
    fallback: React.createElement('div', { className: 'original-engine-row' }, 'Original row'),
  });
  assert.match(html, /class="original-engine-row">Original row/);
  assert.doesNotMatch(html, /mod-ui-client-fallback|ui_client_fault/);
  assert.throws(() => parseModUiParentTree({ type: 'Raster' }), /Unsupported Native Parent UI element/);
});

test('enforces Native Parent bounds and control schemas without adding compatibility aliases', () => {
  assert.doesNotThrow(() => parseModUiParentTree({
    type: 'Box', props: { key: 'controls', display: 'none' },
    hover: { scope: 'controls', display: 'flex' }, group: { plugin: 'fixture-plugin' },
    children: [{ type: 'Text', hover: { color: 'green' }, group: { plugin: 'fixture-plugin' }, children: ['open'] }],
  }));
  assert.throws(() => parseModUiParentTree({
    type: 'Select', props: { key: 'period', options: [{ value: 'day' }, { value: 'day' }] }, press,
  }), /unique/);
  assert.throws(() => parseModUiParentTree({
    type: 'Button', props: { key: 'run', label: 'Run', hotkey: 'Shift+R' }, press,
  }), /hotkey/);
  assert.throws(() => parseModUiParentTree({
    type: 'Markdown', props: { text: '[open](https://example.test)', pressableLinks: ['https://example.test'] },
  }), /press identity/);
  assert.throws(() => parseModUiParentTree({
    type: 'Client', props: { key: 'local', module: 'surface/local.tsx', width: 'intrinsic' }, client: { plugin: 'fixture-plugin' },
  }), /width/);
  assert.throws(() => parseModUiParentTree({ type: 'Box', props: { key: 'x' }, hover: { scope: 'x' } }), /group owner/);
  assert.throws(() => parseModUiParentTree({
    type: 'Box', props: { key: 'x', display: 'none' },
    children: [{ type: 'Text', hover: { color: 'red' }, group: { plugin: 'fixture-plugin' }, children: ['hidden'] }],
  }), /hidden keyed Box/);
  assert.throws(() => parseModUiParentTree({ type: 'Svg', props: { source: '<svg/>', alt: '', width: 0 } }), /width/);
  assert.throws(() => parseModUiParentTree({ type: 'Box', props: {}, children: ['x'.repeat(10_001)] }), /text exceeds/);
});

test('builds Native ui_press, ui_input, and ui_select request bodies from the parent render identity', () => {
  assert.deepEqual(buildNativeUiParentPressRequest(site, press, 'run'), {
    subtype: 'ui_press', plugin: 'fixture-plugin', handle: 31, key: 'run', surface: 'desktop',
  });
  assert.deepEqual(buildNativeUiParentPressRequest(site, press, 'summary', 'https://example.com'), {
    subtype: 'ui_press', plugin: 'fixture-plugin', handle: 31, key: 'summary', href: 'https://example.com', surface: 'desktop',
  });
  assert.deepEqual(buildNativeUiParentInputRequest(site, press, 'filter', 'change', 'partial'), {
    subtype: 'ui_input', plugin: 'fixture-plugin', handle: 31, key: 'filter', kind: 'change', value: 'partial',
    component: 'ToolUse', instance_id: 'tool-row-7', surface: 'desktop',
  });
  assert.deepEqual(buildNativeUiParentInputRequest(site, press, 'filter', 'submit', 'done'), {
    subtype: 'ui_input', plugin: 'fixture-plugin', handle: 31, key: 'filter', kind: 'submit', value: 'done',
    component: 'ToolUse', instance_id: 'tool-row-7', surface: 'desktop',
  });
  assert.deepEqual(buildNativeUiParentSelectRequest(site, press, 'period', 'week'), {
    subtype: 'ui_select', plugin: 'fixture-plugin', handle: 31, key: 'period', value: 'week',
    component: 'ToolUse', instance_id: 'tool-row-7', surface: 'desktop',
  });
});
