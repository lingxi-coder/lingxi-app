// Pure adapters for the native computer-use acceptance driver. These do not
// change tool definitions or dispatch through the legacy combined computer tool.
import assert from 'node:assert/strict';
import { isAbsolute, relative, sep } from 'node:path';

export function createAcceptanceSteps(input, toolId, preludes = []) {
  return [...preludes.map((input, index) => ({ input, toolId: `prelude-${index}-${toolId}`, prelude: true })),
    { input, toolId, prelude: false }];
}

export function nextAcceptanceStep(messages, steps) {
  const completed = new Set(messages.filter(message => message.role === 'tool').map(message => message.tool_call_id));
  const next = steps.findIndex(step => !completed.has(step.toolId));
  if (next === -1) return null;
  assert.ok(!steps.slice(next + 1).some(step => completed.has(step.toolId)), 'Acceptance native results must follow explicit prelude order');
  return steps[next];
}

export function requireAcceptancePreludes(steps, events) {
  return steps.filter(step => step.prelude).map(step => {
    const result = events.find(event => event.type === 'tool_use_result' && event.id === step.toolId);
    assert.ok(result, `Missing explicit prelude result ${step.toolId}`);
    assert.equal(result.is_error, false, `Explicit prelude ${step.input.action} failed: ${result.result_json}`);
    return { step, result };
  });
}

export function inspectedProcessCommand(result) {
  assert.ok(!result.error && !result.stderr?.trim() && [0, 1].includes(result.status), 'Cannot safely inspect the owned application PID');
  const command = result.stdout?.trim() ?? '';
  if (!command) return null; // ps can return success with an empty raced-away row.
  assert.equal(result.status, 0, 'A failed process inspection returned ambiguous output');
  return command;
}

export function selectAcceptanceAction(messages, actions) {
  // Tool discovery appends its own user-role reminder. The active fixture
  // instruction is the most recent user message containing a known marker.
  for (const message of [...messages].reverse()) {
    if (message.role !== 'user') continue;
    const content = JSON.stringify(message.content) ?? '';
    const matches = [...actions].filter(([marker]) => content.includes(marker));
    assert.ok(matches.length <= 1, 'An acceptance user message cannot select multiple fixture actions');
    if (matches.length) return matches[0];
  }
  return null;
}

export function assertDisabledComputerToolResult(result, policy, toolName) {
  assert.equal(policy.enabled, false, 'Missing tools count as disabled only while device policy is independently disabled');
  assert.equal(result.is_error, true, 'Disabled input must fail');
  const error = JSON.parse(result.result_json).error;
  assert.ok(typeof error === 'string' && (error.includes('turned off in Desktop settings')
    || error === `tool not found: ${toolName}`), `Unexpected disabled-tool result: ${error}`);
}

export function assertBrokerServiceAbsent(state, target) {
  if (state.status === 113 && !state.error) return;
  if (state.status === 0) throw new Error(`Credential Broker service ${target} is already registered; refusing to launch any acceptance fixture or application`);
  throw new Error(`Cannot establish that Credential Broker service ${target} is absent (status=${state.status}); no acceptance application may launch`);
}

export function assertOwnedBrokerRemoval(expected, observed) {
  assert.equal(observed.target, expected.target, 'Broker service target changed');
  for (const field of ['plistPath', 'programPath']) {
    assert.equal(observed[field], expected[field], `Broker ${field} does not match the test-owned path`);
    const local = relative(expected.homePath, observed[field]);
    assert.ok(local !== '' && local !== '..' && !local.startsWith(`..${sep}`) && !isAbsolute(local),
      `Broker ${field} resolves outside the private acceptance home`);
  }
  assert.ok(observed.pid === undefined || Number.isSafeInteger(observed.pid) && observed.pid > 1, 'Broker PID is invalid');
  if (expected.pid !== undefined) assert.equal(observed.pid, expected.pid, 'Broker PID changed before guarded teardown');
}

export const computerAcceptanceDiscoveryQuery = 'select:' + [
  'request_access', 'list_granted_applications', 'open_application',
  'screenshot', 'left_click', 'type', 'computer_batch',
  'app_list_windows', 'app_screenshot', 'app_ax_find', 'app_click', 'app_type', 'app_release',
].map(action => `mcp__computer-use__${action}`).join(',');

