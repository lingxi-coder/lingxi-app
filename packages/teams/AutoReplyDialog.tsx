import { useState, type ReactElement } from 'react';
import { executionCommand, type ExecutionScope, type SharedBotCard } from './execution.js';
import type { TeamConversation } from './messages.js';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';
import { Dialog } from './ui';
import { useTeamsText } from './i18n.js';

/** The Chats where a Bot may judge for itself whether to answer: groups, channels and Bot DMs. */
const KINDS: TeamConversation['kind'][] = ['group', 'channel', 'bot_dm'];

/** The Bots of a Chat that can be switched to answer on their own, and those already on (which can always be switched off). `kept` are
 * Bots of the Chat already on that this person cannot see: the setting is the whole list, so a change here leaves them on. */
export function autoReplyBots(conversation: TeamConversation, snapshot: Pick<TeamsSnapshot, 'bots' | 'uid'>): { bots: SharedBotCard[]; on: string[]; kept: string[] } {
  if (!KINDS.includes(conversation.kind) || !conversation.participants.includes(`user:${snapshot.uid}`)) return { bots: [], on: [], kept: [] };
  const on = conversation.autoReply?.targets ?? [];
  const bots = snapshot.bots.filter(bot => conversation.participants.includes(`shared_bot:${bot.sharedBotId}`)
    && (on.includes(bot.sharedBotId) || bot.state === 'published' && !!bot.autoReply));
  const shown = (id: string) => bots.some(bot => bot.sharedBotId === id);
  return { bots, on: on.filter(shown), kept: on.filter(id => !shown(id) && conversation.participants.includes(`shared_bot:${id}`)) };
}

/**
 * Which Bots of this Chat read every new message and answer when they judge a reply is wanted. Any person in the Chat may change it;
 * it says first where the messages go.
 */
export function AutoReplyDialog({ workspace, snapshot, conversation, scope, locale, onClose }: {
  workspace: TeamsWorkspace; snapshot: TeamsSnapshot; conversation: TeamConversation; scope: ExecutionScope; locale: string; onClose(): void;
}): ReactElement {
  const t = useTeamsText(locale);
  const { bots, on, kept } = autoReplyBots(conversation, snapshot);
  // The setting as the dialog opened: a change saved meanwhile by someone else is a conflict, not silently overwritten by this one.
  const [opened] = useState(() => ({ on, kept, revision: conversation.autoReply?.revision ?? 0 }));
  const [chosen, setChosen] = useState<string[]>(opened.on);
  const nameOf = (uid: string) => uid === snapshot.uid ? t('teams_list_history_you_inline') : snapshot.directory.find(member => member.uid === uid)?.displayName || t('tw_its_maintainer');
  const changed = chosen.length !== opened.on.length || chosen.some(id => !opened.on.includes(id));
  const save = async () => {
    const targets = [...bots.map(bot => bot.sharedBotId).filter(id => chosen.includes(id)), ...opened.kept];
    if (await workspace.run(() => workspace.api.messages(executionCommand(scope, 'setConversationAutoReply', { conversationId: conversation.id, expectedRevision: opened.revision, targets })))) onClose();
  };
  return <Dialog title={t('teams_auto_reply_title')} onClose={onClose}>
    <div className="tw-form">
      <p className="tw-hint">{t('teams_auto_reply_note')}</p>
      {bots.map(bot => { const checked = chosen.includes(bot.sharedBotId), available = bot.state === 'published' && !!bot.autoReply; return <div key={bot.sharedBotId}>
        <label className="team-party"><input type="checkbox" checked={checked} disabled={snapshot.messageBusy || !checked && !available}
          onChange={event => setChosen(old => event.target.checked ? [...old, bot.sharedBotId] : old.filter(id => id !== bot.sharedBotId))} />{bot.name}</label>
        <p className="tw-hint">{available
          ? t('teams_auto_reply_disclosure_fmt', nameOf(bot.maintainer.uid), bot.name, bot.autoReply!.daily)
          : t('teams_auto_reply_unavailable_fmt', bot.name)}</p>
      </div>; })}
      <div className="tw-dialog-actions">
        <button type="button" data-autofocus onClick={onClose}>{t('common_cancel')}</button>
        <button type="button" className="tw-primary" disabled={snapshot.messageBusy || !changed} onClick={() => void save()}>{t('common_save')}</button>
      </div>
    </div>
  </Dialog>;
}
