import { initializeApp, getApps } from 'firebase/app';
import { browserLocalPersistence, browserSessionPersistence, connectAuthEmulator, getAuth, indexedDBLocalPersistence, initializeAuth } from 'firebase/auth';

// Shared by the admin console and Remote Control: both sign in against the same
// Firebase app, so a session started on one page is visible to the other.
const emulator = import.meta.env.DEV ? import.meta.env.VITE_AUTH_EMULATOR_URL : undefined;
const app = getApps().find((entry) => entry.name === 'lingxi-web') ?? initializeApp({
  apiKey: 'AIzaSyAW8Uc_kkM1oSKUsP3whISCTx64G7ci_h0',
  authDomain: 'lingxi-5e4b2.firebaseapp.com',
  projectId: emulator ? 'demo-lingxi-backend' : 'lingxi-5e4b2',
  appId: '1:55470483519:web:0100e420a56a3ab957708a',
}, 'lingxi-web');

// `getAuth` registers the popup/redirect resolver, and with it the SDK may load
// Google's sign-in frame on startup. The Remote Control pages run under a strict
// Content-Security-Policy (`firebase.json`) that allows neither, so there the
// auth object is created without the resolver, with the same persistence and
// storage keys, and only reads the signed-in user. Signing in happens on
// `/login/remote`, a separate document.
const strictDocument = typeof location !== 'undefined' && /^\/remote(\/|$)/iu.test(location.pathname);
export const auth = strictDocument
  ? initializeAuth(app, { persistence: [indexedDBLocalPersistence, browserLocalPersistence, browserSessionPersistence] })
  : getAuth(app);
if (emulator) connectAuthEmulator(auth, emulator, { disableWarnings: true });
