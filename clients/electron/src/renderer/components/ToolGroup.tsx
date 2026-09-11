import type { TranscriptToolGroup } from './transcriptRows';
import { Disclosure } from './Disclosure';
import { ToolCall, toolIconName } from './ToolCall';
import { Icon } from './Icon';
import { useT } from '../theme/ThemeContext';

interface ToolGroupProps {
  group: TranscriptToolGroup;
  open: boolean;
  toolOpen(id: string): boolean | undefined;
  onSetOpen(id: string, next: boolean): void;
}

export function ToolGroup({ group, open, toolOpen, onSetOpen }: ToolGroupProps) {
  const t = useT();
  const active = group.tools.filter((tool) => tool.status === 'running');
  if (active.length > 0) {
    return <div className="transcript-tool-group" aria-label="Running tools">
      {active.map((tool) => <ToolCall key={tool.id} item={tool} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </div>;
  }
  const failed = group.tools.filter((tool) => tool.status === 'error').length;
  const lastTool = group.tools.at(-1);
  if (!lastTool) return null;
  const view = lastTool.view;
  const title = view.primary ? `${view.label}(${view.primary})${view.qualifier ?? ''}` : view.title;
  const detail = view.sub_line;
  const summary = `${title}${detail ? ` · ${detail.prefix}${detail.text}` : ''}`;
  const failureSummary = failed ? ` · ${failed} failed` : '';
  return <div className="transcript-tool-group">
    <Disclosure id={group.id} open={open} onToggle={() => onSetOpen(group.id, !open)}
      buttonClassName="tool-group-trigger"
      label={`${summary}${failureSummary} · ${group.tools.length} tools`}
      buttonStyle={{ maxWidth: '100%', minWidth: 0, minHeight: 32, fontSize: 13, gap: 8, color: failed ? t.danger : t.text3 }}
      summary={<>
        <span aria-hidden="true" style={{ display: 'inline-flex', flexShrink: 0 }}><Icon name={toolIconName(lastTool.view.verb)} size={18} stroke={1.8} /></span>
        <span title={summary} style={{ minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>{summary}</span>
        {failed > 0 && <span style={{ flexShrink: 0 }}>{failureSummary}</span>}
      </>}
      bodyStyle={{ borderLeft: `1px solid ${t.border}`, paddingLeft: 12, margin: '4px 0 0 8px' }}>
      {group.tools.map((tool) => <ToolCall key={tool.id} item={tool} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </Disclosure>
  </div>;
}
