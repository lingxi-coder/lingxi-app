import { createHash } from 'node:crypto';
import { posix, win32 } from 'node:path';

const BINDING_PREFIX = 'product-namespace-sha256:';

export function productNamespaceBinding(data) {
  return `${BINDING_PREFIX}${createHash('sha256').update(data).digest('hex')}`;
}

export function productNamespaceBindingFromBundle(bundle) {
  const bindings = new Set(bundle.toString('utf8').match(/product-namespace-sha256:[0-9a-f]{64}/g) ?? []);
  if (bindings.size !== 1) throw new Error('main bundle has no unique product namespace binding');
  return [...bindings][0];
}

export function decodeProductNamespace(data, binding) {
  if (binding !== undefined && productNamespaceBinding(data) !== binding) {
    throw new Error('product namespace does not match its main bundle');
  }
  const value = JSON.parse(data.toString('utf8'));
  const fields = ['envPrefix', 'configDirEnv', 'dotDir', 'globalConfigFile', 'managedDirMacOS',
    'managedDirWindows', 'managedDirUnix', 'localAppPreparationMarker',
    'localAppPreparationDataDirectory', 'remoteAttachmentPrefix'];
  const components = ['dotDir', 'globalConfigFile', 'localAppPreparationMarker',
    'localAppPreparationDataDirectory', 'remoteAttachmentPrefix'];
  if (!value || typeof value !== 'object' || Array.isArray(value) || value.version !== 1
    || !/^[0-9a-f]{40}$/.test(value.sourceRevision)
    || fields.some(key => typeof value[key] !== 'string' || !value[key] || value[key].length > 4096 || value[key].includes('\0'))
    || !/^[A-Z][A-Z0-9_]*_$/.test(value.envPrefix) || !/^[A-Z][A-Z0-9_]*$/.test(value.configDirEnv)
    || components.some(key => value[key] === '.' || value[key] === '..' || /[/\\]/.test(value[key]))
    || !posix.isAbsolute(value.managedDirMacOS) || !posix.isAbsolute(value.managedDirUnix)
    || !win32.isAbsolute(value.managedDirWindows)) throw new Error('unsupported product namespace');
  return value;
}
