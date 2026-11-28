import { createDatasetRender } from './dataset-render.mjs';
import { getStorage } from 'firebase-admin/storage';
import { createDatasetPages } from './dataset-pages.mjs';
import { createTeamVisualizations } from './team-visualizations.mjs';
import { createDatasetBrowserHttp } from './dataset-browser-bridge.mjs';
import { readFileSync } from 'node:fs';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { getMessaging } from 'firebase-admin/messaging';
import { onRequest } from 'firebase-functions/v2/https';
import { onDocumentCreated, onDocumentUpdated, onDocumentWritten } from 'firebase-functions/v2/firestore';
import { getFunctions } from 'firebase-admin/functions';
import { onTaskDispatched } from 'firebase-functions/v2/tasks';
import { info } from 'firebase-functions/logger';
import { hasNotificationRollout,inboxNotificationEnabled } from './inbox-rollout.mjs';
import { onTeamMessageCreated } from './team-message-push.mjs';
import { InboxStore } from './inbox-store.mjs';
import { InboxDelivery } from './inbox-delivery.mjs';
import { InboxJobs } from './inbox-jobs.mjs';
import { HeldReplyJobs, becameHeld } from './held-reply-jobs.mjs';
import { ReplyRoundJobs, becameCandidate, becameWinner } from './reply-round-jobs.mjs';
import { mirrorFriendChat, mirrorFriendRead } from './friend-summary.mjs';
import { onSchedule } from 'firebase-functions/v2/scheduler';
import { defineSecret } from 'firebase-functions/params';
import { createMarketGateway } from './market-gateway.mjs';
import { createHandler } from './catalog.mjs';
import { CatalogStore } from './store.mjs';
import { createPushHandler, PushStore } from './push.mjs';
import { PAIR_CONVERSATION_ID, createTeamsHandler, datasetBodyLimit, teamProfileBodyLimit, TeamsStore } from './teams.mjs';
import { FriendsStore, friendNotifier } from './friends.mjs';
import { sweepUploads } from './team-attachments.mjs';
import { OAuth2Client } from 'google-auth-library';
import { deliverDatasetTaskOutput } from './dataset-task-delivery.mjs';
import { createPrivateDatasetFetch } from './dataset-cloud-run.mjs';
import { createDatasetAgentTurn, firestoreDatasetTurns } from './dataset-agent-turn.mjs';
import { createDatasetGateway } from './dataset-product.mjs';
import { createProductDataTransport } from './lingxi-data-product.mjs';
import { createTeamMarketEligibilityHandler, TeamMarketEligibilityStore } from './team-market-eligibility.mjs';
import { createLlmHandler } from './llm-api.mjs';
import { VaultStore, parseKeyring } from './llm-vault.mjs';
import { serveStreaming } from './llm-http.mjs';
import { createChat } from './llm-chat.mjs';
import { upstreamTransport } from './llm-egress.mjs';

const app = initializeApp();
const productionProject = 'lingxi-5e4b2';
const firebaseProject = app.options.projectId ?? process.env.GCLOUD_PROJECT ?? productionProject;
const datasetStaging = /^lingxi-dataset-v2-stg-[a-z0-9]+$/u.test(firebaseProject);
const browserOrigins = [
  ...(firebaseProject === productionProject ? ['https://lingxios.net'] : []),
  `https://${firebaseProject}.web.app`, `https://${firebaseProject}.firebaseapp.com`,
  'http://localhost:5173', 'http://127.0.0.1:5173',
];
const seed = JSON.parse(readFileSync(new URL('./seed.json', import.meta.url), 'utf8'));
const handle = createHandler(new CatalogStore(getFirestore(app, 'model-catalog'), seed),
  (token) => getAuth(app).verifyIdToken(token, true),
  ['https://lingxios.net', 'https://lingxi-5e4b2.web.app', 'https://lingxi-5e4b2.firebaseapp.com', 'http://localhost:5173', 'http://127.0.0.1:5173']);

