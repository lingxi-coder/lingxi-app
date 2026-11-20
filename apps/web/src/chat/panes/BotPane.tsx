import { useEffect, useMemo } from 'react';
import { botAvailabilityText } from '../../../../../packages/teams/BotMemberDetails';
import { sharedBotUsable } from '../../../../../packages/teams/execution';
import { useTeamsText } from '../../../../../packages/teams/i18n';
import type { UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import { findBotDm } from '../../../../../packages/teams/workspace';
import type { ChatTarget } from '../../app/router';
import { PaneNotice } from './PaneNotice';

/**
 * A shared Bot of a team, from a link: opens the viewer's own conversation with it. Opening never creates one; when
 * there is none the viewer is asked first.
 */
export function BotPane({ teams: { workspace, snapshot }, teamId, botId, locale, onFollow }: { teams: UseTeams; teamId: string; botId: string; locale: string; onFollow(target: ChatTarget, replace: boolean): void }) {
  const t = useTeamsText(locale);
  const zh = locale === 'zh';
  useEffect(() => { workspace?.openConversation(teamId, ''); }, [workspace, teamId]);
  const ready = !!snapshot && !snapshot.loading && snapshot.selectedTeamId === teamId && snapshot.botsLoaded && !snapshot.messagingConnecting;
  const bot = ready ? snapshot.bots.find(card => card.sharedBotId === botId) : undefined;
  const existing = ready && bot ? findBotDm(snapshot.conversations, snapshot.uid, botId, snapshot.bots) : undefined;
  useEffect(() => { if (existing) onFollow({ kind: 'team', teamId, conversationId: existing.id }, true); }, [existing, teamId, onFollow]);
  const retryTeams = useMemo(() => workspace ? { label: zh ? '重试' : 'Try again', run: () => void workspace.refresh() } : undefined, [workspace, zh]);
  const retryBots = useMemo(() => workspace ? { label: zh ? '重试' : 'Try again', run: () => void workspace.refreshBots() } : undefined, [workspace, zh]);
  if (!workspace || !snapshot || snapshot.loading) return <PaneNotice busy text={t('teams_loading')} />;
  if (snapshot.loadError && !snapshot.teams.some(team => team.id === teamId)) return <PaneNotice title={zh ? '暂时无法打开这个 Bot' : 'This Bot could not be loaded'}
    text={zh ? '团队列表加载失败，请检查网络后重试。' : 'Your teams could not be loaded. Check your connection and try again.'} retry={retryTeams} />;
  if (!snapshot.teams.some(team => team.id === teamId)) return <PaneNotice title={zh ? '无法打开这个 Bot' : 'This Bot is not available'}
    text={zh ? '你不是这个 Bot 所在团队的成员。' : 'You are not a member of the team this Bot is shared in.'} action={{ href: '/teams/join', label: zh ? '加入团队' : 'Join a team' }} />;
  // The team's Bots failed to load before any arrived: say so and retry, rather than wait for a list that is not coming.
  if (snapshot.selectedTeamId === teamId && !snapshot.botsLoaded && snapshot.botsError) return <PaneNotice title={zh ? '暂时无法打开这个 Bot' : 'This Bot could not be loaded'}
    text={zh ? '团队的 Bot 列表加载失败，请稍后重试。' : 'The team’s Bots could not be loaded. Try again in a moment.'} retry={retryBots} />;
  if (!ready || existing) return <PaneNotice busy text={t('teams_loading')} />;
  if (!bot) return <PaneNotice title={zh ? '无法打开这个 Bot' : 'This Bot is not available'} text={zh ? '这个 Bot 已不在团队中共享。' : 'This Bot is no longer shared in the team.'} />;
  const usable = sharedBotUsable(bot);
  return <div className="chat-notice chat-bot-intro">
    <span className="chat-bot-mark" aria-hidden="true">{(bot.name.trim()[0] ?? 'B').toUpperCase()}</span>
    <h2>{bot.name}</h2>
    {bot.description ? <p>{bot.description}</p> : null}
    <p className="chat-muted">{botAvailabilityText(bot, locale)}</p>
    <button type="button" className="chat-button" disabled={!usable || snapshot.messageBusy} onClick={() => void workspace.openBotDm(bot).then(opened => {
      const id = workspace.getSnapshot().selectedConversationId;
      if (opened && id) onFollow({ kind: 'team', teamId, conversationId: id }, true);
    })}>{zh ? '开始对话' : 'Start a chat'}</button>
    {snapshot.actionError ? <p role="alert" className="chat-error">{zh ? '暂时无法开始对话，请稍后再试。' : 'The chat could not be started. Try again later.'}</p> : null}
  </div>;
}
