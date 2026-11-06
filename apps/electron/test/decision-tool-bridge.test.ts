import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { setImmediate } from 'node:timers/promises';
import { DecisionToolBridge, type DesktopDecisionService } from '../src/main/decisionToolBridge.js';
import type { ClientCommand } from '@lingxi/bridge-client';
import type { DecisionResult } from '../src/shared/decision.js';

const result: DecisionResult = { state_revision: 0, answers: { ok: { type: 'binary', probability: 0.8 } }, usage: { input_tokens: null, output_tokens: null }, actual_model: null, elapsed_ms: 1 };
const input = { state: 'evidence', questions: { ok: { type: 'binary', instructions: 'Is it valid?' } } };
function fixture() {
  let configured = false;
  const listeners = new Set<() => void>();
  const calls: AbortSignal[] = [];
  const releases: Array<(value: DecisionResult) => void> = [];
  const commands: ClientCommand[] = [];
  const service: DesktopDecisionService = {
    isConfigured: async () => configured,
    onConfigurationChanged: listener => { listeners.add(listener); return () => listeners.delete(listener); },
    evaluateTool: async (_input, signal) => { calls.push(signal); return new Promise(resolve => releases.push(resolve)); },
  };
  const bridge = new DecisionToolBridge(service, command => commands.push(command));
  return { bridge, calls, releases, commands,
    changed(value = configured) { configured = value; for (const listener of listeners) listener(); },
    request(id = randomUUID()) { bridge.handle({ type: 'decision_request', request_id: id, input_json: JSON.stringify(input), timeout_ms: 30_000 }); return id; },
  };
}

test('advertises only capability transitions; backend edits preserve prompt metadata', async () => {
  const f = fixture();
  await f.bridge.refreshConfiguration(); await f.bridge.refreshConfiguration();
  assert.deepEqual(f.commands, [{ type: 'decision_configuration_changed', configured: false }]);
  f.changed(true); await setImmediate();
  f.changed(); await setImmediate(); // key/model/endpoint change while still configured
  assert.deepEqual(f.commands, [{ type: 'decision_configuration_changed', configured: false }, { type: 'decision_configuration_changed', configured: true }]);
  f.changed(false); await setImmediate(); assert.equal(f.commands.length, 3);
  f.bridge.dispose(); f.changed(true); await setImmediate(); assert.equal(f.commands.length, 3);
});

test('routes one foreground call once and returns only its matching normalized result', async () => {
  const f = fixture(); const id = f.request(); f.request(id);
  assert.equal(f.calls.length, 1); f.releases[0]!(result); await setImmediate();
  assert.deepEqual(f.commands, [{ type: 'decision_response', request_id: id, result_json: JSON.stringify({ result }) }]);
  f.request(id); assert.equal(f.calls.length, 1); f.bridge.dispose();
});

test('targeted cancel and connection teardown abort work and discard late results', async () => {
  const f = fixture(); const first = f.request(); const second = f.request();
  f.bridge.handle({ type: 'decision_cancel', request_id: first });
  assert.equal(f.calls[0]!.aborted, true); assert.equal(f.calls[1]!.aborted, false);
  f.releases[0]!(result); await setImmediate(); assert.equal(f.commands.length, 0);
  f.bridge.dispose(); assert.equal(f.calls[1]!.aborted, true);
  f.releases[1]!(result); await setImmediate(); assert.equal(f.commands.length, 0);
  f.bridge.handle({ type: 'decision_cancel', request_id: second });
});

test('rejects endpoint overrides and invalid budgets without invoking the service', async () => {
  const f = fixture();
  f.bridge.handle({ type: 'decision_request', request_id: randomUUID(), input_json: JSON.stringify({ ...input, endpoint: 'https://other.invalid' }), timeout_ms: 30_000 });
  f.bridge.handle({ type: 'decision_request', request_id: randomUUID(), input_json: JSON.stringify(input), timeout_ms: 60_000 });
  await setImmediate(); assert.equal(f.calls.length, 0); assert.equal(f.commands.length, 2);
  f.bridge.dispose();
});
