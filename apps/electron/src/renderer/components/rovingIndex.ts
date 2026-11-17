/**
 * Where roving focus goes for a key press in a list or a tab strip: the next or the previous item (wrapping only when
 * asked to), Home and End. `undefined` when the key does not move focus, or there is nothing to move to.
 *
 * It is the one rule of `packages/teams/roving.ts`, which the Teams and Friends sidebars and the details drawer use, so the
 * Bots sidebar and the Bot's tabs answer the arrow keys as they do. That file imports no React, so the renderer tests that
 * load these components can resolve it (they cannot resolve React from outside this package).
 */
export { rovingIndex } from '../../../../../packages/teams/roving';
