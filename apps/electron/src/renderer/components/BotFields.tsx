import { useEffect, useId, useRef, useState, useSyncExternalStore, type KeyboardEvent, type ReactNode } from 'react';
import type { PermissionModeId } from '@lingxi/bridge-client';
import { tr } from '../i18n/locale';
import { useI18n } from '../i18n/useI18n';
import { groupModelReferences, modelBillingGroups, modelDisplayLabel, type ModelCatalogDetail } from '../bridge/modelCatalog';
import { BOT_REPLY_COOLDOWN_SEC_MAX, BOT_REPLY_KEYWORDS_MAX, BOT_REPLY_POLICY_CHARS_MAX, type BotSandboxAvailability, type BotUi, type BotWorkspaceUi } from '../../shared/bots';
import { BOT_FOLDER_LABEL_MAX, folderBaseName, ownFolderTitle } from '../bridge/botFolders';
import { BOT_DESCRIPTION_MAX, BOT_FOLDERS_MAX, ipcErrorMessage, permissionModeNeedsConfirmation, withAllowedHost, withFolder, type BotDraft, type BotDraftProblem, type BotDraftProblems, type BotHostAdd } from '../bridge/botForm';
import type { BotMemoryProblem } from '../bridge/botMemory';
import type { ObservableState } from '../bridge/observableState';
import { permissionModeOptions } from '../model/permissionModes';
import { DesktopDialog, DesktopDialogActions, DesktopDialogButton } from './DesktopDialog';
import { Icon } from './Icon';
import { Toggle } from './settings/primitives';
import type { RemoteAvailability } from '../bridge/remoteControl';

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
    case 'folders_label_too_long': return tr('bots_problem_folders_label_too_long', BOT_FOLDER_LABEL_MAX);
    case 'accept_too_many': return tr('bots_problem_accept_too_many');
    case 'sandbox_too_many': return tr('bots_sandbox_host_too_many');
    case 'scope_too_many': return tr('bots_problem_scope_too_many');
    case 'reply_policy_too_long': return tr('bots_problem_reply_policy_too_long', BOT_REPLY_POLICY_CHARS_MAX);
    case 'reply_keywords_too_many': return tr('bots_problem_reply_keywords_too_many', BOT_REPLY_KEYWORDS_MAX);
    case 'reply_quiet_invalid': return tr('bots_problem_reply_quiet_invalid');
    case 'reply_cooldown_invalid': return tr('bots_problem_reply_cooldown_invalid', BOT_REPLY_COOLDOWN_SEC_MAX);
    case 'reply_thresholds_invalid': return tr('bots_problem_reply_thresholds_invalid');
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

