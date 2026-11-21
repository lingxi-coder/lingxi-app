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
import { TranscriptLeafWindow } from './TranscriptLeafWindow';

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
      <TranscriptLeafWindow items={group.tools} renderItem={tool => <ToolCall item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />} />
    </div>;
  }
  const running = group.tools.some(tool => tool.status === 'running');
  const singleTool = group.tools.length === 1 ? group.tools[0] : undefined;
  if (singleTool) {
    return <div className="transcript-tool-group" aria-label={running ? tr('desktop_ui_running_tools') : undefined}>
      <ToolCall item={singleTool} modUiSessionId={modUiSessionId} open={toolOpen(singleTool.id)} onSetOpen={onSetOpen} />
    </div>;
  }
  const lastTool = group.tools.at(-1);
  if (!lastTool) return null;
  const summary = toolGroupSummary(group.tools);
  const firstTool = group.tools[0]!;
  const taskWorkflow = transcriptToolKey(firstTool) === 'task-workflow';
  const status = running ? tr('chat_status_running')
    : group.tools.every(tool => tool.status === 'error') ? tr('chat_status_failed') : '';
  const calls = (tools: typeof group.tools) => <div className="tool-group-calls">
    <TranscriptLeafWindow items={tools} gap={2} renderItem={tool => <ToolCall item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />} />
  </div>;
  return <div className="transcript-tool-group" data-tool-group-id={group.id}
    data-task-workflow={taskWorkflow || undefined}
    style={{ '--tool-focus-color': t.text, '--tool-focus-ring': t.accent, '--tool-group-border': t.border } as CSSProperties}>
    <Disclosure id={group.id} open={open} onToggle={() => onSetOpen(group.id, !open)}
      buttonClassName="tool-group-trigger"
      label={[summary, status].filter(Boolean).join(' · ')}
      buttonStyle={{ width: '100%', borderRadius: 6, background: 'transparent', maxWidth: '100%', minWidth: 0, minHeight: 28, fontSize: 13, fontWeight: 400, gap: 9, color: t.text3 }}
      summary={<>
        <span className="tool-group-icon" aria-hidden="true"><ComputerApplicationIcon item={groupComputerApplication(group.tools)} fallback={<ToolActivityIcon name={taskWorkflow ? 'tasks' : toolIconName(firstTool.view.verb, firstTool.tool, firstTool.view.icon)} />} /></span>
        <span className="tool-group-summary-text" title={summary} style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{summary}</span>
        <span className="tool-group-state" role="status">{status}</span>
      </>}
      bodyStyle={{ minWidth: 0 }}>
      {calls(group.tools)}
    </Disclosure>
  </div>;
}
