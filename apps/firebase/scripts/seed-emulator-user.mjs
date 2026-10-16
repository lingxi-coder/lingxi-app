import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
if (process.env.FIREBASE_AUTH_EMULATOR_HOST !== '127.0.0.1:9098') throw Error('Only the local auth emulator is supported.');
const app = initializeApp({ projectId: 'demo-lingxi-backend' });
const auth = getAuth(app);
for (const [email, isAdmin] of [['admin@example.com', true], ['viewer@example.com', false]]) {
  let user;
  try { user = await auth.createUser({ email, password: 'LocalOnly-2026-Admin!', emailVerified: true, displayName: isAdmin ? 'Local administrator' : 'Local viewer' }); }
  catch (error) { if (error.code !== 'auth/email-already-exists') throw error; user = await auth.getUserByEmail(email); }
  await auth.setCustomUserClaims(user.uid, { model_catalog_admin: isAdmin });
}
console.log('Local-only emulator administrator and viewer prepared.');
