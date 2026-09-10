import { useEffect, useRef } from 'react';
import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { scheduledTaskFromJob, type ScheduledCronJob } from '../bridge/scheduledTaskDraft';
import { Icon } from './Icon';

export function ArchiveChatDialog({ title, jobs, loading, busy, error, onClose, onConfirm }: {
  title: string; jobs: ScheduledCronJob[]; loading: boolean; busy: boolean; error: string;
  onClose(): void; onConfirm(): void;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.setAttribute('aria-modal', 'true');
    dialog.current?.querySelector<HTMLButtonElement>('button')?.focus();
    return () => opener?.focus();
  }, []);
  useEffect(() => { if (busy) dialog.current?.focus(); }, [busy]);
  return <DesktopDialog ref={dialog} tabIndex={-1} title={jobs.length ? 'Archive chat and remove scheduled tasks?' : 'Archive chat?'}
    ariaLabel="Archive chat" icon="archive" tone={jobs.length ? 'danger' : 'default'}
    onEscape={() => { if (!busy) onClose(); }} onKeyDown={(event) => {
      if (event.key !== 'Tab') return;
      const controls = dialog.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)');
      const first = controls?.[0]; const last = controls?.[controls.length - 1];
      if (!first || !last) { event.preventDefault(); dialog.current?.focus(); return; }
      if (document.activeElement === dialog.current) { event.preventDefault(); (event.shiftKey ? last : first).focus(); return; }
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}>
    <button type="button" aria-label="Close archive dialog" disabled={busy} onClick={onClose}
      style={{ position: 'absolute', right: 16, top: 16, border: 0, background: 'transparent', color: 'var(--dialog-text-2)', cursor: 'pointer' }}><Icon name="x" size={18} /></button>
    {loading ? <p role="status">Checking scheduled tasks…</p> : jobs.length ? <p style={{ color: 'var(--dialog-text-2)', lineHeight: 1.7 }}>
      This chat has scheduled tasks: <strong>{jobs.map((job) => scheduledTaskFromJob(job).title).join(', ')}</strong>.
      Archiving “{title}” will also remove these tasks and stop future runs. The chat history will be kept.
    </p> : <p style={{ color: 'var(--dialog-text-2)', lineHeight: 1.7 }}>“{title}” will move to Archived chats. You can restore it later.</p>}
    {error && <p role="alert" style={{ color: 'var(--dialog-danger)', lineHeight: 1.6 }}>{error}</p>}
    <DesktopDialogActions>
      <DesktopDialogButton variant="cancel" disabled={busy} onClick={onClose}>Cancel</DesktopDialogButton>
      <DesktopDialogButton variant="primary" disabled={busy || loading || !!error} onClick={onConfirm}>{busy ? 'Archiving…' : jobs.length ? 'Archive and remove' : 'Archive chat'}</DesktopDialogButton>
    </DesktopDialogActions>
  </DesktopDialog>;
}
