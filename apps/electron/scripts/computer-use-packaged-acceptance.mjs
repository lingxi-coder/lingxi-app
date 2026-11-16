#!/usr/bin/env node
/** Opt-in signed Desktop -> real macOS Computer Use acceptance.
 * node scripts/computer-use-packaged-acceptance.mjs --app /path/LingXi\ Code.app --root /tmp/new-run --fixture-root /path/to/new-workspace-fixtures [--tcc-roundtrip] [--tcc-screen-roundtrip] [--swift-module-cache <directory>] [--explicit-computer-rule]
 * Never grants/resets TCC. At a waiting phase, inspect status.json and write
 * control.json {"action":"retry"} after the user changes the indicated permission.
 * All provider credentials, application data and input effects belong to this test.
 */
import assert from 'node:assert/strict';
import { spawn, spawnSync, execFileSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { copyFileSync, existsSync, mkdirSync, readFileSync, realpathSync, symlinkSync, writeFileSync, unlinkSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { pathToFileURL } from 'node:url';
import { createPackagedSettings, createPackagedConversationSpaces, runtimePathsForHome, sanitizePackagedAppEnvironment, assertTemporaryBrokerLaunchdIdentity } from './packaged-app-smoke.mjs';
import { BUNDLE_ID } from './package-support.mjs';
import { nativeComputerCall, decodeComputerProtocolResult, grantedApplicationIds, redactComputerProtocolResult,
  isOwnedApplicationCommand, applicationProcessesForProfile, assertIsolatedProfileAvailable,
  assertOwnedDebugPort, computerAcceptanceDiscoveryQuery, axElementIndex,
  assertBackgroundSurfaceUnchanged, assertBrokerServiceAbsent, assertOwnedBrokerRemoval, assertDisabledComputerToolResult, selectAcceptanceAction, inspectedProcessCommand,
  createAcceptanceSteps, nextAcceptanceStep, requireAcceptancePreludes } from './computer-use-protocol-acceptance-support.mjs';

const bridgeBundleId = `${BUNDLE_ID}.bridge-server.development`;
const appBundleId = `${BUNDLE_ID}.development`;
const escapeRegex = value => value.replace(/[.*+?^\${}()|[\]\\]/g, '\\$&');

const options = { app: '', root: '', fixtureRoot: '', swiftModuleCache: '', tccRoundtrip: false, tccScreenRoundtrip: false, explicitComputerRule: false };
for (let i = 2; i < process.argv.length; i++) {
  if (process.argv[i] === '--tcc-roundtrip') options.tccRoundtrip = true;
  else if (process.argv[i] === '--tcc-screen-roundtrip') options.tccScreenRoundtrip = true;
  else if (process.argv[i] === '--explicit-computer-rule') options.explicitComputerRule = true;
  else if (process.argv[i] === '--fixture-root') options.fixtureRoot = resolve(process.argv[++i]);
  else if (process.argv[i] === '--swift-module-cache') options.swiftModuleCache = resolve(process.argv[++i]);
  else if (process.argv[i] === '--app' || process.argv[i] === '--root') options[process.argv[i].slice(2)] = resolve(process.argv[++i]);
  else throw new Error('Use --app <signed app> --root <new private run directory> --fixture-root <new non-temporary workspace directory> [--tcc-roundtrip] [--tcc-screen-roundtrip] [--swift-module-cache <directory>] [--explicit-computer-rule]');
}
assert.ok(options.app && options.root && options.fixtureRoot, 'Specify --app, --root, and a fresh --fixture-root outside temporary directories for LaunchServices registration.');
assert.ok(!existsSync(options.fixtureRoot), 'Fixture install root must be a new directory owned by this run.');
options.app = realpathSync.native(options.app);
assert.equal(process.platform, 'darwin');
assert.ok(!existsSync(options.root), 'Use a fresh run root; existing test/user data is never overwritten');
mkdirSync(options.root, { recursive: true, mode: 0o700 });
const root = options.root;
const receipt = { status: 'running', startedAtUtc: new Date().toISOString(), phases: [], checks: [], tcc: [], syntheticProvider: true, realNativeBackend: true };
function save() { writeFileSync(join(root, 'receipt.json'), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 }); }
function phase(name, details = {}) {
  const value = { phase: name, at: new Date().toISOString(), ...details };
  receipt.phases.push(value); writeFileSync(join(root, 'status.json'), JSON.stringify(value, null, 2) + '\n', { mode: 0o600 }); save();
  process.stdout.write(JSON.stringify(value) + '\n');
}
function check(name, detail = {}) { receipt.checks.push({ name, passed: true, ...detail }); save(); }
const delay = ms => new Promise(resolvePromise => setTimeout(resolvePromise, ms));
async function waitFor(fn, label, timeout = 60_000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { const value = await fn(); if (value) return value; await delay(100); }
  throw new Error(`Timed out: ${label}`);
}
async function userContinue(name, detail) {
  if (name.includes('revocation')) await verifyResponsibleApplication();
  phase(name, { ...detail, control: 'After completing the indicated system UI action, write control.json with {"action":"retry"}. Use {"action":"abort"} to stop.' });
  await waitFor(() => {
    const path = join(root, 'control.json'); if (!existsSync(path)) return false;
    const command = JSON.parse(readFileSync(path, 'utf8')); unlinkSync(path);
    if (command.action === 'abort') throw new Error('Acceptance aborted by operator');
    return command.action === 'retry';
  }, 'operator continuation', 30 * 60_000);
}
class Cdp {
  id = 0; pending = new Map();
  constructor(url) {
    this.ws = new WebSocket(url);
    this.ready = new Promise((resolvePromise, rejectPromise) => { this.ws.addEventListener('open', resolvePromise); this.ws.addEventListener('error', rejectPromise); });
    this.ws.addEventListener('message', event => { const msg = JSON.parse(String(event.data)); const p = this.pending.get(msg.id); if (p) { this.pending.delete(msg.id); clearTimeout(p.timer); msg.error ? p.reject(new Error(msg.error.message)) : p.resolve(msg.result); } });
    this.ws.addEventListener('close', () => { for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(new Error('Packaged renderer disconnected')); } this.pending.clear(); });
  }
  async send(method, params = {}, timeout = 280_000) {
    await this.ready; const id = ++this.id;
    return new Promise((resolvePromise, rejectPromise) => { const timer = setTimeout(() => { this.pending.delete(id); rejectPromise(new Error(`CDP timeout ${method}`)); }, timeout); this.pending.set(id, { resolve: resolvePromise, reject: rejectPromise, timer }); this.ws.send(JSON.stringify({ id, method, params })); });
  }
  close() { this.ws.close(); }
}
let page, appChild, fixtureChild, shadowChild, provider, providerId, providerAccountId;
let appProcessIdentity, launchedUserDataDirectory, appLaunchConfiguration;
let syntheticCredentialStored=false;
let brokerPreflightConfirmed = false, brokerLaunchAttempted = false, brokerSourceSha256;
const brokerLabel = `${BUNDLE_ID}.credential-broker.development`;
const brokerTarget = `gui/${process.getuid()}/${brokerLabel}`;
let appGeneration=0, attributionStarted;
const appExecutable = join(options.app, 'Contents/MacOS/LingXi Code');
const bridgeExecutable = join(options.app, 'Contents/Resources/bin/bridge-server');
attributionStarted = new Date(receipt.startedAtUtc);
receipt.launchMethod = 'LaunchServices open -n -W -a exact signed bundle';
receipt.responsibleApplicationVerified = false;
let fixtureRegistered = false, shadowRegistered = false;
async function evaluate(expression) {
  const result = await page.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  return result.result?.value;
}
async function invoke(name, ...args) { return evaluate(`window.lingxi.${name}(...${JSON.stringify(args)})`); }
async function settings(command) { return evaluate(`window.lingxi.computerSettings.execute(${JSON.stringify(command)})`); }
async function mutate(action, patch) { const current = await settings({ action: 'get' }); const result = await settings({ action, revision: current.revision, ...patch }); protocolFrame = null; return result; }

