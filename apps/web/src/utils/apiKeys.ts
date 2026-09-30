import { ApiKeyRecord, ApiKeyStatus } from '../types/ApiKeyRecord';

export interface CreatedApiKey {
  record: ApiKeyRecord;
  secret: string;
}

function randomChunk(): string {
  const bytes = new Uint8Array(8);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('').slice(0, 8);
}

export function maskApiKeySecret(secret: string): string {
  if (secret.length <= 8) {
    return '••••••••';
  }
  return `${secret.slice(0, 7)}••••${secret.slice(-4)}`;
}

export function createLocalApiKey(name: string, scope: string, nowIso: string): CreatedApiKey {
  const suffix = `${randomChunk()}${randomChunk()}`;
  const secret = `lx_live_${suffix}`;
  const prefix = secret.slice(0, 7);
  return {
    secret,
    record: {
      id: `key_${randomChunk()}`,
      name,
      prefix,
      last4: secret.slice(-4),
      maskedSecret: maskApiKeySecret(secret),
      scope,
      createdAt: nowIso,
      lastUsedAt: 'Never',
      status: 'active',
    },
  };
}

export function renameApiKey(records: ApiKeyRecord[], id: string, name: string): ApiKeyRecord[] {
  return records.map((record) => (record.id === id ? { ...record, name } : record));
}

export function setApiKeyStatus(
  records: ApiKeyRecord[],
  id: string,
  status: ApiKeyStatus,
): ApiKeyRecord[] {
  return records.map((record) => (record.id === id ? { ...record, status } : record));
}

export function deleteApiKey(records: ApiKeyRecord[], id: string): ApiKeyRecord[] {
  return records.filter((record) => record.id !== id);
}
