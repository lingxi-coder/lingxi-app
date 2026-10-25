import { useEffect, useState } from 'react';
import type { Locale } from '../utils/locale';
import { failureStringKey } from './failureText';
import { formatPairingCode } from './text';
import { t } from './strings';
import type { ConnectionState, ViewerConnection } from './viewerConnection';

interface ConnectionScreenProps {
  locale: Locale;
  hostName: string;
  state: ConnectionState;
  connection: ViewerConnection;
  onLeave(): void;
  onForget(): void;
}

/** Whole seconds until `retryAt` (a `Date.now()` time), counting down while mounted. */
function useCountdown(retryAt: number | undefined): number | undefined {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (retryAt === undefined) return undefined;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 500);
    return () => window.clearInterval(timer);
  }, [retryAt]);
  return retryAt === undefined ? undefined : Math.max(0, Math.ceil((retryAt - now) / 1000));
}

/** What the page shows until the connection is ready: dialling, pairing, waiting to retry, or a reason it stopped. */
export function ConnectionScreen({ locale, hostName, state, connection, onLeave, onForget }: ConnectionScreenProps) {
  const seconds = useCountdown(state.retryAt);
  if (state.phase === 'awaiting_approval' && state.pairingCode) {
    return (
      <section className="rc-panel rc-pairing" aria-live="polite">
        <h2>{t(locale, 'pairingTitle')}</h2>
        <p>{t(locale, 'pairingBody', { name: hostName })}</p>
        <p className="rc-code-label">{t(locale, 'pairingCode')}</p>
        <p className="rc-pairing-code" aria-label={state.pairingCode.split('').join(' ')}>{formatPairingCode(state.pairingCode)}</p>
        <p className="rc-note" role="status">{t(locale, 'pairingWaiting')}</p>
        <div className="rc-actions">
          <button type="button" className="button secondary" onClick={onLeave}>{t(locale, 'cancel')}</button>
        </div>
      </section>
    );
  }

  if (state.phase === 'failed' && state.failure) {
    const { failure } = state;
    return (
      <section className="rc-panel" role="alert">
        <h2>{hostName}</h2>
        <p>{t(locale, failureStringKey(failure))}</p>
        {failure.kind === 'revoked' ? <p className="rc-note">{t(locale, 'pairAgainNote')}</p> : null}
        <div className="rc-actions">
          {failure.kind === 'revoked' ? (
            <button type="button" className="button primary" onClick={() => void connection.repair()}>{t(locale, 'pairAgain')}</button>
          ) : failure.kind === 'verification' || failure.kind === 'pairing_code_mismatch' ? (
            <button type="button" className="button secondary" onClick={onForget}>{t(locale, 'forget')}</button>
          ) : failure.kind === 'unsupported' || failure.kind === 'upgrade_required' || failure.kind === 'disabled' ? null : (
            <button type="button" className="button primary" onClick={() => connection.retry()}>{t(locale, 'retry')}</button>
          )}
          <button type="button" className="button secondary" onClick={onLeave}>{t(locale, 'switchComputer')}</button>
        </div>
      </section>
    );
  }

  const waiting = state.phase === 'reconnecting' && state.retryAt !== undefined;
  return (
    <section className="rc-panel" role="status">
      <h2>{state.everReady ? t(locale, 'reconnecting') : t(locale, 'connectingTo', { name: hostName })}</h2>
      {waiting && state.failure ? <p className="rc-note">{t(locale, failureStringKey(state.failure))}</p> : null}
      {waiting && seconds !== undefined ? <p className="rc-note">{t(locale, 'retryIn', { seconds })}</p> : null}
      <div className="rc-actions">
        <button type="button" className="button secondary" onClick={onLeave}>{t(locale, 'cancel')}</button>
      </div>
    </section>
  );
}
