import { readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'esbuild';

export async function freePort() {
  const server = createServer();
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  const port = server.address().port;
  await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  return port;
}

export async function loadSourceHost() {
  const bundled = await build({
    stdin: {
      contents: "export { LocalAppHost } from './host.ts'; export { LocalAppAgentBroker } from './agentBroker.ts';",
      resolveDir: fileURLToPath(new URL('../../src/main/local-apps/', import.meta.url)), loader: 'ts',
    },
    bundle: true, write: false, platform: 'node', format: 'cjs', external: ['electron'], metafile: true,
    // Carry the test process's isolated bridge-client resolution into esbuild.
    plugins: [{ name: 'bridge-client-test-resolution', setup(builder) {
      builder.onResolve({ filter: /^@lingxi\/bridge-client(?:\/|$)/ }, args => ({ path: fileURLToPath(import.meta.resolve(args.path)) }));
    } }],
  });
  if (process.env.LINGXI_TEST_ARTIFACT_DIR) await writeFile(join(process.env.LINGXI_TEST_ARTIFACT_DIR, 'source-host-inputs.json'), JSON.stringify({
    bridgeClientEntry: import.meta.resolve('@lingxi/bridge-client'), inputs: Object.keys(bundled.metafile.inputs),
  }, null, 2));
  const require = createRequire(import.meta.url);
  const electron = {
    BrowserWindow: class { static getAllWindows() { return []; } },
    ipcMain: { removeHandler() {} },
    session: { fromPartition: () => ({ flushStorageData() {} }) },
    dialog: { showMessageBox: async () => ({ response: 1 }) },
    net: { fetch: async url => {
      const file = new URL(url);
      const contentType = file.pathname.endsWith('.js') ? 'text/javascript' : 'text/html';
      return new Response(await readFile(file), { headers: { 'content-type': contentType + '; charset=utf-8' } });
    } },
  };
  const module = { exports: {} };
  new Function('module', 'exports', 'require', bundled.outputFiles[0].text)(module, module.exports, name => name === 'electron' ? electron : require(name));
  return module.exports;
}
