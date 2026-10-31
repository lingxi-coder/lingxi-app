import { randomBytes } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// Real test users, authorized by the user on 2026-10-29. No admin claims, email
// delivery, rules deployment or existing-account password changes are involved.
const root = resolve(fileURLToPath(new URL('../../..', import.meta.url)));
const config = JSON.parse(readFileSync(resolve(root, 'apps/electron/src/shared/firebaseConfig.json'), 'utf8'));
if (config.projectId !== 'lingxi-5e4b2') throw new Error('Unexpected Firebase project');
const file = resolve(root, 'output/teams-t1-private/firebase-auth.json');
const create = process.argv.includes('--create');
if (!existsSync(file) && !create) throw new Error('Pass --create only when creation of dedicated Firebase test users is authorized');
mkdirSync(dirname(file), { recursive: true, mode: 0o700 }); chmodSync(dirname(file), 0o700);
const state = existsSync(file) ? JSON.parse(readFileSync(file, 'utf8')) : {
  projectId: config.projectId, createdFor: 'LingXi Teams acceptance', accounts: ['owner', 'member'].map(role => ({
    role, email: `lingxi-teams-test-${role}-${randomBytes(5).toString('hex')}@example.test`,
    password: randomBytes(32).toString('base64url'), displayName: `LingXi Teams Test ${role}`, created: false,
  })),
};
if (state.projectId !== config.projectId || state.createdFor !== 'LingXi Teams acceptance' || state.accounts.length !== 2) throw new Error('Unrecognized test-account state');
const save = () => {
  const pending = `${file}.tmp`;
  writeFileSync(pending, JSON.stringify(state, null, 2) + '\n', { mode: 0o600 });
  chmodSync(pending, 0o600); renameSync(pending, file);
};
save(); // Preserve identity before a network call, including an ambiguous create.
async function auth(method, body) {
  const response = await fetch(`https://identitytoolkit.googleapis.com/v1/accounts:${method}?key=${config.apiKey}`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body), redirect: 'error', signal: AbortSignal.timeout(20000),
  });
  const value = await response.json();
  if (!response.ok) throw new Error(value.error?.message ?? 'AUTH_FAILED');
  return value;
}
for (const account of state.accounts) {
  if (!/^lingxi-teams-test-(owner|member)-[a-f0-9]{10}@example\.test$/u.test(account.email)) throw new Error('Refusing a non-test account');
  let session;
  if (account.refreshToken) {
    const response = await fetch(`https://securetoken.googleapis.com/v1/token?key=${config.apiKey}`, { method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ grant_type: 'refresh_token', refresh_token: account.refreshToken }), redirect: 'error', signal: AbortSignal.timeout(20000) });
    if (response.ok) { const value = await response.json(); session = { idToken: value.id_token, refreshToken: value.refresh_token, localId: value.user_id }; }
  }
  if (!session) {
    try { session = await auth('signInWithPassword', { email: account.email, password: account.password, returnSecureToken: true }); }
    catch (error) {
      if (!create || !['INVALID_LOGIN_CREDENTIALS', 'EMAIL_NOT_FOUND', 'INVALID_PASSWORD'].includes(error.message)) throw error;
      // EMAIL_EXISTS is not recovered by resetting someone else's password.
      session = await auth('signUp', { email: account.email, password: account.password, returnSecureToken: true });
    }
  }
  const claims = JSON.parse(Buffer.from(session.idToken.split('.')[1], 'base64url'));
  if (claims.aud !== config.projectId || claims.sub !== session.localId || claims.email !== account.email
    || (account.uid && account.uid !== session.localId)) throw new Error('Unexpected test-account token identity');
  Object.assign(account, { uid: session.localId, idToken: session.idToken, refreshToken: session.refreshToken, created: true }); save();
  if (!account.profileUpdated) {
    const profile = await auth('update', { idToken: account.idToken, displayName: account.displayName, returnSecureToken: true });
    if (profile.idToken) account.idToken = profile.idToken;
    if (profile.refreshToken) account.refreshToken = profile.refreshToken;
    account.profileUpdated = true;
  }
  account.checkedAt = new Date().toISOString(); save();
}
console.log(JSON.stringify({ projectId: state.projectId, accounts: state.accounts.map(({ role, email, uid }) => ({ role, email, uid })),
  privateState: file, note: 'Credentials/tokens are private, mode 0600, Git-ignored. Re-run without --create to refresh and reuse.' }, null, 2));
