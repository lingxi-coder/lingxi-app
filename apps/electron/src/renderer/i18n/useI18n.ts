import { useCallback, useEffect, useSyncExternalStore } from 'react';
import { currentLocale, invalidateLanguagePreference, LANGUAGE_STORAGE_KEY, readLanguagePreference, translate, translationRevision, type LanguagePreference } from './locale';
const CHANGE_EVENT = 'lingxi-language-change';
function subscribe(notify: () => void): () => void {
  const changed = () => { invalidateLanguagePreference(); notify(); };
  const storage = (event: StorageEvent) => { if (event.key === LANGUAGE_STORAGE_KEY || event.key === null) changed(); };
  window.addEventListener(CHANGE_EVENT, changed);
  window.addEventListener('languagechange', changed);
  window.addEventListener('storage', storage);
  return () => { window.removeEventListener(CHANGE_EVENT, changed); window.removeEventListener('languagechange', changed); window.removeEventListener('storage', storage); };
}
function snapshot() { return `${readLanguagePreference()}:${currentLocale()}:${translationRevision()}`; }
export function setLanguagePreference(preference: LanguagePreference): void {
  if (preference) localStorage.setItem(LANGUAGE_STORAGE_KEY, preference);
  else localStorage.removeItem(LANGUAGE_STORAGE_KEY);
  invalidateLanguagePreference();
  window.dispatchEvent(new Event(CHANGE_EVENT));
}
export function useI18n() {
  useSyncExternalStore(subscribe, snapshot, () => ':en');
  const locale = currentLocale();
  const revision = translationRevision();
  const tr = useCallback((key: string, ...args: readonly (string | number)[]) => translate(key, locale, ...args), [locale, revision]);
  useEffect(() => { document.documentElement.lang = locale; }, [locale]);
  return { tr, locale, preference: readLanguagePreference(), setPreference: setLanguagePreference };
}
