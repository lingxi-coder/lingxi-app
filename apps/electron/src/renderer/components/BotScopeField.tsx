import { useEffect, useRef, useState } from 'react';
import { DATA_SOURCE_KINDS } from '../../../../../packages/teams/lingxi-data.js';
import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { desktopHost } from '../bridge/bridgeConnection';
import { useAccountState } from '../bridge/useAccountState';
import type { BotDraft, BotDraftProblems } from '../bridge/botForm';
import {
  BotScopeChoicesController, desktopScopeSources, scopeView, withApp, withDataset, withDatasetAccess,
  type BotScopeChoices, type ScopeDatasetRow, type ScopeView,
} from '../bridge/botScope';
import { problemText, useObservable } from './BotFields';
import { Icon } from './Icon';
import { Toggle } from './settings/primitives';

interface ScopeFieldProps {
  draft: BotDraft;
  problems: BotDraftProblems;
  disabled?: boolean;
  onChange(change: Partial<BotDraft>): void;
}

/**
 * The Scope tab as the page shows it: it reads what the owner has each time it is opened, so an app installed
 * or a data source added meanwhile is there to choose, and the Bot's scope is shown against it.
 */
export function BotScopeTab(props: ScopeFieldProps) {
  const { state, readError } = useAccountState();
  const uid = state?.user?.uid;
  const uidRef = useRef(uid);
  uidRef.current = uid;
  const [controller] = useState(() => new BotScopeChoicesController(() => desktopScopeSources(desktopHost(), uidRef.current)));
  const choices = useObservable(controller);
  // Whether there is an account is not known until the account has answered: asking before would show the data sources as unreadable for a moment.
  const settled = desktopHost()?.account === undefined || state !== null || readError !== '';
  useEffect(() => { if (settled) void controller.load(); }, [controller, settled, uid]);
  return <BotScopeField {...props} choices={choices} />;
}

export function BotScopeField({ draft, problems, choices, disabled = false, onChange }: ScopeFieldProps & { choices: BotScopeChoices }) {
  const view = scopeView(choices, draft.scope);
  const setScope = (scope: BotDraft['scope']) => onChange({ scope });
  return <div className="bots-fields bots-scope">
    <p className="bots-hint">{tr('bots_scope_intro')}</p>
    <BotScopeApps view={view} choices={choices} draft={draft} disabled={disabled} setScope={setScope} />
    <BotScopeData view={view} choices={choices} draft={draft} disabled={disabled} setScope={setScope} />
    {problems.scope ? <p className="bots-problem" role="alert">{problemText(problems.scope)}</p> : null}
  </div>;
}

interface SectionProps {
  view: ScopeView;
  choices: BotScopeChoices;
  draft: BotDraft;
  disabled: boolean;
  setScope(scope: BotDraft['scope']): void;
}

function RemoveButton({ name, onRemove }: { name: string; onRemove(): void }) {
  return <div className="bots-row-actions">
    <button type="button" data-variant="quiet" aria-label={tr('bots_scope_remove', name)} onClick={onRemove}><Icon name="x" size={15} /></button>
  </div>;
}

function BotScopeApps({ view, choices, draft, disabled, setScope }: SectionProps) {
  const { locale } = useI18n();
  const { scope } = draft;
  return <fieldset className="bots-fields bots-scope-section" disabled={disabled}>
    <legend className="bots-label">{tr('desktop_local_apps')}</legend>
    <p className="bots-hint">{tr('bots_scope_apps_hint')}</p>
    {choices.apps === undefined ? <p role="status" className="bots-muted">{tr('bots_scope_apps_unknown')}</p> : null}
    {choices.apps !== undefined && view.apps.length === 0 && view.otherApps.length === 0 ? <p className="bots-empty-line">{tr('bots_scope_apps_empty')}</p> : null}
    {view.apps.length + view.otherApps.length > 0 ? <ul className="bots-rows">
      {view.apps.map((app) => <li key={app.id} className="bots-row">
        <div className="bots-row-main bots-scope-app">
          <span className="bots-scope-avatar" aria-hidden="true">{Array.from(app.name)[0]?.toLocaleUpperCase(locale)}</span>
          <span className="bots-row-title">{app.name}</span>
        </div>
        <Toggle value={app.chosen} disabled={disabled} label={app.name} onChange={(on) => setScope(withApp(scope, app.id, on))} />
      </li>)}
      {view.otherApps.map((app) => <li key={app.id} className="bots-row" data-gone={app.status === 'missing' ? '' : undefined}>
        <div className="bots-row-main">
          <span className="bots-row-title mono">{app.id}</span>
          <span className="bots-row-text">{app.status === 'missing' ? tr('bots_scope_app_missing') : tr('bots_scope_app_unchecked')}</span>
        </div>
        <RemoveButton name={app.id} onRemove={() => setScope(withApp(scope, app.id, false))} />
      </li>)}
    </ul> : null}
  </fieldset>;
}

