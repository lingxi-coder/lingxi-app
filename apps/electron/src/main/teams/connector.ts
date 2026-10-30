import type { BindLocalTeamBotInput, LocalTeamBot, TeamBotCard } from '../../../../../packages/teams/bots';
import type { TeamConversation } from '../../../../../packages/teams/messages';
import type { TeamSummary } from '../../../../../packages/teams';
import type { BotStore, BotTeamBinding } from '../bots/store';
import type { BotContent } from '../bots/content';
import { resolveProviderIdForModel } from '../credential-broker';
import { TeamPrivateStore, type TeamPersonaSnapshot } from './privateStore';
import { runTeamHost, TeamHostFailure, type TeamBotTarget, type TeamHostProcessOptions, type TeamHostResult } from './process';
import type { LocalTeamHost, TeamAccountContext } from './service';

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
  catalog(): Promise<{ models: Array<{ providerId: string; modelId: string; canSelect: boolean }> }>;
  /** Only the selected static Anthropic key. No account/OAuth/refresh credentials. */
  modelCredential(): Promise<string | undefined>;
  /** The selected catalog endpoint must also be supported; never silently reroute. */
  modelEndpoint?(): string | undefined;
  permission(input: Parameters<TeamHostProcessOptions['permission']>[0] & { accountEpoch: number; assertCurrent(): void }, signal: AbortSignal): Promise<boolean>;
  platform?: NodeJS.Platform;
  run?: (options: TeamHostProcessOptions) => Promise<TeamHostResult>;
  pollMs?: number;
  now?: () => number;
}
interface Active { signature: string; controller: AbortController; promise: Promise<void> }
const scopeKey = (teamId: string, botKey: string) => JSON.stringify([teamId, botKey]);

