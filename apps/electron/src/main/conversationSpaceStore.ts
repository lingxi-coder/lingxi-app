import { lstatSync, mkdirSync, readFileSync, realpathSync, renameSync, unlinkSync } from 'node:fs';
import { chmod, copyFile, mkdir, rename, rm, writeFile } from 'node:fs/promises';
import { dirname, isAbsolute, join } from 'node:path';
import type { ConversationSpace, ConversationSpacesSnapshot, OwnedConversation, SpaceNavigation } from '../shared/conversationSpaces.js';
import { isSessionId } from './sessionIdentity.js';

interface StoredSpaces {
  version: 2;
  activeMode: ConversationSpace;
  projects: Record<string, ConversationSpace[]>;
  trustedWorkspaces: string[];
  navigation: Record<ConversationSpace, SpaceNavigation>;
  sessions: OwnedConversation[];
}

const mode = (value: unknown): value is ConversationSpace => value === 'chat' || value === 'code';
const path = (value: unknown): value is string => typeof value === 'string' && value.length > 0
  && value.length <= 32_768 && !value.includes('\0') && isAbsolute(value);

function owner(value: unknown): OwnedConversation {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid conversation owner.');
  const row = value as Record<string, unknown>;
  if (!mode(row.mode) || !isSessionId(row.sessionId) || !path(row.projectPath)
    || !(row.projectId === null || path(row.projectId))) throw new Error('Invalid conversation owner.');
  let handoff: OwnedConversation['handoff'];
  if (row.handoff !== undefined) {
    if (!row.handoff || typeof row.handoff !== 'object' || Array.isArray(row.handoff)) throw new Error('Invalid handoff backlink.');
    const link = row.handoff as Record<string, unknown>;
    if (!mode(link.sourceMode) || link.sourceMode === row.mode || !isSessionId(link.sourceSessionId)
      || !path(link.sourceProjectPath) || typeof link.cutoff !== 'string' || !link.cutoff || link.cutoff.length > 4096
      || typeof link.requestId !== 'string' || !link.requestId || link.requestId.length > 128) throw new Error('Invalid handoff backlink.');
    handoff = { sourceSessionId: link.sourceSessionId.toLowerCase(), sourceProjectPath: link.sourceProjectPath,
      sourceMode: link.sourceMode, cutoff: link.cutoff, requestId: link.requestId };
  }
  return { mode: row.mode, projectId: row.projectId, projectPath: row.projectPath, sessionId: row.sessionId.toLowerCase(),
    ...(handoff ? { handoff } : {}) };
}

function same(left: Pick<OwnedConversation, 'projectPath' | 'sessionId'>, right: Pick<OwnedConversation, 'projectPath' | 'sessionId'>): boolean {
  return left.projectPath === right.projectPath && left.sessionId.toLowerCase() === right.sessionId.toLowerCase();
}

function empty(): StoredSpaces {
  return { version: 2, activeMode: 'chat', projects: {}, trustedWorkspaces: [], sessions: [],
    navigation: { chat: { projectId: null }, code: { projectId: null } } };
}

function decode(value: unknown): StoredSpaces {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Invalid conversation spaces.');
  const input = value as Record<string, unknown>;
  const keys = new Set(['version', 'activeMode', 'projects', 'trustedWorkspaces', 'sessions', 'navigation']);
  if (Object.keys(input).some((key) => !keys.has(key))
    || input.version !== 2 || !mode(input.activeMode) || !input.projects || typeof input.projects !== 'object'
    || Array.isArray(input.projects) || !Array.isArray(input.trustedWorkspaces) || !Array.isArray(input.sessions) || !input.navigation
    || typeof input.navigation !== 'object' || Array.isArray(input.navigation)) throw new Error('Invalid conversation spaces.');
  const projects: Record<string, ConversationSpace[]> = {};
  for (const [project, value] of Object.entries(input.projects)) {
    if (!path(project) || !Array.isArray(value) || value.length === 0 || value.some((entry) => !mode(entry))) {
      throw new Error('Invalid conversation project membership.');
    }
    projects[project] = [...new Set(value as ConversationSpace[])];
  }
  if (input.trustedWorkspaces.some((workspace) => !path(workspace))
    || new Set(input.trustedWorkspaces).size !== input.trustedWorkspaces.length) throw new Error('Invalid trusted workspaces.');
  const sessions = input.sessions.map(owner);
  if (new Set(sessions.map((entry) => entry.sessionId)).size !== sessions.length) {
    throw new Error('Duplicate conversation owner.');
  }
  const navigation = {} as Record<ConversationSpace, SpaceNavigation>;
  for (const space of ['chat', 'code'] as const) {
    const raw = (input.navigation as Record<string, unknown>)[space];
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) throw new Error('Invalid conversation navigation.');
    const row = raw as Record<string, unknown>;
    if (!(row.projectId === null || path(row.projectId))) throw new Error('Invalid conversation navigation.');
    const session = row.session === undefined ? undefined : owner(row.session);
    if (session && (session.mode !== space || session.projectId !== row.projectId
      || !sessions.some((candidate) => same(candidate, session) && candidate.mode === session.mode && candidate.projectId === session.projectId))) {
      throw new Error('Conversation navigation does not match its owner.');
    }
    navigation[space] = { projectId: row.projectId, ...(session ? { session } : {}) };
  }
  return { version: 2, activeMode: input.activeMode, projects,
    trustedWorkspaces: input.trustedWorkspaces as string[], navigation, sessions };
}

