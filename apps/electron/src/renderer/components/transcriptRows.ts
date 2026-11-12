import type { NarrationRunItem, RunItem, ToolRunItem } from '../model/runItem';
import { localAppReference } from '../model/localAppReference';
import { localAppToolRequiresCode } from '../bridge/localAppErrors';
import { transcriptToolKey } from './toolPresentation';
import { skillInstructionNames } from './localAppInstructionPresentation';

export interface TranscriptToolGroup {
  readonly type: 'tool-group';
  readonly id: string;
  readonly tools: ToolRunItem[];
}

interface TranscriptSkillInstructions extends Omit<NarrationRunItem, 'type' | 'role'> {
  readonly type: 'skill-instructions';
  readonly name: string;
}

export type TranscriptRow = Exclude<RunItem, ToolRunItem> | TranscriptToolGroup | TranscriptSkillInstructions;

/** Adjacent, related tools share a group; prose and deliverables retain their place. */
export function transcriptRows(items: readonly RunItem[], running: boolean, hasPendingActivity = false, includeThinking = false): TranscriptRow[] {
  const rows: TranscriptRow[] = [];
  const instructions = skillInstructionNames(items);
  let group: TranscriptToolGroup | undefined;
  // Only the latest unfinished reasoning block can be active.
  let activeThinking: RunItem | undefined;
  if (running) {
    for (let index = items.length - 1; index >= 0; index--) {
      const item = items[index]!;
      if (item.type === 'thinking' && !item.done && item.streamed) { activeThinking = item; break; }
    }
  }
  for (const item of items) {
    if (item.type === 'thinking') {
      if (includeThinking || item === activeThinking) rows.push(item);
      continue;
    }
    if (item.type === 'tool') {
      // Deliverables and Code-required errors must remain visible when
      // surrounding tool groups collapse.
      if (localAppReference(item) || localAppToolRequiresCode(item.tool, item.status, item.result?.headline, item.result?.body)) {
        rows.push({ type: 'tool-group', id: `tool-group:${item.id}`, tools: [item] });
        group = undefined;
        continue;
      }
      const key = transcriptToolKey(item);
      if (!group || !key || key !== transcriptToolKey(group.tools[0]!)) {
        group = { type: 'tool-group', id: `tool-group:${item.id}`, tools: [] };
        rows.push(group);
      }
      group.tools.push(item);
      continue;
    }
    group = undefined;
    const name = item.type === 'narration' ? instructions.get(item.id) : undefined;
    if (item.type === 'narration' && name !== undefined) {
      const { role: _role, ...document } = item;
      rows.push({ ...document, type: 'skill-instructions', name });
    } else {
      rows.push(item);
    }
  }
  // Some providers never emit reasoning deltas. The turn still owes the user
  // a waiting indicator until it finishes, unless another activity owns it.
  if (running && !hasPendingActivity && !activeThinking && !items.some((item) =>
    (item.type === 'tool' || item.type === 'compaction') && item.status === 'running')) {
    rows.push({ type: 'thinking', id: 'thinking:pending', text: '', streamed: true });
  }
  return rows;
}
