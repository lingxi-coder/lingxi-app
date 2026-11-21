import { DATA_PROTOCOL_VERSION, isResourceRef, type ResourceRef } from '../../../../../packages/teams/dataset-resource.js';
import type { LocalAppDataBinding } from '../../shared/localApps.js';

const id = /^[A-Za-z0-9_-]{1,64}$/u;

type ReadReference = Exclude<ResourceRef, { kind: 'pageVersion' }>;
type DataAccess = 'read' | 'edit';

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
  const raw = object(input);
  if (Object.keys(raw).some(key => key !== 'reference' && key !== 'via')) throw new Error('Unknown data binding property');
  if (!isResourceRef(raw.reference) || raw.reference.kind === 'pageVersion') throw new Error('Invalid data binding reference');
  return { reference: raw.reference, ...(raw.via === undefined ? {} : { via: identifier(raw.via) }) };
}

function descriptor(binding: LocalAppDataBinding, command: Record<string, unknown>): Record<string, unknown> {
  const { reference, via } = binding;
  return { protocol: DATA_PROTOCOL_VERSION, dataset: reference.dataset, ...(via ? { via } : {}), command };
}

function cursor(value: unknown, max: number): string {
  if (typeof value !== 'string' || !value || value.length > max || /[\u0000-\u001f\u007f]/u.test(value)) throw new Error('Invalid data cursor');
  return value;
}

function fixedRead(reference: ReadReference, input: Record<string, unknown>): Record<string, unknown> {
  if (reference.kind === 'record' && input.action === 'get') return { action: 'read', reference };
  if (reference.kind === 'snapshot' && input.action === 'list') {
    return { action: 'openSnapshot', snapshot: reference.snapshot, block: input.block === undefined ? 0 : version(input.block, 0) };
  }
  if (reference.kind === 'reportRelease' && input.action === 'getReport') {
    return { action: 'openReport', release: reference.release, ...(input.after === undefined ? {} : { after: cursor(input.after, 160) }) };
  }
  throw new Error('The action requires a live data binding');
}

export function localAppDataRequest(binding: LocalAppDataBinding, access: DataAccess, input: unknown, operation: unknown): Record<string, unknown> {
  const raw = object(input), { reference } = binding;
  if (raw.action === 'describe') return descriptor(binding, { action: 'describe' });
  if (reference.kind !== 'dataset') return descriptor(binding, fixedRead(reference, raw));

  switch (raw.action) {
    case 'get':
      return descriptor(binding, { action: 'read', reference: { kind: 'record', dataset: reference.dataset,
        record: identifier(raw.record), revision: version(raw.revision, 1), schemaVersion: version(raw.schemaVersion, 1) } });
    case 'latest':
      return descriptor(binding, { action: 'latest', record: identifier(raw.record) });
    case 'list':
      return descriptor(binding, { action: 'list',
        ...(raw.after === undefined ? {} : { after: cursor(raw.after, 320) }),
        ...(raw.limit === undefined ? {} : { limit: version(raw.limit, 1) }) });
    case 'put': {
      if (access !== 'edit') throw new Error('This data binding is read-only');
      if (typeof raw.partition !== 'string' || !raw.partition || Buffer.byteLength(raw.partition) > 160 || /[\u0000-\u001f\u007f]/u.test(raw.partition)) throw new Error('Invalid record partition');
      if (raw.archive !== undefined && typeof raw.archive !== 'boolean') throw new Error('Invalid archive value');
      const command = descriptor(binding, { action: 'put', record: identifier(raw.record), partition: raw.partition,
        schemaVersion: version(raw.schemaVersion, 1), expectedRevision: version(raw.expectedRevision, 0), value: object(raw.value),
        ...(raw.archive === undefined ? {} : { archive: raw.archive }) });
      return { ...command, operation: identifier(operation) };
    }
    case 'getReport':
      return descriptor(binding, { action: 'openReport', release: identifier(raw.release) });
    default:
      throw new Error('Unsupported application data operation');
  }
}
