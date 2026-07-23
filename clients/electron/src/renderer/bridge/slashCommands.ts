import type { SlashCommandDto } from '@lingxi/bridge-client';

export interface ActiveSlashCommand {
  start: number;
  end: number;
  query: string;
}

/** Detect a slash-command token at the caret, matching the TUI's line-leading semantics. */
export function activeSlashCommand(text: string, cursor: number): ActiveSlashCommand | null {
  if (!Number.isInteger(cursor) || cursor < 0 || cursor > text.length) return null;
  const before = text.slice(0, cursor);
  const match = /(?:^|\n)[\t ]*\/([^\s/]*)$/.exec(before);
  if (!match) return null;
  const query = match[1] ?? '';
  if (query.length > 128) return null;
  const start = before.lastIndexOf('/');
  return start < 0 ? null : { start, end: cursor, query };
}

function commandScore(command: SlashCommandDto, query: string): number | undefined {
  const name = command.name.toLocaleLowerCase();
  const normalized = query.toLocaleLowerCase();
  if (!normalized) return 0;
  if (name === normalized) return 0;
  if (name.startsWith(normalized)) return 10;
  if (name.includes(normalized)) return 20;
  if (command.description.toLocaleLowerCase().includes(normalized)) return 30;
  if (command.source.toLocaleLowerCase().includes(normalized)) return 40;
  return undefined;
}

/** Rank the live engine catalog without inventing commands in the renderer. */
export function filterSlashCommands(
  commands: readonly SlashCommandDto[],
  query: string,
  limit = 100,
): SlashCommandDto[] {
  const boundedLimit = Number.isSafeInteger(limit) && limit > 0 ? Math.min(limit, 100) : 100;
  const seen = new Set<string>();
  return commands
    .map((command) => ({ command, score: commandScore(command, query) }))
    .filter((entry): entry is { command: SlashCommandDto; score: number } => entry.score !== undefined)
    .sort((left, right) => (
      left.score - right.score
      || (query ? left.command.name.length - right.command.name.length : 0)
      || left.command.name.localeCompare(right.command.name)
    ))
    .filter(({ command }) => {
      const key = command.name.toLocaleLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    })
    .slice(0, boundedLimit)
    .map(({ command }) => command);
}

export function slashCommandText(name: string): string {
  return `/${name.replace(/^\/+/, '')}`;
}

export function reconcileSlashSelectionIndex(
  index: number,
  previousQuery: string | null,
  nextQuery: string,
  resultCount: number,
): number {
  if (previousQuery !== nextQuery || resultCount <= 0) return 0;
  return Math.max(0, Math.min(index, resultCount - 1));
}

export function moveSlashSelectionIndex(
  index: number,
  direction: 'previous' | 'next',
  resultCount: number,
): number {
  if (resultCount <= 0) return 0;
  const delta = direction === 'next' ? 1 : -1;
  return Math.max(0, Math.min(index + delta, resultCount - 1));
}

export function slashNavigationDirection(key: string): 'previous' | 'next' | null {
  if (key === 'ArrowDown' || key === 'Down') return 'next';
  if (key === 'ArrowUp' || key === 'Up') return 'previous';
  return null;
}
