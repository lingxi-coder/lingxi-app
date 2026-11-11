import type { DecisionInput } from './decision';

// The host also checks its complete envelope, including private credentials.
export const DECISION_INPUT_MAX_BYTES = 16 * 1024 * 1024;
const encoder = new TextEncoder();
type InputFields = { state: unknown; questions: unknown };
type ImageFileMetadata = { size: number; type: string };

function jsonBytes(value: object): number {
  const serialized = JSON.stringify(value);
  // Avoid allocating an encoded copy of an input already known to be too large.
  return serialized.length > DECISION_INPUT_MAX_BYTES ? serialized.length : encoder.encode(serialized).byteLength;
}

function assertBudget(bytes: number): void {
  if (bytes > DECISION_INPUT_MAX_BYTES) throw new Error('Decision evidence and images must fit within the 16 MiB request limit');
}

export function assertDecisionInputBudget(input: DecisionInput): void {
  assertBudget(jsonBytes(input));
}

export function assertDecisionImageSelectionBudget(files: readonly ImageFileMetadata[], input: InputFields): void {
  if (files.length > 8 || files.some(file => !['image/png', 'image/jpeg'].includes(file.type) || !Number.isSafeInteger(file.size) || file.size < 0 || file.size > 8 * 1024 * 1024)) {
    throw new Error('Choose up to eight PNG/JPEG images, each under 8 MiB');
  }
  const metadataBytes = jsonBytes({ ...input, images: files.map(file => ({ data_url: `data:${file.type};base64,` })) });
  const base64Bytes = files.reduce((bytes, file) => bytes + 4 * Math.ceil(file.size / 3), 0);
  assertBudget(metadataBytes + base64Bytes);
}
