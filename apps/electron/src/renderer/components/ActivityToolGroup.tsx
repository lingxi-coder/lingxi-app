import type { ReactNode } from 'react';
import { useT } from '../theme/ThemeContext';
import type { TranscriptToolGroup } from './transcriptRows';
import { Icon } from './Icon';
import { ToolActivityIcon } from './ToolActivityIcon';
import { ComputerApplicationIcon } from './ComputerApplicationIcon';
import { groupComputerApplication } from './computerApplication';
import { toolIconName } from './ToolCall';
import { toolGroupSummary } from './toolPresentation';
import type { CSSProperties } from 'react';

export function activityToolSummary(group: TranscriptToolGroup): string {
  const running = group.tools.filter(tool => tool.status === 'running').length;
  const failed = group.tools.filter(tool => tool.status === 'error').length;
  return `${toolGroupSummary(group.tools)}${running ? ` · ${running} running` : ''}${failed ? ` · ${failed} failed` : ''}`;
}

export function ActivityToolGroup({ group, children }: { group: TranscriptToolGroup; children: ReactNode }) {
  const t = useT();
  const first = group.tools[0];
  return <details className="activity-tool-group" style={{ color: t.text3, '--tool-focus-color': t.text, '--tool-error-color': t.danger } as CSSProperties}>
    <summary><ComputerApplicationIcon item={groupComputerApplication(group.tools)} fallback={<ToolActivityIcon name={first ? toolIconName(first.view.verb, first.tool, first.view.icon) : 'box'} />} />
      <span>{toolGroupSummary(group.tools)}</span>
      {group.tools.some(tool => tool.status === 'running') && <small>{group.tools.filter(tool => tool.status === 'running').length} running</small>}
      {group.tools.some(tool => tool.status === 'error') && <small className="activity-tool-failed">{group.tools.filter(tool => tool.status === 'error').length} failed</small>}<Icon name="chevronR" size={13} /></summary>
    <div className="activity-tool-details">{children}</div>
  </details>;
}
