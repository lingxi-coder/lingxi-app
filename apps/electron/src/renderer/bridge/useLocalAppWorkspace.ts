import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { DesktopLocalApp, LocalAppAuthoringSession } from '../../shared/localApps';
import type { UseBridge } from './bridgeTypes';
import { localAppWorkspaceGroups } from './localAppWorkspace';
import { desktopHost } from './bridgeConnection';
import { localAppErrorMessage } from './localAppErrors';

export function useLocalAppWorkspace(bridge: UseBridge) {
  const api = desktopHost()?.localApps;
  const [apps, setApps] = useState<DesktopLocalApp[]>([]);
  const [sources, setSources] = useState<LocalAppAuthoringSession[]>([]);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(true);
  const generation = useRef(0);
  const refresh = useCallback(async () => {
    const current = ++generation.current;
    if (!api) { setLoading(false); return; }
    try {
      const [nextApps, nextSources] = await Promise.all([api.list(), api.authoringSessions?.() ?? Promise.resolve([])]);
      if (generation.current !== current) return;
      setApps(nextApps); setSources(nextSources); setError('');
    } catch (cause) { if (generation.current === current) setError(localAppErrorMessage(cause)); }
    finally { if (generation.current === current) setLoading(false); }
  }, [api]);
  useEffect(() => {
    void refresh();
    const off = api?.onChanged(() => { void refresh(); });
    window.addEventListener('focus', refresh);
    return () => { generation.current++; off?.(); window.removeEventListener('focus', refresh); };
  }, [api, refresh, bridge.bootstrap?.conversationSpaces.activeMode, bridge.activeSession?.sessionId]);
  const groups = useMemo(() => localAppWorkspaceGroups(apps, sources, bridge.bootstrap?.projectCatalogs,
    bridge.bootstrap?.conversationSpaces.drafts, bridge.bootstrap?.conversationSpaces.sessions),
    [apps, sources, bridge.bootstrap?.projectCatalogs, bridge.bootstrap?.conversationSpaces.drafts, bridge.bootstrap?.conversationSpaces.sessions]);
  const pathKey = [...new Set(groups.flatMap(group => group.projectPath ? [group.projectPath] : []))].sort().join('\0');
  const projectPaths = useMemo(() => pathKey ? pathKey.split('\0') : [], [pathKey]);
  const activeGroup = groups.find(group => bridge.activeSession?.mode === 'code'
    && bridge.activeSession.projectId === group.projectPath);
  return { groups, projectPaths, activeGroup, error, loading, refresh };
}
