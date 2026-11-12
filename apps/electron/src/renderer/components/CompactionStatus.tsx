import { useI18n } from '../i18n/useI18n';
import { memo, type CSSProperties } from 'react';

import type { CompactionRunItem } from '../model/runItem';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export const CompactionStatus = memo(function CompactionStatus({ item }: { item: CompactionRunItem }) {
  const t = useT();
  const { tr } = useI18n();
  const title = item.status === 'running' ? tr('chat_compacting_context')
    : item.status === 'complete' ? tr(item.automatic ? 'desktop_ui_compact_done_automatic' : 'desktop_ui_compact_done')
      : item.status === 'skipped' ? tr("chat_compaction_skipped") : item.status === 'cancelled' ? tr("desktop_ui_compact_cancelled") : tr("desktop_ui_compact_failed");
  const detail = item.status === 'error' ? item.detail : undefined;
  const style = {
    '--compact-color': item.status === 'error' ? t.danger : t.text3,
    '--compact-muted': t.text3,
  } as CSSProperties;

  return (
    <div
      className="compact-status"
      data-status={item.status}
      role={item.status === 'error' ? 'alert' : 'status'}
      aria-live="polite"
      style={style}
    >
      <span className="compact-status-icon" aria-hidden="true">
        <Icon name={item.status === 'error' ? 'shieldAlert' : 'context-compact'} size={18} />
      </span>
      <div className="compact-status-content">
        <div className="compact-status-title">{title}</div>
        {detail && <div className="compact-status-detail">{detail}</div>}
      </div>
    </div>
  );
});
