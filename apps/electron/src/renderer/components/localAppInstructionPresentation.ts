import type { RunItem } from '../model/runItem';

/** Recognize an injected skill prompt using its adjacent tool result as evidence. */
export function skillInstructionNames(items: readonly RunItem[]): ReadonlyMap<string, string> {
  const ids = new Map<string, string>();
  let skill: Extract<RunItem, { type: 'tool' }> | undefined;
  for (const item of items) {
    if (item.type === 'thinking') continue;
    if (item.type === 'tool') {
      skill = item.status === 'done' && item.view.verb.toLowerCase() === 'skill' ? item : undefined;
      continue;
    }
    if (skill && item.type === 'narration' && item.role === 'user' && !item.images?.length && !item.delivery) {
      const output = skill.nativeOutput;
      const body = output && typeof output === 'object' && !Array.isArray(output) && typeof output.body === 'string'
        ? output.body.trim() : '';
      const text = item.text.replace(/^Base directory for this skill: [^\n]*\r?\n\r?\n/, '').trim();
      const knownLocalApp = skill.view.primary === 'local-app' && /^# Desktop Local Apps\r?\n/.test(text);
      if ((body && (text === body || text.startsWith(`${body}\n`))) || knownLocalApp) {
        ids.set(item.id, skill.view.primary || skill.view.label);
      }
    }
    skill = undefined;
  }
  return ids;
}
