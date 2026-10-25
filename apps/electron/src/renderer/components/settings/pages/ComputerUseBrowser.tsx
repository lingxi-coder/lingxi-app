import { useI18n } from '../../../i18n/useI18n';
import { useEffect, useRef, useState } from 'react';
import { Icon } from '../../Icon';
import { Toggle } from '../primitives';
import type { ComputerPermission, BrowserSitePermission } from '../../../../shared/computerSettings';
import type { ComputerSettingsController } from './useComputerSettings';

function PermissionSelect({ label, value, disabled, onChange, history = false }: {
  label: string; value: ComputerPermission; disabled: boolean; onChange(value: ComputerPermission): void; history?: boolean;
}) {
  const { tr } = useI18n();
  return <select aria-label={label} disabled={disabled} className="computer-browser-select" value={value} onChange={(event) => onChange(event.target.value as ComputerPermission)}>
    <option value="ask">{history ? tr('desktop_computer_always_ask') : tr('desktop_computer_ask')}</option>
    <option value="allow">{tr('desktop_computer_allow')}</option>
    <option value="deny">{tr('desktop_computer_deny')}</option>
  </select>;
}

export function GoogleChromeSettings({ onBack, settings }: { onBack(): void; settings: ComputerSettingsController }) {
  const { tr } = useI18n();
  const { state, pending } = settings;
  const browser = state?.browser;
  const status = state?.browserStatus;
  const unavailable = !state?.capabilities.browser || pending;
  const [addingSite, setAddingSite] = useState(false);
  const addButton = useRef<HTMLButtonElement>(null);
  const restoreAddFocus = useRef(false);
  useEffect(() => {
    if (!addingSite && !pending && restoreAddFocus.current) {
      restoreAddFocus.current = false;
      addButton.current?.focus();
    }
  }, [addingSite, pending]);
  const update = (key: 'history' | 'siteTools' | 'fullCdp' | 'defaultBrowsing' | 'defaultDownloads', value: ComputerPermission | boolean) => {
    if (state) void settings.execute({ action: 'setBrowser', revision: state.revision, key, value });
  };
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    heading.current?.closest<HTMLElement>('.settings-content-scroll')?.scrollTo(0, 0);
    heading.current?.focus({ preventScroll: true });
  }, []);

  return (
    <div className="computer-browser-settings">
      <nav className="computer-browser-breadcrumb" aria-label={tr('desktop_computer_navigation')}>
        <button type="button" onClick={onBack}>{tr('desktop_computer_title')}</button>
        <Icon name="chevronR" size={15} />
        <span aria-current="page">Google Chrome</span>
      </nav>

      <header className="computer-browser-heading computer-use-page-heading">
        <div>
          <h1 tabIndex={-1} ref={heading}>Google Chrome</h1>
          <p>{status?.connected ? tr('desktop_computer_browser_connected') : status?.installed ? tr('desktop_computer_browser_waiting') : tr('desktop_computer_browser_not_setup')}</p>
        </div>
        <div className="computer-browser-toolbar" aria-label={tr('desktop_computer_browser_actions')}>
          <button type="button" className="computer-use-manage" disabled={unavailable} onClick={() => void settings.execute({ action: 'retryBrowser' })}>{tr('common_retry')}</button>
          <button type="button" className="computer-use-manage" disabled={unavailable} onClick={() => void settings.execute({ action: 'installBrowser' })}>{status?.installed ? tr('desktop_computer_repair_connection') : tr('desktop_computer_setup_extension')}</button>
          <button type="button" className="computer-use-manage computer-browser-remove" disabled={unavailable || !status?.installed} onClick={() => void settings.execute({ action: 'removeBrowser' })}>{tr('desktop_computer_disconnect_extension')}</button>
        </div>
      </header>
      <div className="computer-browser-status">
        {status?.error ? <p role="status">{status.error}</p> : null}
        <p>{status?.connected ? tr('desktop_computer_browser_connected_description') : status?.installed ? tr('desktop_computer_browser_setup_steps') : tr('desktop_computer_browser_setup_description')}</p>
        {status?.installed ? <button type="button" className="computer-use-manage" disabled={unavailable} onClick={() => void settings.execute({ action: 'showExtension' })}>{tr('desktop_computer_show_extension')}</button> : null}
        {state && !state.enabled ? <p>{tr('desktop_computer_browser_disabled_description')}</p> : null}
      </div>

      <section aria-labelledby="computer-browser-permissions-heading">
        <h2 id="computer-browser-permissions-heading">{tr('desktop_computer_browser_permissions')}</h2>
        <div className="computer-use-settings-card">
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-use-row-title">{tr('desktop_computer_history')}</div>
              <div className="computer-use-row-description">{tr('desktop_computer_history_description')}</div>
            </div>
            <PermissionSelect label={tr('desktop_computer_history_access')} value={browser?.history ?? 'ask'} disabled={unavailable} history onChange={(value) => update('history', value)} />
          </div>
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-use-row-title">{tr('desktop_computer_site_tools')}</div>
              <div className="computer-use-row-description">{tr('desktop_computer_site_tools_description')}</div>
            </div>
            <Toggle value={browser?.siteTools ?? false} disabled={unavailable} label={tr('desktop_computer_site_tools')} onChange={(value) => update('siteTools', value)} />
          </div>
        </div>
      </section>

      <section aria-labelledby="computer-website-permissions-heading">
        <div className="computer-browser-section-heading">
          <div>
            <h2 id="computer-website-permissions-heading">{tr('desktop_computer_website_permissions')}</h2>
            <p>{tr('desktop_computer_website_description')}</p>
          </div>
          <button type="button" className="computer-use-manage computer-browser-add" ref={addButton} disabled={unavailable} aria-expanded={addingSite} aria-controls="computer-browser-add-site" onClick={() => setAddingSite((open) => !open)}><Icon name="plus" size={16} />{tr('desktop_computer_add')}</button>
        </div>
        {addingSite ? <SitePermissionForm settings={settings} onClose={() => { restoreAddFocus.current = true; setAddingSite(false); }} /> : null}
        <div className="computer-browser-table-scroll" role="region" aria-label={tr('desktop_computer_website_permissions')} tabIndex={0}>
          <table className="computer-browser-table">
            <thead><tr><th scope="col">{tr('desktop_computer_origin')}</th><th scope="col">{tr('desktop_computer_browsing')}</th><th scope="col">{tr('desktop_computer_downloads')}</th><th scope="col"><span className="computer-settings-sr-only">{tr('desktop_computer_actions')}</span></th></tr></thead>
            <tbody>
              <tr>
                <th scope="row">{tr('desktop_computer_default')}</th>
                <td><PermissionSelect label={tr('desktop_computer_default_browsing')} value={browser?.defaultBrowsing ?? 'ask'} disabled={unavailable} onChange={(value) => update('defaultBrowsing', value)} /></td>
                <td><PermissionSelect label={tr('desktop_computer_default_download')} value={browser?.defaultDownloads ?? 'ask'} disabled={unavailable} onChange={(value) => update('defaultDownloads', value)} /></td><td />
              </tr>
              {browser?.sites.map((site) => <SitePermissionRow key={site.origin} site={site} settings={settings} />)}
            </tbody>
          </table>
        </div>
        <p className="computer-browser-table-note">{tr('desktop_computer_website_note')}</p>
        <p className="computer-browser-table-note">{tr('desktop_computer_downloads_scope')}</p>
      </section>

      <section aria-labelledby="computer-browser-developer-heading">
        <h2 id="computer-browser-developer-heading">{tr('desktop_computer_developer')}</h2>
        <div className="computer-use-settings-card">
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-browser-risk"><Icon name="shieldAlert" size={16} />{tr('desktop_computer_risk')}</div>
              <div className="computer-use-row-title">{tr('desktop_computer_full_cdp')}</div>
              <div className="computer-use-row-description">{tr('desktop_computer_full_cdp_description')}</div>
            </div>
            <Toggle value={browser?.fullCdp ?? false} disabled={unavailable} label={tr('desktop_computer_full_cdp')} onChange={(value) => update('fullCdp', value)} />
          </div>
        </div>
      </section>
    </div>
  );
}

