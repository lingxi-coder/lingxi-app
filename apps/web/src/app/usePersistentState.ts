import { Dispatch, SetStateAction, useEffect, useState } from 'react';

export function usePersistentState<T>(
  key: string,
  initialValue: T | (() => T),
): [T, Dispatch<SetStateAction<T>>] {
  const [value, setValue] = useState<T>(() => {
    const fallback = typeof initialValue === 'function' ? (initialValue as () => T)() : initialValue;
    try {
      const stored = window.localStorage.getItem(key);
      return stored ? (JSON.parse(stored) as T) : fallback;
    } catch {
      return fallback;
    }
  });

  useEffect(() => {
    try {
      window.localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Persistence failures must leave the current in-memory state usable.
    }
  }, [key, value]);

  return [value, setValue];
}
