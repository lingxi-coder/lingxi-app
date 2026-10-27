import { useEffect, useId, useRef } from 'react';
import { tr } from '../i18n/locale';
import { DesktopDialog } from './DesktopDialog';
import { Icon } from './Icon';
import './TrustFolderDialog.css';

export function TrustFolderDialog({ path, onClose, onConfirm }: {
  path: string;
  onClose(): void;
  onConfirm(): void;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const cancel = useRef<HTMLButtonElement>(null);
  const titleId = useId();
  const descriptionId = useId();
  const pathId = useId();
  useEffect(() => {
    panel.current?.setAttribute('aria-modal', 'true');
    cancel.current?.focus();
  }, []);

  return <DesktopDialog ref={panel} title={tr('desktop_project_trust_title')} titleId={titleId}
    ariaLabelledBy={titleId} ariaDescribedBy={`${pathId} ${descriptionId}`}
    className="trust-folder-dialog" zIndex={70} onEscape={onClose}
    onKeyDown={event => {
      event.stopPropagation();
      if (event.key !== 'Tab') return;
      const buttons = panel.current?.querySelectorAll<HTMLButtonElement>('button');
      const first = buttons?.[0], last = buttons?.[buttons.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    }}>
    <button type="button" className="trust-folder-close" aria-label={tr('desktop_create_project_close')} onClick={onClose}>
      <Icon name="x" size={20} stroke={1.7} />
    </button>
    <p id={pathId} className="trust-folder-path" dir="auto">{path}</p>
    <p id={descriptionId} className="trust-folder-description">{tr('desktop_project_trust_description')}</p>
    <div className="trust-folder-actions">
      <button type="button" className="trust-folder-confirm" onClick={onConfirm}>{tr('desktop_project_trust_confirm')}</button>
      <button ref={cancel} type="button" className="trust-folder-cancel" onClick={onClose}>{tr('common_cancel')}</button>
    </div>
  </DesktopDialog>;
}
