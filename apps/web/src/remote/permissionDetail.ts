import { redactSensitiveText } from './reuse';

export interface DetailField {
  key: string;
  value: string;
}

/** Fields that decide what a tool will actually run; shown first so they cannot be scrolled past. */
const EXECUTABLE_KEYS = ['command', 'code', 'script'];
const MAX_VALUE_CHARS = 20_000;

/**
 * The whole payload of a permission request, as labelled fields: what the user
 * is asked to allow must be visible in full, so nothing is summarized and
 * long values are cut only at a hard limit that is announced. Secrets that
 * look like secrets are masked by the shared redaction.
 */
export function permissionFields(toolInputJson: string): DetailField[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(toolInputJson);
  } catch {
    return [{ key: '', value: clip(redactSensitiveText(toolInputJson)) }];
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    return [{ key: '', value: clip(redactSensitiveText(JSON.stringify(parsed, null, 2) ?? String(parsed))) }];
  }
  const entries = Object.entries(parsed as Record<string, unknown>).map(([key, value]): DetailField => ({
    key,
    value: clip(redactSensitiveText(typeof value === 'string' ? value : (JSON.stringify(value, null, 2) ?? String(value)))),
  }));
  const rank = (key: string): number => {
    const index = EXECUTABLE_KEYS.indexOf(key);
    return index < 0 ? EXECUTABLE_KEYS.length : index;
  };
  return entries.map((entry, order) => ({ entry, order })).sort((a, b) => rank(a.entry.key) - rank(b.entry.key) || a.order - b.order).map(({ entry }) => entry);
}

function clip(value: string): string {
  return value.length <= MAX_VALUE_CHARS ? value : `${value.slice(0, MAX_VALUE_CHARS)}\n… (${value.length - MAX_VALUE_CHARS} more characters not shown)`;
}
