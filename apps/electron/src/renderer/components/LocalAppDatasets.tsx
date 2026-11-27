import { useId, type CSSProperties, type ReactElement } from 'react';
import { createPortal } from 'react-dom';
import { PersonalDataPanel, type PersonalDatasetCatalog } from '../../../../../packages/teams/PersonalChatData.js';
import type { TeamSummary, TeamsApi } from '../../../../../packages/teams/index.js';
import { tr } from '../i18n/locale';
import { useT } from '../theme/ThemeContext';
import { DatasetGlyph } from './LocalAppsSidebar';
import './LocalAppDatasets.css';

/** The signed-in person's datasets and the records in the selected dataset. */
export function LocalAppDatasets({ workspace, locale, teamsTheme, sidebarTarget }: {
  /** Absent until someone is signed in. */
  workspace?: { uid: string; api: TeamsApi; teams: TeamSummary[] };
  locale: string;
  teamsTheme: CSSProperties;
  sidebarTarget?: HTMLElement | null;
}): ReactElement {
  const t = useT();
  const heading = useId();
  const title = <div>
    <h1 id={heading}>{tr('apps_datasets')}</h1>
    <p>{tr('apps_datasets_intro')}</p>
  </div>;
  const icon = <DatasetGlyph size={24} />;
  const renderCatalog = sidebarTarget ? (catalog?: PersonalDatasetCatalog) =>
    createPortal(<DatasetSidebar catalog={catalog} />, sidebarTarget) : undefined;
  return <section className="local-app-library local-app-datasets" aria-labelledby={heading} style={{
    '--apps-background': t.stageBg, '--apps-text': t.text, '--apps-muted': t.text3, '--apps-surface': t.surface,
    '--apps-border': t.border, '--apps-accent': t.accent, '--apps-error': t.danger,
    '--apps-tint': t.accentBg, '--apps-hover': t.surfaceHover, '--apps-active': t.surfaceActive,
    '--apps-sidebar': t.sidebarPanelBg, ...teamsTheme,
  } as CSSProperties}>
    <div className="tw-scope tw-data-workspace local-app-datasets-panel">
      {workspace?.api.data
        ? <PersonalDataPanel key={workspace.uid} uid={workspace.uid} api={workspace.api} teams={workspace.teams} locale={locale}
            page={{ heading: title, icon, renderCatalog, labels: {
              newDataset: tr('apps_datasets_new'), create: tr('apps_datasets_create'), cancel: tr('common_cancel'),
              loading: tr('apps_datasets_loading'), loadFailed: tr('apps_datasets_load_failed'),
              opening: tr('apps_datasets_opening'), openFailed: tr('apps_datasets_open_failed'), retry: tr('common_retry'),
              empty: tr('apps_datasets_empty'), emptyHint: tr('apps_datasets_empty_hint'),
              dataset: tr('apps_datasets_picker'), choose: tr('apps_datasets_choose'),
              records: tr('apps_datasets_records'), noRecords: tr('apps_datasets_no_records'), addRecord: tr('apps_datasets_add_record'),
              schemaHelp: tr('apps_datasets_schema_help'), schemaInvalid: tr('apps_datasets_schema_invalid'),
              createFailed: tr('apps_datasets_create_failed'), publish: tr('apps_datasets_publish'),
            } }} />
        : <>
            {renderCatalog?.()}
            <header className="personal-dataset-header">{title}</header>
            <div className="personal-dataset-empty">
              <span className="personal-dataset-empty-icon">{icon}</span>
              <p>{tr('apps_datasets_signed_out')}</p>
            </div>
          </>}
    </div>
  </section>;
}

function DatasetSidebar({ catalog }: { catalog?: PersonalDatasetCatalog }): ReactElement {
  const t = useT();
  const title = useId();
  const unavailable = catalog?.busy || catalog?.status === 'loading';
  return <nav className="desktop-sidebar-list dataset-sidebar no-drag" aria-labelledby={title} style={{
    '--apps-text': t.text, '--apps-muted': t.text3, '--apps-accent': t.accent,
    '--apps-tint': t.accentBg, '--apps-hover': t.surfaceHover, '--apps-active': t.surfaceActive,
  } as CSSProperties}>
    <h2 id={title}>{tr('apps_datasets')}</h2>
    {!catalog && <p>{tr('apps_datasets_signed_out')}</p>}
    {catalog?.status === 'loading' && <p role="status">{tr('apps_datasets_loading')}</p>}
    {catalog?.status === 'error' && <div className="dataset-sidebar-error">
      <p role="alert">{tr('apps_datasets_load_failed')}</p>
      <button type="button" disabled={catalog.busy} onClick={catalog.onRetry}>{tr('common_retry')}</button>
    </div>}
    {catalog?.status === 'ready' && !catalog.datasets.length && <p>{tr('apps_datasets_empty')}</p>}
    {catalog && catalog.datasets.length > 0 && <ul>
      {catalog.datasets.map(dataset => <li key={dataset}>
        <button type="button" className="dataset-sidebar-item" title={dataset}
          aria-current={catalog.selected === dataset ? 'page' : undefined} aria-disabled={unavailable || undefined}
          onClick={() => { if (!unavailable && catalog.selected !== dataset) catalog.onSelect(dataset); }}>
          <DatasetGlyph size={17} /><span>{dataset}</span>
        </button>
      </li>)}
    </ul>}
  </nav>;
}
