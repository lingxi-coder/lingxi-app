import type { SessionRowDto } from '@lingxi/bridge-client';

export type SessionCatalogRow = SessionRowDto & { resume_title?: string };

export interface SessionOpenSelection {
  scope: SessionCatalogScope;
  /** Revalidated against the authenticated catalog before engine use. */
  transcriptPath?: string;
}

/** Claude Code session discovery scopes, independent of sidebar project selection. */
export type SessionCatalogScope = 'current' | 'repository' | 'all';

/** Explicit user commands only. A model's Bash cwd never invokes this operation. */
export interface SessionWorkspaceOperation {
  action: 'cd' | 'add-dir';
  /** Omitted for the native directory picker. */
  path?: string;
}

export const CH_SESSION_WORKSPACE = 'lingxi:session:workspace';
export const SESSION_PICKER_EVENT = 'lingxi:session-picker';

export function sessionCatalogScope(value: unknown): SessionCatalogScope {
  if (value === undefined) return 'current';
  if (value === 'current' || value === 'repository' || value === 'all') return value;
  throw new Error('invalid session catalog scope');
}
