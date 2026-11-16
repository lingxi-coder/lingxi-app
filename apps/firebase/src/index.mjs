import { readFileSync } from 'node:fs';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { getMessaging } from 'firebase-admin/messaging';
import { onRequest } from 'firebase-functions/v2/https';
import { onDocumentCreated, onDocumentUpdated, onDocumentWritten } from 'firebase-functions/v2/firestore';
import { onTeamMessageCreated } from './team-message-push.mjs';
import { mirrorFriendChat, mirrorFriendRead } from './friend-summary.mjs';
import { onSchedule } from 'firebase-functions/v2/scheduler';
import { defineSecret } from 'firebase-functions/params';
import { createMarketGateway } from './market-gateway.mjs';
import { createHandler } from './catalog.mjs';
import { CatalogStore } from './store.mjs';
import { createPushHandler, PushStore } from './push.mjs';
import { PAIR_CONVERSATION_ID, createTeamsHandler, TeamsStore } from './teams.mjs';
import { FriendsStore, friendNotifier } from './friends.mjs';
import { sweepUploads } from './team-attachments.mjs';
import { OAuth2Client } from 'google-auth-library';
import { createTeamMarketEligibilityHandler, TeamMarketEligibilityStore } from './team-market-eligibility.mjs';

const app = initializeApp();
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
  origins: ['https://lingxios.net', 'https://lingxi-5e4b2.web.app'] });
const pushServiceAccount = 'push-api@lingxi-5e4b2.iam.gserviceaccount.com';

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

const teamsHandler = createTeamsHandler({ store: new TeamsStore(teamsDb), friends: friendsStore,
  verifyToken: token => getAuth(app).verifyIdToken(token, true),
  createCustomToken: uid => getAuth(app).createCustomToken(uid),
  origins: ['https://lingxios.net', 'https://lingxi-5e4b2.web.app', 'https://lingxi-5e4b2.firebaseapp.com',
    'http://localhost:5173', 'http://127.0.0.1:5173'] });
// Reuses an existing service identity. Default-database IAM and token-signing
// permission require a deployment preflight; no resource is provisioned here.
export const teamsApi = onRequest({ region: 'us-central1', minInstances: 0, maxInstances: 2, memory: '256MiB',
  timeoutSeconds: 60, invoker: 'public', serviceAccount: pushServiceAccount }, async (req, res) => {
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) if (value) headers.set(key, Array.isArray(value) ? value.join(', ') : value);
  const output = await teamsHandler(new Request(new URL(req.url, 'https://teams.internal'), { method: req.method, headers,
    ...(!['GET', 'HEAD'].includes(req.method) ? { body: req.rawBody } : {}) }));
  output.headers.forEach((value, key) => res.set(key, value));
  res.status(output.status).send(Buffer.from(await output.arrayBuffer()));
});

// Deployment requires the separately approved default-db/Eventarc IAM preflight.
export const onTeamMessage = onDocumentCreated({ document: 'teams/{teamId}/conversations/{conversationId}/messages/{messageId}',
  database: '(default)', region: 'us-central1', memory: '256MiB', maxInstances: 2, serviceAccount: pushServiceAccount,
}, event => onTeamMessageCreated({ db: getFirestore(app), pushStore, messaging: getMessaging(app), ...event.params, original: event.data?.data() }));

// Friend list summaries: a pair team's chat pointer and the owner's read mark are copied onto the friend cards, so clients
// need no per-friend listener. The document paths name the pair chat itself, so ordinary Team conversations never invoke
// these; the handlers still check the team (docs/FRIENDS.md). They are idempotent and monotonic, and the next event heals a lost one.
const friendChat = `teams/{teamId}/conversations/${PAIR_CONVERSATION_ID}`;
const friendTrigger = { database: '(default)', region: 'us-central1', memory: '256MiB', maxInstances: 2, serviceAccount: pushServiceAccount };
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
