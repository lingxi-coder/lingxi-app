/** @jsxRuntime automatic */
import { useRef, type CSSProperties } from 'react';
import './composer-surface.css';

interface SendFailureNoticeProps {
  message: string | null;
  dismissLabel: string;
  onDismiss(): void;
  style?: CSSProperties;
}

export function SendFailureNotice({ message, dismissLabel, onDismiss, style }: SendFailureNoticeProps) {
  const previousFocus = useRef<HTMLElement | null>(null);
  return (
    <div className="send-failure-notice" data-visible={message !== null} style={style}>
      <div role="alert" aria-atomic="true">{message}</div>
      {message !== null && (
        <button type="button" aria-label={dismissLabel}
          onFocus={(event) => {
            previousFocus.current = event.relatedTarget instanceof HTMLElement ? event.relatedTarget : null;
          }}
          onClick={() => {
            onDismiss();
            if (previousFocus.current?.isConnected) previousFocus.current.focus({ preventScroll: true });
          }}>
          <span aria-hidden="true">×</span>
        </button>
      )}
    </div>
  );
}
