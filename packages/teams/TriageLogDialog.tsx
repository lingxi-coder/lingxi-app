import { useEffect, useState, type ReactElement } from 'react';
import type { ExecutionScope, SharedBotCard, TriageAction } from './execution.js';
import type { TeamsApi } from './index.js';
import { messageErrorText } from './errors.js';
import { Dialog, Icon, type IconName } from './ui';
import { intlTag, useTeamsText } from './i18n.js';
import { useTriageLog } from './triage-hooks.js';
import { triageActionText, triageApprovalText, triageOutcome, triageOutcomeText, triageProbabilityText, triageReasonText, triageRoundText } from './triage-log.js';

const VERDICT_ICON: Record<TriageAction, IconName> = { decline: 'pauseCircle', run: 'playCircle', run_may_decline: 'circleAlert' };
/** A time in epoch milliseconds, as the maintainer reads dates: the day and the hour, in their language and their time zone. */
const logTime = (at: number, locale: string): string => new Intl.DateTimeFormat(intlTag(locale), { dateStyle: 'short', timeStyle: 'short' }).format(at);

/**
 * What a Bot decided about the messages nobody asked it about, for the person who maintains it: newest first, a page at a time, in every Chat of
 * the Team or (with `conversation`) in one. Only the decision and the reason are kept, never what a model said. With several `bots` the person picks
 * which one they read. When the service says they do not maintain the Bot after all, `onDenied` takes it out of what is offered.
 */
export function TriageLogDialog({ api, scope, bots, conversation, locale, onClose, onDenied }: {
  api: TeamsApi; scope: ExecutionScope; bots: readonly Pick<SharedBotCard, 'sharedBotId' | 'name'>[];
  /** Only the verdicts in this Chat; `title` is how the person knows it. */
  conversation?: { id: string; title: string };
  locale: string; onClose(): void; onDenied?(sharedBotId: string): void;
}): ReactElement | null {
  const t = useTeamsText(locale);
  const [chosen, setChosen] = useState(bots[0]?.sharedBotId);
  const bot = bots.find(candidate => candidate.sharedBotId === chosen) ?? bots[0];
  const { view, refresh, more } = useTriageLog(api, scope, { sharedBotId: bot?.sharedBotId ?? '', ...(conversation ? { conversationId: conversation.id } : {}) });
  const denied = view.status === 'denied';
  useEffect(() => { if (denied && bot) onDenied?.(bot.sharedBotId); }, [denied, bot?.sharedBotId]); // eslint-disable-line react-hooks/exhaustive-deps
  if (!bot) return null;
  const busy = view.status === 'loading' || view.status === 'more';
  return <Dialog title={t('teams_triage_title')} onClose={onClose}><div className="tw-form tw-triage">
    <p className="tw-hint">{t('teams_triage_intro')}</p>
    {bots.length > 1
      ? <label>{t('teams_bot_tag')}<select value={bot.sharedBotId} onChange={event => setChosen(event.target.value)}>{bots.map(candidate => <option key={candidate.sharedBotId} value={candidate.sharedBotId}>{candidate.name}</option>)}</select></label>
      : <p className="tw-triage-scope"><b>{bot.name}</b></p>}
    {conversation ? <p className="tw-hint">{conversation.title}</p> : null}
    {view.status === 'loading' ? <p role="status">{t('teams_triage_loading')}</p> : null}
    {view.status === 'ready' && !view.items.length ? <p className="tw-hint">{t('teams_triage_empty')}</p> : null}
    {view.items.length ? <ul className="tw-triage-list" aria-label={t('teams_triage_title')}>{view.items.map(item => {
      const outcome = triageOutcome(item.state), why = triageReasonText(t, item.triage.reason), decided = triageApprovalText(t, item.approval), round = triageRoundText(t, item.round);
      return <li key={item.taskId} className="tw-triage-row" data-action={item.triage.action}>
        <span className="tw-triage-icon" aria-hidden="true"><Icon name={VERDICT_ICON[item.triage.action]} /></span>
        <div className="tw-triage-main">
          <span className="tw-triage-title">{triageActionText(t, item.triage.action)}
            {why ? <span className="tw-triage-why">{why}</span> : null}
            {item.triage.probability !== undefined ? <span className="tw-tag">{triageProbabilityText(t, item.triage.probability)}</span> : null}</span>
          <span className="tw-triage-meta">{item.conversationTitle || t('teams_triage_chat_untitled')} · <time dateTime={new Date(item.triage.at).toISOString()}>{logTime(item.triage.at, locale)}</time>{decided ? ` · ${decided}` : ''}{round ? ` · ${round}` : ''}</span>
        </div>
        <span className="tw-tag" data-outcome={outcome}>{triageOutcomeText(t, outcome)}</span>
      </li>;
    })}</ul> : null}
    {view.status === 'failed' && view.error ? <p role="alert">{messageErrorText(view.error, locale)}</p> : null}
    {view.status === 'failed' && !view.items.length ? <div><button type="button" onClick={refresh}>{t('common_retry')}</button></div> : null}
    {view.nextCursor !== undefined && !denied ? <div><button type="button" disabled={busy} aria-busy={view.status === 'more' || undefined} onClick={more}>{view.status === 'more' ? t('teams_triage_loading') : t('teams_triage_more')}</button></div> : null}
    <div className="tw-dialog-actions">
      <button type="button" disabled={busy} onClick={refresh}>{t('teams_list_refresh')}</button>
      <button type="button" data-autofocus onClick={onClose}>{t('common_close')}</button>
    </div>
  </div></Dialog>;
}
