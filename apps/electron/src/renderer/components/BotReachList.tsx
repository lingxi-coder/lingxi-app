import { useEffect, useState } from 'react';
import type { BotReachUi } from '../../shared/bots';
import { tr } from '../i18n/locale';
import { desktopHost } from '../bridge/bridgeConnection';

/** What the Bots are asked about: an installed Local App, or a data source. */
export type BotReachTarget = { kind: 'localApp'; appId: string } | { kind: 'dataSource'; source: string };

/**
 * The owner's Bots whose scope reaches a Local App or a data source, on the page of that app or source. The main
 * process works them out from the Bots' own files. A Bot's name is all that comes back: not its scope, not its keys.
 * Nothing is shown while the answer is awaited, or when there is none to give (no Bots on this computer, no host).
 */
export function BotReachList({ target }: { target: BotReachTarget }) {
  const key = target.kind === 'localApp' ? `app:${target.appId}` : `source:${target.source}`;
  const [found, setFound] = useState<{ key: string; bots: readonly BotReachUi[] }>();
  useEffect(() => {
    const reach = desktopHost()?.bots?.reach;
    if (!reach) return;
    let current = true;
    const asked = target.kind === 'localApp' ? reach.localApp(target.appId) : reach.dataset(target.source);
    asked.then((bots) => { if (current) setFound({ key, bots }); }, () => { if (current) setFound(undefined); });
    return () => { current = false; };
    // `key` is the whole of `target`.
  }, [key]);
  return found?.key === key ? <BotReachView target={target} bots={found.bots} /> : null;
}

export function BotReachView({ target, bots }: { target: BotReachTarget; bots: readonly BotReachUi[] }) {
  return <div className="bots-reach">
    <h3 className="bots-reach-title">{tr(target.kind === 'localApp' ? 'bots_reach_app_title' : 'bots_reach_source_title')}</h3>
    {bots.length > 0
      ? <ul className="bots-reach-list">{bots.map((bot) => <li key={bot.botId}>{bot.name}</li>)}</ul>
      : <p className="bots-reach-none">{tr('bots_reach_none')}</p>}
  </div>;
}