function BotScopeData({ view, choices, draft, disabled, setScope }: SectionProps) {
  const { scope } = draft;
  const nothingToShow = view.groups.length === 0 && view.leftover.length === 0;
  return <fieldset className="bots-fields bots-scope-section" disabled={disabled}>
    <legend className="bots-label">{tr('bots_scope_data_title')}</legend>
    <p className="bots-hint">{tr('bots_scope_data_hint')}</p>
    {choices.data === 'idle' || choices.data === 'loading' ? <p role="status" className="bots-muted">{tr('bots_scope_data_loading')}</p> : null}
    {choices.data === 'unavailable' ? <p role="status" className="bots-muted">{tr('bots_scope_data_unavailable')}</p> : null}
    {choices.data === 'ready' && nothingToShow ? <p className="bots-empty-line">{tr('bots_scope_data_empty')}</p> : null}
    {view.groups.map((group) => <div key={group.source} className="bots-scope-group">
      <h3 className="bots-scope-source">
        <span>{group.name}</span>
        <span className="bots-tag">{DATA_SOURCE_KINDS[group.kind as keyof typeof DATA_SOURCE_KINDS] ?? group.kind}</span>
      </h3>
      {!group.readable ? <p role="status" className="bots-muted">{tr('bots_scope_source_unreadable')}</p> : null}
      {group.readable && group.datasets.length === 0 ? <p className="bots-empty-line">{tr('bots_scope_source_empty')}</p> : null}
      {group.datasets.length > 0 ? <ul className="bots-rows">
        {group.datasets.map((row) => <DatasetRow key={row.dataset} source={group.source} title={row.dataset} row={row} disabled={disabled}
          note={row.gone ? tr('bots_scope_data_gone') : undefined} scope={scope} setScope={setScope} />)}
      </ul> : null}
    </div>)}
    {view.leftover.length > 0 ? <div className="bots-scope-group">
      <h3 className="bots-scope-source">{tr('bots_scope_data_other')}</h3>
      <ul className="bots-rows">
        {view.leftover.map((entry) => <DatasetRow key={`${entry.source}\u0000${entry.dataset}`} source={entry.source} title={`${entry.source} / ${entry.dataset}`}
          row={{ dataset: entry.dataset, access: undefined, chosen: true, write: entry.write, reportReview: entry.reportReview, gone: entry.status === 'missing' }}
          note={entry.status === 'missing' ? tr('bots_scope_source_gone') : choices.data === 'unavailable' ? tr('bots_scope_data_unchecked') : undefined}
          disabled={disabled} scope={scope} setScope={setScope} />)}
      </ul>
    </div> : null}
  </fieldset>;
}

/**
 * One dataset. Chosen and still there, it has its switch and what the Bot may do with it; the owner's own access caps that,
 * since a Bot is not given more than its owner has. Chosen and gone, it is greyed and can only be removed.
 */
function DatasetRow({ source, title, row, note, scope, disabled, setScope }: {
  source: string;
  title: string;
  row: ScopeDatasetRow;
  note: string | undefined;
  scope: BotDraft['scope'];
  disabled: boolean;
  setScope(scope: BotDraft['scope']): void;
}) {
  if (row.gone) {
    return <li className="bots-row" data-gone="">
      <div className="bots-row-main">
        <span className="bots-row-title">{title}</span>
        {note ? <span className="bots-row-text">{note}</span> : null}
      </div>
      <RemoveButton name={title} onRemove={() => setScope(withDataset(scope, source, row.dataset, false))} />
    </li>;
  }
  const readOnly = row.access === 'read';
  return <li className="bots-row">
    <div className="bots-row-main">
      <span className="bots-row-title">{title}</span>
      {note ? <span className="bots-row-text">{note}</span> : null}
      {row.chosen && readOnly ? <span className="bots-row-text">{tr('bots_scope_data_read_only')}</span> : null}
      {row.chosen ? <ul className="bots-scope-flags">
        <li>
          <span>{tr('bots_scope_data_write')}</span>
          <Toggle value={row.write} disabled={disabled || (readOnly && !row.write)} label={`${tr('bots_scope_data_write')}: ${title}`}
            onChange={(on) => setScope(withDatasetAccess(scope, source, row.dataset, { write: on }))} />
        </li>
        <li>
          <span>{tr('bots_scope_data_review')}</span>
          <Toggle value={row.reportReview} disabled={disabled || (readOnly && !row.reportReview)} label={`${tr('bots_scope_data_review')}: ${title}`}
            onChange={(on) => setScope(withDatasetAccess(scope, source, row.dataset, { reportReview: on }))} />
        </li>
      </ul> : null}
    </div>
    <Toggle value={row.chosen} disabled={disabled} label={title} onChange={(on) => setScope(withDataset(scope, source, row.dataset, on))} />
  </li>;
}
