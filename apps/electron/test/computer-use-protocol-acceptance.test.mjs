import assert from 'node:assert/strict';
import test from 'node:test';
import { nativeComputerCall, decodeComputerProtocolResult, grantedApplicationIds, imageDimensions, redactComputerProtocolResult,
  isOwnedApplicationCommand, applicationProcessesForProfile, assertIsolatedProfileAvailable,
  assertOwnedDebugPort, computerAcceptanceDiscoveryQuery, axElementIndex,
  assertBackgroundSurfaceUnchanged, assertBrokerServiceAbsent, assertOwnedBrokerRemoval, assertDisabledComputerToolResult, selectAcceptanceAction, inspectedProcessCommand,
  createAcceptanceSteps, nextAcceptanceStep, requireAcceptancePreludes } from '../scripts/computer-use-protocol-acceptance-support.mjs';

test('explicit same-turn preludes are ordered, individually successful and cannot complete the primary action', () => {
  const steps = createAcceptanceSteps({ action: 'app_click' }, 'primary', [{ action: 'app_screenshot' }, { action: 'app_ax_find' }]);
  assert.equal(new Set(steps.map(step => step.toolId)).size, 3);
  const messages = [{ role: 'tool', tool_call_id: 'discover-primary' }];
  assert.equal(nextAcceptanceStep(messages, steps), steps[0]);
  messages.push({ role: 'tool', tool_call_id: steps[0].toolId });
  assert.equal(nextAcceptanceStep(messages, steps), steps[1]);
  messages.push({ role: 'tool', tool_call_id: steps[1].toolId });
  assert.equal(nextAcceptanceStep(messages, steps), steps[2]);
  const events = steps.slice(0, 2).map(step => ({ type: 'tool_use_result', id: step.toolId, is_error: false, result_json: '{}' }));
  assert.equal(requireAcceptancePreludes(steps, events).length, 2);
  assert.throws(() => requireAcceptancePreludes(steps, events.slice(1)), /Missing explicit prelude/);
  assert.throws(() => requireAcceptancePreludes(steps, [{ ...events[0], is_error: true }, events[1]]), /failed/);
  assert.throws(() => nextAcceptanceStep([{ role: 'tool', tool_call_id: 'primary' }], steps), /prelude order/);
  messages.push({ role: 'tool', tool_call_id: 'primary' });
  assert.equal(nextAcceptanceStep(messages, steps), null);
  assert.equal(createAcceptanceSteps({ action: 'app_click' }, 'negative').length, 1, 'negative cases receive no implicit screenshot');
});

test('process disappearance is distinct from a live changed identity or failed inspection', () => {
  for (const status of [0, 1]) assert.equal(inspectedProcessCommand({ status, stdout: '\n', stderr: '' }), null);
  assert.equal(inspectedProcessCommand({ status: 0, stdout: 'other live executable\n', stderr: '' }), 'other live executable');
  for (const result of [{ status: 1, stdout: 'ambiguous live row' }, { status: null }, { status: 2 }, { status: 0, stderr: 'denied' }]) {
    assert.throws(() => inspectedProcessCommand(result));
  }
});

test('discovery user-role reminders preserve the latest acceptance action until its native result', () => {
  const actions = new Map([['COMPUTER_ACCEPTANCE_old', { toolId: 'old' }], ['COMPUTER_ACCEPTANCE_new', { toolId: 'new' }]]);
  const messages = [
    { role: 'user', content: 'COMPUTER_ACCEPTANCE_old' },
    { role: 'tool', tool_call_id: 'old', content: 'old native result' },
    { role: 'user', content: [{ type: 'text', text: 'COMPUTER_ACCEPTANCE_new' }] },
    { role: 'assistant', tool_calls: [{ id: 'discover-new', function: { name: 'ToolSearch' } }] },
    { role: 'tool', tool_call_id: 'discover-new', content: '13 native tools loaded' },
    { role: 'user', content: 'Deferred tools are now available.' },
  ];
  assert.equal(selectAcceptanceAction(messages, actions)?.[1].toolId, 'new');
  messages.push({ role: 'tool', tool_call_id: 'new', content: 'new native result' });
  const selected = selectAcceptanceAction(messages, actions);
  assert.equal(selected?.[1].toolId, 'new');
  assert.ok(messages.some(message => message.tool_call_id === selected[1].toolId));
  assert.equal(selectAcceptanceAction([{ role: 'assistant', content: 'COMPUTER_ACCEPTANCE_old' }, { role: 'user', content: 'unrelated reminder' }], actions), null);
  assert.throws(() => selectAcceptanceAction([{ role: 'user', content: 'COMPUTER_ACCEPTANCE_old COMPUTER_ACCEPTANCE_new' }], actions), /multiple fixture actions/);
});

