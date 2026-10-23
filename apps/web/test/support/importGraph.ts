import { builtinModules } from 'node:module';
import { existsSync, readFileSync, realpathSync } from 'node:fs';
import path from 'node:path';
import ts from 'typescript';

/**
 * Walks the runtime imports of some entry files and reports which packages they
 * reach by bare name, so the web app's `package.json` can be checked against what
 * the bundle really needs.
 *
 * The check is by *name*, never by whether the import resolves. In a checkout
 * where `node_modules` are symlinks into another tree, a file outside
 * `apps/web` (the shared reducer, the bridge client) can resolve a package that
 * `apps/web` never declared, and the build would pass here and fail in CI.
 */

const ASSET = /\.(?:css|png|jpe?g|gif|svg|webp|ico|woff2?)$/iu;
const BUILTINS = new Set([...builtinModules, ...builtinModules.map((name) => `node:${name}`)]);

export interface BareImport {
  /** The package name, e.g. `react` or `@scope/name`. */
  pkg: string;
  specifier: string;
  /** The file that imports it. */
  from: string;
  builtin: boolean;
  /** Where the package resolved from that file, if it did (outside `apps/web` means masking). */
  resolvedTo?: string;
}

export interface ImportGraph {
  files: Set<string>;
  bare: BareImport[];
}

export function packageNameOf(specifier: string): string {
  const parts = specifier.split('/');
  return specifier.startsWith('@') ? parts.slice(0, 2).join('/') : parts[0]!;
}

function compilerOptionsFor(webRoot: string): ts.CompilerOptions {
  const configPath = path.join(webRoot, 'tsconfig.json');
  if (!existsSync(configPath)) return { moduleResolution: ts.ModuleResolutionKind.Bundler, module: ts.ModuleKind.ESNext, allowImportingTsExtensions: true };
  const read = ts.readConfigFile(configPath, ts.sys.readFile);
  return ts.parseJsonConfigFileContent(read.config, ts.sys, webRoot).options;
}

/** The module specifiers a file loads at run time: static, dynamic and `require`, without type-only ones. */
export function runtimeSpecifiers(fileName: string, text: string): string[] {
  const kind = /\.tsx$/u.test(fileName) ? ts.ScriptKind.TSX : /\.jsx$/u.test(fileName) ? ts.ScriptKind.JSX : /\.[cm]?js$/u.test(fileName) ? ts.ScriptKind.JS : ts.ScriptKind.TS;
  const source = ts.createSourceFile(fileName, text, ts.ScriptTarget.Latest, true, kind);
  const found: string[] = [];
  const visit = (node: ts.Node): void => {
    if (ts.isImportDeclaration(node) && ts.isStringLiteral(node.moduleSpecifier)) {
      const clause = node.importClause;
      const typeOnly = clause !== undefined && (clause.isTypeOnly
        || (clause.name === undefined && clause.namedBindings !== undefined && ts.isNamedImports(clause.namedBindings)
          && clause.namedBindings.elements.length > 0 && clause.namedBindings.elements.every((element) => element.isTypeOnly)));
      if (!typeOnly) found.push(node.moduleSpecifier.text);
    } else if (ts.isExportDeclaration(node) && node.moduleSpecifier !== undefined && ts.isStringLiteral(node.moduleSpecifier)) {
      if (!node.isTypeOnly) found.push(node.moduleSpecifier.text);
    } else if (ts.isCallExpression(node) && node.arguments.length >= 1 && ts.isStringLiteralLike(node.arguments[0]!)) {
      const callee = node.expression;
      if (callee.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(callee) && callee.text === 'require')) found.push((node.arguments[0] as ts.StringLiteralLike).text);
    }
    ts.forEachChild(node, visit);
  };
  visit(source);
  return found;
}

export function collectImportGraph(options: { entries: string[]; webRoot: string; compilerOptions?: ts.CompilerOptions }): ImportGraph {
  const compilerOptions = options.compilerOptions ?? compilerOptionsFor(options.webRoot);
  const files = new Set<string>();
  const bare: BareImport[] = [];
  const queue = options.entries.map((entry) => realpathSync(entry));
  for (let file = queue.pop(); file !== undefined; file = queue.pop()) {
    if (files.has(file)) continue;
    files.add(file);
    for (const specifier of runtimeSpecifiers(file, readFileSync(file, 'utf8'))) {
      if (ASSET.test(specifier)) continue;
      const relative = specifier.startsWith('.') || path.isAbsolute(specifier);
      const resolved = ts.resolveModuleName(specifier, file, compilerOptions, ts.sys).resolvedModule;
      // Something of this repository: follow it (relative imports and tsconfig `paths` aliases).
      if (resolved !== undefined && !resolved.isExternalLibraryImport && !resolved.resolvedFileName.includes(`${path.sep}node_modules${path.sep}`)) {
        queue.push(realpathSync(resolved.resolvedFileName));
        continue;
      }
      if (relative) continue; // an unresolvable relative import is a typecheck failure, not ours
      const pkg = packageNameOf(specifier);
      bare.push({
        pkg,
        specifier,
        from: file,
        builtin: BUILTINS.has(specifier) || BUILTINS.has(pkg),
        ...(resolved === undefined ? {} : { resolvedTo: resolved.resolvedFileName }),
      });
    }
  }
  return { files, bare };
}

/** Bare imports of the graph that `apps/web/package.json` does not declare (builtins are always a violation). */
export function undeclaredImports(graph: ImportGraph, webRoot: string): BareImport[] {
  const manifest = JSON.parse(readFileSync(path.join(webRoot, 'package.json'), 'utf8')) as {
    dependencies?: Record<string, string>;
    devDependencies?: Record<string, string>;
  };
  const declared = new Set([...Object.keys(manifest.dependencies ?? {}), ...Object.keys(manifest.devDependencies ?? {})]);
  return graph.bare.filter((entry) => entry.builtin || !declared.has(entry.pkg));
}
