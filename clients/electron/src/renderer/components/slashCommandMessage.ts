export interface SlashCommandMessage {
  name: string;
  arguments: string;
}

/** Parse only a complete, single-line slash command message. */
export function parseSlashCommandMessage(text: string): SlashCommandMessage | null {
  const match = /^\s*\/([a-z0-9][a-z0-9_-]*)(?:[\t ]+([^\r\n]*?))?[\t ]*$/i.exec(text);
  if (!match) return null;
  return {
    name: match[1] ?? '',
    arguments: match[2]?.trim() ?? '',
  };
}
