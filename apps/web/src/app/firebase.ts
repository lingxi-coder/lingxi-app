import { initializeApp, getApps } from 'firebase/app';
import { browserLocalPersistence, browserSessionPersistence, connectAuthEmulator, getAuth, indexedDBLocalPersistence, initializeAuth, onAuthStateChanged } from 'firebase/auth';
import { setSessionHint } from './sessionHint';

// Shared by the admin console and Remote Control: both sign in against the same
// Firebase app, so a session started on one page is visible to the other.
const emulator = import.meta.env.DEV ? import.meta.env.VITE_AUTH_EMULATOR_URL : undefined;
const emulatorProject = import.meta.env.DEV ? import.meta.env.VITE_FIREBASE_PROJECT_ID : undefined;
if (emulatorProject && (!emulator || !emulatorProject.startsWith('demo-'))) throw new Error('Local Firebase tests require a demo project and Auth emulator');
const defaultConfig = {
  apiKey: 'AIzaSyAW8Uc_kkM1oSKUsP3whISCTx64G7ci_h0',
  authDomain: 'lingxi-5e4b2.firebaseapp.com',
  projectId: emulator ? emulatorProject ?? 'demo-lingxi-backend' : 'lingxi-5e4b2',
  appId: '1:55470483519:web:0100e420a56a3ab957708a',
};

function configuredFirebase(): typeof defaultConfig {
  const source = import.meta.env.VITE_LINGXI_STAGING_FIREBASE;
  if (!source) return defaultConfig;
  const config = JSON.parse(source) as typeof defaultConfig;
  if (emulator || !/^[a-z][a-z0-9-]{4,28}[a-z0-9]$/u.test(config.projectId) ||
      config.projectId === defaultConfig.projectId || config.authDomain !== config.projectId + '.firebaseapp.com' ||
      typeof config.apiKey !== 'string' || !/^1:\d+:web:[a-f0-9]+$/u.test(config.appId)) {
    throw new Error('Invalid staging Firebase configuration');
  }
  return { apiKey: config.apiKey, authDomain: config.authDomain, projectId: config.projectId, appId: config.appId };
}

const app = getApps().find((entry) => entry.name === 'lingxi-web') ?? initializeApp(configuredFirebase(), 'lingxi-web');

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
// Every page that signs in or out imports this module, so the landing page's session hint follows them all.
onAuthStateChanged(auth, user => setSessionHint(!!user));
