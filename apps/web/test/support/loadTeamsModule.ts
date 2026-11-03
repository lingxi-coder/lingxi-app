import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { JsxEmit, ModuleKind, ScriptTarget, transpileModule } from 'typescript';

const packageRoot = fileURLToPath(new URL('../../../../packages/teams/', import.meta.url));
const webRequire = createRequire(import.meta.url);
const loaded = new Map<string, Record<string, unknown>>();

function locate(base: string): string | undefined {
  const bare = base.replace(/\.js$/u, '');
  return [`${bare}.ts`, `${bare}.tsx`, `${bare}/index.ts`, `${bare}/index.tsx`].find(existsSync);
}

/**
 * Load a source file of `packages/teams` (a `.tsx` component and the sibling modules it imports)
 * the way the other Teams UI tests do: shared packages have no React install of their own, so their
 * bare imports (`react`, `react/jsx-runtime`) resolve through this Web package, as Vite does.
 * `name` is the module's path under `packages/teams`, with or without an extension.
 */
export function loadTeamsModule<T>(name: string): T {
  return load(locate(resolve(packageRoot, name)) ?? resolve(packageRoot, name)) as T;
}

function load(path: string): Record<string, unknown> {
  const known = loaded.get(path);
  if (known) return known;
  const compiled = transpileModule(readFileSync(path, 'utf8'), { fileName: path, compilerOptions: { module: ModuleKind.CommonJS, target: ScriptTarget.ES2020, jsx: JsxEmit.ReactJSX, esModuleInterop: true } });
  const module = { exports: {} as Record<string, unknown> };
  loaded.set(path, module.exports);
  const require = (id: string): unknown => {
    if (!id.startsWith('.')) return webRequire(id);
    const target = locate(resolve(dirname(path), id));
    if (!target) throw new Error(`Cannot resolve ${id} from ${path}`);
    return load(target);
  };
  new Function('require', 'module', 'exports', compiled.outputText)(require, module, module.exports);
  loaded.set(path, module.exports);
  return module.exports;
}
