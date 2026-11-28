import { useCallback, useContext, useEffect, useRef, useState, type ReactNode } from 'react';
import { ChatTopbarTarget } from './ChatTopbar';
import type { TeamSettingsSection } from './TeamSettingsPage';
import type { TeamsSnapshot } from './workspace';

interface Route {
  id: number;
  scope: string;
  section: TeamSettingsSection;
  opener?: HTMLElement;
  openedFromHeader: boolean;
}

export function useTeamSettings(snapshot?: TeamsSnapshot) {
  const team = snapshot?.team;
  const scope = team ? JSON.stringify([snapshot?.uid, team.id, team.generation, snapshot?.selectedConversationId]) : '';
  const [route, setRoute] = useState<Route>();
  const nextId = useRef(0);
  const active = !!scope && route?.scope === scope ? route : undefined;
  useEffect(() => { if (route && !active) setRoute(current => current === route ? undefined : current); }, [route, active]);
  const open = useCallback((section: TeamSettingsSection = 'general', trigger?: HTMLElement) => {
    if (!scope) return;
    const opener = trigger ?? (document.activeElement instanceof HTMLElement ? document.activeElement : undefined);
    setRoute({ id: ++nextId.current, scope, section, opener, openedFromHeader: opener?.dataset?.teamSettingsOpener !== undefined });
  }, [scope]);
  const dismiss = useCallback(() => setRoute(undefined), []);
  const back = useCallback(() => {
    dismiss();
    window.requestAnimationFrame(() => {
      const opener = active?.opener;
      const target = opener?.isConnected ? opener
        : active?.openedFromHeader ? document.querySelector<HTMLElement>('[data-team-settings-opener]') : undefined;
      target?.focus({ preventScroll: true });
    });
  }, [active, dismiss]);
  return { active, open, dismiss, back };
}

/** Preserve the conversation layout, scroll position and draft while the settings page covers it. */
export function TeamSettingsSurface({ settings, children }: { settings?: ReactNode; children: ReactNode }) {
  const topbar = useContext(ChatTopbarTarget);
  const covered = !!settings;
  return <div className="tw-settings-surface">
    <div className="tw-settings-underlay" data-settings-covered={covered} {...(covered ? { inert: '' } : {})}>
      <ChatTopbarTarget.Provider value={covered ? null : topbar}>{children}</ChatTopbarTarget.Provider>
    </div>
    {settings}
  </div>;
}
