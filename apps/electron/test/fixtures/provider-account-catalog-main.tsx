import { createRoot } from 'react-dom/client';
import { useBridge } from '../../src/renderer/bridge/useBridge';
import { fixtureAccountSnapshot } from './provider-accounts-mock';
import { DESKTOP_BRIDGE_GLOBAL } from '../../src/shared/desktopBridge';

const projectPath = '/fixture/catalog', sessionId = '11111111-2222-4333-8444-555555555555';
const reference = 'openai-chatgpt/new-published-model';
let bridge: ReturnType<typeof useBridge>;
let published = false, reads = 0, dispatches = 0, finishDispatch: (() => void) | undefined;
let holdSnapshot = false, finishSnapshot: (() => void) | undefined;
let finishAccountCommand: (() => void) | undefined;
const listeners = new Set<(envelope: any) => void>();
const accounts = () => {
  const snapshot = fixtureAccountSnapshot(['openai-chatgpt']);
  if (published) snapshot.modelAvailability!.push({ ...snapshot.modelAvailability![0]!, modelId: 'new-published-model' });
  return snapshot;
};
const until = async (ready: () => boolean) => {
  const deadline = Date.now() + 5_000;
  while (!ready()) {
    if (Date.now() > deadline) throw new Error('Catalog fixture state did not settle');
    await new Promise(resolve => setTimeout(resolve, 20));
  }
};
(window as any)[DESKTOP_BRIDGE_GLOBAL] = {
  platform: 'darwin', isElectron: true,
  onEvent: (listener: (envelope: any) => void) => { listeners.add(listener); return () => listeners.delete(listener); },
  onConnectionStateChanged: () => () => {}, onPermission: () => () => {}, onComputerAccess: () => () => {},
  bootstrap: async () => ({ revision: 1, settings: { projects: [projectPath], activeProject: projectPath,
    activeSession: { projectPath, sessionId }, pinnedSessions: [], archivedSessions: [] },
    activeSession: { projectPath, sessionId }, workspace: { path: projectPath, trusted: true }, connection: { status: 'connected' },
    runtimes: [{ projectPath, sessionId, connection: { status: 'connected' }, turnActive: false, pendingInteractions: 0, pendingAskUserQuestions: 0 }],
    projectCatalogs: { [projectPath]: { sessions: [] } }, providerAccountSnapshot: accounts(), diagnostics: [], versions: { app: 'fixture', electron: 'fixture' } }),
  providerAccounts: { snapshot: async () => {
    reads++;
    if (holdSnapshot) await new Promise<void>(resolve => { finishSnapshot = resolve; });
    return accounts();
  }, command: async () => {
    const oldCatalog = accounts();
    await new Promise<void>(resolve => { finishAccountCommand = resolve; });
    return oldCatalog;
  } },
  command: async (_session: string, command: { type: string }) => {
    if (command.type === 'refresh_provider_catalog') { dispatches++; await new Promise<void>(resolve => { finishDispatch = resolve; }); }
  },
  listProjectSessions: async () => ({ projectPath, sessions: [] }),
};
(window as any).catalogFixture = {
  ready: () => Boolean(bridge?.bootstrap),
  run: async () => {
    const healthy = bridge.bootstrap!.providerAccountSnapshot!.health.every(row => row.status === 'ready');
    const oldAccountCommand = bridge.providerAccounts!.command({ action: 'test', expectedRevision: 1,
      accountId: bridge.bootstrap!.providerAccountSnapshot!.accounts[0]!.accountId });
    const manual = bridge.refreshModelPicker('manual');
    await until(() => dispatches === 1);
    const noPrePublicationRead = dispatches === 1 && reads === 0;
    published = true; holdSnapshot = true;
    for (const event of [{ type: 'provider_model_catalog', providers: [] },
      { type: 'model_list', models: [reference], current: 'openai-chatgpt/fixture-model' }]) {
      for (const listener of listeners) listener({ sessionId, projectPath, event });
    }
    const eligible = () => Boolean(bridge.bootstrap?.providerAccountSnapshot?.modelAvailability?.some(row =>
      row.modelId === 'new-published-model' && row.eligibleAccountIds.length));
    await until(() => Boolean(bridge.providerAccountsRefreshing && finishSnapshot));
    const pendingRowsLoading = bridge.providerAccountsRefreshing && !eligible();
    holdSnapshot = false; finishSnapshot!();
    await until(() => bridge.desktop.models.includes(reference) && eligible());
    const newModelEligible = bridge.desktop.models.includes(reference) && eligible();
    const beforeLateCommand = reads;
    finishAccountCommand!(); await oldAccountCommand;
    await until(() => reads > beforeLateCommand && !bridge.providerAccountsRefreshing && eligible());
    const lateCommandPreservesNewModel = eligible();
    finishDispatch!(); await manual;
    return { healthy, noPrePublicationRead, pendingRowsLoading, newModelEligible, lateCommandPreservesNewModel, manualFinalRows: eligible(),
      noTurn: bridge.bootstrap!.runtimes.every(runtime => !runtime.turnActive), reads };
  },
};
function Fixture() { bridge = useBridge(); return <div>Catalog account projection fixture</div>; }
createRoot(document.getElementById('root')!).render(<Fixture />);
