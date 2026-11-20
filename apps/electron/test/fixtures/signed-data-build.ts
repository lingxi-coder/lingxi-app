import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import type { Plugin } from 'vite';

function port(value: string): number {
  const url = new URL(value.includes('://') ? value : 'http://' + value);
  const result = Number(url.port);
  if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || !Number.isInteger(result) || result < 1024 || result > 65535) {
    throw new Error('Signed Data staging requires unprivileged loopback endpoints');
  }
  return result;
}

export function signedDataBuild(directory: string): Plugin[] {
  const file = process.env.LINGXI_SIGNED_REPORT_STAGING;
  if (!file) return [];
  if (process.env.LINGXI_CREDENTIAL_BROKER_CHANNEL !== 'development') throw new Error('Signed Data staging requires the development channel');
  const line = readFileSync(file, 'utf8').split('\n').find(entry => entry.startsWith('{"web"'));
  if (!line) throw new Error('Missing Data staging endpoints');
  const endpoints = JSON.parse(line);
  if (endpoints.project !== 'demo-lingxi-data') throw new Error('Expected the isolated Data project');
  const ports = { auth: port(endpoints.auth), firestore: port(endpoints.firestore), api: port(endpoints.api) };
  const entry = resolve(directory, 'src/main/index.ts');
  const replacement = resolve(directory, 'test/fixtures/signed-data-services.ts');
  return [{
    name: 'signed-data-staging',
    enforce: 'pre',
    config: () => ({ define: { __LINGXI_DATA_STAGING_PORTS__: JSON.stringify(ports) } }),
    resolveId(source, importer) {
      if (importer === entry && (source === './account.js' || source === './teams/service.js')) return replacement;
      return null;
    },
  }];
}
