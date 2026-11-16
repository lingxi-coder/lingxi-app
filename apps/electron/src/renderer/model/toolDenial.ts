/** Recognize approval refusals in current and historical display text, never shell EACCES output. */
export function toolDenial(failed: boolean, headline?: string, body?: string): { reason: string; autoReview: boolean } | undefined {
  if (!failed) return undefined;
  const reason = body?.trim() || headline?.trim() || '';
  const text = reason.toLowerCase();
  if (!['permission to use ', 'permission denied:', 'permission denied by ', 'tool use denied', 'user denied ', 'automatic approval review failed', 'command declined by auto-review'].some(prefix => text.startsWith(prefix))
    && text !== 'permission denied') return undefined;
  return { reason, autoReview: /auto[- ]review|automatic approval review/.test(text) };
}
