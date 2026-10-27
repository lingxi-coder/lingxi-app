import type { CronJobDto, CronRequestDto, ModelDetailsDto } from '@lingxi/bridge-client';
export const CH_SCHEDULED = 'lingxi:scheduled';
export interface ScheduledScope {
  id: string;
  projectPath?: string;
  label: string;
  unavailable?: boolean;
  /** Present when the scope is a Bot's workspace: the Bot whose routines run in it. Such a scope has no `projectPath`. */
  botId?: string;
}
export interface ScheduledContext { models: ModelDetailsDto[]; currentModel: string; sessions: { uuid: string; title?: string }[] }
export interface ScheduledApi {
  context(scopeId: string): Promise<ScheduledContext>;
  scopes(): Promise<ScheduledScope[]>;
  manage(scopeId: string, request: CronRequestDto): Promise<CronJobDto[]>;
  openSession(scopeId: string, sessionId: string): Promise<unknown>;
}
