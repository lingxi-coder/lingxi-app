import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { sharedBotUsable } from './execution.js';
import { useTeamTasks } from './bot-runs.js';
import { BotRun } from './BotRunCards';
import { BotMemberDetails } from './BotMemberDetails';
import { TaskReportExport } from './TaskReportExport';
import { messageErrorText } from './errors.js';
import type { RenderMarkdown } from './ui';
import { useTeamsText } from './i18n.js';

export function MemberHome({ workspace, snapshot, locale, renderMarkdown }: { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; locale: string; renderMarkdown?: RenderMarkdown }) {
  const t = useTeamsText(locale);
  const team = snapshot.team!;
  const scope = { teamId: team.id, generation: team.generation };
  const tasks = useTeamTasks(workspace.api, scope);
  return <section className="tw-scope tw-member-home"><h2>{t('teams_bots_title')}</h2><p>{t('tw_home_intro')}</p>
    {snapshot.botsUnverified ? <p role="alert">{t('tw_home_unverified')}</p> : null}
    <p className="tw-hint">{t('tw_home_costs')}</p>
    <ul className="team-bot-grid">{snapshot.bots.map(bot => <li className="teams-card" key={bot.sharedBotId}><h3>{bot.name}</h3><p>{bot.description}</p><small>{t('tw_maintainer')}: {snapshot.directory.find(member => member.uid === bot.maintainer.uid)?.displayName ?? bot.maintainer.uid}</small><BotMemberDetails bot={bot} locale={locale} busy={snapshot.messageBusy} onExample={example => void workspace.openBotExample(bot, example)} /><button disabled={snapshot.messageBusy || !sharedBotUsable(bot)} onClick={() => void workspace.openBotDm(bot)}>{t('tw_home_start')}</button></li>)}</ul>
    {!snapshot.bots.length && snapshot.botsLoaded ? <p>{t('tw_home_none')}</p> : null}
    <h2>{t('tw_home_recent')}</h2>{tasks.error ? <p role="alert">{messageErrorText(tasks.error, locale)}</p> : null}
    <TaskReportExport key={`${snapshot.uid}:${team.id}:${team.generation}:${snapshot.authority}`} workspace={workspace} snapshot={snapshot} locale={locale} />
    <ol>{tasks.tasks.slice(0, 20).map(task => <li key={task.taskId}><b>{snapshot.bots.find(bot => bot.sharedBotId === task.sharedBotId)?.name ?? (t('teams_bot_tag'))}</b><button onClick={() => workspace.selectConversation(task.conversationId)}>{t('tw_home_open')}</button><BotRun task={task} api={workspace.api} scope={scope} locale={locale} renderMarkdown={renderMarkdown} snapshot={snapshot} onChanged={tasks.refresh} /></li>)}</ol>
    {!tasks.tasks.length && !tasks.error ? <p>{t('tw_home_recent_empty')}</p> : null}
  </section>;
}
