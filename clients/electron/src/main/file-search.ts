import { lstat, readdir, realpath } from 'node:fs/promises';
import { basename, join } from 'node:path';

export const MAX_WORKSPACE_FILES = 50_000;
export const MAX_WORKSPACE_DIRECTORIES = 20_000;
export const MAX_WORKSPACE_ENTRIES = 100_000;
export const MAX_FILE_SEARCH_RESULTS = 50;
export const MAX_FILE_SEARCH_QUERY_LENGTH = 512;
const MAX_FILE_SEARCH_DEPTH = 16;
const FILE_INDEX_TTL_MS = 5_000;

const SKIPPED_DIRECTORIES = new Set([
  '.cache',
  '.cxx',
  '.dart_tool',
  '.externalNativeBuild',
  '.git',
  '.gradle',
  '.idea',
  '.lingxi',
  '.next',
  '.turbo',
  '.worktrees',
  'DerivedData',
  'Pods',
  'build',
  'coverage',
  'dist',
  'node_modules',
  'out',
  'target',
]);

export interface WorkspaceFileSearchResult {
  files: string[];
  truncated: boolean;
}

interface WorkspaceFileIndex {
  workspace: string;
  files: string[];
  truncated: boolean;
  expiresAt: number;
}

function validateQuery(query: unknown): string {
  if (typeof query !== 'string' || query.length > MAX_FILE_SEARCH_QUERY_LENGTH || query.includes('\0')) {
    throw new Error('invalid workspace file query');
  }
  return query.replaceAll('\\', '/').toLocaleLowerCase();
}

function hiddenPath(path: string): boolean {
  return path.split('/').some((part) => part.startsWith('.'));
}

function fuzzyMatchScore(value: string, query: string): number | undefined {
  let queryIndex = 0;
  let gapPenalty = 0;
  let previousMatch = -1;
  for (let index = 0; index < value.length && queryIndex < query.length; index += 1) {
    if (value[index] !== query[queryIndex]) continue;
    if (previousMatch >= 0) gapPenalty += index - previousMatch - 1;
    previousMatch = index;
    queryIndex += 1;
  }
  return queryIndex === query.length ? gapPenalty : undefined;
}

function segmentedPathScore(path: string, query: string): number | undefined {
  const terms = query.split('/').filter(Boolean);
  if (terms.length < 2) return undefined;
  let cursor = 0;
  let gapPenalty = 0;
  for (const term of terms) {
    const index = path.indexOf(term, cursor);
    if (index < 0) return undefined;
    gapPenalty += index - cursor;
    cursor = index + term.length;
  }
  return gapPenalty;
}

function scorePath(path: string, query: string): number | undefined {
  if (hiddenPath(path) && !query.includes('.')) return undefined;
  if (!query) return path.split('/').length * 10 + path.length / 1_000;

  const lowerPath = path.toLocaleLowerCase();
  const lowerName = basename(path).toLocaleLowerCase();
  if (lowerName === query) return 0;
  if (lowerName.startsWith(query)) return 100 + lowerName.length / 1_000;
  if (lowerName.includes(query)) return 200 + lowerName.indexOf(query) + lowerName.length / 1_000;
  if (lowerPath.startsWith(query)) return 300 + lowerPath.length / 1_000;
  const pathIndex = lowerPath.indexOf(query);
  if (pathIndex >= 0) return 400 + pathIndex + lowerPath.length / 1_000;
  const segmented = segmentedPathScore(lowerPath, query);
  if (segmented !== undefined) return 450 + segmented + lowerPath.length / 1_000;
  // Fuzzy matching across an entire path makes long queries appear to match
  // unrelated build/cache files simply because their directory names contain
  // the requested letters in order. Keep typo-tolerance local to the basename
  // and reject excessively gappy matches.
  const fuzzy = query.length >= 2 ? fuzzyMatchScore(lowerName, query) : undefined;
  const maxGap = Math.max(2, Math.floor(query.length / 2));
  return fuzzy === undefined || fuzzy > maxGap
    ? undefined
    : 500 + fuzzy + lowerName.length / 1_000;
}