// /usr/bin/open is only a waiting launcher, not the application process.
// Identity always includes the exact signed executable and unique test profile.
function canonicalArgumentPaths(path) { return [...new Set([path, realpathSync.native(path)])]; }
function commandMatchesOwnedApp(command) {
  return Boolean(launchedUserDataDirectory) && isOwnedApplicationCommand(command, appExecutable, canonicalArgumentPaths(launchedUserDataDirectory));
}
function inspectBrokerService() {
  return spawnSync('/bin/launchctl', ['print', brokerTarget], { encoding: 'utf8', timeout: 10000 });
}
function requireAbsentBrokerService() {
  assertBrokerServiceAbsent(inspectBrokerService(), brokerTarget);
}
function canonicalPathInPrivateHome(homePath, path, label) {
  const canonical = realpathSync.native(path);
  const local = relative(homePath, canonical);
  assert.ok(local !== '' && local !== '..' && !local.startsWith(`..${sep}`) && !isAbsolute(local),
    `${label} must resolve inside the private acceptance home`);
  return canonical;
}
function ownedBrokerState() {
  const service = inspectBrokerService();
  if (service.status === 113 && !service.error) return null;
  assert.equal(service.status, 0, 'Cannot inspect the current Broker service safely');
  const homePath = realpathSync.native(join(root, 'home'));
  const plistPath = canonicalPathInPrivateHome(homePath, join(homePath, 'Library/LaunchAgents', `${brokerLabel}.plist`), 'Broker LaunchAgent');
  const programPath = canonicalPathInPrivateHome(homePath, join(homePath, 'Library/Application Support/LingXi/CredentialBroker/development/LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker'), 'Broker executable');
  const paths = { homePath, label: brokerLabel, target: brokerTarget, plistPath, programPath };
  const identity = assertTemporaryBrokerLaunchdIdentity(service.stdout, paths);
  const owned = { ...paths, ...identity };
  assertOwnedBrokerRemoval(paths, owned);
  const plist = JSON.parse(execFileSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', plistPath], { encoding: 'utf8' }));
  assert.equal(plist.Label, brokerLabel);
  assert.ok(Array.isArray(plist.ProgramArguments) && plist.ProgramArguments.length === 1);
  assert.equal(realpathSync.native(plist.ProgramArguments[0]), programPath);
  assert.equal(plist.MachServices?.[brokerLabel], true);
  assert.equal(createHash('sha256').update(readFileSync(programPath)).digest('hex'), brokerSourceSha256,
    'Installed private Broker must match the signed candidate Broker bytes');
  if (owned.pid !== undefined) {
    const images = execFileSync('/usr/sbin/lsof', ['-a', '-p', String(owned.pid), '-d', 'txt', '-Fn'], { encoding: 'utf8' })
      .split('\n').filter(line => line.startsWith('n/')).map(line => line.slice(1));
    assert.ok(images.some(path => { try { return realpathSync.native(path) === programPath; } catch { return false; } }),
      'Broker PID must load the private test-owned executable');
  }
  return owned;
}
async function bootoutOwnedBroker() {
  // Preflight failure must never touch the real registered service. Even after
  // launch, a service label alone is insufficient evidence for bootout.
  if (!brokerPreflightConfirmed || !brokerLaunchAttempted) return false;
  assert.deepEqual(ownedAppProcesses(), [], 'The private app must exit before its Broker can be removed');
  const owned = ownedBrokerState();
  if (!owned) return false;
  const current = ownedBrokerState();
  assert.ok(current, 'Private Broker disappeared before guarded teardown');
  assertOwnedBrokerRemoval(owned, current);
  const stopped = spawnSync('/bin/launchctl', ['bootout', current.target], { encoding: 'utf8', timeout: 10000 });
  assert.ok([0, 3].includes(stopped.status), 'Guarded private Broker bootout failed');
  await waitFor(() => {
    const state = inspectBrokerService();
    if (state.status === 113 && !state.error) return true;
    assert.equal(state.status, 0, 'Cannot confirm private Broker removal');
    const remaining = ownedBrokerState();
    if (!remaining) return true; // Service can exit between the two inspections.
    assertOwnedBrokerRemoval(current, remaining);
    return false;
  }, 'test-owned Broker service removal', 10000);
  receipt.brokerTeardowns ??= [];
  receipt.brokerTeardowns.push({ target: current.target, pid: current.pid ?? null, privateHomeVerified: true });
  return true;
}
function ownedAppProcesses() {
  const rows = execFileSync('/bin/ps', ['-ax', '-o', 'pid=,command='], { encoding: 'utf8' });
  return launchedUserDataDirectory ? applicationProcessesForProfile(rows, appExecutable, canonicalArgumentPaths(launchedUserDataDirectory)) : [];
}
function ownedAppPidStillMatches() {
  if (!appProcessIdentity) return false;
  const inspect = () => inspectedProcessCommand(spawnSync('/bin/ps',['-p',String(appProcessIdentity.pid),'-o','command='],{encoding:'utf8'}));
  const command = inspect();
  if (command === null) return false;
  if (!commandMatchesOwnedApp(command)) {
    // A closing app can disappear while ps is formatting the row. Confirm
    // absence before treating a mismatched row as an unrelated live process.
    if (inspect() === null) return false;
    throw new Error('Application PID identity changed; refusing to signal an unrelated process');
  }
  return true;
}
function localLogTime(date) {
  const two=value=>String(value).padStart(2,'0');
  return `${date.getFullYear()}-${two(date.getMonth()+1)}-${two(date.getDate())} ${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
}
function currentOwnedBridgePids() {
  const rows=execFileSync('/bin/ps',['-ax','-o','pid=,command='],{encoding:'utf8'});
  const ownedRoots=canonicalArgumentPaths(root);
  return rows.split('\n').flatMap(line=>{
    const match=line.trim().match(/^(\d+)\s+(.*)$/);if(!match || !match[2].startsWith(`${bridgeExecutable} `) || !ownedRoots.some(path=>match[2].includes(`${path}/`)))return [];
    return [Number(match[1])];
  });
}
function parseOwnedTccAttribution(row, ownedBridgePids) {
  const message=row.eventMessage ?? '';
  const responsible=message.match(/responsible=\{TCCDProcess: identifier=([^,]+), pid=(\d+),[^}]*responsible_path=([^,}]+)/);
  const accessing=message.match(new RegExp(String.raw`accessing=\{TCCDProcess: identifier=${escapeRegex(bridgeBundleId)}, pid=(\d+),[^}]*binary_path=([^}]+)`));
  const requesting=message.match(new RegExp(String.raw`requesting=\{TCCDProcess: identifier=${escapeRegex(bridgeBundleId)}, pid=(\d+),[^}]*binary_path=([^}]+)`));
  const bridge=accessing ?? requesting, attributionRole=accessing ? 'accessing':'requesting';
  if(!responsible || !bridge || !ownedBridgePids.includes(Number(bridge[1])) || bridge[2].trim()!==bridgeExecutable)return null;
  const pathMatches=responsible[3].trim()===appExecutable;
  return {timestamp:row.timestamp ?? null,attributionRole,responsibleBundleId:responsible[1],responsiblePid:Number(responsible[2]),responsiblePathMatchesSignedApp:pathMatches,responsibleApplicationMatches:responsible[1]==='com.lingxi.code.development' && Number(responsible[2])===appProcessIdentity?.pid && pathMatches,bridgeBundleId:'com.lingxi.code.bridge-server.development',bridgePid:Number(bridge[1]),bridgePath:'<signed-app>/Contents/Resources/bin/bridge-server',eventSha256:createHash('sha256').update(message).digest('hex')};
}
async function verifyResponsibleApplication() {
  const ownedBridgePids=currentOwnedBridgePids();
  assert.ok(ownedBridgePids.length>0,"Cannot identify this run's exact packaged bridge process for TCC attribution");
  const predicate='process == "tccd" AND eventMessage CONTAINS "AUTHREQ_ATTRIBUTION" AND eventMessage CONTAINS "com.lingxi.code.bridge-server.development"';
  const evidence=[];
  // A short retry accommodates unified-log publication lag without creating
  // new permission requests or changing any system setting.
  for(let attempt=0;attempt<4;attempt++){
    const result=spawnSync('/usr/bin/log',['show','--start',localLogTime(attributionStarted),'--end',localLogTime(new Date()),'--style','ndjson','--predicate',predicate],{encoding:'utf8',timeout:15000,maxBuffer:4*1024*1024});
    if(result.status!==0){receipt.responsibilityObservationError='TCC attribution logs are unavailable; run this explicit acceptance with permission to read the bounded system log.';save();throw new Error(receipt.responsibilityObservationError)}
    for(const line of result.stdout.split('\n')){
      let row;try{row=JSON.parse(line)}catch{continue}
      const record=parseOwnedTccAttribution(row,ownedBridgePids);
      if(!record)continue;
      if(!evidence.some(prior=>JSON.stringify(prior)===JSON.stringify(record)))evidence.push(record);
    }
    const verified=evidence.some(record=>record.responsibleApplicationMatches===true);
    receipt.tccResponsibility=evidence;receipt.responsibleApplicationVerified=verified;save();
    if(verified){
      receipt.tccResponsibilityHistory ??= [];
      if(!receipt.tccResponsibilityHistory.some(entry=>entry.applicationGeneration===appGeneration))receipt.tccResponsibilityHistory.push({applicationGeneration:appGeneration,applicationPid:appProcessIdentity.pid,attributionStartedAtUtc:attributionStarted.toISOString(),evidence});
      check('tcc-responsibility-is-the-signed-application',{responsibleBundleId:'com.lingxi.code.development',bridgeBundleId:'com.lingxi.code.bridge-server.development',attributionRoles:[...new Set(evidence.filter(record=>record.responsibleApplicationMatches).map(record=>record.attributionRole))]});return}
    await delay(500);
  }
  phase('tcc-responsibility-unverified',{attributions:evidence,instruction:'OS revocation cannot be accepted until the current bridge is attributed to the signed LingXi development application.'});
  throw new Error('The current bridge TCC responsibility is not verified as the signed LingXi application; refusing an ambiguous OS revocation test');
}
async function terminateOwnedApp() {
  if(!launchedUserDataDirectory){receipt.realAppProcessExited=true;return}
  // Resolve even a failed pre-debugger startup so no LaunchServices child leaks.
  let matches=ownedAppProcesses();
  assert.ok(matches.length<=1,'Multiple processes match the unique acceptance profile; refusing ambiguous cleanup');
  if(!appProcessIdentity && matches.length===1)appProcessIdentity=matches[0];
  if(appProcessIdentity && ownedAppPidStillMatches()){try{await waitFor(()=>!ownedAppPidStillMatches(),'graceful signed application close',3000)}catch{/* Revalidate exact PID identity before each fallback signal below. */}}
  if(appProcessIdentity){
    for(const signal of ['SIGTERM','SIGKILL']){
      if(!ownedAppPidStillMatches())break;
      process.kill(appProcessIdentity.pid,signal);
      try{await waitFor(()=>!ownedAppPidStillMatches(),`owned signed application exit after ${signal}`,3000)}catch{if(signal==='SIGKILL')throw new Error('Owned signed application survived exact-identity teardown')}
    }
  }
  matches=ownedAppProcesses();assert.deepEqual(matches,[],'An application with this exact unique test profile survived teardown');
  receipt.realAppProcessExited=true;
  if(appChild){
    try{await waitFor(()=>appChild.exitCode!==null || appChild.signalCode!==null,'LaunchServices waiting launcher exit',5000)}catch{
      // It is safe to stop only our waiting open process after the real app is gone.
      appChild.kill('SIGTERM');await waitFor(()=>appChild.exitCode!==null || appChild.signalCode!==null,'owned open waiter exit',3000);
    }
    receipt.launcherExited=true;
  }
  assert.deepEqual(ownedAppProcesses(),[],'A delayed LaunchServices launch survived cleanup');
}

async function launchOwnedPackagedApp() {
  assert.ok(appLaunchConfiguration,'A private launch configuration is required');
  assert.ok(brokerPreflightConfirmed, 'Broker preflight must complete before application launch');
  requireAbsentBrokerService();
  assert.deepEqual(ownedAppProcesses(),[],'The previous private-profile application must be gone before relaunch');
  appGeneration += 1;appProcessIdentity=undefined;page=undefined;
  attributionStarted=new Date();receipt.responsibleApplicationVerified=false;receipt.tccResponsibility=[];
  receipt.realAppProcessExited=false;receipt.launcherExited=false;
  const {env,launchEnvironment}=appLaunchConfiguration;
  const portServer=createServer();await new Promise(resolvePromise=>portServer.listen(0,'127.0.0.1',resolvePromise));const port=portServer.address().port;await new Promise(resolvePromise=>portServer.close(resolvePromise));
  const stdoutPath=join(root,`app-${appGeneration}.stdout.log`),stderrPath=join(root,`app-${appGeneration}.stderr.log`);
  writeFileSync(stdoutPath,'',{mode:0o600});writeFileSync(stderrPath,'',{mode:0o600});
  launchedUserDataDirectory=appLaunchConfiguration.paths.userDataDir;
  brokerLaunchAttempted = true;
  appChild=spawn('/usr/bin/open',['-n','-W','-a',options.app,'-i','/dev/null','-o',stdoutPath,'--stderr',stderrPath,...Object.entries(launchEnvironment).flatMap(([name,value])=>['--env',`${name}=${value}`]),'--args',`--user-data-dir=${appLaunchConfiguration.paths.userDataDir}`,`--remote-debugging-port=${port}`],{cwd:dirname(options.app),env,stdio:['ignore','pipe','pipe']});
  const launcherLog=join(root,`open-launcher-${appGeneration}.log`);
  for(const stream of [appChild.stdout,appChild.stderr])stream.on('data',chunk=>writeFileSync(launcherLog,chunk,{flag:'a',mode:0o600}));
  appProcessIdentity=await waitFor(()=>{const owned=ownedAppProcesses();assert.ok(owned.length<=1,'Expected exactly one signed app for the unique test profile');return owned[0]},'LaunchServices signed application PID');
  receipt.applicationPid=appProcessIdentity.pid;save();
  await waitFor(() => {
    assert.ok(ownedAppPidStillMatches(), 'The latched signed application identity must remain current');
    const listeners = spawnSync('/usr/sbin/lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN', '-Fp'], { encoding: 'utf8', timeout: 5000 });
    if (listeners.status === 1 && !listeners.stdout.trim() && !listeners.stderr.trim()) return false;
    assert.equal(listeners.status, 0, 'Cannot establish debugger-port ownership');
    assert.equal(listeners.stderr.trim(), '', 'Debugger-port inspection was ambiguous');
    const pids = listeners.stdout.split('\n').filter(line => /^p\d+$/.test(line)).map(line => Number(line.slice(1)));
    assertOwnedDebugPort(pids, appProcessIdentity.pid);
    return true;
  }, 'debugger listener belongs exclusively to the latched signed test application');
  const rendererUrl = pathToFileURL(join(options.app, 'Contents/Resources/app.asar/out/renderer/index.html')).href;
  const target = await waitFor(async () => { try { const response = await fetch(`http://127.0.0.1:${port}/json/list`); const targets = await response.json(); return targets.find(target => target.type === 'page' && target.url === rendererUrl); } catch { return false; } }, 'signed renderer target');
  page = new Cdp(target.webSocketDebuggerUrl); await page.send('Page.enable');
  await waitFor(async () => (await page.send('Page.getFrameTree')).frameTree.frame.url === rendererUrl, 'committed signed renderer');
  await waitFor(() => evaluate('Boolean(Reflect.get(window, "lingxi")?.computerSettings)'), 'real preload');
  receipt.applicationLaunches ??= [];receipt.applicationLaunches.push({applicationGeneration:appGeneration,pid:appProcessIdentity.pid,launchedAtUtc:attributionStarted.toISOString(),privateProfileReused:appGeneration>1});save();
  return port;
}
async function restartOwnedPackagedApp(reason) {
  const previousPid=appProcessIdentity?.pid,previousBridgePids=currentOwnedBridgePids();
  assert.ok(previousPid && previousBridgePids.length>0,'Restart requires known owned application and bridge processes');
  if(page){try{if(ownedAppPidStillMatches())await page.send('Browser.close',{},3000)}catch{}page.close();page=undefined}
  await terminateOwnedApp();
  await waitFor(()=>currentOwnedBridgePids().length===0,'previous owned sidecars fully stopped',10000);
  await bootoutOwnedBroker();
  const port=await launchOwnedPackagedApp();
  const bootstrap=await invoke('bootstrap');
  assert.equal(realpathSync.native(bootstrap.workspace.path),realpathSync.native(appLaunchConfiguration.workspace),'Restart must retain the private test workspace');
  const opened=await invoke('openSession',bootstrap.workspace.path,sessionId);
  assert.equal(opened.activeSession?.sessionId,sessionId,'Restart must reopen the same acceptance conversation');
  await configureAcceptanceModel();
  const newBridgePids=await waitFor(()=>{const pids=currentOwnedBridgePids();return pids.length && pids},'restarted owned sidecar PID');
  assert.notEqual(appProcessIdentity.pid,previousPid,'Screen Recording changes must be tested in a new application process');
  assert.ok(newBridgePids.every(pid=>!previousBridgePids.includes(pid)),'Old sidecars cannot carry OS grants or frames across restart');
  const policy=await settings({action:'get'});assert.equal(policy.enabled,true);assert.ok(!policy.apps.some(app=>app.bundleId===fixtureId && app.permission==='allow'),'Restart acceptance must not inherit a permanent fixture grant');
  const record={reason,previousApplicationPid:previousPid,applicationPid:appProcessIdentity.pid,previousBridgePids,newBridgePids,cdpPort:port,sessionId};
  receipt.ownedApplicationRestarts ??= [];receipt.ownedApplicationRestarts.push(record);phase('owned-application-restarted',record);
}

