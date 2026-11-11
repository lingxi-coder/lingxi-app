import type { TeamMember, TeamsApi, TeamSummary } from '../../../../packages/teams';
import { InviteFriendsDialog } from '../../../../packages/teams/friend-dialogs';
import type { FriendsApi } from '../../../../packages/teams/friends';
import { useFriendsWorkspace } from '../../../../packages/teams/useFriendsWorkspace';

/** D26 on the Teams page: a Friends workspace that lives as long as the picker, so the page runs no Friends listener otherwise. */
export function InviteFriends({ friends, teams, uid, team, members, zh, refreshMembers, onClose }: {
  friends: FriendsApi; teams: TeamsApi; uid: string; team: Pick<TeamSummary, 'id' | 'name'>; members: readonly Pick<TeamMember, 'uid'>[]; zh: boolean;
  refreshMembers(): void; onClose(invited: string[]): void;
}) {
  const { workspace } = useFriendsWorkspace(friends, teams, uid);
  return workspace ? <InviteFriendsDialog workspace={workspace} zh={zh} team={team} members={members} refreshMembers={refreshMembers} onClose={onClose} /> : null;
}
