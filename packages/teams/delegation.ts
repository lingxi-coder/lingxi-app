import type { MemberRef, SharedBotCard, TeamTaskView } from './execution.js';

/**
 * What a task and a Bot's card say of Bots asking Bots (Bot v3 B8), without a screen and without words of their own: the run card, the Bot's card and the auto-reply
 * log put them into the language of the person. iOS (`TeamDelegations`) and Android say the same.
 */
type Named = Pick<SharedBotCard, 'sharedBotId' | 'name'>;

/** What a Bot is called: by its card as the Team lists it now, else by the name it had when it was asked. An id is never a name. */
function nameOf(sharedBotId: string, bots: readonly Named[], asked?: string): string | undefined {
  const named = bots.find(bot => bot.sharedBotId === sharedBotId)?.name;
  if (named) return named;
  return asked && asked !== sharedBotId ? asked : undefined;
}

/** "A → B": the Bots from the one the person asked to the one that runs this task. Nothing for a task nobody delegated. `unnamed` stands for a Bot with no name to give. */
export function delegationChain(task: Pick<TeamTaskView, 'delegation'>, bots: readonly Named[], unnamed: string): string | undefined {
  const chain = task.delegation?.chain;
  return chain && chain.length > 1 ? chain.map(link => nameOf(link.sharedBotId, bots, link.name) ?? unnamed).join(' → ') : undefined;
}

/**
 * The Bot a task that waits for another Bot's reply waits for: by its card, else by the name it had when it was asked, which the task it asked carries in its chain when that
 * task is among `known`. Nothing for a task that waits for something else, or a Bot with no name to give.
 */
export function waitingForBot(task: Pick<TeamTaskView, 'state' | 'wait'>, bots: readonly Named[], known: readonly Pick<TeamTaskView, 'taskId' | 'delegation'>[] = []): string | undefined {
  const asked = task.state === 'waiting' && task.wait?.kind === 'delegation' ? task.wait.delegation : undefined;
  if (!asked) return undefined;
  const link = known.find(other => other.taskId === asked.taskId)?.delegation?.chain.find(item => item.sharedBotId === asked.sharedBotId);
  return nameOf(asked.sharedBotId, bots, link?.name);
}

/**
 * Who may stop a task: the person who asked, the maintainer of the Bot that runs it and, for a task another Bot asked for, the maintainer of the Bot that asked. The service's
 * `canCancel` (`delegations.mjs`) says the same; it is the one that decides.
 */
export function mayCancel(task: Pick<TeamTaskView, 'requester' | 'sharedBotId' | 'delegation'>, viewer: MemberRef, bots: readonly Pick<SharedBotCard, 'sharedBotId' | 'maintainer'>[]): boolean {
  const is = (who: MemberRef) => who.uid === viewer.uid && who.generation === viewer.generation;
  const maintains = (sharedBotId: string) => bots.some(bot => bot.sharedBotId === sharedBotId && is(bot.maintainer));
  return is(task.requester) || maintains(task.sharedBotId) || (task.delegation !== undefined && maintains(task.delegation.parentBotId));
}

const namesOf = (ids: readonly string[] | undefined, bots: readonly Named[]): string[] => (ids ?? []).flatMap(id => bots.find(bot => bot.sharedBotId === id)?.name ?? []);

/** What a published Bot takes from other Bots: nobody, any Bot of the Team, or the Bots it lists that are still in the Team (a list of Bots that are all gone takes nobody). */
export type DelegationAccepts = { mode: 'none' } | { mode: 'team_bots' } | { mode: 'listed'; names: string[] };
export function delegationAccepts(bot: Pick<SharedBotCard, 'acceptDelegation' | 'acceptFrom'>, bots: readonly Named[]): DelegationAccepts {
  if (bot.acceptDelegation === 'team_bots') return { mode: 'team_bots' };
  const names = bot.acceptDelegation === 'listed' ? namesOf(bot.acceptFrom, bots) : [];
  return names.length ? { mode: 'listed', names } : { mode: 'none' };
}
/** The Bots a published Bot may hand part of a request to, by name; the ones no longer in the Team are left out. */
export const delegatesTo = (bot: Pick<SharedBotCard, 'delegateTo'>, bots: readonly Named[]): string[] => namesOf(bot.delegateTo, bots);
