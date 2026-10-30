import { useI18n } from '../../../i18n/useI18n';
import { desktopHost } from '../../../bridge/bridgeConnection';
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import { Toggle } from '../primitives';
import { GoogleChromeSettings } from './ComputerUseBrowser';
import { useComputerSettings, type ComputerSettingsController } from './useComputerSettings';
import type { ComputerApplicationPermission } from '../../../../shared/computerSettings';

function ApplicationIcon({ bundleId, fallback }: { bundleId: string; fallback: ReactNode }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void desktopHost()?.applicationIcon?.get(bundleId).then((icon) => {
      if (active && icon?.startsWith('data:image/png;base64,')) setUrl(icon);
    }).catch(() => undefined);
    return () => { active = false; };
  }, [bundleId]);
  return url
    ? <img src={url} alt="" width={44} height={44} draggable={false} onError={() => setUrl(null)} />
    : <>{fallback}</>;
}

function ControlRow({ icon, title, description, children }: {
  icon: ReactNode; title: string; description: ReactNode; children: ReactNode;
}) {
  return (
    <div className="computer-use-control-row">
      <span className="computer-use-app-icon" aria-hidden="true">{icon}</span>
      <div className="computer-use-row-copy">
        <div className="computer-use-row-title">{title}</div>
        <div className="computer-use-row-description">{description}</div>
      </div>
      <div className="computer-use-row-action">{children}</div>
    </div>
  );
}

/** Device settings are owned by the host and enforced by active runtimes. */
export function ComputerUse() {
  const { tr } = useI18n();
  const t = useT();
  const settings = useComputerSettings();
  const { state, pending, error } = settings;
  const [helpOpen, setHelpOpen] = useState(false);
  const [browserOpen, setBrowserOpen] = useState(false);
  const manageRef = useRef<HTMLButtonElement>(null);
  const returningFromBrowser = useRef(false);
  useEffect(() => {
    if (!browserOpen && returningFromBrowser.current) {
      manageRef.current?.focus();
      returningFromBrowser.current = false;
    }
  }, [browserOpen]);
  return (
    <article className="computer-use-settings" style={{
      '--computer-settings-text': t.text,
      '--computer-settings-muted': t.text3,
      '--computer-settings-border': t.border,
      '--computer-settings-surface': t.windowBg,
      '--computer-settings-hover': t.surfaceHover,
      '--computer-settings-accent': t.accent,
      '--computer-settings-risk': t.dark ? '#ff9f66' : '#c74300',
      '--computer-settings-danger': t.danger,
    } as CSSProperties}>
      <div className="computer-settings-feedback" aria-live="polite" aria-atomic="true">
        {error ? <p role="alert" className="computer-settings-error">{error} <button type="button" onClick={() => void settings.refresh()} disabled={pending}>{tr('desktop_computer_reload')}</button></p>
          : pending ? <p role="status">{tr('desktop_computer_saving')}</p> : !state ? <p role="status">{tr('desktop_computer_loading')}</p> : null}
      </div>
      {browserOpen ? <GoogleChromeSettings settings={settings} onBack={() => {
        returningFromBrowser.current = true;
        setBrowserOpen(false);
      }} /> : <>
      <header className="computer-use-page-heading">
        <h1>{tr('desktop_computer_title')}</h1>
        <p>{tr('desktop_computer_description')}</p>
      </header>

      <section aria-labelledby="computer-use-control-heading">
        <h2 id="computer-use-control-heading">{tr('desktop_computer_control')}</h2>
        <div className="computer-use-settings-card">
          <ControlRow
            icon={<span className="computer-use-any-app"><svg width="30" height="30" viewBox="0 0 32 32" fill="none"><path d="m7 5 19 9-9 3-3 10L7 5Z" fill="white" fillOpacity=".8" stroke="white" strokeWidth="1.5" strokeLinejoin="round" /></svg></span>}
            title={tr('desktop_computer_any_app')}
            description={tr('desktop_computer_any_app_description')}
          >
            <Toggle value={state?.enabled ?? false} disabled={!state?.capabilities.computer || pending} label={tr('desktop_computer_enable')} onChange={(enabled) => { if (state) void settings.execute({ action: 'setEnabled', revision: state.revision, enabled }); }} />
          </ControlRow>
          <ControlRow
            icon={<ApplicationIcon bundleId="com.google.Chrome" fallback={<svg width="44" height="44" viewBox="0 0 48 48"><circle cx="24" cy="24" r="24" fill="#ea4335"/><path d="M24 24 12 44.8A24 24 0 0 1 3.2 12h20.8Z" fill="#34a853"/><path d="M24 12h20.8A24 24 0 0 1 12 44.8L24 24Z" fill="#fbbc05"/><circle cx="24" cy="24" r="10" fill="#4285f4" stroke="white" strokeWidth="2"/></svg>} />}
            title="Google Chrome"
            description={state?.browserStatus.connected ? tr('desktop_computer_browser_connected') : state?.browserStatus.installed ? tr('desktop_computer_browser_waiting_chrome') : tr('desktop_computer_browser_setup')}
          >
            <button className="computer-use-manage" type="button" ref={manageRef} onClick={() => setBrowserOpen(true)} aria-label={tr('desktop_computer_manage_chrome')}>{tr('desktop_computer_manage')}</button>
          </ControlRow>
          <ControlRow
            icon={<ApplicationIcon bundleId="com.microsoft.Excel" fallback={<span className="computer-use-excel">X</span>} />}
            title="Microsoft Excel"
            description={tr('desktop_computer_excel_unavailable')}
          >
            <Toggle value={false} disabled label={tr('desktop_computer_excel_unavailable_label')} onChange={() => {}} />
          </ControlRow>
        </div>
        <div className="computer-use-settings-card computer-use-locked-card">
          <ControlRow
            icon={<span className="computer-use-locked-icon"><svg width="36" height="36" viewBox="0 0 36 36" fill="none"><rect x="5" y="5" width="26" height="20" rx="2" fill="#68aee0" stroke="currentColor" strokeWidth="1.5"/><path d="M2 27h32" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/></svg><Icon name="lock" size={19} /></span>}
            title={tr('desktop_computer_locked')}
            description={<>{tr('desktop_computer_locked_description')} <a href="#computer-use-locked-help" onClick={() => setHelpOpen(true)}>{tr('desktop_computer_learn_more')}</a></>}
          >
            <Toggle value={false} disabled label={tr('desktop_computer_locked_unavailable')} onChange={() => {}} />
          </ControlRow>
        </div>
      </section>

      <section aria-labelledby="computer-use-always-heading">
        <h2 id="computer-use-always-heading">{tr('desktop_computer_always_apps')}</h2>
        <div className="computer-use-settings-card">
          {state?.apps.filter((app) => app.permission === 'allow').map((app) => (
            <ControlRow key={app.bundleId}
              icon={<ApplicationIcon bundleId={app.bundleId} fallback={<Icon name="shield" size={25} />} />}
              title={app.displayName} description={app.bundleId}
            >
              <button type="button" className="computer-settings-revoke" aria-label={tr('desktop_computer_revoke_app', app.displayName)} disabled={!state.capabilities.computer || pending}
                onClick={() => void settings.execute({ action: 'removeApplication', revision: state.revision, bundleId: app.bundleId })}>
                <Icon name="trash" size={18} />
              </button>
            </ControlRow>
          ))}
          {!state?.apps.some((app) => app.permission === 'allow') ? <div className="computer-use-empty">
            <span className="computer-use-empty-icon" aria-hidden="true"><Icon name="shieldCheck" size={25} /></span>
            <div><p>{tr('desktop_computer_no_always_apps')}</p><span>{tr('desktop_computer_app_ask_description')}</span></div>
          </div> : null}
        </div>
        <p className="computer-browser-table-note">{tr('desktop_computer_revoke_description')}</p>
        <ApplicationPermissionForm settings={settings} />
      </section>

      {state?.apps.some((app) => app.permission !== 'allow') ? <section aria-labelledby="computer-use-other-heading">
        <h2 id="computer-use-other-heading">{tr('desktop_computer_other_apps')}</h2>
        <div className="computer-use-settings-card">
          {state.apps.filter((app) => app.permission !== 'allow').map((app) => <ApplicationPermissionRow key={app.bundleId} app={app} settings={settings} />)}
        </div>
      </section> : null}

      <details id="computer-use-locked-help" className="computer-use-help" open={helpOpen} onToggle={(event) => setHelpOpen(event.currentTarget.open)}>
        <summary>{tr('desktop_computer_about')}</summary>
        <p>{tr('desktop_computer_about_description')}</p>
        <p>{tr('desktop_computer_about_revocation')}</p>
        <p>{tr('desktop_computer_about_unavailable')}</p>
      </details>
      </>}
    </article>
  );
}

