import type { ToolRunItem } from '../model/runItem';
import { toolDisplayHeader } from '../model/runItem';
import { tr } from '../i18n/locale';

/** Task bookkeeping is distinct from launching an agent or stopping a process. */
export function isTaskProgressTool(tool: string): boolean {
  return /^(?:Task_?(?:Create|Get|Update|List)|Todo_?Write|List_?Checks|update_plan)$/i.test(tool);
}

const isTaskMessageTool = (tool: string) => /^(?:SendMessage|send_message)$/i.test(tool);

/** Discovery joins the workflow only when every requested tool belongs to it. */
export function isTaskWorkflowTool(item: ToolRunItem): boolean {
  if (isTaskProgressTool(item.tool) || isTaskMessageTool(item.tool)) return true;
  if (item.tool !== 'ToolSearch' || !item.view.primary) return false;
  const targets = item.view.primary.trim().replace(/^select:\s*/i, '').split(/[,\s]+/);
  return targets.length > 0 && targets.every((target) => {
    const name = target.replace(/^\+/, '');
    return isTaskProgressTool(name) || isTaskMessageTool(name);
  });
}

/** Only adjacent calls with the same meaning share a disclosure. */
export function transcriptToolKey(item: ToolRunItem): string | undefined {
  if (item.agentId || /^(Agent|Task|Skill|spawn_agent|fork_agent|AskUserQuestion|ExitPlanMode|EnterPlanMode|SubmitPlan)$/i.test(item.tool)
    || /permission/i.test(`${item.tool} ${item.view.label}`)
    || item.view.verb === 'skill') return undefined;
  if (isTaskWorkflowTool(item)) return 'task-workflow';
  if (['task', 'todo'].includes(item.view.verb)) return undefined;
  if (/^(?:mcp__|mcp[.:/])/i.test(item.tool)) return item.tool;
  if (['read', 'update', 'create', 'shell', 'search', 'fetch'].includes(item.view.verb)) return item.view.verb;
  return item.tool || undefined;
}

const PRESENTED_VERBS = new Set(['read', 'update', 'create', 'shell', 'search', 'fetch']);

/** Change presentation using the engine header; never reconstruct it from raw input. */
export function toolRowPresentation(item: ToolRunItem) {
  const view = toolDisplayHeader(item);
  if (item.tool === 'LocalApp') {
    const argument = view.primary || (item.status === 'error' ? undefined : item.result?.headline) || '';
    return { action: tr('chat_local_app_kind'), argument, displayArgument: argument.replace(/\s+/g, ' '), file: false, qualifier: view.qualifier };
  }
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
  const groups: { key: string; first: ToolRunItem; count: number; tasksOnly: boolean }[] = [];
  for (const item of tools) {
    const key = transcriptToolKey(item) ?? item.id;
    const last = groups.at(-1);
    if (last?.key === key) {
      last.count++;
      last.tasksOnly &&= isTaskProgressTool(item.tool);
    } else groups.push({ key, first: item, count: 1, tasksOnly: isTaskProgressTool(item.tool) });
  }
  return groups.map(({ key, first, count, tasksOnly }) => {
    const label = key === 'task-workflow'
      ? tr(tasksOnly ? 'desktop_ui_tool_group_tasks' : 'desktop_ui_tool_group_coordination')
      : ['read', 'update', 'create', 'shell', 'search', 'fetch'].includes(first.view.verb)
      ? tr(`desktop_ui_tool_group_${first.view.verb}`) : first.view.label;
    return `${label} · ${tr(count === 1 ? 'desktop_ui_tool_call_one' : 'desktop_ui_tool_call_count', count)}`;
  }).join(' / ');
}
