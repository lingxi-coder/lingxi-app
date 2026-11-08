import { randomBytes, randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { open, lstat } from 'node:fs/promises';
import { basename, dirname, extname, join, parse, resolve } from 'node:path';
import { providerAccountEnvelope, type ProviderAccountLaunch } from '../provider-accounts.js';
import type { LocalPublishableBot, PublishLocalBotInput, SelectedSharedFile, SharedBotCard, TeamTaskView } from '../../../../../packages/teams/execution.js';
import { TeamsApiError, type TeamSummary } from '../../../../../packages/teams/index.js';
import type { ChatReadRequest, ChatReadResult } from '../../../../../packages/teams/chat-source.js';
import type { BotStore } from '../bots/store.js';
import { validateBotManifest } from '../bots/package.js';
import { TeamPrivateStore, digest, validateTeamModelRoute, type TeamModelSelection, type TeamPersonaSnapshot, type SharedBotBinding, type PublishedDocument, type SavedExecution } from './privateStore.js';
import { runTeamHost, TeamHostFailure, type TeamHostProcessOptions, type TeamHostResult, type BotChatScope } from './process.js';
import type { LocalTeamHost, TeamAccountContext } from './service.js';

export interface TeamHostApi {
  hostContext(): TeamAccountContext | undefined;
  assertHostContext(context: TeamAccountContext): void;
  hostMessages<T>(context: TeamAccountContext, command: Record<string, unknown>): Promise<T>;
  hostReadChat(context: TeamAccountContext, request: ChatReadRequest): Promise<ChatReadResult>;
  list(uid: string): Promise<{ teams: TeamSummary[] }>;
  onChanged(callback: (event: { invalidated?: boolean }) => void): () => void;
  onMessagesChanged(callback: () => void): () => void;
}
export interface TeamConnectorOptions {
  service: TeamHostApi; store: BotStore; privateStore: TeamPrivateStore;
  binaries(): { host: string; worker: string };
  defaultModel(): string | undefined;
  modelRoute(selected: string): Promise<TeamModelSelection>;
  modelAccounts(credentialId: string, executionBindingId: string, profiles: readonly Record<string, unknown>[]): Promise<ProviderAccountLaunch | undefined>;
  renewModelAccount?(accountId: string, identityGeneration: number, credentialRevision: number): Promise<void>;
  /** Only the native chooser supplies paths; renderer requests carry opaque one-use tokens. */
  chooseFiles(): Promise<string[]>;
  validateModel?(context: TeamAccountContext, snapshot: TeamPersonaSnapshot, selection: TeamModelSelection): Promise<void>;
  platform?: NodeJS.Platform; run?: (options: TeamHostProcessOptions) => Promise<TeamHostResult>;
  retryMs?: number; now?: () => number; maxConcurrent?: number;
  /** Event name plus ids and codes only; never user text, credentials or model output. */
  diagnostics?: (event: string, details: Record<string, unknown>) => void;
}
interface Selected { context: TeamAccountContext; instanceId: string; expires: number; document: PublishedDocument }
interface HostTask extends TeamTaskView { messageId: string; messageSeq: number }
interface TaskContext { task: HostTask; messageId: string; messageSeq: number; inputs: Array<{ grantId: string; resourceId: string; digest: string }>; deployment: { deploymentId: string; definitionDigest: string; accountEpoch: number } }
const id = (value: unknown): value is string => typeof value === 'string' && /^[A-Za-z0-9_-]{8,64}$/u.test(value);
const scopeKey = (b: SharedBotBinding) => JSON.stringify([b.teamId, b.sharedBotId]);
const serialKey = (b: SharedBotBinding, task: HostTask) => JSON.stringify([b.teamId, b.sharedBotId, task.conversationId]);
const activation = (b: SharedBotBinding, c: TeamAccountContext) => JSON.stringify([c.epoch,b.deploymentId,b.accountEpoch]);
const newId = () => randomBytes(16).toString('base64url');
const errorCode = (error: unknown) => error instanceof TeamHostFailure || error instanceof TeamsApiError ? error.code
  : error instanceof Error && /^[A-Z][A-Z0-9_]{2,59}$/u.test(error.message) ? error.message : 'ERROR';
/** The server or the account answered no; anything else (network, 5xx, timeout) may succeed on the next try. */
const definitive = (error: unknown) => error instanceof TeamsApiError ? error.status >= 400 && error.status < 500 && error.status !== 408 && error.status !== 429
  : error instanceof Error && error.message === 'INVALID_SESSION';
/** Server answers after which a saved result can never be accepted; retrying would loop forever. */
const terminalDelivery = (error: unknown) => error instanceof TeamsApiError && ['TASK_REVOKED','TASK_EXPIRED','TASK_REQUIRED','TASK_RESULT_CONFLICT','EXECUTION_PERMIT_REQUIRED'].includes(error.code);
const noCursor = { scope: null, sessionRevision: 0, lastMessageSeq: 0, lastChangeSeq: 0, bootstrapFromSeq: 0 };
/** Source reads, selected account credentials and backend permits stay in trusted main. */
export class TeamConnector implements LocalTeamHost {
  private closed = false; private pending = false; private refresh?: Promise<void>; private timer?: ReturnType<typeof setTimeout>;
  private readonly active = new Map<string, { taskId: string; controller: AbortController; promise: Promise<void> }>();
  private readonly selections = new Map<string, Selected>(); private readonly publishing = new Set<string>();
  private readonly activated = new Map<string, string>(); private readonly heartbeats = new Map<string, number>();
  private readonly deferred = new Map<string, number>(); private readonly scheduled = new Map<string, number>(); private readonly stops: Array<() => void>;
  private revision = 0; private probes = 0;
  constructor(private readonly options: TeamConnectorOptions) {
    this.stops = [options.service.onChanged(() => this.wake()), options.service.onMessagesChanged(() => this.wake())]; this.wake();
  }
  private now() { return (this.options.now ?? Date.now)(); }
  private log(event: string, details: Record<string, unknown>) { try { this.options.diagnostics?.(event, details); } catch { /* Diagnostics never interrupt a task. */ } }
  private abort(a: { taskId: string; controller: AbortController }, reason: string) { this.log('team_task_aborted', { taskId: a.taskId, reason }); a.controller.abort(reason); }
  private available() { if ((this.options.platform ?? process.platform) !== 'darwin') throw new Error('TEAM_HOST_MACOS_REQUIRED'); }
  private context(uid: string) { const c = this.options.service.hostContext(); if (!c || c.uid !== uid || this.closed) throw new Error('INVALID_SESSION'); return c; }
  private assert(c: TeamAccountContext) { if (this.closed) throw new Error('INVALID_SESSION'); this.options.service.assertHostContext(c); }
  private command<T>(c: TeamAccountContext, binding: Pick<SharedBotBinding, 'teamId' | 'generation'>, action: string, payload: Record<string, unknown>, commandId = newId()) {
    this.assert(c); return this.options.service.hostMessages<T>(c, { schemaVersion: 1, commandId, teamId: binding.teamId, generation: binding.generation, action, payload });
  }
  private hostFields(b: SharedBotBinding) { return { deploymentId: b.deploymentId, hostId: this.options.privateStore.hostId, accountEpoch: b.accountEpoch }; }
  async localBots(uid: string): Promise<LocalPublishableBot[]> {
    this.available(); const context = this.context(uid); this.assert(context);
    const bots = await Promise.all(this.options.store.list().map(async b => ({ instanceId: b.bot_id, name: b.name, description: b.description, ...(b.model ? { model: b.model } : {}),
      requiredResources: await this.requiredResources(b.bot_id) }))); this.assert(context); return bots;
  }
  private async requiredResources(instanceId: string): Promise<string[]> {
    const bot = this.options.store.get(instanceId); if (!bot.marketOrigin) return [];
    const path = join(this.options.store.botDir(instanceId), 'market-package.json');
    const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    try { const info = await handle.stat(); if (!info.isFile() || info.nlink !== 1 || info.size > 1024 * 1024) throw new Error('INVALID_BOT_PACKAGE');
      return validateBotManifest(JSON.parse(await handle.readFile('utf8'))).resources.filter(r => r.required).map(r => r.id); }
    finally { await handle.close(); }
  }
  async selectSharedFiles(uid: string, instanceId: string): Promise<SelectedSharedFile[]> {
    this.available(); const context = this.context(uid); this.options.store.get(instanceId);
    const paths = await this.options.chooseFiles(); this.assert(context);
    if (!Array.isArray(paths) || paths.length > 49) throw new Error('INVALID_SHARED_FILE');
    for (const [token,selected] of this.selections) if (selected.expires<=this.now()) this.selections.delete(token);
    if (this.selections.size+paths.length>200) throw new Error('SHARED_FILE_LIMIT');
    const result: SelectedSharedFile[] = []; let total = 0;
    for (const path of paths) {
      if (typeof path !== 'string' || resolve(path) !== path || !['.md','.txt','.csv','.json'].includes(extname(path).toLowerCase())) throw new Error('INVALID_SHARED_FILE');
      // Refuse links at every path component. Only the captured bytes are published.
      let parent = dirname(path); while (parent !== parse(parent).root) { if (!(await lstat(parent)).isDirectory()) throw new Error('INVALID_SHARED_FILE'); parent = dirname(parent); }
      const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
      let text: string;
      try { const info = await handle.stat(); if (!info.isFile() || info.nlink !== 1 || info.size > 256 * 1024) throw new Error('INVALID_SHARED_FILE');
        const bytes = Buffer.alloc(256 * 1024 + 1); let length=0;
        while (length<bytes.length) { const part=await handle.read(bytes,length,bytes.length-length,length); if (!part.bytesRead) break; length+=part.bytesRead; }
        const after=await handle.stat();
        if (length>256*1024 || after.size!==length || after.nlink!==1 || after.mtimeMs!==info.mtimeMs || after.ctimeMs!==info.ctimeMs) throw new Error('INVALID_SHARED_FILE');
        text = new TextDecoder('utf-8', { fatal: true, ignoreBOM: true }).decode(bytes.subarray(0,length)); }
      finally { await handle.close(); }
      this.assert(context); total += Buffer.byteLength(text); if (!text.trim() || text.includes('\0') || total > 2 * 1024 * 1024) throw new Error('INVALID_SHARED_FILE');
      const name = basename(path), token = newId(), hash = digest(text);
      const document: PublishedDocument = { resourceId: newId(), revision: 1, kind: name === 'SKILL.md' ? 'skill' : 'file', name, text, digest: hash };
      this.selections.set(token, { context, instanceId, expires: this.now() + 600000, document });
      result.push({ token, name, sha256: hash, bytes: Buffer.byteLength(text), mediaType: 'text/plain', kind: document.kind as 'file' | 'skill' });
    }
    return result;
  }
  async publishLocalBot(uid: string, input: PublishLocalBotInput): Promise<{ sharedBotId: string; revision: number }> {
    this.available(); const context = this.context(uid), key = JSON.stringify([uid, input?.teamId, input?.sharedBotId]);
    if (this.publishing.has(key)) throw new Error('BOT_REVISION_CONFLICT'); this.publishing.add(key);
    try {
      if (!input || Object.keys(input).some(k => !['teamId','generation','publicationId','sharedBotId','instanceId','expectedRevision','usePolicy','fileTokens','quotas','role','resourceBindings'].includes(k))
        || ![input.teamId,input.publicationId,input.sharedBotId,input.instanceId].every(id) || !Number.isSafeInteger(input.generation) || input.generation < 1
        || !Number.isSafeInteger(input.expectedRevision) || input.expectedRevision < 0 || typeof input.role !== 'string' || !input.role.trim() || input.role.includes('\0') || Buffer.byteLength(input.role) > 64 * 1024
        || !Array.isArray(input.fileTokens) || input.fileTokens.length > 49 || new Set(input.fileTokens).size !== input.fileTokens.length) throw new Error('INVALID_INPUT');
      const policy = input.usePolicy;
      const member = (value: unknown): boolean => !!value && typeof value === 'object' && !Array.isArray(value)
        && Object.keys(value).sort().join(',') === 'generation,uid' && typeof (value as {uid?:unknown}).uid === 'string'
        && /^[A-Za-z0-9_-]{1,128}$/u.test((value as {uid:string}).uid) && Number.isSafeInteger((value as {generation?:unknown}).generation) && (value as {generation:number}).generation > 0;
      if (!policy || typeof policy !== 'object' || (policy.mode === 'all' ? Object.keys(policy).join(',') !== 'mode' : policy.mode !== 'listed'
        || Object.keys(policy).sort().join(',') !== 'members,mode' || !Array.isArray(policy.members) || policy.members.length > 200 || !policy.members.every(member)
        || new Set(policy.members.map(m=>m.uid)).size !== policy.members.length)
        || !input.quotas || typeof input.quotas !== 'object' || Object.keys(input.quotas).sort().join(',') !== 'bot,member,team'
        || Object.values(input.quotas).some(n=>!Number.isSafeInteger(n)||n<1||n>10000) || input.sharedBotId === input.instanceId) throw new Error('INVALID_INPUT');
      const requestDigest = digest(JSON.stringify(input));
      const pending = await this.options.privateStore.publication(uid,input.publicationId); this.assert(context);
      if (pending) {
        if (pending.requestDigest !== requestDigest) throw new Error('COMMAND_ID_CONFLICT');
        const result = await this.command<{sharedBotId:string;revision:number;grantIds:string[]}>(context,pending.binding,'publishSharedBot',pending.payload,input.publicationId); this.assert(context);
        if (result.sharedBotId !== pending.binding.sharedBotId || result.revision !== pending.binding.revision || JSON.stringify(result.grantIds)!==JSON.stringify(pending.binding.grantIds)) throw new Error('INVALID_PUBLICATION');
        await this.adoptPublication(context,pending.binding); this.wake(); return {sharedBotId:result.sharedBotId,revision:result.revision};
      }
      this.options.binaries(); const bot = this.options.store.get(input.instanceId), selected = bot.model ?? this.options.defaultModel(); if (!selected) throw new Error('TEAM_MODEL_UNAVAILABLE');
      const route = await this.options.modelRoute(selected); this.assert(context); validateTeamModelRoute(route.route);
      const accounts = await this.options.modelAccounts(route.route.credential_id, route.route.model_provider, route.profiles); this.assert(context); if (!accounts) throw new Error('TEAM_MODEL_UNAVAILABLE');
      const documents: PublishedDocument[] = [{ resourceId: digest(JSON.stringify([input.publicationId, 'persona'])), revision: 1, kind: 'persona', name: 'Role', text: input.role, digest: digest(input.role) }];
      for (const token of input.fileTokens) { const selection = this.selections.get(token);
        if (!selection || selection.context.uid !== uid || selection.context.epoch !== context.epoch || selection.instanceId !== input.instanceId || selection.expires <= this.now()) throw new Error('INVALID_SHARED_FILE_TOKEN'); documents.push({...selection.document}); }
      const required = await this.requiredResources(input.instanceId); this.assert(context);
      const bindings = input.resourceBindings ?? {};
      if (!bindings || typeof bindings !== 'object' || Array.isArray(bindings) || Object.keys(bindings).some(key => !required.includes(key))
        || required.some(key => typeof bindings[key] !== 'string' || !input.fileTokens.includes(bindings[key]!))) throw new Error('RESOURCE_BINDING_REQUIRED');
      for (const [resource, token] of Object.entries(bindings)) { const selected = this.selections.get(token); const document = documents.find(d => d.resourceId === selected?.document.resourceId); if (document) document.name = `${resource}: ${document.name}`; }
      await route.assertCurrent(); this.assert(context); if (this.options.store.get(input.instanceId).revision !== bot.revision) throw new Error('BOT_REVISION_CONFLICT');
      const instructions = [input.role,...documents.filter(d => d.kind === 'skill').map(d => `\n\n# Published Skill: ${d.name}\n${d.text}`)].join('');
      const snapshot: TeamPersonaSnapshot = { ...route.route, instructions, provider_account_profiles: route.profiles, documents };
      const snapshotId = await this.options.privateStore.saveSnapshot(uid, snapshot); this.assert(context);
      if (this.active.size+this.probes>=Math.min(4,this.options.maxConcurrent??4)) throw new Error('TEAM_HOST_BUSY');
      this.probes++;
      try { if (this.options.validateModel) await this.options.validateModel(context,snapshot,route); else await this.validateModel(context,snapshot,route); }
      finally { this.probes--; this.wake(); }
      this.assert(context);
      const existing = (await this.options.privateStore.bindings(uid)).find(b => b.teamId === input.teamId && b.sharedBotId === input.sharedBotId);
      const binding: SharedBotBinding = { schemaVersion: 1, ownerUid: uid, teamId: input.teamId, generation: input.generation, sharedBotId: input.sharedBotId,
        instanceId: input.instanceId, deploymentId: digest(JSON.stringify([input.publicationId,'deployment'])), revision: input.expectedRevision + 1,
        snapshot: snapshotId, accountEpoch: Math.max(1, (existing?.accountEpoch ?? 0) + 1), publicationId: input.publicationId,
        grantIds: documents.map(d => digest(JSON.stringify([input.sharedBotId,input.expectedRevision+1,d.resourceId]))), sessionEpoch: (existing?.sessionEpoch ?? 0) + 1 };
      const payload = {
        sharedBotId: input.sharedBotId, expectedRevision: input.expectedRevision, name: bot.name, description: bot.description, usePolicy: input.usePolicy, quotas: input.quotas,
        deployment: { deploymentId: binding.deploymentId, sharedBotId: binding.sharedBotId, revision: binding.revision, hostId: this.options.privateStore.hostId, owner: { uid, generation: input.generation },
          instanceId: input.instanceId, definitionDigest: snapshotId, capabilitySnapshotDigest: digest(JSON.stringify({ executionVersion: 2, documents: documents.map(d => d.digest) })), executionVersion: 2,
          capabilities: { durableWait: false, writeOperations: false, delegation: false }, accountEpoch: binding.accountEpoch, enabled: true },
        inputs: documents.map(d => ({ resourceId: d.resourceId, revision: d.revision, digest: d.digest, kind: d.kind, sourceAudience: { mode: 'team' } })),
      };
      await this.options.privateStore.savePublication(uid,{schemaVersion:1,requestDigest,binding,payload}); this.assert(context);
      const result = await this.command<{sharedBotId:string;revision:number;grantIds:string[]}>(context,binding,'publishSharedBot',payload,input.publicationId);
      this.assert(context); if (result.sharedBotId !== binding.sharedBotId || result.revision !== binding.revision || JSON.stringify(result.grantIds) !== JSON.stringify(binding.grantIds)) throw new Error('INVALID_PUBLICATION');
      if (await this.adoptPublication(context,binding)) this.activated.set(scopeKey(binding),activation(binding,context)); for (const token of input.fileTokens) this.selections.delete(token); this.wake(); return { sharedBotId: result.sharedBotId, revision: result.revision };
    } finally { this.publishing.delete(key); this.wake(); }
  }
  private async adoptPublication(c: TeamAccountContext, candidate: SharedBotBinding): Promise<boolean> {
    const directory=await this.command<{bots:SharedBotCard[]}>(c,candidate,'listSharedBots',{}); this.assert(c);
    if (!directory.bots.some(card=>card.sharedBotId===candidate.sharedBotId&&card.revision===candidate.revision&&card.maintainer.uid===c.uid&&card.maintainer.generation===candidate.generation)) return false;
    const current=(await this.options.privateStore.bindings(c.uid)).find(b=>b.teamId===candidate.teamId&&b.sharedBotId===candidate.sharedBotId); this.assert(c);
    if (current && (current.revision>candidate.revision || current.revision===candidate.revision && current.accountEpoch>=candidate.accountEpoch)) return false;
    await this.options.privateStore.saveBinding(c.uid,candidate); this.assert(c); this.stopBinding(candidate.teamId,candidate.sharedBotId); this.activated.delete(scopeKey(candidate)); return true;
  }
  /** Explicit owner publication checks the chosen model with one bounded private call. */
  private async validateModel(c: TeamAccountContext, snapshot: TeamPersonaSnapshot, selection: TeamModelSelection): Promise<void> {
    const paths = await this.options.privateStore.owner(c.uid), bins = this.options.binaries(), taskId = newId();
    const scope: BotChatScope = { teamId:newId(),sharedBotId:newId(),conversationId:newId(),sessionEpoch:1 };
    const source = {messageId:newId(),seq:1,changeSeq:1,changeId:newId(),author:{kind:'user',uid:c.uid,displayName:'Publisher'},createdAt:this.now(),text:'Reply with READY to confirm this model is available.'};
    const reference = await this.options.privateStore.dataRef(c.uid,{schemaVersion:1,batchId:taskId,...scope,expectedSessionRevision:0,fromSeqExclusive:0,toSeqInclusive:1,lastChangeSeq:1,bootstrap:true,currentMessageId:source.messageId,messages:[source],changes:[]});
    const assert = () => this.assert(c); const authorize = async () => { assert(); await selection.assertCurrent(); assert(); };
    try {
      await authorize(); const accounts = await this.options.modelAccounts(snapshot.credential_id,snapshot.model_provider,snapshot.provider_account_profiles); assert(); if (!accounts) throw new Error('TEAM_MODEL_UNAVAILABLE');
      const {credential_id:_credential,provider_account_profiles:_profiles,instructions:_instructions,documents:_documents,...model} = snapshot;
      const result = await (this.options.run ?? runTeamHost)({executable:bins.host,home:paths.home,tmp:paths.tmp,assertCurrent:assert,authorize,signal:AbortSignal.timeout(45000),timeoutMs:45000,
        accountCredentials:async request => { await authorize(); const selected=accounts.providerAccountCredentials[request.accountId];
          if (!selected || selected.identity_generation !== request.identityGeneration) throw new Error('TEAM_MODEL_UNAVAILABLE');
          await this.options.renewModelAccount?.(request.accountId,request.identityGeneration,request.credentialRevision); await authorize();
          const refreshed=await this.options.modelAccounts(snapshot.credential_id,snapshot.model_provider,snapshot.provider_account_profiles); assert(); if (!refreshed) throw new Error('TEAM_MODEL_UNAVAILABLE'); return refreshed; },
        bootstrap:{version:3,type:'run',storageRoot:paths.runtime,workerExecutable:bins.worker,scope,taskId,invocationId:randomUUID(),instructions:'Confirm model readiness. Reply READY. Do not call tools.',inputBatchRef:reference,knowledge:[],deadlineAt:this.now()+45000,
          model,credentials:providerAccountEnvelope(accounts,snapshot.provider_account_profiles),limits:{max_turns:1,context_window_tokens:32768,output_tokens_per_request:64,physical_dispatches:1}}});
      assert(); if (result.status !== 'completed' || !result.text?.trim()) throw new Error('TEAM_MODEL_UNAVAILABLE'); await authorize();
    } finally { await this.options.privateStore.releaseDataRef(c.uid,reference); }
  }
  stopBinding(teamId: string, sharedBotId: string) { for (const [key, a] of this.active) { const scope = JSON.parse(key) as string[]; if (scope[0] === teamId && scope[1] === sharedBotId) this.abort(a,'BINDING_STOPPED'); } this.heartbeats.delete(JSON.stringify([teamId,sharedBotId])); }
  localChanged() { this.revoke(); }
  /** Account reset, sign-out, Team revocation or a local Bot change: running tasks lose their authority. A cache or listener flicker never comes here. */
  revoke() { this.revision++; for (const a of this.active.values()) this.abort(a,'REVOKED'); this.selections.clear(); this.deferred.clear(); this.activated.clear(); this.heartbeats.clear(); this.wake(); }
  private wake() {
    if (this.closed || (this.options.platform ?? process.platform) !== 'darwin') return; this.pending = true; if (this.refresh) return;
    this.refresh = (async () => { while (this.pending && !this.closed) { this.pending = false; try { await this.discover(); } catch { /* Retry discovery; a failed authorization never starts work. */ } } })()
      .finally(() => { this.refresh = undefined; clearTimeout(this.timer); if (!this.closed) { this.timer = setTimeout(() => this.wake(), this.options.retryMs ?? 5000); this.timer.unref(); } });
  }
  async idle() { await this.refresh; }
  private async discover() {
    const c = this.options.service.hostContext(); if (!c) return; const revision = this.revision;
    const assert = () => { this.assert(c); if (revision !== this.revision) throw new Error('INVALID_SESSION'); };
    let bindings = await this.options.privateStore.bindings(c.uid); assert();
    for (const publication of await this.options.privateStore.publications(c.uid)) {
      assert(); const candidate=publication.binding;
      const current=bindings.find(b=>b.teamId===candidate.teamId&&b.sharedBotId===candidate.sharedBotId);
      if (this.publishing.has(JSON.stringify([c.uid,candidate.teamId,candidate.sharedBotId])) || current && current.revision>=candidate.revision) continue;
      try {
        const directory=await this.command<{bots:SharedBotCard[]}>(c,candidate,'listSharedBots',{}); assert();
        if (!directory.bots.some(card=>card.sharedBotId===candidate.sharedBotId&&card.revision===candidate.revision&&card.maintainer.uid===c.uid)) continue;
        const receipt=await this.command<{sharedBotId:string;revision:number;grantIds:string[]}>(c,candidate,'publishSharedBot',publication.payload,candidate.publicationId); assert();
        if(receipt.sharedBotId!==candidate.sharedBotId||receipt.revision!==candidate.revision||JSON.stringify(receipt.grantIds)!==JSON.stringify(candidate.grantIds))throw new Error('INVALID_PUBLICATION');
        await this.adoptPublication(c,candidate); assert();
        bindings=await this.options.privateStore.bindings(c.uid); assert();
      } catch { assert(); }
    }
    if (!bindings.length) return;
    const { teams } = await this.options.service.list(c.uid); assert();
    const candidates: Array<{ binding: SharedBotBinding; task: HostTask; snapshot: TeamPersonaSnapshot }> = [];
    for (let b of bindings) {
      if (!teams.some(t => t.id === b.teamId && t.generation === b.generation)) continue;
      try {
        const { bots } = await this.command<{ bots: SharedBotCard[] }>(c,b,'listSharedBots',{}); assert();
        const card = bots.find(v => v.sharedBotId === b.sharedBotId); if (!card || card.state !== 'published' || card.revision !== b.revision || card.maintainer.uid !== c.uid) { this.stopBinding(b.teamId,b.sharedBotId); continue; }
        if (this.activated.get(scopeKey(b)) !== activation(b,c)) {
          const next = b.accountEpoch + 1; await this.command(c,b,'activateDeployment',{ deploymentId:b.deploymentId,hostId:this.options.privateStore.hostId,expectedAccountEpoch:b.accountEpoch,newAccountEpoch:next },digest(JSON.stringify([b.deploymentId,next,'activate']))); assert();
          b = { ...b, accountEpoch: next }; await this.options.privateStore.saveBinding(c.uid,b); assert(); this.activated.set(scopeKey(b),activation(b,c));
        }
        const snapshot = await this.options.privateStore.snapshot(c.uid,b.snapshot); assert(); let ready = true;
        try { this.options.store.get(b.instanceId); this.options.binaries(); const accounts = await this.options.modelAccounts(snapshot.credential_id,snapshot.model_provider,snapshot.provider_account_profiles); assert(); if (!accounts) ready = false; } catch { ready = false; }
        if ((this.heartbeats.get(scopeKey(b)) ?? 0) + 20000 < this.now() || !ready) {
          await this.command(c,b,'heartbeatDeployment',{...this.hostFields(b),ready}); assert(); this.heartbeats.set(scopeKey(b),this.now());
        }
        if (!ready) { this.stopBinding(b.teamId,b.sharedBotId); continue; }
        let after: string | undefined; const pages = new Set<string>();
        do {
          const response = await this.command<{ tasks: HostTask[]; nextCursor?: string | null }>(c,b,'listHostTasks',{...this.hostFields(b),...(after?{after}:{})}); assert();
          if (!Array.isArray(response.tasks) || response.tasks.length>100) throw new Error('INVALID_TASK_PAGE');
          for (const task of response.tasks) if (task.state === 'queued' && (this.deferred.get(task.taskId)??0)<=this.now()) candidates.push({binding:b,task,snapshot});
          if (!response.nextCursor) break;
          if (!id(response.nextCursor) || pages.has(response.nextCursor)) throw new Error('INVALID_TASK_PAGE');
          pages.add(response.nextCursor); after=response.nextCursor;
        } while (after);
      } catch (error) { if (definitive(error)) this.stopBinding(b.teamId,b.sharedBotId); }
    }
    const delivering = new Set((await this.options.privateStore.deliveries(c.uid)).map(d => d.taskId)); assert();
    for (const intent of await this.options.privateStore.executions(c.uid)) {
      assert(); if (delivering.has(intent.taskId) || [...this.active.values()].some(active => active.taskId === intent.taskId)) continue;
      let result = await this.options.privateStore.savedResult(c.uid,intent.taskId); assert();
      if (!result) {
        try { const paths = await this.options.privateStore.owner(c.uid), bins = this.options.binaries();
          const recovered = await (this.options.run ?? runTeamHost)({executable:bins.host,home:paths.home,tmp:paths.tmp,signal:AbortSignal.timeout(10000),timeoutMs:10000,assertCurrent:assert,
            bootstrap:{version:3,type:'inspect_result',storageRoot:paths.runtime,workerExecutable:bins.worker,scope:intent.scope,taskId:intent.taskId}});
          assert(); if (recovered.status === 'completed' || recovered.status === 'failed') result=recovered;
        } catch { assert(); }
      }
      // No result and no process left past the deadline: end the task instead of inspecting it forever.
      if (!result && !(this.now() < intent.deadlineAt)) { this.log('team_task_recovered_failed', { taskId: intent.taskId }); result = this.failedResult(intent); }
      if (result) await this.saveResult(c,intent,result);
    }
    for (const delivery of await this.options.privateStore.deliveries(c.uid)) {
      assert(); const b = (await this.options.privateStore.bindings(c.uid)).find(b => b.teamId === delivery.teamId && b.deploymentId === delivery.payload.deploymentId); if (!b) continue;
      const payload = { ...delivery.payload, accountEpoch: b.accountEpoch };
      const commandId = delivery.payload.accountEpoch === b.accountEpoch ? delivery.commandId : digest(JSON.stringify([delivery.commandId,b.accountEpoch]));
      try { await this.command(c,delivery,'completeTask',payload,commandId); assert(); await this.options.privateStore.delivered(c.uid,delivery.taskId); }
      catch (error) { // Keep the exact result for another try, never run its model again, unless the server will never accept it.
        if (terminalDelivery(error)) { this.log('team_task_delivery_rejected', { taskId: delivery.taskId, code: errorCode(error) }); await this.options.privateStore.delivered(c.uid,delivery.taskId).catch(() => undefined); } }
    }
    candidates.sort((a,b) => (this.scheduled.get(serialKey(a.binding,a.task)) ?? 0) - (this.scheduled.get(serialKey(b.binding,b.task)) ?? 0) || a.task.messageSeq - b.task.messageSeq);
    for (const candidate of candidates) {
      assert(); if (this.active.size+this.probes >= Math.min(4,this.options.maxConcurrent ?? 4)) break;
      const key = serialKey(candidate.binding,candidate.task); if (this.active.has(key)) continue;
      const controller = new AbortController(); this.scheduled.set(key,this.now());
      const promise = this.execute(c,candidate.binding,candidate.task,candidate.snapshot,controller.signal).catch(error => { this.log('team_task_deferred', { taskId: candidate.task.taskId, code: errorCode(error) }); this.deferred.set(candidate.task.taskId,this.now()+(this.options.retryMs??5000)); }).finally(() => { this.active.delete(key); this.wake(); });
      this.active.set(key,{taskId:candidate.task.taskId,controller,promise});
    }
  }
  private async execute(c: TeamAccountContext, b: SharedBotBinding, task: HostTask, snapshot: TeamPersonaSnapshot, signal: AbortSignal) {
    const execution = new AbortController(); signal = AbortSignal.any([signal, execution.signal]);
    const assert = () => { this.assert(c); signal.throwIfAborted(); };
    const fields = {...this.hostFields(b),taskId:task.taskId}, paths = await this.options.privateStore.owner(c.uid), bins = this.options.binaries(); assert();
    const prepared = await this.command<TaskContext>(c,b,'prepareTask',fields); assert();
    const scope: BotChatScope = { teamId:b.teamId, sharedBotId:b.sharedBotId, conversationId:task.conversationId, sessionEpoch:b.sessionEpoch };
    const run = this.options.run ?? runTeamHost;
    const inspected = await run({ executable:bins.host,home:paths.home,tmp:paths.tmp,signal,assertCurrent:assert,bootstrap:{version:3,type:'inspect_scope',storageRoot:paths.runtime,workerExecutable:bins.worker,scope} }); assert();
    const cursor = inspected.cursor, sourceScope = {teamId:b.teamId,generation:b.generation,conversationId:task.conversationId};
    if (cursor.lastMessageSeq >= prepared.messageSeq) throw new Error('TASK_INPUT_ALREADY_COMMITTED');
    const source = await this.options.service.hostReadChat(c,{...sourceScope,...(cursor.sessionRevision === 0 ? {mode:'recent' as const,throughSeq:prepared.messageSeq} : {mode:'range' as const,fromSeqExclusive:cursor.lastMessageSeq,toSeqInclusive:prepared.messageSeq})}); assert();
    const changes = cursor.lastMessageSeq ? await this.options.service.hostReadChat(c,{...sourceScope,mode:'changes',afterChangeSeq:cursor.lastChangeSeq,throughMessageSeq:cursor.lastMessageSeq}) : undefined; assert();
    if (changes && (changes.lastChangeSeq < source.lastChangeSeq
      || changes.messages.some(message => message.changeSeq > source.lastChangeSeq))) throw new Error('CHAT_CHANGED');
    if (prepared.deployment.definitionDigest !== b.snapshot || prepared.deployment.accountEpoch !== b.accountEpoch || prepared.inputs.some(g => !snapshot.documents.some(d => d.resourceId === g.resourceId && d.digest === g.digest))
      || prepared.inputs.length !== snapshot.documents.length) throw new Error('INPUT_GRANT_REQUIRED');
    const reference = await this.options.privateStore.dataRef(c.uid,{schemaVersion:1,batchId:task.taskId,...scope,expectedSessionRevision:cursor.sessionRevision,
      fromSeqExclusive:cursor.sessionRevision === 0 ? Math.max(0,prepared.messageSeq-200) : cursor.lastMessageSeq,toSeqInclusive:prepared.messageSeq,lastChangeSeq:source.lastChangeSeq,
      bootstrap:cursor.sessionRevision === 0,currentMessageId:prepared.messageId,messages:source.messages,changes:changes?.messages ?? []});
    const references = [reference]; let interval: ReturnType<typeof setInterval> | undefined;
    try {
    assert(); const accounts = await this.options.modelAccounts(snapshot.credential_id,snapshot.model_provider,snapshot.provider_account_profiles); assert(); if (!accounts) throw new Error('TEAM_MODEL_UNAVAILABLE');
    const startPayload={...fields,expectedLastChangeSeq:source.lastChangeSeq};
    const started = await this.command<TaskContext & {execute:boolean;permitId?:string}>(c,b,'startTask',startPayload,digest(JSON.stringify(['startTask',startPayload,reference.sha256]))); assert();
    if (!started.execute || !started.permitId) { this.deferred.set(task.taskId,this.now()+(this.options.retryMs??5000)); return; }
    this.deferred.delete(task.taskId);
    const permitId = started.permitId;
    const intent: SavedExecution = { schemaVersion:1, scope, teamId:b.teamId,generation:b.generation,taskId:task.taskId,
      commandId:digest(JSON.stringify([task.taskId,'delivery'])),deadlineAt:started.task.deadline,payload:{...fields,permitId,inputGrantIds:b.grantIds} };
    await this.options.privateStore.saveExecution(c.uid,intent); assert();
    const authorize = async () => { assert(); await this.command(c,b,'validateTask',{...fields,permitId,expectedLastChangeSeq:source.lastChangeSeq}); assert(); };
    interval = setInterval(() => { void authorize().catch(error => { if (definitive(error)) execution.abort(errorCode(error)); }); },2000); interval.unref();
      const {credential_id:_credential,provider_account_profiles:_profiles,instructions,documents,...model} = snapshot;
      let result: TeamHostResult | undefined, failure: unknown;
      try { result = await run({executable:bins.host,home:paths.home,tmp:paths.tmp,signal,assertCurrent:assert,authorize,
        bootstrap:{version:3,type:'run',storageRoot:paths.runtime,workerExecutable:bins.worker,scope,taskId:task.taskId,invocationId:randomUUID(),instructions,
          inputBatchRef:reference,knowledge:documents.filter(d=>d.kind==='file').map(d=>({id:d.resourceId,title:d.name,text:d.text})),deadlineAt:started.task.deadline,
          model,credentials:providerAccountEnvelope(accounts,snapshot.provider_account_profiles),limits:{max_turns:20,context_window_tokens:2000000,output_tokens_per_request:4096,physical_dispatches:32}},
        accountCredentials:async request => { await authorize();
          const permitted = accounts.providerAccountCredentials[request.accountId]; if (!permitted || permitted.identity_generation !== request.identityGeneration) throw new Error('TEAM_MODEL_UNAVAILABLE');
          await this.options.renewModelAccount?.(request.accountId,request.identityGeneration,request.credentialRevision); assert();
          const current = await this.options.modelAccounts(snapshot.credential_id,snapshot.model_provider,snapshot.provider_account_profiles); assert(); if (!current) throw new Error('TEAM_MODEL_UNAVAILABLE'); return current; },
        readChat:async query => { await authorize();
          if (query.kind==='before' && query.before_seq>prepared.messageSeq+1 || query.kind==='range' && query.to_seq_inclusive>prepared.messageSeq) throw new Error('HISTORY_BOUND_EXCEEDED');
          const request: ChatReadRequest = {...sourceScope,...(query.kind==='before' ? {mode:'before' as const,beforeSeq:query.before_seq,limit:query.limit} : query.kind==='range' ? {mode:'range' as const,fromSeqExclusive:query.from_seq_exclusive,toSeqInclusive:query.to_seq_inclusive} : {mode:'byId' as const,messageId:query.message_id})};
          const result = await this.options.service.hostReadChat(c,request); assert(); if (result.messages.some(message=>message.seq>prepared.messageSeq)) throw new Error('HISTORY_BOUND_EXCEEDED');
          await authorize(); const ref = await this.options.privateStore.dataRef(c.uid,{messages:result.messages,complete:query.kind !== 'before' || !result.hasMore,nextBeforeSeq:query.kind === 'before' && result.hasMore ? result.messages[0]?.seq ?? null : null}); references.push(ref); return ref; },
      }); } catch (error) { failure = error; }
      // The permit is spent: whatever stopped the worker, the task ends with a result and the model never runs again for it.
      if (!result) {
        const code = signal.aborted && typeof signal.reason === 'string' ? signal.reason : errorCode(failure);
        this.log('team_task_failed', { taskId: task.taskId, code, aborted: signal.aborted });
        result = await this.options.privateStore.savedResult(c.uid,task.taskId).catch(() => undefined) ?? this.failedResult(intent);
        await this.saveResult(c,intent,result); return; // Delivered by discovery once the account and fence are current again.
      }
      const delivery = await this.saveResult(c,intent,result); assert();
      await this.command(c,b,'completeTask',delivery.payload,delivery.commandId); assert(); await this.options.privateStore.delivered(c.uid,task.taskId);
    } finally { clearInterval(interval); await Promise.all(references.map(ref=>this.options.privateStore.releaseDataRef(c.uid,ref))); }
  }
  private failedResult(intent: SavedExecution): TeamHostResult {
    return { status: 'failed', cursor: noCursor, finishedAt: Math.min(this.now(), intent.deadlineAt) };
  }
  private async saveResult(c: TeamAccountContext, intent: SavedExecution, result: TeamHostResult) {
    const payload = {...intent.payload,outcome:result.status==='completed'?'completed':'failed',text:result.text || 'The assistant could not complete this request.',finishedAt:result.finishedAt,
      outputAudience:{mode:'published_inputs'},usage:result.usage ?? {status:'missing'},...(result.document ? {document:{title:result.document.title,body:result.document.markdown}} : {})};
    const delivery = {...intent,payload}; await this.options.privateStore.saveDelivery(c.uid,delivery); return delivery;
  }
  close() { this.closed=true; clearTimeout(this.timer); for(const stop of this.stops)stop(); for(const a of this.active.values())this.abort(a,'CLOSED'); this.selections.clear(); }
}
