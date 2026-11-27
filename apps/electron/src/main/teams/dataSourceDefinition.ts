import { isAbsolute } from 'node:path';
import type { DataSourceKind, SourceDraft } from '../../../../../packages/teams/lingxi-data.js';

/** Source IDs and field names accepted in a data host definition. */
export const id = /^[A-Za-z0-9_-]{1,64}$/u;

function object(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value);
}

function keychain(value: unknown, service: string, account: string): boolean {
  if (!object(value) || Object.keys(value).length !== 1 || !object(value.keychain)) return false;
  return value.keychain.service === service && value.keychain.account === account;
}

/** The input field that holds a network database's connection secret. */
function connectionField(kind: string): 'dsn' | 'uri' | 'url' | undefined {
  switch (kind) {
    case 'mysql': case 'postgres': return 'dsn';
    case 'mongodb': return 'uri';
    case 'redis': return 'url';
    default: return undefined;
  }
}

/**
 * Reads a data host definition that the signed-in owner imports. A network database
 * connection must be the Desktop keychain item for its kind and source, never a test
 * file or the loopback test mode; file inputs need an absolute path.
 */
export function readDefinition(content: string, uid: string, expectedKind?: DataSourceKind): SourceDraft {
  const value: unknown = JSON.parse(content);
  if (!object(value) || value.owner !== uid || typeof value.source !== 'string' || !id.test(value.source)
    || !keychain(value.key, 'com.lingxi.data.host-key', value.source) || value.proxy != null
    || !object(value.presets) || Object.keys(value.presets).length !== 1 || !object(value.presets.main)) throw new Error('INVALID_INPUT');
  const { fields, input } = value.presets.main;
  if (!Array.isArray(fields) || !fields.length || fields.length > 64 || fields.some(field => typeof field !== 'string' || !id.test(field))
    || !object(input)) throw new Error('INVALID_INPUT');
  const kind = String(input.kind), connection = connectionField(kind);
  if (connection) {
    if (!keychain(input[connection], `com.lingxi.data.${kind}`, value.source) || input.isolatedLoopback === true) throw new Error('INVALID_INPUT');
  } else if (!['sqlite', 'csv', 'json'].includes(kind) || typeof input.path !== 'string' || !isAbsolute(input.path)) {
    throw new Error('INVALID_INPUT');
  }
  if (expectedKind !== undefined && kind !== expectedKind) throw new Error('DATA_SOURCE_KIND_MISMATCH');
  return { source: value.source, kind: kind as SourceDraft['kind'], fields: [...new Set(fields)] as string[] };
}
