import { useId, useState } from 'react';
import type { CSSProperties, ReactNode } from 'react';
import { useT } from '../theme/ThemeContext';
import { Disclosure } from './Disclosure';
import './ApiRetryNotice.css';

export interface ApiRetryStatus {
  readonly message: string;
  readonly attempt: number;
  readonly max_retries: number;
  readonly delay_ms: number;
}

/** A quiet recovery status, with the failure available directly underneath. */
export function ApiRetryNotice({ retry, children }: { retry: ApiRetryStatus; children?: ReactNode }) {
  const t = useT();
  const id = useId();
  const [open, setOpen] = useState(true);
  const label = `Reconnecting ${retry.attempt}/${retry.max_retries}`;
  return (
    <div className="transcript-api-retry" data-run-type="api-retry"
      style={{ '--retry-text': t.text3, '--retry-focus': t.accent } as CSSProperties}>
      <Disclosure id={id} open={open} onToggle={() => setOpen(value => !value)}
        buttonClassName="transcript-api-retry-trigger" label={label}
        summary={<>
          <svg className="transcript-api-retry-icon" width="18" height="18" viewBox="0 0 24 24"
            fill="none" stroke="currentColor" strokeWidth="1.65" strokeLinecap="round" aria-hidden="true">
            <path d="M2 7.5a16 16 0 0 1 20 0M5.5 11a10.5 10.5 0 0 1 13 0M9 14.5a5 5 0 0 1 6 0" />
            <circle cx="12" cy="18.5" r="1" fill="currentColor" stroke="none" />
          </svg>
          <span role="status">{label}</span>
        </>}>
        <p className="transcript-api-retry-message">{retry.message}</p>
        {children}
      </Disclosure>
    </div>
  );
}
