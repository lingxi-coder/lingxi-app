import type { RunItem } from '../model/runItem';

/** Recognize the desktop skill's injected companion, never a standalone heading. */
export function localAppInstructionIds(items: readonly RunItem[]): ReadonlySet<string> {
  const ids = new Set<string>();
  let awaitingBody = false;
  for (const item of items) {
    if (item.type === 'thinking') continue;
    if (item.type === 'tool') {
      awaitingBody = item.status === 'done' && item.view.verb === 'skill'
        && item.view.primary === 'local-app';
      continue;
    }
    if (awaitingBody && item.type === 'narration' && item.role === 'user'
      && !item.images?.length && !item.delivery
      && /^# Desktop Local Apps\r?\n/.test(item.text.trimStart())) {
      ids.add(item.id);
    }
    awaitingBody = false;
  }
  return ids;
}
