import { tr } from '../i18n/locale';
import { useEffect, useLayoutEffect, useRef } from 'react';
import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { scheduledTaskFromJob, type ScheduledCronJob } from '../bridge/scheduledTaskDraft';
import { Icon } from './Icon';

const useDialogLayoutEffect = typeof window === 'undefined' ? useEffect : useLayoutEffect;

export function ArchiveChatDialog({ title, jobs, loading, busy, error, onClose, onConfirm, onRetry, onRestoreFocus }: {
  title: string; jobs: ScheduledCronJob[]; loading: boolean; busy: boolean; error: string;
  onClose(): void; onConfirm(): void; onRetry?(): void; onRestoreFocus?(shouldRestore: boolean): void;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  const restoreFocus = useRef(onRestoreFocus);
  restoreFocus.current = onRestoreFocus;
  // Capture the opener before the busy-state layout effect moves focus.
  useDialogLayoutEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const root = dialog.current;
    root?.setAttribute('aria-modal', 'true');
    root?.querySelector<HTMLButtonElement>('button')?.focus();
    return () => {
      const active = document.activeElement;
      const shouldRestore = !active || active === document.body || active === document.documentElement || !!root?.contains(active);
      if (restoreFocus.current) restoreFocus.current(shouldRestore);
      else if (shouldRestore && opener?.isConnected) opener.focus();
    };
  }, []);
  // Disabling the focused button can send focus to the document body. Move
  // focus in the same commit, before a delayed passive effect or another
  // keyboard event can escape the modal.
  useDialogLayoutEffect(() => { if (busy) dialog.current?.focus(); }, [busy]);
  return <DesktopDialog ref={dialog} tabIndex={-1} title={jobs.length ? tr('desktop_archive_pause_title') : tr('desktop_archive_title')}
    ariaLabel={tr('desktop_archive_chat')} icon="archive" tone={jobs.length ? 'danger' : 'default'}
    onEscape={() => { if (!busy) onClose(); }} onKeyDown={(event) => {
      if (event.key !== 'Tab') return;
      const controls = dialog.current?.querySelectorAll<HTMLButtonElement>('button:not(:disabled)');
      const first = controls?.[0]; const last = controls?.[controls.length - 1];
      if (!first || !last) { event.preventDefault(); dialog.current?.focus(); return; }
      if (document.activeElement === dialog.current) { event.preventDefault(); (event.shiftKey ? last : first).focus(); return; }
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}>
    <button type="button" aria-label={tr('desktop_archive_close')} disabled={busy} onClick={onClose}
      style={{ position: 'absolute', right: 16, top: 16, border: 0, background: 'transparent', color: 'var(--dialog-text-2)', cursor: 'pointer' }}><Icon name="x" size={18} /></button>
    {loading ? <p role="status">{tr('desktop_archive_checking')}</p> : jobs.length ? <p style={{ color: 'var(--dialog-text-2)', lineHeight: 1.7 }}>
      {tr('desktop_archive_detail', jobs.map((job) => scheduledTaskFromJob(job).title).join(', '), title)}
    </p> : <p style={{ color: 'var(--dialog-text-2)', lineHeight: 1.7 }}>{tr('desktop_archive_move_detail', title)}</p>}
    {busy && <p role="status">{tr('desktop_archive_updating')}</p>}
    {error && <p role="alert" style={{ color: 'var(--dialog-danger)', lineHeight: 1.6 }}>{error}</p>}
    <DesktopDialogActions>
      <DesktopDialogButton variant="cancel" disabled={busy} onClick={onClose}>{tr('common_cancel')}</DesktopDialogButton>
      <DesktopDialogButton variant="primary" disabled={busy || loading || (!!error && !onRetry)} onClick={error ? onRetry : onConfirm}>{error && !busy ? tr('desktop_ui_retry') : busy ? tr('desktop_archive_busy') : jobs.length ? tr('desktop_archive_pause') : tr('desktop_archive_chat')}</DesktopDialogButton>
    </DesktopDialogActions>
  </DesktopDialog>;
}
