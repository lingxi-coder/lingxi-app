import { invalidateLanguagePreference, LANGUAGE_STORAGE_KEY, type DesktopLocale } from '../src/renderer/i18n/locale';

/** Install an isolated language preference for Node renderer tests. */
export function installTestLocale(locale: DesktopLocale): () => void {
  const previous = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null },
  });
  invalidateLanguagePreference();
  return () => {
    if (previous) Object.defineProperty(globalThis, 'localStorage', previous);
    else Reflect.deleteProperty(globalThis, 'localStorage');
    invalidateLanguagePreference();
  };
}