/** Device-local project visibility and immutable logical conversation ownership. */
export class ConversationSpaceStore {
  readonly globalWorkspaces: Readonly<Record<ConversationSpace, string>>;
  private readonly file: string;
  private value: StoredSpaces;
  private pending: Promise<void> = Promise.resolve();

  constructor(userData: string, workspaceRoot = join(userData, 'workspaces')) {
    this.file = join(userData, 'conversation-spaces.v2.json');
    if (!isAbsolute(workspaceRoot)) throw new Error('The brand workspace must be an absolute path.');
    mkdirSync(workspaceRoot, { recursive: true, mode: 0o700 });
    const container = lstatSync(workspaceRoot);
    if (!container.isDirectory() || container.isSymbolicLink()) throw new Error('The brand container must be a real directory.');
    const workspace = (space: ConversationSpace): string => {
      const directory = join(workspaceRoot, space);
      mkdirSync(directory, { recursive: true, mode: 0o700 });
      const stat = lstatSync(directory);
      if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error('The conversation workspace must be a real directory.');
      return realpathSync.native(directory);
    };
    this.globalWorkspaces = { chat: workspace('chat'), code: workspace('code') };
    try { this.value = decode(JSON.parse(readFileSync(this.file, 'utf8'))); }
    catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
      this.value = empty();
    }
  }

  snapshot(): ConversationSpacesSnapshot {
    const { trustedWorkspaces: _trustedWorkspaces, ...publicValue } = structuredClone(this.value);
    return { ...publicValue, globalWorkspaces: { ...this.globalWorkspaces } };
  }

  private transaction<T>(operation: () => Promise<T>): Promise<T> {
    const pending = this.pending.then(operation);
    this.pending = pending.then(() => undefined, () => undefined);
    return pending;
  }

  /** Wait for all admitted ownership/navigation writes, including failed transactions. */
  flush(): Promise<void> { return this.pending; }

  private async stage(value: StoredSpaces): Promise<string> {
    await mkdir(dirname(this.file), { recursive: true, mode: 0o700 });
    const temporary = `${this.file}.tmp`;
    try {
      await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, { mode: 0o600 });
      await chmod(temporary, 0o600);
      return temporary;
    } catch (error) {
      await rm(temporary, { force: true }).catch(() => undefined);
      throw error;
    }
  }

  private async write(value: StoredSpaces): Promise<void> {
    const temporary = await this.stage(value);
    try { await rename(temporary, this.file); }
    catch (error) { await rm(temporary, { force: true }).catch(() => undefined); throw error; }
  }

  /** Called only within the transaction queue; failed writes never publish a new owner. */
  private async update(change: (draft: StoredSpaces) => void): Promise<void> {
    const next = structuredClone(this.value);
    change(next);
    const checked = decode(next);
    await this.write(checked);
    this.value = checked;
  }

  addProject(project: string, space: ConversationSpace): Promise<void> {
    return this.transaction(async () => {
      if (!path(project) || !mode(space)) throw new Error('Invalid conversation project.');
      if (this.value.projects[project]?.includes(space)) return;
      await this.update((draft) => { draft.projects[project] = [...(draft.projects[project] ?? []), space]; });
    });
  }

  removeProject(project: string, space?: ConversationSpace): Promise<void> {
    return this.transaction(async () => {
      if (!this.value.projects[project]) return;
      await this.update((draft) => {
        const remaining = space ? draft.projects[project]!.filter((entry) => entry !== space) : [];
        if (remaining.length) draft.projects[project] = remaining;
        else delete draft.projects[project];
        for (const item of ['chat', 'code'] as const) {
          if ((!space || item === space) && draft.navigation[item].projectId === project) draft.navigation[item] = { projectId: null };
        }
      });
    });
  }

  trustedWorkspaces(): string[] { return [...this.value.trustedWorkspaces]; }

  trustWorkspace(workspace: string): Promise<void> {
    return this.transaction(async () => {
      if (!path(workspace)) throw new Error('Invalid trusted workspace.');
      if (!this.value.trustedWorkspaces.includes(workspace)) await this.update((draft) => { draft.trustedWorkspaces.push(workspace); });
    });
  }

  find(ref: Pick<OwnedConversation, 'projectPath' | 'sessionId'>): OwnedConversation | undefined {
    const match = this.value.sessions.find((entry) => same(entry, ref));
    return match ? structuredClone(match) : undefined;
  }

  findBySessionId(sessionId: string): OwnedConversation | undefined {
    const match = this.value.sessions.find((entry) => entry.sessionId === sessionId.toLowerCase());
    return match ? structuredClone(match) : undefined;
  }

  register(value: OwnedConversation): Promise<void> {
    const requested = structuredClone(value);
    return this.transaction(async () => {
      const next = owner(requested);
      const existing = this.find(next) ?? this.findBySessionId(next.sessionId);
      if (existing) {
        if (existing.mode !== next.mode || existing.projectId !== next.projectId || existing.projectPath !== next.projectPath) {
          throw new Error('Conversation ownership cannot change.');
        }
        return;
      }
      await this.update((draft) => { draft.sessions.push(next); });
    });
  }

  relocate(ref: Pick<OwnedConversation, 'projectPath' | 'sessionId'>, workspace: string): Promise<OwnedConversation> {
    const requested = { ...ref };
    return this.transaction(async () => {
      if (!path(workspace)) throw new Error('Invalid conversation workspace.');
      const existing = this.find(requested);
      if (!existing) throw new Error('Conversation ownership is unavailable.');
      const moved = { ...existing, projectPath: workspace };
      if (this.find(moved) && !same(existing, moved)) throw new Error('Conversation workspace is already owned.');
      if (same(existing, moved)) return moved;
      await this.update((draft) => {
        draft.sessions = draft.sessions.map((entry) => same(entry, existing) ? moved : entry);
        for (const space of ['chat', 'code'] as const) {
          const navigation = draft.navigation[space];
          if (navigation.session && same(navigation.session, existing)) navigation.session = moved;
        }
      });
      return moved;
    });
  }

  /** Stage disk work before a synchronous authority fence publishes selection and host metadata together. */
  select<T>(ref: Pick<OwnedConversation, 'projectPath' | 'sessionId'>, workspace: string,
    commit: (owner: OwnedConversation) => T, authorize: (publish: () => T) => Promise<T>): Promise<T> {
    const requested = { ...ref };
    return this.transaction(async () => {
      if (!path(workspace)) throw new Error('Invalid conversation workspace.');
      const existing = this.find(requested);
      if (!existing) throw new Error('Conversation ownership is unavailable.');
      const moved = { ...existing, projectPath: workspace };
      const navigation = this.value.navigation[existing.mode];
      if (same(existing, moved) && this.value.activeMode === existing.mode
        && navigation.projectId === existing.projectId && navigation.session && same(navigation.session, moved)) {
        return authorize(() => commit(moved));
      }
      const previous = this.value;
      const next = structuredClone(previous);
      next.sessions = next.sessions.map((entry) => same(entry, existing) ? moved : entry);
      next.activeMode = moved.mode;
      for (const space of ['chat', 'code'] as const) {
        if (next.navigation[space].session && same(next.navigation[space].session!, existing)) next.navigation[space].session = moved;
      }
      next.navigation[moved.mode] = { projectId: moved.projectId, session: moved };
      const checked = decode(next);
      const temporary = await this.stage(checked);
      const backup = `${this.file}.rollback`;
      let hadFile = true, preserveBackup = false;
      try {
        try { await copyFile(this.file, backup); }
        catch (error) { if ((error as NodeJS.ErrnoException).code === 'ENOENT') hadFile = false; else throw error; }
        return await authorize(() => {
          // This short publication stays synchronous so authority cannot be
          // revoked between its final check, ownership and runtime activation.
          renameSync(temporary, this.file);
          this.value = checked;
          try { return commit(structuredClone(moved)); }
          catch (error) {
            try {
              if (hadFile) renameSync(backup, this.file);
              else unlinkSync(this.file);
              this.value = previous;
            } catch (rollbackError) {
              preserveBackup = true;
              throw new AggregateError([error, rollbackError], 'Conversation selection failed and its previous registry could not be restored.');
            }
            throw error;
          }
        });
      } finally {
        await Promise.all((preserveBackup ? [temporary] : [temporary, backup]).map(file => rm(file, { force: true }).catch(() => undefined)));
      }
    });
  }

  setMode(space: ConversationSpace): Promise<void> {
    return this.transaction(async () => {
      if (!mode(space)) throw new Error('Invalid conversation space.');
      if (this.value.activeMode !== space) await this.update((draft) => { draft.activeMode = space; });
    });
  }

  setNavigation(space: ConversationSpace, projectId: string | null, session?: OwnedConversation): Promise<void> {
    const requested = session && structuredClone(session);
    return this.transaction(async () => {
      if (!mode(space) || !(projectId === null || path(projectId))) throw new Error('Invalid conversation navigation.');
      const registered = requested && this.find(requested);
      if (requested && (!registered || registered.mode !== space || registered.projectId !== projectId)) {
        throw new Error('Conversation owner does not match navigation.');
      }
      const current = this.value.navigation[space];
      if (current.projectId === projectId && (!current.session && !registered
        || current.session && registered && same(current.session, registered))) return;
      await this.update((draft) => { draft.navigation[space] = { projectId, ...(registered ? { session: registered } : {}) }; });
    });
  }
}
