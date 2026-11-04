import { createContext, useEffect, useState } from 'react';
import type { AccountApi, AccountState } from '../../shared/account';
import { DESKTOP_BRIDGE_GLOBAL } from '../../shared/desktopBridge';
import { tr } from '../i18n/locale';

export const AccountSignInContext = createContext<(() => void) | null>(null);

export function useAccountState(api: AccountApi | undefined = typeof window === 'undefined' ? undefined : window[DESKTOP_BRIDGE_GLOBAL]?.account) {
  const [state, setState] = useState<AccountState | null>(null);
  const [readError, setReadError] = useState('');
  useEffect(() => {
    if (!api) return;
    let active = true;
    let revision = 0;
    const off = api.onChanged(value => { revision++; if (active) setState(value); });
    void api.request({ type: 'status' }).then(value => {
      if (active && revision === 0) setState(value);
    }).catch(() => { if (active) setReadError(tr('desktop_shell_account_read_error')); });
    return () => { active = false; off(); };
  }, [api]);
  return { state, setState, readError };
}
