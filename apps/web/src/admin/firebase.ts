// The Firebase app and auth moved to `app/firebase` so Remote Control shares them.
export { auth } from '../app/firebase';
export const catalogApiOrigin = import.meta.env.VITE_CATALOG_API_ORIGIN || '';
