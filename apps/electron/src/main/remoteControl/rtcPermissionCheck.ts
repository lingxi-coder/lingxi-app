/** Chromium uses media permission status to decide whether ICE may enumerate
 * non-default interfaces. This status check does not grant a capture request. */
export function canEnumerateRtcInterfaces(check: {
  permission: string;
  sameContents: boolean;
  pageUrl: string | undefined;
  requestingUrl: string | undefined;
  isMainFrame: boolean | undefined;
}): boolean {
  return check.permission === 'media'
    && check.sameContents
    && check.isMainFrame === true
    && Boolean(check.pageUrl)
    && check.requestingUrl === check.pageUrl;
}
