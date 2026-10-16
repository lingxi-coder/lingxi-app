import { createHash } from 'node:crypto';
import { z } from 'zod';

export const platforms = ['desktop', 'android', 'ios', 'cli'];
const text = (max) => z.string().trim().min(1).max(max);
const identifier = text(160).regex(/^[a-zA-Z0-9][a-zA-Z0-9._:/@+-]*$/);
const version = z.string().max(80).regex(/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/);
const number = z.number().finite().nonnegative();
const price = z.object({
  currency: z.enum(['USD', 'CNY']), inputPerMillion: number, outputPerMillion: number,
  cacheReadPerMillion: number.optional(), cacheWritePerMillion: number.optional(),
}).strict();
const provider = z.object({
  id: identifier, name: text(160), description: z.string().max(4000),
  baseUrl: z.url().refine((s) => s.startsWith('https://'), 'HTTPS required'),
  protocol: text(100), enabled: z.boolean(), order: z.number().int().min(0).max(100000),
  regions: z.array(text(80)).max(20),
}).strict();
const model = z.object({
  providerId: identifier, id: identifier, name: text(240), description: z.string().max(8000),
  enabled: z.boolean(), order: z.number().int().min(0).max(100000),
  platforms: z.array(z.enum(platforms)).min(1).max(4),
  minAppVersion: version, maxAppVersion: version.nullable(),
  contextWindow: z.number().int().positive().max(100000000),
  maxOutputTokens: z.number().int().positive().max(100000000),
  capabilities: z.object({ tools: z.boolean(), vision: z.boolean(), reasoning: z.boolean(), audio: z.boolean() }).strict(),
  inputModalities: z.array(text(40)).max(20), outputModalities: z.array(text(40)).max(20),
  pricing: price.nullable(),
}).strict();
export const catalogSchema = z.object({ schemaVersion: z.literal(1), providers: z.array(provider).max(100), models: z.array(model).max(5000) }).strict();
export class ApiError extends Error {
  constructor(status, code, message = code) { super(message); this.status = status; this.code = code; }
}

export function compareVersions(a, b) {
  function split(s) {
    version.parse(s);
    const [core, ...prerelease] = s.split('+')[0].split('-');
    const pre = prerelease.join('-') || undefined;
    if (pre?.split('.').some((p) => /^0\d+$/.test(p))) throw new ApiError(400, 'INVALID_VERSION');
    const nums = core.split('.').map(Number);
    if (nums.some((v) => !Number.isSafeInteger(v))) throw new ApiError(400, 'INVALID_VERSION');
    return [nums, pre?.split('.')];
  }
  const [av, ap] = split(a), [bv, bp] = split(b);
  for (let i = 0; i < 3; i++) if (av[i] !== bv[i]) return av[i] < bv[i] ? -1 : 1;
  if (!ap || !bp) return ap ? -1 : bp ? 1 : 0;
  for (let i = 0; i < Math.max(ap.length, bp.length); i++) {
    if (ap[i] === undefined) return -1;
    if (bp[i] === undefined) return 1;
    if (ap[i] === bp[i]) continue;
    const an = /^\d+$/.test(ap[i]), bn = /^\d+$/.test(bp[i]);
    if (an && bn) return BigInt(ap[i]) < BigInt(bp[i]) ? -1 : 1;
    if (an !== bn) return an ? -1 : 1;
    return ap[i] < bp[i] ? -1 : 1;
  }
  return 0;
}

export function validateCatalog(value) {
  const catalog = catalogSchema.parse(value);
  if (Buffer.byteLength(JSON.stringify(catalog)) > 8 * 1024 * 1024) throw new ApiError(413, 'CATALOG_TOO_LARGE');
  const providers = new Set(), models = new Set();
  for (const p of catalog.providers) {
    if (providers.has(p.id)) throw new ApiError(400, 'DUPLICATE_PROVIDER');
    providers.add(p.id);
  }
  for (const m of catalog.models) {
    if (!providers.has(m.providerId)) throw new ApiError(400, 'UNKNOWN_PROVIDER');
    const key = `${m.providerId}\0${m.id}`;
    if (models.has(key)) throw new ApiError(400, 'DUPLICATE_MODEL');
    models.add(key);
    if (new Set(m.platforms).size !== m.platforms.length) throw new ApiError(400, 'DUPLICATE_PLATFORM');
    compareVersions(m.minAppVersion, m.minAppVersion);
    if (m.maxAppVersion && compareVersions(m.minAppVersion, m.maxAppVersion) > 0) throw new ApiError(400, 'INVALID_VERSION_RANGE');
    if (m.maxOutputTokens > m.contextWindow) throw new ApiError(400, 'OUTPUT_EXCEEDS_CONTEXT');
  }
  return catalog;
}

