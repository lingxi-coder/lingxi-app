import { useEffect } from 'react';
import { FriendsMain } from '../../../../../packages/teams/FriendsMain';
import { useTeamsText } from '../../../../../packages/teams/i18n';
import type { UseFriends } from '../../../../../packages/teams/useFriendsWorkspace';
import { renderTeamAvatar } from '../../teams/teamAvatar';
import { renderMarkdown } from '../markdown';
import { PaneNotice } from './PaneNotice';

/**
 * A friend's chat. `uid` names the friend; `pairTeamId` (from a shared team/chat link, which both people can open)
 * names the friendship instead and is resolved to the friend once the cards are in.
 */
export function FriendPane({ friends: { workspace, snapshot }, uid, pairTeamId, locale }: { friends: UseFriends; uid?: string; pairTeamId?: string; locale: string }) {
  const t = useTeamsText(locale);
  const zh = locale === 'zh';
  const friend = uid ?? snapshot?.cards.find(card => card.state === 'friends' && card.teamId === pairTeamId)?.uid;
  useEffect(() => { if (workspace && friend) workspace.openChat(friend); }, [workspace, friend]);
  if (!workspace || !snapshot || snapshot.loading) return <PaneNotice busy text={t('teams_loading')} />;
  if (!friend) return <PaneNotice title={zh ? '无法打开这个对话' : 'This chat is not available'}
    text={zh ? '你们已不是好友，或者链接已过期。' : 'You are no longer friends, or the link has expired.'} action={{ href: '/friends/add', label: zh ? '添加好友' : 'Add a friend' }} />;
  return <div className="chat-pane-conversation">
    <FriendsMain shareLink workspace={workspace} snapshot={snapshot} locale={locale} renderAvatar={renderTeamAvatar} renderMarkdown={renderMarkdown} />
  </div>;
}
