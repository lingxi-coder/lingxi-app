export const CH_APPLICATION_MENU = 'lingxi:application-menu';
/**
 * What a menu item asks of the window. `home`, `bots` and `teams` pick what the
 * sidebar lists, as the rail's buttons do; `toggle-sidebar` shows or hides it.
 */
export type ApplicationMenuAction = 'settings' | 'activity' | 'open-link' | 'home' | 'bots' | 'teams' | 'toggle-sidebar';
