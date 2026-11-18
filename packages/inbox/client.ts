import type { InboxCommand, InboxRequest } from './contracts.js';

/** The token provider must check that the original account is still signed in. */
export function createInboxRequest(
  endpoint: string,
  getToken: () => Promise<string>,
  fetcher = fetch,
): InboxRequest {
  return async <T>(command: InboxCommand): Promise<T> => {
    const response = await fetcher(endpoint, {
      method: 'POST',
      credentials: 'omit',
      cache: 'no-store',
      headers: { 'Content-Type': 'application/json', Authorization: 'Bearer ' + (await getToken()) },
      body: JSON.stringify(command),
    });
    const body = await response.json().catch(() => ({}));
    if (!response.ok)
      throw new Error(
        typeof body.error === 'string'
          ? body.error
          : body.error?.code ||
            (response.status === 401
              ? 'INVALID_SESSION'
              : response.status === 403
                ? 'SOURCE_ACCESS_REQUIRED'
                : 'INBOX_REQUEST_FAILED'),
      );
    return body as T;
  };
}
