import assert from 'node:assert/strict';
import { test } from 'node:test';

import { SessionEventCorrelator } from '../src/eventCorrelation.js';
import {
  parseUiFrameEvent,
  parseUiInvalidateEvent,
  validateClientEvent,
  validateNativeUiControlRequest,
  validateNativeUiControlResponseJson,
  validateUiControlMetadataJson,
  validateUiClientOperation,
  validateUiClientOperationResponse,
} from '../src/validation.js';
import { buildNativeUiControlCommand } from '../src/uiControlCommands.js';

test('Native UI control requests strip protocol extensions and preserve application JSON', () => {
  const request = {
    subtype: 'ui_render',
    surface: 'desktop',
    component: 'AbovePrompt',
    instance_id: 'instance-1',
    props: { title: 'Hello', busy: false },
    viewport: { columns: 80, rows: 24, isFullscreen: true },
    on_screen: { first: 1, last: 4, of: 8 },
    content_rows: 5,
    keyed: [{ plugin: 'plugin-a', key: 'client-a', top: 1, bottom: 4 }],
    bench: { seq: 1, t0: 12.5 },
  } as const;
  assert.deepEqual(validateNativeUiControlRequest(request), request);
  const extended = {
    ...request,
    future_control_field: 'strip-me',
    props: { ...request.props, plugin_owned: { visible: true } },
    viewport: { ...request.viewport, future_viewport_field: 'strip-me' },
    on_screen: { ...request.on_screen, future_range_field: 'strip-me' },
    keyed: [{ ...request.keyed[0], future_region_field: 'strip-me' }],
    bench: { ...request.bench, future_bench_field: 'strip-me' },
  };
  assert.deepEqual(validateNativeUiControlRequest(extended), {
    ...request,
    props: { ...request.props, plugin_owned: { visible: true } },
  });
  assert.throws(() => validateNativeUiControlRequest({ ...request, on_screen: { first: 4, last: 1, of: 8 } }), /on-screen range/);

  const module = validateNativeUiControlRequest({ subtype: 'ui_client_module', plugin: 'plugin-a', ignored: true });
  assert.deepEqual(module, { subtype: 'ui_client_module', plugin: 'plugin-a' });
  const message = validateNativeUiControlRequest({
    subtype: 'ui_message', plugin: 'plugin-a', component: 'AbovePrompt', instance_id: 'instance-1',
    client: 'client-a', module: 'main', data: { plugin_owned: ['keep'] }, ignored: true,
  });
  assert.deepEqual(message, {
    subtype: 'ui_message', plugin: 'plugin-a', component: 'AbovePrompt', instance_id: 'instance-1',
    client: 'client-a', module: 'main', data: { plugin_owned: ['keep'] },
  });
  const prototypeKeyData = JSON.parse('{"__proto__":{"plugin_owned":"keep"}}') as Record<string, unknown>;
  const normalizedPrototypeKeyData = validateNativeUiControlRequest({
    subtype: 'ui_message', plugin: 'plugin-a', component: 'AbovePrompt', instance_id: 'instance-1',
    client: 'client-a', module: 'main', data: prototypeKeyData,
  });
  assert.equal(normalizedPrototypeKeyData.subtype, 'ui_message');
  if (normalizedPrototypeKeyData.subtype !== 'ui_message') assert.fail('message request was not preserved');
  assert.equal(Object.prototype.hasOwnProperty.call(normalizedPrototypeKeyData.data, '__proto__'), true);
  assert.deepEqual((normalizedPrototypeKeyData.data as Record<string, unknown>)['__proto__'], { plugin_owned: 'keep' });
  assert.deepEqual(JSON.parse(JSON.stringify(normalizedPrototypeKeyData.data)), prototypeKeyData);
  const fault = validateNativeUiControlRequest({
    subtype: 'ui_client_fault', plugin: 'plugin-a', component: 'AbovePrompt', instance_id: 'instance-1',
    client: 'client-a', module: 'main', phase: 'render', reason: 'failed', ignored: true,
  });
  assert.deepEqual(fault, {
    subtype: 'ui_client_fault', plugin: 'plugin-a', component: 'AbovePrompt', instance_id: 'instance-1',
    client: 'client-a', module: 'main', phase: 'render', reason: 'failed',
  });
});

