export type CommandPaletteIconName =
  | 'activity'
  | 'box'
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
  | 'hook'
  | 'image'
  | 'info'
  | 'key'
  | 'logIn'
  | 'logOut'
  | 'mcp'
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
  autocompact: 'compact',
  batch: 'tasks',
  brief: 'summary',
  btw: 'chatPlus',
  cd: 'folder',
  clear: 'trash',
  'clear-session': 'trash',
  'code-review': 'search',
  compact: 'compact',
  config: 'sliders',
  'config-dir': 'folder',
  copy: 'copy',
  cron: 'clock',
  dataviz: 'activity',
  'deep-research': 'search',
  diagnostics: 'activity',
  doctor: 'shieldCheck',
  'fewer-permission-prompts': 'shieldCheck',
  filesystem: 'folder',
  fmt: 'code',
  'force-compact': 'compact',
  help: 'info',
  hooks: 'hook',
  ide: 'code',
  lint: 'check',
  login: 'logIn',
  logout: 'logOut',
  loop: 'refresh',
  mcp: 'mcp',
  memory: 'brain',
  model: 'box',
  network: 'server',
  new: 'chatPlus',
  pet: 'user',
  permissions: 'shield',
  pin: 'pin',
  plan: 'bulb',
  plugin: 'puzzle',
  reasoning: 'brain',
  rename: 'pencil',
  run: 'play',
  'run-skill-generator': 'spark',
  share: 'share',
  side: 'chatPlus',
  simplify: 'sparkle',
  'skill-doctor': 'shieldCheck',
  status: 'gauge',
  tasks: 'tasks',
  theme: 'sun',
  verify: 'shieldCheck',

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

/** Resolve a command or local action id to a stable line icon. */
export function commandPaletteIcon(value: string): CommandPaletteIconName {
  const name = value.trim().replace(/^\/+/, '').toLowerCase();
  const exact = ICON_BY_COMMAND[name];
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
