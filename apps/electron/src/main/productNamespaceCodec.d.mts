export interface ProductNamespace {
  version: number;
  sourceRevision: string;
  envPrefix: string;
  configDirEnv: string;
  dotDir: string;
  globalConfigFile: string;
  managedDirMacOS: string;
  managedDirWindows: string;
  managedDirUnix: string;
  localAppPreparationMarker: string;
  localAppPreparationDataDirectory: string;
  remoteAttachmentPrefix: string;
}

export function productNamespaceBinding(data: Uint8Array): string;
export function productNamespaceBindingFromBundle(bundle: Uint8Array): string;
export function decodeProductNamespace(data: Uint8Array, binding?: string): ProductNamespace;
