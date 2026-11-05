/**
 * A private page on this computer, opened from the transcript.
 *
 * The row stays compact. Open mounts one hardened webview and close unmounts
 * it. The page is not an inline visualization and has no public URL.
 */

import { useState } from 'react';
import type { WebviewTag } from 'electron';

import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useT } from '../theme/ThemeContext';
import { PRIVATE_PAGE_PARTITION, privatePageUrl } from '../../shared/privatePage';
import { Icon } from './Icon';

export function PrivatePageCard({
  sessionId,
  reference,
}: {
  sessionId: string;
  reference: { readonly id: string; readonly revision: number };
}) {
  const t = useT();
  useI18n();
  const [open, setOpen] = useState(false);
  const src = privatePageUrl(sessionId, reference.id, reference.revision);
  return (
    <div className="private-page-card" style={{
      border: `1px solid ${t.border}`, borderRadius: 10, background: t.surface, color: t.text,
    }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, padding: '8px 10px' }}>
        <Icon name="page" size={16} />
        <span style={{ flex: 1, minWidth: 0 }}>{tr('private_page_label')}</span>
        <button type="button" onClick={() => setOpen((value) => !value)}
          style={{ border: `1px solid ${t.border}`, background: t.surface, color: t.text, borderRadius: 8, padding: '4px 10px' }}>
          {open ? tr('private_page_close') : tr('private_page_open')}
        </button>
      </div>
      {open && src && (
        <div style={{ height: 480, borderTop: `1px solid ${t.border}` }}>
          <webview
            src={src}
            partition={PRIVATE_PAGE_PARTITION}
            style={{ display: 'flex', width: '100%', height: '100%', border: 0 }}
            ref={(_node: WebviewTag | null) => undefined}
          />
        </div>
      )}
    </div>
  );
}

export function PrivatePagePlaceholder() {
  const t = useT();
  useI18n();
  return (
    <div className="private-page-card" aria-busy="true" style={{ border: `1px solid ${t.border}`, color: t.text3, padding: '8px 10px' }}>
      <span>{tr('private_page_preparing')}</span>
    </div>
  );
}

export function PrivatePageUnavailable() {
  const t = useT();
  useI18n();
  return (
    <div className="private-page-card" role="note" style={{ border: `1px solid ${t.border}`, color: t.text3, padding: '8px 10px' }}>
      <span>{tr('private_page_unavailable')}</span>
    </div>
  );
}
