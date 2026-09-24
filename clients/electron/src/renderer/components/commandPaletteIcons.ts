export type CommandPaletteIconName =
  | 'activity'
  | 'box'
  | 'branch'
  | 'brain'
  | 'bulb'
  | 'chat'
  | 'chatPlus'
  | 'check'
  | 'clock'
  | 'code'
  | 'compact'
  | 'copy'
  | 'folder'
  | 'folderPlus'
  | 'gauge'
  | 'git'
  | 'goal'
  | 'hook'
  | 'image'
  | 'info'
  | 'key'
  | 'logIn'
  | 'logOut'
  | 'mcp'
  | 'notebook'
  | 'pencil'
  | 'pin'
  | 'play'
  | 'puzzle'
  | 'refresh'
  | 'search'
  | 'server'
  | 'share'
  | 'shield'
  | 'shieldCheck'
  | 'sliders'
  | 'spark'
  | 'sparkle'
  | 'stop'
  | 'summary'
  | 'sun'
  | 'tasks'
  | 'terminal'
  | 'trash'
  | 'user'
  | 'users';

const ICON_BY_COMMAND: Readonly<Record<string, CommandPaletteIconName>> = {
  'add-dir': 'folderPlus',
  agents: 'users',
  'api-key': 'key',
  'auto-mode-setup': 'shieldCheck',
  autocompact: 'compact',
  batch: 'tasks',
  brief: 'summary',
  btw: 'chatPlus',
  cd: 'folder',
  clear: 'trash',
  'clear-session': 'trash',
  'code-review': 'search',
  commit: 'git',
  'commit-push-pr': 'share',
  compact: 'compact',
  config: 'sliders',
  'config-dir': 'folder',
  connect: 'server',
  context: 'summary',
  cost: 'gauge',
  copy: 'copy',
  cron: 'clock',
  dataviz: 'activity',
  'deep-research': 'search',
  diagnostics: 'activity',
  diff: 'code',
  doctor: 'shieldCheck',
  effort: 'brain',
  export: 'share',
  fast: 'spark',
  'fewer-permission-prompts': 'shieldCheck',
  filesystem: 'folder',
  fmt: 'code',
  'force-compact': 'compact',
  fork: 'branch',
  fusion: 'users',
  goal: 'goal',
  help: 'info',
  hooks: 'hook',
  ide: 'code',
  init: 'notebook',
  'init-verifiers': 'shieldCheck',
  insights: 'activity',
  keybindings: 'sliders',
  lint: 'check',
  login: 'logIn',
  logout: 'logOut',
  loop: 'refresh',
  mcp: 'mcp',
  memory: 'brain',
  model: 'box',
  network: 'server',
  new: 'chatPlus',
  'output-style': 'pencil',
  pet: 'user',
  permissions: 'shield',
  'allowed-tools': 'shield',
  pin: 'pin',
  plan: 'bulb',
  plugin: 'puzzle',
  plugins: 'puzzle',
  marketplace: 'puzzle',
  powerup: 'bulb',
  reasoning: 'brain',
  recap: 'summary',
  'release-notes': 'notebook',
  rename: 'pencil',
  resume: 'clock',
  run: 'play',
  'run-skill-generator': 'spark',
  'security-review': 'shieldCheck',
  share: 'share',
  settings: 'sliders',
  side: 'chatPlus',
  simplify: 'sparkle',
  'skill-doctor': 'shieldCheck',
  status: 'gauge',
  stats: 'gauge',
  stickers: 'image',
  stop: 'stop',
  subtask: 'users',
  tasks: 'tasks',
  bashes: 'tasks',
  theme: 'sun',
  usage: 'gauge',
  verify: 'shieldCheck',
  workflows: 'tasks',
  worktree: 'branch',

  'copy-last-response': 'copy',
  'open-account': 'user',
  'open-add-directory': 'folderPlus',
  'open-diagnostics': 'activity',
  'open-general-settings': 'sliders',
  'open-plugins': 'puzzle',
  'open-projects': 'folder',
  'open-provider-credentials': 'key',
  'refresh-agents': 'users',
  'refresh-auth': 'refresh',
  'refresh-doctor': 'shieldCheck',
  'refresh-hooks': 'hook',
  'refresh-status': 'activity',
  'reload-plugins': 'refresh',
  'show-help': 'info',
  'toggle-runtime-center': 'tasks',
  'toggle-theme': 'sun',
};