test('Native UI press uses the exact event union and only exposes the reached element/value', () => {
  const press = {
    subtype: 'ui_client_press', plugin: 'plugin-a', component: 'ToolUse', instance_id: 'instance-1',
    client: 'client-a', module: 'main', element: 'submit', event: { type: 'press' },
  } as const;
  assert.deepEqual(validateNativeUiControlRequest(press), press);
  assert.throws(() => validateNativeUiControlRequest({ ...press, client: 'c'.repeat(257) }), /invalid UI client id/);
  const input = { ...press, event: { type: 'input', kind: 'submit', value: 'hello' } } as const;
  assert.deepEqual(validateNativeUiControlRequest(input), input);
  assert.deepEqual(validateNativeUiControlRequest({
    ...input, future_control_field: 'strip-me',
    event: { ...input.event, future_event_field: 'strip-me' },
  }), input);
  assert.deepEqual(validateNativeUiControlRequest({
    ...press, event: { type: 'press', future_event_field: 'strip-me' },
  }), press);
  assert.deepEqual(validateNativeUiControlRequest({
    ...press, event: { type: 'select', value: 'selected', future_event_field: 'strip-me' },
  }), { ...press, event: { type: 'select', value: 'selected' } });
  const response = validateNativeUiControlResponseJson(input, JSON.stringify({
    handled: true, reached: { element: 'submit', value: 'hello' },
  }));
  assert.deepEqual(response, { handled: true, reached: { element: 'submit', value: 'hello' } });
  assert.throws(() => validateNativeUiControlResponseJson(input, JSON.stringify({
    handled: true,
    reached: { element: 'submit', value: { plugin: 'plugin-a', element: 'submit' } },
  })), /invalid UI reached value/);
});

test('Native parent press, input, and select controls normalize exact fields and keep distinct results', () => {
  const press = validateNativeUiControlRequest({
    subtype: 'ui_press', plugin: '', handle: 2 ** 53, key: 'k'.repeat(300), href: '', client_id: 'renderer-1',
    ignored: 'strip-me',
  });
  assert.deepEqual(press, {
    subtype: 'ui_press', plugin: '', handle: 2 ** 53, key: 'k'.repeat(300), href: '',
    client_id: 'renderer-1', surface: 'desktop',
  });
  assert.throws(() => validateNativeUiControlRequest({
    subtype: 'ui_press', plugin: 'plugin-a', handle: 1.5,
  }), /invalid UI handle/);
  assert.throws(() => validateNativeUiControlRequest({
    subtype: 'ui_press', plugin: 'plugin-a', handle: 1, href: 'x'.repeat(2_049),
  }), /invalid UI href/);
  assert.throws(() => validateNativeUiControlRequest({
    subtype: 'ui_press', plugin: 'plugin-a', handle: 1, client_id: 'bad client id',
  }), /invalid UI client id/);

  const input = validateNativeUiControlRequest({
    subtype: 'ui_input', plugin: 'p'.repeat(256), handle: -7, kind: 'change', value: '',
    key: 'k'.repeat(10_001), component: 'AbovePrompt', instance_id: '', client_id: 'renderer.1',
    ignored: true,
  });
  assert.deepEqual(input, {
    subtype: 'ui_input', plugin: 'p'.repeat(256), handle: -7, kind: 'change', value: '',
    key: 'k'.repeat(10_001), component: 'AbovePrompt', instance_id: '', client_id: 'renderer.1', surface: 'desktop',
  });
  assert.throws(() => validateNativeUiControlRequest({
    subtype: 'ui_input', plugin: 'p'.repeat(257), handle: 1, kind: 'submit', value: 'x',
  }), /invalid UI plugin/);
  assert.throws(() => validateNativeUiControlRequest({
    subtype: 'ui_input', plugin: 'plugin-a', handle: 1, kind: 'change', value: 'x'.repeat(16_385),
  }), /invalid UI input value/);

  const select = validateNativeUiControlRequest({
    subtype: 'ui_select', plugin: 'plugin-a', handle: 4, value: 'chosen', surface: 'vscode', ignored: true,
  });
  assert.deepEqual(select, { subtype: 'ui_select', plugin: 'plugin-a', handle: 4, value: 'chosen', surface: 'vscode' });

  for (const request of [press, input, select]) {
    assert.deepEqual(buildNativeUiControlCommand(request, 'request-1'), {
      type: request.subtype, request_id: 'request-1', request_json: JSON.stringify(request),
    });
  }

  assert.deepEqual(validateNativeUiControlResponseJson(press as Extract<typeof press, { subtype: 'ui_press' }>, JSON.stringify({
    handled: true, element: '', ignored: true,
  })), { handled: true, element: '' });
  assert.deepEqual(validateNativeUiControlResponseJson(input as Extract<typeof input, { subtype: 'ui_input' }>, JSON.stringify({
    handled: true, element: 'field', value: 'submitted', ignored: true,
  })), { handled: true, element: 'field', value: 'submitted' });
  assert.deepEqual(validateNativeUiControlResponseJson(select as Extract<typeof select, { subtype: 'ui_select' }>, JSON.stringify({
    handled: false,
  })), { handled: false });
  assert.equal('reached' in validateNativeUiControlResponseJson(press as Extract<typeof press, { subtype: 'ui_press' }>, JSON.stringify({ handled: false })), false);
});

