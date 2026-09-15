import { memo, useEffect, useState, type CSSProperties } from 'react';

import { compactProgressPercent, type CompactionRunItem } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${Math.round(bytes / 1_024)} KB`;
  return `${(bytes / 1_048_576).toFixed(bytes < 10_485_760 ? 1 : 0)} MB`;
}

function useClock(item: CompactionRunItem): number {
  const running = item.status === 'running';
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!running) return undefined;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [running]);
  return item.finishedAt ?? now;
}

const PHASE_TITLES = {
  queued: 'Waiting for engine',
  preparing: 'Preparing compaction',
  summarizing: 'Summarizing conversation',
  restoring: 'Restoring context',
} as const;

export const CompactionStatus = memo(function CompactionStatus({ item }: { item: CompactionRunItem }) {
  const t = useT();
  const running = item.status === 'running';
  const now = useClock(item);
  const elapsed = Math.floor(Math.max(0, now - (item.startedAt ?? now)) / 1_000);
  const percent = compactProgressPercent(running ? item.phase : item.status, now - (item.phaseStartedAt ?? now));
  const estimating = running && percent !== null;
  const hasMetrics = item.messagesBefore !== undefined && item.messagesAfter !== undefined && item.bytesSaved !== undefined;
  const color = item.status === 'error' ? t.danger : t.text2;
  const title = running ? (PHASE_TITLES[item.phase as keyof typeof PHASE_TITLES] ?? 'Compacting context')
    : item.status === 'complete' ? (hasMetrics ? 'Context compacted' : 'Compaction finished')
      : item.status === 'skipped' ? 'No compaction needed' : item.status === 'cancelled' ? 'Compaction cancelled' : 'Compaction failed';
  const detail = item.status === 'complete' && hasMetrics
    ? `${item.messagesBefore} → ${item.messagesAfter} messages · ${formatBytes(item.bytesSaved!)} saved`
    : item.detail;
  const style = {
    '--compact-color': color,
    '--compact-muted': t.text3,
    '--compact-track': t.surfaceActive,
    '--compact-progress': t.accent,
    '--sweep-base': t.text3,
    '--sweep-highlight': t.text,
  } as CSSProperties;

  return (
    <div
      className="compact-status"
      data-status={item.status}
      role={item.status === 'error' ? 'alert' : 'status'}
      aria-live="polite"
      style={style}
    >
      {item.status !== 'complete' && (
        <span className="compact-status-icon" aria-hidden="true">
          <Icon name={item.status === 'error' || item.status === 'cancelled' ? 'x' : 'compact'} size={17} stroke={1.75} />
        </span>
      )}
      <div className="compact-status-content">
        <div className={running ? 'compact-status-title running-sweep' : 'compact-status-title'}>{title}</div>
        {detail && <div className="compact-status-detail">{detail}</div>}
        {estimating && (
          <div
            className="compact-progress-track"
            role="progressbar"
            aria-label="Estimated progress"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent ?? undefined}
            aria-valuetext={title}
          >
            <span className="compact-progress-indicator" style={{ width: `${percent}%` }} />
          </div>
        )}
      </div>
      {(estimating || item.status === 'complete') && (
        <span className="compact-status-elapsed" aria-label={`${elapsed} seconds elapsed`}>
          {estimating ? 'Estimated progress: ' : ''}{percent}% · {elapsed}s
        </span>
      )}
    </div>
  );
});
