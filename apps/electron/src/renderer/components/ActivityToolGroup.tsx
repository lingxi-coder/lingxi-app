import type { ReactNode } from 'react';
import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
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
  return [toolGroupSummary(group.tools), running ? tr('desktop_ui_running_count', running) : '', failed ? tr('desktop_ui_failed_count', failed) : ''].filter(Boolean).join(' · ');
}

export function ActivityToolGroup({ group, children }: { group: TranscriptToolGroup; children: ReactNode }) {
  const t = useT();
  const { tr } = useI18n();
  const first = group.tools[0];
  const running = group.tools.filter(tool => tool.status === 'running').length;
  const failed = group.tools.filter(tool => tool.status === 'error').length;
  return <details className="activity-tool-group" style={{ color: t.text3, '--tool-focus-color': t.text, '--tool-error-color': t.danger } as CSSProperties}>
    <summary><ComputerApplicationIcon item={groupComputerApplication(group.tools)} fallback={<ToolActivityIcon name={first ? toolIconName(first.view.verb, first.tool, first.view.icon) : 'box'} />} />
      <span>{toolGroupSummary(group.tools)}</span>
      {running > 0 && <small>{tr('desktop_ui_running_count', running)}</small>}
      {failed > 0 && <small className="activity-tool-failed">{tr('desktop_ui_failed_count', failed)}</small>}<Icon name="chevronR" size={13} /></summary>
    <div className="activity-tool-details">{children}</div>
  </details>;
}
