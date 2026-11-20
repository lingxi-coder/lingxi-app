import { LingxiDataError } from './lingxi-data.mjs';

export function createFirebaseBotTokens({ auth, projectId, apiKey, emulator, fetchRequest = fetch }) {
  if (emulator && (!projectId.startsWith('demo-') || !/^127\.0\.0\.1:[0-9]{1,5}$/u.test(emulator))) {
    throw new Error('Expected a loopback Firebase Auth emulator in a demo project');
  }
  const url = new URL('/v1/accounts:signInWithCustomToken', 'https://identitytoolkit.googleapis.com');
  if (emulator) {
    url.protocol = 'http:';
    url.host = emulator;
    url.pathname = '/identitytoolkit.googleapis.com' + url.pathname;
  }
  url.searchParams.set('key', apiKey);
  const tokens = new Map();

  return async function botToken(uid) {
    if (!/^lx_data_bot_[a-f0-9]{40}$/u.test(uid)) throw new LingxiDataError(403, 'DATA_ACCESS_REQUIRED');
    const existing = tokens.get(uid);
    if (existing && existing.until > Date.now()) return existing.request;
    const value = { until: Date.now() + 900_000, request: Promise.resolve().then(async () => {
      try {
        const token = await auth.createCustomToken(uid, { lingxiDataSourceBot: true });
        const response = await fetchRequest(url, { method: 'POST', redirect: 'error', signal: AbortSignal.timeout(8000),
          headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ token, returnSecureToken: true }) });
        if (!response.ok) throw new Error();
        const issued = await response.json();
        const expires = Number(issued.expiresIn);
        if (typeof issued.idToken !== 'string' || expires < 60 || !Number.isFinite(expires)) throw new Error();
        value.until = Date.now() + Math.min(expires - 30, 900) * 1000;
        return issued.idToken;
      } catch {
        if (tokens.get(uid) === value) tokens.delete(uid);
        throw new LingxiDataError(503, 'DATA_UNAVAILABLE');
      }
    }) };
    tokens.set(uid, value);
    if (tokens.size > 256) {
      for (const [key, entry] of tokens) if (entry.until <= Date.now()) tokens.delete(key);
      if (tokens.size > 256) tokens.delete(tokens.keys().next().value);
    }
    return value.request;
  };
}
