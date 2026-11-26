import { useEffect, useRef, useState } from 'react';
import type { EditableLayer, PageContentProps } from './SettingsScreen';
import { tr } from '../../i18n/locale';

type Bridge = PageContentProps['bridge'];
export type ExtensionCatalogTab = 'public' | 'personal';

export interface ExtensionScopeNavigation {
  view?: ExtensionCatalogTab;
  setView: (view: ExtensionCatalogTab) => void;
  layer: EditableLayer;
  projectDir: string | null;
  activeProject: string | null;
  projects: string[];
  selectUser: () => void;
  selectRecommended: () => void;
  selectProject: (path: string) => void;
}

function projectKey(path: string) {
  const normalized = path.replace(/\\/g, '/').replace(/\/+$/, '');
  return /^(?:[a-z]:|\/\/)/i.test(normalized) ? normalized.toLowerCase() : normalized;
}

/** Selecting a workspace alone does not change the engine that owns project settings. */
export async function switchSettingsProject(bridge: Bridge, path: string, stillCurrent = () => true) {
  const workspace = await bridge.activateProject(path);
  if (!stillCurrent()) return;
  if (!workspace?.path) throw new Error(tr('desktop_shell_switch_project_error'));
  const target = workspace.path;
  const catalog = bridge.bootstrap?.projectCatalogs?.[target] ?? await bridge.listProjectSessions(target);
  if (!stillCurrent()) return;
  if (!catalog || catalog.error) throw new Error(catalog?.error || tr('desktop_shell_switch_project_error'));
  const spaces = bridge.bootstrap?.conversationSpaces;
  const sessions = catalog.sessions.filter(entry => {
    const owner = spaces?.sessions.find(candidate => candidate.sessionId === entry.uuid);
    return entry.mode === spaces?.activeMode && (!owner || owner.projectId === target);
  });
  const latest = sessions.reduce<(typeof sessions)[number] | undefined>((latest, session) =>
    !latest || session.modified_rfc3339 > latest.modified_rfc3339 ? session : latest, undefined);
  if (!latest) {
    if (!await bridge.newSession(target)) throw new Error(tr('desktop_link_project_conversation_error'));
  } else if (!await bridge.openSession(target, latest.uuid, { scope: 'current', transcriptPath: latest.path })) {
    throw new Error(tr('desktop_link_project_conversation_error'));
  }
  return target;
}

export function useExtensionScopes(bridge: Bridge, page: string, projectDir: string | null,
  layer: EditableLayer, setLayer: (layer: EditableLayer) => void, clearLink: () => void) {
  const [viewState, setViewState] = useState<{ page: string; value: ExtensionCatalogTab }>();
  const [pending, setPending] = useState<{ path: string; target?: string } | null>(null);
  const [lastProject, setLastProject] = useState<{ path: string; target: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const request = useRef(0);
  const workspace = bridge.bootstrap?.workspace?.path;
  // The host may canonicalize a registered directory (for example a symlink).
  // Keep its registered tab selected only while the engine confirms the same target.
  const registered = bridge.bootstrap?.settings.projects ?? [];
  const owner = projectDir && lastProject && projectKey(projectDir) === projectKey(lastProject.target) ? lastProject.path : projectDir;
  const current = owner ?? workspace;
  const byPath = new Map(registered.map(path => [projectKey(path), path]));
  if (current && !byPath.has(projectKey(current))) byPath.set(projectKey(current), current);
  const projects = [...byPath.values()];
  const activeProject = owner ? projects.find(path => projectKey(path) === projectKey(owner)) ?? null : null;

  function setView(value: ExtensionCatalogTab) {
    setViewState({ page, value });
  }

  function cancel() {
    request.current += 1;
    setPending(null);
    setError(null);
  }

  useEffect(() => {
    setPending(null);
    setError(null);
    return () => { request.current += 1; };
  }, [page]);

  useEffect(() => {
    if (pending?.target && projectDir && projectKey(projectDir) === projectKey(pending.target) && bridge.connected && !bridge.sessionLoading) {
      setLastProject({ path: pending.path, target: pending.target });
      setLayer('project');
      setViewState({ page, value: 'personal' });
      setPending(null);
    }
  }, [pending, projectDir, bridge.connected, bridge.sessionLoading, page, setLayer]);

  function selectRecommended() {
    cancel();
    clearLink();
    setLayer('user');
    setView('public');
  }

  function selectUser() {
    cancel();
    clearLink();
    setLayer('user');
    setView('personal');
  }

  async function selectProject(path: string) {
    cancel();
    clearLink();
    if (activeProject === path && bridge.connected && !bridge.sessionLoading) {
      setLayer('project');
      setView('personal');
      return;
    }
    const id = request.current;
    setPending({ path });
    try {
      const inSession = bridge.activeSession?.projectPath && projectKey(bridge.activeSession.projectPath) === projectKey(path);
      const target = inSession && !projectDir ? path : await switchSettingsProject(bridge, path, () => request.current === id);
      if (!target || request.current !== id) return;
      await bridge.refreshSettingsSnapshot();
      if (request.current === id) setPending({ path, target });
    } catch (cause) {
      if (request.current !== id) return;
      setPending(null);
      setError(cause instanceof Error ? cause.message : tr('desktop_shell_switch_project_error'));
    }
  }

  return {
    view: viewState?.page === page ? viewState.value : undefined, setView,
    layer, projectDir, activeProject, projects, selectUser, selectRecommended, selectProject,
    pendingProject: pending?.path, error, cancel,
  };
}
