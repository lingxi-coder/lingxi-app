import type { LocalAppAgentCatalog, LocalAppAgentSettings } from './localAppAgent.js';
import type { AskUserQuestionRequestDto } from '@lingxi/bridge-client';
/** Shared schema-2 application contract; platform execution remains host-owned. */
export const CH_LOCAL_APPS = 'lingxi:local-apps';
export const CH_LOCAL_APPS_CHANGED = 'lingxi:local-apps:changed';
export const LOCAL_APP_SCHEME = 'lingxi-app';

export type LocalAppCapability = 'process' | 'files' | 'clipboard' | 'notifications' | 'camera' | 'microphone' | 'photos' | 'share' | 'agent' | 'background' | 'network' | 'location' | 'calendar' | 'contacts' | 'haptics';
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
  schema: 2;
  targets: LocalAppTarget[];
  dataSchema?: { version: number; compatibleFrom: number };
  remix?: { allowed: boolean; license?: string };
  package?: { files: string[]; dependencyLocks: string[] };
  name: string;
  summary?: string;
  entry: { type: 'static'; path: string } | { type: 'server'; process: string; path?: string };
  install?: LocalAppStep[];
  build?: LocalAppStep[];
  processes?: LocalAppProcess[];
  capabilities?: LocalAppCapability[];
  /** Requested host catalog IDs. This declaration never grants tool access. */
  agent?: { tools: string[]; skills: Array<{ id: string; path: string }> };
}
export interface LocalAppTarget {
  platform: 'android' | 'ios' | 'macos' | 'windows' | 'linux' | 'web';
  environment: 'web' | 'native' | 'mobile-linux';
  architecture?: string; runtimeVersion?: string; runtimeAbi?: string; channels?: string[]; capabilities?: LocalAppCapability[];
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
  authoring?: DesktopAuthoringRef | DesktopAuthoringSourceRef;
  authoringSourceMissing?: boolean;
  authoringAction?: 'sourceMissing' | 'rebindRequired' | 'resumeSession' | 'createSession';
  background: boolean;
  grants: LocalAppCapability[];
  /** Trusted Apps settings, never supplied by an application manifest. */
  agent?: LocalAppAgentSettings;
  phase: LocalAppPhase;
  runningRevision?: number;
  releaseDigest?: string;
  marketOrigin?: { appId: string; releaseId: string; digest: string; remixedFrom?: string };
  currentRevision?: number;
  runId?: string;
  error?: string;
}
export interface LocalAppLog { sequence: number; time: number; runId: string; process: string; stream: 'stdout' | 'stderr' | 'host'; text: string }
export interface LocalAppEditRequest {
  id: string;
  name: string;
  authoring?: DesktopAuthoringRef | DesktopAuthoringSourceRef;
  authoringSourceMissing?: boolean;
  authoringAction?: 'sourceMissing' | 'rebindRequired' | 'resumeSession' | 'createSession';
}
export interface DesktopAuthoringSourceRef { sourceDirectory: string }
export type DesktopAuthoringProjectRef = { kind: 'desktopPath'; path: string };
export interface DesktopAuthoringRef {
  workspaceId: string;
  sourceDirectory: string;
  projectRef: DesktopAuthoringProjectRef;
  authoringSessionId: string;
  appId?: string;
}
export function isDesktopAuthoringRef(value: DesktopAuthoringRef | DesktopAuthoringSourceRef | undefined): value is DesktopAuthoringRef {
  return Boolean(value && 'workspaceId' in value && 'projectRef' in value && 'authoringSessionId' in value);
}
export interface LocalAppAgentQuestion {
  id: string;
  appName: string;
  request: AskUserQuestionRequestDto;
}
export interface LocalAppsApi {
  list(): Promise<DesktopLocalApp[]>;
  open(id: string): Promise<void>;
  stop(id: string): Promise<void>;
  restart(id: string): Promise<void>;
  repair(id: string): Promise<void>;
  logs(id: string): Promise<LocalAppLog[]>;
  versions(id: string): Promise<LocalAppVersion[]>;
  useVersion(id: string, revision: number, expectedCurrentRevision: number): Promise<void>;
  openData(id: string): Promise<void>;
  remove(id: string): Promise<void>;
  setBackground(id: string, enabled: boolean): Promise<void>;
  revoke(id: string): Promise<void>;
  edit(id: string): Promise<void>;
  bindAuthoring(id: string, session: { projectPath: string; sessionId: string }): Promise<DesktopAuthoringRef>;
  /** Persist a host-verified, ownerless source binding before a new app draft is shown. */
  prepareAuthoring(session: { projectPath: string; sessionId: string }): Promise<DesktopAuthoringRef>;
  marketList(query?: string, owned?: boolean): Promise<LocalAppMarketRow[]>;
  marketInstall(remoteId: string, id?: string, expectedRevision?: number): Promise<DesktopLocalApp>;
  marketPublish(id: string, metadata: LocalAppMarketPublication, expectedRevision: number): Promise<{ appId: string; versionId: string; status: string } | undefined>;
  marketManage(): Promise<void>;
  legacyEntries(): Promise<LocalAppLegacyEntry[]>;
  importLegacy(legacyId: string): Promise<void>;
  restoreSource(id: string): Promise<void>;
  remix(id: string): Promise<void>;
  agentCatalog(id: string): Promise<LocalAppAgentCatalog>;
  setAgentSettings(id: string, settings: LocalAppAgentSettings): Promise<void>;
  agentQuestions(): Promise<LocalAppAgentQuestion[]>;
  answerAgentQuestion(id: string, answers: Record<string, string> | null): Promise<void>;
  onChanged(listener: () => void): () => void;
}

export interface LocalAppVersion { revision: number; digest: string; createdAtMs: number; manifest: DesktopAppManifest }

export interface LocalAppMarketRow { id: string; name: string; summary: string; currentVersion?: { id: string }; visibility?: string }
export interface LocalAppLegacyEntry { legacyId: string; name: string; summary: string; status: string }
export interface LocalAppMarketPublication { name: string; summary: string; description: string; category: string; notes: string }
