import type { LocalAppDataBinding } from '../../shared/localApps.js';
import { parseDataRef, type DataRequest } from '../../../../../packages/teams/lingxi-data.js';

const id = /^[A-Za-z0-9_-]{1,64}$/u;

function object(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid application data request');
  return value as Record<string, unknown>;
}

function identifier(value: unknown): string {
  if (typeof value !== 'string' || !id.test(value)) throw new Error('Invalid data identifier');
  return value;
}

function version(value: unknown, min: number): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < min) throw new Error('Invalid data version');
  return value;
}

export function parseLocalAppDataBinding(input: unknown): LocalAppDataBinding {
  const raw = object(input), scope = object(raw.scope);
  const dataset = identifier(raw.dataset);
  if (scope.kind === 'personal') return { dataset, scope: { kind: 'personal' } };
  if (scope.kind !== 'team') throw new Error('Invalid data binding scope');
  return { dataset, scope: { kind: 'team', teamId: identifier(scope.teamId), conversationId: identifier(scope.conversationId), generation: version(scope.generation, 1) } };
}

export function localAppDataRequest(binding: LocalAppDataBinding, access: 'read' | 'edit', input: unknown, operation: unknown): DataRequest {
  const raw = object(input), { scope, dataset } = binding;
  switch (raw.action) {
    case 'describe': return { scope, command: { action: 'describe', dataset } };
    case 'get': {
      const reference = parseDataRef({ dataset, record: raw.record, revision: raw.revision, schemaVersion: raw.schemaVersion });
      if (!reference) throw new Error('Invalid versioned data reference');
      return { scope, command: { action: 'get', reference } };
    }
    case 'list': return { scope, command: { action: 'list', dataset,
      ...(raw.after === undefined ? {} : { after: identifier(raw.after) }),
      ...(raw.limit === undefined ? {} : { limit: version(raw.limit, 1) }) } };
    case 'events': return { scope, command: { action: 'events', dataset, after: version(raw.after, 0) } };
    case 'put':
      if (access !== 'edit') throw new Error('This data binding is read-only');
      if (raw.archive !== undefined && typeof raw.archive !== 'boolean') throw new Error('Invalid archive value');
      return { scope, operation: identifier(operation), command: {
        action: 'put', dataset, record: identifier(raw.record), schemaVersion: version(raw.schemaVersion, 1),
        expectedRevision: version(raw.expectedRevision, 0), value: object(raw.value),
        ...(raw.archive === undefined ? {} : { archive: raw.archive }),
      } };
    default: throw new Error('Unsupported application data operation');
  }
}
