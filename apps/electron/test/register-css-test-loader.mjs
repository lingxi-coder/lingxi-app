import { register } from 'node:module';

// Node renderer tests can import components with side-effect stylesheets, but
// only the browser applies CSS. Register before loading the test modules.
register(new URL('./css-test-load-hook.mjs', import.meta.url));
