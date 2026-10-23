import { useRef, useState } from 'react';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export function SessionStorageNotice({ disabled, onRetry }: { disabled: boolean; onRetry(): Promise<void> }) {
  const t = useT();
  const submitting = useRef(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const retry = async () => {
    if (disabled || submitting.current) return;
    submitting.current = true;
    setPending(true);
    setError('');
    try { await onRetry(); }
    catch { setError('恢复未完成。请确认已释放磁盘空间，然后重试。'); }
    finally { submitting.current = false; setPending(false); }
  };
  return <section role="alert" aria-label="磁盘空间不足，任务已暂停" style={{
    display: 'flex', alignItems: 'flex-start', gap: 10, padding: '12px 14px', marginBottom: 10,
    border: `1px solid ${t.border}`, borderRadius: 12, background: t.surface, color: t.text,
  }}>
    <Icon name="circleAlert" size={18} color={t.danger} />
    <div style={{ flex: 1, minWidth: 0, fontSize: 13, lineHeight: 1.6 }}>
      <strong>磁盘空间不足，任务已暂停</strong>
      <p style={{ margin: '3px 0 8px', color: t.text2 }}>释放磁盘空间后可继续此会话。恢复时会先补存待写入的记录，保留已有历史。</p>
      <button type="button" disabled={disabled || pending} onClick={() => { void retry(); }}
        style={{ color: t.accent, background: t.surfaceHover, border: `1px solid ${t.border}`, borderRadius: 7,
          padding: '5px 10px', font: 'inherit', cursor: disabled || pending ? 'default' : 'pointer' }}>
        {pending ? '正在检查并恢复…' : '重试并继续'}
      </button>
      {error ? <p role="status" style={{ margin: '6px 0 0', color: t.danger }}>{error}</p> : null}
    </div>
  </section>;
}