export const catalogApi = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, memory: '512MiB', timeoutSeconds: 60, invoker: 'public', serviceAccount: 'catalog-api@lingxi-5e4b2.iam.gserviceaccount.com' }, async (req, res) => {
  const url = new URL(req.url, 'https://catalog.internal');
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  const input = new Request(url, { method: req.method, headers, ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) });
  const output = await handle(input);
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status).send(Buffer.from(await output.arrayBuffer()));
});

const marketProxySecret = defineSecret('market-proxy-secret');
export const marketGateway = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, concurrency: 4, memory: '512MiB', timeoutSeconds: 60, invoker: 'public', serviceAccount: 'market-api@lingxi-5e4b2.iam.gserviceaccount.com', secrets: [marketProxySecret] }, createMarketGateway({ origin: 'https://market-api-55470483519.us-central1.run.app', secret: () => marketProxySecret.value() }));

const pushStore = new PushStore(getFirestore(app, 'push-notifications'));
const pushHandler = createPushHandler({ store: pushStore, messaging: getMessaging(app),
  verifyToken: (token) => getAuth(app).verifyIdToken(token, true),
  origins: [...(firebaseProject === productionProject ? ['https://lingxios.net'] : []), `https://${firebaseProject}.web.app`] });
const pushServiceAccount = `push-api@${firebaseProject}.iam.gserviceaccount.com`;

export const pushApi = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, memory: '256MiB',
  timeoutSeconds: 60, invoker: 'public', serviceAccount: pushServiceAccount }, async (req, res) => {
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  const input = new Request(new URL(req.url, 'https://push.internal'), { method: req.method, headers,
    ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) });
  const output = await pushHandler(input);
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status).send(Buffer.from(await output.arrayBuffer()));
});

const teamsDb = getFirestore(app);
// Friends share teamsApi and the (default) database. Email lookup uses Admin Auth; pushes use the Team push path.
const friendsStore = new FriendsStore(teamsDb, Date.now, { auth: getAuth(app),
  notify: friendNotifier({ db: teamsDb, pushStore, messaging: getMessaging(app) }) });

export const prunePushRegistrations = onSchedule({ schedule: 'every 24 hours', timeZone: 'Etc/UTC',
  region: 'us-central1', memory: '256MiB', timeoutSeconds: 540, maxInstances: 1,
  serviceAccount: pushServiceAccount }, async () => { await pushStore.prune(); await friendsStore.prune(); await sweepUploads(teamsDb); });

const datasetOrigin = process.env.LINGXI_DATASET_ORIGIN;
const datasetGateway = createDatasetGateway(datasetOrigin,
  datasetOrigin && process.env.LINGXI_DATASET_PRIVATE_RUN === '1' ? createPrivateDatasetFetch(datasetOrigin) : fetch,
  token => getAuth(app).verifyIdToken(token, true));
const productDataOrigin = process.env.LINGXI_DATA_PRODUCT_ORIGIN;
const productData = createProductDataTransport(productDataOrigin,
  productDataOrigin && process.env.LINGXI_DATA_PRODUCT_PRIVATE_RUN === '1' ? createPrivateDatasetFetch(productDataOrigin) : fetch);
const datasetBrowserHandler = createDatasetBrowserHttp({
  origin: process.env.LINGXI_DATASET_BROWSER_ORIGIN,
  call: datasetGateway?.browser,
});
const datasetPages = process.env.LINGXI_DATASET_PAGES_BUCKET ? createDatasetPages({
  db: teamsDb, bucket: getStorage(app).bucket(process.env.LINGXI_DATASET_PAGES_BUCKET), datasets: datasetGateway?.datasets,
}) : undefined;
const teamVisualizations = createTeamVisualizations({
  db: teamsDb, bucket: process.env.LINGXI_DATASET_PAGES_BUCKET ? getStorage(app).bucket(process.env.LINGXI_DATASET_PAGES_BUCKET) : undefined,
  core: datasetGateway?.command, channels: process.env.LINGXI_TEAM_VISUALIZATION_CHANNELS,
});
export const pruneTeamVisualizations = onSchedule({ schedule: 'every 5 minutes', timeZone: 'Etc/UTC',
  region: 'us-central1', memory: '256MiB', timeoutSeconds: 240, maxInstances: 1,
  serviceAccount: pushServiceAccount }, async () => { await teamVisualizations?.sweep(); });
