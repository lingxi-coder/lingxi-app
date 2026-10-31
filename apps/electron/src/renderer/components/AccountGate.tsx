import { useCallback, useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { AccountSignInContext, useAccountState } from '../bridge/useAccountState';
import { tr } from '../i18n/locale';
import { useT } from '../theme/ThemeContext';
import { LingxiAccount } from './settings/pages/LingxiAccount';

/** Keep local work mounted while an expired account is authenticated again. */
export function AccountGate({ children }: { children: ReactNode }) {
  const t = useT();
  const { state } = useAccountState();
  const [requested, setRequested] = useState(false);
  const requestSignIn = useCallback(() => setRequested(true), []);
  const available = typeof window !== 'undefined' && Boolean(window.lingxi?.account);
  const blocked = available && (!state || (!state.user && (requested || state.sessionExpired === true)));
  const screen = useRef<HTMLElement>(null);
  useLayoutEffect(() => {
    if (!blocked) return;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    screen.current?.focus();
    return () => { if (previous?.isConnected) previous.focus(); };
  }, [blocked]);
  return <AccountSignInContext.Provider value={requestSignIn}>
    <div aria-hidden={blocked ? true : undefined} style={{ display: blocked ? 'none' : 'contents' }}>{children}</div>
    {blocked && <main ref={screen} tabIndex={-1} aria-label={tr('desktop_shell_sign_in')}
      onKeyDown={event => event.stopPropagation()} onKeyUp={event => event.stopPropagation()}
      style={{ position: 'fixed', inset: 0, zIndex: 1000, overflow: 'auto', display: 'grid', alignItems: 'center', background: t.stageBg, color: t.text, padding: '40px 16px', boxSizing: 'border-box', outline: 'none' }}>
      <div className="drag-region" aria-hidden="true" style={{ position: 'fixed', top: 0, left: 0, right: 0, height: 28 }} />
      <div className="no-drag"><LingxiAccount standalone /></div>
    </main>}
  </AccountSignInContext.Provider>;
}
