import type { SlashCommandDto } from '@lingxi/bridge-client';
import { parseSlashArguments } from './nativeSlashArgumentParser.js';

/** Claude Code 2.1.299's commandArgumentHint branch; return the original hint bytes. */
export function slashArgumentHint(
  text: string,
  cursorOffset: number,
  commands: readonly SlashCommandDto[],
): string | undefined {
  if (!text.startsWith('/') || cursorOffset <= 0 || !text.endsWith(' ')) return undefined;
  const separator = text.indexOf(' ');
  if (separator < 0) return undefined;
  const name = text.slice(1, separator);
  if (/[^a-zA-Z0-9.:_-]/.test(name)) return undefined;
  // An exact name wins over aliases, even when its catalog row comes later.
  const command = commands.find((entry) => entry.name === name)
    ?? commands.find((entry) => entry.aliases?.includes(name));
  if (!command) return undefined;
  if (text.length === separator + 1 && command.argument_hint) return command.argument_hint;
  // The protocol only publishes argument_names for prompt commands.
  if (!command.argument_names?.length) return undefined;
  const args = parseSlashArguments(text.slice(separator + 1));
  const remaining = command.argument_names.slice(args.length);
  return remaining.length ? remaining.map((argument) => `[${argument}]`).join(' ') : undefined;
}
