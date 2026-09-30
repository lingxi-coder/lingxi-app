import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { toolInputDetail, toolInputPreview } from '../src/toolview.js';

type ToolEvent = {kind: string; id: string; input_json: string; expected_preview: string};
type Scenario = {id: string; events: ToolEvent[]; expected_groups: {ids: string[]}[];
  permission?: {input_json: string; expected_detail: string}};
const fixture: {version: number; scenarios: Scenario[]} = JSON.parse(
  readFileSync(new URL('../fixtures/native-conversation-parity.json', import.meta.url), 'utf8'));

test('native fixture covers required scenarios with unambiguous tool identities', () => {
  assert.equal(fixture.version, 1);
  assert.deepEqual(fixture.scenarios.map(scenario => scenario.id), [
    'tools_running', 'tools_settled', 'tools_error', 'tools_cancel', 'hidden_reasoning_grouping',
    'permission', 'agent_updates', 'history_session_switch',
  ]);
  for (const scenario of fixture.scenarios) {
    const tools = scenario.events.filter(event => event.kind === 'tool');
    assert.equal(new Set(tools.map(tool => tool.id)).size, tools.length);
    assert.deepEqual(scenario.expected_groups.flatMap(group => group.ids), tools.map(tool => tool.id));
  }
});

test('shared desktop tool and permission previews consume native fixture inputs', () => {
  for (const scenario of fixture.scenarios) {
    for (const event of scenario.events) {
      if (event.kind === 'tool') assert.equal(toolInputPreview(event.input_json), event.expected_preview);
    }
    if (scenario.permission) assert.equal(toolInputDetail(scenario.permission.input_json), scenario.permission.expected_detail);
  }
});
