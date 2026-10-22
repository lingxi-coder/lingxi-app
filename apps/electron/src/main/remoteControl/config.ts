/**
 * Where Remote Control talks to, read from the environment: the Realtime
 * Database, the push API, the STUN servers and the test switches. Unusable values are
 * ignored (and reported in `warnings`) rather than trusted, because the RTDB
 * URL receives the account's ID token.
 */

import { DEFAULT_RTDB_URL, DEFAULT_STUN_URLS } from '@lingxi/bridge-client/remote';

export interface RemoteControlConfig {
  rtdbUrl: string;
  /** The emulator's `ns` query value; absent for the real instance. */
  rtdbNamespace?: string;
  stunUrls: string[];
  /** The push API that sends "your session needs you" to the account's other devices (no trailing slash). */
  pushApiUrl: string;
  /** `LINGXI_REMOTE_FORCE_TUNNEL=1`: answer every offer with the RTDB tunnel. */
  forceTunnel: boolean;
  /** `LINGXI_REMOTE_CONTROL=1` on an unpackaged build: run without the stored setting. */
  forceEnabled: boolean;
  /** Environment values that were ignored, for the diagnostics. */
  warnings: string[];
}

type Env = Readonly<Record<string, string | undefined>>;

/**
 * The deployed `pushApi` function. The mobile apps register their push
 * installations against the same address (`PushRegistration.kt`,
 * `PushRegistration.swift`), which is why it is repeated here.
 */
export const DEFAULT_PUSH_API_URL = 'https://us-central1-lingxi-5e4b2.cloudfunctions.net/pushApi';

const STUN_URL = /^stuns?:[A-Za-z0-9._:%[\]-]{1,250}$/;
const NAMESPACE = /^[A-Za-z0-9._-]{1,128}$/;
const MAX_STUN_URLS = 8;

function databaseUrl(raw: string): string | undefined {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return undefined;
  }
  const loopback = url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '[::1]';
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) return undefined;
  if (url.username || url.password || url.search || url.hash || url.pathname !== '/') return undefined;
  return url.origin;
}

function pushApiUrl(raw: string): string | undefined {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    return undefined;
  }
  const loopback = url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '[::1]';
  if (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback)) return undefined;
  if (url.username || url.password || url.search || url.hash) return undefined;
  return `${url.origin}${url.pathname.replace(/\/+$/, '')}`;
}

export function remoteControlConfig(env: Env, packaged: boolean): RemoteControlConfig {
  const warnings: string[] = [];
  const config: RemoteControlConfig = {
    rtdbUrl: DEFAULT_RTDB_URL,
    stunUrls: [...DEFAULT_STUN_URLS],
    pushApiUrl: DEFAULT_PUSH_API_URL,
    forceTunnel: env['LINGXI_REMOTE_FORCE_TUNNEL'] === '1',
    forceEnabled: !packaged && env['LINGXI_REMOTE_CONTROL'] === '1',
    warnings,
  };

  const rawUrl = env['LINGXI_REMOTE_RTDB_URL']?.trim();
  if (rawUrl) {
    const url = databaseUrl(rawUrl);
    if (url) config.rtdbUrl = url;
    else warnings.push('LINGXI_REMOTE_RTDB_URL ignored: it must be an https URL (http only for localhost) without credentials, path or query');
  }

  const namespace = env['LINGXI_REMOTE_RTDB_NAMESPACE']?.trim();
  if (namespace) {
    if (NAMESPACE.test(namespace)) config.rtdbNamespace = namespace;
    else warnings.push('LINGXI_REMOTE_RTDB_NAMESPACE ignored: it must be letters, digits, dots, dashes or underscores');
  }

  const rawPush = env['LINGXI_REMOTE_PUSH_URL']?.trim();
  if (rawPush) {
    const url = pushApiUrl(rawPush);
    if (url) config.pushApiUrl = url;
    else warnings.push('LINGXI_REMOTE_PUSH_URL ignored: it must be an https URL (http only for localhost) without credentials, query or fragment');
  }

  const rawStun = env['LINGXI_REMOTE_STUN']?.trim();
  if (rawStun) {
    const urls = rawStun.split(',').map((url) => url.trim()).filter(Boolean);
    const valid = urls.filter((url) => STUN_URL.test(url)).slice(0, MAX_STUN_URLS);
    if (valid.length < urls.length) warnings.push('LINGXI_REMOTE_STUN: entries that are not stun:/stuns: URLs were ignored');
    if (valid.length > 0) config.stunUrls = valid;
  }

  return config;
}

const NAME_MAX_CHARS = 128;
const FALLBACK_NAME = 'LingXi Desktop';
// The characters a host record's name may not contain (see `parseHostRecord`).
const DISPLAY_UNSAFE = /[\u0000-\u001f\u007f-\u009f\u2028\u2029]/g;

/** The OS host name as shown to viewers: no `.local` suffix, no control characters, at most 128 characters. */
export function hostDisplayName(hostname: string): string {
  const name = hostname.replace(DISPLAY_UNSAFE, ' ').trim().replace(/\.local$/i, '').trim();
  if (!name) return FALLBACK_NAME;
  // The cap counts UTF-16 units, like the host record's validation; never end on half a surrogate pair.
  const capped = name.slice(0, NAME_MAX_CHARS);
  return /[\ud800-\udbff]$/.test(capped) ? capped.slice(0, -1) : capped;
}
