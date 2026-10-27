import { createHash } from 'node:crypto';
import {
  closeSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  rmSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';

const REQUIRED_PLUGIN_NAMES = [
  'app',
  'audioconvert',
  'audioresample',
  'coreelements',
  'opus',
  'rtp',
  'rtpmanager',
  'vpx',
  'videoconvertscale',
];

const REQUIRED_RUNTIME_LIBRARIES = {
  linux: [
    'libgstreamer-1.0.so.0',
    'libgstapp-1.0.so.0',
    'libgstbase-1.0.so.0',
    'libgstaudio-1.0.so.0',
    'libgstvideo-1.0.so.0',
    'libglib-2.0.so.0',
    'libgobject-2.0.so.0',
    'libgio-2.0.so.0',
  ],
  win32: [
    'gstreamer-1.0-0.dll',
    'gstapp-1.0-0.dll',
    'gstbase-1.0-0.dll',
    'gstaudio-1.0-0.dll',
    'gstvideo-1.0-0.dll',
    'glib-2.0-0.dll',
    'gobject-2.0-0.dll',
    'gio-2.0-0.dll',
  ],
};

const SDK_ENVIRONMENT = {
  linux: 'LINGXI_GSTREAMER_SDK_LINUX',
  win32: 'LINGXI_GSTREAMER_SDK_WINDOWS',
};

const MANIFEST_NAME = 'runtime-manifest.json';
const MANIFEST_VERSION = 1;
const MAX_RUNTIME_FILES = 1_024;
const MAX_RUNTIME_BYTES = 768 * 1024 * 1024;
const MAX_NOTICE_FILES = 512;
const MAX_NOTICE_BYTES = 64 * 1024 * 1024;

function fail(message) {
  throw new Error(message);
}

function within(parent, candidate) {
  const path = relative(parent, candidate);
  return path === '' || (path !== '..' && !path.startsWith(`..${sep}`) && !path.startsWith(sep));
}

function sdkPath(sdkRoot, candidate, label) {
  if (!existsSync(candidate)) fail(`configured GStreamer SDK is missing ${label}: ${candidate}`);
  const resolved = realpathSync(candidate);
  if (!within(sdkRoot, resolved)) fail(`${label} resolves outside the configured GStreamer SDK: ${resolved}`);
  return resolved;
}

function readPrefix(path, length) {
  const fd = openSync(path, 'r');
  try {
    const buffer = Buffer.alloc(length);
    const bytes = readSync(fd, buffer, 0, length, 0);
    return buffer.subarray(0, bytes);
  } finally {
    closeSync(fd);
  }
}

function assertLinuxArchitecture(path, arch, label) {
  const expectedMachine = arch === 'arm64' ? 0xb7 : 0x3e;
  const architectureName = arch === 'arm64' ? 'aarch64' : 'x86_64';
  const bytes = readPrefix(path, 64);
  if (
    bytes.length < 20
    || bytes.subarray(0, 4).toString('hex') !== '7f454c46'
    || bytes[4] !== 2
    || bytes[5] !== 1
    || bytes.readUInt16LE(18) !== expectedMachine
  ) fail(`${label} is not a little-endian Linux ${architectureName} ELF binary: ${path}`);
}

function assertWindowsArchitecture(path, arch, label) {
  const expectedMachine = arch === 'arm64' ? 0xaa64 : 0x8664;
  const architectureName = arch === 'arm64' ? 'ARM64' : 'x64';
  const bytes = readPrefix(path, 4096);
  if (bytes.length < 64 || bytes.subarray(0, 2).toString('ascii') !== 'MZ') {
    fail(`${label} is not a PE binary: ${path}`);
  }
  const peOffset = bytes.readUInt32LE(0x3c);
  if (
    peOffset + 6 > bytes.length
    || bytes.subarray(peOffset, peOffset + 4).toString('ascii') !== 'PE\0\0'
    || bytes.readUInt16LE(peOffset + 4) !== expectedMachine
  ) fail(`${label} is not a Windows ${architectureName} PE binary: ${path}`);
}

function assertArchitecture(path, platform, arch, label) {
  if (platform === 'linux') assertLinuxArchitecture(path, arch, label);
  else assertWindowsArchitecture(path, arch, label);
}

function linuxLibraryRoot(sdkRoot, arch) {
  const multiarch = arch === 'arm64' ? 'aarch64-linux-gnu' : 'x86_64-linux-gnu';
  const candidates = [
    join(sdkRoot, 'lib'),
    join(sdkRoot, 'lib64'),
    join(sdkRoot, 'lib', multiarch),
  ].filter((candidate) => existsSync(join(candidate, 'gstreamer-1.0')));
  const unique = new Map();
  for (const candidate of candidates) {
    const resolved = sdkPath(sdkRoot, candidate, 'GStreamer library directory');
    unique.set(resolved, resolved);
  }
  if (unique.size !== 1) {
    fail(
      `configured Linux GStreamer SDK must have one standard lib/{,${multiarch},64}/gstreamer-1.0 plugin tree; found ${unique.size}`,
    );
  }
  return [...unique.values()][0];
}

function pluginNames(platform) {
  return REQUIRED_PLUGIN_NAMES.map((name) => (
    platform === 'linux' ? `libgst${name}.so` : `gst${name}.dll`
  ));
}

function collectRuntimeFiles(sdkRoot, platform, arch, libraryRoot, pluginRoot) {
  const runtimeDirectory = platform === 'linux' ? 'lib' : 'bin';
  const sourceLibraryDirectory = platform === 'linux' ? libraryRoot : join(sdkRoot, 'bin');
  const packageLibraryDirectory = runtimeDirectory;
  const dynamicLibraryPattern = platform === 'linux' ? /^lib.+\.so(?:\.\d+)*$/ : /\.dll$/i;
  const files = [];
  let totalBytes = 0;

  const addFile = (file) => {
    if (files.length >= MAX_RUNTIME_FILES) fail(`configured GStreamer SDK exceeds the ${MAX_RUNTIME_FILES}-file runtime bound`);
    file.size = statSync(file.source).size;
    totalBytes += file.size;
    if (totalBytes > MAX_RUNTIME_BYTES) fail(`configured GStreamer SDK exceeds the ${MAX_RUNTIME_BYTES}-byte runtime bound`);
    file.sha256 = hashFile(file.source);
    files.push(file);
  };

  for (const entry of readdirSync(sourceLibraryDirectory, { withFileTypes: true })) {
    if (!dynamicLibraryPattern.test(entry.name)) continue;
    if (!entry.isFile() && !entry.isSymbolicLink()) continue;
    const source = sdkPath(sdkRoot, join(sourceLibraryDirectory, entry.name), 'runtime library');
    if (!statSync(source).isFile()) continue;
    const file = {
      kind: 'library',
      sourcePath: relative(sdkRoot, join(sourceLibraryDirectory, entry.name)).split(sep).join('/'),
      source,
      packagePath: join(packageLibraryDirectory, entry.name).split(sep).join('/'),
    };
    assertArchitecture(file.source, platform, arch, `${file.kind} ${basename(file.source)}`);
    addFile(file);
  }

  for (const name of pluginNames(platform)) {
    const source = sdkPath(sdkRoot, join(pluginRoot, name), `required plugin ${name}`);
    if (!statSync(source).isFile()) fail(`required GStreamer plugin is not a file: ${source}`);
    const file = {
      kind: 'plugin',
      sourcePath: relative(sdkRoot, join(pluginRoot, name)).split(sep).join('/'),
      source,
      packagePath: join('lib', 'gstreamer-1.0', name).split(sep).join('/'),
    };
    assertArchitecture(file.source, platform, arch, `${file.kind} ${basename(file.source)}`);
    addFile(file);
  }

  const required = REQUIRED_RUNTIME_LIBRARIES[platform];
  const libraryNames = new Set(files.filter((file) => file.kind === 'library').map((file) => basename(file.packagePath).toLowerCase()));
  for (const name of required) {
    if (!libraryNames.has(name.toLowerCase())) {
      fail(`configured GStreamer SDK is missing required runtime library ${name} in ${sourceLibraryDirectory}`);
    }
  }

  const uniquePackageNames = new Map();
  for (const file of files) {
    const key = file.packagePath.toLowerCase();
    const existing = uniquePackageNames.get(key);
    if (existing) fail(`GStreamer SDK contains a case-insensitive package path collision: ${existing} and ${file.packagePath}`);
    uniquePackageNames.set(key, file.packagePath);
  }
  files.sort((left, right) => left.packagePath.localeCompare(right.packagePath));
  return { files, runtimeDirectory, pluginDirectory: 'lib/gstreamer-1.0', totalBytes };
}

function collectNotices(sdkRoot) {
  const result = [];
  const seen = new Set();
  const candidates = [];
  const addCandidate = (candidate) => {
    if (candidates.length >= MAX_NOTICE_FILES) fail(`GStreamer SDK exceeds the ${MAX_NOTICE_FILES}-notice file bound`);
    candidates.push(candidate);
  };
  for (const name of ['LICENSE', 'LICENSE.txt', 'COPYING', 'COPYING.LIB', 'NOTICE']) {
    const candidate = join(sdkRoot, name);
    if (existsSync(candidate) && lstatSync(candidate).isFile()) addCandidate({ source: candidate, sourcePath: name });
  }
  const licensesRoot = join(sdkRoot, 'share', 'licenses');
  if (existsSync(licensesRoot)) {
    const root = sdkPath(sdkRoot, licensesRoot, 'license directory');
    const visit = (directory, relativeDirectory) => {
      for (const entry of readdirSync(directory, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
        if (entry.isSymbolicLink()) continue;
        const source = join(directory, entry.name);
        const sourcePath = join(relativeDirectory, entry.name).split(sep).join('/');
        if (entry.isDirectory()) visit(source, sourcePath);
        else if (entry.isFile()) addCandidate({ source, sourcePath });
      }
    };
    visit(root, 'share/licenses');
  }
  // Linux distributions commonly install copyright notices under
  // share/doc/<gstreamer-package>/ instead of share/licenses/.
  const docsRoot = join(sdkRoot, 'share', 'doc');
  if (existsSync(docsRoot)) {
    const root = sdkPath(sdkRoot, docsRoot, 'documentation directory');
    const packageName = /^(?:gstreamer|libgstreamer|gstreamer1\.0|libgst|gst-plugins)/i;
    const noticeName = /^(?:copyright|license(?:s)?(?:[._-].*)?|copying(?:[._-].*)?|notice(?:[._-].*)?)$/i;
    for (const entry of readdirSync(root, { withFileTypes: true }).sort((a, b) => a.name.localeCompare(b.name))) {
      if (!entry.isDirectory() || !packageName.test(entry.name)) continue;
      const packageRoot = join(root, entry.name);
      for (const notice of readdirSync(packageRoot, { withFileTypes: true })) {
        if (!notice.isFile() || !noticeName.test(notice.name)) continue;
        addCandidate({
          source: join(packageRoot, notice.name),
          sourcePath: join('share', 'doc', entry.name, notice.name).split(sep).join('/'),
        });
      }
    }
  }
  let totalBytes = 0;
  for (const item of candidates) {
    const source = sdkPath(sdkRoot, item.source, 'license or notice');
    const sourceRelative = relative(sdkRoot, item.source).split(sep).join('/');
    if (seen.has(sourceRelative)) continue;
    seen.add(sourceRelative);
    const size = statSync(source).size;
    totalBytes += size;
    if (totalBytes > MAX_NOTICE_BYTES) fail(`GStreamer SDK license material exceeds the ${MAX_NOTICE_BYTES}-byte bound`);
    result.push({
      kind: 'notice',
      sourcePath: item.sourcePath,
      source,
      packagePath: join('licenses', sourceRelative).split(sep).join('/'),
      size,
      sha256: hashFile(source),
    });
  }
  return result.sort((left, right) => left.packagePath.localeCompare(right.packagePath));
}

function hashFile(path) {
  const hash = createHash('sha256');
  const fd = openSync(path, 'r');
  try {
    const chunk = Buffer.alloc(1024 * 1024);
    while (true) {
      const bytes = readSync(fd, chunk, 0, chunk.length, null);
      if (bytes === 0) break;
      hash.update(chunk.subarray(0, bytes));
    }
    return hash.digest('hex');
  } finally {
    closeSync(fd);
  }
}

/** Validate an explicit SDK before packaging removes any previous artifact. */
export function preflightPortableGStreamerRuntime(sidecar, platform, arch, env = process.env) {
  if (!(platform in SDK_ENVIRONMENT)) fail(`portable GStreamer packaging does not support ${platform}`);
  if (!['x64', 'arm64'].includes(arch)) fail(`portable GStreamer SDK packaging requires x64 or arm64; received ${arch}`);
  const variable = SDK_ENVIRONMENT[platform];
  const configured = env[variable]?.trim();
  if (!configured) fail(`bridge-server uses GStreamer; set ${variable} to the matching runtime SDK prefix before packaging`);
  const configuredPath = resolve(configured);
  if (!existsSync(configuredPath)) fail(`${variable} does not exist: ${configuredPath}`);
  const sdkRoot = realpathSync(configuredPath);
  if (!statSync(sdkRoot).isDirectory()) fail(`${variable} must point to a directory: ${sdkRoot}`);
  if (!existsSync(sidecar)) fail(`bridge-server sidecar is missing: ${sidecar}`);
  assertArchitecture(sidecar, platform, arch, 'bridge-server sidecar');

  const libraryRoot = platform === 'linux'
    ? linuxLibraryRoot(sdkRoot, arch)
    : sdkPath(sdkRoot, join(sdkRoot, 'lib'), 'runtime library directory');
  const pluginRoot = sdkPath(sdkRoot, join(libraryRoot, 'gstreamer-1.0'), 'GStreamer plugin directory');
  const runtime = collectRuntimeFiles(sdkRoot, platform, arch, libraryRoot, pluginRoot);
  const notices = collectNotices(sdkRoot);
  return {
    sdkRoot,
    platform,
    arch,
    runtime,
    notices,
  };
}

/** Copy the preflighted runtime files and write their integrity manifest. */
export function bundlePortableGStreamerRuntime(plan, resourcesDirectory) {
  const runtimeRoot = join(resourcesDirectory, 'gstreamer');
  rmSync(runtimeRoot, { recursive: true, force: true });
  mkdirSync(runtimeRoot, { recursive: true });
  for (const file of [...plan.runtime.files, ...plan.notices]) {
    const destination = join(runtimeRoot, file.packagePath);
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(file.source, destination);
  }
  const manifest = {
    schemaVersion: MANIFEST_VERSION,
    platform: plan.platform,
    architecture: plan.arch,
    runtimeDirectory: plan.runtime.runtimeDirectory,
    pluginDirectory: plan.runtime.pluginDirectory,
    requiredPlugins: pluginNames(plan.platform),
    runtimeBytes: plan.runtime.totalBytes,
    runtimeFiles: plan.runtime.files.map(manifestFile),
    notices: plan.notices.map(manifestFile),
  };
  writeFileSync(join(runtimeRoot, MANIFEST_NAME), `${JSON.stringify(manifest, null, 2)}\n`, 'utf8');
  return verifyPortableGStreamerRuntime(resourcesDirectory, plan.platform, plan.arch);
}

function manifestFile({ kind, sourcePath, packagePath, size, sha256 }) {
  return { kind, sourcePath, packagePath, size, sha256 };
}

function assertManifestFile(root, file, expectedKind) {
  if (file.kind !== expectedKind || typeof file.packagePath !== 'string' || typeof file.sourcePath !== 'string') {
    fail('GStreamer runtime manifest contains an invalid file entry');
  }
  if (
    file.packagePath.includes('\\')
    || file.sourcePath.includes('\\')
    || file.packagePath.includes(':')
    || file.sourcePath.includes(':')
    || file.packagePath.startsWith('/')
    || file.sourcePath.startsWith('/')
    || file.packagePath.split('/').includes('..')
    || file.sourcePath.split('/').includes('..')
  ) fail('GStreamer runtime manifest contains an unsafe path');
  if (!Number.isSafeInteger(file.size) || file.size < 0 || !/^[a-f0-9]{64}$/.test(file.sha256)) {
    fail(`GStreamer runtime manifest contains invalid integrity metadata: ${file.packagePath}`);
  }
  const path = join(root, file.packagePath);
  if (!existsSync(path) || !lstatSync(path).isFile()) fail(`packaged GStreamer file is missing: ${file.packagePath}`);
  const resolvedRoot = realpathSync(root);
  const resolvedFile = realpathSync(path);
  if (!within(resolvedRoot, resolvedFile)) fail(`packaged GStreamer file escapes its runtime directory: ${file.packagePath}`);
  if (statSync(path).size !== file.size || hashFile(path) !== file.sha256) {
    fail(`packaged GStreamer file failed integrity verification: ${file.packagePath}`);
  }
  return path;
}

function collectPackageFiles(directory, relativeDirectory = '') {
  const files = [];
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const relativePath = join(relativeDirectory, entry.name).split(sep).join('/');
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink()) fail(`portable GStreamer runtime contains an unexpected symlink: ${relativePath}`);
    if (entry.isDirectory()) files.push(...collectPackageFiles(path, relativePath));
    else if (entry.isFile()) files.push(relativePath);
    else fail(`portable GStreamer runtime contains an unsupported file type: ${relativePath}`);
  }
  return files;
}

