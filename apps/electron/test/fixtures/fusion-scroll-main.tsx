import { useCallback, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { SettingsScreen } from '../../src/renderer/components/settings/SettingsScreen';
import { Theme } from '../../src/renderer/theme/ThemeContext';
import { tokens } from '../../src/renderer/theme/tokens';
import type { FusionCatalog, FusionStrategyEntry } from '../../src/renderer/components/settings/pages/FusionCatalog';
import '../../src/renderer/global.css';

const initialCatalog: FusionCatalog = {
  scopes: ['user', 'project', 'local'].map((scope) => ({ scope: scope as 'user' | 'project' | 'local', admitted: scope !== 'local', path: `/fixture/${scope}/settings.json`, revision: 'r1',
    strategies: [{ id: `${scope}-review`, name: scope === 'local' ? '排除的策略' : '代码审查', config: { panelModels: [{ profile: 'fixture', model: 'model-0' }, { profile: 'fixture', model: 'model-1' }], analystModel: { profile: 'fixture', model: 'model-2' } } }],
    default_strategy_id: null, globals: {}, diagnostic: null })),
  effective_globals: { enabled: false, proactive: false }, default_strategy: null, diagnostics: [],
};
function Fixture() {
  const [catalog, setCatalog] = useState(initialCatalog);
  const catalogRef = useRef(catalog); catalogRef.current = catalog;
  const [operation, setOperation] = useState<object>();
  const [sessionId, setSessionId] = useState('fixture');
  const sessionRef = useRef(sessionId); sessionRef.current = sessionId;
  const delayedWrite = useRef<{ delay: boolean; release?: () => void }>({ delay: false });
  const [dark, setDark] = useState(false);
  const [closes, setCloses] = useState(0);
  const commands = useRef<object[]>([]);
  const fusionAdmin = useCallback(async (command: { action: string; scope?: string; revision?: string; target?: string; payload_json?: string; operation_id?: number }) => {
    commands.current.push(command);
    if (command.action === 'get_catalog') return;
    const owner = sessionRef.current;
    const next = structuredClone(catalogRef.current);
    const scope = next.scopes.find((entry) => entry.scope === command.scope)!;
    const event = { type: 'configuration_operation', domain: 'fusion', operation_id: command.operation_id, effect: 'applied' };
    if (delayedWrite.current.delay) {
      delayedWrite.current.delay = false;
      await new Promise<void>((resolve) => { delayedWrite.current.release = resolve; });
    }
    if (scope.revision !== command.revision) { setOperation({ ...event, status: 'failed', message: '配置版本冲突，请刷新后重试。' }); return; }
    if (command.action === 'save') {
      const strategy = JSON.parse(command.payload_json!) as FusionStrategyEntry;
      const index = scope.strategies.findIndex((entry) => entry.id === strategy.id);
      if (index < 0) scope.strategies.push(strategy); else scope.strategies[index] = strategy;
      scope.diagnostic = null;
    } else if (command.action === 'delete') {
      scope.strategies = scope.strategies.filter((entry) => entry.id !== command.target);
      if (scope.default_strategy_id === command.target) scope.default_strategy_id = null;
    } else if (command.action === 'set_default') scope.default_strategy_id = command.target ?? null;
    else if (command.action === 'set_globals') scope.globals = JSON.parse(command.payload_json!);
    scope.revision = `r${Number(scope.revision.slice(1)) + 1}`;
    const defaultScope = [...next.scopes].reverse().find((entry) => entry.default_strategy_id);
    next.default_strategy = defaultScope ? { scope: defaultScope.scope, id: defaultScope.default_strategy_id! } : null;
    const receipt = { ...event, status: 'succeeded' };
    if (owner === sessionRef.current && catalogRef.current.scopes.find((entry) => entry.scope === command.scope)?.path === scope.path) {
      catalogRef.current = next; setCatalog(next); setOperation(receipt);
    }
    return receipt;
  }, []);
  (window as unknown as { fusionFixture: object }).fusionFixture = {
    catalog, commands: commands.current, closes, setDark,
    concurrentEdit: () => { const next = structuredClone(catalogRef.current); next.scopes[0]!.revision = 'r999'; catalogRef.current = next; setCatalog(next); },
    invalidateNamedStrategy: () => {
      const next = structuredClone(catalogRef.current);
      next.scopes[0]!.strategies.find((strategy) => strategy.name === 'Review renamed')!.config.maxPanel = 9;
      next.scopes[0]!.diagnostic = 'fusion.maxPanel must be 2..=8';
      catalogRef.current = next; setCatalog(next);
    },
    delayNextWrite: () => { delayedWrite.current.delay = true; },
    completeDelayedWrite: () => { delayedWrite.current.release?.(); },
    switchProjectPath: () => {
      setOperation(undefined);
      const next = structuredClone(catalogRef.current);
      for (const scope of next.scopes) if (scope.scope !== 'user') scope.path = `/other-project/${scope.scope}/settings.json`;
      catalogRef.current = next; setCatalog(next);
    },
    switchSession: () => { sessionRef.current = 'other-session'; setSessionId('other-session'); setOperation(undefined); },
  };
  // No real sessions, credentials, engine, or persistence.
  const bridge = {
    activeSession: { sessionId, projectPath: sessionId === 'fixture' ? '/fixture' : '/other-project' }, connected: true, sessionLoading: false,
    settingsSnapshotEvent: { type: 'settings_snapshot', effective_json: '{}', provenance_json: '{}', layers_json: '{"user":{},"project":{},"local":{}}' },
    refreshSettingsSnapshot: async () => {},
    bootstrap: { settings: {}, workspace: {}, providerCredentials: [{ providerId: 'fixture', configured: true }] },
    desktop: { providerModelCatalog: [{ provider_id: 'fixture', provider_label: 'Configured provider',
      models: Array.from({ length: 72 }, (_, index) => ({ model_id: `model-${index}`, display_name: `Model ${String(index).padStart(2, '0')}`, fusion_analyst_capable: true })) }] },
    updateEngineSettings: async () => {}, fusionAdmin,
    fusionCatalogEvent: { type: 'fusion_catalog', catalog_json: JSON.stringify(catalog) },
    configurationOperations: { fusion: operation },
  };
  return <Theme.Provider value={tokens(dark)}><SettingsScreen bridge={bridge as never} theme={dark ? 'dark' : 'light'} onTheme={() => setDark((value) => !value)} initialPageId="fusion" onClose={() => setCloses((value) => value + 1)} /></Theme.Provider>;
}
createRoot(document.getElementById('root')!).render(<Fixture />);
