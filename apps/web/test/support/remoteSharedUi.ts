import { createRequire, registerHooks } from 'node:module';
import { pathToFileURL } from 'node:url';

const uiRoot = new URL('../../../../packages/', import.meta.url).href;
const webRequire = createRequire(import.meta.url);

// Match the Web bundler's React deduplication for UI source outside apps/web.
registerHooks({
  resolve(specifier, context, nextResolve) {
    if (context.parentURL?.startsWith(uiRoot) && /^react(?:-dom)?(?:\/|$)/u.test(specifier)) {
      return { url: pathToFileURL(webRequire.resolve(specifier)).href, shortCircuit: true };
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url === new URL('chat-ui/composer-surface.css', uiRoot).href) {
      return { format: 'commonjs', source: 'module.exports = {};', shortCircuit: true };
    }
    return nextLoad(url, context);
  },
});
