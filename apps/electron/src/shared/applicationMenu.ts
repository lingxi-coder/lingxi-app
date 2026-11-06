export const CH_APPLICATION_MENU = 'lingxi:application-menu';
/**
 * What a menu item asks of the window. `home`, `apps`, `bots` and `teams` pick what the
 * sidebar lists, as the rail's buttons do; `toggle-sidebar` shows or hides it;
 * `team-details` shows or hides the details panel of the open team, in Teams mode.
 */
export type ApplicationMenuAction = 'settings' | 'activity' | 'open-link' | 'home' | 'bots' | 'teams' | 'apps' | 'toggle-sidebar' | 'team-details';
