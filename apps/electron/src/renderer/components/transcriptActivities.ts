import type { ThinkingRunItem, ToolRunItem } from '../model/runItem';
import { localAppReference } from '../model/localAppReference';
import { localAppToolRequiresCode } from '../bridge/localAppErrors';
import { transcriptToolKey } from './toolPresentation';
import type { AgentTranscriptRow } from './transcriptAgentPlacement';
import type { TranscriptSkillInstructions, TranscriptToolGroup } from './transcriptRows';

export interface TranscriptActivity extends TranscriptToolGroup {
  readonly activity: true;
  readonly thoughts: ThinkingRunItem[];
  readonly skill?: {
    readonly name: string;
    readonly items: readonly (ToolRunItem | ThinkingRunItem | TranscriptSkillInstructions)[];
  };
  active: boolean;
}

export function isTranscriptActivity(row: AgentTranscriptRow): row is TranscriptActivity {
  return row.type === 'tool-group' && 'activity' in row;
}

export function toolGroupSourceIds(group: TranscriptToolGroup): string[] {
  if (!isTranscriptActivity(group)) return group.tools.map(tool => tool.id);
  if (group.skill) return group.skill.items.map(item => item.id);
  return [...group.tools.map(tool => tool.id), ...group.thoughts.map(thought => thought.id)];
}

export function sameTranscriptActivity(first: TranscriptToolGroup, second: TranscriptToolGroup): boolean {
  if (!isTranscriptActivity(first)) return !isTranscriptActivity(second);
  if (!isTranscriptActivity(second)) return false;
  return first.active === second.active && first.thoughts.length === second.thoughts.length
    && first.thoughts.every((thought, index) => thought === second.thoughts[index]);
}

function skillInvocation(rows: readonly AgentTranscriptRow[], index: number, standaloneToolIds: ReadonlySet<string>) {
  const row = rows[index];
  if (row?.type !== 'tool-group' || row.tools.length !== 1) return undefined;
  const tool = row.tools[0]!;
  if (transcriptToolKey(tool) !== 'skill' || tool.status !== 'done' || standaloneToolIds.has(tool.id)) return undefined;
  let end = index + 1;
  const thoughts: ThinkingRunItem[] = [];
  for (let next = rows[end]; next?.type === 'thinking'; next = rows[++end]) {
    thoughts.push(next);
  }
  const instructions = rows[end];
  if (instructions?.type !== 'skill-instructions' || instructions.name !== (tool.view.primary || tool.view.label)) return undefined;
  return { tool, thoughts, instructions, end };
}

/** One stable presentation row per uninterrupted activity, including its initial wait. */
export function transcriptActivities(rows: readonly AgentTranscriptRow[], running: boolean, standaloneToolIds: ReadonlySet<string> = new Set()): AgentTranscriptRow[] {
  const result: AgentTranscriptRow[] = [];
  let activity: TranscriptActivity | undefined;
  const current = () => {
    if (!activity) {
      activity = { type: 'tool-group', activity: true, id: `activity:after:${result.at(-1)?.id ?? 'start'}`,
        tools: [], thoughts: [], active: false };
      result.push(activity);
    }
    return activity;
  };
  for (let index = 0; index < rows.length; index++) {
    const row = rows[index]!;
    const invocation = skillInvocation(rows, index, standaloneToolIds);
    if (invocation) {
      const { tool, thoughts, instructions, end } = invocation;
      const tools = [tool];
      const items: NonNullable<TranscriptActivity['skill']>['items'][number][] = [tool, ...thoughts, instructions];
      let last = end;
      // Completed reasoning between invocations belongs inside the same disclosure.
      while (last + 1 < rows.length) {
        let nextIndex = last + 1;
        const between: ThinkingRunItem[] = [];
        for (let next = rows[nextIndex]; next?.type === 'thinking' && next.done; next = rows[++nextIndex]) {
          between.push(next);
        }
        const next = skillInvocation(rows, nextIndex, standaloneToolIds);
        if (!next || next.instructions.name !== instructions.name) break;
        tools.push(next.tool);
        items.push(...between, next.tool, ...next.thoughts, next.instructions);
        last = next.end;
      }
      if (tools.length > 1) {
        activity = undefined;
        const group: TranscriptActivity = { type: 'tool-group', activity: true, id: `skill-group:${tool.id}`,
          tools, thoughts: [], active: false, skill: { name: instructions.name, items } };
        result.push(group);
        index = last;
        continue;
      }
    }
    if (row.type === 'audio' || (row.type === 'meta' && !row.files?.length)) continue;
    if (row.type === 'thinking') {
      // The streamed answer is its own progress indicator. Do not create a new
      // empty activity below it just because a provider has no reasoning deltas.
      const previous = result.at(-1);
      if (row.id === 'thinking:pending' && !activity && previous?.type === 'narration' && previous.role === 'assistant') continue;
      const group = current();
      if (row.id !== 'thinking:pending') group.thoughts.push(row);
      group.active ||= running && !row.done;
      continue;
    }
    if (row.type === 'tool-group' && row.tools.every(tool => !standaloneToolIds.has(tool.id) && transcriptToolKey(tool)
      && !localAppReference(tool) && !localAppToolRequiresCode(tool.tool, tool.status, tool.result?.headline, tool.result?.body))) {
      const group = current();
      group.tools.push(...row.tools);
      group.active ||= running && row.tools.some(tool => tool.status === 'running');
      continue;
    }
    activity = undefined;
    result.push(row);
  }
  return result;
}
