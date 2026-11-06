import productConfig from '../../../../config/product-config.json';

/** Reuse the unified market and its Desktop Bot handoff, with no second browser. */
export function botMarketLink(): string {
  const url = new URL('/', productConfig.localAppMarketApiRoot);
  url.searchParams.set('resourceKind', 'bot');
  url.searchParams.set('platform', 'macos');
  return url.href;
}
