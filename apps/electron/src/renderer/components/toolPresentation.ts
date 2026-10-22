import type { ToolRunItem } from '../model/runItem';
import { toolDisplayHeader } from '../model/runItem';

const actions: Record<string, [string, string]> = {
  read: ['Reading', 'Read'], update: ['Editing', 'Edited'], create: ['Creating', 'Created'],
  shell: ['Running', 'Ran'], search: ['Searching', 'Searched'], fetch: ['Fetching', 'Fetched'],
};

/** Change presentation using the engine header; never reconstruct it from raw input. */
export function toolRowPresentation(item: ToolRunItem) {
  const view = toolDisplayHeader(item);
  const action = actions[view.verb];
  const argument = view.primary || (view.verb === 'shell' ? view.sub_line?.text : undefined);
  if (!action || !argument) return null;
  const file = ['read', 'update', 'create'].includes(view.verb);
  return {
    action: action[item.status === 'running' ? 0 : 1],
    argument,
    displayArgument: file ? argument.split(/[\\/]/).filter(Boolean).at(-1) || argument : argument,
    file,
    qualifier: view.qualifier,
  };
}

/** A compact account of all calls, in the order their activity first appeared. */
export function toolGroupSummary(tools: readonly ToolRunItem[]): string {
  const counts = new Map<string, number>();
  for (const item of tools) counts.set(item.view.verb, (counts.get(item.view.verb) ?? 0) + 1);
  const phrases = [...counts].map(([verb, count]) => {
    const files = count === 1 ? 'a file' : 'files';
    switch (verb) {
      case 'read': return `read ${files}`;
      case 'update': return `edited ${files}`;
      case 'create': return `created ${files}`;
      case 'shell': return `ran ${count === 1 ? 'a command' : 'commands'}`;
      case 'search': return count === 1 ? 'searched' : 'ran searches';
      case 'fetch': return `fetched ${count === 1 ? 'a page' : 'pages'}`;
      default: return tools.find(item => item.view.verb === verb)!.view.label;
    }
  });
  const summary = phrases.slice(0, 3).join(', ') + (phrases.length > 3 ? ` and ${phrases.length - 3} more` : '');
  return summary ? summary[0]!.toUpperCase() + summary.slice(1) : '';
}
