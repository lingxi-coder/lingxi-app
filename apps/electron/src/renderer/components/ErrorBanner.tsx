import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { useEffect, useState, type CSSProperties } from 'react';
import type { UseBridge } from '../bridge/bridgeTypes.js';
import { classifyDesktopError } from '../bridge/errors';
import { useT } from '../theme/ThemeContext';
import { Icon } from './Icon';

export function ErrorBanner({ bridge }: { bridge: Pick<UseBridge, 'error' | 'clearError'> }) {
  return bridge.error ? <ErrorNotification key={bridge.error} message={bridge.error} onDismiss={bridge.clearError} /> : null;
}

function ErrorNotification({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  const t = useT();
  useI18n();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const error = classifyDesktopError(message);
  useEffect(() => {
    if (hovered || focused || detailsOpen) return;
    const timeout = window.setTimeout(onDismiss, 5_000);
    return () => window.clearTimeout(timeout);
  }, [hovered, focused, detailsOpen, onDismiss]);

  return (
    <div className="desktop-notification-region">
      <div
        className="desktop-notification"
        role="alert"
        aria-atomic="true"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
        onFocusCapture={() => setFocused(true)}
        onBlurCapture={(event) => { if (!event.currentTarget.contains(event.relatedTarget)) setFocused(false); }}
        style={{
          '--notification-surface': t.surface,
          '--notification-text': t.text,
          '--notification-detail': t.text2,
          '--notification-border': t.border,
          '--notification-danger': t.danger,
          '--notification-focus': t.accent,
        } as CSSProperties}
      >
        <span className="desktop-notification-icon" aria-hidden="true"><Icon name="circleAlert" size={19} /></span>
        <div className="desktop-notification-content">
          <strong className="desktop-notification-title">{error.title}</strong>
          <p className="desktop-notification-detail" tabIndex={0} aria-label={tr("desktop_ui_notifications")}>{error.detail}</p>
          {error.technicalDetail && (
            <details className="desktop-notification-diagnostics" onToggle={(event) => setDetailsOpen(event.currentTarget.open)}>
              <summary>{tr("desktop_ui_technical")}</summary>
              <pre tabIndex={0} aria-label={tr("desktop_ui_technical_error")}>{error.technicalDetail}</pre>
            </details>
          )}
        </div>
        <button type="button" className="desktop-notification-close" onClick={onDismiss} aria-label={tr("desktop_ui_dismiss_notification")} title={tr("desktop_ui_dismiss_notification")}>
          <Icon name="x" size={12} stroke={2} />
        </button>
      </div>
    </div>
  );
}