const renderOrigin = process.env.LINGXI_DATASET_RENDER_ORIGIN;
const useRenderSecret = datasetStaging || !!renderOrigin;
// Declared only where it is used: firebase-tools requires every declared secret to exist, and production has no render service.
const renderKey = useRenderSecret ? defineSecret('LINGXI_DATASET_RENDER_KEY') : undefined;
let renderDataset;
const teamsHandler = createTeamsHandler({ store: new TeamsStore(teamsDb), friends: friendsStore,
  datasets: datasetGateway?.datasets, datasetCommand: datasetGateway?.command, datasetRuns: datasetGateway?.run, datasetPages: datasetPages?.execute,
  visualizations: teamVisualizations,
  datasetRender: renderKey && renderOrigin && datasetPages ? (...args) => (renderDataset ??= createDatasetRender({
    origin: renderOrigin, secret: renderKey.value(), pages: datasetPages, db: teamsDb,
  }))(...args) : undefined,
  data: productData,
  verifyToken: token => getAuth(app).verifyIdToken(token, true),
  createCustomToken: uid => getAuth(app).createCustomToken(uid),
  origins: browserOrigins });
// Reuses an existing service identity. Default-database IAM and token-signing
// permission require a deployment preflight; no resource is provisioned here.
export const teamsApi = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, memory: '256MiB',
  timeoutSeconds: 60, invoker: 'public', serviceAccount: pushServiceAccount,
  ...(useRenderSecret ? { secrets: [renderKey] } : {}) }, async (req, res) => {
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  const url = new URL(req.url, 'https://teams.internal');
  const limit = teamProfileBodyLimit(url.pathname) ?? datasetBodyLimit(url.pathname);
  if (limit && req.rawBody?.byteLength > limit) {
    res.status(413).json({ error: { code: 'REQUEST_TOO_LARGE' } });
    return;
  }
  const request = new Request(url, { method: req.method, headers,
    ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) });
  const output = await datasetBrowserHandler?.(request) ?? await teamsHandler(request);
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status).send(Buffer.from(await output.arrayBuffer()));
});

