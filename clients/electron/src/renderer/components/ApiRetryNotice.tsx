import { useEffect, useState } from 'react';
import type { CSSProperties } from 'react';
import { useT } from '../theme/ThemeContext';

export interface ApiRetryStatus {
  readonly message: string;
  readonly attempt: number;
  readonly max_retries: number;
  readonly delay_ms: number;
}

/** Whole seconds still to wait, floored at 0. Exported so a test can pin it. */
export function secondsRemaining(delayMs: number, elapsedMs: number): number {
  return Math.max(0, Math.ceil((delayMs - elapsedMs) / 1000));
}

/**
 * The status line shown while the engine waits out an API retry.
 *
 * The engine has always computed this (`llm-client/src/service.rs`, "surfaced
 * to the UI so it can show a Claude-Code-style 'Retrying in Ns… (attempt X/Y)'
 * status"), and it reached the renderer — but its only consumer was the
 * Diagnostics settings page. A user waiting on a rate-limited request saw a
 * bare "Thinking…" for as long as the backoff lasted, with no sign that
 * anything was happening or that it would be retried.
 *
 * The countdown is live rather than a snapshot of `delay_ms`: a backoff can run
 * to minutes, and a frozen "Retrying in 60s…" would be wrong for all but its
 * first second.
 */
export function ApiRetryNotice({ retry }: { retry: ApiRetryStatus }) {
  const t = useT();
  // Keyed on the event, so a second retry restarts the countdown rather than
  // continuing to run down the first one's clock.
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    setElapsed(0);
    const started = Date.now();
    const id = setInterval(() => setElapsed(Date.now() - started), 1000);
    return () => clearInterval(id);
  }, [retry]);
  const left = secondsRemaining(retry.delay_ms, elapsed);
  return (
    // Coloured as an error, not as neutral progress: this row exists because a
    // call FAILED, and the retry is the recovery attempt. Rendering it in the
    // same grey as "Thinking…" reads as ordinary work in progress, which is the
    // impression that made a rate-limited turn look like nothing was wrong.
    <div className="transcript-run-item transcript-api-retry" data-run-type="api-retry" role="status">
      <span
        className="running-sweep"
        style={{
          '--sweep-base': t.danger,
          // The sweep band must stay ON the danger hue. `t.text` is what the
          // neutral "Thinking…" sweep uses, and over red it reads as a dark
          // smear rather than a shimmer. Lightening `danger` keeps the row one
          // colour, and the reduced-motion rule below falls back to the base.
          '--sweep-highlight': `color-mix(in oklab, ${t.danger} 55%, white)`,
        } as CSSProperties}
      >
        {left > 0 ? `Retrying in ${left}s…` : 'Retrying…'}
      </span>
      <span style={{ color: t.danger, fontSize: 13, marginLeft: 8 }}>
        {`attempt ${retry.attempt}/${retry.max_retries} · ${retry.message}`}
      </span>
    </div>
  );
}
