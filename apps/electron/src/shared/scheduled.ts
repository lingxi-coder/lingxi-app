import type { CronJobDto, CronRequestDto, ModelDetailsDto } from '@lingxi/bridge-client';
export const CH_SCHEDULED = 'lingxi:scheduled';
export interface ScheduledScope {
  id: string;
  projectPath?: string;
  label: string;
  unavailable?: boolean;
  /** Present when the scope is a Bot's workspace: the Bot whose routines run in it. Such a scope has no `projectPath`. */
  botId?: string;
  /** For a Bot that has folders besides its own: the folders a routine of it may run in. The Bot's own has the id `default` and no label. */
  folders?: Array<{ id: string; label: string }>;
}
export interface ScheduledContext { models: ModelDetailsDto[]; currentModel: string; sessions: { uuid: string; title?: string }[] }
export interface ScheduledApi {
  context(scopeId: string): Promise<ScheduledContext>;
  scopes(): Promise<ScheduledScope[]>;
  /** `folderId` chooses the folder a Bot's routine runs in when it is made or changed. */
  manage(scopeId: string, request: CronRequestDto, folderId?: string): Promise<CronJobDto[]>;
  /** The folder each routine of a Bot runs in, by routine id; one that is not listed runs in the Bot's own. */
  routineFolders(scopeId: string): Promise<Record<string, string>>;
  openSession(scopeId: string, sessionId: string): Promise<unknown>;
}