async function configureAcceptanceModel() {
  await observeSession();
  await invoke('command',sessionId,{type:'list_models'});
  const modelReference=`${providerId}/acceptance-model`;
  await waitFor(async () => {
    const events = await evaluate('window.__computerAcceptance.events');
    return events.some(event =>
      event.type === 'model_list'
        ? event.models.includes(modelReference) || event.details?.some(model => model.reference === modelReference)
        : event.type === 'provider_model_catalog' && event.providers.some(provider =>
          provider.provider_id === providerId && provider.models.some(model => model.reference === modelReference)));
  }, 'booted runtime catalog includes the exact synthetic provider/model');
  await invoke('command', sessionId, { type: 'set_model', model: modelReference });
  await waitFor(async () => {
    const events = await evaluate('window.__computerAcceptance.events');
    return events.some(event => event.type === 'model_changed' && (event.model === modelReference || event.model === 'acceptance-model'));
  }, 'synthetic model switch confirmation');
  // Exercise the dedicated Computer Access resolver in ordinary manual mode.
  // The synthetic model does not implement the unrelated Auto-mode classifier.
  await invoke('command', sessionId, { type: 'set_permission_mode', mode: 'default' });
  await waitFor(async () => {
    const events = await evaluate('window.__computerAcceptance.events');
    return events.some(event => event.type === 'permission_mode_changed' && event.mode === 'default');
  }, 'manual permission mode confirmation');
  receipt.permissionMode = 'default';
}

// An actual NSApplication target: all generated input terminates here. Its
// JSON receipt is independent of tool results and proves visible side effects.
const fixtureBundle = join(options.fixtureRoot, 'Permission Target.app');
const fixtureExecutable = join(fixtureBundle, 'Contents/MacOS/Permission Target');
const fixtureRunId = randomUUID();
const fixtureId = `org.lingxi.permission-target.${fixtureRunId}`;
const fixtureStatePath = join(root, 'fixture.json');
const fixtureControlPath = join(root, 'fixture-control.json');
const shadowBundle = join(options.fixtureRoot, 'Unauthorized Target.app');
const shadowExecutable = join(shadowBundle, 'Contents/MacOS/Unauthorized Target');
const shadowId = `org.lingxi.permission-shadow.${fixtureRunId}`;
const shadowStatePath = join(root, 'shadow.json');
const shadowControlPath = join(root, 'shadow-control.json');
const fixtureEnvironment = { ...process.env, HOME: join(root, 'home'), CFFIXED_USER_HOME: join(root, 'home'),
  __CFPREFERENCES_AVOID_DAEMON: '1', TMPDIR: join(root, 'tmp') };
const launchServicesRegister = '/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister';
function fixturePlist(id, name) {
  return `<?xml version="1.0"?><!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd"><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>${id}</string><key>CFBundleName</key><string>${name}</string><key>CFBundleDisplayName</key><string>${name}</string><key>CFBundleExecutable</key><string>${name}</string><key>CFBundlePackageType</key><string>APPL</string><key>CFBundleVersion</key><string>1</string><key>CFBundleShortVersionString</key><string>1.0</string><key>LSMinimumSystemVersion</key><string>14.0</string><key>NSHighResolutionCapable</key><true/><key>NSPrincipalClass</key><string>NSApplication</string></dict></plist>`;
}
function registerFixture(bundle, id) {
  // Bind the executable's identity to this complete application bundle; a
  // linker's ad-hoc Mach-O signature alone does not bind its Info.plist.
  execFileSync('/usr/bin/codesign', ['--force', '--sign', '-', '--identifier', id, '--timestamp=none', bundle], { stdio: 'pipe' });
  execFileSync('/usr/bin/codesign', ['--verify', '--strict', bundle], { stdio: 'pipe' });
  execFileSync(launchServicesRegister, ['-f', bundle], { env: fixtureEnvironment, stdio: 'pipe' });
}