test('disabled projection absence counts only with independently disabled policy and exact tool identity', () => {
  const result = error => ({ is_error: true, result_json: JSON.stringify({ error }) });
  const name = 'mcp__computer-use__type';
  assert.doesNotThrow(() => assertDisabledComputerToolResult(result(`tool not found: ${name}`), { enabled: false }, name));
  assert.doesNotThrow(() => assertDisabledComputerToolResult(result('Computer use is turned off in Desktop settings'), { enabled: false }, name));
  assert.throws(() => assertDisabledComputerToolResult(result(`tool not found: ${name}`), { enabled: true }, name));
  assert.throws(() => assertDisabledComputerToolResult(result('tool not found: other'), { enabled: false }, name));
  assert.throws(() => assertDisabledComputerToolResult(result('network failed'), { enabled: false }, name));
  assert.throws(() => assertDisabledComputerToolResult({ ...result('Computer use is turned off in Desktop settings'), is_error: false }, { enabled: false }, name));
});

const frame = { sourceWidth: 1200, sourceHeight: 800, frameWidth: 600, frameHeight: 400 };
// A bounded JPEG frame header suffices to exercise dimension parsing; no GUI,
// image generation, tool dispatch or application process is involved.
const jpeg = Buffer.from([0xff,0xd8,0xff,0xc0,0,11,8,1,144,2,88,1,1,0x11,0,0xff,0xd9]);
const encoded = jpeg.toString('base64');
const options = { geometry: { width: 1200, height: 800 }, filtering: 'mask' };

test('Broker preflight refuses a registered or uninspectable same-channel global service', () => {
  const target = 'gui/501/com.lingxi.code.credential-broker.development';
  assert.doesNotThrow(() => assertBrokerServiceAbsent({ status: 113 }, target));
  assert.throws(() => assertBrokerServiceAbsent({ status: 0, stdout: 'registered real user service' }, target), /already registered/);
  for (const state of [{ status: 1 }, { status: null }, { status: 113, error: new Error('inspect denied') }]) {
    assert.throws(() => assertBrokerServiceAbsent(state, target), /Cannot establish/);
  }
});

test('Broker teardown requires exact private-home plist, executable, service and stable PID', () => {
  const homePath = '/private/acceptance/home';
  const owned = { homePath, target: 'gui/501/com.lingxi.code.credential-broker.development',
    plistPath: `${homePath}/Library/LaunchAgents/com.lingxi.code.credential-broker.development.plist`,
    programPath: `${homePath}/Library/Application Support/LingXi/CredentialBroker/development/LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker`, pid: 50001 };
  assert.doesNotThrow(() => assertOwnedBrokerRemoval(owned, { ...owned }));
  for (const patch of [{ target: 'gui/501/another-service' }, { pid: 35289 }, { pid: 1 },
    { plistPath: '/Users/example/Library/LaunchAgents/com.lingxi.code.credential-broker.development.plist' },
    { programPath: '/Users/example/Library/Application Support/LingXi/CredentialBroker/development/LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker' }]) {
    assert.throws(() => assertOwnedBrokerRemoval(owned, { ...owned, ...patch }));
  }
  const escaped = { ...owned, programPath: `${homePath}/../real-user-broker` };
  assert.throws(() => assertOwnedBrokerRemoval(escaped, escaped), /outside/);
});

