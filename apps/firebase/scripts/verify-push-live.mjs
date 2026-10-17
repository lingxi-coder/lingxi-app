import { execFileSync } from 'node:child_process';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { Timestamp } from 'firebase-admin/firestore';
import { getMessaging } from 'firebase-admin/messaging';
import { buildPushMessage, REGISTRATION_TTL_MS } from '../src/push.mjs';

// Uses the operator's existing, short-lived gcloud session. No service-account
// key, refresh token, installation secret or FID is printed or saved.
const projectId = 'lingxi-5e4b2';
const accountArgs = process.env.PUSH_VERIFY_ACCOUNT ? [`--account=${process.env.PUSH_VERIFY_ACCOUNT}`] : [];
const args = process.argv.slice(2);
const send = args.includes('--send');
const ids = args.filter(arg => arg !== '--send');
const id = ids[0];
if (ids.length > 1 || (id && !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(id))) {
  throw new Error('Expected one installation UUID');
}
if (send && !id) throw new Error('A specific installation is required to send');
const credential = {
  async getAccessToken() {
    const access_token = execFileSync('gcloud', ['auth', 'print-access-token', `--project=${projectId}`, ...accountArgs], {
      encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'],
    }).trim();
    return { access_token, expires_in: 300 };
  },
};
const app = initializeApp({ projectId, credential });
// Firestore Admin requires ADC or a service-account credential. Read only the
// necessary fields through REST to reuse a short-lived operator access token.
const fields = ['uid', 'platform', 'appId', 'deviceModel', 'notificationsEnabled',
  'revision', 'expiresAt', 'lastSeenAt', 'registrationHash', ...(send ? ['fcmInstallationId'] : [])];
const query = new URLSearchParams(fields.map(field => ['mask.fieldPaths', field]));
if (!id) query.set('pageSize', '20');
const url = `https://firestore.googleapis.com/v1/projects/${projectId}/databases/push-notifications/documents/pushInstallations${id ? `/${id}` : ''}?${query}`;
function decode(field) {
  if ('timestampValue' in field) return Timestamp.fromDate(new Date(field.timestampValue));
  if ('integerValue' in field) return Number(field.integerValue);
  if ('booleanValue' in field) return field.booleanValue;
  return field.stringValue;
}
try {
  const { access_token } = await credential.getAccessToken();
  const response = await fetch(url, { headers: { Authorization: `Bearer ${access_token}` } });
  if (id && response.status === 404) {
    console.log(JSON.stringify({ exists: false }));
    process.exitCode = 1;
  } else if (!response.ok) throw new Error(`Firestore read failed: HTTP ${response.status}`);
  const result = response.ok ? await response.json() : {};
  const records = id ? (result.name ? [result] : []) : (result.documents ?? []);
  for (const doc of records) {
    const value = Object.fromEntries(Object.entries(doc.fields ?? {}).map(([key, field]) => [key, decode(field)]));
    const now = Date.now(), expiresAt = value.expiresAt?.toMillis();
    let claimMatchesInstallation = false;
    if (value.registrationHash) {
      const claimResponse = await fetch(`https://firestore.googleapis.com/v1/projects/${projectId}/databases/push-notifications/documents/pushRegistrationClaims/${value.registrationHash}?mask.fieldPaths=installationId&mask.fieldPaths=expiresAt`,
        { headers: { Authorization: `Bearer ${access_token}` } });
      if (claimResponse.ok) {
        const claim = await claimResponse.json();
        claimMatchesInstallation = !!claim.fields?.expiresAt && claim.fields?.installationId?.stringValue === doc.name.split('/').at(-1) &&
          decode(claim.fields.expiresAt).toMillis() === expiresAt;
      } else if (claimResponse.status !== 404) throw new Error(`Claim read failed: HTTP ${claimResponse.status}`);
    }
    console.log(JSON.stringify({ installation: doc.name.split('/').at(-1).slice(0, 8), bound: !!value.uid,
      user: value.uid?.slice(0, 8), platform: value.platform, appId: value.appId,
      model: value.deviceModel, notificationsEnabled: value.notificationsEnabled,
      revision: value.revision, claimMatchesInstallation,
      expiresAt: Number.isFinite(expiresAt) ? new Date(expiresAt).toISOString() : null,
      leaseIsThirtyDays: !!value.lastSeenAt && expiresAt - value.lastSeenAt.toMillis() === REGISTRATION_TTL_MS }));
    if (send) {
      if (!value.uid || !value.fcmInstallationId || !value.notificationsEnabled || !claimMatchesInstallation || !(expiresAt > now)) {
        throw new Error('Installation is not active and authorized for notifications');
      }
      const message = buildPushMessage(value, { title: '灵犀 FCM 真机验证',
        body: `推送通道验证 ${new Date(now).toISOString()}`, ttlSeconds: 60 }, now);
      try {
        const receipt = await getMessaging(app).send(message);
        console.log(JSON.stringify({ accepted: true, receipt, ttlSeconds: 60 }));
      } catch (error) {
        console.log(JSON.stringify({ accepted: false, code: error.code ?? 'unknown' }));
        process.exitCode = 1;
      }
    }
  }
  if (!id && !records.length) console.log(JSON.stringify({ installations: 0 }));
} finally { await deleteApp(app); }
