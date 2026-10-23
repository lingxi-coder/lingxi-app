import type { Locale } from '../utils/locale';
import type { PinnedHost } from './deviceKey';
import { t } from './strings';
import type { HostsState } from './useRemote';

interface HostListProps {
  locale: Locale;
  hosts: HostsState;
  pins: PinnedHost[];
  onConnect(hostId: string): void;
  onForget(hostId: string): void;
}

const PLATFORMS: Record<string, string> = { darwin: 'macOS', win32: 'Windows', linux: 'Linux' };

export function HostList({ locale, hosts, pins, onConnect, onForget }: HostListProps) {
  const pinned = new Set(pins.map((pin) => pin.host_id));
  return (
    <section className="rc-panel">
      <h2>{t(locale, 'computers')}</h2>
      <p>{t(locale, 'computersIntro')}</p>
      {hosts.status === 'loading' ? <p className="rc-note" role="status">{t(locale, 'loading')}</p> : null}
      {hosts.status === 'error' ? <p className="rc-note rc-danger" role="alert">{t(locale, 'hostsError')}</p> : null}
      {hosts.status === 'ready' && hosts.hosts.length === 0 ? (
        <div className="rc-empty">
          <h3>{t(locale, 'noComputers')}</h3>
          <p>{t(locale, 'noComputersHelp')}</p>
        </div>
      ) : null}
      <ul className="rc-list">
        {hosts.hosts.map((host) => (
          <li key={host.hostId} className="rc-row">
            <div className="rc-row-main">
              <strong>{host.name}</strong>
              <span className="rc-row-sub">
                {PLATFORMS[host.platform] ?? host.platform}
                {' · '}
                <span className={host.online ? 'rc-online' : 'rc-offline'}>{t(locale, host.online ? 'online' : 'offline')}</span>
                {' · '}
                {t(locale, pinned.has(host.hostId) ? 'paired' : 'notPaired')}
              </span>
              {host.online ? null : <span className="rc-note">{t(locale, 'offlineHelp')}</span>}
            </div>
            <div className="rc-row-actions">
              <button type="button" className={`button ${host.online ? 'primary' : 'secondary'}`} onClick={() => onConnect(host.hostId)}>
                {t(locale, host.online ? 'connect' : 'tryAnyway')}
              </button>
              {pinned.has(host.hostId) ? (
                <button type="button" className="rc-link" title={t(locale, 'forgetHelp')} onClick={() => onForget(host.hostId)}>
                  {t(locale, 'forget')}
                </button>
              ) : null}
            </div>
          </li>
        ))}
      </ul>
    </section>
  );
}
