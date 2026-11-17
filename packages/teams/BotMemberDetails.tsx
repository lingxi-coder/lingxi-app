import { sharedBotUsable, type SharedBotCard } from './execution.js';

/** Member projections only: never derive health from a host's private failure reason. */
export function botAvailabilityText(bot: SharedBotCard, zh: boolean): string {
  const names = { ready: ['Ready', '已就绪'], paused: ['Paused', '已暂停'], offline: ['Host offline', '宿主离线'], maintenance: ['Temporarily under maintenance', '暂时维护中'], quota_exhausted: ['Daily quota exhausted', '今日额度已用完'], unverified: ['Status unverified', '状态未确认'] };
  if (bot.state === 'paused') return names.paused[zh ? 1 : 0];
  return names[bot.availability ?? 'unverified'][zh ? 1 : 0];
}

export function BotMemberDetails({ bot, zh, onExample, busy = false, canUseExample = sharedBotUsable(bot), showAvailability = true }: { bot: SharedBotCard; zh: boolean; onExample?(example: string): void; busy?: boolean; canUseExample?: boolean; showAvailability?: boolean }) {
  const t = (en: string, cn: string) => zh ? cn : en;
  const supported = (value: boolean | undefined) => value === undefined ? t('Unknown', '未知') : value ? t('Supported', '支持') : t('Not supported', '不支持');
  const resetDate = bot.quotaResetsAt === undefined ? undefined : new Date(bot.quotaResetsAt);
  const reset = resetDate && Number.isFinite(resetDate.getTime()) ? resetDate.toISOString().replace('T', ' ').replace('.000Z', ' UTC') : t('Unknown', '未知');
  return <div className="tw-bot-member-details">
    {showAvailability ? <p>{botAvailabilityText(bot, zh)}</p> : null}
    <p className="teams-hint">{t('Wait and resume', '等待与恢复')}: {supported(bot.capabilities?.durableWait)} · {t('Write operations', '写入操作')}: {supported(bot.capabilities?.writeOperations)}</p>
    <p className="teams-hint">{t('Your remaining requests today', '本人今日剩余请求')}: {bot.dailyRemaining ?? t('Unknown', '未知')} · {t('Resets', '重置于')}: {reset}</p>
    {onExample && bot.examples?.length ? <><p className="teams-hint">{t('Examples add to your draft. Review and send when ready.', '示例会加入草稿，请检查后自行发送。')}</p><ul role="list">{bot.examples.slice(0, 3).map((example, index) => <li key={index}><button type="button" disabled={busy || !canUseExample} onClick={() => onExample(example)}>{example}</button></li>)}</ul></> : null}
  </div>;
}
