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
    name: 'help',
    args: 'none',
    run(args, ctx) {
      if (args) return ctx.emit('/help takes no arguments', true);
      ctx.showHelp();
    },
  },
  {
    name: 'clear',
    aliases: ['reset', 'new'],
    args: 'optional',
    async run(args, ctx) {
      await ctx.clearSession(args || undefined);
    },
  },
  {
    name: 'compact',
    args: 'optional',
    async run(args, ctx) {
      await ctx.forceCompact(args || undefined);
    },
  },
  {
    name: 'login',
    args: 'none',
    async run(args, ctx) {
      if (args) return ctx.emit('/login takes no arguments', true);
      await ctx.login();
    },
  },
  {
    name: 'logout',
    args: 'none',
    async run(args, ctx) {
      if (args) return ctx.emit('/logout takes no arguments', true);
      await ctx.logout();
    },
  },
  {
    name: 'add-dir',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.openSettingsPage('permissions');
      await ctx.addWorkspaceDirectory(args);
      ctx.emit(`Added working directory: ${args}`);
    },
  },
  {
    name: 'cd',
    args: 'optional',
    async run(args, ctx) {
      if (!args) return ctx.chooseProject();
      await ctx.activateProject(args);
    },
  },
  {
    name: 'copy',
    args: 'none',
    async run(args, ctx) {
      if (args) return ctx.emit('/copy takes no arguments on Desktop', true);
      if (await ctx.copyLastResponse()) ctx.emit('Copied the last response to the clipboard.');
      else ctx.emit('There is no assistant response to copy yet.', true);
    },
  },
  {
    name: 'tasks',
    aliases: ['bashes'],
    args: 'none',
    async run(args, ctx) {
      if (args) return ctx.emit('/tasks takes no arguments on Desktop', true);
      await ctx.openTasks();
    },
  },
  {
    name: 'plugin',
    aliases: ['plugins', 'marketplace'],
    args: 'none',
    run(args, ctx) {
      if (args) return ctx.emit('/plugin arguments are managed in Desktop Settings', true);
      ctx.openSettingsPage('plugins');
    },
  },
  {
    name: 'reload-plugins',
    args: 'none',
    async run(args, ctx) {
      if (args) return ctx.emit('/reload-plugins takes no arguments', true);
      await ctx.reloadPlugins();
    },
  },
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

/** Builtins whose registry handlers are real on the bridge-server. */
export const DESKTOP_ENGINE_BUILTIN_COMMANDS = [
  'agents', 'auto-mode-setup', 'autocompact', 'brief', 'btw', 'commit',
  'commit-push-pr', 'connect', 'context', 'diff', 'doctor', 'export', 'files',
  'fork', 'fusion', 'goal', 'hooks', 'ide', 'init', 'init-verifiers', 'insights',
  'keybindings', 'mcp', 'memory', 'output-style', 'plan', 'powerup', 'recap',
  'release-notes', 'reload-skills', 'rename', 'resume', 'security-review', 'skill-doctor',
  'skills', 'status',
  'stickers', 'stop', 'subtask', 'usage', 'workflows', 'worktree',
] as const;

/**
 * Builtins with no honest Desktop implementation. They remain directly
 * resolvable so old prompts receive an explicit Desktop error, but are not
 * advertised in completion or the command palette.
 */
export const DESKTOP_UNAVAILABLE_BUILTIN_COMMANDS = {
  advisor: 'not available for ordinary external users',
  'autofix-pr': 'disabled upstream',
  background: 'Desktop sessions already continue independently of a terminal',
  branch: 'conversation branching has no Desktop backend yet',
  bug: 'Desktop has no bug-report upload endpoint',
  chrome: 'the Chrome integration is not hosted by Desktop',
  color: 'terminal prompt colors do not apply to Desktop',
  desktop: 'already running in Desktop',
  exit: 'closing the app is owned by the window controls',
  'extra-usage': 'replaced by account settings',
  feedback: 'Desktop has no feedback upload endpoint',
  focus: 'the terminal focus renderer does not apply to Desktop',
  install: 'native installation is managed outside the running app',
  'install-github-app': 'GitHub App installation is not hosted by Desktop',
  'install-slack-app': 'Slack App installation is not hosted by Desktop',
  mobile: 'mobile pairing is not part of the Desktop bridge',
  passes: 'referral passes are not part of LingXi Desktop',
  'privacy-settings': 'consumer privacy settings are not exposed by this engine',
  'rate-limit-options': 'disabled upstream',
  'remote-env': 'remote environments are outside the Desktop bridge',
  rewind: 'checkpoint restoration has no Desktop backend yet',
  session: 'remote session sharing is outside the Desktop bridge',
  statusline: 'terminal status lines do not apply to Desktop',
  teleport: 'disabled upstream',
  'terminal-setup': 'terminal key configuration does not apply to Desktop',
  tui: 'terminal renderer selection does not apply to Desktop',
  ultraplan: 'the web planning product is not part of LingXi Desktop',
  upgrade: 'subscription upgrades are not handled by Desktop',
  'usage-credits': 'usage-credit purchasing is not exposed by this engine',
  version: 'disabled upstream',
  voice: 'voice is controlled by the Desktop composer and Settings UI',
} as const;

const unavailableCommands: readonly DesktopCommand[] = Object.entries(
  DESKTOP_UNAVAILABLE_BUILTIN_COMMANDS,
).map(([name, reason]) => ({
  name,
  args: 'optional',
  advertised: false,
  run(_args, ctx) {
    ctx.emit(`/${name} is not available in Desktop: ${reason}.`, true);
  },
}));

export const ALL_DESKTOP_COMMANDS: readonly DesktopCommand[] = [
  ...DESKTOP_COMMANDS,
  ...unavailableCommands,
];

export type { DesktopCommandContext };
