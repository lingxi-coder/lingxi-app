/** Where the Remote Control modules report what they would otherwise swallow (`DiagnosticBuffer` in production). */
export type RemoteDiagnostics = (level: 'info' | 'warn' | 'error', message: string) => void;

export const noDiagnostics: RemoteDiagnostics = () => undefined;

/** A short, single-line description of a failure, for diagnostics and the status `error`. */
export function describeError(error: unknown): string {
  const message = error instanceof Error ? error.message : String(error);
  return message.replace(/\s+/g, ' ').trim().slice(0, 200) || 'unknown error';
}
