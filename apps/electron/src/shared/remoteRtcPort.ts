/**
 * The two names the hidden WebRTC window's port hand-off uses. Kept apart from
 * `remoteRtc.ts` because the sandboxed preload bundles this file and must not
 * drag the protocol library in with it.
 */

/** IPC channel on which main delivers the MessagePort to the preload. */
export const CH_REMOTE_RTC_PORT = 'lingxi:remote-rtc:port';

/** What the preload posts into the page's main world, together with that port. */
export const REMOTE_RTC_PORT_MESSAGE = 'lingxi-remote-rtc-port';