/** Verify the portable GStreamer runtime tree from its packaged manifest. */
export function verifyPortableGStreamerRuntime(resourcesDirectory, platform, arch) {
  if (!(platform in SDK_ENVIRONMENT) || !['x64', 'arm64'].includes(arch)) fail(`unsupported portable GStreamer target: ${platform}-${arch}`);
  const runtimeRoot = join(resourcesDirectory, 'gstreamer');
  const manifestPath = join(runtimeRoot, MANIFEST_NAME);
  if (!existsSync(runtimeRoot) || !lstatSync(runtimeRoot).isDirectory()) fail(`portable GStreamer runtime directory is missing: ${runtimeRoot}`);
  if (!existsSync(manifestPath)) fail(`portable GStreamer runtime manifest is missing: ${manifestPath}`);
  if (!lstatSync(manifestPath).isFile()) fail('portable GStreamer runtime manifest must be a regular file');
  if (!within(realpathSync(resourcesDirectory), realpathSync(runtimeRoot))) fail('portable GStreamer runtime directory escapes package resources');
  let manifest;
  try {
    manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  } catch (error) {
    fail(`portable GStreamer runtime manifest is invalid JSON: ${error.message}`);
  }
  if (
    manifest.schemaVersion !== MANIFEST_VERSION
    || manifest.platform !== platform
    || manifest.architecture !== arch
    || manifest.runtimeDirectory !== (platform === 'linux' ? 'lib' : 'bin')
    || manifest.pluginDirectory !== 'lib/gstreamer-1.0'
    || JSON.stringify(manifest.requiredPlugins) !== JSON.stringify(pluginNames(platform))
    || !Array.isArray(manifest.runtimeFiles)
    || !Array.isArray(manifest.notices)
  ) fail('portable GStreamer runtime manifest does not match the requested target');

  if (manifest.runtimeFiles.length > MAX_RUNTIME_FILES || manifest.notices.length > MAX_NOTICE_FILES) {
    fail('portable GStreamer runtime manifest exceeds file-count bounds');
  }
  const packagePaths = new Set();
  let runtimeBytes = 0;
  const libraryNames = new Set();
  const pluginPaths = new Set();
  for (const file of manifest.runtimeFiles) {
    const path = assertManifestFile(runtimeRoot, file, file.kind);
    if (!['library', 'plugin'].includes(file.kind)) fail(`invalid portable GStreamer runtime kind: ${file.kind}`);
    const key = file.packagePath.toLowerCase();
    if (packagePaths.has(key)) fail(`duplicate portable GStreamer package path: ${file.packagePath}`);
    packagePaths.add(key);
    runtimeBytes += file.size;
    if (file.kind === 'library') libraryNames.add(basename(file.packagePath).toLowerCase());
    else {
      pluginPaths.add(file.packagePath);
      assertArchitecture(path, platform, arch, `packaged plugin ${basename(path)}`);
    }
    if (file.kind === 'library') assertArchitecture(path, platform, arch, `packaged runtime library ${basename(path)}`);
  }
  if (runtimeBytes > MAX_RUNTIME_BYTES || manifest.runtimeBytes !== runtimeBytes) {
    fail('portable GStreamer runtime byte count does not match its manifest');
  }
  for (const name of REQUIRED_RUNTIME_LIBRARIES[platform]) {
    if (!libraryNames.has(name.toLowerCase())) fail(`packaged GStreamer runtime is missing required library ${name}`);
  }
  for (const name of pluginNames(platform)) {
    const expected = join('lib', 'gstreamer-1.0', name).split(sep).join('/');
    if (!pluginPaths.has(expected)) fail(`packaged GStreamer runtime is missing required plugin ${name}`);
  }
  let noticeBytes = 0;
  const noticePaths = new Set();
  for (const notice of manifest.notices) {
    const path = assertManifestFile(runtimeRoot, notice, 'notice');
    if (!notice.packagePath.startsWith('licenses/')) fail('packaged GStreamer notice is outside licenses/');
    if (noticePaths.has(notice.packagePath.toLowerCase())) fail(`duplicate packaged GStreamer notice: ${notice.packagePath}`);
    noticePaths.add(notice.packagePath.toLowerCase());
    noticeBytes += statSync(path).size;
  }
  if (noticeBytes > MAX_NOTICE_BYTES) fail('packaged GStreamer license material exceeds byte bounds');
  const expectedFiles = new Set([
    MANIFEST_NAME,
    ...manifest.runtimeFiles.map((file) => file.packagePath),
    ...manifest.notices.map((file) => file.packagePath),
  ]);
  const actualFiles = collectPackageFiles(runtimeRoot);
  if (actualFiles.length !== expectedFiles.size || actualFiles.some((path) => !expectedFiles.has(path))) {
    fail('portable GStreamer runtime contains unmanifested or duplicate files');
  }
  const pluginDirectory = join(runtimeRoot, manifest.pluginDirectory);
  const actualPlugins = readdirSync(pluginDirectory).sort();
  if (JSON.stringify(actualPlugins) !== JSON.stringify([...pluginNames(platform)].sort())) {
    fail('portable GStreamer plugin directory contains files outside the required plugin set');
  }
  return {
    bundled: true,
    runtimeDirectory: join(runtimeRoot, manifest.runtimeDirectory),
    pluginDirectory: join(runtimeRoot, manifest.pluginDirectory),
    manifestPath,
    runtimeFiles: manifest.runtimeFiles.length,
  };
}

export const PORTABLE_GSTREAMER_REQUIRED_PLUGINS = [...REQUIRED_PLUGIN_NAMES];