export function isOwnedApplicationCommand(command, executable, profiles) {
  return command.startsWith(`${executable} `) && profiles.length > 0
    && (command.match(/(?:^| )--user-data-dir=/g) ?? []).length === 1
    && profiles.some(profile => command.includes(`--user-data-dir=${profile} `)
      || command.endsWith(`--user-data-dir=${profile}`));
}

export function assertOwnedDebugPort(listenerPids, ownedPid) {
  assert.ok(Number.isSafeInteger(ownedPid) && ownedPid > 0, 'A latched application PID is required');
  assert.deepEqual([...new Set(listenerPids)].sort((a,b) => a-b), [ownedPid],
    'Debugger port does not belong exclusively to the latched test application PID');
}

export function applicationProcessesForProfile(processOutput, executable, profiles) {
  return processOutput.split('\n').flatMap(line => {
    const match = line.trim().match(/^(\d+)\s+(.*)$/);
    if (!match || !isOwnedApplicationCommand(match[2], executable, profiles)) return [];
    const pid = Number(match[1]);
    assert.ok(Number.isSafeInteger(pid) && pid > 0, 'Invalid application PID');
    return [{ pid, command: match[2] }];
  });
}

export function assertIsolatedProfileAvailable(processOutput, executable, profiles) {
  assert.deepEqual(applicationProcessesForProfile(processOutput, executable, profiles), [],
    'A process already owns this exact test bundle and profile; do not reuse or terminate it');
}

export function nativeComputerCall(input, frame = null) {
  const { action, ...args } = input;
  assert.equal(typeof action, 'string');
  assert.notEqual(action, 'display_size', 'display_size is not a native computer-use tool');
  const point = value => frame && Array.isArray(value) && value.length === 2
    ? [value[0] * frame.frameWidth / frame.sourceWidth, value[1] * frame.frameHeight / frame.sourceHeight]
    : value;
  const adapt = value => {
    const result = { ...value };
    if (Array.isArray(result.coordinate)) result.coordinate = point(result.coordinate);
    if (Array.isArray(result.actions)) result.actions = result.actions.map(adapt);
    return result;
  };
  // Window coordinates belong to app_screenshot, never the display frame.
  const arguments_ = action.startsWith('app_') ? { ...args } : adapt(args);
  if (action === 'open_application') {
    arguments_.app = arguments_.app ?? arguments_.bundle_id;
    delete arguments_.bundle_id;
  }
  return { name: `mcp__computer-use__${action}`, arguments: arguments_ };
}

export function imageDimensions(bytes) {
  if (bytes.subarray(0, 8).equals(Buffer.from([137,80,78,71,13,10,26,10]))) {
    assert.ok(bytes.length >= 24);
    return { width: bytes.readUInt32BE(16), height: bytes.readUInt32BE(20) };
  }
  assert.ok(bytes.length >= 4 && bytes[0] === 0xff && bytes[1] === 0xd8, 'Expected JPEG or PNG image bytes');
  let offset = 2;
  while (offset + 4 <= bytes.length) {
    assert.equal(bytes[offset], 0xff, 'Malformed JPEG marker');
    while (bytes[offset] === 0xff) offset++;
    const marker = bytes[offset++];
    if (marker === 0xd9 || marker === 0xda) break;
    if (marker === 0x01 || marker >= 0xd0 && marker <= 0xd7) continue;
    const size = bytes.readUInt16BE(offset);
    assert.ok(size >= 2 && offset + size <= bytes.length, 'Malformed JPEG segment');
    if ([0xc0,0xc1,0xc2,0xc3,0xc5,0xc6,0xc7,0xc9,0xca,0xcb,0xcd,0xce,0xcf].includes(marker)) {
      assert.ok(size >= 7);
      const height = bytes.readUInt16BE(offset + 3), width = bytes.readUInt16BE(offset + 5);
      assert.ok(width > 0 && height > 0);
      return { width, height };
    }
    offset += size;
  }
  throw new Error('Image dimensions were not present in JPEG frame');
}