/** Discovery is a hint. The transactional server queue and Rust journal own execution. */
export class TeamConnector implements LocalTeamHost {
  private closed = false;
  private pending = false;
  private refresh?: Promise<void>;
  private readonly active = new Map<string, Active>();
  private readonly blocked = new Map<string, string>();
  private readonly heartbeats = new Map<string, number>();
  private readonly statuses = new Map<string, string>();
  private readonly bindingOperations = new Set<string>();
  private readonly stops: Array<() => void>;
  private readonly timer: ReturnType<typeof setInterval>;
  private revision = 0;
  private notifyingStatus = false;
  constructor(private readonly options: TeamConnectorOptions) {
    this.stops = [options.service.onChanged(event => {
      if (event.invalidated) this.cancelAll();
      this.wake();
    }), options.service.onMessagesChanged(() => { if (!this.notifyingStatus) this.wake(); })];
    this.timer = setInterval(() => this.wake(), options.pollMs ?? 5000);
    this.timer.unref(); this.wake();
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
    if (!selected || resolveProviderIdForModel(selected) !== 'anthropic') throw new Error('TEAM_MODEL_UNSUPPORTED');
    const endpoint = this.options.modelEndpoint?.() ?? 'https://api.anthropic.com';
    if (endpoint !== 'https://api.anthropic.com') throw new Error('TEAM_MODEL_UNSUPPORTED');
    const nativeModel = selected.includes('/') ? selected.slice(selected.indexOf('/') + 1) : selected;
    const catalog = await this.options.catalog(); service.assertHostContext(context);
    if (!catalog.models.some(model => model.providerId === 'anthropic' && model.modelId === nativeModel && model.canSelect)) throw new Error('TEAM_MODEL_UNAVAILABLE');
    const instructions = await this.options.content.teamInstructions(input.botId); service.assertHostContext(context);
    // A racing local edit cannot bind a mixture of two Bot configurations.
    if (this.options.store.get(input.botId).revision !== bot.revision) throw new Error('BOT_REVISION_CONFLICT');
    const credential = await this.options.modelCredential(); service.assertHostContext(context);
    if (!credential) throw new Error('TEAM_MODEL_UNAVAILABLE');
    const snapshot = this.options.privateStore.saveSnapshot(uid, { instructions, model_id: nativeModel, model_endpoint: 'https://api.anthropic.com' });
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
    this.heartbeats.delete(prefix);
  }
  /** Called synchronously before local Bot deletion/settings changes are published. */
  localChanged(): void { this.cancelAll(); this.wake(); }
  invalidate(): void { this.cancelAll(); this.blocked.clear(); this.heartbeats.clear(); this.statuses.clear(); this.wake(); }
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
        try { await this.discover(); } catch { this.cancelAll(); }
      }
    })().finally(() => { this.refresh = undefined; if (this.pending) this.wake(); });
  }
  /** Exposed for deterministic lifecycle tests and trusted app shutdown only. */
  async idle(): Promise<void> { await this.refresh; }
  private async discover(): Promise<void> {
    const context = this.options.service.hostContext();
    if (!context) { this.cancelAll(); return; }
    const service = this.options.service, version = this.revision;
    const assert = () => { service.assertHostContext(context); if (this.closed || version !== this.revision) throw new Error('INVALID_SESSION'); };
    const own = this.options.store.list().flatMap(bot => (bot.teams ?? []).filter(binding => binding.ownerUid === context.uid).map(binding => ({ bot, binding })));
    if (!own.length) { this.cancelAll(); return; }
    const pendingTargets = this.options.privateStore.pending(context.uid);
    const { teams } = await service.list(context.uid); assert();
    const wanted = new Set<string>();
    for (const team of teams) {
      const local = own.filter(value => value.binding.teamId === team.id && value.binding.generation === team.generation);
      if (!local.length) continue;
      const { bots } = await service.hostMessages<{ bots: TeamBotCard[] }>(context, { action: 'bots', teamId: team.id, generation: team.generation }); assert();
      const matches = local.flatMap(value => {
        const card = bots.find(card => card.enabled !== false && card.key === value.binding.botKey && card.ownerUid === context.uid && card.hostId === this.options.privateStore.hostId
          && card.botId === value.bot.bot_id && card.revision === value.binding.revision
          && card.use === value.binding.use && JSON.stringify([...card.uids].sort()) === JSON.stringify(value.binding.uids));
        return card ? [{ ...value, card }] : [];
      });
      if (!matches.length) continue;
      const conversations: TeamConversation[] = [];
      const pages = new Set<string>(); let after: string | undefined;
      do {
        const page = await service.hostMessages<{ conversations: TeamConversation[]; next?: string | null }>(context, { action: 'list', teamId: team.id,
          generation: team.generation, ...(after ? { after } : {}) }); assert();
        conversations.push(...page.conversations);
        if (conversations.length > 10_000 || (page.next && pages.has(page.next))) throw new Error('INVALID_BOT_QUEUE');
        after = page.next ?? undefined; if (after) pages.add(after);
      } while (after);
      for (const { bot, binding } of matches) {
        const scope = scopeKey(team.id, binding.botKey), now = (this.options.now ?? Date.now)();
        if (now - (this.heartbeats.get(scope) ?? -Infinity) >= 20_000) {
          await service.hostMessages(context, { action: 'heartbeatBot', teamId: team.id, generation: team.generation, botKey: binding.botKey, revision: binding.revision }); assert();
          this.heartbeats.set(scope, now);
        }
        for (const conversation of conversations) {
          if (!conversation.participants.includes(binding.botKey)) continue;
          const key = `${scope}:${conversation.id}`, signature = JSON.stringify([context.uid, context.epoch, binding]);
          wanted.add(key);
          const active = this.active.get(key);
          if (active?.signature === signature) continue;
          active?.controller.abort(); this.active.delete(key);
          // A finish response can be lost AFTER the backend advanced its cursor.
          // The durable index, not a queue entry, discovers that exact result.
          let pending = pendingTargets.find(value => value.target.ownerUid === context.uid && value.target.ownerGeneration === team.generation
            && value.target.teamId === team.id && value.target.conversationId === conversation.id && value.target.botKey === binding.botKey
            && value.target.hostId === this.options.privateStore.hostId && value.target.botId === bot.bot_id
            && value.target.botRevision === binding.revision && value.snapshot === binding.snapshot);
          // Discovery awaited HTTP since listing the index. An active host can
          // have acknowledged settlement meanwhile; never revive that entry.
          if (pending && !this.options.privateStore.hasPending(context.uid, pending)) pending = undefined;
          const retry = pending !== undefined;
          if (!pending) {
            const queue = await service.hostMessages<{ cursor: number; triggers: Array<{ id: string; seq: number }>; hasMore: boolean }>(context,
              { action: 'botQueue', teamId: team.id, generation: team.generation, conversationId: conversation.id, botKey: binding.botKey, after: 0 }); assert();
            if (!Number.isSafeInteger(queue.cursor) || queue.cursor < 0 || !Array.isArray(queue.triggers)) throw new Error('INVALID_BOT_QUEUE');
            const trigger = queue.triggers[0];
            if (!trigger) { this.blocked.delete(key); continue; }
            if (!/^[A-Za-z0-9_-]{8,64}$/u.test(trigger.id) || !Number.isSafeInteger(trigger.seq) || trigger.seq <= queue.cursor) throw new Error('INVALID_BOT_QUEUE');
            pending = { version: 1, snapshot: binding.snapshot, sequence: trigger.seq,
              target: { ownerUid: context.uid, ownerGeneration: team.generation, hostId: this.options.privateStore.hostId,
                botId: bot.bot_id, botRevision: binding.revision, teamId: team.id, conversationId: conversation.id, botKey: binding.botKey, triggerId: trigger.id } };
            this.options.privateStore.savePending(context.uid, pending);
          }
          const durable = pending, target = durable.target;
          if (this.blocked.get(key) === target.triggerId && !retry) continue;
          const controller = new AbortController();
          const record: Active = { signature, controller, promise: Promise.resolve() };
          let advanced = false;
          this.active.set(key, record);
          record.promise = this.execute(context, target, binding, controller, assert, retry).then(result => {
            if (controller.signal.aborted) return;
            assert();
            if (result.status === 'unstarted' && !retry) throw new Error('TEAM_HOST_PROTOCOL');
            if (result.status === 'settled' || result.status === 'unstarted') {
              this.options.privateStore.settlePending(context.uid, durable); this.blocked.delete(key);
              this.status(target); advanced = true;
            } else if (result.status === 'recovery_required') {
              this.status(target, 'recovery_required'); this.blocked.set(key, target.triggerId);
            } else if (result.state === 'result_pending') {
              this.status(target, 'result_pending');
            }
          }).catch(error => {
            if (controller.signal.aborted) return;
            const code = error instanceof TeamHostFailure ? error.code : error instanceof Error ? error.message : '';
            // Journal-lock contention is an observation delay, not evidence
            // that a saved result or required recovery disappeared.
            if (code === 'BUSY') return;
            this.status(target, code === 'RESULT_PENDING' ? 'result_pending' : code === 'MODEL_UNAVAILABLE' ? 'model_unavailable'
              : code === 'TEAM_RUNTIME_UNAVAILABLE' ? 'runtime_unavailable' : ['STORAGE', 'UNSAFE_TEAM_STORAGE', 'INVALID_TEAM_SNAPSHOT'].includes(code) ? 'storage_error' : 'retrying');
            // Same journal + queue head on retry; never invent a new attempt.
          }).finally(() => {
            if (this.active.get(key) === record) this.active.delete(key);
            if (advanced && !controller.signal.aborted) this.wake();
          });
        }
      }
    }
    assert();
    for (const [key, active] of this.active) if (!wanted.has(key)) { active.controller.abort(); this.active.delete(key); }
  }
  private async execute(context: TeamAccountContext, target: TeamBotTarget, binding: BotTeamBinding, controller: AbortController, assert: () => void, retry: boolean): Promise<TeamHostResult> {
    const binaries = this.options.binaries(), roots = this.options.privateStore.owner(context.uid);
    let snapshot: TeamPersonaSnapshot | undefined, key: string | undefined;
    if (!retry) {
      snapshot = this.options.privateStore.snapshot(context.uid, binding.snapshot);
      key = await this.options.modelCredential().catch(() => undefined); assert();
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
        worker_executable: binaries.worker, target, ...(key && snapshot ? { snapshot: { ...snapshot, model_api_key: key,
          max_turns: 8, max_input_bytes: 32 * 1024, output_tokens_per_request: 2048, physical_dispatches: 12 } } : {}) },
      signal: controller.signal, assertCurrent, token: () => this.options.service.hostToken(context),
      permission: async (input, signal) => {
        assertCurrent(); const allow = await this.options.permission({ ...input, accountEpoch: context.epoch, assertCurrent }, signal).catch(() => false); assertCurrent(); return allow === true;
      } });
  }
  close(): void { if (this.closed) return; this.closed = true; clearInterval(this.timer); this.stops.forEach(stop => stop()); this.cancelAll(); }
}