// The source change receipts are committed with the conversation transaction by
// both SDK and HTTP writers. Eventarc and Cloud Tasks use the same ordered fold.
const enqueue = async (payload, id, delay = 0) => {
  const opts = { scheduleDelaySeconds: delay, dispatchDeadlineSeconds: 540, ...(id ? { id } : {}) };
  try {
    await getFunctions(app).taskQueue('locations/us-central1/functions/inboxWork').enqueue(payload, opts);
  } catch (e) {
    if (e.code !== 'functions/task-already-exists') throw e;
  }
};
const inbox = new InboxStore(teamsDb,Date.now,enqueue,cost=>info('inbox.fanout',cost));
const delivery = new InboxDelivery({ db: teamsDb, pushStore, messaging: getMessaging(app), enqueue });
const heldReplies = new HeldReplyJobs({ db: teamsDb, pushStore, messaging: getMessaging(app), enqueue });
const replyRounds = new ReplyRoundJobs({ db: teamsDb, enqueue });
const jobs = new InboxJobs(inbox, delivery, enqueue, heldReplies, replyRounds);
const triggerOpts = {
  database: '(default)',
  region: 'us-central1',
  memory: '256MiB',
  maxInstances: 4,
  timeoutSeconds: 540,
  serviceAccount: pushServiceAccount,
  retry: true
};
export const inboxWork = onTaskDispatched(
  {
    region: 'us-central1',
    memory: '512MiB',
    timeoutSeconds: 540,
    serviceAccount: pushServiceAccount,
    invoker: [pushServiceAccount],
    retryConfig: { maxAttempts: 20, minBackoffSeconds: 2, maxBackoffSeconds: 120 },
    rateLimits: { maxConcurrentDispatches: 16 }
  },
  req => jobs.run(req.data)
);
const inboxHandler = createTeamsHandler({
  inbox: {
    execute: async (uid, p) => {
      if (p?.action === 'list' && !p.after) await jobs.bootstrap(uid);
      return inbox.execute(uid, p);
    }
  },
  verifyToken: token => getAuth(app).verifyIdToken(token, true),
  origins: [
    'https://lingxios.net',
    'https://lingxi-5e4b2.web.app',
    'https://lingxi-5e4b2.firebaseapp.com',
    'http://localhost:5173',
    'http://127.0.0.1:5173'
  ]
});
export const inboxApi = onRequest(
  {
    region: 'us-central1',
    memory: '256MiB',
    maxInstances: 4,
    timeoutSeconds: 60,
    invoker: 'public',
    serviceAccount: pushServiceAccount
  },
  async (req, res) => {
    const headers = new Headers();
    for (const [key, value] of Object.entries(req.headers))
      if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
    const response = await inboxHandler(
      new Request(new URL(req.url, 'https://inbox.internal'), {
        method: req.method,
        headers,
        ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {})
      })
    );
    response.headers.forEach((value, key) => res.set(key, value));
    res.status(response.status).send(Buffer.from(await response.arrayBuffer()));
  }
);
const refOf = (kind, { sourceId, teamId, conversationId }) => ({
  kind,
  sourceId: sourceId ?? teamId,
  conversationId
});
export const onDatasetTaskDelivery = onDocumentCreated(
  { ...triggerOpts, document: 'datasetV2Deliveries/{deliveryId}', retry: true },
  event => deliverDatasetTaskOutput(teamsDb, event.params.deliveryId));

export const onTeamMessage = onDocumentCreated(
  { ...triggerOpts, maxInstances: 2, document: 'teams/{teamId}/conversations/{conversationId}/messages/{messageId}' },
  async e=>{
    const work=[jobs.wake(refOf('team',e.params))];
    if(hasNotificationRollout()) work.push(onTeamMessageCreated({db:teamsDb,pushStore,messaging:getMessaging(app),...e.params,
      original:e.data?.data(),includeRecipient:uid=>!inboxNotificationEnabled(uid)}));
    const results=await Promise.allSettled(work),failure=results.find(r=>r.status==='rejected');
    if(failure) throw failure.reason;
  }
);
const conversationTrigger = (collection, kind) =>
  onDocumentWritten({ ...triggerOpts, document: collection + '/{sourceId}/conversations/{conversationId}' }, e =>
    jobs.conversation(refOf(kind, e.params), e.data.before.data(), e.data.after.data())
  );
export const onInboxTeamConversation = conversationTrigger('teams', 'team');
export const onInboxServiceConversation = conversationTrigger('services', 'service');
const readTrigger = (collection, kind) =>
  onDocumentWritten(
    { ...triggerOpts, document: collection + '/{sourceId}/conversations/{conversationId}/reads/{uid}' },
    async e => {
      const ref = refOf(kind, e.params),
        uid = e.params.uid;
      if (await inbox.fold(uid, ref)) await jobs.wake(ref);

    }
  );
