export function shouldAutoplayTrackedReply(
  activeSessionId: string | null,
  trackedSessionId: string,
  documentHidden: boolean,
): boolean {
  return !documentHidden && activeSessionId === trackedSessionId;
}