test('UI client module source is carried as inert manifest data with bounded files', () => {
  const request = { subtype: 'ui_client_module', plugin: 'plugin-a' } as const;
  const sourceResponse = {
    plugin: 'plugin-a',
    hash: 'sha256-value',
    modules: [{ module: 'main', entry: 'index.tsx', component: 'AbovePrompt' }],
    runtime: 'react',
    limits: { nodes: 100, depth: 8, chars: 20_000, values: 2_000, dataDepth: 32 },
    files: [{ key: 'index.tsx', source: 'export default function Client() { return null; }' }],
  };
  const extendedResponse = {
    ...sourceResponse,
    ignored: true,
    modules: [{ ...sourceResponse.modules[0], ignored: true }],
    limits: { ...sourceResponse.limits, ignored: true },
    files: [{ ...sourceResponse.files[0], ignored: true }],
  };
  assert.deepEqual(validateNativeUiControlResponseJson(request, JSON.stringify(extendedResponse)), sourceResponse);
  assert.equal(validateNativeUiControlResponseJson(request, 'null'), null);
});

test('canonical UI control responses strip structural fields and preserve user JSON', () => {
  const renderRequest = {
    subtype: 'ui_render', surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1', props: {},
  } as const;
  const renderResponse = {
    tree: { type: 'box', plugin_owned: { id: 'keep' } },
    props: { plugin_owned: { state: 'keep' } },
    rewritten: false,
    hooked: true,
    extension: 'strip-me',
  };
  assert.deepEqual(validateNativeUiControlResponseJson(renderRequest, JSON.stringify(renderResponse)), {
    tree: renderResponse.tree,
    props: renderResponse.props,
    rewritten: false,
    hooked: true,
  });

  const pressRequest = {
    subtype: 'ui_client_press', plugin: 'plugin-a', component: 'ToolUse', instance_id: 'instance-1',
    client: 'client-a', module: 'main', element: 'submit', event: { type: 'press' },
  } as const;
  assert.deepEqual(validateNativeUiControlResponseJson(pressRequest, JSON.stringify({
    handled: true, extension: 'strip-me', reached: { element: 'submit', value: 'keep', extension: 'strip-me' },
  })), { handled: true, reached: { element: 'submit', value: 'keep' } });

  const messageRequest = {
    subtype: 'ui_message', plugin: 'plugin-a', component: 'AbovePrompt', instance_id: 'instance-1',
    client: 'client-a', module: 'main', data: {},
  } as const;
  assert.deepEqual(validateNativeUiControlResponseJson(messageRequest, JSON.stringify({
    handled: true, props: { plugin_owned: 'keep' }, extension: 'strip-me',
  })), { handled: true, props: { plugin_owned: 'keep' } });

  const faultRequest = {
    subtype: 'ui_client_fault', plugin: 'plugin-a', component: 'AbovePrompt', instance_id: 'instance-1',
    client: 'client-a', module: 'main', phase: 'render', reason: 'failed',
  } as const;
  assert.deepEqual(validateNativeUiControlResponseJson(faultRequest, JSON.stringify({ handled: false, extension: 'strip-me' })), {
    handled: false,
  });
});

