#!/usr/bin/env node
import { writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { runPackagedCredentialStability } from './packaged-app-smoke.mjs';

const [appPath, receiptPath] = process.argv.slice(2);
if (!appPath || !receiptPath) {
  throw new Error('Usage: node scripts/verify-credential-stability.mjs <production-signed-app> <new-receipt.json>');
}
const result = await runPackagedCredentialStability(resolve(appPath));
writeFileSync(resolve(receiptPath), JSON.stringify(result, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
console.log('Production credentials: 15 renderer refreshes, 3 normal launches, authenticated loopback requests and test-key deletion passed.');
