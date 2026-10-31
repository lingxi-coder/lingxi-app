import type { BindLocalTeamBotInput, LocalTeamBot, TeamBotCard } from '../../../../../packages/teams/bots.js';
import type { TeamSummary } from '../../../../../packages/teams/index.js';
import type { TeamConversation } from '../../../../../packages/teams/messages.js';
import type { BotContent } from '../bots/content.js';
import type { BotStore, BotTeamBinding } from '../bots/store.js';
import { type PendingTeamTarget, TeamPrivateStore, validateTeamModelRoute, type TeamModelSelection, type TeamPersonaSnapshot } from './privateStore.js';
import { runTeamHost, TeamHostFailure, type TeamBotTarget, type TeamHostProcessOptions, type TeamHostResult } from './process.js';
import type { LocalTeamHost, TeamAccountContext } from './service.js';

export interface TeamHostApi {
  hostContext(): TeamAccountContext | undefined;
  assertHostContext(context: TeamAccountContext): void;
  hostToken(context: TeamAccountContext): Promise<string>;
  hostMessages<T>(context: TeamAccountContext, command: Record<string, unknown>): Promise<T>;
  list(uid: string): Promise<{ teams: TeamSummary[] }>;
  onChanged(callback: (event: { invalidated?: boolean }) => void): () => void;
  onMessagesChanged(callback: () => void): () => void;
  notifyHostChanged?(): void;
}
export interface TeamConnectorOptions {
  service: TeamHostApi; store: BotStore; content: Pick<BotContent, 'teamInstructions'>; privateStore: TeamPrivateStore;
  binaries(): { host: string; worker: string };
  defaultModel(): string | undefined;
  /** Resolve exactly one selectable native catalog route, including its static key slot. */
  modelRoute(selected: string): Promise<TeamModelSelection>;
  /** Only the frozen route's static key. No account/OAuth/ambient credentials. */
  modelCredential(credentialId: string): Promise<string | undefined>;
  permission(input: Parameters<TeamHostProcessOptions['permission']>[0] & { accountEpoch: number; assertCurrent(): void }, signal: AbortSignal): Promise<boolean>;
  platform?: NodeJS.Platform;
  run?: (options: TeamHostProcessOptions) => Promise<TeamHostResult>;
  retryMs?: number;
  now?: () => number;
  maxConcurrent?: number;
}
interface QueuePage { cursor: number; triggers: Array<{ id: string; seq: number }>; hasMore: boolean }
interface Active { signature: string; controller: AbortController; promise: Promise<void>; ready: Promise<void>; settling?: boolean }
function scopeKey(teamId: string, botKey: string): string {
  return JSON.stringify([teamId, botKey]);
}

