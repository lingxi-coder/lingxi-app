import type { ThinkingRunItem } from '../model/runItem';
import { localAppReference } from '../model/localAppReference';
import { localAppToolRequiresCode } from '../bridge/localAppErrors';
import { transcriptToolKey } from './toolPresentation';
import type { AgentTranscriptRow } from './transcriptAgentPlacement';
import type { TranscriptToolGroup } from './transcriptRows';

export interface TranscriptActivity extends TranscriptToolGroup {
  readonly activity: true;
  readonly thoughts: ThinkingRunItem[];
  active: boolean;
}

export function isTranscriptActivity(row: AgentTranscriptRow): row is TranscriptActivity {
  return row.type === 'tool-group' && 'activity' in row;
}

export function sameTranscriptActivity(first: TranscriptToolGroup, second: TranscriptToolGroup): boolean {
  if (!isTranscriptActivity(first)) return !isTranscriptActivity(second);
  if (!isTranscriptActivity(second)) return false;
  return first.active === second.active && first.thoughts.length === second.thoughts.length
    && first.thoughts.every((thought, index) => thought === second.thoughts[index]);
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
  for (const row of rows) {
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
