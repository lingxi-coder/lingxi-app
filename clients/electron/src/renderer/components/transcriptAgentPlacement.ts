import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import type { RunItem } from '../model/runItem';
import type { TranscriptRow } from './transcriptRows';

/** Read identity from structured tool metadata, never from user-facing copy. */
export function resultAgentId(json: string): string | undefined {
  try {
    const value = JSON.parse(json);
    const id = value?.agentId ?? value?.agent_id;
    return typeof id === 'string' && id.length > 0 ? id : undefined;
  } catch { return undefined; }
}

export type AgentAnchors = ReadonlyMap<string, string | null>;

/** Hoisted: a literal here would be re-constructed once per transcript item. */
const SPAWNING_TOOL = /^(Agent|Task|spawn_agent|fork_agent)$/i;

export function anchorTranscriptAgents(previous: AgentAnchors, items: readonly RunItem[], agents: readonly SessionAgentSummaryDto[]): AgentAnchors {
  const anchors = new Map(previous);
  const creation = new Map<string, string>();
  for (const item of items) {
    if (item.type === 'tool' && SPAWNING_TOOL.test(item.tool) && item.agentId && !creation.has(item.agentId)) {
      creation.set(item.agentId, item.id);
    }
  }
  for (const agent of agents) {
    if (agent.agent_id === 'main') continue;
    const id = creation.get(agent.agent_id);
    if (id) anchors.set(agent.agent_id, id);
    else if (!anchors.has(agent.agent_id)) anchors.set(agent.agent_id, items.at(-1)?.id ?? null);
  }
  return anchors;
}

export type AgentTranscriptRow = TranscriptRow | { type: 'agents'; id: string; agents: SessionAgentSummaryDto[] };

export function placeTranscriptAgents(inputRows: readonly TranscriptRow[], items: readonly RunItem[], agents: readonly SessionAgentSummaryDto[], anchors: AgentAnchors): AgentTranscriptRow[] {
  // A successful launch receipt adds no information once its agent card exists.
  // Keep the stored event, unmatched receipts, errors, and any additional output.
  const rows = inputRows.filter(row => {
    if (row.type !== 'command' || row.isError) return true;
    const receipt = /^⍼ started [^\r\n]+ in background as (\S+) \(([^()\s]+)\)$/.exec(row.output.trim());
    if (!receipt) return true;
    return !agents.some(agent => agent.agent_id !== 'main'
      && agent.name === receipt[1] && agent.agent_id.endsWith(receipt[2]!));
  });
  const groups = new Map<string | null, SessionAgentSummaryDto[]>();
  const visible = new Set(rows.flatMap(row => row.type === 'tool-group' ? row.tools.map(tool => tool.id) : [row.id]));
  // Folded/hidden rows resolve to their preceding visible boundary.
  const boundaries = new Map<string, string | null>();
  let boundary: string | null = null;
  for (const item of items) {
    if (visible.has(item.id)) boundary = item.id;
    boundaries.set(item.id, boundary);
  }
  for (const [id, anchor] of anchors) {
    const agent = agents.find(candidate => candidate.agent_id === id);
    if (!agent || id === 'main') continue;
    const target = anchor === null ? null : boundaries.get(anchor) ?? null;
    const group = groups.get(target) ?? [];
    group.push(agent);
    groups.set(target, group);
  }
  const result: AgentTranscriptRow[] = [];
  const insert = (id: string | null) => {
    const group = groups.get(id);
    if (group) result.push({ type: 'agents', id: `agents:${id ?? 'start'}`, agents: group });
  };
  insert(null);
  for (const row of rows) {
    if (row.type === 'tool-group') {
      let tools: typeof row.tools = [];
      for (const tool of row.tools) {
        tools.push(tool);
        if (groups.has(tool.id)) {
          result.push({ ...row, id: `tool-group:${tools[0]!.id}`, tools });
          tools = [];
          insert(tool.id);
        }
      }
      if (tools.length) result.push({ ...row, id: `tool-group:${tools[0]!.id}`, tools });
    } else {
      result.push(row);
      insert(row.id);
    }
  }
  return result;
}
