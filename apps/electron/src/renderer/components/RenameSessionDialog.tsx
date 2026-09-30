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
  return <DesktopDialog title="Rename chat" ariaLabel="Rename chat" icon="pencil" onEscape={() => { if (!busy) onClose(); }}>
    <button type="button" aria-label="Close rename dialog" disabled={busy} onClick={onClose}
      style={{ position: 'absolute', right: 16, top: 16, border: 0, background: 'transparent', color: 'var(--dialog-text-2)', cursor: 'pointer' }}><Icon name="x" size={18} /></button>
    <form aria-busy={busy} style={{ display: 'grid', gap: 20 }}
      onSubmit={(event) => { event.preventDefault(); if (!busy && trimmed) onConfirm(trimmed); }}>
      <label style={{ display: 'grid', gap: 7, color: 'var(--dialog-text-2)', fontSize: 12.5 }}>
        Chat name
        <input ref={input} value={value} disabled={busy} maxLength={200} onChange={(event) => setValue(event.target.value)}
          style={{ width: '100%', minHeight: 36, padding: '7px 9px', border: '1px solid var(--dialog-border)', borderRadius: 8, outline: 'none', background: 'var(--dialog-surface)', color: 'var(--dialog-text)', font: 'inherit' }} />
      </label>
      {error ? <p role="alert" style={{ margin: 0, color: 'var(--dialog-danger)', lineHeight: 1.5 }}>{error}</p> : null}
      <DesktopDialogActions>
        <DesktopDialogButton variant="cancel" disabled={busy} onClick={onClose}>Cancel</DesktopDialogButton>
        <DesktopDialogButton variant="primary" type="submit" disabled={busy || !trimmed}>{busy ? 'Renaming…' : 'Rename'}</DesktopDialogButton>
      </DesktopDialogActions>
    </form>
  </DesktopDialog>;
}
