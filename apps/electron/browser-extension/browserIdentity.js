/** Detect the host browser in the extension worker, never from page content. */
export function browserIdentity(navigator) {
  const brands = new Set(navigator?.userAgentData?.brands?.map(({ brand }) => brand) ?? []);
  const agent = navigator?.userAgent ?? '';
  if (brands.has('Microsoft Edge') || /\bEdg\//.test(agent)) return 'edge';
  if (brands.has('Brave') || typeof navigator?.brave?.isBrave === 'function') return 'brave';
  // Do not quietly treat other Chromium derivatives as Google Chrome.
  if (/\b(?:OPR|Vivaldi|YaBrowser|SamsungBrowser)\//.test(agent)) return null;
  if (brands.has('Google Chrome')) return 'chrome';
  if (brands.has('Chromium') || /\bChromium\//.test(agent)) return 'chromium';
  return null;
}
