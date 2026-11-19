import { useEffect, useState, type ReactElement } from 'react';
import { Icon } from './ui';
import { useTeamsText } from './i18n.js';

/** Whether this is a touch device with the system share sheet, where sharing a link means the sheet, not the clipboard. */
function prefersShareSheet(): boolean {
  if (typeof navigator === 'undefined' || typeof navigator.share !== 'function') return false;
  try { return globalThis.matchMedia?.('(pointer: coarse)').matches ?? false; } catch { return false; }
}

/**
 * The header's "Share link": the system share sheet on a touch device, else a copy to the clipboard with a short confirmation.
 * The link opens the app where it is installed and the same conversation on the web otherwise.
 */
export function ShareLinkButton({ link, title, note, locale }: { link: string; title: string; note?: string; locale: string }): ReactElement {
  const t = useTeamsText(locale);
  const [status, setStatus] = useState<'copied' | 'failed'>();
  useEffect(() => {
    if (!status) return;
    const timer = setTimeout(() => setStatus(undefined), 2400);
    return () => clearTimeout(timer);
  }, [status]);
  const share = async () => {
    if (prefersShareSheet()) {
      try { await navigator.share({ title: t('teams_share_text_fmt', title), url: link }); return; }
      catch (error) { if (error instanceof DOMException && error.name === 'AbortError') return; }
    }
    try { await navigator.clipboard.writeText(link); setStatus('copied'); } catch { setStatus('failed'); }
  };
  const label = note ? `${t('teams_share_link')} · ${note}` : t('teams_share_link');
  return <span className="tw-link-share">
    <button type="button" className="tw-icon-btn" aria-label={label} title={label} onClick={() => void share()}><Icon name={status === 'copied' ? 'check' : 'link'} /></button>
    <span className="tw-link-share-status" role="status" aria-live="polite">{status ? t(status === 'copied' ? 'teams_share_link_copied' : 'teams_share_link_failed') : ''}</span>
  </span>;
}