test('isolated preflight leaves unrelated application bundles and profiles outside its owned PID set', () => {
  const executable = '/isolated/LingXi Code.app/Contents/MacOS/LingXi Code';
  const profile = '/private/acceptance/home/Library/Application Support/lingxi-code-desktop';
  const unrelated = [
    `35289 /Users/example/Applications/LingXi Code.app/Contents/MacOS/LingXi Code --user-data-dir=/Users/example/Library/Application Support/lingxi-code-desktop`,
    `40001 ${executable} --user-data-dir=/another/profile`,
    `40002 ${executable}-other --user-data-dir=${profile}`,
    `40003 ${executable} --user-data-dir=${profile}-other`,
  ].join('\n');
  assert.doesNotThrow(() => assertIsolatedProfileAvailable(unrelated, executable, [profile]));
  assert.deepEqual(applicationProcessesForProfile(unrelated, executable, [profile]), []);
  const owned = `${executable} --user-data-dir=${profile} --remote-debugging-port=9234`;
  assert.equal(isOwnedApplicationCommand(owned, executable, [profile]), true);
  assert.deepEqual(applicationProcessesForProfile(`${unrelated}\n50001 ${owned}`, executable, [profile]), [{ pid: 50001, command: owned }]);
  assert.throws(() => assertIsolatedProfileAvailable(`${unrelated}\n50001 ${owned}`, executable, [profile]), /already owns/);
  assert.equal(isOwnedApplicationCommand(`${executable} --user-data-dir=${profile}/child`, executable, [profile]), false);
  assert.equal(isOwnedApplicationCommand(`${owned} --user-data-dir=/user/profile`, executable, [profile]), false);
  assert.doesNotThrow(() => assertOwnedDebugPort([50001,50001], 50001));
  assert.throws(() => assertOwnedDebugPort([35289], 50001), /exclusively/);
  assert.throws(() => assertOwnedDebugPort([50001,35289], 50001), /exclusively/);
  assert.throws(() => assertOwnedDebugPort([], 50001), /exclusively/);
});

test('native acceptance emits FQNs, exact argument names and shared screenshot-frame coordinates', () => {
  assert.ok(computerAcceptanceDiscoveryQuery.startsWith('select:mcp__computer-use__request_access,'));
  assert.ok(computerAcceptanceDiscoveryQuery.slice(7).split(',').length < 20);
  assert.deepEqual(nativeComputerCall({ action: 'request_access', apps: ['Notes'], reason: 'Review note' }), {
    name: 'mcp__computer-use__request_access', arguments: { apps: ['Notes'], reason: 'Review note' },
  });
  assert.deepEqual(nativeComputerCall({ action: 'open_application', bundle_id: 'com.apple.Notes' }), {
    name: 'mcp__computer-use__open_application', arguments: { app: 'com.apple.Notes' },
  });
  assert.deepEqual(nativeComputerCall({ action: 'left_click', coordinate: [400,200] }, frame).arguments, { coordinate: [200,100] });
  assert.deepEqual(nativeComputerCall({ action: 'app_click', app: 'com.example.fixture', window_id: 29, coordinate: [400,200] }, frame).arguments,
    { app: 'com.example.fixture', window_id: 29, coordinate: [400,200] });
  assert.deepEqual(nativeComputerCall({ action: 'computer_batch', actions: [{ action: 'left_click', coordinate: [400,200] }, { action: 'wait', duration: 4 }] }, frame).arguments,
    { actions: [{ action: 'left_click', coordinate: [200,100] }, { action: 'wait', duration: 4 }] });
  assert.throws(() => nativeComputerCall({ action: 'display_size' }), /not a native/);
});