async function buildWorkspaceFileIndex(workspace: string): Promise<WorkspaceFileIndex> {
  const canonical = await realpath(workspace);
  if (!(await lstat(canonical)).isDirectory()) throw new Error('workspace path is not a directory');

  const files: string[] = [];
  const directories: Array<{ absolute: string; relative: string; depth: number }> = [
    { absolute: canonical, relative: '', depth: 0 },
  ];
  let directoryCursor = 0;
  let visitedEntries = 0;
  let truncated = false;

  while (directoryCursor < directories.length && files.length < MAX_WORKSPACE_FILES) {
    const current = directories[directoryCursor++]!;
    let entries;
    try {
      entries = await readdir(current.absolute, { withFileTypes: true });
    } catch {
      continue;
    }
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      visitedEntries += 1;
      if (files.length >= MAX_WORKSPACE_FILES || visitedEntries > MAX_WORKSPACE_ENTRIES) {
        truncated = true;
        break;
      }
      const relative = current.relative ? `${current.relative}/${entry.name}` : entry.name;
      if (/[\u0000-\u001f\u007f]/.test(relative)) continue;
      if (entry.isSymbolicLink()) continue;
      if (entry.isDirectory()) {
        if (current.depth < MAX_FILE_SEARCH_DEPTH && !SKIPPED_DIRECTORIES.has(entry.name)) {
          if (directories.length >= MAX_WORKSPACE_DIRECTORIES) {
            truncated = true;
            continue;
          }
          directories.push({
            absolute: join(current.absolute, entry.name),
            relative,
            depth: current.depth + 1,
          });
        }
      } else if (entry.isFile()) {
        files.push(relative);
      }
    }
    if (visitedEntries > MAX_WORKSPACE_ENTRIES) break;
  }
  if (directoryCursor < directories.length) truncated = true;

  return {
    workspace: canonical,
    files,
    truncated,
    expiresAt: Date.now() + FILE_INDEX_TTL_MS,
  };
}

/** Cached, read-only file index used by the trusted-workspace `@` picker. */
export class WorkspaceFileSearch {
  private index?: WorkspaceFileIndex;
  private pending?: { workspace: string; promise: Promise<WorkspaceFileIndex> };

  invalidate(): void {
    this.index = undefined;
    this.pending = undefined;
  }

  async search(
    workspace: string,
    query: unknown,
    limit = MAX_FILE_SEARCH_RESULTS,
  ): Promise<WorkspaceFileSearchResult> {
    const normalizedQuery = validateQuery(query);
    const boundedLimit = Number.isSafeInteger(limit) && limit > 0
      ? Math.min(limit, MAX_FILE_SEARCH_RESULTS)
      : MAX_FILE_SEARCH_RESULTS;
    const canonical = await realpath(workspace);
    let index = this.index;
    if (!index || index.workspace !== canonical || index.expiresAt <= Date.now()) {
      if (!this.pending || this.pending.workspace !== canonical) {
        const promise = buildWorkspaceFileIndex(canonical);
        this.pending = { workspace: canonical, promise };
      }
      try {
        index = await this.pending.promise;
      } finally {
        if (this.pending?.workspace === canonical) this.pending = undefined;
      }
      this.index = index;
    }

    const matches = index.files
      .map((path) => ({ path, score: scorePath(path, normalizedQuery) }))
      .filter((entry): entry is { path: string; score: number } => entry.score !== undefined)
      .sort((left, right) => left.score - right.score || left.path.localeCompare(right.path));

    return {
      files: matches.slice(0, boundedLimit).map((entry) => entry.path),
      truncated: index.truncated || matches.length > boundedLimit,
    };
  }
}
