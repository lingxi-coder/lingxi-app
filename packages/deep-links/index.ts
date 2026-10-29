/** Product navigation only: parsing a link never performs an action. */
export const MAX_LINK_LENGTH = 32768;
export type DeepLinkTarget = {
  route: string;
  params: Record<string, string>;
  payload?: Record<string, unknown>;
};
export class DeepLinkError extends Error {
  constructor(public readonly code: 'invalid' | 'unsupported', message: string) { super(message); }
}
const routes = /^(session\/[^/]+|project\/[^/]+|new|settings\/[^/]+|provider\/[^/]+|extensions\/(mcp|skill|plugin)\/[^/]+|cron\/(job|run)\/[^/]+|terminal\/[^/]+|apps\/[^/]+|market\/[^/]+|runtime\/(agent|task|plan|review|activity)\/[^/]+|import\/(mcp|plugin|skill|provider)|remote\/[^/]+\/[^/]+|team\/(join|inbox))$/u;
const parameters = new Set(['workspace', 'message', 'mode', 'scope', 'code']);
function validatePayload(value: unknown, depth = 0): void {
  if (depth > 16) throw new DeepLinkError('invalid', 'Payload is too deeply nested');
  if (Array.isArray(value)) { value.forEach(v => validatePayload(v, depth + 1)); return; }
  if (value && typeof value === 'object') {
    for (const [key, child] of Object.entries(value)) {
      if (/^(api[-_]?key|token|access[-_]?token|refresh[-_]?token|authorization|password|secret|headers|env)$/iu.test(key)) throw new DeepLinkError('invalid', 'Credentials must be entered in the application');
      validatePayload(child, depth + 1);
    }
  }
  if (typeof value === 'string' && /^https?:\/\//iu.test(value)) {
    const url = new URL(value);
    if (url.username || url.password || url.search) throw new DeepLinkError('invalid', 'Import URLs must not contain credentials or query parameters');
  }
}
export function parseDeepLink(raw: string): DeepLinkTarget {
  if (!raw || raw.length > MAX_LINK_LENGTH || /[\u0000-\u0020\u007f]/u.test(raw) || /%(?![\da-f]{2})/iu.test(raw)) throw new DeepLinkError('invalid', 'Invalid link');
  // Reject traversal before URL normalizes dot segments into another route.
  if (raw.split(/[?#]/u)[0]!.split('/').some(segment => /^(\.|%2e){1,2}$/iu.test(segment))) throw new DeepLinkError('invalid', 'Invalid route');
  let url: URL;
  try { url = new URL(raw); } catch { throw new DeepLinkError('invalid', 'Invalid link'); }
  if (url.username || url.password || url.port) throw new DeepLinkError('invalid', 'Invalid link authority');
  let encoded: string;
  if (url.protocol === 'lingxi:') encoded = url.hostname + url.pathname;
  else if (url.protocol === 'https:' && url.hostname === 'lingxios.net' && url.pathname.startsWith('/open/')) encoded = url.pathname.slice(6);
  else throw new DeepLinkError('unsupported', 'Unsupported link origin');
  let route: string;
  try {
    route = encoded.split('/').map(segment => {
      const decoded = decodeURIComponent(segment);
      if (!decoded || decoded === '.' || decoded === '..' || /[\/\\\u0000-\u001f\u007f]/u.test(decoded)) throw new Error();
      return decoded;
    }).join('/');
  } catch { throw new DeepLinkError('invalid', 'Invalid route'); }
  if (!routes.test(route)) throw new DeepLinkError('unsupported', 'Unsupported destination');
  const params: Record<string, string> = {};
  for (const [key, value] of url.searchParams) {
    if (!parameters.has(key) || Object.prototype.hasOwnProperty.call(params, key) || !value || value.length > 512 || /[\u0000-\u001f\u007f]/u.test(value)) throw new DeepLinkError('invalid', 'Invalid link parameter');
    params[key] = value;
  }
  if (params.mode && !['chat', 'code'].includes(params.mode)) throw new DeepLinkError('invalid', 'Invalid session mode');
  if (params.message && (!route.startsWith('session/') || !/^[\da-f]{8}(-[\da-f]{4}){3}-[\da-f]{12}$/iu.test(params.message))) throw new DeepLinkError('invalid', 'Invalid message identity');
  if (params.message) params.message = params.message.toLowerCase();
  if (route === 'team/inbox' && Object.keys(params).length) throw new DeepLinkError('invalid', 'Unexpected team inbox parameter');
  if (route === 'team/join' ? Object.keys(params).length !== 1 || !/^[A-Za-z0-9_-]{32}$/u.test(params.code ?? '') : params.code !== undefined) throw new DeepLinkError('invalid', 'Invalid team invitation');
  const result: DeepLinkTarget = { route, params };
  if (url.hash) {
    if (!(route === 'new' || route.startsWith('import/') || route.startsWith('terminal/'))) throw new DeepLinkError('invalid', 'Unexpected payload');
    try {
      const fragment = new URLSearchParams(url.hash.slice(1));
      if ([...fragment].length !== 1 || !fragment.has('payload')) throw new Error();
      const payload: unknown = JSON.parse(fragment.get('payload')!);
      if (!payload || typeof payload !== 'object' || Array.isArray(payload)) throw new Error();
      if (route.startsWith('import/')) validatePayload(payload);
      else if (Object.keys(payload).some(key => key !== (route === 'new' ? 'prompt' : 'command')) || Object.values(payload).some(value => typeof value !== 'string')) throw new Error();
      result.payload = payload as Record<string, unknown>;
    } catch { throw new DeepLinkError('invalid', 'Invalid or credential-bearing payload'); }
  }
  return result;
}
export function buildDeepLink(target: DeepLinkTarget, format: 'https' | 'scheme' = 'https'): string {
  const path = target.route.split('/').map(encodeURIComponent).join('/');
  const params = { ...target.params };
  if (params.message) params.message = params.message.toLowerCase();
  const query = new URLSearchParams(Object.entries(params).sort(([a], [b]) => a.localeCompare(b))).toString();
  const fragment = target.payload ? `#${new URLSearchParams({ payload: JSON.stringify(target.payload) })}` : '';
  const raw = `${format === 'https' ? 'https://lingxios.net/open/' : 'lingxi://'}${path}${query ? `?${query}` : ''}${fragment}`;
  parseDeepLink(raw);
  return raw;
}
