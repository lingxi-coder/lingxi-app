import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { sharedBotUsable } from './execution.js';
import { useTeamTasks } from './bot-runs.js';
import { BotRun } from './BotRunCards';
import { messageErrorText } from './errors.js';

export function MemberHome({ workspace, snapshot, zh }: { workspace: TeamsWorkspace; snapshot: TeamsSnapshot; zh: boolean }) {
  const team = snapshot.team!;
  const scope = { teamId: team.id, generation: team.generation };
  const tasks = useTeamTasks(workspace.api, scope);
  return <section className="tw-scope tw-member-home"><h2>{zh ? '团队助手' : 'Team assistants'}</h2><p>{zh ? '直接向团队助手提问，无需配置模型或密钥。' : 'Ask your team assistants directly. No model or API key setup is needed.'}</p>
    {snapshot.botsUnverified ? <p role="alert">{zh ? '暂时无法确认助手状态。请重新连接。' : 'Assistant status could not be verified. Reconnect to check.'}</p> : null}
    <p className="tw-hint">{zh ? '模型费用由助手维护者承担；维护者可以阅读包含其助手的会话。' : 'Assistant maintainers pay model costs and can read conversations containing their assistant.'}</p>
    <ul className="team-bot-grid">{snapshot.bots.map(bot => <li className="teams-card" key={bot.sharedBotId}><h3>{bot.name}</h3><p>{bot.description}</p><small>{zh ? '维护者' : 'Maintainer'}: {snapshot.directory.find(member => member.uid === bot.maintainer.uid)?.displayName ?? bot.maintainer.uid}</small><p>{bot.ready ? (zh ? '已就绪' : 'Ready') : (zh ? '宿主暂不可用' : 'Host unavailable')}</p><button disabled={snapshot.messageBusy || !sharedBotUsable(bot)} onClick={() => void workspace.openBotDm(bot)}>{zh ? '开始对话' : 'Start conversation'}</button></li>)}</ul>
    {!snapshot.bots.length && snapshot.botsLoaded ? <p>{zh ? '请维护者在 Desktop 发布助手。' : 'Ask a maintainer to publish an assistant from Desktop.'}</p> : null}
    <h2>{zh ? '最近任务与成果' : 'Recent tasks and artifacts'}</h2>{tasks.error ? <p role="alert">{messageErrorText(tasks.error, zh)}</p> : null}
    <ol>{tasks.tasks.slice(0, 20).map(task => <li key={task.taskId}><b>{snapshot.bots.find(bot => bot.sharedBotId === task.sharedBotId)?.name ?? (zh ? '助手' : 'Assistant')}</b><button onClick={() => workspace.selectConversation(task.conversationId)}>{zh ? '打开会话' : 'Open conversation'}</button><BotRun task={task} api={workspace.api} scope={scope} zh={zh} /></li>)}</ol>
    {!tasks.tasks.length && !tasks.error ? <p>{zh ? '你的任务和生成的文档将显示在这里。' : 'Your tasks and generated documents will appear here.'}</p> : null}
  </section>;
}
