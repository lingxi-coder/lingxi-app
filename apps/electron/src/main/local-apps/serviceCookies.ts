import type { Session } from 'electron';

// `.invalid` is reserved and cannot resolve. The per-App Electron partition
// keeps the jar private; this fixed origin is only a key for Chromium's cookie
// implementation and is never used for a network request.
export const LOCAL_APP_SERVICE_COOKIE_ORIGIN = 'https://local-app.invalid';
export const LOCAL_APP_CREDENTIALS_HEADER = 'x-lingxi-local-app-credentials';
const MAX_SET_COOKIE_HEADERS = 64;
const MAX_SET_COOKIE_BYTES = 8192;
const MAX_COOKIE_HEADER_BYTES = 16 * 1024;

type CookieStore = Session['cookies'];

function cookieUrl(path: string): string {
  if (!path.startsWith('/') || path.startsWith('//') || /[\\\r\n\0]/.test(path)) throw new Error('Invalid application service cookie path');
  return `${LOCAL_APP_SERVICE_COOKIE_ORIGIN}${path}`;
}

function defaultCookiePath(requestPath: string): string {
  if (!requestPath.startsWith('/') || requestPath === '/') return '/';
  const lastSlash = requestPath.lastIndexOf('/');
  return lastSlash <= 0 ? '/' : requestPath.slice(0, lastSlash);
}

function publicCookiePath(backendCookiePath: string, publicPrefix: string): string {
  if (!publicPrefix) return backendCookiePath;
  return `${publicPrefix}${backendCookiePath === '/' ? '' : backendCookiePath}`;
}

function splitSetCookieHeaders(headers: Headers): string[] {
  const source = headers as Headers & { getSetCookie?: () => string[] };
  const exact = source.getSetCookie?.();
  if (exact) return exact.slice(0, MAX_SET_COOKIE_HEADERS);
  const combined = headers.get('set-cookie');
  if (!combined) return [];
  // Expires contains a comma followed by a date, not a cookie-pair. Split only
  // at commas followed by a new name=value pair.
  return combined.split(/,(?=\s*[^\s;,=]+=)/).slice(0, MAX_SET_COOKIE_HEADERS);
}

interface ParsedCookie {
  name: string;
  value: string;
  path: string;
  secure: boolean;
  httpOnly: boolean;
  sameSite?: 'unspecified' | 'no_restriction' | 'lax' | 'strict';
  expirationDate?: number;
}

