const remoteSource = new URL('../src/remote.ts', import.meta.url).href;

export function resolve(specifier, context, nextResolve) {
  if (specifier === '@lingxi/bridge-client/remote') return { url: remoteSource, shortCircuit: true };
  return nextResolve(specifier, context);
}