function SitePermissionRow({ site, settings }: { site: BrowserSitePermission; settings: ComputerSettingsController }) {
  const { tr } = useI18n();
  const { state, pending } = settings;
  const save = (key: 'browsing' | 'downloads', value: ComputerPermission) => {
    if (state) void settings.execute({ action: 'setSite', revision: state.revision, ...site, [key]: value });
  };
  return <tr>
    <th scope="row">{site.origin}</th>
    <td><PermissionSelect label={tr('desktop_computer_site_browsing', site.origin)} value={site.browsing} disabled={!state?.capabilities.browser || pending} onChange={(value) => save('browsing', value)} /></td>
    <td><PermissionSelect label={tr('desktop_computer_site_download', site.origin)} value={site.downloads} disabled={!state?.capabilities.browser || pending} onChange={(value) => save('downloads', value)} /></td>
    <td><button type="button" className="computer-settings-revoke" aria-label={tr('desktop_computer_remove_site', site.origin)} disabled={!state?.capabilities.browser || pending} onClick={() => { if (state) void settings.execute({ action: 'removeSite', revision: state.revision, origin: site.origin }); }}><Icon name="trash" size={17} /></button></td>
  </tr>;
}

function SitePermissionForm({ settings, onClose }: { settings: ComputerSettingsController; onClose(): void }) {
  const { tr } = useI18n();
  const { state, pending } = settings;
  const unavailable = !state?.capabilities.browser || pending;
  const [origin, setOrigin] = useState('');
  const [browsing, setBrowsing] = useState<ComputerPermission>('ask');
  const [downloads, setDownloads] = useState<ComputerPermission>('ask');
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => { input.current?.focus(); }, []);
  return <form id="computer-browser-add-site" className="computer-settings-add-form computer-browser-site-form" onSubmit={(event) => {
    event.preventDefault();
    if (state) void settings.execute({ action: 'setSite', revision: state.revision, origin: origin.trim(), browsing, downloads }).then((saved) => { if (saved) onClose(); });
  }} onKeyDown={(event) => { if (event.key === 'Escape' && !event.nativeEvent.isComposing && !pending) { event.preventDefault(); event.stopPropagation(); onClose(); } }}>
    <label htmlFor="computer-browser-site-origin">{tr('desktop_computer_origin')}</label>
    <input id="computer-browser-site-origin" ref={input} type="url" value={origin} onChange={(event) => setOrigin(event.target.value)} placeholder="https://example.com" required disabled={unavailable} maxLength={2048} spellCheck={false} autoCorrect="off" autoCapitalize="none" />
    <p>{tr('desktop_computer_origin_description')}</p>
    <div className="computer-settings-form-controls">
      <label>{tr('desktop_computer_browsing')} <PermissionSelect label={tr('desktop_computer_new_browsing')} value={browsing} disabled={unavailable} onChange={setBrowsing} /></label>
      <label>{tr('desktop_computer_downloads')} <PermissionSelect label={tr('desktop_computer_new_download')} value={downloads} disabled={unavailable} onChange={setDownloads} /></label>
    </div>
    <div className="computer-settings-form-controls computer-settings-form-actions">
      <button type="button" className="computer-use-manage" disabled={pending} onClick={onClose}>{tr('common_cancel')}</button>
      <button type="submit" className="computer-use-manage" disabled={unavailable || !origin.trim()}>{tr('desktop_computer_save_site')}</button>
    </div>
  </form>;
}
