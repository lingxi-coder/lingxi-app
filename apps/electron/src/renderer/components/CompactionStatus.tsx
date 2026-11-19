import { useI18n } from '../i18n/useI18n';
import { memo, useEffect, useState, type CSSProperties } from 'react';

import type { CompactionRunItem } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import { formatElapsed } from './ToolCall';

function CompactionElapsed({ startedAt }: { startedAt: number }) {
  const { tr } = useI18n();
  const [elapsed, setElapsed] = useState(() => Date.now() - startedAt);
  useEffect(() => {
    const update = () => setElapsed(Date.now() - startedAt);
    update();
    const timer = setInterval(update, 1_000);
    return () => clearInterval(timer);
  }, [startedAt]);

  return <span className="compact-status-elapsed" role="timer" aria-live="off"
    aria-label={tr('desktop_ui_elapsed', Math.max(0, Math.floor(elapsed / 1_000)))}>
    {formatElapsed(elapsed)}
  </span>;
}

export const CompactionStatus = memo(function CompactionStatus({ item }: { item: CompactionRunItem }) {
  const t = useT();
  const { tr } = useI18n();
  const running = item.status === 'running';
  const title = running ? tr('chat_compacting_context')
    : item.status === 'complete' ? tr(item.automatic ? 'desktop_ui_compact_done_automatic' : 'desktop_ui_compact_done')
      : item.status === 'skipped' ? tr("chat_compaction_skipped") : item.status === 'cancelled' ? tr("desktop_ui_compact_cancelled") : tr("desktop_ui_compact_failed");
  const detail = item.status === 'error' ? item.detail : undefined;
  const style = {
    '--compact-color': item.status === 'error' ? t.danger : t.text3,
    '--compact-muted': t.text3,
    '--sweep-base': t.text3,
    '--sweep-highlight': t.text,
  } as CSSProperties;

  return (
    <div
      className="compact-status"
      data-status={item.status}
      role={item.status === 'error' ? 'alert' : undefined}
      style={style}
    >
      <span className="compact-status-icon" aria-hidden="true">
        <Icon name={item.status === 'error' ? 'shieldAlert' : 'context-compact'} size={18} />
      </span>
      <div className="compact-status-content">
        <div className="compact-status-heading">
          <span className={running ? 'compact-status-title running-sweep' : 'compact-status-title'}
            role={item.status === 'error' ? undefined : 'status'}>{title}</span>
          {running && item.startedAt !== undefined && <CompactionElapsed startedAt={item.startedAt} />}
        </div>
        {detail && <div className="compact-status-detail">{detail}</div>}
      </div>
    </div>
  );
});
