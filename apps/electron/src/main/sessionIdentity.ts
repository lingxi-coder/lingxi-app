import type { SessionRef } from './bridgeTypes.js';

const SESSION_ID_PATTERN = /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i;

export function isSessionId(value: unknown): value is string {
  return typeof value === 'string' && SESSION_ID_PATTERN.test(value);
}

export function assertSessionRef(ref: SessionRef): void {
  if (!isSessionId(ref.sessionId)) throw new Error('invalid session id');
  if (typeof ref.projectPath !== 'string' || ref.projectPath.length === 0 || ref.projectPath.length > 32_768 || ref.projectPath.includes('\0')) {
    throw new Error('invalid project path');
  }
}