test('Harness VM operations are a separate exact command family with revision-checked results', () => {
  const mount = {
    type: 'mount', surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1',
    plugin: 'plugin-a', client: 'client-a', module: 'main', render_revision: 4, columns: 80, rows: 24,
  } as const;
  assert.deepEqual(validateUiClientOperation(mount), mount);
  const frame = {
    runtimeId: 'runtime-a', renderRevision: 4, frameSequence: 1, tree: { type: 'text', text: 'ready' },
    hasPointerListener: true, hasKeyListener: false,
  };
  assert.deepEqual(validateUiClientOperationResponse(mount, frame), frame);
  assert.throws(() => validateUiClientOperationResponse(mount, { ...frame, renderRevision: 3 }), /revision mismatch/);
  assert.throws(() => validateUiClientOperationResponse(mount, { ...frame, frameSequence: 0 }), /frame sequence/);
  assert.throws(() => validateUiClientOperationResponse(mount, { ...frame, frameSequence: Number.MAX_SAFE_INTEGER + 1 }), /frame sequence/);
  const missingSequenceFrame = {
    runtimeId: frame.runtimeId,
    renderRevision: frame.renderRevision,
    tree: frame.tree,
    hasPointerListener: frame.hasPointerListener,
    hasKeyListener: frame.hasKeyListener,
  };
  assert.throws(() => validateUiClientOperationResponse(mount, missingSequenceFrame), /frame sequence/);

  // Stale/missing/unmounted runtimes are benign no-ops. They are not malformed
  // frames and must not be surfaced as synthetic Client faults.
  assert.deepEqual(validateUiClientOperationResponse(mount, { handled: false, renderRevision: 4 }), {
    handled: false, renderRevision: 4,
  });
  assert.throws(() => validateUiClientOperationResponse(mount, { handled: false, renderRevision: 3 }), /revision mismatch/);
  assert.throws(() => validateUiClientOperationResponse(mount, { handled: true, renderRevision: 4 }), /no-op handled flag/);
  assert.throws(() => validateUiClientOperationResponse(mount, { handled: false, renderRevision: 4, runtimeId: 'unexpected' }), /unsupported fields/);

  const workerFault = {
    handled: false, renderRevision: 4, runtimeId: 'runtime-a',
    fault: { phase: 'run', reason: 'client callback failed', source: 'worker' },
  } as const;
  assert.deepEqual(validateUiClientOperationResponse(mount, workerFault), workerFault);
  assert.deepEqual(validateUiClientOperationResponse(mount, {
    handled: false, renderRevision: 4,
    fault: { phase: 'load', reason: 'client module failed', source: 'worker' },
  }), {
    handled: false, renderRevision: 4,
    fault: { phase: 'load', reason: 'client module failed', source: 'worker' },
  });
  assert.throws(() => validateUiClientOperationResponse(mount, {
    ...workerFault, renderRevision: 3,
  }), /revision mismatch/);
  assert.throws(() => validateUiClientOperationResponse(mount, {
    ...workerFault, handled: true,
  }), /worker fault handled flag/);
  assert.throws(() => validateUiClientOperationResponse(mount, {
    ...workerFault, fault: { ...workerFault.fault, phase: 'unmount' },
  }), /worker fault phase/);
  assert.throws(() => validateUiClientOperationResponse(mount, {
    ...workerFault, fault: { ...workerFault.fault, source: 'renderer' },
  }), /worker fault source/);
  assert.throws(() => validateUiClientOperationResponse(mount, {
    ...workerFault, fault: { ...workerFault.fault, reason: 'x'.repeat(201) },
  }), /invalid UI worker fault reason/);
  assert.throws(() => validateUiClientOperationResponse(mount, {
    ...workerFault, extra: true,
  }), /unsupported fields/);

  const longIdentityMount = { ...mount, client: 'c'.repeat(10_000), module: 'm'.repeat(10_000) } as const;
  assert.deepEqual(validateUiClientOperation(longIdentityMount), longIdentityMount);
  assert.throws(() => validateUiClientOperation({ ...mount, client: 'c'.repeat(10_001) }), /invalid UI client id/);
  assert.throws(() => validateUiClientOperation({ ...mount, module: 'm'.repeat(10_001) }), /invalid UI module id/);

  const longDraw = {
    type: 'draw_commit', surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-1',
    render_revision: 4, clients: [{ plugin: 'plugin-a', key: 'k'.repeat(10_000), module: 'm'.repeat(10_000) }],
  } as const;
  assert.deepEqual(validateUiClientOperation(longDraw), longDraw);
  assert.throws(() => validateUiClientOperation({ ...longDraw, clients: [{ ...longDraw.clients[0], key: 'k'.repeat(10_001) }] }), /invalid UI client key/);
  assert.throws(() => validateUiClientOperation({ ...longDraw, clients: [{ ...longDraw.clients[0], module: 'm'.repeat(10_001) }] }), /invalid UI module id/);

  const runHeld = { type: 'runHeld', runtimeId: 'runtime-a', render_revision: 4, handle: 7 } as const;
  assert.deepEqual(validateUiClientOperation(runHeld), runHeld);
  assert.throws(() => validateUiClientOperation({ ...runHeld, source: 'not-an-operation-field' }), /unsupported fields/);

  const unmount = { type: 'unmount', runtimeId: 'runtime-a', render_revision: 4 } as const;
  assert.deepEqual(validateUiClientOperationResponse(unmount, { handled: true, renderRevision: 4 }), {
    handled: true, renderRevision: 4,
  });
});

