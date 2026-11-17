import { sharedBotUsable, type SharedBotCard } from './execution.js';
import { teamsText, tk, useTeamsText, type TeamsKey } from './i18n.js';

/** What a member is told about a Bot's availability. `paused` is how the card of a Bot whose sharing was stopped reads (U1: "Sharing stopped"). */
const AVAILABILITY: Record<NonNullable<SharedBotCard['availability']>, TeamsKey> = {
  ready: tk('tw_state_ready'),
  paused: tk('teams_bot_disabled'),
  offline: tk('tw_bot_avail_offline'),
  maintenance: tk('tw_bot_avail_maintenance'),
  quota_exhausted: tk('tw_bot_avail_quota'),
  unverified: tk('tw_bot_avail_unverified'),
};

/** Member projections only: never derive health from a host's private failure reason. A value this build does not know reads as unverified. */
export function botAvailabilityText(bot: SharedBotCard, locale: string): string {
  const state = bot.state === 'paused' ? 'paused' : bot.availability ?? 'unverified';
  return teamsText(locale)(Object.prototype.hasOwnProperty.call(AVAILABILITY, state) ? AVAILABILITY[state] : AVAILABILITY.unverified);
}

export function BotMemberDetails({ bot, locale, onExample, busy = false, canUseExample = sharedBotUsable(bot), showAvailability = true }: { bot: SharedBotCard; locale: string; onExample?(example: string): void; busy?: boolean; canUseExample?: boolean; showAvailability?: boolean }) {
  const t = useTeamsText(locale);
  const supported = (value: boolean | undefined) => value === undefined ? t('chat_status_unknown') : value ? t('tw_bot_supported') : t('tw_bot_not_supported');
  const resetDate = bot.quotaResetsAt === undefined ? undefined : new Date(bot.quotaResetsAt);
  const reset = resetDate && Number.isFinite(resetDate.getTime()) ? resetDate.toISOString().replace('T', ' ').replace('.000Z', ' UTC') : t('chat_status_unknown');
  return <div className="tw-bot-member-details">
    {showAvailability ? <p>{botAvailabilityText(bot, locale)}</p> : null}
    <p className="teams-hint">{t('tw_bot_wait_resume')}: {supported(bot.capabilities?.durableWait)} · {t('tw_bot_write_ops')}: {supported(bot.capabilities?.writeOperations)}</p>
    <p className="teams-hint">{t('tw_bot_remaining_today')}: {bot.dailyRemaining ?? t('chat_status_unknown')} · {t('tw_bot_resets')}: {reset}</p>
    {onExample && bot.examples?.length ? <><p className="teams-hint">{t('tw_bot_examples_hint')}</p><ul role="list">{bot.examples.slice(0, 3).map((example, index) => <li key={index}><button type="button" disabled={busy || !canUseExample} onClick={() => onExample(example)}>{example}</button></li>)}</ul></> : null}
  </div>;
}