/** A labelled control with its hint and its problem, which the control points to. */
export function Field({ label, hint, problem, children }: {
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
  return <div className="bots-fields bots-general-fields">
    <div className="bots-general-group">
    <Field label={tr('bots_field_name')} problem={problems.name ? problemText(problems.name) : undefined}>
      {(control) => <input {...control} value={draft.name} disabled={disabled} autoComplete="off" onChange={(event) => onChange({ name: event.target.value })} />}
    </Field>
    <Field label={tr('bots_field_description')} hint={remaining <= 100 && remaining >= 0 ? tr('bots_characters_left', remaining) : tr('bots_field_description_hint')}
      problem={problems.description ? problemText(problems.description) : undefined}>
      {(control) => <textarea {...control} rows={3} value={draft.description} disabled={disabled} onChange={(event) => onChange({ description: event.target.value })} />}
    </Field>
    </div>
    <div className="bots-general-group">
    <Field label={tr('bots_field_model')} hint={tr('bots_model_hint')}>
      {(control) => <select {...control} value={draft.model} disabled={disabled} onChange={(event) => onChange({ model: event.target.value })}>
        <option value="">{tr('bots_model_default')}</option>
        {groups.flatMap((group) => modelBillingGroups(group, models.details).map((billingGroup) => {
          const billingLabel = billingGroup.label === 'subscription' ? tr('provider_accounts_subscription')
            : billingGroup.label === 'api_key' ? tr('provider_accounts_key') : billingGroup.label;
          return <optgroup key={`${group.providerId ?? ''}:${billingGroup.label ?? ''}`} label={billingLabel ? `${group.providerLabel} · ${billingLabel}` : group.providerLabel}>
            {billingGroup.models.map((model) => <option key={model.reference} value={model.reference}>{modelDisplayLabel(model, models.details)}{billingLabel ? ` · ${billingLabel}` : ''}</option>)}
          </optgroup>;
        }))}
        {draft.model && !models.references.includes(draft.model) ? <option value={draft.model}>{tr('bots_model_unavailable', draft.model)}</option> : null}
      </select>}
    </Field>
    </div>
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
  const current = options.find((option) => option.id === draft.permissionMode);
  return <div className="bots-fields">
    <Field label={tr('bots_permission_title')} hint={current?.description ?? tr('bots_permission_app_default_hint')}>
      {(control) => <select {...control} value={draft.permissionMode} disabled={disabled}
        onChange={(event) => {
          const mode = event.target.value as PermissionModeId | '';
          if (permissionModeNeedsConfirmation(mode)) setConfirming(true);
          else onChange({ permissionMode: mode });
        }}>
        <option value="">{tr('bots_permission_app_default')}</option>
        {options.map((option) => <option key={option.id} value={option.id}>{option.label}</option>)}
      </select>}
    </Field>
    <p className="bots-hint">{tr('bots_permission_hint')}</p>
    {confirming ? <BotConfirm title={tr('bots_permission_full_title')} icon="shieldAlert" confirmLabel={tr('bots_permission_full_confirm')}
      onCancel={() => setConfirming(false)} onConfirm={() => { setConfirming(false); onChange({ permissionMode: 'bypassPermissions' }); }}>
      <p>{tr('bots_permission_full_body')}</p>
    </BotConfirm> : null}
  </div>;
}

/**
 * The switch that lets the owner's approved phones and browsers reach a Bot.
 * It cannot be turned on while Remote Control is unavailable, and says why;
 * turning it off is always allowed.
 */
export function BotRemoteField({ draft, availability, disabled = false, onChange }: Pick<FieldsProps, 'draft' | 'disabled' | 'onChange'> & {
  availability: RemoteAvailability;
}) {
  return <ul className="bots-rows bots-remote">
    <li className="bots-row">
      <div className="bots-row-main">
        <span className="bots-row-title">{tr('bots_remote_title')}</span>
        <span className="bots-row-text">{tr('bots_remote_hint')}</span>
        {!availability.available ? <span role="status" className="bots-row-text bots-remote-why">{availability.reason}</span> : null}
      </div>
      <Toggle value={draft.remote} disabled={disabled || (!availability.available && !draft.remote)} label={tr('bots_remote_title')}
        onChange={(remote) => onChange({ remote })} />
    </li>
  </ul>;
}

function hostProblemText(problem: Exclude<BotHostAdd, { ok: true }>['problem']): string {
  switch (problem) {
    case 'invalid': return tr('bots_sandbox_host_invalid');
    case 'duplicate': return tr('bots_sandbox_host_duplicate');
    case 'too_many': return tr('bots_sandbox_host_too_many');
  }
}

/** The hosts the commands of the sandbox may reach: the ones allowed so far, and one more to add. */
function BotSandboxHosts({ draft, problems, disabled, onChange }: Pick<FieldsProps, 'draft' | 'problems' | 'disabled' | 'onChange'>) {
  const [text, setText] = useState('');
  const [problem, setProblem] = useState<Exclude<BotHostAdd, { ok: true }>['problem']>();
  const hosts = draft.sandbox.allowedDomains;
  const set = (allowedDomains: string[]) => onChange({ sandbox: { ...draft.sandbox, allowedDomains } });
  const add = () => {
    if (text.trim() === '') return;
    const added = withAllowedHost(hosts, text);
    if (!added.ok) { setProblem(added.problem); return; }
    setProblem(undefined);
    setText('');
    set(added.domains);
  };
  return <fieldset className="bots-fields bots-sandbox-hosts" disabled={disabled}>
    <legend className="bots-label">{tr('bots_sandbox_hosts_title')}</legend>
    <p className="bots-hint">{tr('bots_sandbox_hosts_hint')}</p>
    {hosts.length > 0 ? <ul className="bots-rows">
      {hosts.map((host) => <li key={host} className="bots-row">
        <div className="bots-row-main"><span className="bots-row-title mono">{host}</span></div>
        <div className="bots-row-actions">
          <button type="button" data-variant="quiet" aria-label={tr('bots_sandbox_host_remove', host)} onClick={() => set(hosts.filter((entry) => entry !== host))}><Icon name="x" size={15} /></button>
        </div>
      </li>)}
    </ul> : <p className="bots-empty-line">{tr('bots_sandbox_hosts_empty')}</p>}
    <div className="bots-sandbox-add">
      <input value={text} placeholder={tr('bots_sandbox_host_placeholder')} aria-label={tr('bots_sandbox_host_label')} autoComplete="off" spellCheck={false}
        aria-invalid={problem ? true : undefined}
        onChange={(event) => { setText(event.target.value); setProblem(undefined); }}
        onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); add(); } }} />
      <button type="button" disabled={text.trim() === ''} onClick={add}>{tr('bots_sandbox_host_add')}</button>
    </div>
    {problem ? <p className="bots-problem" role="alert">{hostProblemText(problem)}</p> : null}
    {problems.sandbox ? <p className="bots-problem" role="alert">{problemText(problems.sandbox)}</p> : null}
  </fieldset>;
}

