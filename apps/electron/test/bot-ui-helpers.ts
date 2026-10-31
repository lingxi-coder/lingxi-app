import { after } from 'node:test';

import { LANGUAGE_STORAGE_KEY, invalidateLanguagePreference } from '../src/renderer/i18n/locale';

/** Make `tr` answer in English whatever the language of the computer running the tests. Undone when the test file ends. */
export function pinEnglish(): void {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'localStorage');
  Object.defineProperty(globalThis, 'localStorage', {
    configurable: true,
    value: { getItem: (key: string) => key === LANGUAGE_STORAGE_KEY ? 'en' : null, setItem() {}, removeItem() {} },
  });
  invalidateLanguagePreference();
  after(() => {
    if (original) Object.defineProperty(globalThis, 'localStorage', original);
    else Reflect.deleteProperty(globalThis, 'localStorage');
    invalidateLanguagePreference();
  });
}