test('UI control metadata preserves native render revision and host client lifecycle tokens', () => {
  const withPrototypeKey = validateUiControlMetadataJson(
    '{"renderRevision":12,"clientRuntimeEpochs":{"plugin-a":3,"__proto__":7},"clientStateToken":"7"}',
  );
  assert.deepEqual(withPrototypeKey, {
    renderRevision: 12,
    clientRuntimeEpochs: Object.fromEntries([['plugin-a', 3], ['__proto__', 7]]),
    clientStateToken: '7',
  });
  assert.equal(Object.hasOwn(withPrototypeKey.clientRuntimeEpochs!, '__proto__'), true);
  assert.equal(withPrototypeKey.clientRuntimeEpochs!['__proto__'], 7);
  const metadata = validateUiControlMetadataJson(JSON.stringify({ clientRuntimeEpochs: { 'plugin-a': 1 } }));
  assert.equal(metadata.renderRevision, undefined);
  assert.equal(Object.hasOwn(metadata.clientRuntimeEpochs!, 'plugin-a'), true);
  assert.equal(validateUiControlMetadataJson('{"clientStateToken":"0"}').clientStateToken, '0');
  assert.throws(() => validateUiControlMetadataJson('{"clientStateToken":""}'), /invalid UI client state token/);
  assert.throws(() => validateUiControlMetadataJson('{"clientStateToken":"v1"}'), /invalid UI client state token/);
  assert.throws(() => validateUiControlMetadataJson(`{"clientStateToken":"${'1'.repeat(21)}"}`), /invalid UI client state token/);
  assert.throws(() => validateUiControlMetadataJson('{}'), /no fields/);
  assert.throws(() => validateUiControlMetadataJson(JSON.stringify({ clientRuntimeEpochs: { 'plugin-a': 0 } })), /runtime epoch/);
  assert.throws(() => validateUiControlMetadataJson(JSON.stringify({ clientRuntimeEpochs: { 'plugin-a': Number.MAX_SAFE_INTEGER + 1 } })), /runtime epoch/);
  assert.throws(() => validateUiControlMetadataJson(JSON.stringify({ renderRevision: 1, unexpected: true })), /unsupported fields/);
});

