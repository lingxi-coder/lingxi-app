import { mergeConfig } from 'vite';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import webConfig from '../vite.config';

const webRoot = fileURLToPath(new URL('..', import.meta.url));
const probeRoot = path.resolve(webRoot, '../../acceptance/teams-t3-complete-20261029/probes/web');

export default mergeConfig(webConfig, {
  root: probeRoot,
  publicDir: false,
  cacheDir: '/private/tmp/lingxi-teams-t3-web-vite-cache',
  resolve: { alias: [
    { find: /^react-dom(?=\/|$)/u, replacement: path.join(webRoot, 'node_modules/react-dom') },
    { find: /^react(?=\/|$)/u, replacement: path.join(webRoot, 'node_modules/react') },
    { find: /^firebase(?=\/|$)/u, replacement: path.join(webRoot, 'node_modules/firebase') },
  ] },
  server: {
    host: '127.0.0.1', port: 18784, strictPort: true, hmr: false,
    proxy: {
      '/v1/teams': { target: 'http://127.0.0.1:18783' },
      '/_acceptance': { target: 'http://127.0.0.1:18783' },
    },
  },
});
