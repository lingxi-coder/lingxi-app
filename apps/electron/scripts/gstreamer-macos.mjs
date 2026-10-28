import {
  copyFileSync,
  closeSync,
  constants,
  existsSync,
  fchmodSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  openSync,
  readSync,
  readdirSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  writeSync,
} from 'node:fs';
import { basename, dirname, join, parse, posix, relative, resolve, sep } from 'node:path';
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

class UnresolvedDependency extends Error {}

function isWithin(parent, candidate) {
  const path = relative(parent, candidate);
  return path === '' || (path !== '..' && !path.startsWith(`..${sep}`) && !path.startsWith(sep));
}

function canonicalFuturePath(path) {
  let current = resolve(path);
  const missing = [];
  while (true) {
    try {
      lstatSync(current);
      return resolve(realpathSync(current), ...missing.reverse());
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
      // A dangling symlink must not be treated as a missing ordinary path.
      try {
        if (lstatSync(current).isSymbolicLink()) fail(`package output path has a dangling symlink: ${current}`);
      } catch (inner) {
        if (inner.code !== 'ENOENT') throw inner;
      }
      const parent = dirname(current);
      if (parent === current) throw error;
      missing.push(basename(current));
      current = parent;
    }
  }
}

/** Refuse to reset or overwrite a path that contains the supplied sidecar. */
export function assertSidecarOutsideOutputs(source, outputPaths) {
  const lexicalSource = resolve(source);
  const sourceReal = realpathSync(source);
  for (const output of outputPaths) {
    if (isWithin(resolve(output), lexicalSource) || isWithin(canonicalFuturePath(output), sourceReal)) {
      fail(`release sidecar is inside a package output path that will be reset: ${source} -> ${output}`);
    }
  }
}

/** Check every existing component before a stage root is reset or written. */
export function assertSafeStageRoot(stageRoot, { allowMissing = false } = {}) {
  const root = resolve(stageRoot);
  const filesystemRoot = parse(root).root;
  let current = filesystemRoot;
  for (const segment of relative(filesystemRoot, root).split(sep).filter(Boolean)) {
    current = join(current, segment);
    let info;
    try {
      info = lstatSync(current);
    } catch (error) {
      if (allowMissing && error.code === 'ENOENT') return;
      throw error;
    }
    if (info.isSymbolicLink()) fail(`package stage path contains a symbolic link: ${current}`);
    if (!info.isDirectory()) fail(`package stage path is not a directory: ${current}`);
  }
}

function assertStagePath(stageRoot, path, { allowMissingLeaf = false } = {}) {
  const root = resolve(stageRoot);
  const target = resolve(path);
  if (!isWithin(root, target)) fail(`package stage path is outside its trusted root: ${path}`);
  assertSafeStageRoot(root);
  const rootReal = realpathSync(root);
  const segments = relative(root, target).split(sep).filter(Boolean);
  let current = root;
  for (const [index, segment] of segments.entries()) {
    current = join(current, segment);
    let info;
    try {
      info = lstatSync(current);
    } catch (error) {
      if (error.code === 'ENOENT' && allowMissingLeaf && index === segments.length - 1) return false;
      throw error;
    }
    if (info.isSymbolicLink()) fail(`package stage path contains a symbolic link: ${current}`);
    if (index < segments.length - 1 && !info.isDirectory()) fail(`package stage parent is not a directory: ${current}`);
    if (!isWithin(rootReal, realpathSync(current))) fail(`package stage path resolves outside its trusted root: ${current}`);
  }
  return true;
}

function ensureStageDirectory(stageRoot, directory) {
  const target = resolve(directory);
  const root = resolve(stageRoot);
  if (!isWithin(root, target)) fail(`package stage directory is outside its trusted root: ${directory}`);
  let current = root;
  assertStagePath(root, current);
  for (const segment of relative(root, target).split(sep).filter(Boolean)) {
    current = join(current, segment);
    if (!assertStagePath(root, current, { allowMissingLeaf: true })) mkdirSync(current);
    if (!lstatSync(current).isDirectory()) fail(`package stage path is not a directory: ${current}`);
  }
}