test('UI frames and invalidations stay scoped to their owning session', () => {
  assert.deepEqual(parseUiFrameEvent('session-a', 'runtime-a', JSON.stringify({
    renderRevision: 2, frameSequence: 2, tree: { type: 'text', text: 'hello' }, hasPointerListener: false, hasKeyListener: true,
  })), {
    sessionId: 'session-a', runtimeId: 'runtime-a', frame: {
      renderRevision: 2, frameSequence: 2, tree: { type: 'text', text: 'hello' }, hasPointerListener: false, hasKeyListener: true,
    },
  });
  assert.throws(() => parseUiFrameEvent('session-a', 'runtime-a', JSON.stringify({
    renderRevision: 2, tree: { type: 'text', text: 'missing sequence' }, hasPointerListener: false, hasKeyListener: true,
  })), /frame sequence/);
  const workerFault = {
    renderRevision: 2,
    fault: { phase: 'render', reason: 'deferred render failed', source: 'worker' },
  };
  assert.deepEqual(parseUiFrameEvent('session-a', 'runtime-a', JSON.stringify(workerFault)), {
    sessionId: 'session-a', runtimeId: 'runtime-a', frame: workerFault,
  });
  assert.throws(() => parseUiFrameEvent('session-a', 'runtime-a', JSON.stringify({
    ...workerFault, frameSequence: 3,
  })), /unsupported fields/);
  assert.throws(() => parseUiFrameEvent('session-a', 'runtime-a', JSON.stringify({
    ...workerFault, fault: { ...workerFault.fault, source: 'renderer' },
  })), /worker fault source/);
  assert.throws(() => parseUiFrameEvent('session-a', 'runtime-a', JSON.stringify({
    ...workerFault, fault: { ...workerFault.fault, reason: 'x'.repeat(201) },
  })), /invalid UI worker fault reason/);
  assert.deepEqual(parseUiInvalidateEvent('session-a', 'session-a', 'uuid-a', JSON.stringify([
    { surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-a' },
  ])), {
    sessionId: 'session-a', uuid: 'uuid-a', instances: [
      { surface: 'desktop', component: 'AbovePrompt', instance_id: 'instance-a' },
    ],
  });
  assert.throws(() => parseUiInvalidateEvent('session-a', 'session-b', 'uuid-a'), /session mismatch/);
  assert.throws(() => validateClientEvent({ type: 'ui_client_frame', runtime_id: 'runtime-a', frame_json: 'not-json' }), /invalid UI client frame/);
});

test('event correlation includes session and request ids and releases pending promises', async () => {
  const correlator = new SessionEventCorrelator<string>();
  const first = correlator.request('session-a', 'same-request-id', 1_000);
  const second = correlator.request('session-b', 'same-request-id', 1_000);
  assert.equal(correlator.resolve('session-a', 'same-request-id', 'first'), true);
  assert.equal(correlator.reject('session-a', 'same-request-id', new Error('late')), false);
  assert.equal(correlator.size, 1);
  assert.equal(correlator.resolve('session-b', 'same-request-id', 'second'), true);
  assert.deepEqual(await Promise.all([first, second]), ['first', 'second']);
  assert.equal(correlator.size, 0);
});
