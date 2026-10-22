/**
 * Errors shared by the remote-control codec layers.
 */

/**
 * The peer (or the data store it writes to) broke the `lingxi-rc/1` wire rules:
 * a malformed fragment, frame or encoding. Thrown by the pure codecs; the
 * transport layer turns it into a `protocol_error` close.
 */
export class RemoteProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'RemoteProtocolError';
  }
}
