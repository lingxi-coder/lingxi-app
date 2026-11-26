/**
 * Whether a Chat message talks to a Bot without asking it: a reply to the Bot's own message, or its name in the text. The service decides this once
 * when it admits the message (`optionalTasks`), so every Desktop agrees; the host no longer judges it (`apps/electron/src/main/teams/replyTriage.ts`).
 */
const fold = text => text.normalize('NFKC').toLowerCase();
/** A letter or digit of a script that puts spaces between words: next to one, a name is only part of a longer word. */
const WORD = /[\p{Script=Latin}\p{Script=Greek}\p{Script=Cyrillic}\p{Nd}]/u;
/** Whether `text` calls `name`: as a whole word where the name begins or ends with a letter or digit of such a script, anywhere otherwise. */
function names(text, name) {
  const open = WORD.test(name[0]), close = WORD.test(name[name.length - 1]);
  for (let at = text.indexOf(name); at >= 0; at = text.indexOf(name, at + 1)) {
    if ((!open || at === 0 || !WORD.test(text[at - 1])) && (!close || at + name.length === text.length || !WORD.test(text[at + name.length]))) return true;
  }
  return false;
}
/** Whether `text` calls the Bot called `botName`, folded for case and width. At least two characters (not UTF-16 units): one letter, or one emoji, would match far too much. */
export function calledBy(text, botName) {
  const name = fold(botName.trim());
  return [...name].length >= 2 && names(fold(text), name);
}
/** The Bots of `bots` (`{ sharedBotId, name }`) that a message addresses without asking: the author of the message it answers, and those whose name it calls. */
export function addressedBots(bots, { text, replied }) {
  const answered = replied?.author?.kind === 'shared_bot' ? replied.author.sharedBotId : undefined;
  return bots.filter(({ sharedBotId, name }) => sharedBotId === answered || (typeof name === 'string' && calledBy(text, name))).map(bot => bot.sharedBotId);
}
