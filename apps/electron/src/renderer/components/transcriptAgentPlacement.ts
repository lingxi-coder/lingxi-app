import type { SessionAgentSummaryDto } from '@lingxi/bridge-client';
import type { RunItem } from '../model/runItem';
import type { TranscriptRow } from './transcriptRows';

/**
 * The spawned agent named by `tool`'s result, or `undefined` for every other
 * tool.
 *
 * Gating on the name before parsing is what keeps this off the hot path: only a
 * spawning tool ever sets `agentId`, while a restored transcript holds
 * thousands of tool results whose payloads are the largest strings in it. A
 * result with no paired call lowers with an EMPTY tool name, which is correct
 * to skip — {@link anchorTranscriptAgents} could not have matched it either.
 */
export function spawnResultAgentId(tool: string, json: string): string | undefined {
  return SPAWNING_TOOL.test(tool) ? resultAgentId(json) : undefined;
}

/** Read identity from structured tool metadata, never from user-facing copy. */
export function resultAgentId(json: string): string | undefined {
  try {
    const value = JSON.parse(json);
    const id = value?.agentId ?? value?.agent_id;
    return typeof id === 'string' && id.length > 0 ? id : undefined;
  } catch { return undefined; }
}

export type AgentAnchors = ReadonlyMap<string, string | null>;

/**
 * Every tool that can spawn an agent. Hoisted: a literal here would be
 * re-constructed once per transcript item.
 *
 * `Skill` belongs here with `Agent`/`Task`: a forked skill spawns through the
 * same `SubagentSpawner::spawn_async` seam and reports the new agent the same
 * way (`tools/skill/src/skill.rs` -> `fork::fork_result`). Those two are the
 * whole set — they are the only `spawn_async` callers under `tools/`.
 * `spawn_agent`/`fork_agent` are agent-DEFINITION names, never tool names, and
 * are kept only because an older transcript may carry them.
 */
const SPAWNING_TOOL = /^(Agent|Task|Skill|spawn_agent|fork_agent)$/i;

/**
 * One agent, one key — the bare uuid.
 *
 * Both spellings are live, so neither side can be trusted to be canonical: the
 * wire `agent_id` is always the prefixed `agent:<uuid>` (`protocol/src/ids.rs`
 * `Display`), while a spawn result reports whichever form its tool happened to
 * build — `tools/agent/src/agent.rs` writes the BARE uuid (`as_uuid()`, the id
 * the MODEL hands back to `SendMessage`), `tools/skill/src/skill.rs` writes the
 * prefixed one (`agent_id.to_string()`). Comparing raw spellings therefore
 * never matched for `Agent`, so EVERY agent fell through to the "wherever the
 * transcript currently ends" fallback below — which looks correct while the
 * spawning call is still the last row, and puts the whole set at the bottom
 * once a restart replays the full history. The engine normalizes the same way
 * (`AgentId::parse_prefixed` accepts both).
 */
function agentKey(id: string): string {
  return id.startsWith('agent:') ? id.slice('agent:'.length) : id;
}

export function anchorTranscriptAgents(previous: AgentAnchors, items: readonly RunItem[], agents: readonly SessionAgentSummaryDto[]): AgentAnchors {
  const live = new Set(items.map(item => item.id));
  // Carry forward only anchors whose row still exists. An anchor is a real tool
  // id now, not always the last item, so a transcript that no longer holds it
  // (a rewind, or a spawn call that fell outside a compacted window) would
  // resolve to `null` — which `placeTranscriptAgents` renders at the very TOP.
  const anchors = new Map([...previous].filter(([, anchor]) => anchor === null || live.has(anchor)));
  const creation = new Map<string, string>();
  for (const item of items) {
    if (item.type === 'tool' && SPAWNING_TOOL.test(item.tool) && item.agentId && !creation.has(agentKey(item.agentId))) {
      creation.set(agentKey(item.agentId), item.id);
    }
  }
  for (const agent of agents) {
    if (agent.agent_id === 'main') continue;
    const id = creation.get(agentKey(agent.agent_id));
    // No creation site is normal, not exceptional: a Fusion run reports no
    // single `agentId` by design, a `/`-typed background fork leaves no tool
    // call in the transcript at all, and a transcript predating the
    // `toolUseResult` stamp has nothing to recover. Those keep the fallback.
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
