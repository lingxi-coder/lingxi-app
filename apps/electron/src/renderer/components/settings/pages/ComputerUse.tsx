import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { useT } from '../../../theme/ThemeContext';
import { Icon } from '../../Icon';
import { Toggle } from '../primitives';
import { GoogleChromeSettings } from './ComputerUseBrowser';

function ApplicationIcon({ bundleId, fallback }: { bundleId: string; fallback: ReactNode }) {
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    let active = true;
    void window.lingxi?.applicationIcon?.get(bundleId).then((icon) => {
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

/** Presentation only until the host exposes persistent computer-access settings.
 * Never turn a local preference or a generic tool rule into an app grant.
 */
export function ComputerUse() {
  const t = useT();
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
      {browserOpen ? <GoogleChromeSettings onBack={() => {
        returningFromBrowser.current = true;
        setBrowserOpen(false);
      }} /> : <>
      <header className="computer-use-page-heading">
        <h1>Computer use</h1>
        <p>Manage how LingXi uses other applications on your computer</p>
      </header>

      <section aria-labelledby="computer-use-control-heading">
        <h2 id="computer-use-control-heading">Control</h2>
        <div className="computer-use-settings-card">
          <ControlRow
            icon={<span className="computer-use-any-app"><svg width="30" height="30" viewBox="0 0 32 32" fill="none"><path d="m7 5 19 9-9 3-3 10L7 5Z" fill="white" fillOpacity=".8" stroke="white" strokeWidth="1.5" strokeLinejoin="round" /></svg></span>}
            title="Any App"
            description="App access is requested in each conversation. A global control switch is not available yet."
          >
            <Toggle value={false} disabled label="Any App — global control unavailable" onChange={() => {}} />
          </ControlRow>
          <ControlRow
            icon={<ApplicationIcon bundleId="com.google.Chrome" fallback={<svg width="44" height="44" viewBox="0 0 48 48"><circle cx="24" cy="24" r="24" fill="#ea4335"/><path d="M24 24 12 44.8A24 24 0 0 1 3.2 12h20.8Z" fill="#34a853"/><path d="M24 12h20.8A24 24 0 0 1 12 44.8L24 24Z" fill="#fbbc05"/><circle cx="24" cy="24" r="10" fill="#4285f4" stroke="white" strokeWidth="2"/></svg>} />}
            title="Google Chrome"
            description="Browser extension integration is not available yet"
          >
            <button className="computer-use-manage" type="button" ref={manageRef} onClick={() => setBrowserOpen(true)} aria-label="Manage Google Chrome">Manage</button>
          </ControlRow>
          <ControlRow
            icon={<ApplicationIcon bundleId="com.microsoft.Excel" fallback={<span className="computer-use-excel">X</span>} />}
            title="Microsoft Excel"
            description="Excel add-in control is not available yet"
          >
            <Toggle value={false} disabled label="Microsoft Excel — add-in unavailable" onChange={() => {}} />
          </ControlRow>
        </div>
        <div className="computer-use-settings-card computer-use-locked-card">
          <ControlRow
            icon={<span className="computer-use-locked-icon"><svg width="36" height="36" viewBox="0 0 36 36" fill="none"><rect x="5" y="5" width="26" height="20" rx="2" fill="#68aee0" stroke="currentColor" strokeWidth="1.5"/><path d="M2 27h32" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/></svg><Icon name="lock" size={19} /></span>}
            title="Locked use"
            description={<>Control while your Mac is locked is not available. <a href="#computer-use-locked-help" onClick={() => setHelpOpen(true)}>Learn more</a></>}
          >
            <Toggle value={false} disabled label="Locked use — unavailable" onChange={() => {}} />
          </ControlRow>
        </div>
      </section>

      <section aria-labelledby="computer-use-always-heading">
        <h2 id="computer-use-always-heading">Always-allowed apps</h2>
        <div className="computer-use-settings-card computer-use-empty">
          <span className="computer-use-empty-icon" aria-hidden="true"><Icon name="shieldCheck" size={25} /></span>
          <div>
            <p>No always-allowed apps</p>
            <span>App permissions currently apply to the conversation. Permanent app access is not available yet.</span>
          </div>
        </div>
      </section>

      <details id="computer-use-locked-help" className="computer-use-help" open={helpOpen} onToggle={(event) => setHelpOpen(event.currentTarget.open)}>
        <summary>About computer access</summary>
        <p>LingXi asks before granting access to an app. You can review the requested apps, access level, clipboard access and system shortcuts in the conversation.</p>
        <p>Locked use and permanent app permissions are not supported in this version. The controls above will become available when their integrations are connected.</p>
      </details>
      </>}
    </article>
  );
}
