/** Targets for the cross-platform Electron Local Apps fixture. */
export function desktopFixtureTargets(nodePlatform: NodeJS.Platform) {
  const platform = nodePlatform === 'darwin' ? 'macos' : nodePlatform === 'win32' ? 'windows' : 'linux';
  return {
    native: [{ platform, environment: 'native' }],
    incompatible: [{ platform: platform === 'macos' ? 'linux' : 'macos', environment: 'native' }],
  };
}
