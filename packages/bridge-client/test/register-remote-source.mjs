import { register } from 'node:module';

// Integration tests must exercise current remote source without rebuilding dist.
register(new URL('./remote-source-load-hook.mjs', import.meta.url));
