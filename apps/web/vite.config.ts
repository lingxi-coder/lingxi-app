import { defineConfig, type Plugin } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'node:path';
import { writeFile } from 'node:fs/promises';
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

export default defineConfig({ plugins: [react(), compilerDocumentation()] });
