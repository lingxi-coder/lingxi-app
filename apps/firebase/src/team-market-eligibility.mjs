import { z } from 'zod';

const principal = z.object({ uid: z.string().min(1).max(128).regex(/^[^/\u0000-\u001f\u007f]+$/u),
  issuer: z.string().url(), audience: z.string().min(1).max(128), tenantId: z.string().min(1).max(128).nullable() }).strict();
const requestSchema = z.object({ schemaVersion: z.literal(1), principal,
  teamId: z.string().regex(/^[A-Za-z0-9_-]{8,64}$/u), operation: z.enum(['read', 'publish']),
  requestDigest: z.string().regex(/^[a-f0-9]{64}$/u) }).strict();
function revision(snapshot) {
  const stamp = snapshot?.updateTime;
  const value = stamp ? stamp.seconds * 1000000 + Math.floor(stamp.nanoseconds / 1000) : 0;
  if (!Number.isSafeInteger(value) || value < 0) throw new Error('INVALID_DATABASE_REVISION');
  return value;
}

/** Uses authoritative existing Team records. No client-provided role,
 * generation, email merge or positive eligibility cache is accepted. */
export class TeamMarketEligibilityStore {
  constructor(db) { this.db = db; }
  eligibility(teamId, uid, operation) {
    return this.db.runTransaction(async tx => {
      const teamRef = this.db.doc(`teams/${teamId}`);
      const [team, member] = await tx.getAll(teamRef, teamRef.collection('members').doc(uid));
      const generation = member.data()?.generation;
      const eligible = team.data()?.state === 'active' && Number.isSafeInteger(generation) && generation > 0
        && (operation !== 'publish' || ['owner', 'admin'].includes(member.data().role));
      return { schemaVersion: 1, eligible, memberGeneration: eligible ? generation : null,
        // Firestore commit timestamps have microsecond precision. This is an
        // observed policy version, not an independently maintained ACL cache.
        policyRevision: Math.max(revision(team), revision(member)) };
    });
  }
}

/** Only the dedicated Market service may vouch for its separately verified
 * Firebase viewer. A browser Firebase token is never service authentication. */
export function createTeamMarketEligibilityHandler({ store, verifyServiceToken, config }) {
  const reply = (body, status = 200) => Response.json(body, { status,
    headers: { 'Cache-Control': 'no-store', 'Content-Type': 'application/json', 'X-Content-Type-Options': 'nosniff' } });
  return async request => {
    if (new URL(request.url).pathname !== '/internal/v1/market/eligibility' || request.method !== 'POST') return reply({ error: { code: 'NOT_FOUND' } }, 404);
    // Do not add a CORS branch. This endpoint is not a browser capability.
    const settings = config();
    if (!settings.serviceEmail || !settings.serviceSubject || !settings.serviceAudience || !settings.projectId
      || !Object.hasOwn(settings, 'tenantId')) return reply({ error: { code: 'ELIGIBILITY_NOT_CONFIGURED' } }, 503);
    const token = /^Bearer (\S+)$/u.exec(request.headers.get('Authorization') ?? '')?.[1];
    if (!token || token.length > 16384) return reply({ error: { code: 'SERVICE_AUTH_REQUIRED' } }, 401);
    let claims;
    try { claims = await verifyServiceToken(token, settings.serviceAudience); } catch { return reply({ error: { code: 'INVALID_SERVICE_IDENTITY' } }, 401); }
    if (!claims || !['accounts.google.com', 'https://accounts.google.com'].includes(claims.iss)
      || claims.aud !== settings.serviceAudience || claims.sub !== settings.serviceSubject
      || claims.email !== settings.serviceEmail || claims.email_verified !== true) return reply({ error: { code: 'SERVICE_IDENTITY_REQUIRED' } }, 403);
    const declaredLength = request.headers.get('Content-Length');
    if (declaredLength && Number(declaredLength) > 16384) return reply({ error: { code: 'REQUEST_TOO_LARGE' } }, 413);
    let input;
    try {
      const raw = await request.text();
      if (Buffer.byteLength(raw, 'utf8') > 16384) return reply({ error: { code: 'REQUEST_TOO_LARGE' } }, 413);
      input = requestSchema.parse(JSON.parse(raw));
    } catch { return reply({ error: { code: 'INVALID_INPUT' } }, 400); }
    if (input.principal.issuer !== `https://securetoken.google.com/${settings.projectId}`
      || input.principal.audience !== settings.projectId || input.principal.tenantId !== settings.tenantId) {
      return reply({ error: { code: 'ACCOUNT_DOMAIN_MISMATCH' } }, 403);
    }
    try { return reply(await store.eligibility(input.teamId, input.principal.uid, input.operation)); }
    catch { return reply({ error: { code: 'SERVICE_UNAVAILABLE' } }, 503); }
  };
}
