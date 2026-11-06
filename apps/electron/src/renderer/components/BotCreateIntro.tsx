import { useId, useRef, useState } from 'react';
import { BOT_CREATION_REQUEST_MAX } from '../bridge/botCreation';
import { ipcErrorMessage } from '../bridge/botForm';
import { tr } from '../i18n/locale';
import { Icon } from './Icon';
import './BotCreation.css';

export function BotCreateIntro({ request, onChange, onContinue }: {
  request: string;
  onChange(value: string): void;
  onContinue(request: string): Promise<void>;
}) {
  const id = useId();
  const input = useRef<HTMLTextAreaElement>(null);
  const pending = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  return <form className="bot-create-intro" aria-busy={busy || undefined} onSubmit={async event => {
    event.preventDefault();
    if (!request.trim() || pending.current) return;
    pending.current = true; setBusy(true); setError('');
    try { await onContinue(request); }
    catch (cause) { setError(ipcErrorMessage(cause) || tr('bots_assist_open_failed')); }
    finally { pending.current = false; setBusy(false); }
  }}>
    <div className="bot-create-intro-icon" aria-hidden="true"><Icon name="bot" size={30} stroke={1.5} /></div>
    <div className="bot-create-intro-copy"><h2>{tr('bots_assist_title')}</h2><p>{tr('bots_assist_body')}</p></div>
    <div className="bots-field">
      <label className="bots-label" htmlFor={id}>{tr('bots_assist_request')}</label>
      <textarea id={id} ref={input} rows={5} value={request} maxLength={BOT_CREATION_REQUEST_MAX} disabled={busy}
        placeholder={tr('bots_assist_placeholder')} aria-describedby={`${id}-hint`}
        onChange={event => { onChange(event.target.value); setError(''); }} />
      <p className="bots-hint" id={`${id}-hint`}>{tr('bots_assist_chat_hint')}</p>
    </div>
    <div className="bot-create-examples" aria-label={tr('bots_assist_examples')}>
      {['writing', 'research', 'code'].map(example => <button key={example} type="button" disabled={busy}
        onClick={() => { onChange(tr(`bots_assist_example_${example}`)); input.current?.focus(); }}>{tr(`bots_assist_example_${example}_label`)}</button>)}
    </div>
    <div className="bot-create-intro-footer">
      <span className="bots-hint">{tr('bots_assist_steps')}</span>
      <button type="submit" data-variant="primary" disabled={busy || !request.trim()}>{tr(busy ? 'bots_assist_opening' : 'bots_assist_continue')}</button>
    </div>
    {error ? <p role="alert" className="bots-problem">{error}</p> : null}
  </form>;
}
