import { EventEmitter } from 'node:events';
import { createHash } from 'node:crypto';
import { watch as fsWatch, type FSWatcher } from 'node:fs';
import { lstat, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import path from 'node:path';
import type { GitScope, GitRequest, GitResult, GitStatus, GitFile, GitEvent, GitDiff } from '../shared/git';
import { git as runGit, stopGitCommands } from './git/command';
interface Repository {
    root: string;
    gitDir: string;
    commonDir: string;
}
const conflicts = new Set(['DD', 'AU', 'UD', 'UA', 'DU', 'AA', 'UU']);
const writing = new Set(['init', 'stage', 'unstage', 'discard', 'commit', 'checkout', 'fetch', 'pull', 'push', 'merge', 'mergeContinue', 'mergeAbort', 'stashCreate', 'stashApply', 'stashDrop', 'conflictSave', 'conflictResolve']);
const rewriting = new Set(['checkout', 'discard', 'pull', 'merge', 'mergeContinue', 'mergeAbort', 'stashCreate', 'stashApply', 'conflictSave', 'conflictResolve']);
/**
 * Which untracked paths a submitted patch could possibly be talking about.
 *
 * `null` means "cannot tell" — a `diff --git` header with a quoted path (git
 * C-quotes anything non-ASCII or containing a space), where the caller must fall
 * back to previewing every untracked file rather than silently refusing a patch
 * it should accept. Matching on `/<path>` rather than parsing the header keeps
 * this insensitive to the `a/`-vs-`b/` side and to rename headers.
 */
function patchedUntrackedPaths(patch: string): ((candidate: string) => boolean) | null {
    const headers = patch.split('\n').filter(line => line.startsWith('diff --git '));
    if (headers.some(line => line.includes('"'))) return null;
    return candidate => headers.some(line => line.includes(`/${candidate}`));
}
export class GitService {
    private events = new EventEmitter();
    private disposed = false;
    private async remove(target: string, options: Parameters<typeof rm>[1]) { this.assertActive(); return rm(target, options); }
    private async write(target: string, content: string | Buffer) { this.assertActive(); return writeFile(target, content); }
    private assertActive(): void { if (this.disposed) throw new Error('Git service is closed'); }
    private async git(cwd: string, args: string[], input?: string | Buffer, allowFailure = false, maxBytes?: number) { this.assertActive(); this.servicedRoots.add(cwd); return runGit(cwd, args, input, allowFailure, maxBytes); }
    private async text(cwd: string, args: string[], allowFailure = false): Promise<string> { return (await this.git(cwd, args, undefined, allowFailure)).stdout.toString('utf8').trimEnd(); }
    private servicedRoots = new Set<string>();
    private queues = new Map<string, Promise<unknown>>();
    private ownedMerges = new Map<string, string>();
    private watchers = new Set<() => void>();
    private sharedWatches = new Map<string, {
        references: number;
        close: () => void;
    }>();
    constructor(private options: {
        isBusy?: (root: string) => boolean | Promise<boolean>;
    } = {}) { }
    onChanged(callback: (event: GitEvent) => void): () => void { this.events.on('change', callback); return () => { this.events.off('change', callback); }; }
    async resolve(scope: GitScope): Promise<Repository | null> {
        this.assertActive();
        const cwd = await realpath(scope.projectPath);
        this.assertActive();
        const result = await this.git(cwd, ['rev-parse', '--show-toplevel', '--absolute-git-dir', '--git-common-dir'], undefined, true);
        if (result.code !== 0) {
            if (/not a git repository/i.test(result.stderr)) return null;
            throw new Error(result.stderr.trim() || 'Unable to read Git repository');
        }
        const [root, gitDir, common] = result.stdout.toString().trimEnd().split('\n');
        return { root: await realpath(root), gitDir, commonDir: path.resolve(cwd, common) };
    }
    async watch(scope: GitScope): Promise<() => void> {
        this.assertActive();
        const repo = await this.resolve(scope);
        this.assertActive();
        if (!repo)
            return () => { };
        const release = () => { const entry = this.sharedWatches.get(repo.root); if (entry && --entry.references === 0) {
            entry.close();
            this.sharedWatches.delete(repo.root);
        } };
        const existing = this.sharedWatches.get(repo.root);
        if (existing) {
            existing.references++;
            let released = false;
            return () => { if (!released) {
                released = true;
                release();
            } };
        }
        let timer: ReturnType<typeof setTimeout> | undefined;
        const handles: FSWatcher[] = [];
        const invalidate = () => { clearTimeout(timer); timer = setTimeout(() => this.events.emit('change', { root: repo.root }), 180); timer.unref(); };
        for (const dir of new Set([repo.root, repo.gitDir, repo.commonDir])) {
            try {
                handles.push(fsWatch(dir, { recursive: true }, invalidate));
            }
            catch {
                try {
                    handles.push(fsWatch(dir, invalidate));
                }
                catch { }
            }
        }
        const close = () => { clearTimeout(timer); handles.forEach(h => h.close()); this.watchers.delete(close); };
        this.watchers.add(close);
        this.sharedWatches.set(repo.root, { references: 1, close });
        let released = false;
        return () => { if (!released) {
            released = true;
            release();
        } };
    }
    dispose(): void { this.disposed = true; stopGitCommands(this.servicedRoots); for (const close of this.watchers)
        close(); this.sharedWatches.clear(); this.events.removeAllListeners(); }
    private async files(repo: Repository): Promise<GitFile[]> {
        const raw = (await this.git(repo.root, ['status', '--porcelain=v1', '-z', '--untracked-files=all'])).stdout.toString('utf8').split('\0');
        const files: GitFile[] = [];
        for (let i = 0; i < raw.length; i++) {
            const item = raw[i];
            if (!item)
                continue;
            const index = item[0], working = item[1], name = item.slice(3);
            const renamed = index === 'R' || index === 'C' || working === 'R' || working === 'C';
            files.push({ path: name, oldPath: renamed ? raw[++i] : undefined, index, working, untracked: index === '?', conflict: conflicts.has(index + working), additions: 0, deletions: 0, binary: false });
        }
        const indexEntries = (await this.git(repo.root, ['ls-files', '--stage', '-z'])).stdout.toString().split('\0');
        for (const entry of indexEntries) {
            if (entry.startsWith('160000 ')) {
                const file = files.find(file => file.path === entry.slice(entry.indexOf('\t') + 1));
                if (file) file.submodule = true;
            }
        }
        for (const args of [['diff', '--numstat', '-z'], ['diff', '--cached', '--numstat', '-z']]) {
            const entries = (await this.git(repo.root, args)).stdout.toString().split('\0');
            for (let i = 0; i < entries.length; i++) {
                const match = /^([^\t]+)\t([^\t]+)\t([\s\S]*)$/.exec(entries[i]);
                if (!match)
                    continue;
                let name = match[3];
                if (!name) {
                    i++;
                    name = entries[++i];
                }
                const file = files.find(f => f.path === name);
                if (file) {
                    file.binary ||= match[1] === '-';
                    file.additions += Number(match[1]) || 0;
                    file.deletions += Number(match[2]) || 0;
                }
            }
        }
        return files;
    }
    private async token(repo: Repository, files: GitFile[], head: string): Promise<string> {
        const hash = createHash('sha256').update(head).update(JSON.stringify(files));
        for (const name of ['HEAD', 'MERGE_HEAD']) {
            try {
                hash.update(await readFile(path.join(repo.gitDir, name)));
            }
            catch { }
        }
        try {
            hash.update(await readFile(path.join(repo.gitDir, 'index')));
        }
        catch { }
        // Identity comes from the stat triple, never the bytes. `token` is
        // computed inside `status`, which runs at the head of EVERY request and
        // on every debounced watcher tick, so hashing file contents made one
        // refresh read the whole dirty tree — untracked build output included,
        // since `files` comes from `--untracked-files=all` — in the main
        // process. size+mtime+ctime changes on every write git itself can see.
        await Promise.all(files.map(async (file) => {
            try {
                const stat = await lstat(path.join(repo.root, file.path));
                return `${file.path}:${stat.isFile() ? `${stat.size}:${stat.mtimeMs}:${stat.ctimeMs}` : `${stat.mode}:${stat.mtimeMs}`}`;
            }
            catch {
                return `${file.path}:missing`;
            }
        })).then((entries) => { for (const entry of entries) hash.update(entry); });
        return hash.digest('hex');
    }
    private async status(repo: Repository): Promise<GitStatus> {
        const files = await this.files(repo);
        const head = await this.text(repo.root, ['rev-parse', '--verify', 'HEAD'], true);
        const branch = await this.text(repo.root, ['symbolic-ref', '--short', '-q', 'HEAD'], true) || '(detached HEAD)';
        const upstream = await this.text(repo.root, ['rev-parse', '--abbrev-ref', '--symbolic-full-name', '@{upstream}'], true) || undefined;
        const counts = upstream ? (await this.text(repo.root, ['rev-list', '--left-right', '--count', 'HEAD...@{upstream}'], true)).split(/\s+/) : [];
        const mergeHead = await this.text(repo.root, ['rev-parse', '-q', '--verify', 'MERGE_HEAD'], true);
        const merging = !!mergeHead;
        if (!merging || this.ownedMerges.get(repo.root) !== mergeHead)
            this.ownedMerges.delete(repo.root);
        return { available: true, repository: true, ...repo, branch, head, upstream, ahead: Number(counts[0]) || 0, behind: Number(counts[1]) || 0, files, token: await this.token(repo, files, head), busy: await this.options.isBusy?.(repo.root) || false, mergeOwned: this.ownedMerges.has(repo.root), merging, remotes: (await this.text(repo.root, ['remote'])).split('\n').filter(Boolean) };
    }
    private async checkedPath(repo: Repository, value: string): Promise<string> {
        if (!value || value.includes('\0') || path.isAbsolute(value) || value.split(/[\\/]/).includes('..') || value.split(/[\\/]/).includes('.git'))
            throw new Error('Invalid repository path');
        const abs = path.resolve(repo.root, value);
        let ancestor = path.dirname(abs);
        let parent: string;
        for (;;) {
            try {
                parent = await realpath(ancestor);
                break;
            }
            catch (error) {
                if ((error as NodeJS.ErrnoException).code !== 'ENOENT')
                    throw error;
                const next = path.dirname(ancestor);
                if (next === ancestor)
                    throw error;
                ancestor = next;
            }
        }
        if (parent !== repo.root && !parent.startsWith(repo.root + path.sep))
            throw new Error('Path escapes repository');
        return value;
    }
    private async ref(repo: Repository, value: string): Promise<string> { if (!value || value.startsWith('-') || value.includes('\0'))
        throw new Error('Invalid Git reference'); return await this.text(repo.root, ['rev-parse', '--verify', `${value}^{commit}`]); }
    async request(scope: GitScope, request: GitRequest): Promise<GitResult> {
        this.assertActive();
        if (!scope || typeof scope.projectPath !== 'string' || typeof scope.sessionId !== 'string' || !request || typeof request.kind !== 'string')
            throw new Error('Invalid Git request');
        let repo: Repository | null;
        try {
            repo = await this.resolve(scope);
        }
        catch (error) {
            if ((error as NodeJS.ErrnoException).code === 'ENOENT' && request.kind === 'status')
                return { status: { available: false, repository: false, root: scope.projectPath, gitDir: '', commonDir: '', branch: '', head: '', ahead: 0, behind: 0, files: [], token: '', busy: false, mergeOwned: false, merging: false, remotes: [] } };
            throw error;
        }
        if (!repo) {
            if (request.kind === 'init') {
                await this.git(scope.projectPath, ['init']);
                repo = await this.resolve(scope);
            }
            else if (request.kind === 'status')
                return { status: { available: true, repository: false, root: scope.projectPath, gitDir: '', commonDir: '', branch: '', head: '', ahead: 0, behind: 0, files: [], token: '', busy: false, mergeOwned: false, merging: false, remotes: [] } };
            else
                throw new Error('This directory is not a Git repository');
        }
        if (!repo)
            throw new Error('Unable to initialize repository');
        const selected = repo;
        this.servicedRoots.add(repo.root);
        const run = async () => { this.assertActive(); const result = await this.perform(selected, request); if (writing.has(request.kind))
            this.events.emit('change', { root: selected.root }); return result; };
        if (!writing.has(request.kind))
            return run();
        const previous = this.queues.get(repo.commonDir) || Promise.resolve();
        const next = previous.catch(() => { }).then(run);
        this.queues.set(repo.commonDir, next);
        try {
            return await next;
        }
        finally {
            if (this.queues.get(repo.commonDir) === next)
                this.queues.delete(repo.commonDir);
        }
    }
    async resolveRoot(projectPath: string): Promise<string | null> { return (await this.resolve({ projectPath, sessionId: '' }))?.root ?? null; }
    private async perform(repo: Repository, request: GitRequest): Promise<GitResult> {
        this.assertActive();
        const current = await this.status(repo);
        this.assertActive();
        if ('token' in request && request.token !== current.token)
            throw new Error('Repository changed. Refresh before retrying this operation.');
        if (rewriting.has(request.kind) && current.busy)
            throw new Error('An agent is using this worktree. Wait for it to finish before changing the working tree.');
        const cwd = repo.root;
        const paths = async (values: string[]) => Promise.all(values.map(p => this.checkedPath(repo, p)));
        switch (request.kind) {
            case 'status':
            case 'init': return { status: current };
            case 'branches': {
                const occupied = new Map<string, string>();
                let location = '';
                for (const line of (await this.text(cwd, ['worktree', 'list', '--porcelain'])).split('\n')) {
                    if (line.startsWith('worktree '))
                        location = line.slice(9);
                    if (line.startsWith('branch refs/heads/'))
                        occupied.set(line.slice(18), location);
                }
                const lines = (await this.text(cwd, ['for-each-ref', '--format=%(refname)%00%(upstream:short)', 'refs/heads/', 'refs/remotes/'])).split('\n').filter(Boolean);
                return { branches: lines.filter(l => !l.split('\0')[0].endsWith('/HEAD')).map(line => { const [ref, upstream] = line.split('\0'); const remote = ref.startsWith('refs/remotes/'); const name = ref.slice(remote ? 13 : 11); return { name, remote, current: !remote && name === current.branch, upstream: upstream || undefined, worktree: occupied.get(name) }; }) };
            }
            case 'history': {
                if (!current.head)
                    return { commits: [] };
                const skip = Math.max(0, Math.floor(request.skip || 0)), limit = Math.max(1, Math.min(100, Math.floor(request.limit || 40)));
                const raw = await this.text(cwd, ['log', `--skip=${skip}`, `--max-count=${limit}`, '--format=%H%x00%P%x00%an%x00%aI%x00%s%x00%b%x00']);
                const fields = raw.split('\0');
                const commits = [];
                for (let i = 0; i + 5 < fields.length; i += 6) {
                    commits.push({ id: fields[i].trim(), parents: fields[i + 1].split(' ').filter(Boolean), author: fields[i + 2], date: fields[i + 3], subject: fields[i + 4], body: fields[i + 5] });
                }
                return { commits };
            }
            case 'diff': return { diff: await this.diff(repo, request, current) };
            case 'stage':
            case 'unstage': {
                if (request.patch) {
                    if (request.patch.length > 2 * 1024 * 1024)
                        throw new Error('Patch too large');
                    const diff = await this.diff(repo, { kind: 'diff', mode: request.kind === 'stage' ? 'working' : 'staged' }, current);
                    if (diff.truncated)
                        throw new Error('Truncated diffs cannot be staged by hunk');
                    let full = diff.patch;
                    if (request.kind === 'stage') {
                        // Only the untracked paths the submitted patch actually
                        // names. Previewing EVERY untracked file costs two git
                        // spawns each, sequentially, before validation even runs
                        // — measured at 411 spawns / 3.1s for 200 untracked
                        // files, all of it discarded when the patch then fails
                        // to match. A freshly `init`ed project with no
                        // .gitignore yet is a first-class state here (the panel
                        // offers `init`), so "untracked" is routinely the whole
                        // node_modules tree.
                        const named = patchedUntrackedPaths(request.patch);
                        for (const file of current.files.filter(f => f.untracked && (!named || named(f.path)))) {
                            const preview = await this.diff(repo, { kind: 'diff', mode: 'working', path: file.path }, current);
                            if (!preview.truncated)
                                full += preview.patch;
                        }
                    }
                    await this.validatePatch(repo, request.patch, full);
                    await this.git(cwd, ['apply', '--cached', ...(request.kind === 'unstage' ? ['--reverse'] : []), '--check', '-'], request.patch);
                    await this.git(cwd, ['apply', '--cached', ...(request.kind === 'unstage' ? ['--reverse'] : []), '-'], request.patch);
                }
                else {
                    const requested = request.paths ?? current.files.filter(f => !f.conflict).map(f => f.path);
                    const renamePaths = requested.flatMap(name => { const file = current.files.find(file => file.path === name); return file?.oldPath ? [file.oldPath, name] : [name]; });
                    const selected = await paths([...new Set(renamePaths)]);
                    if (!selected.length)
                        throw new Error('No files selected');
                    if (request.kind === 'stage')
                        await this.git(cwd, ['add', '--', ...selected]);
                    else if (current.head)
                        await this.git(cwd, ['restore', '--staged', '--', ...selected]);
                    else
                        await this.git(cwd, ['rm', '--cached', '--ignore-unmatch', '--', ...selected]);
                }
                break;
            }
            case 'discard': {
                const selected = await paths(request.paths);
                this.assertActive();
                if (!selected.length)
                    throw new Error('No files selected');
                for (const p of selected) {
                    const file = current.files.find(f => f.path === p);
                    if (!file || file.untracked !== request.untracked || file.conflict)
                        throw new Error('The selected discard scope no longer matches');
                }
                if (request.untracked) {
                    // `recursive` because `git status -uall` does NOT descend into
                    // an untracked nested repository — it reports the whole thing
                    // as one entry with a trailing slash (`?? vendor/thing/`), and
                    // a non-recursive rm rejects that with EISDIR.
                    for (const p of selected)
                        await this.remove(path.join(cwd, p), { force: true, recursive: true });
                }
                else
                    await this.git(cwd, ['restore', '--worktree', '--', ...selected]);
                break;
            }
            case 'commit':
                if (!request.message.trim())
                    throw new Error('Enter a commit message');
                if (current.files.some(f => f.conflict))
                    throw new Error('Resolve conflicts before committing');
                if (!current.files.some(f => f.index !== ' ' && f.index !== '?'))
                    throw new Error('No staged changes');
                await this.git(cwd, ['commit', '--file=-'], request.message);
                break;
            case 'checkout': {
                if (current.files.length)
                    throw new Error('Commit or explicitly stash your changes before switching branches');
                if (request.create) {
                    await this.git(cwd, ['check-ref-format', '--branch', request.branch]);
                    await this.git(cwd, ['switch', '-c', request.branch]);
                }
                else {
                    await this.ref(repo, request.branch);
                    if (request.branch.startsWith('-'))
                        throw new Error('Invalid branch');
                    const remoteRef = await this.git(cwd, ['show-ref', '--verify', `refs/remotes/${request.branch}`], undefined, true);
                    if (remoteRef.code === 0)
                        await this.git(cwd, ['switch', '--track', request.branch]);
                    else
                        await this.git(cwd, ['switch', request.branch]);
                }
                break;
            }
            case 'fetch': {
                const remote = request.remote || current.remotes[0];
                if (!remote || !current.remotes.includes(remote))
                    throw new Error('Select a configured remote');
                await this.git(cwd, ['fetch', '--', remote]);
                break;
            }
            case 'pull':
            case 'push': {
                if (!current.remotes.includes(request.remote) || request.remote.startsWith('-'))
                    throw new Error('Select a configured remote');
                await this.git(cwd, ['check-ref-format', '--branch', request.branch]);
                if (request.kind === 'pull') {
                    if (current.files.length)
                        throw new Error('Commit or stash changes before pulling');
                    await this.git(cwd, ['pull', '--ff-only', '--no-rebase', '--', request.remote, request.branch]);
                }
                else
                    await this.git(cwd, ['push', ...(request.setUpstream ? ['--set-upstream'] : []), '--', request.remote, `HEAD:refs/heads/${request.branch}`]);
                break;
            }
            case 'merge': {
                if (current.files.length || current.merging)
                    throw new Error('Commit or stash changes before merging');
                const ref = await this.ref(repo, request.branch);
                this.ownedMerges.set(cwd, ref);
                try {
                    await this.git(cwd, ['merge', '--no-edit', '--no-ff', ref]);
                }
                catch (error) {
                    this.events.emit('change', { root: cwd });
                    throw error;
                }
                break;
            }
            case 'mergeContinue':
                if (!current.mergeOwned || !current.merging)
                    throw new Error('This merge was not started in this application');
                if (current.files.some(f => f.conflict))
                    throw new Error('Mark all conflicts resolved first');
                if (!request.message.trim())
                    throw new Error('Enter a merge commit message');
                await this.git(cwd, ['commit', '--file=-'], request.message);
                this.ownedMerges.delete(cwd);
                break;
            case 'mergeAbort':
                if (!current.mergeOwned || !current.merging)
                    throw new Error('This merge was not started in this application');
                await this.git(cwd, ['merge', '--abort']);
                this.ownedMerges.delete(cwd);
                break;
            case 'stashCreate':
                await this.git(cwd, ['stash', 'push', ...(request.includeUntracked ? ['--include-untracked'] : []), '--message', request.message || 'Desktop stash']);
                break;
            case 'stashList': return { stashes: await this.stashes(repo) };
            case 'stashApply':
            case 'stashDrop': {
                const stash = (await this.stashes(repo)).find(s => s.id === request.id);
                if (!stash)
                    throw new Error('Stash changed. Refresh and retry');
                await this.git(cwd, ['stash', request.kind === 'stashApply' ? 'apply' : 'drop', request.kind === 'stashApply' ? stash.id : stash.ref]);
                break;
            }
            case 'conflictRead': {
                const p = await this.checkedPath(repo, request.path);
                if (!current.files.some(f => f.path === p && f.conflict))
                    throw new Error('File is not conflicted');
                const parts = await Promise.all([1, 2, 3].map(n => this.git(cwd, ['show', `:${n}:${p}`], undefined, true)));
                if ((await lstat(path.join(cwd, p)).catch(() => null))?.isSymbolicLink())
                    throw new Error('Resolve symbolic links using the terminal');
                const result = await readFile(path.join(cwd, p)).catch(() => Buffer.alloc(0));
                return { conflict: { path: p, baseExists: parts[0].code === 0, oursExists: parts[1].code === 0, theirsExists: parts[2].code === 0, base: parts[0].stdout.toString(), ours: parts[1].stdout.toString(), theirs: parts[2].stdout.toString(), result: result.toString(), binary: [...parts.map(p => p.stdout), result].some(b => b.includes(0)), token: current.token } };
            }
            case 'conflictSave': {
                const p = await this.checkedPath(repo, request.path);
                if (!current.files.some(f => f.path === p && f.conflict))
                    throw new Error('File is not conflicted');
                const stat = await lstat(path.join(cwd, p)).catch(() => null);
                if (stat?.isSymbolicLink())
                    throw new Error('Resolve symbolic links using the terminal');
                if (request.side) {
                    const version = await this.git(cwd, ['show', `:${request.side === 'ours' ? 2 : 3}:${p}`], undefined, true);
                    if (version.code !== 0)
                        await this.remove(path.join(cwd, p), { force: true });
                    else
                        await this.write(path.join(cwd, p), version.stdout);
                }
                else {
                    if (typeof request.content !== 'string' || request.content.length > 8 * 1024 * 1024)
                        throw new Error('Invalid conflict content');
                    const conflict = await this.perform(repo, { kind: 'conflictRead', path: p });
                    if (conflict.conflict?.binary)
                        throw new Error('Binary conflicts require choosing a version');
                    await this.write(path.join(cwd, p), request.content);
                }
                break;
            }
            case 'conflictResolve': {
                const p = await this.checkedPath(repo, request.path);
                if (!current.files.some(f => f.path === p && f.conflict))
                    throw new Error('File is not conflicted');
                await this.git(cwd, ['add', '--', p]);
                break;
            }
            default: throw new Error('Unsupported Git operation');
        }
        return { status: await this.status(repo) };
    }
    private async stashes(repo: Repository) { const raw = await this.text(repo.root, ['stash', 'list', '--format=%H%x00%gd%x00%gs%x00%aI']); return raw.split('\n').filter(Boolean).map(line => { const [id, ref, subject, date] = line.split('\0'); return { id, ref, subject, date }; }); }
    private async diff(repo: Repository, request: Extract<GitRequest, {
        kind: 'diff';
    }>, current: GitStatus): Promise<GitDiff> {
        let untrackedStash: string | undefined;
        const args = ['diff', '--no-ext-diff', '--no-textconv', '--find-renames', '--no-color'];
        if (request.mode === 'staged')
            args.push('--cached');
        if (request.mode === 'branch') {
            const base = await this.ref(repo, request.base || 'HEAD'), target = await this.ref(repo, request.target || 'HEAD');
            const ancestor = await this.text(repo.root, ['merge-base', base, target]);
            args.push(ancestor, target);
        }
        if (request.mode === 'commit') {
            const ref = await this.ref(repo, request.target || 'HEAD');
            args.splice(0, args.length, 'show', '--format=', '--no-ext-diff', '--no-textconv', '--no-color', ref);
        }
        if (request.mode === 'stash') {
            const stash = (await this.stashes(repo)).find(s => s.id === request.target || s.ref === request.target);
            if (!stash)
                throw new Error('Stash not found');
            args.push(`${stash.id}^1`, stash.id);
            const third = await this.git(repo.root, ['rev-parse', '--verify', `${stash.id}^3`], undefined, true);
            if (third.code === 0) untrackedStash = third.stdout.toString().trim();
        }
        if (request.path) {
            await this.checkedPath(repo, request.path);
            args.push('--', request.path);
        }
        let result = await this.git(repo.root, args, undefined, false, 2 * 1024 * 1024);
        if (request.mode === 'working' && request.path && current.files.some(f => f.path === request.path && f.untracked)) {
            result = await this.git(repo.root, ['diff', '--no-index', '--no-ext-diff', '--no-textconv', '--no-color', '--', process.platform === 'win32' ? 'NUL' : '/dev/null', request.path], undefined, true, 2 * 1024 * 1024);
        }
        const extraArgs = untrackedStash ? ['show', '--format=', '--root', '--no-ext-diff', '--no-textconv', '--no-color', untrackedStash, ...(request.path ? ['--', request.path] : [])] : undefined;
        if (extraArgs) { const extra = await this.git(repo.root, extraArgs, undefined, false, 2 * 1024 * 1024); const combined = Buffer.concat([result.stdout, extra.stdout]); result = { ...result, stdout: combined.subarray(0, 2 * 1024 * 1024), truncated: result.truncated || extra.truncated || combined.length > 2 * 1024 * 1024 }; }
        const patch = result.stdout.toString();
        let files = current.files;
        if (request.mode === 'branch' || request.mode === 'commit' || request.mode === 'stash') {
            const nameArgs = args.filter(x => !['--no-color', '--find-renames'].includes(x));
            nameArgs.splice(1, 0, '--numstat', '-z');
            let rawData = (await this.git(repo.root, nameArgs)).stdout;
            if (extraArgs) { const stats = [...extraArgs]; stats.splice(1, 0, '--numstat', '-z'); rawData = Buffer.concat([rawData, (await this.git(repo.root, stats)).stdout]); }
            const raw = rawData.toString().split('\0');
            files = [];
            for (let i = 0; i < raw.length; i++) {
                const match = /^(\d+|-)\t(\d+|-)\t([\s\S]*)$/.exec(raw[i]);
                if (!match)
                    continue;
                let p = match[3], oldPath;
                if (!p) {
                    oldPath = raw[++i];
                    p = raw[++i];
                }
                files.push({ path: p, oldPath, index: ' ', working: 'M', untracked: false, conflict: false, additions: Number(match[1]) || 0, deletions: Number(match[2]) || 0, binary: match[1] === '-' });
            }
        }
        return { patch, truncated: result.truncated, binary: /^Binary files |^GIT binary patch/m.test(patch), token: current.token, files: request.path ? files.filter(f => f.path === request.path) : files };
    }
    private async validatePatch(repo: Repository, patch: string, full: string): Promise<void> {
        // Only accept complete hunks copied from the current Git diff, including exact file headers.
        const sections = (value: string) => value.split(/(?=^diff --git )/m).filter(Boolean);
        const available = sections(full);
        for (const section of sections(patch)) {
            const header = section.split(/^@@/m)[0];
            const source = available.find(s => s.startsWith(header));
            if (!source || !header.startsWith('diff --git '))
                throw new Error('Patch does not match current diff');
            const hunks = section.slice(header.length).split(/(?=^@@ )/m).filter(Boolean);
            if (!hunks.length || hunks.some(h => !source.includes(h)))
                throw new Error('Only complete current hunks may be staged');
        }
        if (!patch.trim())
            throw new Error('Empty patch');
    }
}
