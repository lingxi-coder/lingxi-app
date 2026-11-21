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

/** The semantic key keeps interactive tools outside ordinary activity. */
export function transcriptToolKey(item: ToolRunItem): string | undefined {
  if (item.agentId || /^(Agent|Task|spawn_agent|fork_agent|AskUserQuestion|ExitPlanMode|EnterPlanMode|SubmitPlan)$/i.test(item.tool)
    || /permission/i.test(`${item.tool} ${item.view.label}`)) return undefined;
  if (/^Skill$/i.test(item.tool) || item.view.verb === 'skill') return 'skill';
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

/** Ordinary file and command activity shares one row while special tools keep their own boundaries. */
export function transcriptToolGroupingKey(item: ToolRunItem): string | undefined {
  const key = transcriptToolKey(item);
  return key && PRESENTED_VERBS.has(key) ? 'file-and-command-activity' : key;
}

export function isImageInspectionTool(name: string): boolean {
  return /(?:^|[_./:])(?:view_?image|read_?image)(?:$|[_./:])/i.test(name);
}

function summaryKey(item: ToolRunItem): string {
  const key = transcriptToolKey(item);
  if (key === 'task-workflow') return key;
  if (isImageInspectionTool(item.tool)) return 'image';
  return PRESENTED_VERBS.has(item.view.verb) ? item.view.verb : key ?? item.id;
}

/** Show each action once, and use completed wording only for confirmed results. */
export function toolGroupSummary(tools: readonly ToolRunItem[], localize: (key: string) => string = tr): string {
  const groups = new Map<string, { first: ToolRunItem; completed: boolean; tasksOnly: boolean }>();
  for (const tool of tools) {
    const key = summaryKey(tool);
    const group = groups.get(key);
    if (group) {
      group.completed &&= tool.status === 'done';
      group.tasksOnly &&= isTaskProgressTool(tool.tool);
    } else {
      groups.set(key, { first: tool, completed: tool.status === 'done', tasksOnly: isTaskProgressTool(tool.tool) });
    }
  }
  const labels = [...groups].map(([key, { first, completed, tasksOnly }]) => {
    if (key === 'task-workflow') return localize(tasksOnly ? 'chat_tool_group_tasks' : 'chat_tool_group_coordination');
    if (key === 'image' || (completed && PRESENTED_VERBS.has(key))) return localize(`chat_tool_group_${key}${completed ? '_done' : ''}`);
    if (PRESENTED_VERBS.has(key)) return localize(`chat_tool_group_${key}`);
    return first.view.label || first.tool;
  });
  return [...new Set(labels)].join(', ');
}
