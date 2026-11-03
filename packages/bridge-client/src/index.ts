/**
 * `@lingxi/bridge-client` — TypeScript wire-protocol types + a Node-side
 * {@link BridgeClient} for the LingXi bridge-server (M10 A1).
 */

export * from './protocol.js';
export * from './uiControlCommands.js';
export * from './toolview.js';
export * from './lockfile.js';
export * from './validation.js';
export * from './eventCorrelation.js';
export * from './workspace.js';
export { versionCompatible } from './version.js';
export {
  BridgeClient,
  SessionMetadataUnconfirmedError,
  type BridgeClientOptions,
  type BridgeClientEvents,
  type SessionMetadataOwner,
} from './client.js';
