import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'node:path';
import { readFileSync } from 'node:fs';
import { writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { availableRustdocManifest } from './scripts/rustdoc-availability';

function compilerDocumentation(): Plugin {
  let root: string;
  let output: string;
  return {
    name: 'compiler-documentation-availability',
    configResolved(config) { root = config.root; output = path.resolve(root, config.build.outDir); },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        if (request.url?.split('?')[0] !== '/rustdoc-manifest.json') return next();
        void availableRustdocManifest(path.join(root, 'public')).then((manifest) => {
          response.setHeader('Content-Type', 'application/json; charset=utf-8');
          response.end(JSON.stringify(manifest));
        }).catch(next);
      });
    },
    async writeBundle() {
      const manifest = await availableRustdocManifest(output);
      await writeFile(path.join(output, 'rustdoc-manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
    },
  };
}

const webRoot = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(webRoot, '../..');
const bridgeClient = path.join(repoRoot, 'packages/bridge-client/src');
const electronSrc = path.join(repoRoot, 'apps/electron/src');
const { version } = JSON.parse(readFileSync(path.join(webRoot, 'package.json'), 'utf8')) as { version: string };

// Remote Control reuses code that lives next to the other clients; `tsconfig.json` has the same map for the
// type checker, and `test/remoteImports.test.ts` checks that nothing reached through it needs a package this
// app does not declare. The bare `@lingxi/bridge-client` is the types-only `protocol.ts` rather than the
// package root: the root re-exports `client.ts` (`ws`, `node:events`) and `lockfile.ts` (`node:fs`, `node:os`),
// which a browser bundle can neither resolve nor run.
const sharedSources = [
  { find: /^@lingxi\/bridge-client\/remote$/u, replacement: path.join(bridgeClient, 'remote.ts') },
  { find: /^@lingxi\/bridge-client\/toolview$/u, replacement: path.join(bridgeClient, 'toolview.ts') },
  { find: /^@lingxi\/bridge-client\/protocol$/u, replacement: path.join(bridgeClient, 'protocol.ts') },
  { find: /^@lingxi\/bridge-client$/u, replacement: path.join(bridgeClient, 'protocol.ts') },
  { find: /^@lingxi\/electron\//u, replacement: `${electronSrc}/` },
];

export default defineConfig({
  plugins: [react(), compilerDocumentation()],
  resolve: { alias: sharedSources, dedupe: ['react', 'react-dom'] },
  define: { __LINGXI_WEB_VERSION__: JSON.stringify(version) },
  // The dev server only serves files under the app by default; the shared sources are two directories up.
  server: { fs: {
    allow: [repoRoot],
    // Keep reusable acceptance credentials/session exports private even though
    // shared source modules live outside the Web directory.
    deny: ['.env', '.env.*', '*.{crt,pem}', '**/.git/**', '**/teams-t1-private/**'],
  } },
});
