export interface SlashCommandMessage {
  name: string;
  arguments: string;
}

/** Preserve the exact source text for the editor's visual layer, including argument whitespace. */
export function parseSlashCommandPrefix(text: string): { name: string; prefix: string; rest: string } | null {
  const match = /^([\t ]*\/([a-z0-9][a-z0-9:._-]*))(?=\s|$)/i.exec(text);
  if (!match) return null;
  return { name: match[2]!, prefix: match[1]!, rest: text.slice(match[1]!.length) };
}

/** Match an invocation, including namespaced commands and multiline arguments. */
export function parseSlashCommandMessage(text: string): SlashCommandMessage | null {
  const command = parseSlashCommandPrefix(text.trim());
  if (!command) return null;
  return {
    name: command.name,
    arguments: command.rest.trim(),
  };
}
