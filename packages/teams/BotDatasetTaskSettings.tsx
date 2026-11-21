import { useId, useRef, useState, type FormEvent } from 'react';
import type { DatasetBotScope } from './dataset-run.js';
import type { TeamsApi } from './index.js';
import { useTeamsText } from './i18n.js';

export function BotDatasetTaskSettings({ api, scope, name, locale }: {
  api: TeamsApi; scope: DatasetBotScope; name: string; locale: string;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useId();
  const operation = useRef<{ payload: string; id: string }>();
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; error?: boolean }>();
  const t = useTeamsText(locale);
  const approve = api.approveDatasetTask;
  const revoke = api.revokeBotDatasetTask;
  if (!approve || !revoke) return null;
  const label = t('tw_dataset_bot_permission');

  async function buildApproval(event: FormEvent<HTMLFormElement>): Promise<void> {
    event.preventDefault();
    if (busy) return;
    const fields = new FormData(event.currentTarget);
    const dataset = String(fields.get('dataset') ?? '').trim();
    const source = String(fields.get('plan') ?? '');
    let plan: Record<string, unknown>;
    try {
      const value: unknown = JSON.parse(source);
      if (!value || typeof value !== 'object' || Array.isArray(value)) throw new TypeError();
      plan = value as Record<string, unknown>;
    } catch {
      setStatus({ text: t('desktop_dataset_task_invalid_plan'), error: true });
      return;
    }
    const payload = JSON.stringify([dataset, source]);
    if (operation.current?.payload !== payload) operation.current = { payload, id: crypto.randomUUID() };
    setBusy(true);
    setStatus(undefined);
    try {
      await approve!({ ...scope, executor: 'teamBot', dataset, plan, operation: operation.current.id });
      setStatus({ text: t('desktop_dataset_task_approved') });
    } catch (failure) {
      setStatus({ text: failure instanceof Error ? failure.message : String(failure), error: true });
    } finally { setBusy(false); }
  }

  async function revokeApproval(): Promise<void> {
    if (busy) return;
    setBusy(true);
    setStatus(undefined);
    try {
      await revoke!(scope);
      operation.current = undefined;
      setStatus({ text: t('desktop_dataset_task_revoked') });
    } catch (failure) {
      setStatus({ text: failure instanceof Error ? failure.message : String(failure), error: true });
    } finally { setBusy(false); }
  }

  return <>
    <button type="button" className="tw-link" onClick={() => dialog.current?.showModal()}>{label}</button>
    <dialog ref={dialog} className="tw-dialog tw-data-record" aria-labelledby={heading} onCancel={event => { if (busy) event.preventDefault(); }}>
      <div className="tw-dialog-panel">
        <h2 id={heading}>{label} · {name}</h2>
        <p>{t('tw_dataset_bot_permission_scope')}</p>
        <form onSubmit={event => void buildApproval(event)}>
          <fieldset disabled={busy}>
            <label>{t('desktop_dataset_task_id')} <input name="dataset" required pattern="[A-Za-z0-9_\-]{1,64}" maxLength={64} /></label>
            <label>{t('desktop_dataset_task_plan')}
              <textarea name="plan" required rows={10} maxLength={88_000} spellCheck={false} /></label>
          </fieldset>
          <button type="submit" className="tw-primary" disabled={busy}>{t('desktop_dataset_task_review')}</button>
          <button type="button" className="tw-link" disabled={busy} onClick={() => void revokeApproval()}>{t('desktop_dataset_task_revoke')}</button>
        </form>
        {status && <p role={status.error ? 'alert' : 'status'}>{status.text}</p>}
        <form method="dialog"><button type="submit" className="tw-btn" disabled={busy}>{t('common_close')}</button></form>
      </div>
    </dialog>
  </>;
}
