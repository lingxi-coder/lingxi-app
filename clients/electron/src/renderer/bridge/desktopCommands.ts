/**
 * The desktop's local slash commands.
 *
 * Every name here is one the ENGINE already knows — the desktop only
 * intercepts it because it owns a better surface than the headless fallback.
 * The reconciliation test (`test/slash-registry-reconciliation.test.ts`) is
 * what keeps this table honest against the engine's registry.
 */
import type { DesktopCommand, DesktopCommandContext } from './slashDispatch';

const PERMISSION_MODE_IDS = ['default', 'acceptEdits', 'plan', 'auto', 'dontAsk', 'bypassPermissions'] as const;

export const DESKTOP_COMMANDS: readonly DesktopCommand[] = [
  {
    name: 'model',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.openModelPicker('model');
      if (!ctx.knownModel(args)) return ctx.emit(`Unknown model: ${args}`, true);
      await ctx.setModel(args);
    },
  },
  {
    name: 'permissions',
    aliases: ['allowed-tools'],
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.openPermissionPicker();
      const mode = PERMISSION_MODE_IDS.find((id) => id === args);
      if (!mode) return ctx.emit(`Unknown permission mode: ${args}. Valid modes: ${PERMISSION_MODE_IDS.join(', ')}`, true);
      await ctx.setPermissionMode(mode);
    },
  },
  {
    name: 'effort',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.openModelPicker('effort');
      if (args === 'auto') return ctx.setReasoningAutomatic();
      if (args === 'off') return ctx.setReasoningDisabled();
      await ctx.setReasoningLevel(args);
    },
  },
  {
    name: 'fast',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.setFastMode(!ctx.fastMode());
      if (args === 'on') return ctx.setFastMode(true);
      if (args === 'off') return ctx.setFastMode(false);
      ctx.emit(`/fast takes on or off, not: ${args}`, true);
    },
  },
  {
    name: 'theme',
    args: 'optional',
    run(args, ctx) {
      if (!args) return ctx.openSettings();
      if (args === 'dark' || args === 'light') return ctx.setTheme(args);
      ctx.emit(`/theme takes dark or light, not: ${args}`, true);
    },
  },
  {
    name: 'config',
    aliases: ['settings'],
    args: 'optional',
    run(args, ctx) {
      if (args) return ctx.emit('/config takes no arguments', true);
      ctx.openSettings();
    },
  },
];

export type { DesktopCommandContext };
