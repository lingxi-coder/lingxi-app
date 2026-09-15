import type { RunItem, TurnFileChange } from './runItem';

/** Snapshot only confirmed edits owned by this turn, never other transcript tools. */
export function collectTurnFileChanges(items: readonly RunItem[], toolIds: readonly string[]): TurnFileChange[] {
  const current = new Set(toolIds);
  const files = new Map<string, TurnFileChange>();
  for (const item of items) {
    if (item.type !== 'tool' || item.status !== 'done' || !current.delete(item.id)) continue;
    const diff = item.result?.diff;
    const path = diff?.file_path?.trim();
    if (!diff || !path || (diff.additions === 0 && diff.removals === 0)) continue;
    const previous = files.get(path);
    // Keep each sequential edit for review, detached from mutable wire payloads.
    const snapshot = structuredClone(diff);
    files.set(path, {
      path,
      additions: (previous?.additions ?? 0) + diff.additions,
      removals: (previous?.removals ?? 0) + diff.removals,
      diffs: [...(previous?.diffs ?? []), snapshot],
    });
  }
  return [...files.values()];
}
