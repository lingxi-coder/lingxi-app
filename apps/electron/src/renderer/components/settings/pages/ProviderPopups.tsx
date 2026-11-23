import { useEffect, useId, useRef, type ReactNode } from 'react';
import { tr } from '../../../i18n/locale';
import { Icon } from '../../Icon';

export function ProviderInfo({ label, children }: { label: string; children: ReactNode }) {
  const id = useId();
  return <>
    <button type="button" className="pa-icon-button pa-info-button" aria-label={label} {...{ popovertarget: id }}>
      <Icon name="info" size={16} />
    </button>
    <div id={id} className="pa-popup pa-info" {...{ popover: 'auto' }}>{children}</div>
  </>;
}

export function ProviderFeedback({ error, notice, children }: {
  error?: string;
  notice?: string;
  children?: ReactNode;
}) {
  const popup = useRef<HTMLDivElement>(null);
  const previousFocus = useRef<HTMLElement | null>(null);
  useEffect(() => {
    const element = popup.current;
    if (!element) return;
    if (error || notice) {
      if (!element.matches(':popover-open')) {
        previousFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        element.showPopover();
      }
    } else {
      element.hidePopover();
    }
  }, [error, notice]);

  const dismiss = () => {
    const restoreFocus = popup.current?.contains(document.activeElement);
    popup.current?.hidePopover();
    if (restoreFocus) previousFocus.current?.focus();
  };
  return <>
    {error && children && <button type="button" className="pa-icon-button pa-feedback-reopen" aria-label={tr('common_retry')} onClick={event => {
      previousFocus.current = event.currentTarget;
      popup.current?.showPopover();
      popup.current?.querySelector('button')?.focus();
    }}><Icon name="warning" size={16} /></button>}
    <div className="pa-announcement" role="alert">{error}</div>
    <div className="pa-announcement" role="status">{notice}</div>
    <div ref={popup} className="pa-popup pa-toast" role="region" aria-label={tr('provider_accounts_accounts')} {...{ popover: 'manual' }}
      onKeyDown={event => {
        if (event.key !== 'Escape') return;
        event.preventDefault();
        event.stopPropagation();
        dismiss();
      }}>
      <div className="pa-toast-content">
        <div>
          {error && <p className="pa-toast-message pa-toast-error"><Icon name="warning" size={16} />{error}</p>}
          {notice && <p className="pa-toast-message pa-toast-success"><Icon name="check" size={16} />{notice}</p>}
          {children}
        </div>
        <button type="button" className="pa-icon-button" aria-label={tr('common_close')} onClick={dismiss}><Icon name="x" size={14} /></button>
      </div>
    </div>
  </>;
}
