import type { RunItem, ToolRunItem } from '../model/runItem';

export interface TranscriptToolGroup {
  readonly type: 'tool-group';
  readonly id: string;
  readonly tools: ToolRunItem[];
}

export type TranscriptRow = Exclude<RunItem, ToolRunItem> | TranscriptToolGroup;

/** Presentation only: thinking never divides tools, other events retain boundaries. */
export function transcriptRows(items: readonly RunItem[], running: boolean): TranscriptRow[] {
  const rows: TranscriptRow[] = [];
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
      if (item === activeThinking) rows.push(item);
      continue;
    }
    if (item.type === 'tool') {
      if (!group) {
        group = { type: 'tool-group', id: `tool-group:${item.id}`, tools: [] };
        rows.push(group);
      }
      group.tools.push(item);
      continue;
    }
    group = undefined;
    rows.push(item);
  }
  // Some providers never emit reasoning deltas. The turn still owes the user
  // a waiting indicator until it finishes, unless another activity owns it.
  if (running && !activeThinking && !items.some((item) =>
    (item.type === 'tool' || item.type === 'compaction') && item.status === 'running')) {
    rows.push({ type: 'thinking', id: 'thinking:pending', text: '', streamed: true });
  }
  return rows;
}
