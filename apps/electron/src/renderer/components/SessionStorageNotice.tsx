import { useRef, useState } from 'react';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';
import { useI18n } from '../i18n/useI18n';

export function SessionStorageNotice({ disabled, onRetry }: { disabled: boolean; onRetry(): Promise<void> }) {
  const { tr } = useI18n();
  const t = useT();
  const submitting = useRef(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const retry = async () => {
    if (disabled || submitting.current) return;
    submitting.current = true;
    setPending(true);
    setError(false);
    try { await onRetry(); }
    catch { setError(true); }
    finally { submitting.current = false; setPending(false); }
  };
  return <section role="alert" aria-label={tr('desktop_misc_disk_full')} style={{
    display: 'flex', alignItems: 'flex-start', gap: 10, padding: '12px 14px', marginBottom: 10,
    border: `1px solid ${t.border}`, borderRadius: 12, background: t.surface, color: t.text,
  }}>
    <Icon name="circleAlert" size={18} color={t.danger} />
    <div style={{ flex: 1, minWidth: 0, fontSize: 13, lineHeight: 1.6 }}>
      <strong>{tr('desktop_misc_disk_full')}</strong>
      <p style={{ margin: '3px 0 8px', color: t.text2 }}>{tr('desktop_misc_disk_recovery_hint')}</p>
      <button type="button" disabled={disabled || pending} onClick={() => { void retry(); }}
        style={{ color: t.accent, background: t.surfaceHover, border: `1px solid ${t.border}`, borderRadius: 7,
          padding: '5px 10px', font: 'inherit', cursor: disabled || pending ? 'default' : 'pointer' }}>
        {pending ? tr('desktop_misc_recovering') : tr('desktop_misc_retry_continue')}
      </button>
      {error ? <p role="status" style={{ margin: '6px 0 0', color: t.danger }}>{tr('desktop_misc_recovery_failed')}</p> : null}
    </div>
  </section>;
}
