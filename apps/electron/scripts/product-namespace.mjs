import { execFileSync, spawnSync } from 'node:child_process';
import { copyFileSync, mkdirSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, resolve } from 'node:path';
import { extractFile } from '@electron/asar';
import { decodeProductNamespace, productNamespaceBindingFromBundle } from '../src/main/productNamespaceCodec.mjs';

const repoRoot = resolve(fileURLToPath(new URL('../../..', import.meta.url)));
const exporter = resolve(repoRoot, 'scripts/lib/export_product_namespace.py');
const output = resolve(repoRoot, 'apps/electron/.native-cache/branding/product-namespace.json');

function namespacePython() {
  const candidates = process.platform === 'win32'
    ? ['python', 'python3', 'python3.14', 'python3.13', 'python3.12', 'python3.11']
    : ['python3', 'python3.14', 'python3.13', 'python3.12', 'python3.11'];
  const python = candidates.find(candidate => spawnSync(candidate, ['-c', 'import tomllib'], { stdio: 'ignore' }).status === 0);
  if (!python) throw new Error('Product namespace export requires Python 3.11 or newer');
  return python;
}

/** Read the same Cargo-locked namespace used by the packaged client. */
export function productNamespace() {
  try { return decodeProductNamespace(readFileSync(output)); }
  catch (error) { throw new Error('Generated product namespace is missing or invalid; run the build first', { cause: error }); }
}

/** Verify the exact resource the packaged main process loads, for any platform. */
export function productNamespaceFromAsar(asar) {
  try {
    const binding = productNamespaceBindingFromBundle(extractFile(asar, 'out/main/index.js'));
    return decodeProductNamespace(extractFile(asar, 'out/main/product-namespace.json'), binding);
  } catch (error) {
    throw new Error('Packaged product namespace is missing, invalid, or does not match the main bundle', { cause: error });
  }
}

/** An acceptance run uses the namespace that actually shipped in its target app. */
export function packagedProductNamespace(appPath) {
  return productNamespaceFromAsar(join(appPath, 'Contents', 'Resources', 'app.asar'));
}

/** Generate the private build input consumed and bundled by Electron. */
export function generateProductNamespace() {
  execFileSync(namespacePython(), [exporter, '--output', output], { cwd: repoRoot, stdio: 'inherit' });
}

export function stageProductNamespace() {
  const buildDirectory = resolve(repoRoot, 'apps/electron/out/main');
  productNamespace();
  mkdirSync(buildDirectory, { recursive: true });
  copyFileSync(output, join(buildDirectory, 'product-namespace.json'));
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv[2] === '--stage-build') stageProductNamespace();
  else if (process.argv.length === 2) generateProductNamespace();
  else throw new Error('unknown product namespace command');
}
