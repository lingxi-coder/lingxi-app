/**
 * `@lingxi/bridge-client` — TypeScript wire-protocol types + a Node-side
 * {@link BridgeClient} for the LingXi bridge-server (M10 A1).
 */

export * from './protocol.js';
export * from './toolview.js';
export * from './lockfile.js';
export * from './validation.js';
export { versionCompatible } from './version.js';
export {
  BridgeClient,
  type BridgeClientOptions,
  type BridgeClientEvents,
} from './client.js';