const swiftSource = `import Cocoa
final class Delegate: NSObject, NSApplicationDelegate, NSTextFieldDelegate {
 var window:NSWindow!; var field:NSTextField!; var button:NSButton!; var marker:NSBox!; var clicks=0;var lastYield="";var lastYieldNonce="";var clickHistory:[[String:Any]]=[]
 var foregroundEventCount=0;var activationObserver:NSObjectProtocol?;var lastControlNonce="";var sampleSequence=0
 let state=CommandLine.arguments[1]; let control=CommandLine.arguments[2]
 let unauthorized=CommandLine.arguments.contains("--unauthorized")
 func rect(_ view:NSView)->[Int] {
  let r=window.convertToScreen(view.convert(view.bounds,to:nil));let screen=NSScreen.screens.first!;let scale=screen.backingScaleFactor
  return [Int(r.minX*scale),Int((screen.frame.maxY-r.maxY)*scale),Int(r.width*scale),Int(r.height*scale)]
 }
 func point(_ view:NSView)->[Int] {let r=rect(view);return [r[0]+r[2]/2,r[1]+r[3]/2]}
 func save(){
  sampleSequence += 1
  let screen=NSScreen.screens.first!
  let mouse=NSEvent.mouseLocation
  let d:[String:Any]=["pid":ProcessInfo.processInfo.processIdentifier,"bundleId":Bundle.main.bundleIdentifier ?? "","windowId":window.windowNumber,"frontmostBundleId":NSWorkspace.shared.frontmostApplication?.bundleIdentifier ?? "","foregroundEventCount":foregroundEventCount,"cursor":[mouse.x,mouse.y],"mouseMoveEventCount":CGEventSource.counterForEventType(.combinedSessionState,eventType:.mouseMoved),"lastControlNonce":lastControlNonce,"unauthorized":unauthorized,"clicks":clicks,"clickHistory":clickHistory,"text":field.stringValue,"fieldFocused":field.currentEditor() != nil && window.firstResponder === field.currentEditor(),"button":point(button),"field":point(field),"marker":point(marker),"markerRect":rect(marker),"contentRect":rect(window.contentView!),"screen":["width":Int(screen.frame.width*screen.backingScaleFactor),"height":Int(screen.frame.height*screen.backingScaleFactor),"scale":screen.backingScaleFactor],"lastYield":lastYield,"lastYieldNonce":lastYieldNonce,"visible":window.isVisible && window.occlusionState.contains(.visible),"frontmost":NSWorkspace.shared.frontmostApplication?.bundleIdentifier == Bundle.main.bundleIdentifier]
  var output=d;output["sampleSequence"]=sampleSequence;output["windowSizePhysical"]=[Int(window.frame.width*screen.backingScaleFactor),Int(window.frame.height*screen.backingScaleFactor)]
  if let bytes=try? JSONSerialization.data(withJSONObject:output,options:[.sortedKeys]){try? bytes.write(to:URL(fileURLWithPath:state),options:.atomic)}
 }
 @objc func clicked(){
  clicks += 1
  var record:[String:Any]=["at":ISO8601DateFormatter().string(from:Date()),"epochMilliseconds":Date().timeIntervalSince1970*1000,"clickIndex":clicks]
  if let event=NSApp.currentEvent{
   record["eventType"]=event.type.rawValue;record["eventUptimeSeconds"]=event.timestamp
   let point=window.convertPoint(toScreen:event.locationInWindow);let screen=NSScreen.screens.first!;record["positionPhysical"]=[point.x*screen.backingScaleFactor,(screen.frame.maxY-point.y)*screen.backingScaleFactor]
   if [.leftMouseDown,.leftMouseUp,.rightMouseDown,.rightMouseUp,.otherMouseDown,.otherMouseUp].contains(event.type){record["eventNumber"]=event.eventNumber;record["clickCount"]=event.clickCount}
  }else{record["eventType"]="none"}
  clickHistory.append(record);if clickHistory.count>30{clickHistory.removeFirst(clickHistory.count-30)};save()
 }
 func controlTextDidChange(_ obj:Notification){save()}
 func makeWindow(activate:Bool){
  let screen=NSScreen.screens.first!;let width=min(600,(screen.frame.width-120)/2);let x=unauthorized ? screen.frame.width-40-width:40
  window=NSWindow(contentRect:NSRect(x:x,y:180,width:width,height:260),styleMask:[.titled,.closable],backing:.buffered,defer:false)
  window.isReleasedWhenClosed=false
  window.title=unauthorized ? "Unauthorized Target — Never Granted":"Permission Acceptance — Authorized Target"
  let label=NSTextField(labelWithString:unauthorized ? "This independent app must stay BLACK in authorized captures.":"Disposable input target. No user documents or network.")
  label.frame=NSRect(x:24,y:194,width:width-48,height:34);window.contentView!.addSubview(label)
  field=NSTextField(frame:NSRect(x:24,y:120,width:width-48,height:36));field.delegate=self;field.placeholderString="Only synthetic acceptance input belongs here";window.contentView!.addSubview(field)
  button=NSButton(title:"Record test click",target:self,action:#selector(clicked));button.frame=NSRect(x:24,y:52,width:160,height:40);window.contentView!.addSubview(button)
  marker=NSBox(frame:NSRect(x:width-144,y:52,width:120,height:40));marker.boxType = .custom;marker.borderWidth=0
  marker.fillColor=unauthorized ? NSColor(srgbRed:0.1,green:0.85,blue:0.2,alpha:1):NSColor(srgbRed:0.93,green:0.14,blue:0.67,alpha:1);window.contentView!.addSubview(marker)
  if activate{window.makeKeyAndOrderFront(nil);NSApp.activate()}else{window.orderFront(nil)}
  save()
 }
 func applicationDidFinishLaunching(_ n:Notification){
  makeWindow(activate:true)
  activationObserver=NSWorkspace.shared.notificationCenter.addObserver(forName:NSWorkspace.didActivateApplicationNotification,object:nil,queue:.main){_ in self.foregroundEventCount += 1;self.save()}
  Timer.scheduledTimer(withTimeInterval:0.1,repeats:true){_ in
   if let bytes=try? Data(contentsOf:URL(fileURLWithPath:self.control)),let command=(try? JSONSerialization.jsonObject(with:bytes)) as? [String:Any]{
    try? FileManager.default.removeItem(atPath:self.control)
    self.lastControlNonce=command["nonce"] as? String ?? self.lastControlNonce
    if command["replaceWindow"] as? Bool == true{let text=self.field.stringValue;self.window.close();self.makeWindow(activate:false);self.field.stringValue=text}
    else if command["clearText"] as? Bool == true{self.field.stringValue=""}
    else if let target=command["yieldTo"] as? String,["${fixtureId}","${shadowId}"].contains(target){NSApp.yieldActivation(toApplicationWithBundleIdentifier:target);self.lastYield=target;self.lastYieldNonce=command["nonce"] as? String ?? ""}
    else if command["observe"] as? Bool != true{self.window.makeKeyAndOrderFront(nil);NSApp.activate();self.window.makeFirstResponder(self.field)}
   };self.save()
  }
 }
}
if CommandLine.arguments.count > 2 && CommandLine.arguments[1] == "--inspect" {
 guard let data=try? Data(contentsOf:URL(fileURLWithPath:CommandLine.arguments[2])),let bitmap=NSBitmapImageRep(data:data)else{exit(2)}
 var black=0;var nonblack=0
 for y in stride(from:0,to:bitmap.pixelsHigh,by:3){for x in stride(from:0,to:bitmap.pixelsWide,by:3){if let c=bitmap.colorAt(x:x,y:y)?.usingColorSpace(.sRGB){if c.redComponent < 0.02 && c.greenComponent < 0.02 && c.blueComponent < 0.02{black += 1}else{nonblack += 1}}}}
 var result:[String:Any]=["width":bitmap.pixelsWide,"height":bitmap.pixelsHigh,"blackSamples":black,"nonBlackSamples":nonblack]
 if CommandLine.arguments.count > 4,let x=Int(CommandLine.arguments[3]),let y=Int(CommandLine.arguments[4]),x>=0,y>=0,x<bitmap.pixelsWide,y<bitmap.pixelsHigh,let c=bitmap.colorAt(x:x,y:y)?.usingColorSpace(.sRGB){result["sampleRGB"]=[c.redComponent,c.greenComponent,c.blueComponent]}
 if CommandLine.arguments.count > 7 && CommandLine.arguments[3] == "--region",let x=Int(CommandLine.arguments[4]),let y=Int(CommandLine.arguments[5]),let w=Int(CommandLine.arguments[6]),let h=Int(CommandLine.arguments[7]),x>=0,y>=0,w>0,h>0,x+w<=bitmap.pixelsWide,y+h<=bitmap.pixelsHigh{
  var total=0;var notBlack=0
  for py in y..<y+h{for px in x..<x+w{guard let c=bitmap.colorAt(x:px,y:py)?.usingColorSpace(.sRGB)else{exit(3)};total += 1;if c.redComponent>=0.02 || c.greenComponent>=0.02 || c.blueComponent>=0.02{notBlack += 1}}}
  result["region"]=["rect":[x,y,w,h],"pixels":total,"nonBlackPixels":notBlack,"allBlack":notBlack==0]
 }
 if let out=try? JSONSerialization.data(withJSONObject:result){print(String(data:out,encoding:.utf8)!)};exit(0)
}
let app=NSApplication.shared;let delegate=Delegate();app.setActivationPolicy(.regular);app.delegate=delegate;app.run()
`;
function fixtureState() { return JSON.parse(readFileSync(fixtureStatePath, 'utf8')); }
function shadowState() { return JSON.parse(readFileSync(shadowStatePath, 'utf8')); }
function mappedRegion(rect, image, pixels) {
  const x=Math.ceil((rect[0]+4)*pixels.width/image.width), y=Math.ceil((rect[1]+4)*pixels.height/image.height);
  const right=Math.floor((rect[0]+rect[2]-4)*pixels.width/image.width), bottom=Math.floor((rect[1]+rect[3]-4)*pixels.height/image.height);
  assert.ok(right>x && bottom>y, 'Controlled region must map to valid image pixels'); return [x,y,right-x,bottom-y];
}
function inspectRegion(path, rect, image, pixels) {
  const result=JSON.parse(execFileSync(fixtureExecutable,['--inspect',path,'--region',...mappedRegion(rect,image,pixels).map(String)],{encoding:'utf8'}));
  assert.ok(result.region?.pixels>100,'A real bounded controlled region must be sampled'); return result.region;
}
async function activateOwnedTarget(bundle, statePath, controlPath, id) {
  const child = id === fixtureId ? fixtureChild : shadowChild;
  assert.ok(child && child.exitCode === null && child.signalCode === null, 'Only an already-running owned fixture may be activated');
  assert.equal(JSON.parse(readFileSync(statePath, 'utf8')).pid, child.pid, 'Fixture PID must remain owned; activation must not launch a replacement');
  // macOS 14+ activation is cooperative; ignoringOtherApps is no longer a
  // foreground guarantee. Only the two disposable applications can yield.
  const other=id===fixtureId ? {state:shadowStatePath,control:shadowControlPath}:{state:fixtureStatePath,control:fixtureControlPath};
  if(existsSync(other.state) && JSON.parse(readFileSync(other.state,'utf8')).frontmost){
    const nonce=randomUUID();writeFileSync(other.control,JSON.stringify({yieldTo:id,nonce}));
    await waitFor(()=>{const state=JSON.parse(readFileSync(other.state,'utf8'));return state.lastYield===id && state.lastYieldNonce===nonce},'cooperative activation yield',3000);
  }
  // LaunchServices supplies an actual reopen/activation request even when an
  // unrelated application (for example System Settings) currently owns focus.
  for (let attempt = 1; attempt <= 3; attempt++) {
    execFileSync('/usr/bin/open',['-a',bundle],{env:fixtureEnvironment,stdio:'pipe',timeout:10000});
    const nonce=randomUUID();
    writeFileSync(controlPath,JSON.stringify({nonce})); // ACK must be produced after this exact open.
    let stableSince, lastSequence=-1;
    try {
      await waitFor(()=>{
        const state=JSON.parse(readFileSync(statePath,'utf8'));
        assert.equal(state.pid,child.pid,'The activation target must remain the owned fixture');
        if(state.lastControlNonce!==nonce || !state.frontmost || !state.fieldFocused){stableSince=undefined;return false}
        if(state.sampleSequence===lastSequence)return false;
        lastSequence=state.sampleSequence;
        stableSince ??= Date.now();
        return Date.now()-stableSince>=300;
      },'fresh activation ACK and stable foreground of the owned disposable target',3000);
      return;
    } catch(error) { if(attempt===3)throw error; }
  }
}
async function showControlledPair() {
  await activateOwnedTarget(shadowBundle,shadowStatePath,shadowControlPath,shadowId);
  await activateFixture();await waitFor(()=>shadowState().visible && fixtureState().visible,'both controlled windows visible');
  const a=fixtureState().contentRect,b=shadowState().contentRect;assert.ok(a[0]+a[2]<=b[0] || b[0]+b[2]<=a[0] || a[1]+a[3]<=b[1] || b[1]+b[3]<=a[1],'Controlled windows must not overlap');
}
async function activateFixture() { await activateOwnedTarget(fixtureBundle,fixtureStatePath,fixtureControlPath,fixtureId); }
const key = `synthetic-${randomUUID()}`;
const actions = new Map(); let sessionId;
let protocolFrame = null, protocolFiltering = null;
const toolEmissionCounts = new Map();
const providerRequests = [];
let providerFailure, providerRequestSequence=0;
function effectSnapshot() {
  if(!existsSync(fixtureStatePath))return null;
  const state=fixtureState();return {pid:state.pid,clicks:state.clicks,textLength:state.text.length,textSha256:createHash('sha256').update(state.text).digest('hex'),frontmost:state.frontmost,fieldFocused:state.fieldFocused,clickHistory:state.clickHistory ?? []};
}
function backgroundSurfaceSnapshot() {
  const state = fixtureState();
  return { frontmostBundleId: state.frontmostBundleId, foregroundEventCount: state.foregroundEventCount,
    cursor: state.cursor, mouseMoveEventCount: state.mouseMoveEventCount };
}
async function prepareBackgroundSurface() {
  for(let attempt=1;attempt<=3;attempt++) {
    await activateOwnedTarget(shadowBundle,shadowStatePath,shadowControlPath,shadowId);
    const nonce=randomUUID();writeFileSync(fixtureControlPath,JSON.stringify({observe:true,nonce}));
    let stableSince,previous,sequence=-1;
    try {
      return await waitFor(()=>{
        const state=fixtureState();
        assert.equal(state.pid,fixtureChild.pid,'The passive observer must remain the owned fixture');
        if(state.lastControlNonce!==nonce || state.frontmostBundleId!==shadowId){stableSince=undefined;previous=undefined;return false}
        if(sequence===state.sampleSequence)return false;
        sequence=state.sampleSequence;
        const current={frontmostBundleId:state.frontmostBundleId,foregroundEventCount:state.foregroundEventCount,cursor:state.cursor,mouseMoveEventCount:state.mouseMoveEventCount};
        if(JSON.stringify(current)!==JSON.stringify(previous)){previous=current;stableSince=Date.now();return false}
        return Date.now()-stableSince>=300 && current;
      },'fresh passive ACK and stable controlled foreground/cursor before the native background call',3000);
    } catch(error) { if(attempt===3)throw new Error(`External focus or pointer movement prevented a stable controlled background precondition: ${error.message}`); }
  }
}
function effectTrace(stage, toolId, action, label) {
  const row={at:new Date().toISOString(),stage,toolId,action,label,fixture:effectSnapshot()};
  writeFileSync(join(root,'effect-log.jsonl'),JSON.stringify(row)+'\n',{flag:'a',mode:0o600});
  receipt.effectTraceCount=(receipt.effectTraceCount ?? 0)+1;
}
function diagnosticSnapshot() {
  return {providerRequestCount:providerRequestSequence,toolEmissionCounts:Object.fromEntries(toolEmissionCounts),providerRequests:providerRequests.slice(-60),providerFailure:providerFailure ?? null,finalFixture:effectSnapshot()};
}

