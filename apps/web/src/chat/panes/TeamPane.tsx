import { useEffect, useMemo, useRef, useState } from 'react';
import type { TeamMember } from '../../../../../packages/teams';
import type { FriendsApi } from '../../../../../packages/teams/friends';
import { TeamConversationView } from '../../../../../packages/teams/TeamConversationView';
import { TeamDetailsDrawer } from '../../../../../packages/teams/TeamDetailsDrawer';
import { TeamSettingsPage } from '../../../../../packages/teams/TeamSettingsPage';
import { TeamSettingsSurface, useTeamSettings } from '../../../../../packages/teams/TeamSettingsSurface';
import { useTeamsText } from '../../../../../packages/teams/i18n';
import type { UseTeams } from '../../../../../packages/teams/useTeamsWorkspace';
import type { ChatTarget } from '../../app/router';
import { MemberFriend } from '../../friends/MemberFriend';
import { renderTeamAvatar } from '../../teams/teamAvatar';
import { renderMarkdown } from '../markdown';
import { PaneNotice } from './PaneNotice';

interface Props {
  teams: UseTeams;
  teamId: string;
  conversationId: string;
  postId?: string;
  locale: string;
  friends?: FriendsApi;
  /** The open conversation changed inside the pane (a new Bot DM, a details action): follow it in the address bar. */
  onFollow(target: ChatTarget): void;
}

/** One conversation of one team. A team the viewer is not in shows that, never another team. */
export function TeamPane({ teams: { workspace, snapshot }, teamId, conversationId, postId, locale, friends, onFollow }: Props) {
  const t = useTeamsText(locale);
  const zh = locale === 'zh';
  const [drawer, setDrawer] = useState(false);
  const [adding, setAdding] = useState<TeamMember>();
  useEffect(() => { workspace?.openConversation(teamId, conversationId); }, [workspace, teamId, conversationId]);
  const selected = snapshot?.selectedTeamId === teamId ? snapshot.selectedConversationId : '';
  const settings = useTeamSettings(selected === conversationId ? snapshot : undefined);
  // Follow only a change made after the requested conversation opened, never the previous one still selected.
  const settled = useRef('');
  useEffect(() => {
    const requested = `${teamId}\n${conversationId}`;
    if (selected === conversationId) { settled.current = requested; return; }
    if (selected && settled.current === requested) onFollow({ kind: 'team', teamId, conversationId: selected });
  }, [selected, conversationId, teamId, onFollow]);
  const retry = useMemo(() => workspace ? { label: zh ? '重试' : 'Try again', run: () => void workspace.refresh() } : undefined, [workspace, zh]);
  if (!workspace || !snapshot || snapshot.loading) return <PaneNotice busy text={t('teams_loading')} />;
  // A list that failed to load says nothing about membership: that is a retry, not "you are not a member".
  if (snapshot.loadError && !snapshot.teams.some(team => team.id === teamId)) return <PaneNotice title={zh ? '暂时无法打开这个对话' : 'This chat could not be loaded'}
    text={zh ? '团队列表加载失败，请检查网络后重试。' : 'Your teams could not be loaded. Check your connection and try again.'} retry={retry} />;
  if (!snapshot.teams.some(team => team.id === teamId)) return <PaneNotice title={zh ? '无法打开这个对话' : 'This chat is not available'}
    text={zh ? '你不是这个团队的成员，或者团队已不存在。请让成员重新邀请你。' : 'You are not a member of this team, or it no longer exists. Ask a member to invite you again.'}
    action={{ href: '/teams/join', label: zh ? '加入团队' : 'Join a team' }} />;
  if (snapshot.selectedTeamId !== teamId) return <PaneNotice busy text={t('teams_loading')} />;
  // A server-confirmed directory without the conversation (the model then drops the selection): say so, rather than the
  // team's home. Not while connecting (each selection reconnects) or offline, when the list may be the cache's.
  if (conversationId && selected !== conversationId && !snapshot.messagingConnecting && !snapshot.offline
    && !snapshot.conversations.some(conversation => conversation.id === conversationId)) return <PaneNotice
    title={zh ? '无法打开这个对话' : 'This chat is not available'}
    text={zh ? '这个对话已不存在，或者你不在其中。' : 'This conversation no longer exists, or you are not in it.'} />;
  return <div className="chat-pane-team">
    <TeamSettingsSurface settings={settings.active ? <TeamSettingsPage key={settings.active.id} initialSection={settings.active.section}
      workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar}
      onClose={settings.back} onOpenChat={settings.back} onAddFriend={friends ? setAdding : undefined} /> : undefined}>
      <div className="chat-pane-conversation">
        <TeamConversationView workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar} renderMarkdown={renderMarkdown}
          sharedPostId={selected === conversationId ? postId : undefined} onExitPost={() => onFollow({ kind: 'team', teamId, conversationId })}
          detailsOpen={drawer} onToggleDetails={() => setDrawer(current => !current)} onOpenChannelSettings={() => setDrawer(false)}
          covered={!!settings.active} onOpenTeamSettingsInChat={selected === conversationId ? opener => settings.open('general', opener) : undefined} />
      </div>
      {drawer ? <TeamDetailsDrawer snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar}
        onClose={() => setDrawer(false)} onAddFriend={friends ? setAdding : undefined} /> : null}
    </TeamSettingsSurface>
    {adding && friends && snapshot.team ? <MemberFriend friends={friends} teams={workspace.api} uid={snapshot.uid} teamId={snapshot.team.id} member={adding} locale={locale} onClose={() => setAdding(undefined)} /> : null}
  </div>;
}
