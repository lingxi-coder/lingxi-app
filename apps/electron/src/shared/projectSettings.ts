import type { ClientCommand, ClientEvent } from '@lingxi/bridge-client';

export const CH_PROJECT_SETTINGS = 'lingxi:project-settings';

export type ProjectSettingsCommand = Extract<ClientCommand, { type:
  'refresh_listings' | 'update_settings' | 'update_permission_rules' | 'set_default_permission_mode'
  | 'update_workspace_directories' | 'skill_admin' | 'mcp_admin' | 'plugin_admin' | 'hook_admin'
}>;

export type ProjectSettingsRequest = (projectPath: string, command: ProjectSettingsCommand) => Promise<ClientEvent[]>;

export function isProjectSettingsCommand(command: ClientCommand): command is ProjectSettingsCommand {
  switch (command.type) {
    case 'refresh_listings': return command.which.length === 1 && command.which[0]?.type === 'settings';
    case 'update_settings': case 'update_permission_rules': case 'set_default_permission_mode':
    case 'update_workspace_directories': case 'skill_admin': case 'mcp_admin': case 'plugin_admin': case 'hook_admin':
      return true;
    default: return false;
  }
}
