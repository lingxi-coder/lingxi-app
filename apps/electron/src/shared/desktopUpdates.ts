export const CH_DESKTOP_UPDATES = 'lingxi:desktop-updates';
export const CH_DESKTOP_UPDATES_CHANGED = 'lingxi:desktop-updates:changed';

export interface DesktopUpdateRelease {
  version: string;
  downloadUrl: string;
}

export interface DesktopUpdateState {
  currentVersion: string;
  release: DesktopUpdateRelease | null;
}

export interface DesktopUpdatesApi {
  getState(): Promise<DesktopUpdateState>;
  download(): Promise<void>;
  remindLater(): Promise<void>;
  onChanged(listener: (state: DesktopUpdateState) => void): () => void;
}
