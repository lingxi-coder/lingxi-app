import { useEffect, useId, useRef, useState, useSyncExternalStore, type KeyboardEvent, type ReactNode } from 'react';
import type { PermissionModeId } from '@lingxi/bridge-client';
import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { groupModelReferences, modelDisplayLabel, type ModelCatalogDetail } from '../bridge/modelCatalog';
import { BOT_DESCRIPTION_MAX, BOT_FOLDERS_MAX, ipcErrorMessage, permissionModeNeedsConfirmation, withFolder, type BotDraft, type BotDraftProblem, type BotDraftProblems } from '../bridge/botForm';
import type { BotMemoryProblem } from '../bridge/botMemory';
import type { ObservableState } from '../bridge/observableState';
import { permissionModeOptions } from '../model/permissionModes';
import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { Icon } from './Icon';

/** Read a controller's state; the component renders again whenever it changes. */
export function useObservable<S>(source: ObservableState<S>): S {
  return useSyncExternalStore(source.subscribe, source.getSnapshot, source.getSnapshot);
}

/** The models a Bot can be given: the ones the displayed chat's engine lists. */
export interface ModelChoices {
  references: readonly string[];
  details: readonly ModelCatalogDetail[];
}

export function problemText(problem: BotDraftProblem | BotMemoryProblem): string {
  switch (problem) {
    case 'name_required': return tr('bots_problem_name_required');
    case 'name_too_long': return tr('bots_problem_name_too_long');
    case 'name_invalid': return tr('bots_problem_name_invalid');
    case 'description_too_long': return tr('bots_problem_description_too_long');
    case 'persona_too_long': return tr('bots_problem_persona_too_long');
    case 'folders_too_many': return tr('bots_problem_folders_too_many');
    case 'folders_duplicate': return tr('bots_problem_folders_duplicate');
    case 'too_large': return tr('bots_problem_memory_too_large');
    case 'invalid': return tr('bots_problem_memory_invalid');
  }
}

function trapTab(event: KeyboardEvent<HTMLDivElement>, dialog: HTMLElement | null): void {
  if (event.key !== 'Tab' || !dialog) return;
  const controls = dialog.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled)');
  const first = controls[0];
  const last = controls[controls.length - 1];
  if (!first || !last) { event.preventDefault(); dialog.focus(); return; }
  if (document.activeElement === dialog) { event.preventDefault(); (event.shiftKey ? last : first).focus(); return; }
  if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
  else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
}

/** A dialog the owner answers in the app: focus goes in and comes back, Tab stays inside, Escape closes unless `busy`. */
export function BotDialog({ title, icon, danger = false, busy = false, footer, onClose, children }: {
  title: string;
  icon: string;
  danger?: boolean;
  busy?: boolean;
  footer: ReactNode;
  onClose(): void;
  children: ReactNode;
}) {
  const dialog = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    dialog.current?.setAttribute('aria-modal', 'true');
    // The safe answer holds focus first: a stray Enter must not delete anything.
    (dialog.current?.querySelector<HTMLButtonElement>('.desktop-dialog-action--cancel') ?? dialog.current?.querySelector<HTMLButtonElement>('button'))?.focus();
    return () => opener?.focus();
  }, []);
  return <DesktopDialog ref={dialog} tabIndex={-1} title={title} ariaLabel={title} icon={icon} tone={danger ? 'danger' : 'default'} size="compact"
    onEscape={() => { if (!busy) onClose(); }} onKeyDown={(event) => trapTab(event, dialog.current)} footer={footer}>
    {children}
  </DesktopDialog>;
}

