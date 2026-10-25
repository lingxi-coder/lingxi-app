import { LANGUAGE_STORAGE_KEY, type LanguagePreference } from '../../src/renderer/i18n/locale';

/** Pin UI language in a synchronous render/unit test without changing the host. */
export function withDesktopLocale<T>(locale: LanguagePreference, run: () => T): T {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? locale : null },
  });
  try { return run(); }
  finally {
    if (descriptor) Object.defineProperty(globalThis, 'localStorage', descriptor);
    else Reflect.deleteProperty(globalThis, 'localStorage');
  }
}
