import { GoogleAuth } from 'google-auth-library';

/** The workload authorizes the hop to Cloud Run; the separate Firebase or Task bearer reaches Core. */
export function createPrivateDatasetFetch(origin, auth = new GoogleAuth(), transport = fetch) {
  const service = new URL(origin);
  if (service.protocol !== 'https:' || service.pathname !== '/' || service.username || service.password ||
      service.search || service.hash || service.port || !service.hostname.endsWith('.run.app')) {
    throw new Error('Expected a private Cloud Run service origin');
  }
  let client;
  return async function fetchDataset(resource, options) {
    const url = new URL(resource);
    if (url.origin !== service.origin || url.username || url.password) throw new Error('Unexpected Dataset destination');
    client ??= await auth.getIdTokenClient(service.origin);
    const bearer = (await client.getRequestHeaders()).get('authorization');
    if (!bearer?.startsWith('Bearer ')) throw new Error('Missing Cloud Run service identity');
    const headers = new Headers(options?.headers);
    headers.set('X-Serverless-Authorization', bearer);
    return transport(url, { ...options, headers, redirect: 'error' });
  };
}
