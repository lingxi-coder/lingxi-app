import { test } from 'node:test';
import assert from 'node:assert/strict';

import {
  abovePromptRequest,
  clientFrameForNode,
  clientOperationRequest,
  isValidModUiTree,
} from '../src/renderer/components/ModUiSurface.js';

test('desktop AbovePrompt render request carries the mounted desktop identity and measured viewport', () => {
  assert.deepEqual(abovePromptRequest('renderer-1', true, { columns: 720, rows: 180 }), {
    subtype: 'ui_render',
    surface: 'desktop',
    component: 'AbovePrompt',
    instance_id: 'electron-above-prompt',
    client_id: 'renderer-1',
    props: {
      hasSurvey: false,
      isWorking: true,
      maxRows: 12,
      bodyColumns: 720,
      scroll: { offset: 0, bodyRows: 12 },
      view: {},
    },
    viewport: { columns: 720, rows: 180, isFullscreen: false },
  });
});

test('desktop Mod UI tree keeps Text children and accepts bounded parent controls', () => {
  assert.equal(isValidModUiTree({
    type: 'Box',
    props: { flexDirection: 'column' },
    children: [
      'Intro',
      { type: 'Text', props: { bold: true }, children: ['More'] },
      { type: 'Button', props: { key: 'apply', label: 'Apply' }, press: { plugin: 'review', handle: 1 } },
      { type: 'Input', props: { key: 'query', value: 'q'.repeat(16_384) }, press: { plugin: 'review', handle: 2 } },
      { type: 'Select', props: { key: 'mode', options: [{ value: 'safe', label: 'Safe' }] }, press: { plugin: 'review', handle: 3 } },
    ],
  }), true);
});

test('desktop Mod UI renderer fails closed for excessive depth, control values, and select options', () => {
  let deep: unknown = 'end';
  for (let index = 0; index < 34; index += 1) deep = { type: 'Box', children: [deep] };
  assert.equal(isValidModUiTree(deep), false);
  assert.equal(isValidModUiTree({
    type: 'Input',
    props: { key: 'query', value: 'q'.repeat(16_385) },
    press: { plugin: 'review', handle: 1 },
  }), false);
  assert.equal(isValidModUiTree({
    type: 'Select',
    props: { key: 'mode', options: Array.from({ length: 65 }, (_, index) => ({ value: String(index) })) },
    press: { plugin: 'review', handle: 1 },
  }), false);
});

test('Client frames attach only to the matching parent identity and revision', () => {
  const clientNode = {
    type: 'Client',
    props: { key: 'suggestions', module: 'client/suggestions.tsx' },
    client: { plugin: 'reviewer' },
  };
  const frame = {
    renderRevision: 8,
    frameSequence: 2,
    hasPointerListener: true,
    hasKeyListener: false,
    parent: { surface: 'desktop', component: 'AbovePrompt', instance_id: 'electron-above-prompt' },
    client: { plugin: 'reviewer', key: 'suggestions', module: 'client/suggestions.tsx' },
    tree: { type: 'Button', props: { key: 'accept', label: 'Accept' }, held: 4 },
  };
  const snapshots = [{ sessionId: 'session-1', runtimeId: 'client-runtime-1', frame }];

  const matched = clientFrameForNode(snapshots, 'session-1', 8, clientNode);
  assert.equal(matched?.scope.runtimeId, 'client-runtime-1');
  assert.equal(matched?.scope.hasPointerListener, true);
  assert.equal(clientFrameForNode(snapshots, 'session-1', 9, clientNode), null);
  assert.equal(clientFrameForNode(snapshots, 'session-2', 8, clientNode), null);
  assert.equal(clientFrameForNode(snapshots, 'session-1', 8, {
    ...clientNode,
    props: { key: 'other', module: 'client/suggestions.tsx' },
  }), null);
});

test('Client controls keep Host VM handles and Markdown link callbacks explicit', () => {
  const request = clientOperationRequest({
    type: 'runHeld',
    runtimeId: 'client-runtime-1',
    render_revision: 8,
    handle: 4,
    event: { type: 'press' },
  });
  assert.equal(request.type, 'ui_client_operation');
  assert.deepEqual(JSON.parse(request.operation_json), {
    type: 'runHeld', runtimeId: 'client-runtime-1', render_revision: 8, handle: 4, event: { type: 'press' },
  });
  assert.equal(isValidModUiTree({
    type: 'Markdown',
    props: { key: 'docs', text: '[guide](https://example.com)', pressableLinks: ['https://example.com'] },
    press: { plugin: 'reviewer', handle: 5 },
  }), true);
  assert.equal(isValidModUiTree({
    type: 'Svg',
    props: { source: '<svg viewBox="0 0 12 12"><path d="M0 0h12v12z" /></svg>', alt: 'Square', width: 24 },
  }), true);
  assert.equal(isValidModUiTree({
    type: 'Svg',
    props: { source: 'x'.repeat(100_001), alt: 'Oversized' },
  }), false);
});
