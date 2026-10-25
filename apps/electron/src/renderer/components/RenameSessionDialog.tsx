import { tr } from '../i18n/locale';
import { useEffect, useRef, useState } from 'react';

import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { Icon } from './Icon';

export function RenameSessionDialog({ title, busy, error, onClose, onConfirm }: {
  title: string;
  busy: boolean;
  error: string;
  onClose(): void;
  onConfirm(title: string): void;
}) {
  const input = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(title);
  const trimmed = value.trim();
  useEffect(() => {
    input.current?.focus();
    input.current?.select();
  }, []);
  return <DesktopDialog title={tr('desktop_rename_chat')} ariaLabel={tr('desktop_rename_chat')} icon="pencil" onEscape={() => { if (!busy) onClose(); }}>
    <button type="button" aria-label={tr('desktop_rename_close')} disabled={busy} onClick={onClose}
      style={{ position: 'absolute', right: 16, top: 16, border: 0, background: 'transparent', color: 'var(--dialog-text-2)', cursor: 'pointer' }}><Icon name="x" size={18} /></button>
    <form aria-busy={busy} style={{ display: 'grid', gap: 20 }}
      onSubmit={(event) => { event.preventDefault(); if (!busy && trimmed) onConfirm(trimmed); }}>
      <label style={{ display: 'grid', gap: 7, color: 'var(--dialog-text-2)', fontSize: 12.5 }}>
        {tr('desktop_chat_name')}
        <input ref={input} value={value} disabled={busy} maxLength={200} onChange={(event) => setValue(event.target.value)}
          style={{ width: '100%', minHeight: 36, padding: '7px 9px', border: '1px solid var(--dialog-border)', borderRadius: 8, outline: 'none', background: 'var(--dialog-surface)', color: 'var(--dialog-text)', font: 'inherit' }} />
      </label>
      {error ? <p role="alert" style={{ margin: 0, color: 'var(--dialog-danger)', lineHeight: 1.5 }}>{error}</p> : null}
      <DesktopDialogActions>
        <DesktopDialogButton variant="cancel" disabled={busy} onClick={onClose}>{tr('common_cancel')}</DesktopDialogButton>
        <DesktopDialogButton variant="primary" type="submit" disabled={busy || !trimmed}>{busy ? tr('desktop_renaming') : tr('desktop_rename')}</DesktopDialogButton>
      </DesktopDialogActions>
    </form>
  </DesktopDialog>;
}
