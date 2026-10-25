import { useEffect, useRef } from 'react';
import { Icon } from '../../Icon';
import { Toggle } from '../primitives';

function PermissionSelect({ label }: { label: string }) {
  return (
    <select aria-label={label} disabled className="computer-browser-select" defaultValue="ask">
      <option value="ask">Requires approval</option>
    </select>
  );
}

/** Browser settings remain read-only until the host exposes these capabilities. */
export function GoogleChromeSettings({ onBack }: { onBack(): void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    heading.current?.closest<HTMLElement>('.settings-content-scroll')?.scrollTo(0, 0);
    heading.current?.focus({ preventScroll: true });
  }, []);

  return (
    <div className="computer-browser-settings">
      <nav className="computer-browser-breadcrumb" aria-label="Computer use navigation">
        <button type="button" onClick={onBack}>Computer use</button>
        <Icon name="chevronR" size={15} />
        <span aria-current="page">Google Chrome</span>
      </nav>

      <header className="computer-browser-heading computer-use-page-heading">
        <div>
          <h1 tabIndex={-1} ref={heading}>Google Chrome</h1>
          <p>Browser extension status unavailable</p>
        </div>
        <div className="computer-browser-toolbar" aria-label="Browser extension actions">
          <button type="button" className="computer-use-manage" disabled>Retry</button>
          <button type="button" className="computer-use-manage" disabled>Reinstall extension</button>
          <button type="button" className="computer-use-manage computer-browser-remove" disabled>Remove extension</button>
        </div>
      </header>
      <p className="computer-browser-status">Browser extension management and permission settings are not available in this version.</p>

      <section aria-labelledby="computer-browser-permissions-heading">
        <h2 id="computer-browser-permissions-heading">Browser permissions</h2>
        <div className="computer-use-settings-card">
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-use-row-title">History</div>
              <div className="computer-use-row-description">Choose if LingXi asks for approval before accessing your browser’s history</div>
            </div>
            <select aria-label="History access" disabled className="computer-browser-select" defaultValue="ask">
              <option value="ask">Always ask</option>
            </select>
          </div>
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-use-row-title">Use site tools by default</div>
              <div className="computer-use-row-description">Allow LingXi to discover and call tools provided by websites, including WebMCP, unless you set a different permission for a site</div>
            </div>
            <Toggle value={false} disabled label="Use site tools by default — unavailable" onChange={() => {}} />
          </div>
        </div>
      </section>

      <section aria-labelledby="computer-website-permissions-heading">
        <div className="computer-browser-section-heading">
          <div>
            <h2 id="computer-website-permissions-heading">Website permissions</h2>
            <p>Choose default permissions and add exceptions for specific websites</p>
          </div>
          <button type="button" className="computer-use-manage computer-browser-add" disabled><Icon name="plus" size={16} />Add</button>
        </div>
        <div className="computer-browser-table-scroll" role="region" aria-label="Website permissions" tabIndex={0}>
          <table className="computer-browser-table">
            <thead><tr><th scope="col">Site or pattern</th><th scope="col">Browsing</th><th scope="col">Downloads</th></tr></thead>
            <tbody>
              <tr>
                <th scope="row">Default</th>
                <td><PermissionSelect label="Default browsing permission" /></td>
                <td><PermissionSelect label="Default download permission" /></td>
              </tr>
            </tbody>
          </table>
        </div>
        <p className="computer-browser-table-note">No custom website permissions. Only sites with custom permissions appear here.</p>
      </section>

      <section aria-labelledby="computer-browser-developer-heading">
        <h2 id="computer-browser-developer-heading">Developer mode</h2>
        <div className="computer-use-settings-card">
          <div className="computer-use-control-row">
            <div className="computer-use-row-copy">
              <div className="computer-browser-risk"><Icon name="shieldAlert" size={16} />Elevated risk</div>
              <div className="computer-use-row-title">Enable full CDP access</div>
              <div className="computer-use-row-description">Allow LingXi to use full Chrome DevTools Protocol (CDP) access in connected Browser Use sessions. Full CDP access lets LingXi inspect and control sensitive browser internals that may put your data at risk.</div>
            </div>
            <Toggle value={false} disabled label="Enable full CDP access — unavailable" onChange={() => {}} />
          </div>
        </div>
      </section>
    </div>
  );
}
