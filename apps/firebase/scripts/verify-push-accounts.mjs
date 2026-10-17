import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { initializeApp, deleteApp } from 'firebase-admin/app';
import { Timestamp } from 'firebase-admin/firestore';
import { getMessaging } from 'firebase-admin/messaging';
import { buildPushMessage, REGISTRATION_TTL_MS } from '../src/push.mjs';

class VerificationFailure extends Error {}

// Opt-in physical-device verification. Build/install PushAccountDeviceTest first.
// Sign-in/out happens in the real Android SDK. Opaque encrypted Auth preferences
// are backed up locally (0600) and restored together after process shutdown.
// Credentials are never parsed, logged or sent to another service.
const projectId = 'lingxi-5e4b2';
const serial = process.argv[2];
const reportPath = process.argv[3];
if (!serial || !/^[A-Za-z0-9:_-]+$/.test(serial) || !reportPath) {
  throw new Error('Usage: node scripts/verify-push-accounts.mjs <USB-serial> <report.json>');
}
const adb = process.env.ADB ?? 'adb';
const packageId = 'com.lingxi.code.debug';
const directory = 'no_backup/push-live-verification';
const session = randomUUID();
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));
const report = { project: projectId, session, startedAt: new Date().toISOString(), checks: {} };
function device(args, input) {
  return execFileSync(adb, ['-s', serial, ...args], { input, encoding: 'utf8', timeout: 20_000, stdio: ['pipe', 'pipe', 'pipe'] }).trim();
}
function status() {
  try {
    const value = JSON.parse(device(['exec-out', 'run-as', packageId, 'cat', `${directory}/status.json`]));
    return value.session === session ? value : null;
  } catch { return null; }
}
function command(action) {
  device(['shell', 'run-as', packageId, 'tee', `${directory}/control.json`], JSON.stringify({ session, action }));
}
function requireThat(value, description) { if (!value) throw new VerificationFailure(description); }
async function until(label, operation, timeoutMs = 90_000) {
  const deadline = Date.now() + timeoutMs;
  let nextNotice = Date.now() + 25_000;
  while (Date.now() < deadline) {
    const result = await operation();
    if (result) return result;
    if (expectInstrumentation && childDone) throw new VerificationFailure(`Instrumentation ended before ${label}`);
    if (Date.now() >= nextNotice) { console.log(JSON.stringify({ waiting: label })); nextNotice += 25_000; }
    await pause(700);
  }
  throw new VerificationFailure(`Timed out: ${label}`);
}
const originalNetwork = Object.fromEntries(['wifi_on', 'mobile_data'].map(key => [key, device(['shell', 'settings', 'get', 'global', key])]));
requireThat(Object.values(originalNetwork).every(value => ['0', '1'].includes(value)), 'Network settings must be stable before testing');
let networkChanged = false;
function offline() {
  networkChanged = true;
  device(['shell', 'svc', 'wifi', 'disable']);
  device(['shell', 'svc', 'data', 'disable']);
}
function restoreNetwork() {
  if (!networkChanged) return;
  device(['shell', 'svc', 'wifi', originalNetwork.wifi_on === '1' ? 'enable' : 'disable']);
  device(['shell', 'svc', 'data', originalNetwork.mobile_data === '1' ? 'enable' : 'disable']);
  networkChanged = false;
}
let token, tokenExpires = 0;
const credential = { async getAccessToken() {
  if (Date.now() >= tokenExpires) {
    token = execFileSync('gcloud', ['auth', 'print-access-token', `--project=${projectId}`,
      ...(process.env.PUSH_VERIFY_ACCOUNT ? [`--account=${process.env.PUSH_VERIFY_ACCOUNT}`] : [])],
    { encoding: 'utf8', timeout: 30_000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
    tokenExpires = Date.now() + 240_000;
  }
  return { access_token: token, expires_in: 300 };
} };
const app = initializeApp({ projectId, credential });
function decode(field) {
  if ('timestampValue' in field) return Timestamp.fromDate(new Date(field.timestampValue));
  if ('integerValue' in field) return Number(field.integerValue);
  if ('booleanValue' in field) return field.booleanValue;
  if ('nullValue' in field) return null;
  return field.stringValue;
}
async function document(path, fields) {
  const { access_token } = await credential.getAccessToken();
  const query = new URLSearchParams(fields.map(field => ['mask.fieldPaths', field]));
  const response = await fetch(`https://firestore.googleapis.com/v1/projects/${projectId}/databases/push-notifications/documents/${path}?${query}`,
    { headers: { Authorization: `Bearer ${access_token}` }, signal: AbortSignal.timeout(15_000) });
  if (response.status === 404) return null;
  if (!response.ok) throw new VerificationFailure(`Firestore HTTP ${response.status}`);
  const result = await response.json();
  return Object.fromEntries(Object.entries(result.fields ?? {}).map(([key, value]) => [key, decode(value)]));
}
async function authCall(action, body) {
  const { access_token } = await credential.getAccessToken();
  const response = await fetch(`https://identitytoolkit.googleapis.com/v1/projects/${projectId}/accounts:${action}`,
    { method: 'POST', headers: { Authorization: `Bearer ${access_token}`, 'Content-Type': 'application/json', 'x-goog-user-project': projectId },
      body: JSON.stringify(body), signal: AbortSignal.timeout(15_000) });
  if (!response.ok) throw new VerificationFailure(`Test account cleanup HTTP ${response.status}`);
  return response.json();
}
const deviceFields = ['uid', 'revision', 'fcmInstallationId', 'registrationHash', 'notificationsEnabled', 'expiresAt', 'lastSeenAt', 'appId'];
let installationId;
async function binding(uid, phase) {
  return until(`binding ${phase}`, async () => {
    const value = status();
    if (!value || value.phase !== phase || value.uid !== uid) return null;
    installationId ??= value.installationId;
    requireThat(value.installationId === installationId, 'Account change replaced the installation UUID');
    const bound = await document(`pushInstallations/${installationId}`, deviceFields);
    if (!bound || bound.uid !== uid || bound.revision !== value.revision) return null;
    requireThat(bound.appId === packageId && bound.notificationsEnabled && bound.fcmInstallationId, 'Registration is not deliverable');
    requireThat(bound.expiresAt.toMillis() - bound.lastSeenAt.toMillis() === REGISTRATION_TTL_MS, 'Registration lease is not thirty days');
    const claim = await document(`pushRegistrationClaims/${bound.registrationHash}`, ['installationId', 'expiresAt']);
    requireThat(claim?.installationId === installationId && claim.expiresAt.toMillis() === bound.expiresAt.toMillis(), 'Registration claim mismatch');
    return { status: value, bound };
  });
}
async function probe(bound, uid, label) {
  const id = randomUUID(), title = `FCM QA ${label} ${id.slice(0, 8)}`;
  const message = buildPushMessage({ ...bound, uid }, { title, body: 'Android account and offline logout verification', ttlSeconds: 180 });
  message.data.verificationId = id;
  const receipt = await getMessaging(app).send(message);
  return { id, title, receipt };
}
async function observed(probe, result) {
  await until(`FCM ${result}`, () => device(['logcat', '-d', '-s', 'LingxiPush:D', '*:S']).includes(`verification=${probe.id} result=${result}`));
}
function hasNotification(probe) { return status()?.notifications.some(value => value.title === probe.title); }
function systemNotificationPosted(probe) {
  const messageId = probe.receipt.split('/').at(-1);
  let notificationId = 0;
  for (let index = 0; index < messageId.length; index++) notificationId = (31 * notificationId + messageId.charCodeAt(index)) | 0;
  return device(['shell', 'dumpsys', 'notification']).split('\n').some(line =>
    line.includes('NotificationRecord(') && line.includes(`pkg=${packageId} `) &&
    line.includes(` id=${notificationId} `) && line.includes('tag=lingxi_push'));
}
let savedAuthFiles = [], authFilesRestored = false;
function restoreAuthFiles() {
  if (!savedAuthFiles.length || authFilesRestored) return;
  device(['shell', 'am', 'force-stop', packageId]);
  for (const file of savedAuthFiles) {
    device(['shell', 'run-as', packageId, 'rm', '-f', `${file.path}.bak`]);
    device(['shell', 'run-as', packageId, 'tee', file.path], file.content);
    device(['shell', 'run-as', packageId, 'chmod', '600', file.path]);
  }
  device(['shell', 'am', 'start', '-n', `${packageId}/com.lingxi.code.MainActivity`]);
  authFilesRestored = true;
}
let child, childDone = false, expectInstrumentation = false, instrumentationOutput = '', originalUid;
let failure;
try {
  console.log(JSON.stringify({ phase: 'starting', project: projectId }));
  device(['shell', 'am', 'force-stop', packageId]);
  const authNames = device(['exec-out', 'run-as', packageId, 'ls', 'shared_prefs']).split('\n')
    .filter(name => /^com\.google\.firebase\.auth\.api\.(Store|crypto)\.[A-Za-z0-9+_.=-]+\.xml$/.test(name));
  requireThat(authNames.length === 2 && authNames.some(name => name.includes('.Store.')) && authNames.some(name => name.includes('.crypto.')),
    'Expected one encrypted Auth store and its matching crypto preferences');
  savedAuthFiles = authNames.map(name => ({ path: `shared_prefs/${name}`, content: device(['exec-out', 'run-as', packageId, 'cat', `shared_prefs/${name}`]) }));
  await writeFile(`${reportPath}.auth-cache-backup.json`, JSON.stringify(savedAuthFiles), { mode: 0o600 });
  expectInstrumentation = true;
  child = spawn(adb, ['-s', serial, 'shell', 'am', 'instrument', '-w', '-r',
    '-e', 'class', 'com.lingxi.code.push.PushAccountDeviceTest#accountSwitchAndOfflineLogout',
    '-e', 'livePushProject', projectId, '-e', 'livePushSession', session,
    `${packageId}.test/androidx.test.runner.AndroidJUnitRunner`], { stdio: ['ignore', 'pipe', 'pipe'] });
  child.stdout.on('data', data => { instrumentationOutput = (instrumentationOutput + data.toString()).slice(-64_000); });
  child.stderr.on('data', () => {});
  child.on('exit', () => { childDone = true; });
  const initial = await until('initial signed-in user', () => status()?.phase === 'account_a' && status());
  originalUid = initial.originalUid;
  const a = await binding(initial.originalUid, 'account_a');
  report.installation = installationId.slice(0, 8);
  report.originalUser = initial.originalUid.slice(0, 8);
  command('switch');
  const bStatus = await until('temporary account sign-in', () => status()?.phase === 'account_b' && status());
  const b = await binding(bStatus.temporaryUid, 'account_b');
  requireThat(b.bound.registrationHash === a.bound.registrationHash, 'FCM identity unexpectedly changed during account switch');
  requireThat(b.bound.revision > a.bound.revision && b.bound.uid !== a.bound.uid, 'Cloud account binding did not change');
  report.checks.accountSwitch = true;
  console.log(JSON.stringify({ phase: 'account_b_bound', sameInstallation: true, sameFcmRegistration: true }));

  const oldA = await probe(b.bound, initial.originalUid, 'old-A');
  await observed(oldA, 'rejected');
  requireThat(!hasNotification(oldA), 'A notification for the old account was displayed');
  const currentB = await probe(b.bound, bStatus.temporaryUid, 'current-B');
  await observed(currentB, 'accepted');
  await until('new account system notification', () => hasNotification(currentB));
  report.checks.oldAccountMessageRejected = true;
  report.checks.newAccountMessageDisplayed = true;
  console.log(JSON.stringify({ phase: 'account_message_isolation_passed' }));

  offline();
  await until('device offline', () => status()?.backendReachable === false);
  command('logout_offline');
  const loggedOut = await until('offline sign-out', () => status()?.phase === 'offline_logout' && status());
  requireThat(loggedOut.uid === null && loggedOut.mayBeBound && loggedOut.revision > b.bound.revision, 'Offline logout was not durably queued');
  requireThat(loggedOut.notifications.length === 0, 'Sign-out did not clear displayed push notifications');
  const stillBound = await document(`pushInstallations/${installationId}`, deviceFields);
  requireThat(stillBound.uid === bStatus.temporaryUid && stillBound.revision === b.bound.revision, 'Offline phase unexpectedly reached the backend');
  report.checks.offlineLogoutPersisted = true;
  report.checks.signOutClearedDisplayedPush = true;
  const queuedB = await probe(b.bound, bStatus.temporaryUid, 'queued-B');
  await pause(2_000);
  requireThat(!device(['logcat', '-d', '-s', 'LingxiPush:D', '*:S']).includes(`verification=${queuedB.id}`), 'Queued message arrived before network restoration');
  console.log(JSON.stringify({ phase: 'offline_logout_queued', signedOut: true, pendingRevocation: true }));
  restoreNetwork();
  await until('automatic cloud revocation', async () => {
    const value = status(), revoked = await document(`pushInstallations/${installationId}`, deviceFields);
    return value?.backendReachable && value.uid === null && !value.mayBeBound &&
      revoked?.uid === null && revoked.revision >= loggedOut.revision && !revoked.fcmInstallationId && !revoked.registrationHash;
  });
  requireThat(await document(`pushRegistrationClaims/${b.bound.registrationHash}`, ['installationId']) === null, 'Revoked registration claim still exists');
  await observed(queuedB, 'rejected');
  requireThat(!hasNotification(queuedB), 'A queued message was displayed after offline sign-out');
  report.checks.networkRecoveryRevokedBinding = true;
  report.checks.revocationRemovedFcmClaim = true;
  report.checks.queuedMessageRejectedAfterReconnect = true;
  console.log(JSON.stringify({ phase: 'network_recovery_revocation_passed' }));

  command('cleanup');
  await until('temporary account deletion', () => status()?.phase === 'logged_out' && status()?.temporaryDeleted);
  command('finish');
  await until('instrumentation completion', () => childDone, 30_000);
  expectInstrumentation = false;
  requireThat(/OK \(1 test\)/.test(instrumentationOutput), 'Device instrumentation did not pass');
  requireThat(status()?.uid === null && status()?.phase === 'complete', 'Device did not remain signed out through cleanup');
  report.instrumentationPassed = true;
  restoreAuthFiles();
  const restored = await until('original account after process restart', async () => {
    const value = await document(`pushInstallations/${installationId}`, deviceFields);
    if (value?.uid !== originalUid || value.revision <= loggedOut.revision || !value.notificationsEnabled) return null;
    const claim = await document(`pushRegistrationClaims/${value.registrationHash}`, ['installationId']);
    return claim?.installationId === installationId && value;
  });
  const finalA = await probe(restored, originalUid, 'restored-A');
  await observed(finalA, 'accepted');
  await until('restored account system notification', () => systemNotificationPosted(finalA));
  report.checks.originalAccountRestored = true;
  report.checks.restoredAccountMessageDisplayed = true;
  report.checks.temporaryAccountDeleted = true;
} catch (error) {
  failure = error;
  report.failure = error instanceof VerificationFailure ? error.message : (error.code ?? error.name ?? 'Unknown verification failure');
} finally {
  restoreNetwork();
  expectInstrumentation = false;
  if (child && !childDone) {
    try { command('abort'); } catch {}
    await until('safe account restoration', () => childDone, 110_000).catch(() => {});
  }
  const last = status();
  restoreAuthFiles();
  report.finalAccountRestored = !!originalUid && !!installationId && !!(await until('final restored binding', async () =>
    (await document(`pushInstallations/${installationId}`, ['uid']))?.uid === originalUid, 45_000).catch(() => false));
  report.finalPhase = last?.phase ?? null;
  report.temporaryAccountDeleted = last?.temporaryDeleted ?? false;
  if (!report.temporaryAccountDeleted && last?.temporaryUid && last.temporaryUid !== last.originalUid) {
    try {
      const user = (await authCall('lookup', { localId: [last.temporaryUid] })).users?.[0];
      // Fallback cleanup is limited to the exact user created by this invocation.
      if (user) {
        requireThat(user.localId === last.temporaryUid && /^fcm-verify-[a-f0-9-]+@example\.com$/.test(user.email ?? '') &&
          Number(user.createdAt) >= Date.parse(report.startedAt) - 2_000,
        'Refusing to delete an account outside this verification');
        await authCall('delete', { localId: user.localId });
      }
      report.temporaryAccountDeleted = true;
    } catch (error) {
      if (error.code === 'auth/user-not-found') report.temporaryAccountDeleted = true;
      else report.cleanupFailure = error instanceof VerificationFailure ? error.message : (error.code ?? error.name);
    }
  }
  report.networkSettingsRestored = Object.entries(originalNetwork).every(([key, value]) => device(['shell', 'settings', 'get', 'global', key]) === value);
  report.encryptedAuthCacheRestored = authFilesRestored;
  report.finishedAt = new Date().toISOString();
  await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, { mode: 0o600 });
  if (!failure && report.finalAccountRestored && report.temporaryAccountDeleted && report.networkSettingsRestored && !report.cleanupFailure) {
    device(['shell', 'run-as', packageId, 'rm', '-rf', directory]);
  }
  await deleteApp(app);
}
console.log(JSON.stringify(report));
if (failure || !report.finalAccountRestored || !report.temporaryAccountDeleted || !report.networkSettingsRestored || report.cleanupFailure) process.exitCode = 1;
