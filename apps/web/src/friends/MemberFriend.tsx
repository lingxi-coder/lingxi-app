import type { TeamsApi, TeamMember } from '../../../../packages/teams';
import { MemberFriendDialog } from '../../../../packages/teams/friend-dialogs';
import type { FriendsApi } from '../../../../packages/teams/friends';
import { useFriendsWorkspace } from '../../../../packages/teams/useFriendsWorkspace';

/** D15 on the Teams page: a Friends workspace that lives as long as the dialog, so the page runs no Friends listener otherwise. */
export function MemberFriend({ friends, teams, uid, teamId, member, zh, onClose }: {
  friends: FriendsApi; teams: TeamsApi; uid: string; teamId: string; member: TeamMember; zh: boolean; onClose(): void;
}) {
  const { workspace } = useFriendsWorkspace(friends, teams, uid);
  return workspace ? <MemberFriendDialog workspace={workspace} zh={zh} teamId={teamId} member={member} onClose={onClose} /> : null;
}