/** Accept a command token or a complete invocation without coloring its arguments. */
export function commandPaletteName(value: string): string {
  return value.trim().replace(/^\/+/, '').split(/\s/, 1)[0] ?? '';
}

/** Resolve a command or local action id to a stable line icon. */
export function commandPaletteIcon(value: string): CommandPaletteIconName {
  const name = commandPaletteName(value).toLowerCase();
  const exact = Object.prototype.hasOwnProperty.call(ICON_BY_COMMAND, name) ? ICON_BY_COMMAND[name] : undefined;
  if (exact) return exact;

  if (/(search|research|review)/.test(name)) return 'search';
  if (/(agent|team|collaborat)/.test(name)) return 'users';
  if (/(refresh|reload|restart|sync|loop)/.test(name)) return 'refresh';
  if (/(config|setting|preference)/.test(name)) return 'sliders';
  if (/(permission|security|shield|trust)/.test(name)) return 'shield';
  if (/(plugin|skill|extension)/.test(name)) return 'puzzle';
  if (/(mcp|server|network)/.test(name)) return 'server';
  if (/(file|folder|directory|workspace|project)/.test(name)) return 'folder';
  if (/(status|doctor|diagnostic|health)/.test(name)) return 'activity';
  if (/(task|batch|job)/.test(name)) return 'tasks';
  if (/(memory|reason|think)/.test(name)) return 'brain';
  if (/(image|visual|diagram|chart)/.test(name)) return 'image';
  if (/(format|fmt|code|lint|simplif)/.test(name)) return 'code';
  if (/(run|shell|terminal|exec)/.test(name)) return 'terminal';
  return 'terminal';
}

type CommandPaletteTone = 'blue' | 'violet' | 'teal' | 'green' | 'amber' | 'orange' | 'rose' | 'neutral';

const TONE_BY_ICON: Readonly<Record<CommandPaletteIconName, CommandPaletteTone>> = {
  activity: 'green',
  box: 'violet',
  branch: 'violet',
  brain: 'violet',
  bulb: 'amber',
  chat: 'blue',
  chatPlus: 'blue',
  check: 'green',
  clock: 'orange',
  code: 'teal',
  compact: 'teal',
  copy: 'blue',
  folder: 'blue',
  folderPlus: 'blue',
  gauge: 'green',
  git: 'orange',
  goal: 'orange',
  hook: 'orange',
  image: 'violet',
  info: 'blue',
  key: 'amber',
  logIn: 'blue',
  logOut: 'rose',
  mcp: 'teal',
  notebook: 'blue',
  pencil: 'blue',
  pin: 'orange',
  play: 'green',
  puzzle: 'violet',
  refresh: 'teal',
  search: 'blue',
  server: 'teal',
  share: 'blue',
  shield: 'amber',
  shieldCheck: 'green',
  sliders: 'neutral',
  spark: 'violet',
  sparkle: 'violet',
  stop: 'rose',
  summary: 'teal',
  sun: 'amber',
  tasks: 'orange',
  terminal: 'neutral',
  trash: 'rose',
  user: 'blue',
  users: 'violet',
};

// Deeper inks on light surfaces; softer, brighter inks on dark surfaces.
const COMMAND_COLORS: Readonly<Record<CommandPaletteTone, { light: string; dark: string }>> = {
  blue: { light: '#2563c2', dark: '#83b4ff' },
  violet: { light: '#7951b0', dark: '#c3a0f0' },
  teal: { light: '#177c80', dark: '#73cec9' },
  green: { light: '#2f7f4b', dark: '#83d49b' },
  amber: { light: '#936813', dark: '#e4c06e' },
  orange: { light: '#b15d28', dark: '#eeac7e' },
  rose: { light: '#b84965', dark: '#ee9fb1' },
  neutral: { light: '#5e6572', dark: '#b8bfcd' },
};

/** Keep built-ins and extension commands with the same meaning visually related. */
export function commandPaletteColor(value: string, dark: boolean): string {
  const palette = COMMAND_COLORS[TONE_BY_ICON[commandPaletteIcon(value)]];
  return dark ? palette.dark : palette.light;
}
