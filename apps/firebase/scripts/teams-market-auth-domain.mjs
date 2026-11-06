import { readFile, writeFile, mkdir } from 'node:fs/promises';
import { resolve, dirname } from 'node:path';
import { randomBytes, randomUUID } from 'node:crypto';
import { parseArgs } from 'node:util';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';

const { values } = parseArgs({ options: { out: { type: 'string' }, 'create-test-user': { type: 'boolean', default: false } } });
if (!values.out || !values['create-test-user']) throw new Error('Usage: --create-test-user --out RECEIPT.json');
if (process.env.FIREBASE_AUTH_EMULATOR_HOST || process.env.FIRESTORE_EMULATOR_HOST) throw new Error('AUTH-DOMAIN-01 requires actual production token verification');
const workspace = resolve(import.meta.dirname, '../../../..');
const marketConfig = JSON.parse(await readFile(resolve(workspace, 'local-app-market/src/web/firebaseConfig.json'), 'utf8'));
const teamSource = await readFile(resolve(workspace, 'lingxi-app/apps/web/src/app/firebase.ts'), 'utf8');
const projectId = marketConfig.projectId;
if (projectId !== 'lingxi-5e4b2' || !teamSource.includes(`: '${projectId}'`) || !teamSource.includes(marketConfig.apiKey)) throw new Error('PRODUCTION_CLIENT_CONFIG_MISMATCH');
const teamOrigin = 'https://lingxios.net', marketOrigin = 'https://lingxi-5e4b2-market.web.app';
const receipt = { schemaVersion: 1, experiment: 'AUTH-DOMAIN-01', at: new Date().toISOString(),
  productionProject: projectId, productionTeamOrigin: teamOrigin, productionMarketOrigin: marketOrigin,
  status: 'incomplete', steps: [], noTokensOrUidsStored: true };
let idToken, app;
async function postIdentity(action, body) {
  const response = await fetch(`https://identitytoolkit.googleapis.com/v1/accounts:${action}?key=${encodeURIComponent(marketConfig.apiKey)}`,
    { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), signal: AbortSignal.timeout(20000), redirect: 'error' });
  const data = await response.json();
  if (!response.ok) throw new Error(`IDENTITY_${response.status}_${String(data.error?.message ?? 'FAILED').replace(/[^A-Z_0-9]/gu, '').slice(0, 80)}`);
  return data;
}
try {
  // An isolated disposable account is created for this test; no existing user
  // credential, browser session, email, Team data or model key is modified.
  const created = await postIdentity('signUp', { email: `teams-market-qa-${randomUUID()}@users.invalid`,
    password: `Qa!${randomBytes(32).toString('base64url')}`, returnSecureToken: true });
  idToken = created.idToken;
  if (typeof idToken !== 'string' || typeof created.localId !== 'string') throw new Error('INVALID_SIGNUP_RESPONSE');
  receipt.steps.push({ step: 'production-account-created', passed: true });
  app = initializeApp({ projectId }, `auth-domain-${randomUUID()}`);
  const verified = await getAuth(app).verifyIdToken(idToken);
  receipt.productionIssuer = verified.iss; receipt.productionAudience = verified.aud;
  receipt.productionTenantId = verified.firebase?.tenant ?? null;
  if (verified.iss !== `https://securetoken.google.com/${projectId}` || verified.aud !== projectId
    || verified.uid !== created.localId || receipt.productionTenantId !== null) throw new Error('VERIFIED_ACCOUNT_DOMAIN_MISMATCH');
  receipt.steps.push({ step: 'production-signature-issuer-audience-verified', passed: true });
  const teams = await fetch(`${teamOrigin}/v1/teams/session`, { method: 'POST',
    headers: { Authorization: `Bearer ${idToken}`, 'Content-Type': 'application/json' }, body: '{}', signal: AbortSignal.timeout(30000), redirect: 'error' });
  const teamReply = await teams.json();
  if (!teams.ok || teamReply.uid !== verified.uid || typeof teamReply.customToken !== 'string') throw new Error(`TEAMS_IDENTITY_${teams.status}`);
  receipt.steps.push({ step: 'production-teams-verified-uid', passed: true, httpStatus: teams.status });
  const exchanged = await postIdentity('signInWithCustomToken', { token: teamReply.customToken, returnSecureToken: true });
  const teamVerified = await getAuth(app).verifyIdToken(exchanged.idToken);
  if (teamVerified.uid !== verified.uid || teamVerified.iss !== verified.iss || teamVerified.aud !== verified.aud) throw new Error('TEAMS_CUSTOM_TOKEN_UID_MISMATCH');
  receipt.steps.push({ step: 'production-teams-custom-token-roundtrip-same-uid', passed: true });
  const market = await fetch(`${marketOrigin}/api/v2/me`, { headers: { Authorization: `Bearer ${exchanged.idToken}` },
    signal: AbortSignal.timeout(30000), redirect: 'error' });
  const marketReply = await market.json();
  if (!market.ok || marketReply.data?.user?.id !== verified.uid) throw new Error(`MARKET_IDENTITY_${market.status}`);
  receipt.steps.push({ step: 'production-market-verified-same-uid', passed: true, httpStatus: market.status });
  receipt.status = 'passed';
} catch (error) {
  const code = error instanceof Error && /^[A-Z_0-9]+$/u.test(error.message) ? error.message : 'EXPERIMENT_TRANSPORT_OR_VERIFICATION_FAILED';
  receipt.failure = code;
  process.exitCode = 1;
} finally {
  if (idToken) {
    try { await postIdentity('delete', { idToken }); receipt.disposableAuthAccountDeleted = true; }
    catch { receipt.disposableAuthAccountDeleted = false; process.exitCode = 1; }
  }
  if (app) await deleteApp(app);
  await mkdir(dirname(resolve(values.out)), { recursive: true });
  await writeFile(resolve(values.out), JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  console.log(JSON.stringify(receipt));
}