/** Ask before doing something that cannot be taken back; nothing happens until the owner confirms. */
export function BotConfirm({ title, icon = 'trash', danger = true, confirmLabel, busyLabel, busy = false, disabled = false, error, onConfirm, onCancel, children }: {
  title: string;
  icon?: string;
  danger?: boolean;
  confirmLabel: string;
  busyLabel?: string;
  busy?: boolean;
  /** The question cannot be answered yes right now. */
  disabled?: boolean;
  error?: string | undefined;
  onConfirm(): void;
  onCancel(): void;
  children: ReactNode;
}) {
  return <BotDialog title={title} icon={icon} danger={danger} busy={busy} onClose={onCancel}
    footer={<DesktopDialogActions>
      <DesktopDialogButton variant="cancel" disabled={busy} onClick={onCancel}>{tr('common_cancel')}</DesktopDialogButton>
      <DesktopDialogButton variant="primary" disabled={busy || disabled} onClick={onConfirm}>{busy ? busyLabel ?? confirmLabel : confirmLabel}</DesktopDialogButton>
    </DesktopDialogActions>}>
    <div className="bots-dialog-body">
      {children}
      {error ? <p role="alert" className="bots-problem">{error}</p> : null}
    </div>
  </BotDialog>;
}

function Field({ label, hint, problem, children }: {
  label: string;
  hint?: string;
  problem?: string | undefined;
  children(control: { id: string; 'aria-invalid'?: true; 'aria-describedby'?: string }): ReactNode;
}) {
  const id = useId();
  const described = [hint ? `${id}-hint` : '', problem ? `${id}-problem` : ''].filter(Boolean).join(' ');
  return <div className="bots-field">
    <label className="bots-label" htmlFor={id}>{label}</label>
    {children({ id, ...(problem ? { 'aria-invalid': true as const } : {}), ...(described ? { 'aria-describedby': described } : {}) })}
    {hint ? <p id={`${id}-hint`} className="bots-hint">{hint}</p> : null}
    {problem ? <p id={`${id}-problem`} className="bots-problem" role="alert">{problem}</p> : null}
  </div>;
}

interface FieldsProps {
  draft: BotDraft;
  problems: BotDraftProblems;
  disabled?: boolean;
  onChange(change: Partial<BotDraft>): void;
}

export function BotGeneralFields({ draft, problems, models, disabled = false, onChange }: FieldsProps & { models: ModelChoices }) {
  const groups = groupModelReferences(models.references);
  const remaining = BOT_DESCRIPTION_MAX - draft.description.trim().length;
  return <div className="bots-fields">
    <Field label={tr('bots_field_name')} problem={problems.name ? problemText(problems.name) : undefined}>
      {(control) => <input {...control} value={draft.name} disabled={disabled} autoComplete="off" onChange={(event) => onChange({ name: event.target.value })} />}
    </Field>
    <Field label={tr('bots_field_description')} hint={remaining <= 100 && remaining >= 0 ? tr('bots_characters_left', remaining) : tr('bots_field_description_hint')}
      problem={problems.description ? problemText(problems.description) : undefined}>
      {(control) => <textarea {...control} rows={3} value={draft.description} disabled={disabled} onChange={(event) => onChange({ description: event.target.value })} />}
    </Field>
    <Field label={tr('bots_field_model')} hint={tr('bots_model_hint')}>
      {(control) => <select {...control} value={draft.model} disabled={disabled} onChange={(event) => onChange({ model: event.target.value })}>
        <option value="">{tr('bots_model_default')}</option>
        {groups.map((group) => <optgroup key={group.providerId ?? ''} label={group.providerLabel}>
          {group.models.map((model) => <option key={model.reference} value={model.reference}>{modelDisplayLabel(model, models.details)}</option>)}
        </optgroup>)}
        {draft.model && !models.references.includes(draft.model) ? <option value={draft.model}>{tr('bots_model_unavailable', draft.model)}</option> : null}
      </select>}
    </Field>
  </div>;
}

export function BotPersonaField({ draft, problems, disabled = false, onChange }: FieldsProps) {
  return <div className="bots-fields">
    <Field label={tr('bots_tab_persona')} hint={tr('bots_field_persona_hint')} problem={problems.persona ? problemText(problems.persona) : undefined}>
      {(control) => <textarea {...control} className="bots-persona" rows={16} spellCheck={false} value={draft.persona} disabled={disabled} onChange={(event) => onChange({ persona: event.target.value })} />}
    </Field>
  </div>;
}