/** Copy a source sidecar only into an exclusive, checked staging path. */
export function stageMacGStreamerRuntime(source, destination, frameworksDirectory, expectedArch, stageRoot) {
  if (!stageRoot) fail('a trusted package stage root is required');
  assertStagePath(stageRoot, frameworksDirectory);
  if (!lstatSync(frameworksDirectory).isDirectory()) fail('package Frameworks is not a directory');
  for (const name of ['GStreamerLibraries', 'GStreamerPlugins']) {
    assertStagePath(stageRoot, join(frameworksDirectory, name), { allowMissingLeaf: true });
  }
  ensureStageDirectory(stageRoot, dirname(destination));
  if (assertStagePath(stageRoot, destination, { allowMissingLeaf: true })) {
    fail(`packaged sidecar destination already exists: ${destination}`);
  }
  const originalSidecar = realpathSync(source);
  const input = openSync(originalSidecar, constants.O_RDONLY);
  let output;
  try {
    output = openSync(destination, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o755);
    const buffer = Buffer.allocUnsafe(1024 * 1024);
    let count;
    while ((count = readSync(input, buffer, 0, buffer.length, null)) > 0) {
      let offset = 0;
      while (offset < count) offset += writeSync(output, buffer, offset, count - offset);
    }
    fchmodSync(output, 0o755);
  } finally {
    if (output !== undefined) closeSync(output);
    closeSync(input);
  }
  return prepareMacGStreamerRuntime(destination, frameworksDirectory, expectedArch, { stageRoot, sourceSidecar: originalSidecar });
}

function machOArchitectures(path) {
  return run('/usr/bin/lipo', ['-archs', path]).split(/\s+/).filter(Boolean);
}

function cachedMachOArchitectures(path, cache) {
  if (!cache.has(path)) cache.set(path, machOArchitectures(path));
  return cache.get(path);
}

function isSystemDependency(path) {
  return posix.normalize(path) === path && SYSTEM_PREFIXES.some((prefix) => path.startsWith(prefix));
}

function dylibDependencies(path, arch) {
  // Select the target slice: without -arch, universal binaries contain a
  // separate heading for each architecture, which is not a library path.
  const lines = run('/usr/bin/otool', ['-arch', arch, '-L', path]).split(/\r?\n/);
  if (lines.shift() !== `${path}:`) fail(`unexpected otool dependency header for ${path}`);
  const dependencies = lines.map((line) => {
    const match = line.match(/^\s+(.+) \(compatibility version [^,]+, current version [^)]+\)$/);
    if (!match) fail(`unexpected otool dependency entry for ${path}: ${line}`);
    return match[1];
  });
  // otool lists a dylib's own install ID as its first row. It is metadata,
  // not an LC_LOAD_DYLIB edge and need not resolve as a separate library.
  const ids = run('/usr/bin/otool', ['-arch', arch, '-D', path]).split(/\r?\n/);
  if (ids.shift() !== `${path}:` || ids.length > 1) fail(`unexpected otool install ID for ${path}`);
  if (ids.length === 1) {
    if (dependencies.shift() !== ids[0]) fail(`inconsistent otool install ID for ${path}`);
  }
  return [...new Set(dependencies)];
}

function rpaths(path, arch) {
  const output = run('/usr/bin/otool', ['-arch', arch, '-l', path]);
  return [...new Set([...output.matchAll(/cmd LC_RPATH\s+cmdsize \d+\s+path (.+?) \(offset \d+\)/g)].map((match) => match[1]))];
}

function expandRpath(value, binaryPath, executablePath) {
  if (value.startsWith('@loader_path/')) return resolve(dirname(binaryPath), value.slice('@loader_path/'.length));
  if (value === '@loader_path') return dirname(binaryPath);
  if (value.startsWith('@executable_path/')) return resolve(dirname(executablePath), value.slice('@executable_path/'.length));
  if (value === '@executable_path') return dirname(executablePath);
  return value;
}

