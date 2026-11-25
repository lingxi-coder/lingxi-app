import { useState, type ReactElement } from 'react';
import { inBotChat } from './AutoReplyDialog';
import { botScopeLimits, choiceOf, effectiveBotScope, emptyBotScope, narrowingOf, type BotScopeChoice, type BotScopeNarrowing } from './bot-scope.js';
import { messageErrorText } from './errors.js';
import { executionCommand, type ExecutionScope, type SharedBotCard } from './execution.js';
import { useTeamsText } from './i18n.js';
import type { TeamConversation } from './messages.js';
import { Dialog } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

/** The Bots of a Chat that share something of their maintainer's computer: these are the ones a person in the Chat can limit. */
export function scopedBots(conversation: TeamConversation, snapshot: Pick<TeamsSnapshot, 'bots' | 'uid'>): SharedBotCard[] {
  return inBotChat(conversation, snapshot.uid)
    ? snapshot.bots.filter(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`) && !!(bot.scope?.localApps.length || bot.scope?.datasets.length))
    : [];
}

const sameIds = (a: string[], b: string[]): boolean => a.length === b.length && a.every(id => b.includes(id));
const toggled = (ids: string[], id: string, on: boolean): string[] => on ? [...ids.filter(value => value !== id), id] : ids.filter(value => value !== id);

/**
 * What each Bot of this Chat may use here, of what its maintainer shared: a person in the Chat unticks what a Bot should not use in it. Saving names
 * what stays allowed; allowing everything again lifts the limit. A change someone made meanwhile, or by the maintainer, is not written over:
 * what the Bot may use now is shown again, with the reason.
 */
export function BotScopeDialog({ workspace, snapshot, conversation, scope, locale, onClose }: {
  workspace: TeamsWorkspace; snapshot: TeamsSnapshot; conversation: TeamConversation; scope: ExecutionScope; locale: string; onClose(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const bots = scopedBots(conversation, snapshot);
  /** The last save or reset: `code` is empty when it was written. */
  const [result, setResult] = useState<{ sharedBotId: string; code: string }>();
  const send = async (bot: SharedBotCard, narrowing: BotScopeNarrowing | null) => {
    setResult(undefined);
    let code = '';
    const written = await workspace.run(async () => {
      try {
        await workspace.api.messages(executionCommand(scope, 'setConversationBotScope', { conversationId: conversation.id, sharedBotId: bot.sharedBotId,
          expectedRevision: conversation.botScopes?.[bot.sharedBotId]?.revision ?? 0, scope: narrowing }));
      } catch (error) { code = error instanceof Error ? error.message : 'UNKNOWN'; throw error; }
    });
    if (written) { setResult({ sharedBotId: bot.sharedBotId, code: '' }); return; }
    if (!code) return;
    setResult({ sharedBotId: bot.sharedBotId, code });
    // What the Bot shares or what the Chat holds changed under this person: the Bots are read again, and the Chat's side arrives by itself.
    if (code === 'SCOPE_NOT_PUBLISHED' || code === 'BOT_SCOPE_REVISION_CONFLICT') void workspace.refreshBots();
  };
  return <Dialog title={t('teams_bot_scope_title')} onClose={onClose}>
    <div className="tw-form">
      <p className="tw-hint">{t('teams_bot_scope_note')}</p>
      {bots.map(bot => <BotScopeEditor key={bot.sharedBotId} bot={bot} conversation={conversation} busy={snapshot.messageBusy} locale={locale}
        result={result?.sharedBotId === bot.sharedBotId ? result.code : undefined} onEdit={() => setResult(undefined)} onSend={narrowing => void send(bot, narrowing)} />)}
      <div className="tw-dialog-actions"><button type="button" onClick={onClose}>{t('common_close')}</button></div>
    </div>
  </Dialog>;
}

/** One Bot: what it shares as checkboxes, ticked where this Chat allows it. What is shown follows the Chat and the Bot as they are now. */
function BotScopeEditor({ bot, conversation, busy, locale, result, onEdit, onSend }: {
  bot: SharedBotCard; conversation: TeamConversation; busy: boolean; locale: string; result: string | undefined; onEdit(): void; onSend(narrowing: BotScopeNarrowing | null): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const published = bot.scope ?? emptyBotScope(), entry = conversation.botScopes?.[bot.sharedBotId];
  const allowed = choiceOf(effectiveBotScope(published, entry));
  // The ticks start from what the Chat allows. When that changes (a save, someone else, the maintainer), they start again from it.
  const now = JSON.stringify([entry?.revision ?? 0, published, allowed]);
  const [seen, setSeen] = useState(now), [choice, setChoice] = useState<BotScopeChoice>(allowed);
  if (seen !== now) { setSeen(now); setChoice(allowed); }
  const changed = !sameIds(choice.localApps, allowed.localApps) || !sameIds(choice.datasets, allowed.datasets);
  const edit = (next: BotScopeChoice) => { setChoice(next); onEdit(); };
  return <fieldset className="tw-bot-scope-edit">
    <legend>{t('teams_bot_scope_limit_title_fmt', bot.name)}</legend>
    {published.localApps.length ? <fieldset className="tw-bot-scope-group"><legend>{t('teams_bot_scope_apps')}</legend>
      {published.localApps.map(app => <label key={app.id} className="tw-check">
        <input type="checkbox" checked={choice.localApps.includes(app.id)} disabled={busy} onChange={event => edit({ ...choice, localApps: toggled(choice.localApps, app.id, event.target.checked) })} />{app.name}
      </label>)}</fieldset> : null}
    {published.datasets.length ? <fieldset className="tw-bot-scope-group"><legend>{t('teams_bot_scope_datasets')}</legend>
      {published.datasets.map(row => <label key={row.dataset} className="tw-check">
        <input type="checkbox" checked={choice.datasets.includes(row.dataset)} disabled={busy} onChange={event => edit({ ...choice, datasets: toggled(choice.datasets, row.dataset, event.target.checked) })} />
        {row.dataset}{row.write ? <span className="tw-tag">{t('teams_bot_scope_edit')}</span> : null}
      </label>)}</fieldset> : null}
    {result === '' ? <p role="status" className="tw-hint">{t('teams_bot_scope_saved')}</p> : null}
    {result ? <p role="alert" className="tw-error">{messageErrorText(result, locale)}</p> : null}
    <div className="tw-dialog-actions">
      <button type="button" disabled={busy || !botScopeLimits(entry)} onClick={() => onSend(null)}>{t('teams_bot_scope_reset')}</button>
      <button type="button" className="tw-primary" disabled={busy || !changed} onClick={() => onSend(narrowingOf(published, choice))}>{t('common_save')}</button>
    </div>
  </fieldset>;
}
