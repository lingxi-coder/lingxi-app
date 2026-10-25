import { useEffect, useState } from 'react';
import { navigateTo, parseRemoteRoute, remoteSessionRoute, type AppRoute } from '../app/router';
import type { Theme } from '../app/useTheme';
import type { Locale } from '../utils/locale';
import { ConnectionScreen } from '../remote/ConnectionScreen';
import { HostList } from '../remote/HostList';
import { SessionList } from '../remote/SessionList';
import { SessionView } from '../remote/SessionView';
import { t } from '../remote/strings';
import { browserDeviceName } from '../remote/deviceKey';
import {
  getDeviceKeys,
  signOutUser,
  supportsRemoteCrypto,
  useAuthState,
  useConnectionState,
  useHosts,
  usePersistentPairing,
  usePins,
  useRemoteHost,
  useRtdb,
} from '../remote/useRemote';
import '../remote/remote.css';

interface RemotePageProps {
  locale: Locale;
  theme: Theme;
  route: AppRoute;
  onLocaleChange(locale: Locale): void;
  onThemeToggle(): void;
}

function signInHref(): string {
  return `/login/remote?next=${encodeURIComponent(window.location.pathname)}`;
}

/** Remote Control: the signed-in user's computers, their sessions, and one session at a time. */
export default function RemotePage(props: RemotePageProps) {
  const { locale, theme, onLocaleChange, onThemeToggle } = props;
  const auth = useAuthState();
  const [crypto, setCrypto] = useState<boolean | undefined>(undefined);
  const persistent = usePersistentPairing();

  useEffect(() => {
    // The strict policy is set for the exact paths /remote and /remote/**; a differently-cased URL would load without it.
    const prefix = /^\/remote(?=\/|$)/iu.exec(window.location.pathname)?.[0];
    if (prefix !== undefined && prefix !== '/remote') {
      window.location.replace(`/remote${window.location.pathname.slice(prefix.length)}${window.location.search}${window.location.hash}`);
    }
  }, []);

  useEffect(() => {
    let live = true;
    void supportsRemoteCrypto().then((ok) => live && setCrypto(ok));
    return () => {
      live = false;
    };
  }, []);

  return (
    <div className="rc-page">
      <header className="rc-shell-head">
        <h1>{t(locale, 'title')}</h1>
        <div className="rc-shell-actions">
          {auth.status === 'signedIn' && auth.email ? <span className="rc-account" title={t(locale, 'signedInAs', { email: auth.email })}>{auth.email}</span> : null}
          <button type="button" className="control-chip" onClick={() => onLocaleChange(locale === 'zh' ? 'en' : 'zh')}>{t(locale, 'language')}</button>
          <button type="button" className="control-chip" aria-label={t(locale, 'theme')} onClick={onThemeToggle}>{theme === 'dark' ? '☀' : '☾'}</button>
          {auth.status === 'signedIn' ? (
            <button type="button" className="control-chip" onClick={() => void signOutUser()}>{t(locale, 'signOut')}</button>
          ) : null}
        </div>
      </header>
      {!persistent ? <p className="rc-banner" role="status">{t(locale, 'storageUnavailable')}</p> : null}
      <main className="rc-main">
        {auth.status === 'loading' || crypto === undefined ? (
          <p className="rc-note" role="status">{t(locale, 'loading')}</p>
        ) : crypto === false ? (
          <section className="rc-panel" role="alert"><p>{t(locale, 'unsupportedBrowser')}</p></section>
        ) : auth.status === 'signedOut' ? (
          <section className="rc-panel">
            <h2>{t(locale, 'signedOutTitle')}</h2>
            <p>{t(locale, 'signedOutBody')}</p>
            <div className="rc-actions">
              <a className="button primary" href={signInHref()}>{t(locale, 'signIn')}</a>
            </div>
          </section>
        ) : (
          <SignedIn key={auth.uid} uid={auth.uid} locale={locale} route={props.route} />
        )}
      </main>
    </div>
  );
}

function SignedIn({ uid, locale, route }: { uid: string; locale: Locale; route: AppRoute }) {
  const rtdb = useRtdb(uid);
  const hosts = useHosts(uid, rtdb);
  const target = parseRemoteRoute(route);
  const [chosen, setChosen] = useState<string | null>(null);
  const hostId = target?.hostId ?? chosen;
  const remote = useRemoteHost(uid, rtdb, hostId);
  const state = useConnectionState(remote?.connection ?? null);
  const [pinVersion, setPinVersion] = useState(0);
  const pins = usePins(`${pinVersion}:${hostId}:${state?.phase}`);
  const deviceName = browserDeviceName();

  const leave = (): void => {
    setChosen(null);
    navigateTo('/remote');
  };
  const forget = (id: string): void => {
    void getDeviceKeys().unpair(id).then(() => setPinVersion((value) => value + 1), () => undefined);
  };

  if (hostId === null) {
    return <HostList locale={locale} hosts={hosts} pins={pins} onConnect={setChosen} onForget={forget} />;
  }
  const hostName = hosts.hosts.find((host) => host.hostId === hostId)?.name ?? state?.host?.name ?? hostId;
  if (remote === null || state === null) return <p className="rc-note" role="status">{t(locale, 'loading')}</p>;

  const usable = state.phase === 'ready' || (state.everReady && state.phase === 'reconnecting');
  if (!usable) {
    return (
      <ConnectionScreen
        locale={locale}
        hostName={hostName}
        state={state}
        connection={remote.connection}
        onLeave={leave}
        onForget={() => {
          forget(hostId);
          leave();
        }}
      />
    );
  }
  if (target !== null) {
    return (
      <SessionView
        key={`${uid}:${hostId}:${target.sessionId}`}
        locale={locale}
        client={remote.client}
        sessionId={target.sessionId}
        connection={state}
        deviceName={deviceName}
        onBack={() => {
          setChosen(hostId);
          navigateTo('/remote');
        }}
      />
    );
  }
  return (
    <>
      <div className="rc-crumb">
        <button type="button" className="rc-link" onClick={leave}>{t(locale, 'switchComputer')}</button>
        <span>{hostName}</span>
      </div>
      <SessionList locale={locale} client={remote.client} onOpen={(sessionId) => navigateTo(remoteSessionRoute(hostId, sessionId))} />
    </>
  );
}