async function observeSession() {
  protocolFrame = null; protocolFiltering = null;
  await evaluate(`(() => {window.__computerAcceptance?.unsubs?.forEach(off=>off());const s={events:[],requests:[],genericRequests:[],unsubs:[]};window.__computerAcceptance=s;s.unsubs.push(window.lingxi.onEvent(e=>{if(e.sessionId===${JSON.stringify(sessionId)})s.events.push(e.event)}));s.unsubs.push(window.lingxi.onComputerAccess(e=>{if(e.sessionId===${JSON.stringify(sessionId)})s.requests.push(e.event)}));s.unsubs.push(window.lingxi.onPermission(e=>{if(e.sessionId===${JSON.stringify(sessionId)})s.genericRequests.push(e.event)}));return true})()`);
}
async function requestAction(input, label, resolveRequest = null, allowCancelled = false, preludes = [], expectedElement = null) {
  const marker = `COMPUTER_ACCEPTANCE_${randomUUID()}`;
  const toolId = `accept-${randomUUID()}`; actions.set(marker, { input, toolId, steps: createAcceptanceSteps(input, toolId, preludes), expectedElement });
  effectTrace('before',toolId,input.action,label);
  await evaluate('window.__computerAcceptance.events=[]; window.__computerAcceptance.requests=[]; window.__computerAcceptance.genericRequests=[]; true');
  await invoke('sendPrompt', sessionId, marker);
  const handled = new Set();
  const events = await waitFor(async () => {
    if(providerFailure)throw new Error(providerFailure);
    const state = await evaluate('({events:window.__computerAcceptance.events,requests:window.__computerAcceptance.requests,genericRequests:window.__computerAcceptance.genericRequests})');
    if (state.genericRequests.length) {
      const request = state.genericRequests[0];
      let input;
      try { input = JSON.parse(request.kind?.tool_input_json ?? '{}'); } catch { input = {}; }
      phase('unexpected-generic-permission', { label, requestId: request.request_id,
        kind: request.kind?.type ?? null, tool: request.kind?.tool_name?.slice(0, 160) ?? null,
        reason: typeof input.reason === 'string' ? input.reason.slice(0, 512) : null,
        source: { owner: request.owner ?? null, autoModePrompt: request.auto_mode_prompt ?? null,
          defaultAllow: request.kind?.default_allow ?? null, suppressAlwaysAllowRule: request.suppress_always_allow_rule ?? false } });
      await invoke('deny', sessionId, request.request_id);
      throw new Error(`Unexpected generic permission gate before Computer Access: ${label}`);
    }
    for (const req of state.requests) {
      if (handled.has(req.request_id)) continue; handled.add(req.request_id);
      if (!resolveRequest) { await invoke('denyComputerAccess', sessionId, req.request_id); throw new Error(`Unexpected approval in ${label}`); }
      await resolveRequest(req);
    }
    return state.events.some(event => event.type === 'turn_ended') && state.events;
  }, label, 31 * 60_000);
  const result = events.find(event => event.type === 'tool_use_result' && event.id === toolId);
  for (const { step, result: preludeResult } of requireAcceptancePreludes(actions.get(marker).steps, events)) {
    check('explicit-same-turn-prelude-succeeded', { label, primaryToolId: toolId, preludeToolId: step.toolId, action: step.input.action });
    phase('tool-prelude-result', { label, primaryToolId: toolId, toolId: step.toolId, action: step.input.action,
      isError: preludeResult.is_error, nativeProtocolResult: redactComputerProtocolResult(JSON.parse(preludeResult.result_json)) });
  }
  assert.ok(result || allowCancelled && events.some(event => event.type === 'turn_ended' && event.outcome?.type !== 'end_turn'), `${label}: expected a real computer tool result; ${JSON.stringify(events)}`);
  const rawProtocolResult = result ? JSON.parse(result.result_json) : null;
  const reported = result ? decodeComputerProtocolResult(rawProtocolResult, { geometry: fixtureState().screen,
    filtering: protocolFiltering, scope: input.action.startsWith('app_') ? 'app' : 'display' }) : null;
  if (reported?.screenshotFiltering === 'mask') protocolFiltering = reported.screenshotFiltering;
  if (reported?.type === 'image' && reported.captureScope !== 'app') protocolFrame = reported.frame;
  // Existing receipt assertions consume a decoded view, but retain the actual
  // native content separately so this adapter cannot claim an invented wire form.
  if (result) result.result_json = JSON.stringify(reported);
  const backgroundBefore = actions.get(marker).backgroundSurfaceBefore;
  if (backgroundBefore) {
    await delay(200); // Let the independent fixture publish activation notifications.
    const after = backgroundSurfaceSnapshot();
    receipt.backgroundSurfaceInvariants ??= [];
    receipt.backgroundSurfaceInvariants.push({ action: input.action, toolId, before: backgroundBefore, after,
      observation: 'NSWorkspace activation notification count, live cursor position and Quartz mouse-move event counter from the independent controlled fixture' });
    save();
    assertBackgroundSurfaceUnchanged(backgroundBefore, after, shadowId);
    check('background-action-keeps-foreground-and-cursor', { action: input.action, toolId });
  }
  assert.ok(reported?.tool_denial_kind !== 'permission-rule' && !reported?.tool_denial_kind?.startsWith('automode-'),
    `${label}: a generic permission/classifier failure is not native policy acceptance`);
  const safeResult = redactComputerProtocolResult(reported);
  effectTrace('after',toolId,input.action,label);
  receipt.diagnostics=diagnosticSnapshot();
  phase('tool-result', { label, toolId, emissionCount:toolEmissionCounts.get(toolId) ?? 0, isError: result?.is_error ?? null, result: safeResult, nativeProtocolResult: redactComputerProtocolResult(rawProtocolResult) });
  return { result, events };
}
// Preserve chromatic identity across display profiles and JPEG conversion.
// Pink must dominate green; black, white, and the comparison app's green fail.
function isTargetMarker(rgb) {
  return Array.isArray(rgb) && rgb.length === 3 && rgb.every(Number.isFinite)
    && rgb[0] > 0.7 && rgb[2] > 0.45 && rgb[1] < 0.45 && rgb[0] - rgb[1] > 0.3;
}
function requireSuccess(value, label) { assert.equal(value.result.is_error, false, `${label}: ${value.result.result_json}`); }
function requireBlocked(value, label) {
  assert.equal(value.result.is_error, true, `${label}: expected policy denial`);
  assert.doesNotMatch(value.result.result_json, /tool not found:/i, `${label}: a missing enabled tool is not policy-denial acceptance`);
}
const visiblePromptPhases = new Set();
async function recordVisibleComputerPrompt(req, label) {
  const kind = req.tcc_state ? 'tcc' : 'application';
  const selector = req.tcc_state ? '.computer-access-dialog' : '.computer-access-prompt';
  const visible = await waitFor(() => evaluate(`(() => {
    const element = document.querySelector(${JSON.stringify(selector)});
    if (!element) return false;
    for (let node = element; node instanceof Element; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (node.hasAttribute('hidden') || style.display === 'none'
        || style.visibility === 'hidden' || style.visibility === 'collapse' || Number(style.opacity) === 0) return false;
    }
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0 || rect.right <= 0 || rect.bottom <= 0
      || rect.left >= innerWidth || rect.top >= innerHeight) return false;
    return { selector: ${JSON.stringify(selector)}, bounds: { x: rect.x, y: rect.y, width: rect.width, height: rect.height },
      viewport: { width: innerWidth, height: innerHeight }, visible: true };
  })()`), `actual rendered ${kind} computer permission prompt`, 10000);
  if (!visiblePromptPhases.has(kind)) {
    check(`computer-${kind}-permission-prompt-visible`, { label, requestId: req.request_id, ...visible });
    visiblePromptPhases.add(kind);
  }
}
async function access(allow, label) {
  let tccObserved = false;
  const result = await requestAction({ action: 'request_access', apps: [fixtureId], reason: 'Exercise only the disposable permission acceptance window.' }, label, async req => {
    await recordVisibleComputerPrompt(req, label);
    if (req.tcc_state) {
      tccObserved = true; receipt.tcc.push({ label, at: new Date().toISOString(), ...req.tcc_state }); save();
      await userContinue('waiting-for-system-tcc', { requestId: req.request_id, tcc: req.tcc_state, applicationBundleId: 'com.lingxi.code.development' });
      await invoke('denyComputerAccess', sessionId, req.request_id); // TCC panel's Try Again uses this exact IPC; runtime rechecks the OS.
    } else {
      receipt.tcc.push({ label, at: new Date().toISOString(), accessibility: true, screen_recording: true, source: 'Real request_access passed both OS probes before delivering the application approval panel.' }); save();
      assert.ok(req.apps.every(app=>app.label===fixtureId),'Only the intended controlled bundle may be offered for authorization');
      if (allow) await invoke('approveComputerAccess', sessionId, req.request_id, { granted_apps: req.apps.map(app => app.label), clipboard_read: false, clipboard_write: false, system_key_combos: false, preferred_mode: 'full_control' });
      else await invoke('denyComputerAccess', sessionId, req.request_id);
    }
  });
  return { ...result, tccObserved };
}

async function acceptBackgroundProtocol() {
  const listed = await requestAction({ action: 'app_list_windows', app: fixtureId }, 'background list of the same disposable target windows');
  requireSuccess(listed, 'background window list');
  const windows = JSON.parse(listed.result.result_json);
  assert.ok(Array.isArray(windows), 'app_list_windows must return its native window records');
  const windowId = fixtureState().windowId;
  assert.ok(Number.isInteger(windowId) && windows.some(window => window.window_id === windowId), 'Native listing must identify the exact disposable fixture window');
  const shot = await requestAction({ action: 'app_screenshot', app: fixtureId, window_id: windowId }, 'background screenshot and AX summary');
  requireSuccess(shot, 'background screenshot');
  const image = JSON.parse(shot.result.result_json);
  assert.equal(image.captureScope, 'app');
  assert.match(image.message, /<ax-summary>/);
  assert.ok(image.message.includes(`window_id ${windowId}`), 'The image must bind the actual window ID');
  const sourceSize = fixtureState().windowSizePhysical;
  assert.ok(Math.abs(sourceSize[0] * image.height - sourceSize[1] * image.width) <= Math.max(...sourceSize) * 2,
    'The background image must have the controlled window aspect ratio, rather than a full-screen capture');
  const imagePath = join(root, 'background-target.image');
  writeFileSync(imagePath, Buffer.from(image.file.base64, 'base64'), { mode: 0o600 });
  const inspected = JSON.parse(execFileSync(fixtureExecutable, ['--inspect', imagePath], { encoding: 'utf8' }));
  assert.ok(inspected.nonBlackSamples > 100, 'A real nonempty target image must be returned');
  check('native-background-window-read', { windowId, imageSha256: createHash('sha256').update(Buffer.from(image.file.base64, 'base64')).digest('hex'), pixels: inspected });
  const backgroundPrelude = [{ action: 'app_screenshot', app: fixtureId, window_id: windowId }];
  const buttons = await requestAction({ action: 'app_ax_find', app: fixtureId, window_id: windowId, role: 'AXButton', title_contains: 'Record test click' }, 'background AX button discovery', null, false, backgroundPrelude);
  requireSuccess(buttons, 'background button discovery');
  const buttonIndex = axElementIndex(JSON.parse(buttons.result.result_json).message, 'AXButton', 'Record test click');
  const fields = await requestAction({ action: 'app_ax_find', app: fixtureId, window_id: windowId, role: 'AXTextField' }, 'background AX text-field discovery', null, false, backgroundPrelude);
  requireSuccess(fields, 'background field discovery');
  const fieldIndex = axElementIndex(JSON.parse(fields.result.result_json).message, 'AXTextField');
  const beforeClick = fixtureState();
  const clicked = await requestAction({ action: 'app_click', app: fixtureId, window_id: windowId, element_index: buttonIndex }, 'background AX press on the disposable button', null, false, backgroundPrelude, { role: 'AXButton', titleContains: 'Record test click' });
  requireSuccess(clicked, 'background AX button press');
  await waitFor(() => fixtureState().clicks === beforeClick.clicks + 1, 'actual background NSButton effect');
  check('native-background-click-effect', { windowId, elementIndex: buttonIndex, before: beforeClick.clicks, after: fixtureState().clicks });
  const text = `background-accepted-${randomUUID()}`;
  const typed = await requestAction({ action: 'app_type', app: fixtureId, window_id: windowId,
    element_index: fieldIndex, text, mode: 'replace', overwrite_existing: true }, 'background AX write into the disposable field', null, false, backgroundPrelude, { role: 'AXTextField' });
  requireSuccess(typed, 'background field write');
  await waitFor(() => fixtureState().text === text, 'actual background NSTextField value');
  check('native-background-type-effect', { windowId, elementIndex: fieldIndex, textSha256: createHash('sha256').update(text).digest('hex') });
  const replacementNonce = randomUUID();
  writeFileSync(fixtureControlPath, JSON.stringify({ replaceWindow: true, nonce: replacementNonce }));
  await waitFor(() => fixtureState().lastControlNonce === replacementNonce && fixtureState().windowId !== windowId, 'the same fixture replaced its old window without taking foreground');
  const beforeStale = fixtureState();
  const stale = await requestAction({ action: 'app_click', app: fixtureId, window_id: windowId, element_index: buttonIndex }, 'stale closed window ID must not dispatch input');
  requireBlocked(stale, 'stale window ID');
  assert.match(JSON.stringify(JSON.parse(stale.result.result_json)), /window_id|window.*belong|no longer|doesn.t exist/i);
  assert.equal(fixtureState().clicks, beforeStale.clicks);
  assert.equal(fixtureState().text, beforeStale.text);
  check('native-background-stale-window-refusal', { oldWindowId: windowId, replacementWindowId: fixtureState().windowId });
  requireSuccess(await requestAction({ action: 'app_release', app: fixtureId }, 'release disposable background locks before display verification', null, false, [{ action: 'app_screenshot', app: fixtureId, window_id: fixtureState().windowId }]), 'background lock release');
  const clearNonce = randomUUID();
  writeFileSync(fixtureControlPath, JSON.stringify({ clearText: true, nonce: clearNonce }));
  await waitFor(() => fixtureState().lastControlNonce === clearNonce && fixtureState().text === '', 'reset only the disposable fixture field for the later display test');
}

