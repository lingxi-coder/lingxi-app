import { useEffect, useId, useRef, useState, type CSSProperties } from 'react';
import type { DesktopUpdateState } from '../../shared/desktopUpdates';
import { useI18n } from '../i18n/useI18n';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

function DownloadGlyph() {
  return <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M12 3v11m-4-4 4 4 4-4M5 14v3a4 4 0 0 0 4 4h6a4 4 0 0 0 4-4v-3" />
  </svg>;
}

export function DesktopUpdateReminder() {
  const [state, setState] = useState<DesktopUpdateState | null>(null);
  useEffect(() => {
    const api = window.lingxi?.desktopUpdates;
    if (!api) return;
    let active = true;
    let receivedEvent = false;
    const unsubscribe = api.onChanged(next => { receivedEvent = true; if (active) setState(next); });
    void api.getState().then(next => { if (active && !receivedEvent) setState(next); }, () => {});
    return () => { active = false; unsubscribe(); };
  }, []);
  return <DesktopUpdateNotice state={state} onDownload={() => window.lingxi!.desktopUpdates!.download()} onLater={() => window.lingxi!.desktopUpdates!.remindLater()} />;
}

/** Kept separate from IPC so availability and the reminder can be previewed in isolation. */
export function DesktopUpdateNotice({ state, onDownload, onLater }: {
  state: DesktopUpdateState | null;
  onDownload(): Promise<void>;
  onLater(): Promise<void>;
}) {
  const { tr } = useI18n();
  const t = useT();
  const id = useId();
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const download = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const version = state?.release?.version;
  useEffect(() => { setOpen(false); setError(false); }, [version]);
  useEffect(() => {
    if (!open) return;
    download.current?.focus();
    const outside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false);
    };
    document.addEventListener('pointerdown', outside);
    return () => document.removeEventListener('pointerdown', outside);
  }, [open]);
  if (!state?.release) return null;
  const label = tr('desktop_update_available', state.release.version);
  const close = () => { setOpen(false); trigger.current?.focus(); };
  const act = async (action: () => Promise<void>) => {
    if (busy) return;
    setBusy(true); setError(false);
    try { await action(); close(); }
    catch { setError(true); }
    finally { setBusy(false); }
  };
  return <div ref={root} className="desktop-update-reminder" style={{ '--update-window': t.windowBg, '--update-text': t.text, '--update-text-2': t.text2, '--update-border': t.border, '--update-hover': t.surfaceHover, '--update-danger': t.danger } as CSSProperties}
    onKeyDown={event => { if (event.key === 'Escape' && open) { event.stopPropagation(); event.preventDefault(); close(); } }}
    onBlur={event => { if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) setOpen(false); }}>
    <span className="desktop-update-announcement" role="status">{label}</span>
    <button ref={trigger} type="button" className="desktop-update-trigger" aria-label={label} title={label} aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? id : undefined}
      onClick={() => { setError(false); setOpen(value => !value); }}><DownloadGlyph /></button>
    {open && <div id={id} className="desktop-update-popover" role="dialog" aria-labelledby={`${id}-title`} aria-describedby={`${id}-description`}>
      <button type="button" className="desktop-update-close" aria-label={tr('desktop_update_close')} onClick={close}><Icon name="x" size={16} /></button>
      <h2 id={`${id}-title`}>{tr('desktop_update_title')}</h2>
      <p className="desktop-update-version">LingXi {state.release.version}</p>
      <p id={`${id}-description`}>{tr('desktop_update_description', state.currentVersion)}</p>
      {error && <p className="desktop-update-error" role="alert">{tr('desktop_update_error')}</p>}
      <div className="desktop-update-actions" aria-busy={busy}>
        <button type="button" disabled={busy} onClick={() => { void act(onLater); }}>{tr('desktop_update_later')}</button>
        <button ref={download} type="button" className="desktop-update-download" disabled={busy} onClick={() => { void act(onDownload); }}>{tr('desktop_update_download')}</button>
      </div>
    </div>}
  </div>;
}
