/** User-operated Desktop Git API. Never part of the model transport. */
export const CH_GIT_REQUEST = 'lingxi:git:request';
export const CH_GIT_EVENT = 'lingxi:git:event';
export interface GitScope {
    projectPath: string;
    sessionId: string;
}
export interface GitFile {
    path: string;
    oldPath?: string;
    index: string;
    working: string;
    untracked: boolean;
    conflict: boolean;
    additions: number;
    deletions: number;
    binary: boolean;
    submodule?: boolean;
}
export interface GitBranch {
    name: string;
    remote: boolean;
    current: boolean;
    upstream?: string;
    worktree?: string;
}
export interface GitStatus {
    available: boolean;
    repository: boolean;
    root: string;
    gitDir: string;
    commonDir: string;
    branch: string;
    head: string;
    upstream?: string;
    ahead: number;
    behind: number;
    files: GitFile[];
    token: string;
    busy: boolean;
    mergeOwned: boolean;
    merging: boolean;
    remotes: string[];
}
export interface GitCommit {
    id: string;
    parents: string[];
    author: string;
    date: string;
    subject: string;
    body?: string;
}
export interface GitStash {
    id: string;
    ref: string;
    subject: string;
    date: string;
}
export interface GitDiff {
    patch: string;
    truncated: boolean;
    binary: boolean;
    token: string;
    files: GitFile[];
}
export interface GitConflict {
    baseExists?: boolean;
    oursExists?: boolean;
    theirsExists?: boolean;
    path: string;
    base: string;
    ours: string;
    theirs: string;
    result: string;
    binary: boolean;
    token: string;
}
export type GitRequest = {
    kind: 'status' | 'branches' | 'stashList' | 'init';
} | {
    kind: 'diff';
    mode: 'working' | 'staged' | 'branch' | 'commit' | 'stash';
    path?: string;
    base?: string;
    target?: string;
} | {
    kind: 'history';
    skip?: number;
    limit?: number;
} | {
    kind: 'stage' | 'unstage';
    paths?: string[];
    patch?: string;
    token: string;
} | {
    kind: 'discard';
    paths: string[];
    untracked: boolean;
    token: string;
} | {
    kind: 'commit';
    message: string;
    token: string;
} | {
    kind: 'checkout';
    branch: string;
    create?: boolean;
    token: string;
} | {
    kind: 'fetch';
    remote?: string;
} | {
    kind: 'pull' | 'push';
    remote: string;
    branch: string;
    setUpstream?: boolean;
    token: string;
} | {
    kind: 'merge';
    branch: string;
    token: string;
} | {
    kind: 'mergeContinue';
    message: string;
    token: string;
} | {
    kind: 'mergeAbort';
    token: string;
} | {
    kind: 'stashCreate';
    message: string;
    includeUntracked: boolean;
    token: string;
} | {
    kind: 'stashApply' | 'stashDrop';
    id: string;
    token: string;
} | {
    kind: 'conflictRead';
    path: string;
} | {
    kind: 'conflictSave';
    path: string;
    content?: string;
    side?: 'ours' | 'theirs';
    token: string;
} | {
    kind: 'conflictResolve';
    path: string;
    token: string;
};
export interface GitResult {
    status?: GitStatus;
    branches?: GitBranch[];
    diff?: GitDiff;
    commits?: GitCommit[];
    stashes?: GitStash[];
    conflict?: GitConflict;
    output?: string;
}
export interface GitEvent {
    root: string;
}
export interface GitApi {
    request(scope: GitScope, request: GitRequest): Promise<GitResult>;
    onChanged(callback: (event: GitEvent) => void): () => void;
}
