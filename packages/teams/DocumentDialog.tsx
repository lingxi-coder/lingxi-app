import { useRef, useState } from 'react';
import { DOCUMENT_BYTES } from './execution-validation.js';
import { executionCommand, type ArtifactRevised, type ArtifactRevisions, type ExecutionScope, type RevisionAuthor, type TeamArtifact } from './execution.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog, type RenderMarkdown } from './ui';
import type { TeamsSnapshot } from './workspace.js';
import { intlTag, useTeamsText } from './i18n.js';

/** Who the history names: team members and the Bots that write documents. */
export type DocumentPeople = Pick<TeamsSnapshot, 'bots' | 'directory'>;
interface Draft { title: string; body: string }
const failure = (error: unknown) => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';

/**
 * A shared document: read it, edit it, and look through its history.
 * An edit is saved only on top of the revision it was made from. When someone saved first, the text being
 * edited stays in the editor next to the newer version, and saving again is a decision the person makes.
 */
export function DocumentDialog({ api, scope, first, locale, renderMarkdown, people, onClose }: {
  api: TeamsApi; scope: ExecutionScope; first: TeamArtifact; locale: string; renderMarkdown?: RenderMarkdown; people?: DocumentPeople; onClose(): void;
}) {
  const [shown, setShown] = useState(first), [draft, setDraft] = useState<Draft>();
  /** The newer revision that refused the draft; while set, the draft is saved on top of it. */
  const [newer, setNewer] = useState<TeamArtifact>();
  const [history, setHistory] = useState<ArtifactRevisions>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [leaving, setLeaving] = useState(false);
  /** One operation id per exact edit, so repeating a save whose answer was lost creates no second revision. */
  const operation = useRef<{ key: string; id: string }>();
  const t = useTeamsText(locale);
  const artifactId = first.artifactId, base = newer ?? shown;
  const read = (revision?: number) => api.messages<TeamArtifact>(executionCommand(scope, 'readArtifact', { artifactId, ...(revision === undefined ? {} : { revision }) }));
  const attempt = async (work: () => Promise<void>) => {
    if (busy) return; setBusy(true); setError('');
    try { await work(); } catch (e) { setError(failure(e)); } finally { setBusy(false); }
  };
  const title = draft?.title.trim() ?? '', bytes = draft ? new TextEncoder().encode(draft.body).length : 0;
  const changed = !!draft && (title !== base.document.title || draft.body !== base.document.body);
  const valid = title.length >= 1 && title.length <= 120 && bytes >= 1 && bytes <= DOCUMENT_BYTES;
  const save = () => attempt(async () => {
    if (!draft || !changed || !valid) return;
    const document = { title, body: draft.body }, key = JSON.stringify([base.revision, document]);
    if (operation.current?.key !== key) operation.current = { key, id: crypto.randomUUID() };
    try {
      await api.messages<ArtifactRevised>(executionCommand(scope, 'reviseArtifact', { artifactId, expectedRevision: base.revision, operationId: operation.current.id, document }));
    } catch (e) {
      if (failure(e) !== 'ARTIFACT_REVISION_CONFLICT') throw e;
      setNewer(await read()); return;
    }
    setShown(await read()); setDraft(undefined); setNewer(undefined);
  });
  const stopEditing = () => { setDraft(undefined); setError(''); setLeaving(false); if (newer) { setShown(newer); setNewer(undefined); } };
  const close = () => { if (changed) setLeaving(true); else onClose(); };
  const author = (who: RevisionAuthor) => who.kind === 'user'
    ? people?.directory.find(member => member.uid === who.uid)?.displayName || t('teams_member')
    : people?.bots.find(bot => bot.sharedBotId === who.sharedBotId)?.name || t('teams_bot_tag');
  const when = (ms: number) => new Intl.DateTimeFormat(intlTag(locale), { dateStyle: 'medium', timeStyle: 'short' }).format(ms);
  const body = (text: string) => renderMarkdown ? <div className="tw-md">{renderMarkdown(text)}</div> : <div style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{text}</div>;
  const alert = error ? <p role="alert">{messageErrorText(error, locale)}</p> : null;

  if (draft) return <Dialog title={t('teams_doc_edit_title')} onClose={close}>
    <form className="tw-form tw-doc-edit" onSubmit={event => { event.preventDefault(); void save(); }}>
      <label>{t('teams_doc_field_title')}<input value={draft.title} maxLength={120} onChange={event => setDraft({ ...draft, title: event.target.value })} /></label>
      <label>{t('teams_doc_field_body')}<textarea data-autofocus rows={14} value={draft.body} onChange={event => setDraft({ ...draft, body: event.target.value })} /></label>
      {bytes > DOCUMENT_BYTES ? <p role="alert">{t('teams_doc_too_long')}</p> : null}
      {newer ? <section className="tw-doc-conflict" aria-label={t('teams_doc_newer')}>
        <p role="alert">{t('tw_doc_conflict_fmt', newer.revision)}</p>
        <h3>{newer.document.title}</h3>{body(newer.document.body)}
      </section> : null}
      {leaving ? <p role="alert">{t('teams_doc_unsaved')}</p> : null}
      {alert}
      <div className="tw-dialog-actions">
        {leaving ? <><button type="button" onClick={() => setLeaving(false)}>{t('teams_doc_keep_editing')}</button><button type="button" className="tw-danger-btn" onClick={onClose}>{t('teams_doc_discard')}</button></>
          : <><button type="button" onClick={stopEditing}>{newer ? t('teams_doc_discard_mine') : t('common_cancel')}</button>
            <button type="submit" className="tw-primary" disabled={busy || !changed || !valid}>{busy ? t('teams_doc_saving') : newer ? t('teams_doc_save_over_fmt', newer.revision) : t('common_save')}</button></>}
      </div>
    </form>
  </Dialog>;

  if (history) return <Dialog title={t('teams_doc_history_title')} onClose={onClose}>
    <ol className="tw-doc-history">{history.revisions.map(row => <li key={row.revision}>
      <button type="button" disabled={busy} aria-label={t('tw_doc_open_revision_fmt', row.revision)}
        onClick={() => void attempt(async () => { setShown(await read(row.revision)); setHistory(undefined); })}>{t('tw_doc_revision')} {row.revision}</button>
      <span>{row.title}</span>
      <span className="tw-hint">{author(row.author)} · {when(row.createdAt)}{row.revision === history.currentRevision ? ` · ${t('teams_doc_current')}` : ''}</span>
    </li>)}</ol>
    {alert}
    <div className="tw-dialog-actions"><button type="button" onClick={() => { setHistory(undefined); setError(''); }}>{t('remote_back')}</button></div>
  </Dialog>;

  const old = shown.revision < shown.currentRevision;
  return <Dialog title={shown.document.title} onClose={onClose}><article className="tw-artifact">
    <p className="tw-hint">{old ? t('tw_doc_revision_old_fmt', shown.revision, shown.currentRevision) : t('teams_doc_revision_fmt', shown.revision)}</p>
    {body(shown.document.body)}
    {alert}
    <div className="tw-dialog-actions">
      {old ? <button type="button" disabled={busy} onClick={() => void attempt(async () => setShown(await read()))}>{t('teams_doc_show_latest')}</button>
        : <button type="button" disabled={busy} onClick={() => { setError(''); setDraft({ ...shown.document }); }}>{t('teams_doc_edit')}</button>}
      <button type="button" disabled={busy} onClick={() => void attempt(async () => setHistory(await api.messages<ArtifactRevisions>(executionCommand(scope, 'listArtifactRevisions', { artifactId }))))}>{t('teams_doc_history')}</button>
      <button type="button" onClick={onClose}>{t('common_close')}</button>
    </div>
  </article></Dialog>;
}