/** Discovery is a hint. The transactional server queue and Rust journal own execution. */
export class TeamConnector implements LocalTeamHost {
  private closed = false;
  private pending = false;
  private refresh?: Promise<void>;
  private readonly active = new Map<string, Active>();
  private readonly blocked = new Map<string, string>();
  private readonly slots = new Set<Active>();
  private readonly scans = new Map<string, string>();
  private readonly emptyQueues = new Map<string, string>();
  private readonly observations = new Map<string, { triggerId: string; delay: number; until: number }>();
  private readonly scheduled = new Map<string, number>();
  private admissions = 0;
  private readonly heartbeats = new Map<string, number>();
  private readonly statuses = new Map<string, string>();
  private readonly bindingOperations = new Set<string>();
  private readonly stops: Array<() => void>;
  private timer?: ReturnType<typeof setTimeout>;
  private failedDiscovery = false;
  private revision = 0;
  private notifyingStatus = false;
  constructor(private readonly options: TeamConnectorOptions) {
    this.stops = [options.service.onChanged(event => {
      if (event.invalidated) this.cancelAll();
      this.wake();
    }), options.service.onMessagesChanged(() => { if (!this.notifyingStatus) { this.observations.clear(); this.wake(); } })];
    this.wake();
  }
  private available(): void { if ((this.options.platform ?? process.platform) !== 'darwin') throw new Error('TEAM_HOST_MACOS_REQUIRED'); }
  async localBots(uid: string): Promise<LocalTeamBot[]> {
    this.available(); const context = this.context(uid); this.options.service.assertHostContext(context);
    return this.options.store.list().map(bot => ({ botId: bot.bot_id, name: bot.name, description: bot.description, ...(bot.model ? { model: bot.model } : {}) }));
  }
  private context(uid: string): TeamAccountContext {
    const context = this.options.service.hostContext();
    if (!context || context.uid !== uid || this.closed) throw new Error('INVALID_SESSION');
    return context;
  }
  async bindLocalBot(uid: string, input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }> {
    const key = JSON.stringify([uid, input?.teamId, input?.botId]);
    if (this.bindingOperations.has(key)) throw new Error('BOT_REVISION_CONFLICT');
    this.bindingOperations.add(key);
    try { return await this.bind(uid, input); } finally { this.bindingOperations.delete(key); }
  }
  private async bind(uid: string, input: BindLocalTeamBotInput): Promise<{ botKey: string; revision: number }> {
    this.available(); const context = this.context(uid), service = this.options.service;
    if (!input || Object.keys(input).some(key => !['teamId', 'generation', 'botId', 'use', 'uids', 'expectedRevision'].includes(key))
      || !/^[A-Za-z0-9_-]{8,64}$/u.test(input.teamId) || !/^[A-Za-z0-9_-]{22}$/u.test(input.botId)
      || !Number.isSafeInteger(input.generation) || input.generation < 1
      || !Number.isSafeInteger(input.expectedRevision) || input.expectedRevision < 0
      || !['all', 'listed'].includes(input.use) || !Array.isArray(input.uids) || input.uids.length > 200
      || !input.uids.every(value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/u.test(value))
      || new Set(input.uids).size !== input.uids.length) throw new Error('INVALID_INPUT');
    this.options.binaries(); // Surface an unsupported/unpackaged runtime before enabling a card.
    const bot = this.options.store.get(input.botId);
    const botKey = `bot:${uid}:${this.options.privateStore.hostId}:${bot.bot_id}`;
    const cards = await service.hostMessages<{ bots: TeamBotCard[] }>(context, { action: 'bots', teamId: input.teamId, generation: input.generation });
    service.assertHostContext(context);
    if ((cards.bots.find(card => card.key === botKey)?.revision ?? 0) !== input.expectedRevision) throw new Error('BOT_REVISION_CONFLICT');
    const selected = bot.model ?? this.options.defaultModel();
    if (!selected) throw new Error('TEAM_MODEL_UNAVAILABLE');
    const selection = await this.options.modelRoute(selected); service.assertHostContext(context);
    const route = structuredClone(selection.route);
    validateTeamModelRoute(route);
    const instructions = await this.options.content.teamInstructions(input.botId); service.assertHostContext(context);
    // A racing local edit cannot bind a mixture of two Bot configurations.
    if (this.options.store.get(input.botId).revision !== bot.revision) throw new Error('BOT_REVISION_CONFLICT');
    const credential = await this.options.modelCredential(route.credential_id); service.assertHostContext(context);
    if (!credential) throw new Error('TEAM_MODEL_UNAVAILABLE');
    await selection.assertCurrent(); service.assertHostContext(context);
    const snapshot = await this.options.privateStore.saveSnapshot(uid, { instructions, ...route }); service.assertHostContext(context);
    await selection.assertCurrent(); service.assertHostContext(context);
    this.stopBinding(input.teamId, botKey);
    // Save the intended revision first. A lost bind response can then be discovered,
    // while a rejected request never matches the server card and cannot execute.
    const binding: BotTeamBinding = { ownerUid: uid, teamId: input.teamId, generation: input.generation,
      botKey, revision: input.expectedRevision + 1, snapshot, use: input.use, uids: [...input.uids].sort() };
    const current = this.options.store.get(input.botId);
    if (current.revision !== bot.revision) throw new Error('BOT_REVISION_CONFLICT');
    this.options.store.setTeamBindings(bot.bot_id, [...(current.teams ?? []).filter(value => value.ownerUid !== uid || value.teamId !== input.teamId), binding]);
    try {
      const result = await service.hostMessages<{ botKey: string; revision: number }>(context, {
        action: 'bindBot', teamId: input.teamId, generation: input.generation, hostId: this.options.privateStore.hostId,
        botId: bot.bot_id, name: bot.name, description: bot.description, use: input.use, uids: [...input.uids], expectedRevision: input.expectedRevision,
      });
      service.assertHostContext(context);
      if (result.botKey !== botKey || result.revision !== binding.revision) throw new Error('INVALID_BOT_BINDING');
      return result;
    } finally { this.wake(); }
  }
  stopBinding(teamId: string, botKey: string): void {
    const prefix = scopeKey(teamId, botKey);
    for (const [key, active] of this.active) if (key.startsWith(`${prefix}:`)) { active.controller.abort(); this.active.delete(key); }
    this.heartbeats.delete(prefix); this.scans.delete(prefix); this.scheduled.delete(prefix);
    for (const map of [this.emptyQueues, this.observations, this.blocked]) for (const key of map.keys()) if (key.startsWith(`${prefix}:`)) map.delete(key);
  }
  /** Called synchronously before local Bot deletion/settings changes are published. */
  localChanged(): void { this.cancelAll(); this.wake(); }
  invalidate(): void { this.cancelAll(); this.scans.clear(); this.emptyQueues.clear(); this.observations.clear(); this.scheduled.clear(); this.blocked.clear(); this.heartbeats.clear(); this.statuses.clear(); this.wake(); }
  runStatus(uid: string, teamId: string, conversationId: string, triggerId: string): string | undefined {
    this.context(uid); return this.statuses.get(JSON.stringify([uid, teamId, conversationId, triggerId]));
  }
  private status(target: TeamBotTarget, value?: string): void {
    const key = JSON.stringify([target.ownerUid, target.teamId, target.conversationId, target.triggerId]);
    if (this.statuses.get(key) === value) return;
    if (value) this.statuses.set(key, value); else this.statuses.delete(key);
    // The owner UI needs the diagnostic, but this synchronous local echo is
    // not a remote queue change and must not drive another observation.
    this.notifyingStatus = true;
    try { this.options.service.notifyHostChanged?.(); } finally { this.notifyingStatus = false; }
  }
  private cancelAll(): void { this.revision++; for (const active of this.active.values()) active.controller.abort(); this.active.clear(); }
  private wake(): void {
    if (this.closed || (this.options.platform ?? process.platform) !== 'darwin') return;
    this.pending = true;
    if (this.refresh) return;
    this.refresh = (async () => {
      while (this.pending && !this.closed) {
        this.pending = false;
        this.failedDiscovery = false;
        try { await this.discover(); } catch { this.failedDiscovery = true; this.cancelAll(); }
      }
    })().finally(() => { this.refresh = undefined; if (this.pending) this.wake(); else this.schedule(); });
  }
  private schedule(): void {
    clearTimeout(this.timer); this.timer = undefined;
    if (this.closed) return;
    const now = (this.options.now ?? Date.now)();
    const deadlines = [...this.heartbeats.values()].map(value => value + 20000);
    for (const value of this.observations.values()) if (value.until > now) deadlines.push(value.until);
    if (this.scans.size && this.slots.size < (this.options.maxConcurrent ?? 4)) deadlines.push(now + 1);
    if (this.failedDiscovery) deadlines.push(now + (this.options.retryMs ?? 5000));
    if (!deadlines.length) return;
    const delay = this.failedDiscovery ? this.options.retryMs ?? 5000 : Math.max(1, Math.min(...deadlines) - now);
    this.timer = setTimeout(() => this.wake(), delay); this.timer.unref();
  }
  /** Exposed for deterministic lifecycle tests and trusted app shutdown only. */
  async idle(): Promise<void> {
    do {
      await this.refresh;
      await Promise.all([...this.slots].map(value => value.ready));
      await new Promise<void>(resolve => setImmediate(resolve));
      await Promise.all([...this.slots].filter(value => value.settling).map(value => value.promise));
    } while (this.refresh);
  }
  private async discover(): Promise<void> {
    const context = this.options.service.hostContext();
    if (!context) { this.cancelAll(); return; }
    const service = this.options.service, version = this.revision;
    const assert = () => { service.assertHostContext(context); if (this.closed || version !== this.revision) throw new Error('INVALID_SESSION'); };
    const own = this.options.store.list().flatMap(bot => (bot.teams ?? []).filter(binding => binding.ownerUid === context.uid).map(binding => ({ bot, binding })));
    if (!own.length) { this.cancelAll(); return; }
    const pendingTargets = await this.options.privateStore.pending(context.uid); assert();
    const { teams } = await service.list(context.uid); assert();
    type Candidate = { bot: ReturnType<BotStore['list']>[number]; binding: BotTeamBinding; conversation?: TeamConversation; pending?: PendingTeamTarget };
    const groups: Array<{ scope: string; candidates: Candidate[] }> = [];
    const validScopes = new Set<string>();
    for (const team of teams) {
      const local = own.filter(value => value.binding.teamId === team.id && value.binding.generation === team.generation);
      if (!local.length) continue;
      try {
        const { bots } = await service.hostMessages<{ bots: TeamBotCard[] }>(context, { action: 'bots', teamId: team.id, generation: team.generation }); assert();
        for (const { bot, binding } of local) {
          const card = bots.find(card => card.enabled !== false && card.key === binding.botKey && card.ownerUid === context.uid && card.hostId === this.options.privateStore.hostId
            && card.botId === bot.bot_id && card.revision === binding.revision && card.use === binding.use
            && JSON.stringify([...card.uids].sort()) === JSON.stringify(binding.uids));
          if (!card) continue;
          const scope = scopeKey(team.id, binding.botKey), now = (this.options.now ?? Date.now)();
          validScopes.add(scope);
          if (now - (this.heartbeats.get(scope) ?? -Infinity) >= 20_000) {
            await service.hostMessages(context, { action: 'heartbeatBot', teamId: team.id, generation: team.generation, botKey: binding.botKey, revision: binding.revision }); assert();
            this.heartbeats.set(scope, now);
          }
          // One bounded page per Bot per pass. Ordinary human-only threads cannot
          // consume this scan, and a large directory resumes on the next pass.
          const after = this.scans.get(scope);
          const page = await service.hostMessages<{ conversations: TeamConversation[]; next?: string | null }>(context,
            { action: 'botConversations', teamId: team.id, generation: team.generation, botKey: binding.botKey, ...(after ? { after } : {}) }); assert();
          if (!Array.isArray(page.conversations) || page.conversations.length > 100
            || page.next && (!/^[A-Za-z0-9_-]{8,64}$/u.test(page.next) || after && page.next <= after)) throw new Error('INVALID_BOT_QUEUE');
          if (page.next) this.scans.set(scope, page.next); else this.scans.delete(scope);
          const saved = pendingTargets.filter(value => value.target.ownerUid === context.uid && value.target.ownerGeneration === team.generation
            && value.target.teamId === team.id && value.target.botKey === binding.botKey && value.target.hostId === this.options.privateStore.hostId
            && value.target.botId === bot.bot_id && value.target.botRevision === binding.revision && value.snapshot === binding.snapshot);
          const savedIds = new Set(saved.map(value => value.target.conversationId));
          groups.push({ scope, candidates: [
            ...saved.map(pending => ({ bot, binding, pending })),
            ...page.conversations.filter(conversation => conversation.participants.includes(binding.botKey) && !savedIds.has(conversation.id))
              .map(conversation => ({ bot, binding, conversation })),
          ] });
        }
      } catch (error) {
        assert();
        const code = error instanceof Error ? error.message : '';
        if (['INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(code)) throw error;
        // A failed Team cannot cancel independent Teams. Stop only this Team on
        // an authorization/protocol failure; transient HTTP errors defer discovery.
        if (['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'BOT_OWNER_REQUIRED', 'BOT_REVISION_CONFLICT', 'INVALID_BOT_QUEUE'].includes(code)) {
          for (const { binding } of local) { validScopes.delete(scopeKey(team.id, binding.botKey)); this.stopBinding(team.id, binding.botKey); }
        } else { this.failedDiscovery = true; for (const { binding } of local) validScopes.add(scopeKey(team.id, binding.botKey)); }
      }
    }
    assert();
    for (const [key, active] of this.active) if (![...validScopes].some(scope => key.startsWith(`${scope}:`))) active.controller.abort();
    for (const map of [this.scans, this.heartbeats, this.scheduled]) for (const scope of map.keys()) if (!validScopes.has(scope)) map.delete(scope);
    groups.sort((a, b) => (this.scheduled.get(a.scope) ?? 0) - (this.scheduled.get(b.scope) ?? 0));
    const limit = this.options.maxConcurrent ?? 4;
    if (!Number.isSafeInteger(limit) || limit < 1 || limit > 16) throw new Error('INVALID_HOST_CONCURRENCY');
    // Round robin across Bots; the resumable scan rotates their conversations.
    while (groups.some(group => group.candidates.length) && this.slots.size < limit) {
      for (const group of groups) {
        if (!validScopes.has(group.scope)) { group.candidates = []; continue; }
        if (this.slots.size >= limit) break;
        const candidate = group.candidates.shift();
        if (!candidate) continue;
        const { bot, binding, conversation } = candidate, scope = group.scope;
        const id = candidate.pending?.target.conversationId ?? conversation!.id;
        const key = `${scope}:${id}`, signature = JSON.stringify([context.uid, context.epoch, binding]);
        const active = this.active.get(key);
        if (active) { if (active.signature !== signature) active.controller.abort(); continue; }
        let pending = candidate.pending;
        if (pending && !await this.options.privateStore.hasPending(context.uid, pending)) pending = undefined;
        assert();
        const retry = pending !== undefined;
        if (!pending) {
          if (!conversation) continue;
          const queueVersion = JSON.stringify([signature, conversation.lastChangeSeq]);
          if (Number.isSafeInteger(conversation.lastChangeSeq) && this.emptyQueues.get(key) === queueVersion) continue;
          let queue: QueuePage;
          try {
            queue = await service.hostMessages<QueuePage>(context,
              { action: 'botQueue', teamId: binding.teamId, generation: binding.generation, conversationId: id, botKey: binding.botKey, after: 0 }); assert();
            if (!Number.isSafeInteger(queue.cursor) || queue.cursor < 0 || !Array.isArray(queue.triggers) || queue.triggers.length > 30) throw new Error('INVALID_BOT_QUEUE');
            const head = queue.triggers[0];
            if (head && (!/^[A-Za-z0-9_-]{8,64}$/u.test(head.id) || !Number.isSafeInteger(head.seq) || head.seq <= queue.cursor)) throw new Error('INVALID_BOT_QUEUE');
          } catch (error) {
            assert();
            const code = error instanceof Error ? error.message : '';
            if (['INVALID_SESSION', 'SIGN_IN_REQUIRED'].includes(code)) throw error;
            if (['MEMBERSHIP_REQUIRED', 'STALE_MEMBERSHIP', 'BOT_OWNER_REQUIRED', 'BOT_REVISION_CONFLICT', 'INVALID_BOT_QUEUE'].includes(code)) { validScopes.delete(scope); this.stopBinding(binding.teamId, binding.botKey); }
            else if (code !== 'CONVERSATION_REQUIRED') this.failedDiscovery = true;
            continue;
          }
          const trigger = queue.triggers[0];
          if (!trigger) { this.emptyQueues.set(key, queueVersion); if (this.emptyQueues.size > 4096) this.emptyQueues.delete(this.emptyQueues.keys().next().value!); this.blocked.delete(key); continue; }
          this.emptyQueues.delete(key);
          // A slot is available before persisting a new execution identity.
          pending = { version: 1, snapshot: binding.snapshot, sequence: trigger.seq,
            target: { ownerUid: context.uid, ownerGeneration: binding.generation, hostId: this.options.privateStore.hostId,
              botId: bot.bot_id, botRevision: binding.revision, teamId: binding.teamId, conversationId: id, botKey: binding.botKey, triggerId: trigger.id } };
          await this.options.privateStore.savePending(context.uid, pending); assert();
        }
        const observation = this.observations.get(key);
        if (observation?.triggerId === pending.target.triggerId && observation.until > (this.options.now ?? Date.now)()) continue;
          const durable = pending, target = durable.target;
          if (this.blocked.get(key) === target.triggerId && !retry) continue;
          const controller = new AbortController();
          let ready!: () => void;
          const record: Active = { signature, controller, promise: Promise.resolve(), ready: new Promise(resolve => { ready = resolve; }) };
          let advanced = false;
          this.active.set(key, record); this.slots.add(record);
          this.scheduled.set(scope, ++this.admissions);
          record.promise = this.execute(context, target, binding, controller, assert, retry, ready).then(async result => {
            record.settling = true;
            if (controller.signal.aborted) return;
            assert();
            if (result.status === 'unstarted' && !retry) throw new Error('TEAM_HOST_PROTOCOL');
            if (result.status === 'settled' || result.status === 'unstarted') {
              await this.options.privateStore.settlePending(context.uid, durable);
              assert(); if (controller.signal.aborted) return;
              this.observations.delete(key); this.blocked.delete(key);
              this.status(target); advanced = true;
            } else if (result.status === 'recovery_required') {
              this.status(target, 'recovery_required'); this.blocked.set(key, target.triggerId);
              const prior = this.observations.get(key);
              const delay = Math.min(prior?.triggerId === target.triggerId ? prior.delay * 2 : 10000, 60000);
              this.observations.set(key, { triggerId: target.triggerId, delay, until: (this.options.now ?? Date.now)() + delay });
            } else if (result.state === 'result_pending') {
              this.status(target, 'result_pending');
            }
          }).catch(error => {
            if (controller.signal.aborted) return;
            let code = '';
            if (error instanceof TeamHostFailure) code = error.code;
            else if (error instanceof Error) code = error.message;
            // Journal-lock contention is an observation delay, not evidence
            // that a saved result or required recovery disappeared.
            switch (code) {
              case 'BUSY':
                return;
              case 'RESULT_PENDING':
                this.status(target, 'result_pending');
                break;
              case 'MODEL_UNAVAILABLE':
                this.status(target, 'model_unavailable');
                break;
              case 'TEAM_RUNTIME_UNAVAILABLE':
                this.status(target, 'runtime_unavailable');
                break;
              case 'STORAGE':
              case 'UNSAFE_TEAM_STORAGE':
              case 'INVALID_TEAM_SNAPSHOT':
                this.status(target, 'storage_error');
                break;
              default:
                this.status(target, 'retrying');
            }
            // Same journal + queue head on retry; never invent a new attempt.
          }).finally(() => {
            this.slots.delete(record);
            if (this.active.get(key) === record) this.active.delete(key);
            if (advanced || controller.signal.aborted) this.wake(); else this.schedule();
          });
      }
    }
  }
  private async execute(context: TeamAccountContext, target: TeamBotTarget, binding: BotTeamBinding, controller: AbortController, assert: () => void, retry: boolean, ready: () => void): Promise<TeamHostResult> {
    try {
    const binaries = this.options.binaries(), roots = await this.options.privateStore.owner(context.uid); assert();
    let snapshot: TeamPersonaSnapshot | undefined, key: string | undefined;
    if (!retry) {
      snapshot = await this.options.privateStore.snapshot(context.uid, binding.snapshot); assert();
      key = await this.options.modelCredential(snapshot.credential_id).catch(() => undefined); assert();
    }
    if (controller.signal.aborted) throw new Error('INVALID_SESSION');
    const assertCurrent = () => {
      assert(); if (controller.signal.aborted) throw new Error('INVALID_SESSION');
      const bot = this.options.store.get(target.botId);
      if (!(bot.teams ?? []).some(current => JSON.stringify(current) === JSON.stringify(binding))) throw new Error('BOT_REVISION_CONFLICT');
    };
    assertCurrent();
    return (this.options.run ?? runTeamHost)({ executable: binaries.host, home: roots.home, tmp: roots.tmp,
      bootstrap: { version: 1, type: 'start', mode: retry ? 'retry_result' : 'run', owner_uid: context.uid, account_epoch: context.epoch,
        endpoint: 'https://lingxios.net/v1/teams/messages', journal_root: roots.journal, runtime_root: roots.runtime,
        worker_executable: binaries.worker, target, ...(key && snapshot ? { snapshot: { instructions: snapshot.instructions, model_provider: snapshot.model_provider, model_protocol: snapshot.model_protocol,
          model_id: snapshot.model_id, model_endpoint: snapshot.model_endpoint, model_profile: snapshot.model_profile, model_api_key: key,
          max_turns: 8, max_input_bytes: 32 * 1024, output_tokens_per_request: 2048, physical_dispatches: 12 } } : {}) },
      signal: controller.signal, assertCurrent, token: () => this.options.service.hostToken(context),
      permission: async (input, signal) => {
        assertCurrent(); const allow = await this.options.permission({ ...input, accountEpoch: context.epoch, assertCurrent }, signal).catch(() => false); assertCurrent(); return allow === true;
      } });
    } finally { ready(); }
  }
  close(): void { if (this.closed) return; this.closed = true; clearTimeout(this.timer); this.stops.forEach(stop => stop()); this.cancelAll(); }
}
