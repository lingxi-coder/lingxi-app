import { useEffect, useState, type ReactElement } from 'react';
import { AutoReplyDialog, autoReplyBots } from './AutoReplyDialog';
import { BotScopeDialog, scopedBots } from './BotScopeDialog';
import type { ExecutionScope } from './execution.js';
import { useTeamsText } from './i18n.js';
import type { TeamConversation } from './messages.js';
import { Icon } from './ui';
import type { TeamsSnapshot, TeamsWorkspace } from './workspace.js';

export function ChannelSettings({ workspace, snapshot, conversation, scope, locale }: {
  workspace: TeamsWorkspace; snapshot: TeamsSnapshot; conversation: TeamConversation; scope: ExecutionScope; locale: string;
}): ReactElement {
  const t = useTeamsText(locale);
  const [editor, setEditor] = useState<'autoReply' | 'botScope'>();
  const hasAutoReply = autoReplyBots(conversation, snapshot).bots.length > 0;
  const hasBotScope = scopedBots(conversation, snapshot).length > 0;
  const available = editor === 'autoReply' ? hasAutoReply : editor === 'botScope' && hasBotScope;
  const activeEditor = !snapshot.offline && available ? editor : undefined;
  const close = () => setEditor(undefined);
  useEffect(() => { if (!activeEditor) setEditor(undefined); }, [activeEditor]);

  return <>
    {hasAutoReply || hasBotScope ? <div className="tw-channel-settings">
      {hasAutoReply ? <button type="button" className="tw-channel-setting" aria-haspopup="dialog" disabled={snapshot.offline} onClick={() => setEditor('autoReply')}>
        <Icon name="bot" /><span>{t('teams_auto_reply_title')}</span><Icon name="chevronRight" size={14} />
      </button> : null}
      {hasBotScope ? <button type="button" className="tw-channel-setting" aria-haspopup="dialog" disabled={snapshot.offline} onClick={() => setEditor('botScope')}>
        <Icon name="sliders" /><span>{t('teams_bot_scope_title')}</span><Icon name="chevronRight" size={14} />
      </button> : null}
    </div> : null}
    {snapshot.offline ? <p className="tw-empty-line" role="status">{t('teams_messages_offline')}</p>
      : snapshot.botsUnverified ? <p className="tw-empty-line" role="status">{t('teams_bots_unverified')}</p>
      : !snapshot.botsLoaded ? <p className="tw-empty-line" role="status">{t('teams_bots_loading')}</p>
      : !hasAutoReply && !hasBotScope ? <p className="tw-empty-line">{t('tw_channel_no_settings')}</p> : null}
    {activeEditor === 'autoReply' ? <AutoReplyDialog workspace={workspace} snapshot={snapshot}
      conversation={conversation} scope={scope} locale={locale} onClose={close} /> : null}
    {activeEditor === 'botScope' ? <BotScopeDialog workspace={workspace} snapshot={snapshot}
      conversation={conversation} scope={scope} locale={locale} onClose={close} /> : null}
  </>;
}