/**
 * The sandbox the shell commands of a Bot run in, and the hosts they may reach in it. It cannot be switched on where this
 * device cannot give one, and says why. Switching it off asks first: the commands are then confined by nothing but the
 * owner's trust in the Bot, since the rules that keep its file tools in its folders do not reach them.
 */
export function BotSandboxField({ draft, problems, availability, disabled = false, onChange }: FieldsProps & { availability: BotSandboxAvailability }) {
  const [confirming, setConfirming] = useState(false);
  const { enabled } = draft.sandbox;
  return <div className="bots-fields bots-sandbox">
    <ul className="bots-rows">
      <li className="bots-row">
        <div className="bots-row-main">
          <span className="bots-row-title">{tr('bots_sandbox_switch_title')}</span>
          <span className="bots-row-text">{tr('bots_sandbox_switch_hint')}</span>
          {!availability.available ? <span role="status" className="bots-row-text bots-sandbox-why">{tr(availability.reason)}</span> : null}
        </div>
        <Toggle value={enabled} disabled={disabled || (!availability.available && !enabled)} label={tr('bots_sandbox_switch_title')}
          onChange={(on) => { if (on) onChange({ sandbox: { ...draft.sandbox, enabled: true } }); else setConfirming(true); }} />
      </li>
    </ul>
    {enabled ? <BotSandboxHosts draft={draft} problems={problems} disabled={disabled} onChange={onChange} /> : null}
    {confirming ? <BotConfirm title={tr('bots_sandbox_off_title')} icon="shieldAlert" confirmLabel={tr('bots_sandbox_off_confirm')}
      onCancel={() => setConfirming(false)} onConfirm={() => { setConfirming(false); onChange({ sandbox: { ...draft.sandbox, enabled: false } }); }}>
      <p>{tr('bots_sandbox_off_body')}</p>
    </BotConfirm> : null}
  </div>;
}

/**
 * Where this device cannot give a Bot a sandbox the form makes the Bot without one: the switch is off and cannot be turned
 * on, and the owner says they understand what that means before the Bot can be made.
 */
export function BotSandboxAcknowledge({ reason, acknowledged, missing, disabled = false, onChange }: {
  /** The key of the text that says why this device has no sandbox. */
  reason: string;
  acknowledged: boolean;
  /** The owner tried to make the Bot without ticking the box. */
  missing: boolean;
  disabled?: boolean;
  onChange(acknowledged: boolean): void;
}) {
  return <fieldset className="bots-fields bots-sandbox" disabled={disabled}>
    <ul className="bots-rows">
      <li className="bots-row">
        <div className="bots-row-main">
          <span className="bots-row-title">{tr('bots_sandbox_switch_title')}</span>
          <span className="bots-row-text">{tr('bots_sandbox_unavailable')}</span>
          <span role="status" className="bots-row-text bots-sandbox-why">{tr(reason)}</span>
        </div>
        <Toggle value={false} disabled label={tr('bots_sandbox_switch_title')} onChange={() => undefined} />
      </li>
    </ul>
    <label className="bots-sandbox-ack">
      <input type="checkbox" checked={acknowledged} aria-invalid={missing && !acknowledged ? true : undefined} onChange={(event) => onChange(event.target.checked)} />
      <span>{tr('bots_sandbox_ack')}</span>
    </label>
    {missing && !acknowledged ? <p className="bots-problem" role="alert">{tr('bots_sandbox_ack_required')}</p> : null}
  </fieldset>;
}

