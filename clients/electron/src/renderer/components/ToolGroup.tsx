import type { TranscriptToolGroup } from './transcriptRows';
import { Disclosure } from './Disclosure';
import { ToolCall } from './ToolCall';
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
  const summary = `Used ${group.tools.length} tool${group.tools.length === 1 ? '' : 's'}${failed ? ` · ${failed} failed` : ''}`;
  return <div className="transcript-tool-group">
    <Disclosure id={group.id} open={open} onToggle={() => onSetOpen(group.id, !open)}
      buttonClassName="tool-group-trigger"
      buttonStyle={{ minHeight: 32, fontSize: 13, gap: 8, color: failed ? t.danger : t.text3 }}
      summary={<><Icon name="summary-list" size={18} /><span>{summary}</span></>}
      bodyStyle={{ borderLeft: `1px solid ${t.border}`, paddingLeft: 12, margin: '4px 0 0 8px' }}>
      {group.tools.map((tool) => <ToolCall key={tool.id} item={tool} open={toolOpen(tool.id)} onSetOpen={onSetOpen} />)}
    </Disclosure>
  </div>;
}
