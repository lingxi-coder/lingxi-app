import { tr } from '../i18n/locale';
import { loginItemView, type LoginItemController } from '../bridge/botLoginItem';
import { useObservable } from './BotFields';
import { Toggle } from './settings/primitives';

/** What the Bots ask of this Mac to go on working while the owner is away: that it stays awake, and that LingXi is open. */
export function BotsBackground({ keepAwake, onKeepAwake, loginItem }: {
  keepAwake: boolean;
  onKeepAwake(enabled: boolean): void;
  loginItem: LoginItemController;
}) {
  const state = useObservable(loginItem);
  const view = loginItemView(state.status, state.busy);
  const note = view.note === 'unavailable' ? tr('bots_login_item_unavailable')
    : view.note === 'approval' ? tr('bots_login_item_approval') : tr('bots_login_item_hint');
  return <section className="bots-background" aria-label={tr('bots_background_title')}>
    <h2>{tr('bots_background_title')}</h2>
    <ul className="bots-rows">
      <li className="bots-row">
        <div className="bots-row-main">
          <span className="bots-row-title">{tr('bots_keep_awake')}</span>
          <span className="bots-row-text">{tr('bots_keep_awake_hint')}</span>
        </div>
        <Toggle value={keepAwake} label={tr('bots_keep_awake')} onChange={onKeepAwake} />
      </li>
      <li className="bots-row">
        <div className="bots-row-main">
          <span className="bots-row-title">{tr('bots_login_item')}</span>
          <span className="bots-row-text">{note}</span>
          {state.error !== undefined
            ? <span role="alert" className="bots-problem">{state.error === '' ? tr('bots_login_item_failed') : tr('bots_action_failed', state.error)}</span>
            : null}
        </div>
        <div className="bots-row-actions">
          {view.canOpenSettings ? <button type="button" onClick={() => void loginItem.openSettings()}>{tr('bots_login_item_open_settings')}</button> : null}
          <Toggle value={view.checked} disabled={view.disabled} label={tr('bots_login_item')} onChange={(enabled) => void loginItem.setEnabled(enabled)} />
        </div>
      </li>
    </ul>
  </section>;
}