try {
  phase('preflight');
  const brokerManifest = JSON.parse(readFileSync(join(options.app, 'Contents/Resources/credential-broker/broker-manifest.json'), 'utf8'));
  assert.equal(brokerManifest.channel, 'development', 'This acceptance must use the isolated development channel');
  requireAbsentBrokerService();
  brokerPreflightConfirmed = true;
  brokerSourceSha256 = createHash('sha256').update(readFileSync(join(options.app, 'Contents/Resources/credential-broker/LingXiCredentialBroker.app/Contents/MacOS/LingXiCredentialBroker'))).digest('hex');
  receipt.brokerIsolation = { channel: 'development', target: brokerTarget, absentBeforeAnyFixtureLaunch: true,
    fixedUserHome: join(root, 'home'), preferencesDaemonAvoided: true };
  execFileSync('/usr/bin/codesign', ['--verify', '--deep', '--strict', options.app], { stdio: 'pipe' });
  const signature = spawnSync('/usr/bin/codesign', ['-dv', '--verbose=4', options.app], { encoding: 'utf8' });
  assert.equal(signature.status, 0); assert.match(signature.stderr, /^TeamIdentifier=AZ4AX7J833$/m); assert.match(signature.stderr, new RegExp(`^Identifier=${escapeRegex(appBundleId)}$`, 'm'));
  const isolatedProfile = runtimePathsForHome(join(root, 'home')).userDataDir;
  const processOutput = execFileSync('/bin/ps', ['-ax', '-o', 'pid=,command='], { encoding: 'utf8' });
  assertIsolatedProfileAvailable(processOutput, appExecutable, [isolatedProfile]);
  receipt.processIsolation = { exactSignedExecutable: appExecutable, privateProfile: isolatedProfile,
    cleanupGuard: 'Signals require the latched owned PID, exact candidate executable and this unique profile; other running applications are never selected.' };
  receipt.app = { path: options.app, signatureVerified: true, team: 'AZ4AX7J833', bundleId: 'com.lingxi.code.development' };
  const sidecar = join(options.app, 'Contents/Resources/bin/bridge-server');
  execFileSync('/usr/bin/codesign', ['--verify', '--strict', sidecar], { stdio: 'pipe' });
  receipt.sidecarSha256 = createHash('sha256').update(readFileSync(sidecar)).digest('hex');
  receipt.scriptSha256 = createHash('sha256').update(readFileSync(new URL(import.meta.url))).digest('hex');
  // macOS 27 accepts LSRegisterURL for /tmp apps yet intentionally omits them
  // from bundle-ID lookup. Install only these disposable fixtures in an explicit
  // non-temporary workspace directory; never alter the user's real Applications.
  mkdirSync(options.fixtureRoot, { recursive: true, mode: 0o700 });
  mkdirSync(join(root, 'home/Applications'), { recursive: true, mode: 0o700 });
  mkdirSync(join(root, 'tmp'), { recursive: true, mode: 0o700 });
  mkdirSync(dirname(fixtureExecutable), { recursive: true, mode: 0o700 });
  writeFileSync(join(fixtureBundle, 'Contents/Info.plist'), fixturePlist(fixtureId, 'Permission Target'));
  const swiftPath = join(root, 'PermissionTarget.swift'); writeFileSync(swiftPath, swiftSource);
  const moduleCache = options.swiftModuleCache || join(root, 'swift-module-cache');
  execFileSync('/usr/bin/xcrun', ['swiftc', '-module-cache-path', moduleCache, swiftPath, '-o', fixtureExecutable], { stdio: 'pipe', timeout: 120_000 });
  registerFixture(fixtureBundle, fixtureId); fixtureRegistered = true;
  symlinkSync(fixtureBundle, join(root, 'home/Applications/Permission Target.app'), 'dir');
  mkdirSync(dirname(shadowExecutable), { recursive: true, mode: 0o700 }); copyFileSync(fixtureExecutable, shadowExecutable);
  writeFileSync(join(shadowBundle, 'Contents/Info.plist'), fixturePlist(shadowId, 'Unauthorized Target'));
  registerFixture(shadowBundle, shadowId); shadowRegistered = true;
  symlinkSync(shadowBundle, join(root, 'home/Applications/Unauthorized Target.app'), 'dir');
  const probeSource = join(root, 'LaunchServicesProbe.swift'), probeExecutable = join(root, 'LaunchServicesProbe');
  writeFileSync(probeSource, `import Cocoa
let identifier=CommandLine.arguments[1]
let result:[String:Any]=["bundleId":identifier,"resolvedPath":NSWorkspace.shared.urlForApplication(withBundleIdentifier:identifier)?.path as Any? ?? NSNull(),"homeMatchesExpected":ProcessInfo.processInfo.environment["HOME"]==CommandLine.arguments[2]]
print(String(data:try JSONSerialization.data(withJSONObject:result,options:[.sortedKeys]),encoding:.utf8)!)
`);
  execFileSync('/usr/bin/xcrun', ['swiftc', '-module-cache-path', moduleCache, probeSource, '-o', probeExecutable], { stdio: 'pipe', timeout: 120_000 });
  receipt.fixtureRegistration = [];
  for (const [id, bundle] of [[fixtureId, fixtureBundle], [shadowId, shadowBundle]]) {
    for (const [context, environment] of [['fixture-home', fixtureEnvironment], ['launcher-home', process.env]]) {
      const lookup = JSON.parse(execFileSync(probeExecutable, [id, environment.HOME ?? ''], { env: environment, encoding: 'utf8' }));
      assert.equal(lookup.homeMatchesExpected, true); assert.equal(lookup.resolvedPath, realpathSync.native(bundle), 'LaunchServices must resolve the exact owned fixture; use a --fixture-root outside /tmp.');
      receipt.fixtureRegistration.push({ context, ...lookup });
    }
  }
  requireAbsentBrokerService();
  fixtureChild = spawn(fixtureExecutable, [fixtureStatePath, fixtureControlPath], { env: fixtureEnvironment, stdio: ['ignore', 'pipe', 'pipe'] });
  fixtureChild.stderr.on('data', chunk => writeFileSync(join(root, 'fixture.stderr.log'), chunk, { flag: 'a', mode: 0o600 })); fixtureChild.stdout.resume();
  await waitFor(() => existsSync(fixtureStatePath), 'native target startup');
  requireAbsentBrokerService();
  shadowChild = spawn(shadowExecutable, [shadowStatePath, shadowControlPath, '--unauthorized'], { env: fixtureEnvironment, stdio: ['ignore', 'ignore', 'pipe'] });
  shadowChild.stderr.on('data', chunk => writeFileSync(join(root, 'shadow.stderr.log'), chunk, { flag: 'a', mode: 0o600 }));
  await waitFor(() => existsSync(shadowStatePath), 'independent unauthorized target startup');
  assert.equal(shadowState().bundleId, shadowId); assert.equal(fixtureState().bundleId, fixtureId); assert.notEqual(shadowState().pid, fixtureState().pid);
  for (const [id, statePath] of [[fixtureId, fixtureStatePath], [shadowId, shadowStatePath]]) {
    requireAbsentBrokerService();
    execFileSync('/usr/bin/open', ['-b', id], { env: fixtureEnvironment, stdio: 'pipe', timeout: 10000 });
    await waitFor(() => JSON.parse(readFileSync(statePath, 'utf8')).frontmost, 'owned fixture activation by bundle ID', 10000);
  }
  check('fixture-bundle-identity-and-open-by-id', { fixtureId, shadowId, separatePids: true });
  await showControlledPair();

  const home = join(root, 'home'), temp = join(root, 'tmp'), workspace = join(root, 'workspace');
  for (const path of [home, temp, workspace]) mkdirSync(path, { recursive: true, mode: 0o700 });
  const paths = runtimePathsForHome(home); mkdirSync(paths.userDataDir, { recursive: true, mode: 0o700 });
  writeFileSync(paths.settingsPath, JSON.stringify(createPackagedSettings({ workspace })), { mode: 0o600 });
  writeFileSync(join(paths.userDataDir, 'conversation-spaces.v2.json'), JSON.stringify(createPackagedConversationSpaces({ workspace })), { mode: 0o600 });
  provider = createServer(async (request, response) => {
    const trace={requestId:++providerRequestSequence,at:new Date().toISOString(),method:request.method,path:request.url.split('?')[0],authenticated:request.headers.authorization===`Bearer ${key}`};
    providerRequests.push(trace);if(providerRequests.length>200)providerRequests.shift();
    const traceUpdate=patch=>{Object.assign(trace,patch);writeFileSync(join(root,'provider-request-log.jsonl'),JSON.stringify({...trace,observedAt:new Date().toISOString()})+'\n',{flag:'a',mode:0o600})};
    request.once('aborted',()=>traceUpdate({requestAborted:true}));
    response.once('finish',()=>traceUpdate({responseFinished:true}));
    response.once('close',()=>traceUpdate({responseClosed:true,closedBeforeFinish:!response.writableFinished}));
    traceUpdate({phase:'received'});
    try {
    if (request.headers.authorization !== `Bearer ${key}`) { response.writeHead(401).end(); return; }
    if (request.method === 'GET') {
      if (new URL(request.url,'http://127.0.0.1').pathname === '/v1/models') {response.writeHead(200,{'Content-Type':'application/json'}).end(JSON.stringify({object:'list',data:[{id:'acceptance-model',object:'model',created:0,owned_by:'local-acceptance-only'}]}));}
      else response.writeHead(404).end();
      return;
    }
    assert.equal(request.method,'POST','The local acceptance provider only accepts model listings and completion requests');
    let body = ''; for await (const chunk of request) body += chunk;
    const payload = JSON.parse(body); const messages = payload.messages ?? [];
    const selection = selectAcceptanceAction(messages, actions);
    assert.ok(selection, 'Synthetic provider must resolve a known acceptance instruction before emitting a response');
    traceUpdate({phase:'parsed',model:payload.model=== 'acceptance-model' ? payload.model:'<other-model>',messageCount:messages.length,roles:messages.map(message=>message.role),selectedMarkerSha256:selection ? createHash('sha256').update(selection[0]).digest('hex'):null,toolId:selection?.[1].toolId ?? null,action:selection?.[1].input.action ?? null,containsExactToolResult:!!selection && messages.some(message=>message.tool_call_id===selection[1].toolId),assistantToolCallIds:messages.flatMap(message=>message.role==='assistant' ? (message.tool_calls ?? []).map(call=>call.id):[]).slice(-30),toolResultIds:messages.filter(message=>message.role==='tool').map(message=>message.tool_call_id).slice(-30)});
    response.writeHead(200, { 'Content-Type': 'text/event-stream' });
    const send = data => response.write(`data: ${JSON.stringify(data)}\n\n`);
    const entry = selection[1];
    const step = nextAcceptanceStep(messages, entry.steps);
    if (step) {
      const { input, toolId } = step;
      traceUpdate({ nativeStepToolId: toolId, prelude: step.prelude, nativeAction: input.action });
      const priorSteps = entry.steps.slice(0, entry.steps.indexOf(step));
      if (priorSteps.length) {
        const events = await waitFor(async () => {
          const events = await evaluate('window.__computerAcceptance.events');
          return priorSteps.every(prior => events.some(event => event.type === 'tool_use_result' && event.id === prior.toolId)) && events;
        }, 'renderer received every exact same-turn prelude result', 5000);
        for (const { step: prior, result } of requireAcceptancePreludes(priorSteps, events)) {
          const decoded = decodeComputerProtocolResult(JSON.parse(result.result_json), { geometry: fixtureState().screen,
            filtering: protocolFiltering, scope: prior.input.action.startsWith('app_') ? 'app' : 'display' });
          if (decoded?.frame) entry.frame = decoded.frame;
          if (decoded?.captureScope === 'app' && entry.expectedElement) {
            const expected = entry.expectedElement;
            assert.equal(axElementIndex(decoded.message, expected.role, expected.titleContains ?? ''), entry.input.element_index,
              'The fresh same-turn AX summary must confirm the exact intended element index');
          }
        }
      }
      const nativeCall = nativeComputerCall(input, entry.frame ?? protocolFrame);
      const offered = payload.tools?.some(tool => tool.function?.name === nativeCall.name);
      if (!offered) {
        const discoveryId = `discover-${toolId}`;
        const discovered = messages.some(message => message.tool_call_id === discoveryId);
        if (!discovered) {
          assert.ok(payload.tools?.some(tool => tool.function?.name === 'ToolSearch'), 'Deferred native tools require the real ToolSearch entry point');
          send({ choices: [{ index: 0, delta: { role: 'assistant', tool_calls: [{ index: 0, id: discoveryId, type: 'function', function: { name: 'ToolSearch', arguments: JSON.stringify({ query: computerAcceptanceDiscoveryQuery, max_results: 20 }) } }] } }] });
          send({ choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] });
          response.end('data: [DONE]\n\n');
          return;
        }
        // A disabled host intentionally hides these descriptors. Emit the same
        // FQN once to verify its native policy rejection, never a legacy fallback.
        assert.equal((await settings({ action: 'get' })).enabled, false, `Native tool was not discovered: ${nativeCall.name}`);
      }
      const emissionCount=(toolEmissionCounts.get(toolId) ?? 0)+1;toolEmissionCounts.set(toolId,emissionCount);traceUpdate({emissionCount});
      if(emissionCount>1){providerFailure=`Synthetic provider attempted to emit duplicate tool call ${toolId}; this run is invalid, not accepted by deduplication.`;receipt.diagnostics=diagnosticSnapshot();save();throw new Error(providerFailure)}
      // Submitting a prompt can activate Desktop's composer. Restore only the
      // owned target immediately before emitting keyboard tools so focus is
      // established at execution time, rather than before prompt submission.
      if (!entry.chainStarted) {
        if (['type', 'key', 'key_down', 'key_up', 'hold_key', 'left_click', 'computer_batch'].includes(entry.input.action)) await activateFixture();
        if (entry.input.action.startsWith('app_')) entry.backgroundSurfaceBefore = await prepareBackgroundSurface();
        entry.chainStarted = true;
      }
      effectTrace('provider-emitted',toolId,input.action,'synthetic model tool emission');
      send({ choices: [{ index: 0, delta: { role: 'assistant', tool_calls: [{ index: 0, id: toolId, type: 'function', function: { name: nativeCall.name, arguments: JSON.stringify(nativeCall.arguments) } }] } }] });
      send({ choices: [{ index: 0, delta: {}, finish_reason: 'tool_calls' }] });
    } else {
      send({ choices: [{ index: 0, delta: { role: 'assistant', content: 'Permission acceptance step complete.' } }] });
      send({ choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } });
    }
    response.end('data: [DONE]\n\n');
    } catch (error) { traceUpdate({phase:'error',error:String(error.message ?? error)}); providerFailure=String(error.message ?? error); receipt.providerError = String(error.message ?? error); save(); if (!response.headersSent) response.writeHead(500); response.end(); }
  });
  await new Promise((resolvePromise, rejectPromise) => { provider.once('error', rejectPromise); provider.listen(0, '127.0.0.1', resolvePromise); });
  const env = sanitizePackagedAppEnvironment(process.env, { HOME: home, CFFIXED_USER_HOME: home,
    __CFPREFERENCES_AVOID_DAEMON: '1', TMPDIR: temp, LINGXI_CONFIG_DIR: join(home, '.lingxi') });
  for(const name of Object.keys(env))if(/(?:KEY|TOKEN|SECRET|PASSWORD|CREDENTIAL)/i.test(name))delete env[name];
  // LaunchServices creates the normal application responsibility boundary.
  // Directly spawning Contents/MacOS inherits the test runner's TCC identity.
  const launchEnvironment=Object.fromEntries(['HOME','CFFIXED_USER_HOME','__CFPREFERENCES_AVOID_DAEMON','TMPDIR','TMP','TEMP','PATH','LANG','LC_ALL','LC_CTYPE','USER','LOGNAME','SHELL','LINGXI_CONFIG_DIR'].filter(name=>env[name]!==undefined).map(name=>[name,env[name]]));
  appLaunchConfiguration={env,launchEnvironment,paths,workspace};
  const port=await launchOwnedPackagedApp();
  const bootstrap = await invoke('bootstrap'); sessionId = (await invoke('newSession', bootstrap.workspace.path)).activeSession.sessionId;
  await observeSession();
  providerId = `computer-acceptance-${randomUUID()}`;
  const patch = { providers: { [providerId]: { type: 'openai', baseUrl: `http://127.0.0.1:${provider.address().port}/v1`, models: [{ id: 'acceptance-model', capabilities: { streaming: true, tools: true, vision: true }, metadata: { inputModalities: ['text', 'image'], outputModalities: ['text'] } }] } } };
  // Providers belong to the Host registry; session update_settings rejects them.
  // Match the signed packaged smoke's optimistic Host-global settings exchange.
  const providerSettingsBefore = await invoke('providerSettings');
  const providerSettingsAfter = await invoke('providerSettings', patch, providerSettingsBefore.raw);
  assert.equal(providerSettingsAfter.effective.providers?.[providerId]?.baseUrl, patch.providers[providerId].baseUrl, 'Host must persist the exact private synthetic provider endpoint');
  await invoke('command', sessionId, { type: 'refresh_listings', which: [{ type: 'settings' }] });
  await waitFor(() => evaluate(`window.__computerAcceptance.events.some(e=>e.type==='settings_snapshot'&&JSON.parse(e.effective_json).providers?.[${JSON.stringify(providerId)}])`), 'synthetic provider registration');
  const beforeAccount = await invoke('providerAccounts.snapshot');
  const stored = await invoke('providerAccounts.command', { action: 'add', expectedRevision: beforeAccount.configRevision,
    providerId, executionBindingId: providerId, displayName: 'Computer acceptance fixture', authKind: 'api_key' }, key);
  providerAccountId = stored.accounts.find(entry => entry.providerId === providerId)?.accountId;
  syntheticCredentialStored = Boolean(providerAccountId);
  assert.ok(providerAccountId, 'The synthetic provider account must be registered');
  assert.equal(stored.health.find(entry => entry.accountId === providerAccountId)?.status, 'ready');
  const privateBroker = await waitFor(() => { const state = ownedBrokerState(); return state?.pid ? state : false; }, 'candidate credential Broker is installed and running only inside the private acceptance home', 10000);
  receipt.brokerIsolation.privateProgramVerified = true;
  receipt.brokerIsolation.pid = privateBroker.pid;
  save();
  if (options.explicitComputerRule) {
    await invoke('command', sessionId, {
      type: 'update_permission_rules', destination: 'user', behavior: 'allow', add: ['mcp__computer-use__*'], remove: [],
    });
    await invoke('command', sessionId, { type: 'refresh_listings', which: [{ type: 'settings' }] });
    await waitFor(async () => {
      const events = await evaluate('window.__computerAcceptance.events');
      return events.some(event => event.type === 'settings_snapshot' && JSON.parse(event.effective_json).permissions?.allow?.includes('mcp__computer-use__*'));
    }, 'explicit generic tool rule persisted before runtime restart');
    receipt.genericToolRule = { behavior: 'allow', tool: 'mcp__computer-use__*', scope: 'disposable test profile only; not default-pipeline acceptance' };
  } else receipt.genericToolRule = null;
  // Provider adapters are assembled at sidecar startup. Persist through the
  // public settings API first, then restart the disposable runtime from that state.
  // A settings snapshot alone is not proof that a running routing adapter changed.
  // newSession intentionally reuses an empty draft, so it cannot prove a fresh adapter.
  await invoke('restartBridge', sessionId);
  await configureAcceptanceModel();
  const modelReference=`${providerId}/acceptance-model`;
  phase('synthetic-provider-ready',{modelReference,freshRuntime:true});
  phase('signed-app-ready', { root, sessionId, cdpPort: port, fixture: fixtureState() });
  await mutate('setEnabled', { enabled: false });
  const disabledBefore = fixtureState();
  const disabledInput = await requestAction({ action: 'type', text: 'MUST_NOT_APPEAR_DISABLED' }, 'master disabled rejects input');
  assertDisabledComputerToolResult(disabledInput.result, await settings({ action: 'get' }), 'mcp__computer-use__type');
  assert.equal(fixtureState().text, disabledBefore.text);
  assert.equal(fixtureState().clicks, disabledBefore.clicks);
  check('master-disabled-no-effects', { disabledProjectionOrPolicyRejected: true });
  await mutate('setEnabled', { enabled: true });
  let denial = await access(false, 'dedicated access deny');
  while (denial.tccObserved) denial = await access(false, 'dedicated access deny after OS retry');
  requireSuccess(denial, 'dedicated denial response'); const denied = JSON.parse(denial.result.result_json); assert.deepEqual(denied.granted, []); assert.ok(denied.denied.includes(fixtureId));
  const deniedBefore = fixtureState(); requireBlocked(await requestAction({ action: 'left_click', coordinate: deniedBefore.button }, 'input after dedicated denial'), 'denied input'); assert.equal(fixtureState().clicks, deniedBefore.clicks); check('dedicated-approval-denial');
  let grant = await access(true, 'dedicated application grant');
  while (grant.tccObserved) grant = await access(true, 'dedicated grant after OS retry');
  requireSuccess(grant, 'application grant'); assert.ok(grantedApplicationIds(JSON.parse(grant.result.result_json)).includes(fixtureId)); check('dedicated-approval-grant');
  await verifyResponsibleApplication();
  await acceptBackgroundProtocol();
  const grantedList=await requestAction({action:'list_granted_applications'},'verify independent target is not granted');requireSuccess(grantedList,'granted list');assert.ok(!grantedList.result.result_json.includes(shadowId),'The independent comparison application must remain unauthorized');
  requireSuccess(await requestAction({ action: 'open_application', bundle_id: fixtureId }, 'open granted target'), 'open target'); await waitFor(()=>fixtureState().frontmost,'real Computer Use open_application activates its granted target',10000); await activateFixture();
  receipt.geometry = { ...fixtureState().screen, source: 'controlled native fixture display; native protocol has no display_size tool' };
  await showControlledPair();
  const visiblePair={authorized:fixtureState(),unauthorized:shadowState()};
  const shot = await requestAction({ action: 'screenshot' }, 'authorized real screenshot'); requireSuccess(shot, 'screenshot');
  const image = JSON.parse(shot.result.result_json); assert.equal(image.type, 'image'); assert.equal(image.screenshotFiltering, 'mask');
  const imageBytes = Buffer.from(image.file.base64, 'base64'); const imagePath = join(root, 'authorized-screenshot.image'); writeFileSync(imagePath, imageBytes, { mode: 0o600 });
  const initialPixels = JSON.parse(execFileSync(fixtureExecutable, ['--inspect', imagePath], { encoding: 'utf8' }));
  const marker = fixtureState().marker; const imageMarker = [Math.round(marker[0] * initialPixels.width / image.width), Math.round(marker[1] * initialPixels.height / image.height)];
  const pixels = JSON.parse(execFileSync(fixtureExecutable, ['--inspect', imagePath, ...imageMarker.map(String)], { encoding: 'utf8' }));
  assert.ok(pixels.nonBlackSamples > 100, 'Authorized target must appear in actual captured pixels');
  assert.ok(isTargetMarker(pixels.sampleRGB), 'Actual fixture marker must appear at its Retina-mapped screenshot coordinate');
  const maskedRegion=inspectRegion(imagePath,visiblePair.unauthorized.contentRect,image,pixels);assert.equal(maskedRegion.allBlack,true,'The visible independent unauthorized application content must be entirely masked');
  check('unauthorized-visible-application-masked',{bundleId:shadowId,sourceRect:visiblePair.unauthorized.contentRect,region:maskedRegion,visible:visiblePair.unauthorized.visible});
  check('authorized-real-screenshot', { imageSha256: createHash('sha256').update(imageBytes).digest('hex'), filtering: image.screenshotFiltering, pixels, originalWidth: image.width, originalHeight: image.height });
  let before = fixtureState();
  requireSuccess(await requestAction({ action: 'left_click', coordinate: before.button }, 'real target click', null, false, [{ action: 'screenshot' }]), 'target click');
  await waitFor(() => fixtureState().clicks === before.clicks + 1, 'native button click receipt'); check('native-click-effect');
  requireSuccess(await requestAction({ action: 'left_click', coordinate: fixtureState().field }, 'focus disposable field', null, false, [{ action: 'screenshot' }]), 'focus field');
  const text = `permission-accepted-${randomUUID()}`; requireSuccess(await requestAction({ action: 'type', text }, 'real target typing', null, false, [{ action: 'screenshot' }]), 'target typing');
  await waitFor(() => fixtureState().text === text, 'native text receipt'); check('native-type-effect');
  await mutate('setApplication', { bundleId: fixtureId, permission: 'allow' }); await activateFixture();
  const permanentBefore=fixtureState();requireSuccess(await requestAction({ action: 'left_click', coordinate: permanentBefore.button }, 'permanent application grant', null, false, [{ action: 'screenshot' }]), 'permanent grant');await waitFor(()=>fixtureState().clicks===permanentBefore.clicks+1,'permanent grant native effect'); check('permanent-grant-effective');
  await mutate('removeApplication', { bundleId: fixtureId }); before = fixtureState();
  requireBlocked(await requestAction({ action: 'left_click', coordinate: before.button }, 'removed permanent grant'), 'removed grant'); assert.equal(fixtureState().clicks, before.clicks); check('permanent-revocation-no-effects');
  await mutate('setApplication', { bundleId: fixtureId, permission: 'ask' });
  requireBlocked(await requestAction({ action: 'left_click', coordinate: before.button }, 'ask does not restore old session'), 'ask reset'); check('ask-reset-does-not-revive');
  requireSuccess(await access(true, 'fresh grant before active revocation'), 'fresh grant'); await activateFixture(); before = fixtureState();
  const active = requestAction({ action: 'computer_batch', actions: [{ action: 'left_click', coordinate: before.button }, { action: 'wait', duration: 4 }, { action: 'left_click', coordinate: before.button }] }, 'active batch revocation', null, true, [{ action: 'screenshot' }]);
  await waitFor(() => fixtureState().clicks === before.clicks + 1, 'first batch effect'); await mutate('setEnabled', { enabled: false });
  const revoked = await active; assert.ok(revoked.result?.is_error || revoked.events.some(event => event.type === 'turn_ended' && event.outcome?.type !== 'end_turn'));
  await delay(4500);effectTrace('after-revocation-settle',null,'computer_batch','active batch revocation');receipt.diagnostics=diagnosticSnapshot();save(); assert.equal(fixtureState().clicks, before.clicks + 1); check('active-batch-revocation-stops-next-effect');
  await mutate('setEnabled', { enabled: true });
  requireBlocked(await requestAction({ action: 'left_click', coordinate: before.button }, 're-enable cannot restore old grants'), 're-enable'); check('re-enable-no-old-grants');
  if (options.tccRoundtrip) {
    requireSuccess(await access(true, 'grant before OS revocation'), 'OS revoke preparation'); await activateFixture(); before = fixtureState();
    await userContinue('waiting-for-system-tcc-revocation', { applicationBundleId: 'com.lingxi.code.development', instruction: 'Disable Accessibility for this development application only. Keep the process running.' });
    await activateFixture();
    const attempt = await requestAction({ action: 'left_click', coordinate: before.button }, 'after OS Accessibility revocation');
    await delay(500); assert.equal(fixtureState().clicks, before.clicks); check('os-accessibility-revocation-no-effects', { toolReportedError: attempt.result.is_error });
    await activateFixture(); const revokedTextBefore=fixtureState().text;
    const typeAfterRevocation=await requestAction({action:'type',text:'MUST_NOT_APPEAR_OS_REVOKED'},'typing after OS Accessibility revocation');
    await delay(500); assert.equal(fixtureState().text,revokedTextBefore,'OS Accessibility revocation must prevent real keyboard input as well as hit-tested clicks');
    check('os-accessibility-revocation-no-type-effects',{toolReportedError:typeAfterRevocation.result.is_error});
    let sawMissing = false;
    const recheck = await requestAction({ action: 'request_access', apps: [fixtureId], reason: 'Recheck real OS permissions after operator revocation.' }, 'OS revoked recheck', async req => {
      assert.ok(req.tcc_state && !req.tcc_state.accessibility, 'OS revocation must appear in real TCC panel'); sawMissing = true;
      receipt.tcc.push({ label: 'after-operator-revocation', ...req.tcc_state }); save();
      await userContinue('waiting-for-system-tcc-restoration', { tcc: req.tcc_state, applicationBundleId: 'com.lingxi.code.development' }); await invoke('denyComputerAccess', sessionId, req.request_id);
    });
    assert.ok(sawMissing);
    requireBlocked(recheck, 'restored OS setup still requires a fresh app-selection call');
    assert.match(JSON.stringify(JSON.parse(recheck.result.result_json)), /macOS Accessibility and Screen Recording are now both granted/);
    check('os-accessibility-restoration-rechecked');
    requireSuccess(await access(true, 'fresh grant after OS restoration'), 'OS restored app grant'); await activateFixture(); before = fixtureState();
    requireSuccess(await requestAction({ action: 'left_click', coordinate: before.button }, 'input after OS restoration', null, false, [{ action: 'screenshot' }]), 'restored input'); await waitFor(() => fixtureState().clicks === before.clicks + 1, 'restored click'); check('os-restoration-real-execution');
    await activateFixture(); const restoredTextBefore=fixtureState().text; const restoredText='RESTORED_ACCESSIBILITY_INPUT';assert.ok(!restoredTextBefore.includes(restoredText));
    requireSuccess(await requestAction({action:'type',text:restoredText},'typing after OS Accessibility restoration', null, false, [{ action: 'screenshot' }]),'restored typing');
    await waitFor(()=>fixtureState().text!==restoredTextBefore && fixtureState().text.includes(restoredText),'restored keyboard input');check('os-restoration-real-typing');
  }
  if (options.tccScreenRoundtrip) {
    requireSuccess(await access(true,'grant before Screen Recording revocation'),'Screen Recording revoke preparation');await activateFixture();
    await userContinue('waiting-for-screen-recording-revocation',{
      applicationBundleId:'com.lingxi.code.development',
      instruction:'Disable Screen & System Audio Recording for this exact development application. Choose Later if macOS says recording continues until quit. Do not use system Quit & Reopen; the test will close and relaunch only its own private profile.'
    });
    await restartOwnedPackagedApp('apply Screen Recording revocation after macOS process-boundary notice');
    let sawMissing=false;
    const offRequest=await requestAction({action:'request_access',apps:[fixtureId],reason:'Probe Screen Recording after restarting the private signed app with permission disabled.'},'Screen Recording off after owned restart',async req=>{
      assert.ok(req.tcc_state && !req.tcc_state.screen_recording,'Restarted application must report actual Screen Recording denial');sawMissing=true;
      receipt.tcc.push({label:'screen-recording-disabled-after-owned-restart',applicationGeneration:appGeneration,...req.tcc_state});save();
      await invoke('denyComputerAccess',sessionId,req.request_id);
    });
    assert.ok(sawMissing);requireBlocked(offRequest,'Screen Recording denial after restart');
    await verifyResponsibleApplication();
    check('os-screen-recording-denial-after-owned-restart',{applicationGeneration:appGeneration});
    const grantsWhileDenied=await requestAction({action:'list_granted_applications'},'Screen Recording denial also blocks grant enumeration');requireBlocked(grantsWhileDenied,'OS denied grant enumeration');
    check('os-screen-recording-denial-blocks-non-access-operations',{toolReportedError:true});
    await activateFixture();
    const deniedShot=await requestAction({action:'screenshot'},'Screen Recording off rejects capture and old frame after restart');requireBlocked(deniedShot,'Screen Recording off capture');
    const deniedData=JSON.parse(deniedShot.result.result_json);assert.notEqual(deniedData?.type,'image');assert.ok(!deniedData?.file?.base64 && !deniedData?.computer_frame,'No cached old screenshot or frame may be returned by the new process');
    check('os-screen-recording-revocation-no-content-after-restart',{toolReportedError:true,noImageOrFrame:true});
    await userContinue('waiting-for-screen-recording-restoration',{
      applicationBundleId:'com.lingxi.code.development',
      instruction:'Restore Screen & System Audio Recording for the same exact development application. Choose Later if macOS offers Quit & Reopen. The test will close its own private process and relaunch it with the same test profile.'
    });
    await restartOwnedPackagedApp('apply restored Screen Recording permission after macOS process-boundary notice');
    const grantsBeforeFreshApproval=await requestAction({action:'list_granted_applications'},'restored OS permission does not restore previous process app grants');requireSuccess(grantsBeforeFreshApproval,'restored OS grant enumeration');
    const restoredGrantList=JSON.parse(grantsBeforeFreshApproval.result.result_json);assert.ok(Array.isArray(restoredGrantList.apps));assert.deepEqual(restoredGrantList.apps,[],'Old app grants must remain empty before a new dedicated approval');
    check('owned-restart-does-not-restore-session-app-grants',{emptyAppsBeforeFreshApproval:true});
    const restoredGrant=await access(true,'fresh application grant after Screen Recording restoration and restart');
    assert.equal(restoredGrant.tccObserved,false,'The restarted restored application must pass both OS probes before the dedicated application panel');requireSuccess(restoredGrant,'fresh restored application grant');assert.ok(grantedApplicationIds(JSON.parse(restoredGrant.result.result_json)).includes(fixtureId));
    check('os-screen-recording-restoration-rechecked',{applicationGeneration:appGeneration});await showControlledPair();
    const restoredShot=await requestAction({action:'screenshot'},'image after Screen Recording restoration and owned restart');requireSuccess(restoredShot,'restored Screen Recording');
    const restoredImage=JSON.parse(restoredShot.result.result_json);assert.equal(restoredImage.type,'image');assert.equal(restoredImage.screenshotFiltering,'mask');
    const restoredPath=join(root,'restored-screenshot.image');writeFileSync(restoredPath,Buffer.from(restoredImage.file.base64,'base64'),{mode:0o600});
    const restoredPixels=JSON.parse(execFileSync(fixtureExecutable,['--inspect',restoredPath],{encoding:'utf8'}));const restoredMarker=fixtureState().marker;const restoredPoint=[Math.round(restoredMarker[0]*restoredPixels.width/restoredImage.width),Math.round(restoredMarker[1]*restoredPixels.height/restoredImage.height)];
    const restoredSample=JSON.parse(execFileSync(fixtureExecutable,['--inspect',restoredPath,...restoredPoint.map(String)],{encoding:'utf8'}));assert.ok(isTargetMarker(restoredSample.sampleRGB),'Restored recording must expose the controlled target marker again');
    const restoredMask=inspectRegion(restoredPath,shadowState().contentRect,restoredImage,restoredPixels);assert.equal(restoredMask.allBlack,true,'The independent unauthorized window remains masked after restart');
    await verifyResponsibleApplication();check('os-screen-recording-restoration-execution',{pixels:restoredSample,unauthorizedMask:restoredMask,applicationGeneration:appGeneration});
  }
  receipt.status = 'passed'; phase('passed', { checks: receipt.checks.length });
} catch (error) {
  receipt.diagnostics=diagnosticSnapshot();
  receipt.status = 'failed'; receipt.error = String(error.stack ?? error); phase('failed', { error: String(error.message ?? error) }); process.exitCode = 1;
} finally {
  if(syntheticCredentialStored && !page){receipt.syntheticCredentialRemoved=false;receipt.credentialCleanupError='Application disconnected before synthetic credential cleanup; the run is not fully cleaned.'}
  if (page && providerAccountId) { try { const snapshot = await invoke('providerAccounts.snapshot'); await invoke('providerAccounts.command', { action: 'remove', expectedRevision: snapshot.configRevision, accountId: providerAccountId }); receipt.syntheticCredentialRemoved = true; } catch (error) { receipt.syntheticCredentialRemoved = false; receipt.credentialCleanupError = String(error.message ?? error); } }
  if (page) { try { if(ownedAppPidStillMatches())await page.send('Browser.close', {}, 3000); } catch {} page.close(); }
  try { await terminateOwnedApp(); } catch(error) { receipt.realAppProcessExited=false;receipt.applicationCleanupError=String(error.message ?? error); }
  try {
    if (brokerLaunchAttempted) {
      assert.equal(receipt.realAppProcessExited, true, 'Private application teardown must be confirmed before Broker removal');
      await waitFor(() => currentOwnedBridgePids().length === 0, 'private sidecars exit before Broker removal', 10000);
      await bootoutOwnedBroker();
      receipt.brokerCleanupPassed = true;
    } else {
      receipt.brokerCleanupPassed = true;
      receipt.brokerCleanupSkipped = 'The candidate application never launched; no Broker service was modified.';
    }
  } catch (error) { receipt.brokerCleanupPassed = false; receipt.brokerCleanupError = String(error.message ?? error); }
  if (fixtureChild && fixtureChild.exitCode === null) fixtureChild.kill('SIGTERM');
  if (shadowChild && shadowChild.exitCode === null) shadowChild.kill('SIGTERM');
  receipt.ownedProcessesExited=receipt.realAppProcessExited===true && (!appChild || receipt.launcherExited===true);
  for(const child of [fixtureChild,shadowChild])if(child){try{await waitFor(()=>child.exitCode!==null || child.signalCode!==null,'owned test process teardown',3000)}catch{receipt.ownedProcessesExited=false}}
  if (shadowRegistered) {try{execFileSync(launchServicesRegister,['-u',shadowBundle],{env:fixtureEnvironment,stdio:'pipe'});receipt.shadowUnregistered=true}catch{receipt.shadowUnregistered=false}}
  if (fixtureRegistered) { try { execFileSync(launchServicesRegister, ['-u', fixtureBundle], { env:fixtureEnvironment, stdio: 'pipe' }); receipt.fixtureUnregistered = true; } catch { receipt.fixtureUnregistered = false; } }
  if (provider) { provider.closeAllConnections(); await new Promise(resolvePromise => provider.close(resolvePromise)); }
  receipt.acceptancePassed=receipt.status==='passed';
  receipt.cleanupPassed=receipt.syntheticCredentialRemoved!==false && receipt.fixtureUnregistered!==false && receipt.shadowUnregistered!==false && receipt.ownedProcessesExited===true && receipt.brokerCleanupPassed===true;
  if(!receipt.cleanupPassed){if(receipt.acceptancePassed)receipt.status='passed_with_cleanup_error';process.exitCode=1;phase('cleanup-failed',{syntheticCredentialRemoved:receipt.syntheticCredentialRemoved,fixtureUnregistered:receipt.fixtureUnregistered,shadowUnregistered:receipt.shadowUnregistered})}
  receipt.diagnostics=diagnosticSnapshot();receipt.finishedAtUtc = new Date().toISOString(); save();
}
