import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateCrashlyticsConfig } from './desktop-crashlytics.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const [channel, symbols] = process.argv.slice(2);
if (!['development', 'production'].includes(channel) || !symbols || !existsSync(resolve(symbols))) {
  throw new Error('Usage: node scripts/upload-crashlytics-symbols.mjs development|production <dSYM-directory>');
}
const config = join(root, 'native', 'crashlytics', 'Firebase', channel, 'GoogleService-Info.plist');
const bundleId = channel === 'production' ? 'com.lingxi.code' : 'com.lingxi.code.development';
validateCrashlyticsConfig(JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', config], { encoding: 'utf8' })), bundleId);
const uploader = join(root, '.native-cache', 'crashlytics', 'checkouts', 'firebase-ios-sdk', 'Crashlytics', 'upload-symbols');
// Official Firebase uploader: do not implement a private Crashlytics protocol.
execFileSync(uploader, ['-gsp', config, '-p', 'mac', resolve(symbols)], { stdio: 'inherit' });