export function decodeComputerProtocolResult(raw, { geometry, filtering, scope = 'display' } = {}) {
  const parsed = typeof raw === 'string' ? JSON.parse(raw) : raw;
  // Native ToolCallResult.data is the content array. MCP/provider receipt
  // envelopes may retain it under content or data.content respectively.
  const blocks = Array.isArray(parsed) ? parsed : parsed?.content ?? parsed?.data?.content;
  if (!Array.isArray(blocks)) return parsed;
  const images = blocks.filter(block => block.type === 'image');
  if (images.length) {
    assert.equal(images.length, 1, 'Screenshot acceptance expects exactly one image');
    const block = images[0];
    const encoded = block.data ?? (block.source?.type === 'base64' ? block.source.data : undefined);
    const mimeType = block.mimeType ?? block.source?.media_type;
    assert.ok(typeof encoded === 'string' && ['image/jpeg', 'image/png'].includes(mimeType), 'Unsupported image receipt');
    const dimensions = imageDimensions(Buffer.from(encoded, 'base64'));
    const message = blocks.filter(block => block.type === 'text').map(block => block.text).join('\n');
    if (scope === 'app') {
      return { type: 'image', file: { base64: encoded, type: mimeType },
        width: dimensions.width, height: dimensions.height, captureScope: 'app', message,
        acceptanceGeometrySource: 'dimensions read from actual returned window image' };
    }
    assert.ok(geometry?.width > 0 && geometry?.height > 0, 'The controlled fixture must establish source display geometry');
    assert.equal(filtering, 'mask', 'Filtering must be established by the actual native grant response');
    assert.ok(Math.abs(geometry.width * dimensions.height - geometry.height * dimensions.width)
      <= Math.max(geometry.width, geometry.height), 'Screenshot aspect ratio differs from the controlled fixture display');
    return { type: 'image', file: { base64: encoded, type: mimeType },
      width: geometry.width, height: geometry.height, screenshotFiltering: filtering,
      acceptanceGeometrySource: 'controlled native fixture display; frame dimensions read from actual returned image',
      frame: { sourceWidth: geometry.width, sourceHeight: geometry.height,
        frameWidth: dimensions.width, frameHeight: dimensions.height } };
  }
  const texts = blocks.filter(block => block.type === 'text').map(block => block.text);
  if (texts.length === 1) {
    try { return JSON.parse(texts[0]); } catch { /* Plain tool text is an expected result. */ }
  }
  return { content: blocks, message: texts.join('\n') };
}

export function axElementIndex(summary, role, titleContains = '') {
  const matches = summary.split('\n').flatMap(line => {
    const match = line.match(/^\[(\d+)\] (AX[A-Za-z]+)(?=[\s(])/);
    return match && match[2] === role && line.toLocaleLowerCase('en-US').includes(titleContains.toLocaleLowerCase('en-US'))
      ? [Number(match[1])] : [];
  });
  assert.equal(matches.length, 1, `The controlled fixture must expose exactly one matching ${role}`);
  return matches[0];
}

export function assertBackgroundSurfaceUnchanged(before, after, expectedFrontmost) {
  for (const sample of [before, after]) {
    assert.ok(Number.isSafeInteger(sample.foregroundEventCount) && sample.foregroundEventCount >= 0);
    assert.ok(Number.isSafeInteger(sample.mouseMoveEventCount) && sample.mouseMoveEventCount >= 0);
    assert.ok(Array.isArray(sample.cursor) && sample.cursor.length === 2 && sample.cursor.every(Number.isFinite));
  }
  assert.equal(before.frontmostBundleId, expectedFrontmost, 'The controlled comparison app must be foreground before the background call');
  assert.equal(after.frontmostBundleId, expectedFrontmost, 'Background tool changed the foreground application');
  assert.equal(after.foregroundEventCount, before.foregroundEventCount, 'Foreground activation occurred during the background call');
  assert.deepEqual(after.cursor, before.cursor, 'Background tool moved the host cursor');
  assert.equal(after.mouseMoveEventCount, before.mouseMoveEventCount, 'Mouse-move events occurred during the background call');
}

export function grantedApplicationIds(result) {
  return (result.granted ?? []).map(app => typeof app === 'string' ? app : app.bundleId);
}

export function redactComputerProtocolResult(value) {
  if (Array.isArray(value)) return value.map(redactComputerProtocolResult);
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key,
    key === 'base64' || key === 'data' && (value.type === 'image' || value.type === 'base64')
      ? '[omitted from receipt]' : redactComputerProtocolResult(item)]));
}