export const onInboxTeamRead = readTrigger('teams', 'team');
export const onInboxServiceRead = readTrigger('services', 'service');
const membershipTrigger = (collection, kind) =>
  onDocumentWritten({ ...triggerOpts, document: collection + '/{sourceId}/members/{uid}' }, async e => {
    const { uid, sourceId } = e.params;
    if (e.data.before.data()?.generation === e.data.after.data()?.generation) return;
    await enqueue({type:'reconcile',uid,kind,sourceId});
    if (e.data.after.exists) await enqueue({type:'scanConversations',uid,kind,sourceId});
  });
export const onInboxTeamMember = membershipTrigger('teams', 'team');
export const onInboxServiceMember = membershipTrigger('services', 'service');
const taskTrigger = (collection, kind) =>
  onDocumentWritten({ ...triggerOpts, document: collection + '/{sourceId}/tasks/{taskId}' }, async e => {
    const task = e.data.after.data() ?? e.data.before.data();
    if (!task?.requester || !task.conversationId) return;
    const ref = {
      kind,
      sourceId: e.params.sourceId,
      conversationId: task.conversationId,
      generation: task.requester.generation
    };
    const work = [jobs.run({ type: 'task', uid: task.requester.uid, ref, taskId: e.params.taskId })];
    // A reply just held for its Bot's maintainer: its time limit and the maintainer's push. Only a Team holds replies.
    if (kind === 'team' && becameHeld(e.data.before.data(), e.data.after.data())) work.push(heldReplies.held(e.params.sourceId, e.params.taskId));
    // A task that opens a reply round, and one that wins it: the round's two timers (`reply-round-jobs.mjs`). Only a Team has rounds.
    if (kind === 'team' && becameCandidate(e.data.before.data(), e.data.after.data())) work.push(replyRounds.candidate(e.params.sourceId, e.params.taskId));
    if (kind === 'team' && becameWinner(e.data.before.data(), e.data.after.data())) work.push(replyRounds.winner(e.params.sourceId, e.params.taskId));
    const results = await Promise.allSettled(work), failure = results.find(r => r.status === 'rejected');
    if (failure) throw failure.reason;
  });
export const onInboxTeamTask = taskTrigger('teams', 'team');
export const onInboxServiceTask = taskTrigger('services', 'service');
export const onInboxNotice = onDocumentCreated({ ...triggerOpts, document: 'users/{uid}/inboxNotices/{id}' }, e =>
  delivery.onNotice(e.params.uid, e.params.id)
);
export const onInboxReadBatch = onDocumentCreated({...triggerOpts,document:'users/{uid}/inboxReadBatches/{id}'},
  e=>enqueue({type:'readBatch',uid:e.params.uid,id:e.params.id}));
export const onInboxBootstrap = onDocumentCreated(
  { ...triggerOpts, document: 'users/{uid}/inboxJobs/bootstrap' },
  e => enqueue({ type: 'bootstrap', uid: e.params.uid })
);
export const onInboxSourceBinding = onDocumentWritten(
  { ...triggerOpts, document: 'users/{uid}/inboxSources/{sourceId}' },
  e => {
    if (!e.data.after.data()?.enabled) return enqueue({type:'reconcile',uid:e.params.uid,kind:'assistant',sourceId:e.params.sourceId});
  }
);
const scopePolicyTrigger = (collection, kind) =>
  onDocumentUpdated(
    {
      ...triggerOpts,
      document: collection + '/{sourceId}'
    },
    async e => {
      const before = e.data.before.data(),
        after = e.data.after.data();
      if (before.state === after.state && JSON.stringify(before.audience) === JSON.stringify(after.audience)) return;
      await enqueue({type:'scanMembers',kind,sourceId:e.params.sourceId});
    }
  );
export const onInboxServicePolicy = scopePolicyTrigger('services', 'service');
export const onInboxTeamPolicy = scopePolicyTrigger('teams', 'team');

