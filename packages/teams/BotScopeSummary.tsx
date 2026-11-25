import type { ReactElement } from 'react';
import type { PublishedBotScope } from './bot-scope.js';
import { useTeamsText } from './i18n.js';

/**
 * What a Bot may use of its maintainer's computer: the names of its Local Apps and datasets, and a dataset it may edit says so. Nothing else of
 * them is known here. `limited` says a Chat uses less of it than the maintainer shared. A Bot that uses none of it says that instead.
 */
export function BotScopeSummary({ scope, limited = false, locale }: { scope: PublishedBotScope; limited?: boolean; locale: string }): ReactElement {
  const t = useTeamsText(locale);
  const tag = limited ? <> <span className="tw-tag">{t('teams_bot_scope_limited')}</span></> : null;
  if (!scope.localApps.length && !scope.datasets.length) return <p className="teams-hint tw-bot-scope">{t('teams_bot_scope_none')}{tag}</p>;
  return <div className="tw-bot-scope">
    <p className="teams-hint">{t('teams_bot_scope_uses')}{tag}</p>
    <dl className="tw-bot-scope-list">
      {scope.localApps.length ? <div><dt>{t('teams_bot_scope_apps')}</dt>
        <dd><ul className="tw-bot-scope-items">{scope.localApps.map(app => <li key={app.id}>{app.name}</li>)}</ul></dd></div> : null}
      {scope.datasets.length ? <div><dt>{t('teams_bot_scope_datasets')}</dt>
        <dd><ul className="tw-bot-scope-items">{scope.datasets.map(row => <li key={row.dataset}>{row.dataset}{row.write ? <span className="tw-tag">{t('teams_bot_scope_edit')}</span> : null}</li>)}</ul></dd></div> : null}
    </dl>
  </div>;
}
