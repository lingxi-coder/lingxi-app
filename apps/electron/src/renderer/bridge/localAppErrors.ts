import { tr } from '../i18n/locale';
import { classifyDesktopError, type ActionableError } from './errors';
import type { LocalAppAuthoringError } from './localAppChat';

const REQUIRES_CODE = 'local_app_development_requires_code';
const REQUIRES_CODE_REASON = /^\s*(?:Error:\s*)?local_app_development_requires_code(?=$|[\s:])/;

/** Preserve the chosen project and retry instructions without exposing a raw host error. */
export function localAppAuthoringErrorPresentation(error: LocalAppAuthoringError): ActionableError {
  const rawCause = error.cause instanceof Error ? error.cause.message : typeof error.cause === 'string' ? error.cause : '';
  const diagnosis = classifyDesktopError(rawCause);
  const title = tr('desktop_misc_action_failed');
  // Classifier titles are fixed translations. Its detail or technicalDetail
  // may contain the host's raw input and must not be exposed here.
  const cause = diagnosis.title === title ? '' : `\n${diagnosis.title}`;
  return { title, detail: `${error.message}${cause}` };
}

/** Translate the stable SDK reason without altering other diagnostic text. */
export function localAppErrorMessage(value: unknown): string {
  const message = value instanceof Error ? value.message : String(value);
  return REQUIRES_CODE_REASON.test(message) ? tr(REQUIRES_CODE) : message;
}

export function localAppToolRequiresCode(tool: string, status: string, ...parts: Array<string | undefined>): boolean {
  return /^local_?app$/i.test(tool) && status === 'error' && parts.some(part => part !== undefined && REQUIRES_CODE_REASON.test(part));
}
