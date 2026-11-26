import { useEffect, useRef, useState, type ReactElement } from 'react';
import { expiresIn } from './BotRunCards';
import { heldReplyErrorText } from './errors.js';
import { heldEdit, type HeldRow } from './held-replies.js';
import { useTeamsText } from './i18n.js';
import { Dialog, Icon, type RenderMarkdown } from './ui';

/**
 * A reply of a Bot that waits for its maintainer (Bot v3, B6), as that person reads it: the Bot's words and the document that goes with them, the time they have to
 * decide, and what they can do. They approve the reply as it is, change its text and send that, or reject it after being asked once more. Nobody else is shown any of it.
 *
 * Once the service says the reply is no longer there to decide (it was decided, its time ran out, a newer message dropped it) the card says so in its place, without the
 * words and without the buttons, and goes when it is closed.
 */
export function HeldReplyCard({ row, now, locale, renderMarkdown, heading = true, onApprove, onReject, onDismiss }: {
  row: HeldRow;
  /** The time the card is shown at, from the list: the time left is as new as it is. */
  now: number;
  locale: string; renderMarkdown?: RenderMarkdown;
  /** The line that says what this is. A card in a list is already under the Chat it belongs to, and leaves it out. */
  heading?: boolean;
  /** Let the reply go, with the maintainer's own text or (none) as the Bot made it. Whether the service took the decision. */
  onApprove(text?: string): Promise<boolean>;
  onReject(): Promise<boolean>;
  onDismiss(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const { reply } = row;
  const [editing, setEditing] = useState<string>(), [confirming, setConfirming] = useState(false);
  const editor = useRef<HTMLTextAreaElement>(null);
  const writing = editing !== undefined;
  useEffect(() => { if (writing) editor.current?.focus(); }, [writing]);
  // The time can run out on this screen before the service has been asked again: the card does not wait for it.
  const ended = row.ended ?? (reply.expiresAt <= now ? 'REPLY_EXPIRED' : undefined);
  if (ended !== undefined) {
    return <section className="team-bot-run tw-held" data-state="ended" aria-label={t('teams_held_title')}>
      <p className="tw-run-state" role="status"><span className="tw-run-glyph"><Icon name="clockSlash" /></span><span>{heldReplyErrorText(ended, locale)}</span></p>
      {row.ended !== undefined ? <div className="teams-row-actions"><button type="button" onClick={onDismiss}>{t('common_close')}</button></div> : null}
    </section>;
  }
  const busy = row.busy !== undefined;
  const edit = writing ? heldEdit(reply.text, editing) : undefined;
  // The question is answered once it is asked: what comes of the decision is told on the card, which is the one thing on screen that can say it.
  const reject = () => { setConfirming(false); void onReject(); };
  return <section className="team-bot-run tw-held" data-state="held" data-busy={row.busy} aria-label={t('teams_held_title')} aria-busy={busy || undefined}>
    {heading ? <p className="tw-run-state" role="status"><span className="tw-run-glyph"><Icon name="clock" /></span><span>{t('teams_held_title')}</span></p> : null}
    <p className="tw-run-detail">{t('teams_held_expires_fmt', expiresIn(reply.expiresAt, now, locale))}{reply.unasked ? <> <span className="tw-tag">{t('teams_held_unasked')}</span></> : null}</p>
    {writing
      ? <textarea ref={editor} className="tw-held-edit" aria-label={t('teams_held_edit_label')} aria-invalid={edit && !edit.ok ? true : undefined} rows={Math.min(12, Math.max(3, editing.split('\n').length + 1))}
        value={editing} disabled={busy} onChange={event => setEditing(event.target.value)}
        onKeyDown={event => { if (event.key === 'Escape' && !busy) { event.preventDefault(); event.stopPropagation(); setEditing(undefined); } }} />
      : renderMarkdown ? <div className="tw-held-text tw-md" data-markdown="true">{renderMarkdown(reply.text)}</div> : <p className="tw-held-text">{reply.text}</p>}
    {reply.document ? <p className="tw-run-detail tw-held-document"><Icon name="file" size={13} /> {t('teams_held_document_fmt', reply.document.title)}</p> : null}
    {edit && !edit.ok ? <p className="tw-run-detail" role="status">{t(edit.problem === 'empty' ? 'teams_held_empty_text' : 'teams_held_too_long')}</p> : null}
    {writing
      ? <div className="teams-row-actions">
        <button type="button" disabled={busy || !edit?.ok} onClick={() => void onApprove(edit?.ok ? edit.text : undefined)}>{t('teams_message_send')}</button>
        <button type="button" disabled={busy} onClick={() => setEditing(undefined)}>{t('common_cancel')}</button>
      </div>
      : <div className="teams-row-actions">
        <button type="button" disabled={busy} onClick={() => void onApprove()}>{t('teams_approval_approve')}</button>
        <button type="button" disabled={busy} onClick={() => setEditing(reply.text)}>{t('teams_message_edit')}</button>
        <button type="button" className="tw-danger-btn" disabled={busy} onClick={() => setConfirming(true)}>{t('teams_held_reject')}</button>
      </div>}
    {row.error !== undefined ? <p role="alert">{heldReplyErrorText(row.error, locale)}</p> : null}
    {confirming ? <Dialog role="alertdialog" title={t('teams_held_reject_confirm')} description={t('teams_held_reject_detail')} onClose={() => setConfirming(false)}>
      <div className="tw-form">
        <div className="tw-dialog-actions">
          <button type="button" data-autofocus onClick={() => setConfirming(false)}>{t('common_cancel')}</button>
          <button type="button" className="tw-danger-btn" disabled={busy} onClick={reject}>{t('teams_held_reject')}</button>
        </div>
      </div>
    </Dialog> : null}
  </section>;
}
