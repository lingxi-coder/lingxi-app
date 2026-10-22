/** Private Desktop controls. These do not change the shared client protocol. */
export interface SetWorkingDirectoryRequest {
  path: string;
  trust_accepted?: boolean;
  trusted_directory?: string;
}

export type SetWorkingDirectoryResult =
  | { status: 'ok'; cwd: string; changed: boolean; transcript_relocated: boolean }
  | { status: 'needs_trust'; directory: string; trust_root?: string }
  | { status: 'rejected'; reason: string; message: string };

export interface SessionWorkspaceState {
  cwd: string;
  working_directory?: string;
  initial_project_root?: string;
  transcript_path?: string;
  /** Whether the live engine has user or assistant history to resume. */
  has_conversation?: boolean;
  prompt_directories?: string[];
  /** Explicit session grants, excluding settings-derived directories. */
  additional_directories: string[];
}

function record(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('invalid workspace response');
  return value as Record<string, unknown>;
}

function text(value: unknown): string {
  if (typeof value !== 'string' || !value.length || value.length > 32_768 || value.includes('\0')) {
    throw new Error('invalid workspace response text');
  }
  return value;
}

export function validateSetWorkingDirectoryResult(value: unknown): SetWorkingDirectoryResult {
  const input = record(value);
  switch (input.status) {
    case 'ok':
      if (typeof input.changed !== 'boolean' || typeof input.transcript_relocated !== 'boolean') break;
      return { status: 'ok', cwd: text(input.cwd), changed: input.changed, transcript_relocated: input.transcript_relocated };
    case 'needs_trust':
      return { status: 'needs_trust', directory: text(input.directory),
        ...(input.trust_root === undefined ? {} : { trust_root: text(input.trust_root) }) };
    case 'rejected':
      return { status: 'rejected', reason: text(input.reason), message: text(input.message) };
  }
  throw new Error('invalid working directory result');
}

export function validateSessionWorkspaceState(value: unknown): SessionWorkspaceState {
  const input = record(value);
  if (!Array.isArray(input.additional_directories) || input.additional_directories.length > 1024) {
    throw new Error('invalid additional directories');
  }
  if (input.prompt_directories !== undefined && (!Array.isArray(input.prompt_directories) || input.prompt_directories.length > 1024)) throw new Error('invalid prompt directories');
  if (input.has_conversation !== undefined && typeof input.has_conversation !== 'boolean') throw new Error('invalid workspace conversation state');
  return { cwd: text(input.cwd), additional_directories: input.additional_directories.map(text),
    ...(input.working_directory === undefined ? {} : { working_directory: text(input.working_directory) }),
    ...(input.initial_project_root === undefined ? {} : { initial_project_root: text(input.initial_project_root) }),
    ...(input.transcript_path === undefined ? {} : { transcript_path: text(input.transcript_path) }),
    ...(input.has_conversation === undefined ? {} : { has_conversation: input.has_conversation }),
    ...(input.prompt_directories === undefined ? {} : { prompt_directories: (input.prompt_directories as unknown[]).map(text) }),
  };
}
