import { useMemo, useSyncExternalStore } from 'react';
import type { ProjectSettingsCommand, ProjectSettingsRequest } from '../../../shared/projectSettings';
import type { ConfigurationOperationEvent, UseBridge } from '../../bridge/bridgeTypes';

type State = Pick<UseBridge, 'settingsSnapshotEvent' | 'skillCatalogEvent' | 'skillDocumentEvent'
  | 'mcpConfigurationSnapshotEvent' | 'pluginCatalogEvent' | 'configurationOperations'>;
const empty: State = {
  settingsSnapshotEvent: null, skillCatalogEvent: null, skillDocumentEvent: null,
  mcpConfigurationSnapshotEvent: null, pluginCatalogEvent: null, configurationOperations: {},
};
const idle = () => () => {};
const emptyState = () => empty;

/** Each selected directory owns its commands and replies, even after the user leaves that tab. */
export function createProjectSettings(path: string, request?: ProjectSettingsRequest) {
  let state = empty;
  let tail: Promise<unknown> = Promise.resolve();
  const subscribers = new Set<() => void>();
  function dispatch(command: ProjectSettingsCommand): Promise<ConfigurationOperationEvent | undefined> {
    const result = tail.then(async () => {
      if (!request) throw new Error('Project settings service unavailable.');
      const events = await request(path, command);
      let operation: ConfigurationOperationEvent | undefined;
      let next = state;
      for (const event of events) {
        switch (event.type) {
          case 'settings_snapshot': next = { ...next, settingsSnapshotEvent: event }; break;
          case 'skill_catalog': next = { ...next, skillCatalogEvent: event }; break;
          case 'skill_document': next = { ...next, skillDocumentEvent: event }; break;
          case 'plugin_catalog': next = { ...next, pluginCatalogEvent: event }; break;
          case 'mcp_configuration_snapshot': next = { ...next, mcpConfigurationSnapshotEvent: event }; break;
          case 'configuration_operation':
            operation = event;
            next = { ...next, configurationOperations: { ...next.configurationOperations, [event.domain]: event } };
            break;
          case 'error': throw new Error(event.message);
        }
      }
      state = next;
      for (const notify of subscribers) notify();
      return operation;
    });
    tail = result.catch(() => {});
    return result;
  }
  const send = async (command: ProjectSettingsCommand) => { await dispatch(command); };
  const refreshSkills = () => send({ type: 'skill_admin', command: { action: 'get_catalog' } });
  const actions: Pick<UseBridge, 'refreshSettingsSnapshot' | 'updateEngineSettings' | 'updatePermissionRules'
    | 'setDefaultPermissionMode' | 'updateWorkspaceDirectories' | 'refreshSkills' | 'refreshMcpServers'
    | 'skillAdmin' | 'mcpAdmin' | 'pluginAdmin' | 'hookAdmin' | 'runSlashCommand'> = {
    refreshSettingsSnapshot: () => send({ type: 'refresh_listings', which: [{ type: 'settings' }] }),
    updateEngineSettings: (destination, patch) => send({ type: 'update_settings', destination, patch_json: JSON.stringify(patch) }),
    updatePermissionRules: (destination, behavior, add, remove) => send({ type: 'update_permission_rules', destination, behavior, add, remove }),
    setDefaultPermissionMode: (destination, mode) => send({ type: 'set_default_permission_mode', destination, mode }),
    updateWorkspaceDirectories: (destination, add, remove) => send({ type: 'update_workspace_directories', destination, add, remove }),
    refreshSkills,
    refreshMcpServers: () => send({ type: 'mcp_admin', command: { action: 'get_snapshot' } }),
    skillAdmin: command => dispatch({ type: 'skill_admin', command }),
    mcpAdmin: command => dispatch({ type: 'mcp_admin', command }),
    pluginAdmin: command => dispatch({ type: 'plugin_admin', command }),
    hookAdmin: command => dispatch({ type: 'hook_admin', command }),
    runSlashCommand: async raw => {
      if (raw !== '/reload-skills') return false;
      await refreshSkills();
      return true;
    },
  };
  return {
    actions, getSnapshot: () => state,
    subscribe: (listener: () => void) => { subscribers.add(listener); return () => { subscribers.delete(listener); }; },
  };
}

export function useProjectSettingsBridge(live: UseBridge, path: string | undefined): UseBridge {
  const store = useMemo(() => path ? createProjectSettings(path, live.projectSettings) : null, [path, live.projectSettings]);
  const state = useSyncExternalStore(store?.subscribe ?? idle, store?.getSnapshot ?? emptyState, emptyState);
  return store ? {
    ...live, ...state, ...store.actions, settingsOnly: true, connected: true, sessionLoading: false,
    activeSession: undefined, agentCatalog: [], hooksCatalog: [], skillsEvent: null, mcpServersEvent: null,
  } : live;
}
