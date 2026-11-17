import { useEffect, useState } from 'react';
import type { SharedBotCard, SharedHostBot, SharedHostStatus } from './execution.js';
import type { TeamsApi } from './index.js';
import { useTeamsText } from './i18n.js';

/**
 * This Mac as the host of the Bots its owner publishes here: which of them it answers for, what stands in the way, and the two
 * switches that keep it serving while the owner is away (the same ones as on the Bots page). Desktop only. The state is the host's
 * and the system's, so nothing is kept: it is read again whenever the cards change and after each change made here.
 */
export function SharedHostPanel({ api, teamId, bots, locale, busy, attempt }: {
  api: TeamsApi; teamId: string; /** The published Bots of this team or friendship that the person maintains. */ bots: SharedBotCard[]; locale: string; busy: boolean;
  attempt(action: () => Promise<unknown>): Promise<void>;
}) {
  const t = useTeamsText(locale);
  const [status, setStatus] = useState<SharedHostStatus>(), [reads, setReads] = useState(0);
  const cards = bots.map(bot => `${bot.sharedBotId}:${bot.revision}:${bot.ready}`).join('|');
  useEffect(() => {
    let current = true;
    void api.hostStatus!().then(value => { if (current) setStatus(value); }, () => { if (current) setStatus(undefined); });
    return () => { current = false; };
  }, [api, cards, reads]);
  if (!status || !bots.length) return null;
  const served = (bot: SharedBotCard) => status.bots.find(value => value.teamId === teamId && value.sharedBotId === bot.sharedBotId);
  const line = (host: SharedHostBot | undefined) => {
    if (!host) return t('tw_host_elsewhere');
    if (host.state === 'ready') return host.running ? t('tw_host_answering_fmt', host.running) : t('tw_host_answering');
    if (host.state === 'checking') return t('tw_host_checking');
    if (host.state === 'unreachable') return t('tw_host_unreachable');
    if (host.state === 'stopped') return t('tw_host_stopped');
    return host.reason === 'bot' ? t('tw_host_no_bot')
      : host.reason === 'runtime' ? t('tw_host_no_runtime')
      : t('tw_host_no_model');
  };
  const here = bots.some(bot => served(bot)), { openAtLogin, keepAwake, onBattery } = status;
  const change = (value: { openAtLogin?: boolean; keepAwake?: boolean }) => void attempt(async () => { try { await api.setHostBackground!(value); } finally { setReads(count => count + 1); } });
  return <section className="team-bot-host" aria-label={t('tw_host_this_mac')}>
    <h4>{t('tw_host_this_mac')}</h4>
    <ul>{bots.map(bot => { const host = served(bot); return <li key={bot.sharedBotId} data-state={host?.state ?? 'elsewhere'}><b>{bot.name}</b> {line(host)}</li>; })}</ul>
    {here ? <>
      <p className="teams-hint">{t('tw_host_background_hint')}</p>
      <label className="team-party"><input type="checkbox" checked={openAtLogin.enabled} disabled={busy || !openAtLogin.available || !api.setHostBackground} onChange={event => change({ openAtLogin: event.target.checked })} />{t('tw_host_login_item')}</label>
      {!openAtLogin.available ? <p className="teams-hint">{t('tw_host_installed_only')}</p>
        : openAtLogin.needsApproval ? <p role="alert">{t('tw_host_allow_login_item')} {api.openLoginItems ? <button type="button" disabled={busy} onClick={() => void attempt(() => api.openLoginItems!())}>{t('tw_host_open_login_items')}</button> : null}</p>
        : !openAtLogin.enabled ? <p role="alert">{t('tw_host_after_restart')}</p> : null}
      <label className="team-party"><input type="checkbox" checked={keepAwake} disabled={busy || !api.setHostBackground} onChange={event => change({ keepAwake: event.target.checked })} />{t('tw_host_keep_awake')}</label>
      {!keepAwake ? <p role="alert">{t('tw_host_may_sleep')}</p>
        : onBattery ? <p role="alert">{t('tw_host_on_battery')}</p>
        : <p className="teams-hint">{t('tw_host_plugged_in')}</p>}
    </> : null}
  </section>;
}
