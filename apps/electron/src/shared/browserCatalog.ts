/** Browsers with an explicit Desktop adapter. A Chromium engine alone is not a support claim. */
export const browserCatalog = [
  { id: 'chrome', name: 'Google Chrome', bundleId: 'com.google.Chrome', profileDirectory: 'Google/Chrome', extensionsPage: 'chrome://extensions' },
  { id: 'edge', name: 'Microsoft Edge', bundleId: 'com.microsoft.edgemac', profileDirectory: 'Microsoft Edge', extensionsPage: 'edge://extensions' },
  { id: 'brave', name: 'Brave', bundleId: 'com.brave.Browser', profileDirectory: 'BraveSoftware/Brave-Browser', extensionsPage: 'brave://extensions' },
  { id: 'chromium', name: 'Chromium', bundleId: 'org.chromium.Chromium', profileDirectory: 'Chromium', extensionsPage: 'chrome://extensions' },
  { id: 'safari', name: 'Safari', bundleId: 'com.apple.Safari', profileDirectory: null, extensionsPage: null },
  { id: 'firefox', name: 'Firefox', bundleId: 'org.mozilla.firefox', profileDirectory: null, extensionsPage: null },
] as const;

export type BrowserId = typeof browserCatalog[number]['id'];
export function browserDefinition(id: unknown) { return browserCatalog.find(browser => browser.id === id); }
export const browserSettingsPath = 'Settings → Computer use → Browser';
