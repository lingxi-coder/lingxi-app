import { tr } from '../i18n/locale';

const REQUIRES_CODE = 'local_app_development_requires_code';
const REQUIRES_CODE_REASON = /^\s*(?:Error:\s*)?local_app_development_requires_code(?=$|[\s:])/;

/** Translate the stable SDK reason without altering other diagnostic text. */
export function localAppErrorMessage(value: unknown): string {
  const message = value instanceof Error ? value.message : String(value);
  return REQUIRES_CODE_REASON.test(message) ? tr(REQUIRES_CODE) : message;
}

export function localAppToolRequiresCode(tool: string, status: string, ...parts: Array<string | undefined>): boolean {
  return /^local_?app$/i.test(tool) && status === 'error' && parts.some(part => part !== undefined && REQUIRES_CODE_REASON.test(part));
}
