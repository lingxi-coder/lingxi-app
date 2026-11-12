import { useRef, useState } from 'react';
import { DOCUMENT_BYTES } from './execution-validation.js';
import { executionCommand, type ArtifactRevised, type ArtifactRevisions, type ExecutionScope, type RevisionAuthor, type TeamArtifact } from './execution.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog, type RenderMarkdown } from './ui';
import type { TeamsSnapshot } from './workspace.js';

/** Who the history names: team members and the Bots that write documents. */
export type DocumentPeople = Pick<TeamsSnapshot, 'bots' | 'directory'>;
interface Draft { title: string; body: string }
const failure = (error: unknown) => error instanceof Error ? error.message : 'SERVICE_UNAVAILABLE';

/**
 * A shared document: read it, edit it, and look through its history.
 * An edit is saved only on top of the revision it was made from. When someone saved first, the text being
 * edited stays in the editor next to the newer version, and saving again is a decision the person makes.
 */
export function DocumentDialog({ api, scope, first, zh, renderMarkdown, people, onClose }: {
  api: TeamsApi; scope: ExecutionScope; first: TeamArtifact; zh: boolean; renderMarkdown?: RenderMarkdown; people?: DocumentPeople; onClose(): void;
}) {
  const [shown, setShown] = useState(first), [draft, setDraft] = useState<Draft>();
  /** The newer revision that refused the draft; while set, the draft is saved on top of it. */
  const [newer, setNewer] = useState<TeamArtifact>();
  const [history, setHistory] = useState<ArtifactRevisions>();
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [leaving, setLeaving] = useState(false);
  /** One operation id per exact edit, so repeating a save whose answer was lost creates no second revision. */
  const operation = useRef<{ key: string; id: string }>();
  const t = (en: string, cn: string) => zh ? cn : en;
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
    ? people?.directory.find(member => member.uid === who.uid)?.displayName || t('Member', '成员')
    : people?.bots.find(bot => bot.sharedBotId === who.sharedBotId)?.name || t('Assistant', '助手');
  const when = (ms: number) => new Intl.DateTimeFormat(zh ? 'zh-CN' : 'en', { dateStyle: 'medium', timeStyle: 'short' }).format(ms);
  const body = (text: string) => renderMarkdown ? <div className="tw-md">{renderMarkdown(text)}</div> : <div style={{ whiteSpace: 'pre-wrap', overflowWrap: 'anywhere' }}>{text}</div>;
  const alert = error ? <p role="alert">{messageErrorText(error, zh)}</p> : null;

  if (draft) return <Dialog title={t('Edit document', '编辑文档')} onClose={close}>
    <form className="tw-form tw-doc-edit" onSubmit={event => { event.preventDefault(); void save(); }}>
      <label>{t('Title', '标题')}<input value={draft.title} maxLength={120} onChange={event => setDraft({ ...draft, title: event.target.value })} /></label>
      <label>{t('Content (Markdown)', '内容（Markdown）')}<textarea data-autofocus rows={14} value={draft.body} onChange={event => setDraft({ ...draft, body: event.target.value })} /></label>
      {bytes > DOCUMENT_BYTES ? <p role="alert">{t('This document is too long to save. Shorten it and try again.', '文档过长，无法保存。请缩短后重试。')}</p> : null}
      {newer ? <section className="tw-doc-conflict" aria-label={t('Newer version', '较新的版本')}>
        <p role="alert">{t(`This document was changed while you were editing (now revision ${newer.revision}). Your text is still above and has not been saved. Compare it with the newer version below, then save or discard your changes.`,
          `在你编辑期间，文档已被修改（现为版本 ${newer.revision}）。你的内容仍在上方，尚未保存。请对照下方的较新版本，然后保存或放弃你的修改。`)}</p>
        <h3>{newer.document.title}</h3>{body(newer.document.body)}
      </section> : null}
      {leaving ? <p role="alert">{t('You have unsaved changes.', '你有尚未保存的修改。')}</p> : null}
      {alert}
      <div className="tw-dialog-actions">
        {leaving ? <><button type="button" onClick={() => setLeaving(false)}>{t('Keep editing', '继续编辑')}</button><button type="button" className="tw-danger-btn" onClick={onClose}>{t('Discard changes', '放弃修改')}</button></>
          : <><button type="button" onClick={stopEditing}>{newer ? t('Discard my changes', '放弃我的修改') : t('Cancel', '取消')}</button>
            <button type="submit" className="tw-primary" disabled={busy || !changed || !valid}>{busy ? t('Saving…', '正在保存…') : newer ? t(`Save over revision ${newer.revision}`, `覆盖版本 ${newer.revision} 并保存`) : t('Save', '保存')}</button></>}
      </div>
    </form>
  </Dialog>;

  if (history) return <Dialog title={t('Document history', '文档历史')} onClose={onClose}>
    <ol className="tw-doc-history">{history.revisions.map(row => <li key={row.revision}>
      <button type="button" disabled={busy} aria-label={t(`Open revision ${row.revision}`, `打开版本 ${row.revision}`)}
        onClick={() => void attempt(async () => { setShown(await read(row.revision)); setHistory(undefined); })}>{t('Revision', '版本')} {row.revision}</button>
      <span>{row.title}</span>
      <span className="tw-hint">{author(row.author)} · {when(row.createdAt)}{row.revision === history.currentRevision ? ` · ${t('current', '当前')}` : ''}</span>
    </li>)}</ol>
    {alert}
    <div className="tw-dialog-actions"><button type="button" onClick={() => { setHistory(undefined); setError(''); }}>{t('Back', '返回')}</button></div>
  </Dialog>;

  const old = shown.revision < shown.currentRevision;
  return <Dialog title={shown.document.title} onClose={onClose}><article className="tw-artifact">
    <p className="tw-hint">{t('Revision', '版本')} {shown.revision}{old ? t(` of ${shown.currentRevision} · an earlier version`, ` / ${shown.currentRevision} · 较早的版本`) : ''}</p>
    {body(shown.document.body)}
    {alert}
    <div className="tw-dialog-actions">
      {old ? <button type="button" disabled={busy} onClick={() => void attempt(async () => setShown(await read()))}>{t('Show latest', '查看最新版本')}</button>
        : <button type="button" disabled={busy} onClick={() => { setError(''); setDraft({ ...shown.document }); }}>{t('Edit', '编辑')}</button>}
      <button type="button" disabled={busy} onClick={() => void attempt(async () => setHistory(await api.messages<ArtifactRevisions>(executionCommand(scope, 'listArtifactRevisions', { artifactId }))))}>{t('History', '历史')}</button>
      <button type="button" onClick={onClose}>{t('Close', '关闭')}</button>
    </div>
  </article></Dialog>;
}
