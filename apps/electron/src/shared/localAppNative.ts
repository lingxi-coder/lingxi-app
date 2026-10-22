// Keep the guest bridge separate from the library preload. Sandboxed preloads
// cannot require shared Rollup chunks; each entry must be a standalone bundle.
export const CH_LOCAL_APP_NATIVE = 'lingxi:local-app:native';