export function selectCatalog(catalog, platform, appVersion) {
  z.enum(platforms).parse(platform); version.parse(appVersion);
  compareVersions(appVersion, appVersion);
  const providers = catalog.providers.filter((p) => p.enabled).sort((a, b) => a.order - b.order || a.id.localeCompare(b.id));
  const ids = new Set(providers.map((p) => p.id));
  const models = catalog.models.filter((m) => m.enabled && ids.has(m.providerId) && m.platforms.includes(platform)
    && compareVersions(appVersion, m.minAppVersion) >= 0 && (!m.maxAppVersion || compareVersions(appVersion, m.maxAppVersion) <= 0))
    .sort((a, b) => a.order - b.order || `${a.providerId}/${a.id}`.localeCompare(`${b.providerId}/${b.id}`));
  return { schemaVersion: 1, providers, models };
}

export function createHandler(service, verifyToken, origins) {
  return async function handle(request) {
    const url = new URL(request.url), origin = request.headers.get('Origin');
    const headers = new Headers({ 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', Vary: 'Origin' });
    const reply = (body, status = 200) => new Response(status === 304 ? null : JSON.stringify(body), { status, headers });
    try {
      if (origin && !origins.includes(origin)) throw new ApiError(403, 'ORIGIN_NOT_ALLOWED');
      if (origin) headers.set('Access-Control-Allow-Origin', origin);
      if (request.method === 'OPTIONS') {
        headers.set('Access-Control-Allow-Methods', 'GET, PUT, POST, OPTIONS');
        headers.set('Access-Control-Allow-Headers', 'Authorization, Content-Type, If-None-Match');
        return new Response(null, { status: 204, headers });
      }
      if (request.method === 'GET' && url.pathname === '/health') return reply({ status: 'ok', service: 'model-catalog', schemaVersion: 1 });
      if (request.method === 'GET' && url.pathname === '/v1/model-catalog') {
        if (url.searchParams.get('schemaVersion') !== '1') throw new ApiError(400, 'UNSUPPORTED_SCHEMA');
        const platform = url.searchParams.get('platform'), appVersion = url.searchParams.get('appVersion');
        z.enum(platforms).parse(platform); version.parse(appVersion);
        const current = await service.current();
        if (!current) throw new ApiError(404, 'CATALOG_NOT_PUBLISHED');
        const data = { releaseId: current.id, publishedAt: current.createdAt, ...selectCatalog(current.catalog, platform, appVersion) };
        const etag = `"${createHash('sha256').update(JSON.stringify(data)).digest('hex')}"`;
        headers.set('ETag', etag); headers.set('Access-Control-Expose-Headers', 'ETag'); headers.set('Cache-Control', 'public, max-age=60, stale-while-revalidate=60');
        if (request.headers.get('If-None-Match') === etag) return reply(null, 304);
        return reply(data);
      }
      if (!url.pathname.startsWith('/v1/admin/')) throw new ApiError(404, 'NOT_FOUND');
      const bearer = /^Bearer (\S+)$/.exec(request.headers.get('Authorization') || '');
      if (!bearer) throw new ApiError(401, 'SIGN_IN_REQUIRED');
      let user;
      try { user = await verifyToken(bearer[1]); } catch { throw new ApiError(401, 'INVALID_SESSION'); }
      if (user.model_catalog_admin !== true || user.email_verified !== true) throw new ApiError(403, 'ADMIN_REQUIRED');
      if (request.method === 'GET' && url.pathname === '/v1/admin/models') return reply(await service.draft());
      if (request.method === 'GET' && url.pathname === '/v1/admin/releases') return reply({ releases: await service.history() });
      if (request.method !== 'PUT' && request.method !== 'POST') throw new ApiError(405, 'METHOD_NOT_ALLOWED');
      const raw = await request.text();
      if (Buffer.byteLength(raw) > 8 * 1024 * 1024) throw new ApiError(413, 'CATALOG_TOO_LARGE');
      let body;
      try { body = JSON.parse(raw); } catch { throw new ApiError(400, 'INVALID_JSON'); }
      const expected = z.number().int().nonnegative().parse(body.expectedRevision);
      if (request.method === 'POST' && url.pathname === '/v1/admin/preview') return reply(selectCatalog(validateCatalog(body.catalog), body.platform, body.appVersion));
      if (request.method === 'PUT' && url.pathname === '/v1/admin/models') return reply(await service.save(validateCatalog(body.catalog), expected, user.uid));
      if (request.method === 'POST' && url.pathname === '/v1/admin/releases') return reply(await service.publish(expected, text(160).parse(body.label), user.uid), 201);
      const restore = /^\/v1\/admin\/releases\/([a-f0-9-]{36})\/restore$/.exec(url.pathname);
      if (request.method === 'POST' && restore) return reply(await service.restore(restore[1], expected, user.uid));
      throw new ApiError(404, 'NOT_FOUND');
    } catch (error) {
      if (error instanceof z.ZodError) return reply({ error: { code: 'INVALID_INPUT', message: error.issues.slice(0, 8).map((x) => `${x.path.join('.')}: ${x.message}`).join('; ') } }, 400);
      if (error instanceof ApiError) return reply({ error: { code: error.code, message: error.message } }, error.status);
      console.error('catalog request failed', { path: url.pathname, code: error.code || 'INTERNAL' });
      return reply({ error: { code: 'SERVICE_UNAVAILABLE', message: 'Please retry later.' } }, 503);
    }
  };
}
