import type { RunItem } from '../model/runItem';

// File-based skills include this host-generated preamble before the document.
// Accept both the original directory line and the current asset-resolution line;
// the document must still match the adjacent successful tool's body.
const SKILL_DIRECTORY_PREAMBLE = /^Base directory for this skill: [^\n]*\n(?:Resolve relative references and assets from this directory; use their exact paths instead of searching the application home directory\. Use read-only file tools for skill assets; keep project writes in the project directory\.\n)?\n/;
const normalizedText = (text: string) => text.replace(/\r\n?/g, '\n');

/** Keep host asset-resolution metadata out of the readable skill document. */
export function skillInstructionText(text: string): string {
  return normalizedText(text).replace(SKILL_DIRECTORY_PREAMBLE, '').trim();
}

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
        ? normalizedText(output.body).trim() : '';
      const text = skillInstructionText(item.text);
      const knownLocalApp = skill.view.primary === 'local-app' && /^# (?:Desktop Local Apps|Local apps)\n/.test(text);
      if ((body && (text === body || text.startsWith(`${body}\n`))) || knownLocalApp) {
        ids.set(item.id, skill.view.primary || skill.view.label);
      }
    }
    skill = undefined;
  }
  return ids;
}