export function BotPermissionField({ draft, disabled = false, onChange }: Pick<FieldsProps, 'draft' | 'disabled' | 'onChange'>) {
  const { locale } = useI18n();
  const [confirming, setConfirming] = useState(false);
  const options = permissionModeOptions(locale);
  const current = options.find((option) => option.id === (draft.permissionMode || 'default'));
  return <div className="bots-fields">
    <Field label={tr('bots_permission_title')} hint={current?.description}>
      {(control) => <select {...control} value={draft.permissionMode} disabled={disabled}
        onChange={(event) => {
          const mode = event.target.value as PermissionModeId | '';
          if (permissionModeNeedsConfirmation(mode)) setConfirming(true);
          else onChange({ permissionMode: mode });
        }}>
        {options.map((option) => <option key={option.id} value={option.id === 'default' ? '' : option.id}>{option.label}</option>)}
      </select>}
    </Field>
    <p className="bots-hint">{tr('bots_permission_hint')}</p>
    {confirming ? <BotConfirm title={tr('bots_permission_full_title')} icon="shieldAlert" confirmLabel={tr('bots_permission_full_confirm')}
      onCancel={() => setConfirming(false)} onConfirm={() => { setConfirming(false); onChange({ permissionMode: 'bypassPermissions' }); }}>
      <p>{tr('bots_permission_full_body')}</p>
    </BotConfirm> : null}
  </div>;
}

export function BotFoldersField({ draft, problems, disabled = false, pickFolder, onChange }: FieldsProps & {
  /** Ask the owner for a folder with the system dialog; null when they cancel. */
  pickFolder(): Promise<{ path: string } | null>;
}) {
  const [picking, setPicking] = useState(false);
  const [error, setError] = useState('');
  const add = async () => {
    if (picking) return;
    setPicking(true);
    setError('');
    try {
      const selection = await pickFolder();
      if (selection) onChange({ workspaces: withFolder(draft.workspaces, selection.path) });
    } catch (cause) {
      setError(tr('bots_folder_pick_failed', ipcErrorMessage(cause)));
    } finally {
      setPicking(false);
    }
  };
  return <fieldset className="bots-fields bots-folders" disabled={disabled}>
    <legend className="bots-label">{tr('bots_tab_folders')}</legend>
    <p className="bots-hint">{tr('bots_folders_hint')}</p>
    {draft.workspaces.length === 0 ? <p className="bots-empty-line">{tr('bots_folders_empty')}</p> : <ul className="bots-folder-list">
      {draft.workspaces.map((folder) => <li key={folder.path}>
        <span className="bots-folder-path mono" title={folder.path}>{folder.path}</span>
        <select aria-label={tr('bots_folder_access', folder.path)} value={folder.mode}
          onChange={(event) => onChange({ workspaces: draft.workspaces.map((entry) => entry.path === folder.path ? { ...entry, mode: event.target.value === 'write' ? 'write' : 'read' } : entry) })}>
          <option value="read">{tr('bots_folder_read')}</option>
          <option value="write">{tr('bots_folder_write')}</option>
        </select>
        <button type="button" data-variant="quiet" aria-label={tr('bots_folder_remove', folder.path)}
          onClick={() => onChange({ workspaces: draft.workspaces.filter((entry) => entry.path !== folder.path) })}><Icon name="x" size={15} /></button>
      </li>)}
    </ul>}
    <div><button type="button" disabled={picking || draft.workspaces.length >= BOT_FOLDERS_MAX} onClick={() => void add()}>{tr('bots_folders_add')}</button></div>
    {problems.folders ? <p className="bots-problem" role="alert">{problemText(problems.folders)}</p> : null}
    {error ? <p className="bots-problem" role="alert">{error}</p> : null}
  </fieldset>;
}