test('background app images retain AX summaries and use their own returned window dimensions', () => {
  const content = [{ type: 'image', data: encoded, mimeType: 'image/jpeg' }, { type: 'text', text: 'Captured window_id 29.\n<ax-summary>\n[2] AXButton [20,30 80×24] "Record test click"\n[4] AXTextField [30,80 200×30]\n</ax-summary>' }];
  const image = decodeComputerProtocolResult(content, { scope: 'app' });
  assert.equal(image.captureScope, 'app');
  assert.equal(image.width, 600);
  assert.equal(image.height, 400);
  assert.equal(image.frame, undefined);
  assert.equal(axElementIndex(image.message, 'AXButton', 'Record test click'), 2);
  assert.equal(axElementIndex(image.message, 'AXTextField'), 4);
  assert.throws(() => axElementIndex(image.message, 'AXTextArea'), /exactly one/);
  assert.throws(() => axElementIndex(image.message + '\n[7] AXButton [1,1 2×2] "Record test click"', 'AXButton', 'Record test click'), /exactly one/);
});

test('background acceptance rejects activation, pointer movement and missing passive evidence', () => {
  const before = { frontmostBundleId: 'com.example.shadow', foregroundEventCount: 12, cursor: [20,30], mouseMoveEventCount: 45 };
  assert.doesNotThrow(() => assertBackgroundSurfaceUnchanged(before, { ...before }, 'com.example.shadow'));
  for (const patch of [{ frontmostBundleId: 'com.example.target' }, { foregroundEventCount: 13 }, { cursor: [21,30] }, { mouseMoveEventCount: 46 }, { mouseMoveEventCount: undefined }]) {
    assert.throws(() => assertBackgroundSurfaceUnchanged(before, { ...before, ...patch }, 'com.example.shadow'));
  }
});

test('native text results decode actual grant records without fabricating string IDs', () => {
  const output = { granted: [{ bundleId: 'com.apple.Notes', displayName: 'Notes' }], denied: [], screenshotFiltering: 'mask' };
  const content = [{ type: 'text', text: JSON.stringify(output) }];
  assert.deepEqual(decodeComputerProtocolResult(JSON.stringify(content)), output);
  assert.deepEqual(grantedApplicationIds(output), ['com.apple.Notes']);
  assert.deepEqual(decodeComputerProtocolResult({ content }), output);
  assert.deepEqual(decodeComputerProtocolResult({ data: { content } }), output);
  assert.equal(decodeComputerProtocolResult([{ type: 'text', text: 'Permission is still missing.' }]).message, 'Permission is still missing.');
});

test('image receipts derive frame size from returned bytes and distinguish controlled geometry provenance', () => {
  assert.deepEqual(imageDimensions(jpeg), { width: 600, height: 400 });
  const content = [{ type: 'image', data: encoded, mimeType: 'image/jpeg' }];
  const result = decodeComputerProtocolResult(content, options);
  assert.equal(result.file.base64, encoded);
  assert.deepEqual(result.frame, frame);
  assert.match(result.acceptanceGeometrySource, /controlled native fixture/);
  const provider = [{ type: 'image', source: { type: 'base64', data: encoded, media_type: 'image/jpeg' } }];
  assert.deepEqual(decodeComputerProtocolResult(provider, options), result);
  assert.throws(() => decodeComputerProtocolResult(content, { geometry: options.geometry }), /Filtering/);
  assert.throws(() => decodeComputerProtocolResult(content, { ...options, geometry: { width: 100, height: 100 } }), /aspect ratio/);
  assert.throws(() => decodeComputerProtocolResult([{ type: 'image', source: { type: 'url', url: 'https://example.test/image' } }], options), /Unsupported/);
});

test('receipt redaction removes binary data from native and provider image blocks', () => {
  const value = { content: [{ type: 'image', data: encoded, mimeType: 'image/jpeg' },
    { type: 'image', source: { type: 'base64', media_type: 'image/jpeg', data: encoded } }], file: { base64: encoded } };
  const redacted = JSON.stringify(redactComputerProtocolResult(value));
  assert.ok(!redacted.includes(encoded));
  assert.ok(redacted.includes('omitted from receipt'));
});
