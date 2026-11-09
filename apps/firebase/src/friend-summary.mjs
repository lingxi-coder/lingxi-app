import { PAIR_CONVERSATION_ID, PAIR_PREFIX } from './teams.mjs';

const whole = value => Number.isSafeInteger(value) && value >= 0;
// Only a pair team's own chat is mirrored. The team id space is reserved, so this is a cheap first filter; the team's kind decides.
const isPairChat = (teamId, conversationId) => typeof teamId === 'string' && teamId.startsWith(PAIR_PREFIX) && conversationId === PAIR_CONVERSATION_ID;
const twoUids = members => Array.isArray(members) && members.length === 2 && members.every(member => typeof member === 'string' && member) && members[0] !== members[1];
const isPairTeam = async (db, teamId) => (await db.doc(`teams/${teamId}`).get()).data()?.kind === 'pair';

/** Friend list summaries (docs/FRIENDS.md): each `users/{uid}/friends/{other}` card carries the pair chat's newest-message
 * pointer, so a client listens to one collection instead of one conversation per friend.
 *
 * Every write is a transaction that reads the card first. It is made only while the card is still this very friendship
 * (`state: 'friends'` with this `teamId`), so an event that arrives after an unfriend, or from an older friendship of the
 * same two people, writes nothing. Counters never move backwards, and events are at-least-once and unordered: the card
 * keeps the `lastChangeSeq` it last took, and a conversation event that is not newer is ignored. An event carries the
 * whole pointer, so a lost event is healed by the next. */
export async function mirrorFriendChat({ db, teamId, conversationId, after }) {
  if (!isPairChat(teamId, conversationId) || !after || !twoUids(after.memberUids)) return;
  const { memberUids, lastSeq, lastChangeSeq, lastMessage = null } = after;
  if (!whole(lastSeq) || !whole(lastChangeSeq) || lastChangeSeq < 1 || (lastMessage !== null && typeof lastMessage !== 'object')) return;
  if (!(await isPairTeam(db, teamId))) return;
  await Promise.all([memberUids, [...memberUids].reverse()].map(([owner, peer]) => db.runTransaction(async tx => {
    const card = await tx.get(db.doc(`users/${owner}/friends/${peer}`)), current = card.data();
    if (current?.state !== 'friends' || current.teamId !== teamId || (whole(current.lastChangeSeq) ? current.lastChangeSeq : 0) >= lastChangeSeq) return;
    tx.update(card.ref, { lastSeq: Math.max(whole(current.lastSeq) ? current.lastSeq : 0, lastSeq), lastMessage, lastChangeSeq });
  }, { maxAttempts: 20 })));
}

/** A user's read mark (`reads/{uid}`) goes to that user's own card only, and only forwards. A mark made under another
 * membership generation than the card's is not the card's. Unread is `max(0, lastSeq - lastReadSeq)` on the card. */
export async function mirrorFriendRead({ db, teamId, conversationId, uid, after }) {
  if (!isPairChat(teamId, conversationId) || typeof uid !== 'string' || !after || !whole(after.lastReadSeq)) return;
  // The conversation outlives an unfriend and names both members, which is how the card of `uid` is found.
  const [team, conversation] = await db.getAll(db.doc(`teams/${teamId}`), db.doc(`teams/${teamId}/conversations/${conversationId}`));
  const members = conversation.data()?.memberUids;
  if (team.data()?.kind !== 'pair' || !twoUids(members) || !members.includes(uid)) return;
  const peer = members.find(member => member !== uid);
  await db.runTransaction(async tx => {
    const card = await tx.get(db.doc(`users/${uid}/friends/${peer}`)), current = card.data();
    if (current?.state !== 'friends' || current.teamId !== teamId || current.generation !== after.generation
      || (whole(current.lastReadSeq) ? current.lastReadSeq : 0) >= after.lastReadSeq) return;
    tx.update(card.ref, { lastReadSeq: after.lastReadSeq });
  }, { maxAttempts: 20 });
}
