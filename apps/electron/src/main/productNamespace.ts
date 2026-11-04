import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, parse, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import * as electron from 'electron';
import { decodeProductNamespace } from './productNamespaceCodec.mjs';

declare const __PRODUCT_NAMESPACE_BINDING__: string | undefined;

function sourceNamespacePath(): string {
  // ESM source and the development bundle are anchored to their own checkout.
  // CJS test bundles have no import.meta.url and run from their source checkout.
  const location = typeof import.meta.url === 'string' ? dirname(fileURLToPath(import.meta.url)) : process.cwd();
  let directory = resolve(location);
  while (true) {
    for (const candidate of [directory, join(directory, 'apps', 'electron')]) {
      const manifest = join(candidate, 'package.json');
      try {
        if (JSON.parse(readFileSync(manifest, 'utf8')).name === 'lingxi-code-desktop') {
          return join(candidate, '.native-cache', 'branding', 'product-namespace.json');
        }
      } catch { /* This directory is not the desktop source root. */ }
    }
    const parent = dirname(directory);
    if (parent === directory || directory === parse(directory).root) break;
    directory = parent;
  }
  throw new Error('desktop source root is unavailable');
}

function loadProductNamespace() {
  const packaged = electron.app?.isPackaged === true;
  const binding = typeof __PRODUCT_NAMESPACE_BINDING__ === 'undefined' ? undefined : __PRODUCT_NAMESPACE_BINDING__;
  if (packaged && binding === undefined) throw new Error('packaged main bundle has no product namespace binding');
  const path = packaged
    ? join(electron.app.getAppPath(), 'out', 'main', 'product-namespace.json')
    : sourceNamespacePath();
  try {
    if (!existsSync(path)) throw new Error('namespace resource is missing');
    return Object.freeze(decodeProductNamespace(readFileSync(path), binding));
  } catch (error) {
    throw new Error('Product namespace is missing, invalid, or does not match the main bundle', { cause: error });
  }
}

/** Loaded once from the generated source resource or from the packaged ASAR. */
export const productNamespace = loadProductNamespace();
export const productEnvironmentKey = (suffix: string): string => `${productNamespace.envPrefix}${suffix}`;
