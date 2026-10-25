import { useI18n } from '../../../i18n/useI18n';
import { useEffect, useRef } from 'react';
import { Icon } from '../../Icon';
import { Toggle } from '../primitives';

function PermissionSelect({ label }: { label: string }) {
  const { tr } = useI18n();
  return (
    <select aria-label={label} disabled className="computer-browser-select" defaultValue="ask">
      <option value="ask">{tr('desktop_computer_ask')}</option>
    </select>
  );
}

/** Browser settings remain read-only until the host exposes these capabilities. */
export function GoogleChromeSettings({ onBack }: { onBack(): void }) {
  const { tr } = useI18n();
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
          <p>{tr('desktop_computer_readonly_browser_status')}</p>
        </div>
        <div className="computer-browser-toolbar" aria-label={tr('desktop_computer_browser_actions')}>
          <button type="button" className="computer-use-manage" disabled>{tr('common_retry')}</button>
          <button type="button" className="computer-use-manage" disabled>{tr('desktop_computer_reinstall_extension')}</button>
          <button type="button" className="computer-use-manage computer-browser-remove" disabled>{tr('desktop_computer_remove_extension')}</button>
        </div>
      </header>
      <p className="computer-browser-status">{tr('desktop_computer_readonly_browser_settings')}</p>

      <section aria-labelledby="computer-browser-permissions-heading">
        <h2 id="computer-browser-permissions-heading">{tr('desktop_computer_browser_permissions')}</h2>
        <div className="computer-use-settings-card">
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-use-row-title">{tr('desktop_computer_history')}</div>
              <div className="computer-use-row-description">{tr('desktop_computer_history_description')}</div>
            </div>
            <select aria-label={tr('desktop_computer_history_access')} disabled className="computer-browser-select" defaultValue="ask">
              <option value="ask">{tr('desktop_computer_always_ask')}</option>
            </select>
          </div>
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-use-row-title">{tr('desktop_computer_site_tools')}</div>
              <div className="computer-use-row-description">{tr('desktop_computer_readonly_site_tools_description')}</div>
            </div>
            <Toggle value={false} disabled label={tr('desktop_computer_readonly_site_tools_label')} onChange={() => {}} />
          </div>
        </div>
      </section>

      <section aria-labelledby="computer-website-permissions-heading">
        <div className="computer-browser-section-heading">
          <div>
            <h2 id="computer-website-permissions-heading">{tr('desktop_computer_website_permissions')}</h2>
            <p>{tr('desktop_computer_readonly_website_description')}</p>
          </div>
          <button type="button" className="computer-use-manage computer-browser-add" disabled><Icon name="plus" size={16} />{tr('desktop_computer_add')}</button>
        </div>
        <div className="computer-browser-table-scroll" role="region" aria-label={tr('desktop_computer_website_permissions')} tabIndex={0}>
          <table className="computer-browser-table">
            <thead><tr><th scope="col">{tr('desktop_computer_site_or_pattern')}</th><th scope="col">{tr('desktop_computer_browsing')}</th><th scope="col">{tr('desktop_computer_downloads')}</th></tr></thead>
            <tbody>
              <tr>
                <th scope="row">{tr('desktop_computer_default')}</th>
                <td><PermissionSelect label={tr('desktop_computer_default_browsing')} /></td>
                <td><PermissionSelect label={tr('desktop_computer_default_download')} /></td>
              </tr>
            </tbody>
          </table>
        </div>
        <p className="computer-browser-table-note">{tr('desktop_computer_readonly_website_note')}</p>
      </section>

      <section aria-labelledby="computer-browser-developer-heading">
        <h2 id="computer-browser-developer-heading">{tr('desktop_computer_developer')}</h2>
        <div className="computer-use-settings-card">
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-browser-risk"><Icon name="shieldAlert" size={16} />{tr('desktop_computer_risk_lower')}</div>
              <div className="computer-use-row-title">{tr('desktop_computer_full_cdp')}</div>
              <div className="computer-use-row-description">{tr('desktop_computer_readonly_full_cdp_description')}</div>
            </div>
            <Toggle value={false} disabled label={tr('desktop_computer_readonly_full_cdp_label')} onChange={() => {}} />
          </div>
        </div>
      </section>
    </div>
  );
}
