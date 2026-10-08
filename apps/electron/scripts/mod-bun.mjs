import { execFileSync } from 'node:child_process';
import { chmodSync, copyFileSync, mkdirSync, readFileSync, realpathSync, writeFileSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { join } from 'node:path';

const manifestName = 'mod-bun.json';
const noticeName = 'Bun-LICENSE.md';
const inspectExpression = `JSON.stringify({
  executable: process.execPath, version: Bun.version, revision: Bun.revision,
  platform: process.platform, arch: process.arch,
  transformed: new Bun.Transpiler({loader: 'tsx', macro: false}).transformSync(
    '/** @jsxRuntime classic */ /** @jsx h */ /** @jsxFrag Fragment */\\nexport default () => <Text>client</Text>'
  )
})`;

function digest(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

export function modBunFilename(platform) {
  return platform === 'win32' ? 'bun.exe' : 'bun';
}

/** Inspect the compiler that will be copied, rather than assuming a developer's PATH at runtime. */
export function inspectModBun(executable, platform, arch) {
  let identity;
  try {
    identity = JSON.parse(execFileSync(executable, ['--no-env-file', '--print', inspectExpression], {
      encoding: 'utf8', stdio: 'pipe', timeout: 15_000, maxBuffer: 1024 * 1024,
    }));
  } catch (error) {
    throw new Error(`Mod compiler must be a working Bun executable: ${error.message}`);
  }
  if (identity.platform !== platform || identity.arch !== arch) {
    throw new Error(`Mod compiler is ${identity.platform}-${identity.arch}; expected ${platform}-${arch}`);
  }
  if (!/^\d+\.\d+\.\d+$/.test(identity.version) || !/^[a-f0-9]{7,40}$/.test(identity.revision)
    || typeof identity.transformed !== 'string' || !identity.transformed.includes('h(Text')) {
    throw new Error('Mod compiler did not expose the required Bun.Transpiler contract');
  }
  return {
    executable: realpathSync(identity.executable),
    version: identity.version, revision: identity.revision, platform, arch,
  };
}

/** Resolve and capture the release's own notices before assembling a Desktop package. */
export async function prepareModBun({ platform, arch, configuredPath = process.env.LINGXI_MOD_BUN_EXECUTABLE }) {
  const identity = inspectModBun(configuredPath || 'bun', platform, arch);
  const noticesUrl = `https://raw.githubusercontent.com/oven-sh/bun/bun-v${identity.version}/LICENSE.md`;
  const response = await fetch(noticesUrl, { signal: AbortSignal.timeout(15_000) });
  if (!response.ok) throw new Error(`Unable to read Bun ${identity.version} notices (${response.status})`);
  const notices = await response.text();
  if (!notices.includes('MIT') || !notices.includes('JavaScriptCore')) {
    throw new Error(`Bun ${identity.version} notices do not describe the compiler distribution`);
  }
  return { ...identity, noticesUrl, notices };
}

export function stageModBun(resources, compiler) {
  const binDir = join(resources, 'bin');
  mkdirSync(binDir, { recursive: true });
  const filename = modBunFilename(compiler.platform);
  const executable = join(binDir, filename);
  copyFileSync(compiler.executable, executable);
  if (compiler.platform !== 'win32') chmodSync(executable, 0o755);
  writeFileSync(join(resources, noticeName), compiler.notices, 'utf8');
  writeFileSync(join(resources, manifestName), `${JSON.stringify({
    version: compiler.version, revision: compiler.revision,
    platform: compiler.platform, arch: compiler.arch, executable: filename,
    noticesUrl: compiler.noticesUrl, notices: noticeName,
    sourceSha256: digest(compiler.executable), sha256: digest(executable),
    noticesSha256: digest(join(resources, noticeName)),
  }, null, 2)}\n`);
  return executable;
}

/** Signing changes Mach-O bytes; seal the packaged digest after inside-out signing. */
export function sealModBun(resources) {
  const path = join(resources, manifestName);
  const manifest = JSON.parse(readFileSync(path, 'utf8'));
  manifest.sha256 = digest(join(resources, 'bin', modBunFilename(manifest.platform)));
  writeFileSync(path, `${JSON.stringify(manifest, null, 2)}\n`);
}

export function verifyModBun(resources, platform, arch) {
  const manifest = JSON.parse(readFileSync(join(resources, manifestName), 'utf8'));
  const executable = join(resources, 'bin', modBunFilename(platform));
  if (manifest.platform !== platform || manifest.arch !== arch
    || manifest.executable !== modBunFilename(platform) || manifest.notices !== noticeName
    || manifest.sha256 !== digest(executable)
    || manifest.noticesSha256 !== digest(join(resources, noticeName))) {
    throw new Error('Packaged Mod compiler does not match its recorded identity');
  }
  const identity = inspectModBun(executable, platform, arch);
  if (identity.version !== manifest.version || identity.revision !== manifest.revision) {
    throw new Error('Packaged Mod compiler version does not match its recorded identity');
  }
  return manifest;
}
