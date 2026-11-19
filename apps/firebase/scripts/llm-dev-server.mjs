// Local llmApi for Web development, on the Firestore and Auth emulators only. The keyring is random per run, so keys
// saved here are unreadable after a restart. By default the upstream is a fake that streams an echo; set
// LLM_DEV_UPSTREAM=real to call real providers with keys you save (they stay in the emulator).
//
//   firebase emulators:exec --only auth,firestore … "node scripts/llm-dev-server.mjs"
//   VITE_LLM_API_URL=http://127.0.0.1:4710/llmApi npm --prefix ../web run dev
import { createServer } from 'node:http';
import { randomBytes } from 'node:crypto';
import { initializeApp } from 'firebase-admin/app';
import { getAuth } from 'firebase-admin/auth';
import { getFirestore } from 'firebase-admin/firestore';
import { createLlmHandler } from '../src/llm-api.mjs';
import { createChat } from '../src/llm-chat.mjs';
import { EgressError, upstreamTransport } from '../src/llm-egress.mjs';
import { serveStreaming } from '../src/llm-http.mjs';
import { VaultStore, parseKeyring } from '../src/llm-vault.mjs';

const local = value => /^(127\.0\.0\.1|localhost):\d+$/u.test(value ?? '');
if (!local(process.env.FIRESTORE_EMULATOR_HOST) || !local(process.env.FIREBASE_AUTH_EMULATOR_HOST)) throw new Error('llm-dev-server runs on the local emulators only');
const app = initializeApp({ projectId: process.env.GCLOUD_PROJECT ?? 'demo-lingxi-teams-t0' }, 'llm-dev');
const keyring = parseKeyring(JSON.stringify({ current: 'dev', keys: { dev: randomBytes(32).toString('base64') } }));
const vault = new VaultStore(getFirestore(app), keyring);

const encoder = new TextEncoder();
const wait = (ms, signal) => new Promise((resolve, reject) => {
  const timer = setTimeout(resolve, ms);
  signal?.addEventListener('abort', () => { clearTimeout(timer); reject(new EgressError('ABORTED')); }, { once: true });
});
/** Answers like an OpenAI-compatible server: a little reasoning, then the last message echoed word by word. */
const fakeTransport = async ({ body, signal }) => {
  const { messages } = JSON.parse(body), last = messages.at(-1)?.content ?? '';
  if (last.includes('fail-auth')) return { status: 401, body: (async function* () { yield encoder.encode('{}'); })() };
  const words = `You said: ${last}`.split(/(?<= )/u);
  return { status: 200, body: (async function* () {
    yield encoder.encode(`data: ${JSON.stringify({ choices: [{ delta: { reasoning_content: 'Thinking about it…' } }] })}\n\n`);
    for (const word of words) { await wait(40, signal); yield encoder.encode(`data: ${JSON.stringify({ choices: [{ delta: { content: word } }] })}\n\n`); }
    yield encoder.encode('data: {"choices":[{"delta":{},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n');
  })() };
};
const { chat, test } = createChat({ vault, transport: process.env.LLM_DEV_UPSTREAM === 'real' ? upstreamTransport() : fakeTransport });
const origins = (process.env.LLM_DEV_ORIGINS ?? 'http://127.0.0.1:5173,http://localhost:5173,http://127.0.0.1:5181').split(',');
const handler = createLlmHandler({ vault, chat, test, origins, verifyToken: token => getAuth(app).verifyIdToken(token, true),
  observe: entry => console.log('llm', entry.route, entry.status, entry.code ?? '', `${entry.ms}ms`) });

const port = Number(process.env.LLM_DEV_PORT ?? 4710);
createServer(async (req, res) => {
  const parts = []; for await (const part of req) parts.push(part);
  req.rawBody = Buffer.concat(parts);
  res.set = (key, value) => res.setHeader(key, value);
  res.status = code => { res.statusCode = code; return res; };
  try { await serveStreaming(handler, req, res, `http://127.0.0.1:${port}`); }
  catch { if (!res.headersSent) { res.statusCode = 500; } res.end(); }
}).listen(port, '127.0.0.1', () => console.log(`LLM_DEV_READY http://127.0.0.1:${port}/llmApi (${process.env.LLM_DEV_UPSTREAM === 'real' ? 'real upstreams' : 'fake upstream'})`));
