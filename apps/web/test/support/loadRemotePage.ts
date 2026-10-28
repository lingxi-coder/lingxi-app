import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { JsxEmit, ModuleKind, ScriptTarget, transpileModule } from 'typescript';
import type { ReactElement } from 'react';

/**
 * The routing component of the remote page, `src/pages/RemotePage.tsx` as it
 * is, with the modules it imports replaced where `modules` says so (by the
 * specifier the page writes): the hooks that need Firebase, the views, and
 * `navigateTo`. Everything else is the real module.
 */
export function loadRemotePage(modules: Record<string, unknown>): (props: Record<string, unknown>) => ReactElement {
  const require = createRequire(import.meta.url);
  const transpiled = transpileModule(readFileSync(new URL('../../src/pages/RemotePage.tsx', import.meta.url), 'utf8'), {
    compilerOptions: { jsx: JsxEmit.ReactJSX, module: ModuleKind.CommonJS, target: ScriptTarget.ES2020 },
  }).outputText;
  const module = { exports: {} as { default: (props: Record<string, unknown>) => ReactElement } };
  const substitute = (specifier: string): unknown => {
    if (specifier in modules) return modules[specifier];
    if (specifier === '../remote/deviceKey') return { browserDeviceName: () => 'Test Browser' };
    if (specifier === '../remote/remote.css') return {};
    if (specifier === '../app/router') return require('../../src/app/router.ts');
    if (specifier === '../remote/strings') return require('../../src/remote/strings.ts');
    return require(specifier);
  };
  new Function('require', 'module', 'exports', transpiled)(substitute, module, module.exports);
  return module.exports.default;
}
