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
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const error = classifyDesktopError(message);
  useEffect(() => {
    if (hovered || focused) return;
    const timeout = window.setTimeout(onDismiss, 5_000);
    return () => window.clearTimeout(timeout);
  }, [hovered, focused, onDismiss]);

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
        <span className="desktop-notification-icon"><Icon name="circleAlert" size={19} /></span>
        <div className="desktop-notification-content">
          <strong className="desktop-notification-title">{error.title}</strong>
          <p className="desktop-notification-detail" tabIndex={0} aria-label="Notification details">{error.detail}</p>
        </div>
        <button type="button" className="desktop-notification-close" onClick={onDismiss} aria-label="Dismiss notification" title="Dismiss notification">
          <Icon name="x" size={14} />
        </button>
      </div>
    </div>
  );
}
