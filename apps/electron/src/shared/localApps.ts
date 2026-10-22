/** Desktop Local Apps. Mobile retains its independent document-only contract. */
export const CH_LOCAL_APPS = 'lingxi:local-apps';
export const CH_LOCAL_APPS_CHANGED = 'lingxi:local-apps:changed';
export const LOCAL_APP_SCHEME = 'lingxi-app';

export type LocalAppCapability = 'process' | 'files' | 'clipboard' | 'notifications' | 'camera' | 'microphone';
export interface LocalAppStep {
  /** A shell command, executed in cwd with the application's environment. */
  command: string;
  cwd?: string;
  env?: Record<string, string>;
  /** An app-relative Python environment. Created during install, activated on every run. */
  venv?: string;
  python?: string;
}
export interface LocalAppProcess extends LocalAppStep {
  name: string;
  /** Other named services must be ready before this service is started. */
  dependsOn?: string[];
  ready?: { path?: string; output?: string; timeoutMs?: number };
}
export interface DesktopAppManifest {
  schema: 1;
  name: string;
  summary?: string;
  entry: { type: 'static'; path: string } | { type: 'server'; process: string; path?: string };
  install?: LocalAppStep[];
  build?: LocalAppStep[];
  processes?: LocalAppProcess[];
  capabilities?: LocalAppCapability[];
}
export type LocalAppPhase = 'stopped' | 'installing' | 'building' | 'starting' | 'running' | 'stopping' | 'failed';
export interface DesktopLocalApp {
  id: string;
  name: string;
  summary: string;
  revision: number;
  manifest: DesktopAppManifest;
  createdAt: number;
  updatedAt: number;
  sourcePath?: string;
  sourceSession?: { projectPath: string; sessionId: string };
  background: boolean;
  grants: LocalAppCapability[];
  phase: LocalAppPhase;
  runningRevision?: number;
  error?: string;
}
export interface LocalAppLog { sequence: number; time: number; runId: string; process: string; stream: 'stdout' | 'stderr' | 'host'; text: string }
export interface LocalAppEditRequest {
  id: string;
  name: string;
  sourcePath?: string;
  sourceSession?: DesktopLocalApp['sourceSession'];
}
export interface LocalAppsApi {
  list(): Promise<DesktopLocalApp[]>;
  open(id: string): Promise<void>;
  stop(id: string): Promise<void>;
  restart(id: string): Promise<void>;
  repair(id: string): Promise<void>;
  logs(id: string): Promise<LocalAppLog[]>;
  remove(id: string): Promise<void>;
  setBackground(id: string, enabled: boolean): Promise<void>;
  revoke(id: string): Promise<void>;
  edit(id: string): Promise<void>;
  onChanged(listener: () => void): () => void;
}
