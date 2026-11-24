import { useId, type CSSProperties, type ReactElement } from 'react';
import { PersonalDataPanel } from '../../../../../packages/teams/PersonalChatData.js';
import type { TeamSummary, TeamsApi } from '../../../../../packages/teams/index.js';
import { tr } from '../i18n/locale';
import { useT } from '../theme/ThemeContext';
import { DatasetGlyph } from './LocalAppsSidebar';

/** The Datasets page of the Apps workspace: the signed-in person's datasets, which apps bind to. */
export function LocalAppDatasets({ workspace, locale, teamsTheme }: {
  /** Absent until someone is signed in. */
  workspace?: { uid: string; api: TeamsApi; teams: TeamSummary[] };
  locale: string;
  teamsTheme: CSSProperties;
}): ReactElement {
  const t = useT();
  const heading = useId();
  return <section className="local-app-library local-app-datasets" aria-labelledby={heading} style={{
    '--apps-background': t.stageBg, '--apps-text': t.text, '--apps-muted': t.text3, '--apps-surface': t.surface,
    '--apps-border': t.border, '--apps-accent': t.accent, '--apps-error': t.danger,
  } as CSSProperties}>
    <header className="local-app-library-header"><div>
      <h1 id={heading}>{tr('apps_datasets')}</h1>
      <p>{tr('apps_datasets_intro')}</p>
    </div></header>
    {workspace?.api.data
      ? <div className="tw-data-workspace local-app-datasets-panel" style={teamsTheme}>
          <PersonalDataPanel key={workspace.uid} uid={workspace.uid} api={workspace.api} teams={workspace.teams} locale={locale} />
        </div>
      : <div className="local-app-empty"><DatasetGlyph size={38}/><p>{tr('apps_datasets_signed_out')}</p></div>}
  </section>;
}
