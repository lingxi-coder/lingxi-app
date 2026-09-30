import { createContext, useContext } from 'react';
import type { Tokens } from './tokens';

export const Theme = createContext<Tokens | null>(null);

/** Access the active theme tokens. The provider always supplies a value. */
export const useT = (): Tokens => {
  const t = useContext(Theme);
  if (!t) throw new Error('useT must be used within a <Theme.Provider>');
  return t;
};
