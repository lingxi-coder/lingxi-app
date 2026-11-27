import { BotCardSummary } from '../chat-ui/BotCardSummary';
import { botScopeNarrowed, effectiveBotScope } from './bot-scope.js';
import { BotScopeSummary } from './BotScopeSummary';
import { delegatesTo, delegationAccepts } from './delegation.js';
import { sharedBotUsable, type SharedBotCard } from './execution.js';
import { teamsText, tk, useTeamsText, type TeamsKey } from './i18n.js';
import type { TeamConversation } from './messages.js';
import { Avatar, type RenderAvatar } from './ui';

/** What a member is told about a Bot's availability. `paused` is how the card of a Bot whose sharing was stopped reads (U1: "Sharing stopped"). */
const AVAILABILITY: Record<NonNullable<SharedBotCard['availability']>, TeamsKey> = {
  ready: tk('teams_bot_avail_ready'),
  paused: tk('teams_bot_disabled'),
  offline: tk('teams_bot_avail_offline'),
  maintenance: tk('teams_bot_avail_maintenance'),
  quota_exhausted: tk('teams_bot_avail_quota'),
  unverified: tk('teams_bot_avail_unverified'),
};

/** Member projections only: never derive health from a host's private failure reason. A value this build does not know reads as unverified. */
export function botAvailabilityText(bot: SharedBotCard, locale: string): string {
  const state = bot.state === 'paused' ? 'paused' : bot.availability ?? 'unverified';
  return teamsText(locale)(Object.prototype.hasOwnProperty.call(AVAILABILITY, state) ? AVAILABILITY[state] : AVAILABILITY.unverified);
}

export function SharedBotCardSummary({ bot, maintainer, locale, renderAvatar, compact }: {
  bot: SharedBotCard; maintainer: string; locale: string; renderAvatar?: RenderAvatar; compact?: boolean;
}) {
  const t = useTeamsText(locale);
  const online = bot.state === 'published' && bot.availability === 'ready';
  return <BotCardSummary heading={compact ? 'h4' : 'h3'} compact={compact} name={bot.name} description={bot.description}
    avatar={<Avatar identity={{ kind: 'bot', id: bot.sharedBotId, name: bot.name }} size={compact ? 32 : 40} render={renderAvatar} />}
    status={<span className="bot-card-status tw-bot-card-status" data-online={online}>{botAvailabilityText(bot, locale)}</span>}
    metadata={`${t('tw_maintainer')}: ${maintainer}`} />;
}

/**
 * With a `conversation`, what the Bot may use is what that Chat leaves of what its maintainer shared; without one, all of what they shared. `bots` are the Bots of the
 * Team, which name the ones this card lists: what a published Bot takes from other Bots and whom it may hand part of a request to (Bot v3 B8), read only: it is chosen
 * where the Bot is published.
 */
export function BotMemberDetails({ bot, bots, locale, onExample, busy = false, canUseExample = sharedBotUsable(bot), showAvailability = true, conversation }: { bot: SharedBotCard; bots: readonly Pick<SharedBotCard, 'sharedBotId' | 'name'>[]; locale: string; onExample?(example: string): void; busy?: boolean; canUseExample?: boolean; showAvailability?: boolean; conversation?: Pick<TeamConversation, 'botScopes'> }) {
  const t = useTeamsText(locale);
  const usable = effectiveBotScope(bot.scope, conversation?.botScopes?.[bot.sharedBotId]);
  const supported = (value: boolean | undefined) => value === undefined ? t('chat_status_unknown') : value ? t('teams_bot_supported') : t('teams_bot_not_supported');
  const resetDate = bot.quotaResetsAt === undefined ? undefined : new Date(bot.quotaResetsAt);
  const reset = resetDate && Number.isFinite(resetDate.getTime()) ? resetDate.toISOString().replace('T', ' ').replace('.000Z', ' UTC') : t('chat_status_unknown');
  const accepts = delegationAccepts(bot, bots), asks = delegatesTo(bot, bots);
  return <div className="tw-bot-member-details">
    {showAvailability ? <p>{botAvailabilityText(bot, locale)}</p> : null}
    <p className="teams-hint">{t('teams_bot_wait_resume')}: {supported(bot.capabilities?.durableWait)} · {t('teams_bot_write_ops')}: {supported(bot.capabilities?.writeOperations)}</p>
    <p className="teams-hint">{t('teams_bot_remaining_today')}: {bot.dailyRemaining ?? t('chat_status_unknown')} · {t('teams_bot_resets')}: {reset}</p>
    {bot.state === 'published' ? <p className="teams-hint tw-bot-delegation">{t('teams_bot_accepts_delegation')}: {accepts.mode === 'team_bots' ? t('teams_bot_accepts_team') : accepts.mode === 'listed' ? accepts.names.join(', ') : t('teams_bot_accepts_none')}{asks.length ? ` · ${t('teams_bot_delegates_to')}: ${asks.join(', ')}` : ''}</p> : null}
    <BotScopeSummary scope={usable} limited={botScopeNarrowed(bot.scope, usable)} locale={locale} />
    {onExample && bot.examples?.length ? <><p className="teams-hint">{t('teams_bot_examples_hint')}</p><ul role="list">{bot.examples.slice(0, 3).map((example, index) => <li key={index}><button type="button" disabled={busy || !canUseExample} onClick={() => onExample(example)}>{example}</button></li>)}</ul></> : null}
  </div>;
}
