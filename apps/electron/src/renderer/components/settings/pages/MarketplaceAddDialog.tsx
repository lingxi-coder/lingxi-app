import { useId, useLayoutEffect, useRef, type PointerEvent } from 'react';
import { tr } from '../../../i18n/locale';
import { useT } from '../../../theme/ThemeContext';
import { extensionHubStyle } from './ExtensionHub';
import { ExtensionGlyph } from './ExtensionHubIcons';

interface MarketplaceAddDialogProps {
  source: string;
  reviewed: boolean;
  canConfirm: boolean;
  adminAvailable: boolean;
  busy: boolean;
  error: string | null;
  hasUnsavedChanges: boolean;
  discardPending: boolean;
  onChangeSource: (source: string) => void;
  onPreview: () => void;
  onConfirm: () => void;
  onEdit: () => void;
  onClose: () => void;
  onKeepEditing: () => void;
  onDiscard: () => void;
}

function isBackdropPointer(event: PointerEvent<HTMLDialogElement>) {
  if (event.target !== event.currentTarget) return false;
  const { left, right, top, bottom } = event.currentTarget.getBoundingClientRect();
  return event.clientX < left || event.clientX > right || event.clientY < top || event.clientY > bottom;
}

export function MarketplaceAddDialog({
  source, reviewed, canConfirm, adminAvailable, busy, error, hasUnsavedChanges, discardPending,
  onChangeSource, onPreview, onConfirm, onEdit, onClose, onKeepEditing, onDiscard,
}: MarketplaceAddDialogProps) {
  const t = useT();
  const id = useId();
  const dialogRef = useRef<HTMLDialogElement>(null);
  const sourceRef = useRef<HTMLInputElement>(null);
  const reviewRef = useRef<HTMLDivElement>(null);
  const keepEditingRef = useRef<HTMLButtonElement>(null);
  const backdropPressed = useRef(false);
  const canPreview = adminAvailable && !busy && Boolean(source.trim());
  const sourceLabel = tr('settings_parity_marketplace_source');
  const feedback = error ?? (reviewed && !canConfirm ? tr('desktop_extension_missing_revision') : null);

  useLayoutEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    dialog.showModal();
    return () => dialog.close();
  }, []);

  useLayoutEffect(() => {
    const target = discardPending ? keepEditingRef.current : reviewed ? reviewRef.current : sourceRef.current;
    target?.focus({ preventScroll: true });
  }, [discardPending, reviewed]);

  return <dialog
    ref={dialogRef}
    className="extension-detail-dialog marketplace-add-dialog"
    aria-labelledby={`${id}-title`}
    {...{ closedby: busy || hasUnsavedChanges ? 'none' : 'any' }}
    style={extensionHubStyle(t)}
    onCancel={(event) => {
      event.preventDefault();
      if (!busy) (discardPending ? onKeepEditing : onClose)();
    }}
    onKeyDown={(event) => {
      // A native close request can be non-cancelable. Keep the draft open for confirmation.
      if (event.key !== 'Escape' || !hasUnsavedChanges || busy) return;
      event.preventDefault();
      if (!event.repeat) (discardPending ? onKeepEditing : onClose)();
    }}
    onPointerDown={(event) => {
      backdropPressed.current = event.button === 0 && isBackdropPointer(event);
    }}
    onPointerUp={(event) => {
      const backdropClicked = backdropPressed.current && isBackdropPointer(event);
      backdropPressed.current = false;
      if (backdropClicked && hasUnsavedChanges && !busy && !discardPending) onClose();
    }}
  >
    <header className="marketplace-add-header">
      <span className="marketplace-add-icon" aria-hidden="true"><ExtensionGlyph name="plugin" size={22} /></span>
      <div>
        <h1 id={`${id}-title`}>{tr('plugins_marketplace_add')}</h1>
        <p>{tr('desktop_extension_marketplace_add_description')}</p>
      </div>
      <button type="button" className="extension-hub-icon-button" disabled={busy || discardPending}
        aria-label={tr('desktop_extension_close_details')} onClick={onClose}>
        <ExtensionGlyph name="x" size={16} />
      </button>
    </header>
    <form className="marketplace-add-form" onSubmit={(event) => {
      event.preventDefault();
      if (!reviewed && !discardPending && canPreview) onPreview();
    }}>
      <div className="marketplace-add-content">
        {discardPending ? <p id={`${id}-discard`} className="marketplace-add-discard">
          {tr('desktop_shell_unsaved_question')}
        </p> : reviewed ? <div className="marketplace-add-review" ref={reviewRef} tabIndex={-1}
          aria-labelledby={`${id}-review-title`} aria-describedby={`${id}-review-source`} role="group">
          <span className="marketplace-add-check" aria-hidden="true"><ExtensionGlyph name="check" size={16} /></span>
          <div>
            <h2 id={`${id}-review-title`}>{tr('desktop_extension_marketplace_review_ready')}</h2>
            <dl id={`${id}-review-source`}>
              <dt>{sourceLabel}</dt>
              <dd dir="ltr">{source.trim()}</dd>
            </dl>
          </div>
        </div> : <div className="marketplace-add-field">
          <label htmlFor={`${id}-source`}>{sourceLabel}</label>
          <p id={`${id}-hint`}>{tr('desktop_extension_add_marketplace_hint')}</p>
          <input id={`${id}-source`} ref={sourceRef} value={source} onChange={(event) => onChangeSource(event.target.value)}
            aria-describedby={`${id}-hint${error ? ` ${id}-error` : ''}`} dir="ltr" readOnly={busy}
            placeholder={tr('desktop_extension_marketplace_placeholder')} autoComplete="off" autoCapitalize="none" spellCheck={false} required />
        </div>}
        {!discardPending && feedback && <p id={`${id}-error`} className="marketplace-add-error" role="alert">
          <ExtensionGlyph name="circleAlert" size={16} />{feedback}
        </p>}
      </div>
      <footer className="marketplace-add-actions">
        {discardPending ? <>
          <button type="button" className="extension-hub-danger" onClick={onDiscard}>{tr('desktop_shell_discard_leave')}</button>
          <button type="button" className="extension-hub-primary" ref={keepEditingRef} aria-describedby={`${id}-discard`} onClick={onKeepEditing}>
            {tr('desktop_extension_continue_editing')}
          </button>
        </> : reviewed ? <>
          <button type="button" className="extension-hub-icon-text" disabled={busy} onClick={onEdit}>{tr('desktop_extension_marketplace_edit_source')}</button>
          <button type="button" className="extension-hub-primary" disabled={busy || !canConfirm} onClick={onConfirm}>
            {tr(busy ? 'desktop_extension_marketplace_adding' : 'plugins_marketplace_add')}
          </button>
        </> : <>
          <button type="button" className="extension-hub-icon-text" disabled={busy} onClick={onClose}>{tr('common_cancel')}</button>
          <button type="submit" className="extension-hub-primary" disabled={!canPreview}>
            {tr(busy ? 'desktop_extension_marketplace_preparing_preview' : 'desktop_extension_preview_add')}
          </button>
        </>}
      </footer>
    </form>
  </dialog>;
}
