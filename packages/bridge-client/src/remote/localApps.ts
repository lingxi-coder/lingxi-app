/** Read-only catalog and browser bootstrap for Desktop-hosted Local Apps. */
export interface RemoteLocalApp {
  app_id: string;
  name: string;
  summary: string;
  revision: number;
  phase: 'stopped' | 'installing' | 'building' | 'starting' | 'running' | 'stopping' | 'failed';
  background: boolean;
  execution_host: 'desktop';
  remote_enabled: true;
}
export type AppsListParams = Record<string, never>;
export interface AppsListResult { apps: RemoteLocalApp[] }
export interface AppsOpenParams { app_id: string }
export interface AppsOpenResult { app_id: string; revision: number; url: string; expires_at: number }

const phases = new Set(['stopped', 'installing', 'building', 'starting', 'running', 'stopping', 'failed']);
const object = (v: unknown): v is Record<string, unknown> => v !== null && typeof v === 'object' && !Array.isArray(v);
const keys = (v: Record<string, unknown>, expected: string[]): boolean => Object.keys(v).length === expected.length && expected.every(k => Object.prototype.hasOwnProperty.call(v, k));
const str = (v: unknown, max: number, empty = false): v is string => typeof v === 'string' && (empty || v.length > 0) && v.length <= max && !/[\u0000-\u001f\u007f]/u.test(v);
const uint = (v: unknown): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v >= 0;
export const isLocalAppId = (v: unknown): v is string => typeof v === 'string' && /^a[a-f0-9]{32}$/u.test(v);
export function isAppsListParams(v: unknown): v is AppsListParams { return object(v) && keys(v, []); }
export function isAppsOpenParams(v: unknown): v is AppsOpenParams { return object(v) && keys(v, ['app_id']) && isLocalAppId(v.app_id); }
export function isAppsListResult(v: unknown): v is AppsListResult {
  if (!object(v) || !keys(v, ['apps']) || !Array.isArray(v.apps) || v.apps.length > 256) return false;
  const ids = new Set<string>();
  return v.apps.every((a: unknown) => {
    if (!object(a) || !keys(a, ['app_id', 'name', 'summary', 'revision', 'phase', 'background', 'execution_host', 'remote_enabled']) ||
        !isLocalAppId(a.app_id) || ids.has(a.app_id) || !str(a.name, 256) || !str(a.summary, 4096, true) ||
        !uint(a.revision) || a.revision === 0 || typeof a.phase !== 'string' || !phases.has(a.phase) || typeof a.background !== 'boolean' ||
        a.execution_host !== 'desktop' || a.remote_enabled !== true) return false;
    ids.add(a.app_id); return true;
  });
}
export function isAppsOpenResult(v: unknown, allowLoopbackHttp = false): v is AppsOpenResult {
  if (!object(v) || !keys(v, ['app_id', 'revision', 'url', 'expires_at']) || !isLocalAppId(v.app_id) ||
      !uint(v.revision) || v.revision === 0 || !uint(v.expires_at) || v.expires_at === 0 || !str(v.url, 8192)) return false;
  try {
    const url = new URL(v.url);
    const loopback = url.hostname.endsWith('.localhost');
    const [label, ...domain] = url.hostname.split('.');
    const session = label?.slice(v.app_id.length + 1);
    return (url.protocol === 'https:' || (allowLoopbackHttp && url.protocol === 'http:' && loopback)) &&
      !url.username && !url.password && label?.startsWith(v.app_id + '-') && domain.length > 0 && /^[a-f0-9]{20}$/u.test(session ?? '') &&
      url.pathname === '/__lingxi_remote_bootstrap' && !url.search && /^#[A-Za-z0-9_-]{43}$/u.test(url.hash);
  } catch { return false; }
}
