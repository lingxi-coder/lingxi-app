#!/usr/bin/env node
import { X509Certificate } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const webRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const variantPackages = { play: 'com.lingxi.code', direct: 'com.lingxi.code.direct' };
const productionPackages = Object.values(variantPackages);

/** Only fingerprints derived from supplied public production certificates count
 * as verified. Debug package variants are never production associations.
 */
export function validateAndroidReleaseAssociations(entries, verifiedCertificates, variant = 'all') {
  if (variant !== 'all' && !Object.hasOwn(variantPackages, variant)) throw new Error('Expected --variant all, direct or play.');
  const requiredPackages = variant === 'all' ? productionPackages : [variantPackages[variant]];
  if (!Array.isArray(entries) || entries.length === 0) throw new Error('Android release association has no verified signing entries. Supply production Play/direct public certificates; debug signing is insufficient.');
  const covered = new Set();
  for (const entry of entries) {
    const target = entry.target;
    if (!Array.isArray(entry.relation) || !entry.relation.includes('delegate_permission/common.handle_all_urls') || target?.namespace !== 'android_app' || !productionPackages.includes(target.package_name)) throw new Error('Association must target a configured production package and handle_all_urls relation.');
    if (!requiredPackages.includes(target.package_name)) continue;
    const fingerprints = target.sha256_cert_fingerprints;
    if (!Array.isArray(fingerprints) || !fingerprints.length || fingerprints.some(fingerprint => typeof fingerprint !== 'string' || !/^(?:[0-9A-F]{2}:){31}[0-9A-F]{2}$/u.test(fingerprint))) throw new Error('Association has a missing or invalid SHA-256 certificate fingerprint.');
    const verified = verifiedCertificates.get(target.package_name) ?? new Set();
    if (fingerprints.some(fingerprint => !verified.has(fingerprint))) throw new Error(`Association for ${target.package_name} contains a fingerprint not verified from a supplied production certificate.`);
    covered.add(target.package_name);
  }
  for (const name of requiredPackages) {
    if (!covered.has(name)) {
      const guidance = name === variantPackages.play ? ' Verify the actual Play App Signing certificate first.' : '';
      throw new Error(`Missing production association for ${name}.${guidance}`);
    }
  }
}

export function verifyDeepLinkRelease(argv, root = webRoot) {
  const certificates = new Map();
  let variant = 'all';
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--variant') {
      variant = argv[++i];
      if (!['all', 'play', 'direct'].includes(variant)) throw new Error('Expected --variant all, direct or play.');
      continue;
    }
    if (argv[i] !== '--certificate') throw new Error('Usage: [--variant all|direct|play] --certificate package=/path/to/public-signing.der (repeat for certificate rotations)');
    const value = argv[++i] ?? '';
    const equal = value.indexOf('=');
    const name = value.slice(0, equal);
    if (equal <= 0 || !productionPackages.includes(name) || !value.slice(equal + 1)) throw new Error('Expected a configured production package and public certificate path.');
    const fingerprint = new X509Certificate(readFileSync(value.slice(equal + 1))).fingerprint256;
    if (!certificates.has(name)) certificates.set(name, new Set());
    certificates.get(name).add(fingerprint);
  }
  const files = ['apple-app-site-association', 'assetlinks.json'];
  const android = JSON.parse(readFileSync(resolve(root, 'public/.well-known/assetlinks.json'), 'utf8'));
  validateAndroidReleaseAssociations(android, certificates, variant);
  const apple = JSON.parse(readFileSync(resolve(root, 'public/.well-known/apple-app-site-association'), 'utf8'));
  const details = apple.applinks?.details;
  const identities = details?.flatMap(detail => detail.appIDs ?? []) ?? [];
  if (identities.length !== 2 || !['AZ4AX7J833.com.lingxi.code', 'AZ4AX7J833.com.lingxi.code.full'].every(id => identities.includes(id)) || !details.some(detail => detail.components?.some(component => component['/'] === '/open/*'))) throw new Error('AASA does not match configured production iOS identities and /open/* paths.');
  for (const name of files) {
    const source = readFileSync(resolve(root, 'public/.well-known', name));
    const built = readFileSync(resolve(root, 'dist/.well-known', name));
    if (!source.equals(built)) throw new Error(`Built ${name} is stale. Run the web build before publication.`);
  }
  return variant;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const variant = verifyDeepLinkRelease(process.argv.slice(2));
    process.stdout.write(`Deep-link ${variant} association release preflight passed.\n`);
  } catch (error) {
    process.stderr.write(`Deep-link release gate: ${error.message}\n`);
    process.exitCode = 1;
  }
}
