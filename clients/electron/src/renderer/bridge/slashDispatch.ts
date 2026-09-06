/**
 * Desktop-local slash dispatch — the client-side half of the command surface.
 *
 * The engine owns the catalog; this module owns the short list of commands the
 * desktop answers better than the engine's headless fallback (which replies
 * "/x is available in interactive TUI mode only" — see
 * `commands/core/src/register.rs:419`). It mirrors the TUI's `BUILTIN` table
 * (`tui/src/command.rs:88`) and is deliberately free of React and `window`, so
 * the whole resolution path is unit-testable.
 */
import type { PermissionModeId, SlashCommandDto } from '@lingxi/bridge-client';

export interface ParsedSlashLine {
  readonly name: string;
  /** The trimmed argument tail; `''` when the command was invoked bare. */
  readonly args: string;
}

/** Split a line-leading slash command into its name and argument tail. */
export function parseSlashLine(raw: string): ParsedSlashLine | null {
  const match = /^\s*\/([^\s/]+)\s*([\s\S]*)$/.exec(raw);
  if (!match) return null;
  return { name: match[1]!, args: match[2]!.trim() };
}

/** What a desktop-local command may do. Built by the composer, never imported by the table. */
export interface DesktopCommandContext {
  setModel(model: string): Promise<void>;
  knownModel(model: string): boolean;
  setPermissionMode(mode: PermissionModeId): Promise<void>;
  setReasoningLevel(level: string): Promise<void>;
  setReasoningAutomatic(): Promise<void>;
  setReasoningDisabled(): Promise<void>;
  setFastMode(enabled: boolean): Promise<void>;
  fastMode(): boolean;
  setTheme(theme: 'dark' | 'light'): void;
  openModelPicker(section: 'model' | 'effort'): void;
  openPermissionPicker(): void;
  openSettings(): void;
  openSettingsPage(pageId: string): void;
  addWorkspaceDirectory(path: string): Promise<void>;
  chooseProject(): Promise<void>;
  activateProject(path: string): Promise<void>;
  clearSession(): Promise<void>;
  forceCompact(instructions?: string): Promise<void>;
  copyLastResponse(): Promise<boolean>;
  login(): Promise<void>;
  logout(): Promise<void>;
  reloadPlugins(): Promise<void>;
  openTasks(): Promise<void>;
  showHelp(): void;
  /** Push a line of the command's own output into the transcript. */
  emit(output: string, isError?: boolean): void;
}

export interface DesktopCommand {
  readonly name: string;
  readonly aliases?: readonly string[];
  readonly args: 'none' | 'optional' | 'required';
  /** False keeps a directly-typed compatibility handler out of menus. */
  readonly advertised?: boolean;
  run(args: string, ctx: DesktopCommandContext): Promise<void> | void;
}

/**
 * Find the desktop command for a raw line, or `null` to forward it to the
 * engine. A `required` command invoked bare forwards on purpose, matching
 * `ArgSpec::Required` (`tui/src/command.rs:31`).
 */
export function resolveDesktopCommand(
  raw: string,
  table: readonly DesktopCommand[],
): { command: DesktopCommand; args: string } | null {
  const parsed = parseSlashLine(raw);
  if (!parsed) return null;
  const name = parsed.name.toLocaleLowerCase();
  const command = table.find((entry) => (
    entry.name === name || (entry.aliases ?? []).includes(name)
  ));
  if (!command) return null;
  if (command.args === 'required' && !parsed.args) return null;
  // A `none`-args command still resolves when arguments were supplied, deliberately,
  // so its `run` can tell the user about the misuse.
  return { command, args: parsed.args };
}

/** Preserve registry shadowing: project/plugin/skill commands beat same-named Desktop builtins. */
export function desktopCommandIsShadowed(
  raw: string,
  catalog: readonly SlashCommandDto[],
): boolean {
  const parsed = parseSlashLine(raw);
  if (!parsed) return false;
  const name = parsed.name.toLocaleLowerCase();
  return catalog.some((command) => (
    command.source !== 'builtin'
    && (
      command.name.toLocaleLowerCase() === name
      || (command.aliases ?? []).some((alias) => alias.toLocaleLowerCase() === name)
    )
  ));
}
