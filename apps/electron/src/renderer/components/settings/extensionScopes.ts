import { useState } from 'react';
import type { EditableLayer } from './SettingsScreen';

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

export function projectKey(path: string) {
  const normalized = path.replace(/\\/g, '/').replace(/\/+$/, '');
  return /^(?:[a-z]:|\/\/)/i.test(normalized) ? normalized.toLowerCase() : normalized;
}

export function useExtensionScopes(registered: readonly string[], page: string, projectDir: string | null,
  layer: EditableLayer, setLayer: (layer: EditableLayer) => void, setProject: (project?: string) => void, clearLink: () => void): ExtensionScopeNavigation {
  const [viewState, setViewState] = useState<{ page: string; value: ExtensionCatalogTab }>();
  const byPath = new Map(registered.map(path => [projectKey(path), path]));
  if (projectDir && !byPath.has(projectKey(projectDir))) byPath.set(projectKey(projectDir), projectDir);
  const setView = (value: ExtensionCatalogTab) => setViewState({ page, value });
  function select(view: ExtensionCatalogTab, project?: string) {
    clearLink();
    setProject(project);
    setLayer(project ? 'project' : 'user');
    setView(view);
  }
  return {
    view: viewState?.page === page ? viewState.value : layer === 'project' && projectDir ? 'personal' : undefined, setView,
    layer, projectDir, projects: [...byPath.values()], activeProject: projectDir ? byPath.get(projectKey(projectDir)) ?? null : null,
    selectUser: () => select('personal'), selectRecommended: () => select('public'), selectProject: path => select('personal', path),
  };
}