function parseSetCookie(header: string, requestPath: string): ParsedCookie | undefined {
  if (Buffer.byteLength(header, 'utf8') > MAX_SET_COOKIE_BYTES) return undefined;
  const [pair, ...attributes] = header.split(';');
  if (!pair) return undefined;
  const separator = pair.indexOf('=');
  if (separator <= 0) return undefined;
  const name = pair.slice(0, separator).trim();
  let value = pair.slice(separator + 1).trim();
  if (!/^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/.test(name)) return undefined;
  if (value.startsWith('"') && value.endsWith('"') && value.length >= 2) value = value.slice(1, -1);
  if (!/^[\x21\x23-\x2B\x2D-\x3A\x3C-\x5B\x5D-\x7E]*$/.test(value)) return undefined;

  let path = defaultCookiePath(requestPath);
  let secure = false;
  let httpOnly = false;
  let sameSite: ParsedCookie['sameSite'];
  let expirationDate: number | undefined;
  let maxAge: number | undefined;
  for (const rawAttribute of attributes) {
    const attribute = rawAttribute.trim();
    if (!attribute) continue;
    const index = attribute.indexOf('=');
    const key = (index < 0 ? attribute : attribute.slice(0, index)).trim().toLowerCase();
    const attributeValue = index < 0 ? '' : attribute.slice(index + 1).trim();
    // Upstream service domains do not map to the app's custom origin. Refuse
    // them instead of broadening a cookie to the shared adapter origin.
    if (key === 'domain') return undefined;
    if (key === 'partitioned') return undefined;
    if (key === 'path' && attributeValue.startsWith('/') && attributeValue.length <= 2048 && !/[\\\x00-\x1f\x7f]/.test(attributeValue)) path = attributeValue;
    else if (key === 'secure') secure = true;
    else if (key === 'httponly') httpOnly = true;
    else if (key === 'samesite') {
      const value = attributeValue.toLowerCase();
      if (value === 'strict') sameSite = 'strict';
      else if (value === 'lax') sameSite = 'lax';
      else if (value === 'none') sameSite = 'no_restriction';
    } else if (key === 'max-age' && /^-?\d+$/.test(attributeValue)) {
      const parsed = Number(attributeValue);
      if (Number.isSafeInteger(parsed)) maxAge = parsed;
    } else if (key === 'expires') {
      const parsed = Date.parse(attributeValue);
      if (Number.isFinite(parsed)) expirationDate = Math.floor(parsed / 1000);
    }
  }
  if (sameSite === 'no_restriction' && !secure) return undefined;
  if (name.startsWith('__Secure-') && !secure) return undefined;
  if (name.startsWith('__Host-') && (!secure || path !== '/')) return undefined;
  if (maxAge !== undefined) expirationDate = Math.floor(Date.now() / 1000) + Math.max(-1, Math.min(maxAge, 400 * 24 * 60 * 60));
  return { name, value, path, secure, httpOnly, ...(sameSite ? { sameSite } : {}), ...(expirationDate === undefined ? {} : { expirationDate }) };
}

/** Returns only cookies from this App's persistent partition matching its app-origin path. */
export async function serviceCookieHeader(store: CookieStore, appPath: string): Promise<string | undefined> {
  const cookies = await store.get({ url: cookieUrl(appPath) });
  cookies.sort((left, right) => (right.path?.length ?? 0) - (left.path?.length ?? 0));
  let header = '';
  for (const cookie of cookies) {
    const pair = `${cookie.name}=${cookie.value}`;
    const next = header ? `${header}; ${pair}` : pair;
    if (Buffer.byteLength(next, 'utf8') > MAX_COOKIE_HEADER_BYTES) break;
    header = next;
  }
  return header || undefined;
}

/** Store service cookies only in the owning App partition; custom app schemes cannot own Chromium cookies. */
export async function storeServiceCookies(store: CookieStore, backendPath: string, publicPrefix: string, headers: Headers): Promise<void> {
  for (const value of splitSetCookieHeaders(headers)) {
    const parsed = parseSetCookie(value, backendPath);
    if (!parsed) continue;
    // A __Host- cookie requires Path=/, which cannot be preserved as a
    // service-prefixed Path. Do not silently weaken that prefix invariant.
    if (publicPrefix && parsed.name.startsWith('__Host-')) continue;
    const cookie = { ...parsed, path: publicCookiePath(parsed.path, publicPrefix) };
    if (cookie.path.length > 2048) continue;
    if (cookie.expirationDate !== undefined && cookie.expirationDate <= Date.now() / 1000) {
      // An expired set using the exact path avoids CookieStore.remove's URL
      // path matching ambiguously selecting a same-name prefix cookie.
      await store.set({ url: cookieUrl(cookie.path), name: cookie.name, value: '', path: cookie.path,
        secure: cookie.secure, httpOnly: cookie.httpOnly, sameSite: cookie.sameSite, expirationDate: cookie.expirationDate }).catch(() => undefined);
      continue;
    }
    try {
      await store.set({
        url: cookieUrl(cookie.path),
        name: cookie.name,
        value: cookie.value,
        path: cookie.path,
        httpOnly: cookie.httpOnly,
        sameSite: cookie.sameSite,
        expirationDate: cookie.expirationDate,
        secure: cookie.secure,
      });
    } catch { /* Invalid service cookies are ignored; they cannot widen scope. */ }
  }
}
