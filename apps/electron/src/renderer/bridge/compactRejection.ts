/** Admission refusals are feedback for a command, not failures of the active turn. */
export const COMPACT_DURING_TURN = 'cannot compact the session while a turn is in flight';

export function isCompactTurnRejection(message: string): boolean {
  const detail = message.trim().replace(/^(?:force_compact failed:\s*)?(?:handle action failed:\s*)?/i, '');
  return detail === COMPACT_DURING_TURN
    || detail === 'Compact is disabled while a chat is in progress';
}