function dependencySource(reference, binaryPath, executablePath, sdkRoot, runpathStack, expectedArch, architectureCache) {
  if (isSystemDependency(reference)) return null;
  const candidates = [];
  if (reference.startsWith('@loader_path/') || reference === '@loader_path'
      || reference.startsWith('@executable_path/') || reference === '@executable_path') {
    candidates.push(expandRpath(reference, binaryPath, executablePath));
  } else if (reference.startsWith('@rpath/')) {
    const suffix = reference.slice('@rpath/'.length);
    candidates.push(...runpathStack.map((directory) => join(directory, suffix)));
  } else if (reference.startsWith('/')) {
    candidates.push(reference);
  } else {
    fail(`GStreamer dependency is not anchored to the executable, loader, or rpath: ${reference} required by ${binaryPath}`);
  }
  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      const source = realpathSync(candidate);
      // dyld continues searching when an existing image has no compatible
      // architecture; do not mistake that image for the loaded dependency.
      if (!cachedMachOArchitectures(source, architectureCache).includes(expectedArch)) continue;
      if (!isWithin(sdkRoot, source)) {
        fail(`GStreamer dependency resolves outside LINGXI_GSTREAMER_SDK_MACOS: ${reference} -> ${source}`);
      }
      return source;
    }
  }
  throw new UnresolvedDependency(`cannot resolve GStreamer dependency ${reference} required by ${binaryPath} within ${sdkRoot}`);
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

function addRpath(binary, rpath, arch) {
  if (!rpaths(binary, arch).includes(rpath)) execFileSync('/usr/bin/install_name_tool', ['-add_rpath', rpath, binary]);
}

function rebaseBinary(binary, pluginDirectory, replacements, arch, isExecutable = false) {
  for (const oldPath of rpaths(binary, arch)) {
    execFileSync('/usr/bin/install_name_tool', ['-delete_rpath', oldPath, binary]);
  }
  for (const [oldPath, newPath] of replacements) {
    execFileSync('/usr/bin/install_name_tool', ['-change', oldPath, newPath, binary]);
  }
  if (isExecutable) {
    addRpath(binary, '@executable_path/../../Frameworks/GStreamerLibraries', arch);
    return;
  }
  execFileSync('/usr/bin/install_name_tool', ['-id', `@rpath/${basename(binary)}`, binary]);
  addRpath(binary, pluginDirectory ? '@loader_path/../GStreamerLibraries' : '@loader_path', arch);
}

function findGStreamerReferences(dependencies) {
  return dependencies.filter((dependency) => /(?:gstreamer|gst[a-z]|gst-)/i.test(dependency));
}

function assertSystemOnlySidecar(dependencies, sidecar) {
  const external = dependencies.find((dependency) => !isSystemDependency(dependency));
  if (external) fail(`sidecar without a bundled GStreamer runtime has an unbundled non-system dependency: ${sidecar} -> ${external}`);
}

// Package only the requested native slice. Apart from reducing the artifact,
// this makes install_name_tool operate on one coherent dependency/rpath graph.
function copyMachOSlice(source, destination, expectedArch) {
  const architectures = machOArchitectures(source);
  if (!architectures.includes(expectedArch)) {
    fail(`GStreamer binary ${source} lacks ${expectedArch}: ${architectures.join(', ')}`);
  }
  if (architectures.length === 1) {
    if (source !== destination) copyFileSync(source, destination);
    return;
  }
  const temporaryDirectory = mkdtempSync(join(dirname(destination), '.lingxi-macho-'));
  try {
    const thin = join(temporaryDirectory, 'slice');
    execFileSync('/usr/bin/lipo', [source, '-thin', expectedArch, '-output', thin]);
    renameSync(thin, destination);
  } finally {
    rmSync(temporaryDirectory, { recursive: true, force: true });
  }
}

