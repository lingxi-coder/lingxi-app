import type { DatasetBindingDraft, DatasetBotScope } from '../../../../../packages/teams/dataset-run.js';
import { app, dialog, ipcMain, type BrowserWindow } from 'electron';
import type { DesktopDataSources } from './dataSources.js';
import { scheduledDatasetProgram, taskRecipeDigest, teamBotDatasetKey, type DatasetTaskHost } from './datasetTasks.js';
import type { TeamsService } from './service.js';
import type { ScheduledTaskService } from '../scheduled.js';

function botScope(input: Partial<DatasetBotScope>): DatasetBotScope {
  if (typeof input.team !== 'string' || typeof input.bot !== 'string' || typeof input.conversation !== 'string' ||
      ![input.team, input.bot, input.conversation].every(value => /^[A-Za-z0-9_-]{8,64}$/u.test(value))) {
    throw new Error('INVALID_INPUT');
  }
  return { team: input.team, bot: input.bot, conversation: input.conversation };
}

export function installTeamsIpc(service: TeamsService, windows: () => readonly BrowserWindow[], sources?: DesktopDataSources, taskHost?: DatasetTaskHost,
  schedules?: ScheduledTaskService): () => void {
  const channel = 'lingxi:teams';
  const senders = new Map<number, () => void>();
  ipcMain.handle(channel, async (event, input: unknown) => {
    const window = windows().find(target => event.sender === target.webContents && event.senderFrame === target.webContents.mainFrame);
    if (!window) throw new Error('Invalid sender');
    if (!input || typeof input !== 'object') throw new Error('Invalid Team command');
    const command = input as Record<string, unknown>;
    if (typeof command.uid !== 'string' || !command.uid || command.uid.length > 128) throw new Error('Invalid account');
    try {
      let value: unknown;
      switch (command.type) {
        case 'localDataSource':
          if (!sources || !command.request || typeof command.request !== 'object') throw new Error('INVALID_INPUT');
          value = await sources.execute(command.uid, command.request as Parameters<DesktopDataSources['execute']>[1], window);
          break;
        case 'watchMessages':
        case 'unwatchMessages': {
          if (typeof command.leaseId !== 'string' || !/^[A-Za-z0-9_-]{8,64}$/u.test(command.leaseId)) throw new Error('INVALID_INPUT');
          const prefix = `${event.sender.id}:`;
          const lease = `${prefix}${command.leaseId}:`;
          if (command.type === 'unwatchMessages') { service.releaseMessageWatches(lease); break; }
          if (!command.scope || typeof command.scope !== 'object' || typeof command.conversationId !== 'string') throw new Error('INVALID_INPUT');
          if (!senders.has(event.sender.id)) {
            const sender = event.sender, senderId = sender.id;
            const release = () => service.releaseMessageWatches(prefix);
            const navigate = (_event: unknown, _url: string, inPlace: boolean, main: boolean) => { if (main && !inPlace) release(); };
            const destroyed = () => { release(); senders.get(senderId)?.(); senders.delete(senderId); };
            sender.on('destroyed', destroyed); sender.on('did-start-navigation', navigate);
            senders.set(sender.id, () => { sender.off('destroyed', destroyed); sender.off('did-start-navigation', navigate); });
          }
          await service.watchMessages(lease, command.uid, command.scope as Parameters<TeamsService['watchMessages']>[2], command.conversationId);
          break;
        }
        case 'list':
          value = await service.list(command.uid);
          break;
        case 'localBots':
          value = await service.localBots(command.uid);
          break;
        case 'publishLocalBot':
          if (!command.input || typeof command.input !== 'object') throw new Error('INVALID_INPUT');
          value = await service.publishLocalBot(command.uid, command.input as Parameters<TeamsService['publishLocalBot']>[1]);
          break;
        case 'selectSharedFiles':
          if (typeof command.instanceId !== 'string') throw new Error('INVALID_INPUT');
          value = await service.selectSharedFiles(command.uid, command.instanceId);
          break;
        case 'offerSharedBot':
          if (!command.input || typeof command.input !== 'object') throw new Error('INVALID_INPUT');
          value = await service.offerSharedBot(command.uid, command.input as Parameters<TeamsService['offerSharedBot']>[1]);
          break;
        case 'acceptSharedBot':
          if (!command.input || typeof command.input !== 'object') throw new Error('INVALID_INPUT');
          value = await service.acceptSharedBot(command.uid, command.input as Parameters<TeamsService['acceptSharedBot']>[1]);
          break;
        case 'hostStatus':
          value = await service.hostStatus(command.uid);
          break;
        case 'publishedKnowledge':
          if (typeof command.teamId !== 'string' || typeof command.sharedBotId !== 'string') throw new Error('INVALID_INPUT');
          value = await service.publishedKnowledge(command.uid, command.teamId, command.sharedBotId);
          break;
        case 'triageTargets':
          if (typeof command.instanceId !== 'string') throw new Error('INVALID_INPUT');
          value = await service.triageTargets(command.uid, command.instanceId);
          break;
        case 'triageLog':
          if (!command.request || typeof command.request !== 'object' || Array.isArray(command.request)) throw new Error('INVALID_INPUT');
          value = await service.triageLog(command.uid, command.request as Parameters<TeamsService['triageLog']>[1]);
          break;
        case 'detail':
          if (typeof command.teamId !== 'string') throw new Error('INVALID_INPUT');
          value = await service.detail(command.teamId, command.uid);
          break;
        case 'historyPage':
          value = await service.historyPage(command.uid,command.scope as Parameters<TeamsService['historyPage']>[1],
            command.conversationId as string, command.beforeSeq as number);
          break;
        case 'datasets': {
          if (!command.request || typeof command.request !== 'object' || Array.isArray(command.request)) throw new Error('INVALID_INPUT');
          const request = command.request as Record<string, unknown>;
          const action = (request.command as { action?: string } | undefined)?.action;
          if (action === 'approveTask' || action === 'revokeTask') throw new Error('NATIVE_APPROVAL_REQUIRED');
          value = await service.datasets(command.uid, request);
          break;
        }
        case 'approveDatasetTask': {
          if (!taskHost || !command.draft || typeof command.draft !== 'object') throw new Error('DATA_UNAVAILABLE');
          const d = command.draft as DatasetBindingDraft;
          if (!['nativeSchedule', 'teamBot', 'localAppAgent'].includes(d.executor) ||
              typeof d.dataset !== 'string' || typeof d.operation !== 'string' || !d.plan ||
              typeof d.plan !== 'object' || Array.isArray(d.plan) ||
              JSON.stringify(d).length > 96000) throw new Error('INVALID_INPUT');
          let program;
          if (d.executor === 'nativeSchedule') {
            if (!schedules || typeof d.scope !== 'string' || typeof d.task !== 'string') throw new Error('INVALID_INPUT');
            const path = schedules.resolveScope(d.scope);
            const job = (await schedules.manage(d.scope, { action: 'list' })).find(job => job.id === d.task);
            if (!job || job.permanent || job.durable === false) throw new Error('TASK_UNAVAILABLE');
            program = scheduledDatasetProgram(job, path);
          } else if (d.executor === 'teamBot') {
            const scope = botScope(d);
            program = await service.datasetBotProgram(command.uid, scope);
          } else {
            if (typeof d.task !== 'string' || !d.recipe) throw new Error('INVALID_INPUT');
            program = { executor: d.executor, task: d.task, recipe: d.recipe };
          }
          const zh = app.getLocale().toLowerCase().startsWith('zh');
          const detail = JSON.stringify({ dataset: d.dataset, task: program.task, executor: d.executor,
            ...(d.executor === 'teamBot' ? { bot: d.bot, conversation: d.conversation } : {}),
            recipeSha256: taskRecipeDigest(program), plan: d.plan }, null, 2);
          const result = await dialog.showMessageBox(window, { type: 'question', title: zh ? '数据任务授权' : 'Dataset task approval',
            message: zh ? '允许此任务按以下方案读取、写入或发布数据？' : 'Allow this task to use Data under this plan?',
            detail, buttons: zh ? ['取消', '批准方案'] : ['Cancel', 'Approve plan'], defaultId: 0, cancelId: 0, noLink: true });
          if (result.response !== 1) throw new Error('APPROVAL_CANCELLED');
          value = await taskHost.approve(program, d.dataset, d.plan, d.operation, command.uid);
          break;
        }
        case 'revokeScheduledDatasetTask':
        case 'revokeBotDatasetTask': {
          if (!taskHost) throw new Error('DATA_UNAVAILABLE');
          let program: { executor: 'nativeSchedule' | 'teamBot'; task: string };
          if (command.type === 'revokeBotDatasetTask') {
            if (!command.scope || typeof command.scope !== 'object') throw new Error('INVALID_INPUT');
            program = { executor: 'teamBot', task: teamBotDatasetKey(botScope(command.scope as Partial<DatasetBotScope>)) };
          } else {
            if (typeof command.task !== 'string' || !/^[A-Za-z0-9_-]{1,64}$/u.test(command.task)) throw new Error('INVALID_INPUT');
            program = { executor: 'nativeSchedule', task: command.task };
          }
          const zh = app.getLocale().toLowerCase().startsWith('zh');
          const result = await dialog.showMessageBox(window, { type: 'question', message: zh ? '撤销此任务的数据许可？' : 'Revoke this task’s Dataset approval?',
            buttons: zh ? ['取消', '撤销许可'] : ['Cancel', 'Revoke approval'], defaultId: 0, cancelId: 0, noLink: true });
          if (result.response !== 1) throw new Error('APPROVAL_CANCELLED');
          await taskHost.revoke(program, command.uid);
          break;
        }
        case 'data':
          if (!command.request || typeof command.request !== 'object') throw new Error('INVALID_INPUT');
          value = await service.data(command.request as Parameters<TeamsService['data']>[0], command.uid);
          break;
        case 'messages':
          if (!command.command || typeof command.command !== 'object') throw new Error('INVALID_INPUT');
          value = await service.messages(command.command as Parameters<TeamsService['messages']>[0], command.uid);
          break;
        case 'services':
          if (!command.command || typeof command.command !== 'object') throw new Error('INVALID_INPUT');
          value = await service.services(command.command as Parameters<TeamsService['services']>[0], command.uid);
          break;
        case 'serviceMessages':
          value = await service.serviceMessages(command.command, command.uid);
          break;
        case 'execute':
          if (!command.command || typeof command.command !== 'object') throw new Error('INVALID_INPUT');
          value = await service.execute(command.command as Parameters<TeamsService['execute']>[0], command.uid);
          break;
        default:
          throw new Error('INVALID_INPUT');
      }
      return { value };
    } catch (error) {
      // Electron's invoke error serialization discards custom error fields.
      const code = error instanceof Error && /^[A-Z_]{3,60}$/u.test(error.message) ? error.message : 'SERVICE_UNAVAILABLE';
      return { error: code };
    }
  });
  const stop = service.onChanged(event => { for (const target of windows()) if (!target.isDestroyed()) target.webContents.send('lingxi:teams:changed', event); });
  const stopSnapshots = service.onMessageSnapshot(event => {
    const [senderId, leaseId] = event.leaseId.split(':');
    for (const target of windows()) if (!target.isDestroyed() && String(target.webContents.id) === senderId) target.webContents.send('lingxi:teams:snapshot', { leaseId, snapshot: event.snapshot });
  });
  const stopMessages = service.onMessagesChanged(signal => { for (const target of windows()) if (!target.isDestroyed()) target.webContents.send('lingxi:teams:messages', signal); });
  return () => { for (const release of senders.values()) release(); senders.clear(); stopSnapshots(); stopMessages(); stop(); ipcMain.removeHandler(channel); service.close(); };
}
