import { memo, useEffect, useState, type CSSProperties } from 'react';

import { compactProgressPercent, type CompactionRunItem } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

function formatBytes(bytes: number): string {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${Math.round(bytes / 1_024)} KB`;
  return `${(bytes / 1_048_576).toFixed(bytes < 10_485_760 ? 1 : 0)} MB`;
}

function useElapsedSeconds(running: boolean): number {
  const [seconds, setSeconds] = useState(0);
  useEffect(() => {
    if (!running) return undefined;
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      setSeconds(Math.floor((Date.now() - startedAt) / 1_000));
    }, 1_000);
    return () => window.clearInterval(timer);
  }, [running]);
  return seconds;
}

export const CompactionStatus = memo(function CompactionStatus({ item }: { item: CompactionRunItem }) {
  const t = useT();
  const running = item.status === 'running';
  const elapsed = useElapsedSeconds(running);
  const percent = compactProgressPercent(elapsed * 1_000);
  const color = item.status === 'error' ? t.danger : item.status === 'complete' ? t.ok : t.text3;
  const title = running ? 'Compacting context' : item.status === 'complete' ? 'Context compacted' : 'Compaction failed';
  const detail = item.status === 'complete'
    ? `${item.messagesBefore ?? 0} → ${item.messagesAfter ?? 0} messages · ${formatBytes(item.bytesSaved ?? 0)} saved`
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
      role={item.status === 'error' ? 'alert' : 'status'}
      aria-live="polite"
      style={style}
    >
      <span className="compact-status-icon" aria-hidden="true">
        <Icon name={item.status === 'complete' ? 'check' : item.status === 'error' ? 'x' : 'compact'} size={17} stroke={1.75} />
      </span>
      <div className="compact-status-content">
        <div className={running ? 'compact-status-title running-sweep' : 'compact-status-title'}>{title}</div>
        {detail && <div className="compact-status-detail">{detail}</div>}
        {running && (
          <div
            className="compact-progress-track"
            role="progressbar"
            aria-label="Compaction in progress"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={percent}
          >
            <span className="compact-progress-indicator" style={{ width: `${percent}%` }} />
          </div>
        )}
      </div>
      {running && (
        <span className="compact-status-elapsed" aria-label={`${elapsed} seconds elapsed`}>
          {percent}% · {elapsed}s
        </span>
      )}
    </div>
  );
});