/** Copy and rebase the exact dynamic GStreamer runtime used by the sidecar. */
export function prepareMacGStreamerRuntime(sidecar, frameworksDirectory, expectedArch, { stageRoot, sourceSidecar = sidecar } = {}) {
  if (!['arm64', 'x86_64'].includes(expectedArch)) fail(`unsupported macOS architecture: ${expectedArch}`);
  if (!stageRoot) fail('a trusted package stage root is required');
  assertStagePath(stageRoot, sidecar);
  assertStagePath(stageRoot, frameworksDirectory);
  if (!lstatSync(frameworksDirectory).isDirectory()) fail('package Frameworks is not a directory');
  for (const name of ['GStreamerLibraries', 'GStreamerPlugins']) {
    assertStagePath(stageRoot, join(frameworksDirectory, name), { allowMissingLeaf: true });
  }
  const originalSidecar = realpathSync(sourceSidecar);
  if (sourceSidecar !== sidecar) {
    const sourceInfo = statSync(originalSidecar, { bigint: true });
    const stageInfo = statSync(sidecar, { bigint: true });
    if (sourceInfo.dev === stageInfo.dev && sourceInfo.ino === stageInfo.ino) {
      fail('packaged sidecar must not alias the source executable');
    }
  }
  copyMachOSlice(sidecar, sidecar, expectedArch);
  // Inspect the copied bytes, while resolving executable/loader-relative
  // paths from the original location. The source may change after copying.
  const stagedSidecar = realpathSync(sidecar);
  const sidecarDependencies = dylibDependencies(stagedSidecar, expectedArch);
  const references = findGStreamerReferences(sidecarDependencies);
  if (references.length === 0) {
    assertSystemOnlySidecar(sidecarDependencies, stagedSidecar);
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
  const libraryDirectory = join(frameworksDirectory, 'GStreamerLibraries');
  const pluginDirectory = join(frameworksDirectory, 'GStreamerPlugins');
  rmSync(libraryDirectory, { recursive: true, force: true });
  rmSync(pluginDirectory, { recursive: true, force: true });
  mkdirSync(libraryDirectory, { recursive: true });
  mkdirSync(pluginDirectory, { recursive: true });

  const sourceByDestination = new Map();
  const destinationNames = new Map();
  const architectureCache = new Map();
  const plugins = [];
  for (const name of REQUIRED_PLUGINS) {
    const source = join(pluginSource, name);
    if (!existsSync(source)) fail(`configured GStreamer SDK is missing required plugin ${source}`);
    const resolvedSource = realpathSync(source);
    if (!isWithin(sdkRoot, resolvedSource)) fail(`GStreamer plugin resolves outside configured SDK root: ${source} -> ${resolvedSource}`);
    const architectures = cachedMachOArchitectures(resolvedSource, architectureCache);
    if (!architectures.includes(expectedArch)) fail(`GStreamer plugin ${name} has architectures ${architectures.join(', ')}, expected ${expectedArch}`);
    plugins.push({ source: resolvedSource, destination: join(pluginDirectory, name), plugin: true });
  }

  const queue = [{ source: stagedSidecar, destination: sidecar, sidecar: true, ancestors: [] }];
  const metadata = new Map();
  const replacementsByDestination = new Map();
  const processedStates = new Set();
  const dependencySources = new Map();
  const potentialCallerRpaths = new Set();
  const pluginEdges = [];
  let next = 0;
  while (next < queue.length) {
    const item = queue[next++];
    const source = realpathSync(item.source);
    const destination = resolve(item.destination);
    const previousSource = sourceByDestination.get(destination);
    if (previousSource && previousSource !== source) {
      fail(`GStreamer runtime destination would contain conflicting sources: ${destination}`);
    }
    sourceByDestination.set(destination, source);
    if (!item.plugin && !item.sidecar) {
      const name = basename(source);
      const previous = destinationNames.get(name);
      if (previous && previous !== source) fail(`GStreamer SDK contains conflicting dylib names: ${previous} and ${source}`);
      destinationNames.set(name, source);
    }
    let info = metadata.get(source);
    if (!info) {
      const archs = cachedMachOArchitectures(source, architectureCache);
      if (!archs.includes(expectedArch)) fail(`GStreamer binary ${source} lacks ${expectedArch}: ${archs.join(', ')}`);
      const loaderPath = item.sidecar ? originalSidecar : source;
      const ownRpaths = rpaths(source, expectedArch).map((path) => {
        const expanded = expandRpath(path, loaderPath, originalSidecar);
        if (!expanded.startsWith('/')) fail(`unsupported GStreamer LC_RPATH ${path} in ${source}`);
        return resolve(expanded);
      });
      info = { ownRpaths, dependencies: dylibDependencies(source, expectedArch) };
      metadata.set(source, info);
    }
    // dyld searches the immediate loader's runpaths first, then each
    // ancestor's. Preserve that order and the context for every graph edge.
    const stack = [...new Set([...info.ownRpaths, ...item.ancestors])];
    const pluginPrefix = item.runtimePlugin
      ? [...new Set([...info.ownRpaths, ...(item.pluginPrefix ?? [])])]
      : [];
    if (!item.sidecar) for (const path of info.ownRpaths) potentialCallerRpaths.add(path);
    // Dependency resolution depends on this runpath stack, not on the
    // sequence of images that happened to produce it. Deduplicating the
    // complete resolution state avoids enumerating exponentially many paths
    // through a shared dependency DAG.
    const state = JSON.stringify([source, destination, stack, Boolean(item.runtimePlugin), pluginPrefix]);
    if (processedStates.has(state)) continue;
    if (processedStates.size >= 1024) fail('GStreamer dependency graph has too many runpath states');
    processedStates.add(state);
    const replacementRows = [];
    for (const dependency of info.dependencies) {
      const edge = JSON.stringify([destination, dependency]);
      let dependencyBinary;
      try {
        dependencyBinary = dependencySource(dependency, item.sidecar ? originalSidecar : source, originalSidecar, sdkRoot, stack, expectedArch, architectureCache);
      } catch (error) {
        // Static Mach-O metadata does not prove which loaded image calls
        // dlopen(). A missing edge in any reachable plugin caller context
        // therefore fails that whole context; it cannot be supplied by a
        // different caller's runpath.
        if (item.runtimePlugin && error instanceof UnresolvedDependency) {
          throw new Error(`cannot prove a complete plugin load context: ${error.message}`);
        }
        throw error;
      }
      if (!dependencyBinary) continue;
      const prior = dependencySources.get(edge);
      if (prior && prior !== dependencyBinary) {
        fail(`GStreamer dependency resolves to different sources under different loader paths: ${dependency} -> ${prior} or ${dependencyBinary}`);
      }
      dependencySources.set(edge, dependencyBinary);
      if (item.runtimePlugin && dependency.startsWith('@rpath/')) {
        pluginEdges.push({ dependency, source, expected: dependencyBinary, pluginPrefix });
      }
      const name = basename(dependencyBinary);
      const previous = destinationNames.get(name);
      if (previous && previous !== dependencyBinary) fail(`GStreamer SDK contains conflicting dylib names: ${previous} and ${dependencyBinary}`);
      destinationNames.set(name, dependencyBinary);
      const dependencyDestination = join(libraryDirectory, name);
      replacementRows.push([dependency, `@rpath/${name}`]);
      // Revisiting an image can only add conservative contexts. A cycle
      // whose runpaths have stopped changing reaches an already-seen state;
      // differing states are still checked and the global bound is retained.
      queue.push({ source: dependencyBinary, destination: dependencyDestination, ancestors: stack,
        runtimePlugin: item.runtimePlugin, pluginPrefix });
    }
    if (!item.sidecar && !item.plugin && !existsSync(destination)) copyMachOSlice(source, destination, expectedArch);
    const existing = replacementsByDestination.get(destination);
    if (!existing) replacementsByDestination.set(destination, { ...item, source, destination, replacements: replacementRows });
    else for (const row of replacementRows) {
      if (!existing.replacements.some(([old]) => old === row[0])) existing.replacements.push(row);
    }
    // First prove each plugin's whole closure with only its own load chain
    // and the executable's runpaths. The actual dlopen caller is unknown.
    if (item.sidecar) for (const plugin of plugins) {
      queue.push({ ...plugin, ancestors: stack, runtimePlugin: true, pluginPrefix: [] });
    }
  }
  for (const edge of pluginEdges) {
    // If the plugin's own chain resolves the edge, later caller runpaths can
    // never change it. Otherwise any already-loaded image may prepend its
    // own runpath before the executable's, so check every such directory.
    try {
      dependencySource(edge.dependency, edge.source, originalSidecar, sdkRoot, edge.pluginPrefix, expectedArch, architectureCache);
      continue;
    } catch (error) {
      if (!(error instanceof UnresolvedDependency)) throw error;
    }
    for (const path of potentialCallerRpaths) {
      let candidate;
      try {
        candidate = dependencySource(edge.dependency, edge.source, originalSidecar, sdkRoot, [path], expectedArch, architectureCache);
      } catch (error) {
        if (error instanceof UnresolvedDependency) continue;
        throw error;
      }
      if (candidate !== edge.expected) {
        fail(`GStreamer dependency resolves to different sources under different loader paths: ${edge.dependency} -> ${edge.expected} or ${candidate}`);
      }
    }
  }
  for (const item of replacementsByDestination.values()) {
    const destination = item.destination;
    if (item.sidecar) {
      rebaseBinary(sidecar, null, item.replacements, expectedArch, true);
    } else if (item.plugin) {
      // Plugins live beside the library bundle so Rust can load them by one
      // directory path while their dependencies resolve through this rpath.
      copyMachOSlice(item.source, destination, expectedArch);
      rebaseBinary(destination, pluginDirectory, item.replacements, expectedArch);
    } else {
      rebaseBinary(destination, null, item.replacements, expectedArch);
    }
  }
  return {
    bundled: true,
    pluginDirectory,
    libraries: [...replacementsByDestination.values()]
      .filter((item) => !item.plugin && !item.sidecar)
      .map((item) => item.destination),
    plugins: REQUIRED_PLUGINS.map((name) => join(pluginDirectory, name)),
  };
}

/** Verify the outer app and its nested code after signing. */
export function verifyMacAppSignature(appPath) {
  execFileSync('/usr/bin/codesign', ['--verify', '--deep', '--strict', appPath], { stdio: 'pipe' });
}

/** Reject packaged media binaries that still depend on a build-host path. */
export function verifyMacGStreamerRuntime(frameworksDirectory, sidecar, { expectedArch, appRoot } = {}) {
  const pluginDirectory = join(frameworksDirectory, 'GStreamerPlugins');
  const libraryDirectory = join(frameworksDirectory, 'GStreamerLibraries');
  if (lstatSync(frameworksDirectory).isSymbolicLink()) fail('packaged Frameworks directory is a symlink');
  if (appRoot) {
    if (lstatSync(appRoot).isSymbolicLink()) fail('packaged macOS application is a symlink');
    const root = realpathSync(appRoot);
    if (!isWithin(root, realpathSync(frameworksDirectory)) || !isWithin(root, realpathSync(sidecar))) {
      fail('packaged Frameworks or sidecar resolves outside the application');
    }
  }
  const sidecarArchs = machOArchitectures(sidecar);
  if (sidecarArchs.length !== 1 || !['arm64', 'x86_64'].includes(sidecarArchs[0])) {
    fail(`packaged macOS sidecar must have exactly one supported architecture: ${sidecarArchs.join(', ')}`);
  }
  const arch = sidecarArchs[0];
  if (expectedArch && expectedArch !== arch) fail(`packaged macOS sidecar has architecture ${arch}; expected ${expectedArch}`);
  const sidecarDependencies = dylibDependencies(sidecar, arch);
  if (!existsSync(pluginDirectory) && findGStreamerReferences(sidecarDependencies).length === 0) {
    if (existsSync(libraryDirectory)) fail('packaged GStreamer libraries exist without the plugin directory');
    assertSystemOnlySidecar(sidecarDependencies, sidecar);
    execFileSync('/usr/bin/codesign', ['--verify', '--strict', sidecar], { stdio: 'pipe' });
    return { bundled: false };
  }
  for (const name of REQUIRED_PLUGINS) {
    const plugin = join(pluginDirectory, name);
    if (!existsSync(plugin)) fail(`packaged GStreamer plugin is missing: ${plugin}`);
  }
  if (readdirSync(pluginDirectory).some((name) => !REQUIRED_PLUGINS.includes(name))) {
    fail('packaged GStreamer plugin directory contains unexpected entries');
  }
  if (!existsSync(libraryDirectory)) fail('packaged GStreamer libraries are missing');
  const binaries = [sidecar, ...REQUIRED_PLUGINS.map((name) => join(pluginDirectory, name)),
    ...readdirSync(libraryDirectory).map((name) => join(libraryDirectory, name))];
  const realFrameworksDirectory = realpathSync(frameworksDirectory);
  const realLibraryDirectory = realpathSync(libraryDirectory);
  const realPluginDirectory = realpathSync(pluginDirectory);
  if (!isWithin(realFrameworksDirectory, realLibraryDirectory) || !isWithin(realFrameworksDirectory, realPluginDirectory)) {
    fail('packaged GStreamer runtime directory resolves outside Frameworks');
  }
  for (const binary of binaries) {
    if (binary !== sidecar) {
      const expectedDirectory = binary.startsWith(`${pluginDirectory}${sep}`)
        ? realPluginDirectory : realLibraryDirectory;
      if (!isWithin(expectedDirectory, realpathSync(binary))) fail(`packaged GStreamer binary resolves outside its directory: ${binary}`);
    }
    const architectures = machOArchitectures(binary);
    if (architectures.length !== 1 || architectures[0] !== arch) {
      fail(`packaged GStreamer binary has unexpected architectures: ${binary} -> ${architectures.join(', ')}, expected ${arch}`);
    }
    for (const dependency of dylibDependencies(binary, arch)) {
      if (!isSystemDependency(dependency) && !dependency.startsWith('@rpath/')) {
        fail(`packaged GStreamer binary retains an external load path: ${binary} -> ${dependency}`);
      }
      if (dependency.startsWith('@rpath/')) {
        const name = dependency.slice('@rpath/'.length);
        if (!name || name !== basename(name) || name === '.' || name === '..') {
          fail(`packaged GStreamer dependency has an invalid library name: ${binary} -> ${dependency}`);
        }
        const path = join(libraryDirectory, name);
        if (!existsSync(path)) fail(`packaged GStreamer dependency is missing: ${binary} -> ${dependency}`);
        if (!isWithin(realLibraryDirectory, realpathSync(path))) {
          fail(`packaged GStreamer dependency resolves outside its library directory: ${binary} -> ${dependency}`);
        }
      }
    }
    const binaryRpaths = rpaths(binary, arch);
    const requiredRpath = binary === sidecar
      ? '@executable_path/../../Frameworks/GStreamerLibraries'
      : binary.startsWith(`${pluginDirectory}${sep}`)
        ? '@loader_path/../GStreamerLibraries'
        : '@loader_path';
    if (binaryRpaths.length !== 1 || binaryRpaths[0] !== requiredRpath) {
      fail(`packaged GStreamer binary has unexpected LC_RPATH; required ${requiredRpath}: ${binary} -> ${binaryRpaths.join(', ')}`);
    }
  }
  // Verify each actual loadable image; verification of the outer app alone is
  // not a substitute for validating every nested Mach-O signature.
  for (const binary of binaries) execFileSync('/usr/bin/codesign', ['--verify', '--strict', binary], { stdio: 'pipe' });
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
