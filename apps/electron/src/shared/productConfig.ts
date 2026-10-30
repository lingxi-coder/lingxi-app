import config from '../../../../config/product-config.json';
export const localAppMarketApiRoot = config.localAppMarketApiRoot;
export const localAppMarketUrl = new URL('/', localAppMarketApiRoot).href;
export const desktopUpdateManifestUrl = config.desktopUpdateManifestUrl;
