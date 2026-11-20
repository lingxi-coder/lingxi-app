import { readFile } from 'node:fs/promises';
import { createServer } from 'node:https';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';

import { createDataActorProjection } from '../src/lingxi-data-actors.mjs';
import { createBotDataRecords } from '../src/lingxi-data-bot-records.mjs';
import { createDataRules } from '../src/lingxi-data-rules.mjs';
import { createDataSourceManagement } from '../src/lingxi-data-source-management.mjs';
import { createDataTemplates } from '../src/lingxi-data-templates.mjs';
import { createPrivatePreview } from '../src/lingxi-data-private-preview.mjs';
import { createFirebaseBotTokens } from '../src/lingxi-data-bot-tokens.mjs';
import { createProductDataListener } from '../src/lingxi-data-product.mjs';
import { createLingxiDataGateway } from '../src/lingxi-data.mjs';

function env(name) {
  const value = process.env[name];
  if (!value) throw new Error('Missing ' + name);
  return value;
}

const pinned = new URL(env('LINGXI_DATA_INTERNAL_ORIGIN'));
if (pinned.hostname !== '127.0.0.1' && pinned.hostname !== '[::1]') {
  throw new Error('The pinned Data service must use the local private listener');
}
const port = Number(env('LINGXI_DATA_PRODUCT_PORT'));
if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error('Invalid product Data adapter port');
const [cert, key] = await Promise.all([
  readFile(env('LINGXI_DATA_PRODUCT_TLS_CERT')),
  readFile(env('LINGXI_DATA_PRODUCT_TLS_KEY')),
]);
const projectId = env('GCLOUD_PROJECT');
const app = initializeApp({ projectId });
const projectActor = await createDataActorProjection(env('LINGXI_DATA_ACTOR_FILE'));
const db = getFirestore(app), auth = getAuth(app);
const gateway = createLingxiDataGateway({ db, endpoint: pinned.href, projectActor });
const secret = process.env.LINGXI_DATA_PREVIEW_KEY;
const botData = secret ? { db, endpoint: pinned.href, secret, projectActor,
  botToken: createFirebaseBotTokens({ auth, projectId, apiKey: env('LINGXI_DATA_FIREBASE_API_KEY'),
    emulator: process.env.FIREBASE_AUTH_EMULATOR_HOST }),
} : undefined;
const rules = botData && process.env.LINGXI_DATA_RULES === '1' ? createDataRules(botData) : undefined;
const listener = createProductDataListener({ verifyToken: token => auth.verifyIdToken(token, true), gateway,
  preview: botData ? createPrivatePreview(botData) : undefined, botRecords: botData ? createBotDataRecords(botData) : undefined, rules,
  sources: botData ? createDataSourceManagement(botData) : undefined, templates: createDataTemplates({ db, gateway }) });
const server = createServer({ cert, key, requestTimeout: 30_000, headersTimeout: 10_000 }, listener);
server.listen(port, '127.0.0.1');
if (rules) {
  async function poll() {
    try { await rules.poll(); }
    catch { console.error('Lingxi Data rule poll failed'); }
    finally { setTimeout(poll, 1_000).unref(); }
  }
  void poll();
}
