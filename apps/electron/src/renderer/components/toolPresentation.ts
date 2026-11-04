import type { ToolRunItem } from '../model/runItem';
import { toolDisplayHeader } from '../model/runItem';
import { tr } from '../i18n/locale';

/** Only adjacent calls with the same meaning share a disclosure. */
export function transcriptToolKey(item: ToolRunItem): string | undefined {
  if (item.agentId || /^(Agent|Task|Skill|spawn_agent|fork_agent|AskUserQuestion|ExitPlanMode|EnterPlanMode|SubmitPlan)$/i.test(item.tool)
    || /permission/i.test(`${item.tool} ${item.view.label}`)
    || ['task', 'todo', 'skill'].includes(item.view.verb)) return undefined;
  if (/^(?:mcp__|mcp[.:/])/i.test(item.tool)) return item.tool;
  if (['read', 'update', 'create', 'shell', 'search', 'fetch'].includes(item.view.verb)) return item.view.verb;
  return item.tool || undefined;
}

const PRESENTED_VERBS = new Set(['read', 'update', 'create', 'shell', 'search', 'fetch']);

/** Change presentation using the engine header; never reconstruct it from raw input. */
export function toolRowPresentation(item: ToolRunItem) {
  const view = toolDisplayHeader(item);
  const argument = view.primary || (view.verb === 'shell' ? view.sub_line?.text : undefined);
  if (!PRESENTED_VERBS.has(view.verb) || !argument) return null;
  const file = ['read', 'update', 'create'].includes(view.verb);
  return {
    action: tr(`desktop_ui_tool_${view.verb}_${item.status}`),
    argument,
    displayArgument: file ? argument.split(/[\\/]/).filter(Boolean).at(-1) || argument : argument,
    file,
    qualifier: view.qualifier,
  };
}

/** Neutral activity labels do not imply that a failed or running call succeeded. */
export function toolGroupSummary(tools: readonly ToolRunItem[]): string {
  const groups: { key: string; first: ToolRunItem; count: number }[] = [];
  for (const item of tools) {
    const key = transcriptToolKey(item) ?? item.id;
    const last = groups.at(-1);
    if (last?.key === key) last.count++;
    else groups.push({ key, first: item, count: 1 });
  }
  return groups.map(({ first, count }) => {
    const label = ['read', 'update', 'create', 'shell', 'search', 'fetch'].includes(first.view.verb)
      ? tr(`desktop_ui_tool_group_${first.view.verb}`) : first.view.label;
    return `${label} · ${tr(count === 1 ? 'desktop_ui_tool_call_one' : 'desktop_ui_tool_call_count', count)}`;
  }).join(' / ');
}