// Friend list summaries: a pair team's chat pointer and the owner's read mark are copied onto the friend cards, so clients
// need no per-friend listener. The document paths name the pair chat itself, so ordinary Team conversations never invoke
// these; the handlers still check the team (docs/FRIENDS.md). Idempotent, monotonic updates allow event retry.
const friendChat = `teams/{teamId}/conversations/${PAIR_CONVERSATION_ID}`;
const friendTrigger = { retry: true, database: '(default)', region: 'us-central1', memory: '256MiB', maxInstances: 2, serviceAccount: pushServiceAccount };
export const onFriendChatUpdated = onDocumentUpdated({ document: friendChat, ...friendTrigger },
  event => mirrorFriendChat({ db: getFirestore(app), teamId: event.params.teamId, conversationId: PAIR_CONVERSATION_ID, after: event.data?.after.data() }));
export const onFriendChatRead = onDocumentWritten({ document: `${friendChat}/reads/{uid}`, ...friendTrigger },
  event => mirrorFriendRead({ db: getFirestore(app), teamId: event.params.teamId, conversationId: PAIR_CONVERSATION_ID, uid: event.params.uid, after: event.data?.after.data() }));

const marketServiceVerifier = new OAuth2Client();
const eligibilityHandler = createTeamMarketEligibilityHandler({ store: new TeamMarketEligibilityStore(getFirestore(app)),
  verifyServiceToken: async (token, audience) => (await marketServiceVerifier.verifyIdToken({ idToken: token, audience })).getPayload(),
  config: () => ({ serviceEmail: process.env.TEAMS_MARKET_SERVICE_EMAIL, serviceSubject: process.env.TEAMS_MARKET_SERVICE_SUBJECT,
    serviceAudience: process.env.TEAMS_MARKET_ELIGIBILITY_AUDIENCE, projectId: app.options.projectId ?? process.env.GCLOUD_PROJECT,
    tenantId: process.env.TEAMS_AUTH_TENANT_ID || null }) });
// IAM and exact OIDC claims both apply. Production allowlist configuration is
// required; an absent configuration stays closed instead of trusting a viewer.
export const marketTeamEligibility = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, memory: '256MiB',
  timeoutSeconds: 30, invoker: 'private', serviceAccount: pushServiceAccount }, async (req, res) => {
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  const output = await eligibilityHandler(new Request(new URL(req.url, 'https://teams.internal'), { method: req.method, headers,
    ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) }));
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status).send(Buffer.from(await output.arrayBuffer()));
});

// Model-provider key vault and proxy. Keys are sealed under the `provider-vault-key` keyring (Secret Manager) and the
// function runs as its own identity, which alone may read that secret. The Web calls the function URL directly
// (Hosting rewrites cap at 60 s and may buffer a stream); `/v1/llm` on Hosting serves the rest.
const vaultKey = defineSecret('provider-vault-key');
let vaultKeyring;
const vault = new VaultStore(teamsDb, () => (vaultKeyring ??= parseKeyring(vaultKey.value())));
const llmChat = createChat({ vault, transport: upstreamTransport() });
const datasetTurn = datasetGateway && process.env.LINGXI_DATASET_WEB_AGENT === '1' ? createDatasetAgentTurn({
  turns: firestoreDatasetTurns(teamsDb),
  model: llmChat.start,
  data: (uid, token, body, signal) => datasetGateway.datasets(teamsDb, { uid }, token, body, signal),
}) : undefined;
const llmHandler = createLlmHandler({
  vault, chat: llmChat.chat, test: llmChat.test, datasetTurn,
  verifyToken: token => getAuth(app).verifyIdToken(token, true),
  origins: browserOrigins,
  observe: entry => info('llm.request', entry),
});
export const llmApi = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 4, memory: '256MiB', timeoutSeconds: 300,
  invoker: 'public', serviceAccount: `llm-proxy@${firebaseProject}.iam.gserviceaccount.com`, secrets: [vaultKey] },
(req, res) => serveStreaming(llmHandler, req, res, 'https://llm.internal'));
