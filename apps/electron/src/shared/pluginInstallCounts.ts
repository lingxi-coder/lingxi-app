export const CH_PLUGIN_INSTALL_COUNTS = 'lingxi:plugin-install-counts';

export const claudeCodeOfficialMarketplace = {
  name: 'claude-plugins-official',
  source: 'anthropics/claude-plugins-official',
} as const;

export type PluginInstallCounts = Readonly<Record<string, number>>;
