import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import { transcriptRows } from '../src/renderer/components/transcriptRows';
import type { RunItem, ToolRunItem } from '../src/renderer/model/runItem';

type Scenario = {
  id: string;
  events: { kind: string; id: string; name?: string; status?: string; text?: string }[];
  expected_groups: { ids: string[]; active_ids: string[]; summary: string }[];
};
const fixture: { scenarios: Scenario[] } = JSON.parse(readFileSync(
  new URL('../../shared/fixtures/native-conversation-parity.json', import.meta.url), 'utf8',
));

for (const scenario of fixture.scenarios) {
  test(`Desktop consumes shared native transcript scenario: ${scenario.id}`, () => {
    const items: RunItem[] = scenario.events.map((event): RunItem => {
      if (event.kind === 'reasoning') {
        return { type: 'thinking', id: event.id, text: event.text ?? '', done: true, streamed: true };
      }
      const status: ToolRunItem['status'] = event.status === 'running' ? 'running'
        : event.status === 'failed' || event.status === 'cancelled' ? 'error' : 'done';
      return {
        type: 'tool', id: event.id, tool: event.name!, status,
        view: { verb: event.name === 'Bash' ? 'exec' : 'read', label: event.name!, title: event.name! },
      };
    });
    const groups = transcriptRows(items, items.some(item => item.type === 'tool' && item.status === 'running'))
      .filter(row => row.type === 'tool-group');
    assert.deepEqual(groups.map(group => ({
      ids: group.tools.map(tool => tool.id),
      active_ids: group.tools.filter(tool => tool.status === 'running').map(tool => tool.id),
      summary: group.tools.at(-1)?.tool,
    })), scenario.expected_groups.map(({ ids, active_ids, summary }) => ({ ids, active_ids, summary })));
  });
}
