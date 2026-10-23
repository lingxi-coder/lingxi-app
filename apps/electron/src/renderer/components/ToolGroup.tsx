import { questionAnswers } from './AskUserQuestionSummary';
import type { CSSProperties } from 'react';
import type { TranscriptToolGroup } from './transcriptRows';
import { toolGroupSummary } from './toolPresentation';
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
  /** An outer activity disclosure already supplies the group heading. */
  ungrouped?: boolean;
  toolOpen(id: string): boolean | undefined;
  onSetOpen(id: string, next: boolean): void;
}

export function ToolGroup({ group, modUiSessionId, open, toolOpen, onSetOpen, ungrouped = false }: ToolGroupProps) {
  const t = useT();
  if (ungrouped || group.tools.some((tool) => questionAnswers(tool))) {
    return <div className="transcript-tool-group">
      {group.tools.map((tool) => <ToolCall key={tool.id} item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </div>;
  }
  const active = group.tools.filter((tool) => tool.status === 'running');
  if (active.length > 0) {
    return <div className="transcript-tool-group" aria-label="Running tools">
      {active.map((tool) => <ToolCall key={tool.id} item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </div>;
  }
  const singleTool = group.tools.length === 1 ? group.tools[0] : undefined;
  if (singleTool) {
    return <div className="transcript-tool-group">
      <ToolCall item={singleTool} modUiSessionId={modUiSessionId} open={toolOpen(singleTool.id)} onSetOpen={onSetOpen} />
    </div>;
  }
  const failed = group.tools.filter((tool) => tool.status === 'error').length;
  const lastTool = group.tools.at(-1);
  if (!lastTool) return null;
  const summary = toolGroupSummary(group.tools);
  const firstTool = group.tools[0]!;
  const failureSummary = failed ? ` · ${failed} failed` : '';
  return <div className="transcript-tool-group">
    <Disclosure id={group.id} open={open} onToggle={() => onSetOpen(group.id, !open)}
      buttonClassName="tool-group-trigger"
      label={`${summary}${failureSummary} · ${group.tools.length} tools`}
      buttonStyle={{ width: '100%', borderRadius: 6, background: 'var(--tool-group-background, transparent)', '--tool-hover-background': t.surfaceHover, maxWidth: '100%', minWidth: 0, minHeight: 28, fontSize: 13, fontWeight: 400, gap: 8, color: t.text3, '--tool-focus-color': t.text } as CSSProperties}
      summary={<>
        <span aria-hidden="true" style={{ display: 'inline-flex', flexShrink: 0 }}><ComputerApplicationIcon item={groupComputerApplication(group.tools)} fallback={<ToolActivityIcon name={toolIconName(firstTool.view.verb, firstTool.tool, firstTool.view.icon)} />} /></span>
        <span className="tool-group-summary-text" title={summary} style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{summary}</span>
        {failed > 0 && <span style={{ flexShrink: 0, color: t.danger }}>{failureSummary}</span>}
      </>}
      bodyStyle={{ margin: '2px 0 0', minWidth: 0 }}>
      {group.tools.map((tool) => <ToolCall key={tool.id} item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </Disclosure>
  </div>;
}
