import { useState, type ReactElement } from 'react';
import { executionCommand, type ExecutionScope, type SharedBotCard } from './execution.js';
import type { TeamConversation } from './messages.js';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { Dialog } from './ui';

/** The Chats where a Bot may judge for itself whether to answer: groups, channels and Bot DMs. */
const KINDS: TeamConversation['kind'][] = ['group', 'channel', 'bot_dm'];

/** The Bots of a Chat that can be switched to answer on their own, and those already on (which can always be switched off). */
export function autoReplyBots(conversation: TeamConversation, snapshot: Pick<TeamsSnapshot, 'bots' | 'uid'>): { bots: SharedBotCard[]; on: string[] } {
  if (!KINDS.includes(conversation.kind) || !conversation.participants.includes(`user:${snapshot.uid}`)) return { bots: [], on: [] };
  const on = conversation.autoReply?.targets ?? [];
  const bots = snapshot.bots.filter(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`)
    && (on.includes(bot.sharedBotId) || bot.state === 'published' && !!bot.autoReply));
  return { bots, on: on.filter(id => bots.some(bot => bot.sharedBotId === id)) };
}

/**
 * Which Bots of this Chat read every new message and answer when they judge a reply is wanted. Any person in the Chat may change it;
 * it says first where the messages go.
 */
export function AutoReplyDialog({ workspace, snapshot, conversation, scope, zh, onClose }: {
  workspace: TeamsWorkspace; snapshot: TeamsSnapshot; conversation: TeamConversation; scope: ExecutionScope; zh: boolean; onClose(): void;
}): ReactElement {
  const t = (en: string, cn: string) => zh ? cn : en;
  const { bots, on } = autoReplyBots(conversation, snapshot);
  const [chosen, setChosen] = useState<string[]>(on);
  const nameOf = (uid: string) => uid === snapshot.uid ? t('you', '你') : snapshot.directory.find(member => member.uid === uid)?.displayName || t('its maintainer', '其维护者');
  const changed = chosen.length !== on.length || chosen.some(id => !on.includes(id));
  const save = async () => {
    const targets = bots.map(bot => bot.sharedBotId).filter(id => chosen.includes(id));
    if (await workspace.run(() => workspace.api.messages(executionCommand(scope, 'setConversationAutoReply', { conversationId: conversation.id, expectedRevision: conversation.autoReply?.revision ?? 0, targets })))) onClose();
  };
  return <Dialog title={t('Bots that answer on their own', '自主回复的 Bot')} onClose={onClose}>
    <div className="tw-form">
      <p className="tw-hint">{t('A Bot turned on here reads every new message in this chat and answers when it judges a reply is wanted, without being asked.', '在此开启的 Bot 会读取此会话的每条新消息，并在判断需要时主动回复，无需被点名。')}</p>
      {bots.map(bot => { const checked = chosen.includes(bot.sharedBotId), available = bot.state === 'published' && !!bot.autoReply; return <div key={bot.sharedBotId}>
        <label className="team-party"><input type="checkbox" checked={checked} disabled={snapshot.messageBusy || !checked && !available}
          onChange={event => setChosen(old => event.target.checked ? [...old, bot.sharedBotId] : old.filter(id => id !== bot.sharedBotId))} />{bot.name}</label>
        <p className="tw-hint">{available
          ? t(`New messages in this chat are sent to the Decision service and model that ${nameOf(bot.maintainer.uid)} set up for ${bot.name}. It answers on its own at most ${bot.autoReply!.daily} times a day.`,
            `开启后，此会话的新消息会发送给 ${nameOf(bot.maintainer.uid)} 为 ${bot.name} 配置的判断服务和模型。它每天最多自主回复 ${bot.autoReply!.daily} 次。`)
          : t(`${bot.name} no longer answers on its own. Turn it off here.`, `${bot.name} 已不再支持自主回复，可在此关闭。`)}</p>
      </div>; })}
      <div className="tw-dialog-actions">
        <button type="button" data-autofocus onClick={onClose}>{t('Cancel', '取消')}</button>
        <button type="button" className="tw-primary" disabled={snapshot.messageBusy || !changed} onClick={() => void save()}>{t('Save', '保存')}</button>
      </div>
    </div>
  </Dialog>;
}
