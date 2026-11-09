import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { questionAnswers } from './AskUserQuestionSummary';
import type { CSSProperties } from 'react';
import type { TranscriptToolGroup } from './transcriptRows';
import { toolGroupSummary, transcriptToolKey } from './toolPresentation';
import { Disclosure } from './Disclosure';
import { ToolCall, toolIconName } from './ToolCall';
import { ToolActivityIcon } from './ToolActivityIcon';
import { ComputerApplicationIcon } from './ComputerApplicationIcon';
import { groupComputerApplication } from './computerApplication';
import { useT } from '../theme/ThemeContext';

interface ToolGroupProps {
  group: TranscriptToolGroup;
  modUiSessionId?: string;
  open: boolean;
  toolOpen(id: string): boolean | undefined;
  onSetOpen(id: string, next: boolean): void;
}

export function ToolGroup({ group, modUiSessionId, open, toolOpen, onSetOpen }: ToolGroupProps) {
  const t = useT();
  useI18n();
  if (group.tools.some((tool) => questionAnswers(tool))) {
    return <div className="transcript-tool-group">
      {group.tools.map((tool) => <ToolCall key={tool.id} item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </div>;
  }
  const active = group.tools.filter((tool) => tool.status === 'running');
  const singleTool = group.tools.length === 1 ? group.tools[0] : undefined;
  if (singleTool) {
    return <div className="transcript-tool-group" aria-label={active.length ? tr('desktop_ui_running_tools') : undefined}>
      <ToolCall item={singleTool} modUiSessionId={modUiSessionId} open={toolOpen(singleTool.id)} onSetOpen={onSetOpen} />
    </div>;
  }
  const lastTool = group.tools.at(-1);
  if (!lastTool) return null;
  const summary = toolGroupSummary(group.tools);
  const firstTool = group.tools[0]!;
  const taskWorkflow = transcriptToolKey(firstTool) === 'task-workflow';
  const unsuccessful = group.tools.filter((tool) => tool.status === 'error').length;
  // Counts describe individual calls, never the outcome of the whole task.
  // Task workflows retain that context even when all their calls are hidden.
  const allFailed = group.tools.every(tool => tool.status === 'error');
  const runningState = active.length ? tr('desktop_ui_running_count', active.length) : '';
  const outcomeState = taskWorkflow && unsuccessful
    ? tr(unsuccessful === 1 ? 'desktop_ui_tool_call_unsuccessful_one' : 'desktop_ui_tool_call_unsuccessful_count', unsuccessful)
    : !active.length && allFailed ? tr('desktop_ui_tool_calls_unsuccessful') : '';
  const state = [runningState, outcomeState].filter(Boolean).join(' · ');
  const calls = (tools: typeof group.tools) => <div className="tool-group-calls">
    {tools.map((tool) => <ToolCall key={tool.id} item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
  </div>;
  return <div className="transcript-tool-group" data-tool-group-id={group.id}
    data-task-workflow={taskWorkflow || undefined}
    style={{ '--tool-focus-color': t.text, '--tool-focus-ring': t.accent, '--tool-group-border': t.border, '--tool-group-danger': t.danger } as CSSProperties}>
    <Disclosure id={group.id} open={open} onToggle={() => onSetOpen(group.id, !open)}
      buttonClassName="tool-group-trigger"
      label={[summary, state].filter(Boolean).join(' · ')}
      buttonStyle={{ width: '100%', borderRadius: 6, background: 'transparent', maxWidth: '100%', minWidth: 0, minHeight: 28, fontSize: 13, fontWeight: 400, gap: 9, color: t.text3 }}
      summary={<>
        <span className="tool-group-icon" aria-hidden="true"><ComputerApplicationIcon item={groupComputerApplication(group.tools)} fallback={<ToolActivityIcon name={taskWorkflow ? 'tasks' : toolIconName(firstTool.view.verb, firstTool.tool, firstTool.view.icon)} />} /></span>
        <span className="tool-group-summary-text" title={summary} style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{summary}</span>
        {runningState && <span className="tool-group-state" role="status">{runningState}</span>}
        {outcomeState && <span className="tool-group-state" data-unsuccessful={taskWorkflow && unsuccessful > 0 || undefined}>{outcomeState}</span>}
      </>}
      bodyStyle={{ minWidth: 0 }}>
      {calls(group.tools)}
    </Disclosure>
    {!open && !taskWorkflow && active.length > 0 && calls(active)}
  </div>;
}
