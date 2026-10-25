export const CH_COMPUTER_SETTINGS = 'lingxi:computer-settings';
export const CH_COMPUTER_SETTINGS_CHANGED = 'lingxi:computer-settings:changed';
export type ComputerPermission = 'ask' | 'allow' | 'deny';
export interface ComputerApplicationPermission {
  bundleId: string;
  displayName: string;
  permission: ComputerPermission;
}
export interface BrowserSitePermission {
  origin: string;
  browsing: ComputerPermission;
  downloads: ComputerPermission;
}
export interface BrowserPermissionSettings {
  history: ComputerPermission;
  siteTools: boolean;
  fullCdp: boolean;
  defaultBrowsing: ComputerPermission;
  defaultDownloads: ComputerPermission;
  sites: BrowserSitePermission[];
}
export interface ComputerDevicePolicy {
  revision: string;
  enabled: boolean;
  apps: ComputerApplicationPermission[];
  browser: BrowserPermissionSettings;
}
export interface ComputerSettingsSnapshot extends ComputerDevicePolicy {
  error?: string;
  browserStatus: { connected: boolean; installed: boolean; extensionDirectory: string; error?: string };
  capabilities: { computer: boolean; browser: boolean; lockedUse: boolean; excel: boolean };
}
export type ComputerSettingsCommand =
  | { action: 'get' }
  | { action: 'setEnabled'; revision: string; enabled: boolean }
  | { action: 'setApplication'; revision: string; bundleId: string; permission: ComputerPermission }
  | { action: 'removeApplication'; revision: string; bundleId: string }
  | { action: 'setBrowser'; revision: string; key: 'history' | 'siteTools' | 'fullCdp' | 'defaultBrowsing' | 'defaultDownloads'; value: ComputerPermission | boolean }
  | { action: 'setSite'; revision: string; origin: string; browsing: ComputerPermission; downloads: ComputerPermission }
  | { action: 'removeSite'; revision: string; origin: string }
  | { action: 'retryBrowser' | 'installBrowser' | 'removeBrowser' | 'showExtension' };
export interface ComputerSettingsApi {
  execute(command: ComputerSettingsCommand): Promise<ComputerSettingsSnapshot>;
  onChanged(listener: (state: ComputerSettingsSnapshot) => void): () => void;
}
