import { questionAnswers } from './AskUserQuestionSummary';
import type { CSSProperties } from 'react';
import type { TranscriptToolGroup } from './transcriptRows';
import { toolDisplayHeader } from '../model/runItem';
import { Disclosure } from './Disclosure';
import { ToolCall, toolIconName } from './ToolCall';
import { ToolActivityIcon } from './ToolActivityIcon';
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
  if (group.tools.some((tool) => questionAnswers(tool))) {
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
    const singleView = toolDisplayHeader(singleTool);
    if (/permission/i.test(`${singleTool.tool} ${singleView.label} ${singleView.title}`)) {
      return <div className="transcript-tool-group">
        <ToolCall item={singleTool} modUiSessionId={modUiSessionId} open={toolOpen(singleTool.id)} onSetOpen={onSetOpen} />
      </div>;
    }
  }
  const failed = group.tools.filter((tool) => tool.status === 'error').length;
  const lastTool = group.tools.at(-1);
  if (!lastTool) return null;
  const view = toolDisplayHeader(lastTool);
  const title = view.primary ? `${view.label}(${view.primary})${view.qualifier ?? ''}` : view.title;
  const detail = view.sub_line;
  const summary = `${title}${detail ? ` · ${detail.prefix}${detail.text}` : ''}`;
  const failureSummary = failed ? ` · ${failed} failed` : '';
  return <div className="transcript-tool-group">
    <Disclosure id={group.id} open={open} onToggle={() => onSetOpen(group.id, !open)}
      buttonClassName="tool-group-trigger"
      label={`${summary}${failureSummary} · ${group.tools.length} tools`}
      buttonStyle={{ width: '100%', borderRadius: 6, background: 'var(--tool-group-background, transparent)', '--tool-hover-background': t.surfaceHover, maxWidth: '100%', minWidth: 0, minHeight: 32, fontSize: 13, gap: 8, color: lastTool.status === 'error' ? t.danger : t.text3 } as CSSProperties}
      summary={<>
        <span aria-hidden="true" style={{ display: 'inline-flex', flexShrink: 0 }}><ToolActivityIcon name={toolIconName(lastTool.view.verb, lastTool.tool, lastTool.view.icon)} /></span>
        <span title={summary} style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{summary}</span>
        {failed > 0 && <span style={{ flexShrink: 0, color: t.danger }}>{failureSummary}</span>}
      </>}
      bodyStyle={{ borderLeft: `1px solid ${t.border}`, paddingLeft: 12, margin: '4px 0 0 8px' }}>
      {group.tools.map((tool) => <ToolCall key={tool.id} item={tool} modUiSessionId={modUiSessionId} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </Disclosure>
  </div>;
}
