import { initializeApp, getApps } from 'firebase/app';
import { connectAuthEmulator, getAuth } from 'firebase/auth';

const emulator = import.meta.env.DEV ? import.meta.env.VITE_AUTH_EMULATOR_URL : undefined;
const app = getApps().find((entry) => entry.name === 'lingxi-web') ?? initializeApp({
  apiKey: 'AIzaSyAW8Uc_kkM1oSKUsP3whISCTx64G7ci_h0',
  authDomain: 'lingxi-5e4b2.firebaseapp.com',
  projectId: emulator ? 'demo-lingxi-backend' : 'lingxi-5e4b2',
  appId: '1:55470483519:web:0100e420a56a3ab957708a',
}, 'lingxi-web');
export const auth = getAuth(app);
if (emulator) connectAuthEmulator(auth, emulator, { disableWarnings: true });
export const catalogApiOrigin = import.meta.env.VITE_CATALOG_API_ORIGIN || '';
