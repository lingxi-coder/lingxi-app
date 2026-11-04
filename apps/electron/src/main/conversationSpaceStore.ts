import { lstatSync, mkdirSync, readFileSync, realpathSync, renameSync, writeFileSync } from 'node:fs';
import { homedir } from 'node:os';
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

  constructor(userData: string, brandHome = process.env.LINGXI_CONFIG_DIR || join(homedir(), '.lingxi')) {
    this.file = join(userData, 'conversation-spaces.v2.json');
    if (!isAbsolute(brandHome)) throw new Error('The brand workspace must be an absolute path.');
    mkdirSync(brandHome, { recursive: true, mode: 0o700 });
    const container = lstatSync(brandHome);
    if (!container.isDirectory() || container.isSymbolicLink()) throw new Error('The brand container must be a real directory.');
    const workspace = (space: ConversationSpace): string => {
      const directory = join(brandHome, space);
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

  private update(change: (draft: StoredSpaces) => void): void {
    const next = structuredClone(this.value);
    change(next);
    const checked = decode(next);
    mkdirSync(dirname(this.file), { recursive: true, mode: 0o700 });
    const temporary = `${this.file}.tmp`;
    writeFileSync(temporary, `${JSON.stringify(checked, null, 2)}\n`, { mode: 0o600 });
    renameSync(temporary, this.file);
    this.value = checked;
  }

  addProject(project: string, space: ConversationSpace): void {
    if (!path(project) || !mode(space)) throw new Error('Invalid conversation project.');
    if (this.value.projects[project]?.includes(space)) return;
    this.update((draft) => { draft.projects[project] = [...(draft.projects[project] ?? []), space]; });
  }

  removeProject(project: string, space?: ConversationSpace): void {
    if (!this.value.projects[project]) return;
    this.update((draft) => {
      const remaining = space ? draft.projects[project]!.filter((entry) => entry !== space) : [];
      if (remaining.length) draft.projects[project] = remaining;
      else {
        delete draft.projects[project];
      }
      for (const item of ['chat', 'code'] as const) {
        if ((!space || item === space) && draft.navigation[item].projectId === project) draft.navigation[item] = { projectId: null };
      }
    });
  }

  trustedWorkspaces(): string[] { return [...this.value.trustedWorkspaces]; }

  trustWorkspace(workspace: string): void {
    if (!path(workspace)) throw new Error('Invalid trusted workspace.');
    if (!this.value.trustedWorkspaces.includes(workspace)) this.update((draft) => { draft.trustedWorkspaces.push(workspace); });
  }

  find(ref: Pick<OwnedConversation, 'projectPath' | 'sessionId'>): OwnedConversation | undefined {
    const match = this.value.sessions.find((entry) => same(entry, ref));
    return match ? { ...match } : undefined;
  }

  findBySessionId(sessionId: string): OwnedConversation | undefined {
    const match = this.value.sessions.find((entry) => entry.sessionId === sessionId.toLowerCase());
    return match ? structuredClone(match) : undefined;
  }

  register(value: OwnedConversation): void {
    const next = owner(value);
    const existing = this.find(next) ?? this.findBySessionId(next.sessionId);
    if (existing) {
      if (existing.mode !== next.mode || existing.projectId !== next.projectId || existing.projectPath !== next.projectPath) {
        throw new Error('Conversation ownership cannot change.');
      }
      return;
    }
    this.update((draft) => { draft.sessions.push(next); });
  }

  relocate(ref: Pick<OwnedConversation, 'projectPath' | 'sessionId'>, workspace: string): OwnedConversation {
    if (!path(workspace)) throw new Error('Invalid conversation workspace.');
    const existing = this.find(ref);
    if (!existing) throw new Error('Conversation ownership is unavailable.');
    const moved = { ...existing, projectPath: workspace };
    if (this.find(moved) && !same(existing, moved)) throw new Error('Conversation workspace is already owned.');
    this.update((draft) => {
      draft.sessions = draft.sessions.map((entry) => same(entry, existing) ? moved : entry);
      for (const space of ['chat', 'code'] as const) {
        const navigation = draft.navigation[space];
        if (navigation.session && same(navigation.session, existing)) navigation.session = moved;
      }
    });
    return moved;
  }

  setMode(space: ConversationSpace): void {
    if (!mode(space)) throw new Error('Invalid conversation space.');
    if (this.value.activeMode !== space) this.update((draft) => { draft.activeMode = space; });
  }

  setNavigation(space: ConversationSpace, projectId: string | null, session?: OwnedConversation): void {
    if (!mode(space) || !(projectId === null || path(projectId))) throw new Error('Invalid conversation navigation.');
    if (session) {
      const registered = this.find(session);
      if (!registered || registered.mode !== space || registered.projectId !== projectId) throw new Error('Conversation owner does not match navigation.');
      session = registered;
    }
    this.update((draft) => { draft.navigation[space] = { projectId, ...(session ? { session } : {}) }; });
  }
}
