import type { RunItem } from '../model/runItem';

const GOAL_CLEAR_TOKENS = new Set(['clear', 'stop', 'off', 'reset', 'none', 'cancel']);

/** Project active Goal state from the command lifecycle already visible in this conversation. */
export function composerGoalState(items: readonly RunItem[]): { active: boolean; objective: string } {
  let objective = "";
  let active = false;
  for (const item of items) {
    if (item.type === 'narration' && item.role === 'user') {
      const match = /^\s*\/goal(?:\s+([\s\S]*))?\s*$/i.exec(item.text);
      const args = match?.[1]?.trim();
      if (args && !item.delivery) {
        active = !GOAL_CLEAR_TOKENS.has(args.toLocaleLowerCase());
        if (active) objective = args;
      }
      continue;
    }
    if (item.type === 'command' && item.name.trim().split(/\s/, 1)[0]?.toLocaleLowerCase() === '/goal') {
      const output = item.output.trim().toLocaleLowerCase();
      if (output.startsWith('goal active:') || output.startsWith('goal set:')) {
        active = !item.isError;
        const reported = item.output.trim().replace(/^goal (?:active|set):\s*/i, '').trim();
        objective = output.startsWith('goal active:')
          ? reported.replace(/ \((?:not yet evaluated|\d+ turns?)\)(?:\nLast check: [\s\S]*)?$/, '')
          : reported;
      }
      else if (
        output.startsWith('no goal set')
        || output.startsWith('goal cleared:')
        || output.includes('only available in trusted workspaces')
        || output.includes("can't run while hooks are restricted")
        || output.startsWith('goal condition is limited')
      ) active = false;
      continue;
    }
    if (
      item.type === 'narration'
      && item.text.includes('Goal cleared after an unrecoverable error')
    ) active = false;
  }
  return { active, objective };
}


export function goalMessageObjective(text: string): string | null {
  const match = /^\s*\/goal\s+([\s\S]+?)\s*$/i.exec(text);
  const objective = match?.[1];
  return objective && !GOAL_CLEAR_TOKENS.has(objective.toLowerCase()) ? objective : null;
}
