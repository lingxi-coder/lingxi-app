import config from '../../../../config/product-config.json';
export const localAppMarketApiRoot = config.localAppMarketApiRoot;
export const localAppMarketUrl = new URL('/', localAppMarketApiRoot).href;
export function parseBotMarketApiRoot(value: unknown): string {
  if (typeof value !== 'string') throw new Error('Invalid Bot market API root');
  const url = new URL(value);
  if (url.protocol !== 'https:' || url.username || url.password || url.search || url.hash
    || url.pathname !== '/api/v3/') throw new Error('Invalid Bot market API root');
  return url.href;
}
export const botMarketApiRoot = parseBotMarketApiRoot(config.botMarketApiRoot);
export const desktopUpdateManifestUrl = config.desktopUpdateManifestUrl;
