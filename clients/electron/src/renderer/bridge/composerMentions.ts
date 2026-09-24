import type { SkillDto } from '@lingxi/bridge-client';
import { fileMentionsFromPrompt } from './fileMentions';

export type ContextMention = { kind: 'skill' | 'plugin' | 'file' | 'folder'; name: string; target: string };
export type MentionAction = 'files' | 'attach' | 'goal' | 'plan';
export interface MentionMenuEntry {
  id: string;
  name: string;
  description: string;
  group: 'Add' | 'Plugins' | 'Skills' | 'Files and folders';
  icon: string;
  action?: MentionAction;
  path?: string;
  mention?: ContextMention;
  disabled?: boolean;
}

function record(value: unknown): Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function envelope(value?: string): Record<string, unknown> {
  try { return record(JSON.parse(value ?? '{}')); } catch { return {}; }
}

function entries(value: unknown): Record<string, unknown>[] {
  return Array.isArray(value) ? value.map(record) : [];
}

function string(value: unknown): string {
  return typeof value === 'string' && !/[\u0000-\u001f\u007f]/.test(value) ? value : '';
}

/** Use the effective plugin policy, never a single settings layer or marketplace suggestions. */
export function mentionCatalog(input: {
  skills?: readonly SkillDto[];
  skillCatalog?: string;
  pluginCatalog?: string;
  effectiveSettings?: string;
}): MentionMenuEntry[] {
  const result: MentionMenuEntry[] = [];
  const enabled = record(envelope(input.effectiveSettings).enabledPlugins);
  for (const plugin of entries(envelope(input.pluginCatalog).installed)) {
    const id = string(plugin.id) || string(plugin.name);
    const name = string(plugin.display_name) || string(plugin.name) || id;
    if (!id || !name || (typeof enabled[id] === 'boolean' ? !enabled[id] : plugin.default_enabled === false)) continue;
    result.push({ id: `plugin:${id}`, name, description: string(plugin.description) || id,
      group: 'Plugins', icon: 'puzzle', mention: { kind: 'plugin', name, target: id } });
  }
  const discovered = input.skills ?? [];
  const metadata = entries(envelope(input.skillCatalog).entries);
  // The discovered listing is authoritative: disabled plugins' skills can still
  // appear in the administration catalog, which is used only for descriptions.
  for (const skill of discovered) {
    if (!string(skill.name) || !string(skill.source_dir)) continue;
    const detail = metadata.find((entry) => entry.directory === skill.source_dir);
    const target = `${skill.source_dir.replace(/[\\/]+$/, '')}/SKILL.md`;
    result.push({ id: `skill:${target}`, name: skill.name,
      description: string(detail?.description) || string(detail?.whenToUse) || skill.source_dir,
      group: 'Skills', icon: 'sparkle', mention: { kind: 'skill', name: skill.name, target } });
  }
  const seen = new Set<string>();
  return result.filter((entry) => {
    if (seen.has(entry.id)) return false;
    seen.add(entry.id);
    return true;
  });
}

export function mentionMenuEntries(input: {
  query: string;
  filesOnly: boolean;
  files: readonly string[];
  catalog: readonly MentionMenuEntry[];
  running: boolean;
  planActive: boolean;
  goalAvailable: boolean;
  goalHasAttachments?: boolean;
}): MentionMenuEntry[] {
  const query = input.query.trim().toLocaleLowerCase();
  const actions: MentionMenuEntry[] = [
    { id: 'files', name: 'Files and folders', description: 'Search this workspace', group: 'Add', icon: 'folder', action: 'files' },
    { id: 'attach', name: 'Attach files', description: 'Add files or images from your computer', group: 'Add', icon: 'image', action: 'attach' },
    ...(input.goalAvailable ? [{ id: 'goal', name: 'Goal', description: input.running ? 'Available after the current turn' : input.goalHasAttachments ? 'Remove attachments to set a goal' : 'Set a goal to keep pursuing', group: 'Add' as const, icon: 'goal', action: 'goal' as const, disabled: input.running || input.goalHasAttachments }] : []),
    { id: 'plan', name: 'Plan mode', description: input.planActive ? 'Plan mode is on' : 'Turn plan mode on', group: 'Add', icon: 'bulb', action: 'plan', disabled: input.running || input.planActive },
  ];
  const matches = (entry: MentionMenuEntry) => !query || `${entry.name} ${entry.description}`.toLocaleLowerCase().includes(query);
  const context = input.filesOnly ? [] : [...actions, ...input.catalog].filter(matches);
  const files = input.filesOnly || query ? input.files.map((path): MentionMenuEntry => ({
    id: `file:${path}`, name: path.replace(/\/$/, '').split('/').pop() ?? path,
    description: path, group: 'Files and folders', icon: path.endsWith('/') ? 'folder' : 'file', path,
  })) : [];
  return [...context, ...files];
}

/** A complete, recoverable hyperlink, restricted to known reference types. */
export function contextMentionHref(mention: ContextMention): string {
  const query = new URLSearchParams({ name: mention.name, target: mention.target });
  return `lingxi-mention://${mention.kind}?${query}`;
}

export function parseContextMentionHref(href: string): ContextMention | null {
  try {
    const url = new URL(href);
    const kind = url.hostname;
    const name = url.searchParams.get('name') ?? '';
    const target = url.searchParams.get('target') ?? '';
    if (url.protocol !== 'lingxi-mention:' || (kind !== 'skill' && kind !== 'plugin' && kind !== 'file' && kind !== 'folder')
      || url.username || url.password || url.port || url.pathname || url.hash
      || !string(name) || !string(target) || name.length > 512 || target.length > 4096) return null;
    return { kind, name, target };
  } catch { return null; }
}

export function contextMentionMarkdown(mention: ContextMention): string {
  const label = mention.name.replace(/[\\[\]]/g, '\\$&');
  // Keep the full human-readable target in the title as well as the URI, so
  // the engine can resolve the reference without relying on display labels.
  const target = mention.target.replace(/[\\"]/g, '\\$&');
  return `[@${label}](${contextMentionHref(mention)} "${target}")`;
}

/** Preserve the engine's @path format on the wire, and reconstruct links when displaying history. */
export function promptWithMentionLinks(text: string): string {
  const paths = fileMentionsFromPrompt(text);
  if (!paths.length) return text;
  const separator = text.indexOf('\n\n');
  const body = separator >= 0 ? text.slice(separator + 2) : '';
  const links = paths.map((target) => contextMentionMarkdown({
    kind: target.endsWith('/') ? 'folder' : 'file', target,
    name: target.replace(/\/$/, '').split(/[\\/]/).pop() ?? target,
  })).join(' ');
  return body ? `${links}\n\n${body}` : links;
}

export const OPEN_CONTEXT_MENTION_EVENT = 'lingxi:open-context-mention';

export function openContextMention(mention: ContextMention): void {
  window.dispatchEvent(new CustomEvent(OPEN_CONTEXT_MENTION_EVENT, { detail: mention }));
}
