import type { ReactNode } from 'react';
import { useT } from '../theme/ThemeContext';
import type { TranscriptToolGroup } from './transcriptRows';
import { Icon } from './Icon';

export function activityToolSummary(group: TranscriptToolGroup): string {
  const labels = [...new Set(group.tools.map(tool => tool.view.label))];
  const running = group.tools.filter(tool => tool.status === 'running').length;
  const failed = group.tools.filter(tool => tool.status === 'error').length;
  const activity = labels.slice(0, 3).join(', ') + (labels.length > 3 ? ` and ${labels.length - 3} more` : '');
  return `${activity}${group.tools.length > 1 ? ` · ${group.tools.length} tools` : ''}${running ? ` · ${running} running` : ''}${failed ? ` · ${failed} failed` : ''}`;
}

export function ActivityToolGroup({ group, children }: { group: TranscriptToolGroup; children: ReactNode }) {
  const t = useT();
  const failed = group.tools.some(tool => tool.status === 'error');
  const running = group.tools.some(tool => tool.status === 'running');
  return <details className="activity-tool-group" style={{ color: failed ? t.danger : t.text3 }}>
    <summary><Icon name={failed ? 'circleAlert' : running ? 'clock' : 'terminal'} size={16} />
      <span>{activityToolSummary(group)}</span><Icon name="chevronR" size={13} /></summary>
    <div className="activity-tool-details">{children}</div>
  </details>;
}
