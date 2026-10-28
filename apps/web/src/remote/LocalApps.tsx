import { useLayoutEffect, useMemo } from 'react';
import type { Locale } from '../utils/locale';
import { LocalAppsController, type AppWindow } from './appsController';
import { t } from './strings';
import { displayText } from './text';
import { useStore } from './useRemote';
import type { ViewerConnection } from './viewerConnection';

export function LocalApps({ locale, connection, ready }: { locale: Locale; connection: ViewerConnection; ready: boolean }) {
  const controller = useMemo(() => new LocalAppsController(connection), [connection]);
  const state = useStore(controller)!;
  useLayoutEffect(() => {
    controller.activate();
    return () => controller.deactivate();
  }, [controller]);
  const errors = { list: 'appsListFailed', open: 'appsOpenFailed', popup: 'appsPopupBlocked', invalid: 'appsInvalidUrl' } as const;
  return (
    <section className="rc-panel">
      <h2>{t(locale, 'apps')}</h2>
      <p>{t(locale, 'appsPlacement')}</p>
      <p>{t(locale, 'appsLongRunning')}</p>
      <div className="rc-actions"><button type="button" className="control-chip" onClick={() => void controller.refresh()} disabled={!ready || state.loading}>{t(locale, 'appsRefresh')}</button></div>
      {!ready ? <p className="rc-note" role="status">{t(locale, 'reconnecting')}</p> : null}
      {state.loading ? <p className="rc-note" role="status">{t(locale, 'loading')}</p> : null}
      {state.error ? <p className="rc-danger" role="alert">{t(locale, errors[state.error])}</p> : null}
      {state.apps?.length === 0 && !state.loading ? <div className="rc-empty"><h3>{t(locale, 'noApps')}</h3><p>{t(locale, 'noAppsHelp')}</p></div> : null}
      <ul className="rc-list">
        {(state.apps ?? []).map(app => <li key={app.app_id}>
          <div className="rc-row">
            <span className="rc-row-main"><strong>{displayText(app.name, 120)}</strong><span className="rc-row-sub">{displayText(app.summary, 240)}</span><span className="rc-row-sub">{t(locale, 'appsRunsOnDesktop')}</span></span>
            <button type="button" className="control-chip" disabled={!ready || state.opening !== undefined} onClick={() => void controller.open(app.app_id, () => window.open('', '_blank') as AppWindow | null)}>{state.opening === app.app_id ? t(locale, 'appsOpening') : t(locale, 'appsOpen')}</button>
          </div>
        </li>)}
      </ul>
    </section>
  );
}