function ApplicationPermissionForm({ settings }: { settings: ComputerSettingsController }) {
  const { tr } = useI18n();
  const [bundleId, setBundleId] = useState('');
  const { state, pending } = settings;
  return <form className="computer-settings-add-form" onSubmit={(event) => {
    event.preventDefault();
    if (!state || !bundleId.trim()) return;
    void settings.execute({ action: 'setApplication', revision: state.revision, bundleId: bundleId.trim(), permission: 'allow' }).then((saved) => { if (saved) setBundleId(''); });
  }}>
    <label htmlFor="computer-app-bundle-id">{tr('desktop_computer_add_installed_app')}</label>
    <div className="computer-settings-form-controls">
      <input id="computer-app-bundle-id" value={bundleId} onChange={(event) => setBundleId(event.target.value)} placeholder="com.example.app" autoCapitalize="none" autoCorrect="off" spellCheck={false} maxLength={255} required disabled={!state?.capabilities.computer || pending} />
      <button type="submit" className="computer-use-manage" disabled={!state?.capabilities.computer || pending || !bundleId.trim()}><Icon name="plus" size={16} />{tr('desktop_computer_add_app')}</button>
    </div>
    <p>{tr('desktop_computer_bundle_description')}</p>
  </form>;
}

function ApplicationPermissionRow({ app, settings }: { app: ComputerApplicationPermission; settings: ComputerSettingsController }) {
  const { tr } = useI18n();
  const { state, pending } = settings;
  return <ControlRow icon={<ApplicationIcon bundleId={app.bundleId} fallback={<Icon name="shield" size={25} />} />} title={app.displayName}
    description={<>{app.permission === 'deny' ? tr('desktop_computer_blocked') : tr('desktop_computer_ask')} · {app.bundleId}</>}>
    <select className="computer-browser-select" aria-label={tr('desktop_computer_app_permission', app.displayName)} value={app.permission} disabled={!state?.capabilities.computer || pending}
      onChange={(event) => { if (state) void settings.execute({ action: 'setApplication', revision: state.revision, bundleId: app.bundleId, permission: event.target.value as ComputerApplicationPermission['permission'] }); }}>
      <option value="ask">{tr('desktop_computer_ask')}</option><option value="allow">{tr('desktop_computer_allow')}</option><option value="deny">{tr('desktop_computer_deny')}</option>
    </select>
  </ControlRow>;
}
