import { Dispatch, SetStateAction, useCallback, useEffect, useState } from 'react';

export type Theme = 'light' | 'dark';
const key = 'lingxi.web.theme';
function readTheme(): Theme {
  try { return JSON.parse(localStorage.getItem(key) ?? 'null') === 'dark' ? 'dark' : 'light'; }
  catch { return 'light'; }
}

export function useTheme(): [Theme, Dispatch<SetStateAction<Theme>>] {
  const [theme, updateTheme] = useState<Theme>(readTheme);
  const setTheme = useCallback<Dispatch<SetStateAction<Theme>>>((next) => {
    const value = typeof next === 'function' ? next(document.documentElement.dataset.theme === 'dark' ? 'dark' : 'light') : next;
    document.documentElement.dataset.theme = value;
    try { localStorage.setItem(key, JSON.stringify(value)); } catch { /* Theme remains usable without storage. */ }
    updateTheme(value);
  }, []);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => {
    const sync = (event: StorageEvent) => {
      if (event.key === key || event.key === null) updateTheme(readTheme());
    };
    window.addEventListener('storage', sync);
    return () => window.removeEventListener('storage', sync);
  }, []);
  return [theme, setTheme];
}