export function BotFoldersField({ draft, problems, disabled = false, pickFolder, saved, chatDisabled = false, onNewChat, onChange }: FieldsProps & {
  /** Ask the owner for a folder with the system dialog; null when they cancel. */
  pickFolder(): Promise<{ path: string } | null>;
  /** The folders as the Bot has them saved: the ones a chat can be started in. Absent while the Bot is not made yet. */
  saved?: readonly BotWorkspaceUi[];
  /** A chat is being opened, or the window is busy showing one. */
  chatDisabled?: boolean;
  /** Start a chat of the Bot in a folder: `workspaceId` is absent for the Bot's own folder. Absent where the Bot is not made yet. */
  onNewChat?(workspaceId: string | undefined): void;
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
  const change = (path: string, change: { mode?: 'read' | 'write'; label?: string }) =>
    onChange({ workspaces: draft.workspaces.map((entry) => entry.path === path ? { ...entry, ...change } : entry) });
  return <fieldset className="bots-fields bots-folders" disabled={disabled}>
    <legend className="bots-label">{tr('bots_tab_folders')}</legend>
    <p className="bots-hint">{tr('bots_folders_hint')}</p>
    <ul className="bots-folder-list">
      {onNewChat ? <li className="bots-folder-own">
        <span className="bots-folder-name-text">{ownFolderTitle()}</span>
        <span className="bots-hint bots-folder-own-hint">{tr('bots_folder_own_hint')}</span>
        <button type="button" className="bots-folder-chat" data-variant="quiet" disabled={chatDisabled} aria-label={tr('bots_folder_new_chat_in', ownFolderTitle())}
          onClick={() => onNewChat(undefined)}>{tr('bots_new_chat')}</button>
      </li> : null}
      {draft.workspaces.map((folder) => {
        // A folder is one a chat can start in once it is saved, with the access it was saved with.
        const bound = saved?.find((entry) => entry.path === folder.path);
        return <li key={folder.path}>
          <input className="bots-folder-name" value={folder.label ?? ''} placeholder={folderBaseName(folder.path)} aria-label={tr('bots_folder_name', folder.path)}
            autoComplete="off" onChange={(event) => change(folder.path, { label: event.target.value })} />
          <select aria-label={tr('bots_folder_access', folder.path)} value={folder.mode}
            onChange={(event) => change(folder.path, { mode: event.target.value === 'write' ? 'write' : 'read' })}>
            <option value="read">{tr('bots_folder_read')}</option>
            <option value="write">{tr('bots_folder_write')}</option>
          </select>
          <button type="button" data-variant="quiet" aria-label={tr('bots_folder_remove', folder.path)}
            onClick={() => onChange({ workspaces: draft.workspaces.filter((entry) => entry.path !== folder.path) })}><Icon name="x" size={15} /></button>
          <span className="bots-folder-path mono" title={folder.path}>{folder.path}</span>
          {onNewChat && bound ? <button type="button" className="bots-folder-chat" data-variant="quiet" disabled={chatDisabled}
            aria-label={tr('bots_folder_new_chat_in', folder.label?.trim() || folderBaseName(folder.path))} onClick={() => onNewChat(bound.id)}>{tr('bots_new_chat')}</button> : null}
        </li>;
      })}
    </ul>
    {draft.workspaces.length === 0 ? <p className="bots-empty-line">{tr('bots_folders_empty')}</p> : null}
    <div><button type="button" disabled={picking || draft.workspaces.length >= BOT_FOLDERS_MAX} onClick={() => void add()}>{tr('bots_folders_add')}</button></div>
    {problems.folders ? <p className="bots-problem" role="alert">{problemText(problems.folders)}</p> : null}
    {error ? <p className="bots-problem" role="alert">{error}</p> : null}
  </fieldset>;
}

/**
 * Which of the owner's other Bots may hand this one tasks. `peers` are the other
 * Bots, or absent while the list of Bots has not been read, when nothing can be
 * chosen and the saved list is left as it is.
 */
export function BotAcceptField({ draft, problems, peers, disabled = false, onChange }: Pick<FieldsProps, 'draft' | 'problems' | 'disabled' | 'onChange'> & {
  peers: readonly BotUi[] | undefined;
}) {
  const chosen = new Set(draft.acceptFrom);
  const choose = (botId: string, on: boolean) => onChange({ acceptFrom: on ? [...draft.acceptFrom, botId] : draft.acceptFrom.filter((id) => id !== botId) });
  const sorted = [...(peers ?? [])].sort((left, right) => left.name.localeCompare(right.name) || left.botId.localeCompare(right.botId));
  return <fieldset className="bots-fields bots-accept" disabled={disabled}>
    <legend className="bots-label">{tr('bots_accept_title')}</legend>
    <p className="bots-hint">{tr('bots_accept_intro')}</p>
    {peers === undefined ? <p role="status" className="bots-muted">{tr('bots_accept_unknown')}</p> : null}
    {peers !== undefined && sorted.length === 0 ? <p className="bots-empty-line">{tr('bots_accept_empty')}</p> : null}
    {sorted.length > 0 ? <ul className="bots-accept-list">
      {sorted.map((peer) => <li key={peer.botId}>
        <label>
          <input type="checkbox" checked={chosen.has(peer.botId)} onChange={(event) => choose(peer.botId, event.target.checked)} />
          <span className="bots-accept-name">{peer.name}</span>
          {peer.description ? <span className="bots-muted bots-accept-description">{peer.description}</span> : null}
        </label>
      </li>)}
    </ul> : null}
    <ul className="bots-hint bots-hint-list">
      <li>{tr('bots_accept_none')}</li>
      <li>{tr('bots_accept_run')}</li>
      <li>{tr('bots_accept_files')}</li>
      <li>{tr('bots_accept_visible')}</li>
    </ul>
    {problems.accept ? <p className="bots-problem" role="alert">{problemText(problems.accept)}</p> : null}
  </fieldset>;
}
