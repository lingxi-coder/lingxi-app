import {
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  realpathSync,
  rmSync,
} from 'node:fs';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';
import { execFileSync } from 'node:child_process';

const REQUIRED_PLUGINS = [
  'libgstapp.dylib',
  'libgstaudioconvert.dylib',
  'libgstaudioresample.dylib',
  'libgstcoreelements.dylib',
  'libgstopus.dylib',
  'libgstrtp.dylib',
  'libgstrtpmanager.dylib',
  'libgstvpx.dylib',
  'libgstvideoconvertscale.dylib',
];
const SYSTEM_PREFIXES = ['/System/Library/', '/usr/lib/'];

function run(command, args) {
  return execFileSync(command, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function fail(message) {
  throw new Error(message);
}

function isWithin(parent, candidate) {
  const path = relative(parent, candidate);
  return path === '' || (path !== '..' && !path.startsWith(`..${sep}`) && !path.startsWith(sep));
}

function machOArchitectures(path) {
  return run('/usr/bin/lipo', ['-archs', path]).split(/\s+/).filter(Boolean);
}

function isSystemDependency(path) {
  return SYSTEM_PREFIXES.some((prefix) => path.startsWith(prefix));
}

function dylibDependencies(path) {
  const output = run('/usr/bin/otool', ['-L', path]);
  return output.split(/\r?\n/).slice(1).map((line) => line.trim().split(/\s+\(compatibility version/)[0]).filter(Boolean);
}

function rpaths(path) {
  const output = run('/usr/bin/otool', ['-l', path]);
  return [...output.matchAll(/cmd LC_RPATH\s+cmdsize \d+\s+path (.+?) \(offset \d+\)/g)].map((match) => match[1]);
}

function expandRpath(value, binaryPath, executablePath) {
  if (value.startsWith('@loader_path/')) return resolve(dirname(binaryPath), value.slice('@loader_path/'.length));
  if (value === '@loader_path') return dirname(binaryPath);
  if (value.startsWith('@executable_path/')) return resolve(dirname(executablePath), value.slice('@executable_path/'.length));
  if (value === '@executable_path') return dirname(executablePath);
  return value;
}

function dependencySource(reference, binaryPath, executablePath, sdkRoot, sdkSearchDirs) {
  if (isSystemDependency(reference)) return null;
  const candidates = [];
  if (reference.startsWith('@loader_path/') || reference === '@loader_path'
      || reference.startsWith('@executable_path/') || reference === '@executable_path') {
    candidates.push(expandRpath(reference, binaryPath, executablePath));
  } else if (reference.startsWith('@rpath/')) {
    const suffix = reference.slice('@rpath/'.length);
    for (const path of rpaths(binaryPath)) {
      candidates.push(join(expandRpath(path, binaryPath, executablePath), suffix));
    }
    candidates.push(...sdkSearchDirs.map((directory) => join(directory, suffix)));
  } else if (reference.startsWith('/')) {
    candidates.push(reference);
  } else {
    candidates.push(join(dirname(binaryPath), reference));
    candidates.push(...sdkSearchDirs.map((directory) => join(directory, reference)));
  }
  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      const source = realpathSync(candidate);
      if (!isWithin(sdkRoot, source)) {
        fail(`GStreamer dependency resolves outside LINGXI_GSTREAMER_SDK_MACOS: ${reference} -> ${source}`);
      }
      return source;
    }
  }
  fail(`cannot resolve GStreamer dependency ${reference} required by ${binaryPath} within ${sdkRoot}`);
}

function findPluginDirectory(sdkRoot) {
  const pkgConfigDir = join(sdkRoot, 'lib', 'pkgconfig');
  const env = { ...process.env, PKG_CONFIG_PATH: '', PKG_CONFIG_LIBDIR: pkgConfigDir };
  let plugins;
  try {
    plugins = execFileSync('pkg-config', ['--variable=pluginsdir', 'gstreamer-1.0'], {
      encoding: 'utf8', env, stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
  } catch (error) {
    fail(`cannot query GStreamer plugin directory from ${pkgConfigDir}; set LINGXI_GSTREAMER_SDK_MACOS to the SDK prefix (${error.message})`);
  }
  if (!plugins) fail(`pkg-config returned an empty GStreamer plugin directory for ${sdkRoot}`);
  const resolved = realpathSync(plugins);
  if (!isWithin(sdkRoot, resolved)) fail(`GStreamer plugin directory is outside configured SDK root: ${resolved}`);
  return resolved;
}

function sdkSearchDirectories(sdkRoot, pluginDirectory) {
  const directories = [join(sdkRoot, 'lib'), join(sdkRoot, 'lib', 'gstreamer-1.0'), pluginDirectory];
  const formulaRoot = join(sdkRoot, 'opt');
  if (existsSync(formulaRoot)) {
    for (const entry of readdirSync(formulaRoot)) {
      const prefix = join(formulaRoot, entry);
      for (const suffix of ['lib', 'lib/gstreamer-1.0']) {
        if (existsSync(join(prefix, suffix))) directories.push(join(prefix, suffix));
      }
    }
  }
  return [...new Set(directories.filter(existsSync).map((path) => realpathSync(path)))];
}

function addRpath(binary, rpath) {
  if (!rpaths(binary).includes(rpath)) execFileSync('/usr/bin/install_name_tool', ['-add_rpath', rpath, binary]);
}

function rebaseBinary(binary, libraryDirectory, pluginDirectory, executablePath, replacements, isExecutable = false) {
  for (const oldPath of rpaths(binary)) {
    execFileSync('/usr/bin/install_name_tool', ['-delete_rpath', oldPath, binary]);
  }
  for (const [oldPath, newPath] of replacements) {
    execFileSync('/usr/bin/install_name_tool', ['-change', oldPath, newPath, binary]);
  }
  if (isExecutable) {
    addRpath(binary, '@executable_path/../../Frameworks/GStreamerLibraries');
    return;
  }
  execFileSync('/usr/bin/install_name_tool', ['-id', `@rpath/${basename(binary)}`, binary]);
  addRpath(binary, pluginDirectory ? '@loader_path/../GStreamerLibraries' : '@loader_path');
}

function findGStreamerReferences(sidecar) {
  return dylibDependencies(sidecar).filter((dependency) => /(?:gstreamer|gst[a-z]|gst-)/i.test(dependency));
}

/** Copy and rebase the exact dynamic GStreamer runtime used by the sidecar. */
export function prepareMacGStreamerRuntime(sidecar, frameworksDirectory, expectedArch) {
  const references = findGStreamerReferences(sidecar);
  if (references.length === 0) {
    rmSync(join(frameworksDirectory, 'GStreamerLibraries'), { recursive: true, force: true });
    rmSync(join(frameworksDirectory, 'GStreamerPlugins'), { recursive: true, force: true });
    return { bundled: false, pluginDirectory: null, libraries: [] };
  }
  const configured = process.env.LINGXI_GSTREAMER_SDK_MACOS?.trim();
  if (!configured) {
    fail('bridge-server links dynamic GStreamer; set LINGXI_GSTREAMER_SDK_MACOS to the SDK prefix used for the build so its dylibs and plugins can be bundled');
  }
  const sdkRoot = realpathSync(resolve(configured));
  const pluginSource = findPluginDirectory(sdkRoot);
  const searchDirs = sdkSearchDirectories(sdkRoot, pluginSource);
  const libraryDirectory = join(frameworksDirectory, 'GStreamerLibraries');
  const pluginDirectory = join(frameworksDirectory, 'GStreamerPlugins');
  rmSync(libraryDirectory, { recursive: true, force: true });
  rmSync(pluginDirectory, { recursive: true, force: true });
  mkdirSync(libraryDirectory, { recursive: true });
  mkdirSync(pluginDirectory, { recursive: true });

  const sourceByDestination = new Map();
  const destinationNames = new Map();
  const queue = [];
  for (const name of REQUIRED_PLUGINS) {
    const source = join(pluginSource, name);
    if (!existsSync(source)) fail(`configured GStreamer SDK is missing required plugin ${source}`);
    const architectures = machOArchitectures(source);
    if (!architectures.includes(expectedArch)) fail(`GStreamer plugin ${name} has architectures ${architectures.join(', ')}, expected ${expectedArch}`);
    queue.push({ source: realpathSync(source), destination: join(pluginDirectory, name), plugin: true });
  }
  for (const dependency of dylibDependencies(sidecar)) {
    const source = dependencySource(dependency, sidecar, sidecar, sdkRoot, searchDirs);
    if (source) queue.push({ source, destination: join(libraryDirectory, basename(source)), plugin: false });
  }

  const replacementsByDestination = new Map();
  const processedDestinations = new Set();
  while (queue.length > 0) {
    const item = queue.shift();
    const source = realpathSync(item.source);
    const destination = resolve(item.destination);
    if (processedDestinations.has(destination)) continue;
    const previousSource = sourceByDestination.get(destination);
    if (previousSource && previousSource !== source) {
      fail(`GStreamer runtime destination would contain conflicting sources: ${destination}`);
    }
    processedDestinations.add(destination);
    sourceByDestination.set(destination, source);
    if (!item.plugin) {
      const name = basename(source);
      const previous = destinationNames.get(name);
      if (previous && previous !== source) fail(`GStreamer SDK contains conflicting dylib names: ${previous} and ${source}`);
      destinationNames.set(name, source);
    }
    const archs = machOArchitectures(source);
    if (!archs.includes(expectedArch)) fail(`GStreamer binary ${source} lacks ${expectedArch}: ${archs.join(', ')}`);
    const replacementRows = [];
    for (const dependency of dylibDependencies(source)) {
      const dependencyBinary = dependencySource(dependency, source, sidecar, sdkRoot, searchDirs);
      if (!dependencyBinary) continue;
      const name = basename(dependencyBinary);
      const previous = destinationNames.get(name);
      if (previous && previous !== dependencyBinary) fail(`GStreamer SDK contains conflicting dylib names: ${previous} and ${dependencyBinary}`);
      destinationNames.set(name, dependencyBinary);
      const dependencyDestination = join(libraryDirectory, name);
      replacementRows.push([dependency, `@rpath/${name}`]);
      queue.push({ source: dependencyBinary, destination: dependencyDestination, plugin: false });
    }
    if (!item.plugin && !existsSync(destination)) copyFileSync(source, destination);
    replacementsByDestination.set(destination, {
      ...item,
      source,
      destination,
      replacements: replacementRows,
    });
  }

  const sidecarReplacements = [];
  for (const dependency of dylibDependencies(sidecar)) {
    const source = dependencySource(dependency, sidecar, sidecar, sdkRoot, searchDirs);
    if (source) sidecarReplacements.push([dependency, `@rpath/${basename(source)}`]);
  }
  for (const item of replacementsByDestination.values()) {
    const destination = item.destination;
    if (item.plugin) {
      // Plugins live beside the library bundle so Rust can load them by one
      // directory path while their dependencies resolve through this rpath.
      copyFileSync(item.source, destination);
      rebaseBinary(destination, libraryDirectory, pluginDirectory, sidecar, item.replacements);
    } else {
      rebaseBinary(destination, libraryDirectory, null, sidecar, item.replacements);
    }
  }
  rebaseBinary(sidecar, libraryDirectory, null, sidecar, sidecarReplacements, true);
  return {
    bundled: true,
    pluginDirectory,
    libraries: [...replacementsByDestination.values()]
      .filter((item) => !item.plugin)
      .map((item) => item.destination),
    plugins: REQUIRED_PLUGINS.map((name) => join(pluginDirectory, name)),
  };
}

/** Reject packaged media binaries that still depend on a build-host path. */
export function verifyMacGStreamerRuntime(frameworksDirectory, sidecar) {
  const pluginDirectory = join(frameworksDirectory, 'GStreamerPlugins');
  const libraryDirectory = join(frameworksDirectory, 'GStreamerLibraries');
  if (!existsSync(pluginDirectory) && findGStreamerReferences(sidecar).length === 0) return { bundled: false };
  for (const name of REQUIRED_PLUGINS) {
    const plugin = join(pluginDirectory, name);
    if (!existsSync(plugin)) fail(`packaged GStreamer plugin is missing: ${plugin}`);
  }
  if (!existsSync(libraryDirectory)) fail('packaged GStreamer libraries are missing');
  const binaries = [sidecar, ...REQUIRED_PLUGINS.map((name) => join(pluginDirectory, name)),
    ...readdirSync(libraryDirectory).filter((name) => name.endsWith('.dylib')).map((name) => join(libraryDirectory, name))];
  for (const binary of binaries) {
    for (const dependency of dylibDependencies(binary)) {
      if (!isSystemDependency(dependency) && !dependency.startsWith('@rpath/')) {
        fail(`packaged GStreamer binary retains an external load path: ${binary} -> ${dependency}`);
      }
      if (dependency.startsWith('@rpath/') && !existsSync(join(libraryDirectory, dependency.slice('@rpath/'.length)))) {
        fail(`packaged GStreamer dependency is missing: ${binary} -> ${dependency}`);
      }
    }
    const binaryRpaths = rpaths(binary);
    if (binaryRpaths.some((path) => path.startsWith('/'))) fail(`packaged GStreamer binary retains an absolute LC_RPATH: ${binary}`);
    const requiredRpath = binary === sidecar
      ? '@executable_path/../../Frameworks/GStreamerLibraries'
      : binary.startsWith(`${pluginDirectory}${sep}`)
        ? '@loader_path/../GStreamerLibraries'
        : '@loader_path';
    if (!binaryRpaths.includes(requiredRpath)) fail(`packaged GStreamer binary is missing its bundle-relative rpath ${requiredRpath}: ${binary}`);
  }
  return { bundled: true, pluginDirectory, libraryDirectory };
}

export function signMacGStreamerRuntime(frameworksDirectory, identity = '-') {
  const paths = [
    join(frameworksDirectory, 'GStreamerLibraries'),
    join(frameworksDirectory, 'GStreamerPlugins'),
  ];
  const binaries = paths.flatMap((directory) => existsSync(directory)
    ? readdirSync(directory).map((name) => join(directory, name)).filter((path) => existsSync(path))
    : []);
  for (const binary of binaries.sort((left, right) => right.length - left.length || left.localeCompare(right))) {
    execFileSync('/usr/bin/codesign', [
      '--force', '--sign', identity, ...(identity === '-' ? ['--timestamp=none'] : ['--timestamp']),
      '--options', 'runtime', binary,
    ], { stdio: 'inherit' });
  }
}
