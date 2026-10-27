/**
 * What a Bot is made of besides its record: the persona (`LINGXI.md`), what it
 * remembers (the markdown files of its memory folder) and its skills. Free of
 * Electron. Every name that comes from outside is checked before it becomes a
 * path, and nothing outside the Bot's own home is read or written except the
 * one skill that is copied from the owner's.
 */

import { chmod, copyFile, lstat, mkdir, open, readFile, readdir, realpath, rename, rm, stat, unlink } from 'node:fs/promises';
import { constants } from 'node:fs';
import { randomUUID } from 'node:crypto';
import { join, relative, sep } from 'node:path';

import type { BotMemoryEntryUi, BotSkillUi } from '../../shared/bots.js';
import { writeFileAtomic } from './files.js';
import { personaText, MAX_PERSONA_BYTES, type BotStore } from './store.js';

export const MAX_MEMORY_BYTES = 128 * 1024;
export const MAX_MEMORY_FILES = 500;
export const MAX_SKILLS = 200;
export const MAX_SKILL_FILES = 512;
export const MAX_SKILL_BYTES = 16 * 1024 * 1024;
const MAX_SKILL_DEPTH = 8;
const FRONT_MATTER_BYTES = 8 * 1024;
const MEMORY_NAME = /^[\p{L}\p{N}][\p{L}\p{N}_ .-]{0,120}\.md$/u;
const SKILL_NAME = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/;

export class BotContentNotFoundError extends Error {
  constructor(what: string) {
    super(`${what} not found`);
    this.name = 'BotContentNotFoundError';
  }
}

export function memoryName(value: unknown): string {
  if (typeof value !== 'string' || !MEMORY_NAME.test(value) || value.includes('..')) throw new Error('invalid memory name');
  return value;
}

export function skillName(value: unknown): string {
  if (typeof value !== 'string' || !SKILL_NAME.test(value) || value.includes('..')) throw new Error('invalid skill name');
  return value;
}

/** The `key: value` lines of a leading `---` block. Quotes around a value are dropped. */
export function frontMatter(text: string): Record<string, string> {
  const match = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)/.exec(text);
  const fields: Record<string, string> = {};
  if (!match) return fields;
  for (const line of match[1]!.split(/\r?\n/)) {
    const field = /^([A-Za-z][\w-]*):\s*(.*)$/.exec(line);
    if (field) fields[field[1]!] = field[2]!.trim().replace(/^(["'])(.*)\1$/, '$2').slice(0, 300);
  }
  return fields;
}

function missing(error: unknown): boolean {
  return (error as NodeJS.ErrnoException).code === 'ENOENT';
}

/** The text of a regular file of at most `maxBytes`; a link or anything else is refused. */
async function readRegularText(path: string, maxBytes: number, what: string): Promise<string> {
  let info;
  try { info = await lstat(path); }
  catch (error) { throw missing(error) ? new BotContentNotFoundError(what) : error; }
  if (!info.isFile()) throw new Error(`${what} is not a regular file`);
  if (info.size > maxBytes) throw new Error(`${what} is too large`);
  return readFile(path, 'utf8');
}

/** The first bytes of a regular file, for its front matter; a missing file or a link has none. */
async function readHead(path: string): Promise<string> {
  let handle;
  try { handle = await open(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0)); }
  catch (error) {
    if (missing(error) || (error as NodeJS.ErrnoException).code === 'ELOOP') return '';
    throw error;
  }
  try {
    const buffer = Buffer.alloc(FRONT_MATTER_BYTES);
    const { bytesRead } = await handle.read(buffer, 0, FRONT_MATTER_BYTES, 0);
    return buffer.subarray(0, bytesRead).toString('utf8');
  } finally {
    await handle.close();
  }
}

export class BotContent {
  constructor(private readonly store: BotStore) {}

  // Persona

  async persona(botId: string): Promise<string> {
    this.store.get(botId);
    try { return await readRegularText(join(this.store.homeDir(botId), 'LINGXI.md'), MAX_PERSONA_BYTES, 'persona'); }
    catch (error) {
      if (error instanceof BotContentNotFoundError) return '';
      throw error;
    }
  }

  async setPersona(botId: string, text: unknown): Promise<void> {
    this.store.get(botId);
    writeFileAtomic(join(this.store.homeDir(botId), 'LINGXI.md'), personaText(text));
  }

  // Memory

  /**
   * The memory folder of the Bot, made if `create` says so. Every step from the
   * Bot's home down must be a directory of its own: a link there could lead
   * out of the Bot, so it is refused rather than followed.
   */
  private async memoryDirectory(botId: string, create: boolean): Promise<string | undefined> {
    this.store.get(botId);
    const home = this.store.homeDir(botId);
    let current = home;
    for (const part of relative(home, this.store.memoryDir(botId)).split(sep)) {
      current = join(current, part);
      let info;
      try { info = await lstat(current); }
      catch (error) {
        if (!missing(error)) throw error;
        if (!create) return undefined;
        await mkdir(current, { mode: 0o700 });
        continue;
      }
      if (!info.isDirectory()) throw new Error('the memory folder is not a regular directory');
    }
    return current;
  }

  async listMemory(botId: string): Promise<BotMemoryEntryUi[]> {
    const directory = await this.memoryDirectory(botId, false);
    if (directory === undefined) return [];
    const entries: BotMemoryEntryUi[] = [];
    for (const dirent of await readdir(directory, { withFileTypes: true })) {
      if (!dirent.isFile() || !MEMORY_NAME.test(dirent.name) || dirent.name.includes('..')) continue;
      if (entries.length >= MAX_MEMORY_FILES) break;
      const path = join(directory, dirent.name);
      const info = await lstat(path);
      const fields = frontMatter(await readHead(path));
      entries.push({
        name: dirent.name,
        ...(fields['description'] ? { description: fields['description'] } : {}),
        ...(fields['type'] ? { type: fields['type'] } : {}),
        size: info.size,
        modifiedAt: Math.floor(info.mtimeMs),
      });
    }
    return entries.sort((left, right) => left.name.localeCompare(right.name));
  }

  async readMemory(botId: string, name: unknown): Promise<string> {
    const file = memoryName(name);
    const directory = await this.memoryDirectory(botId, false);
    if (directory === undefined) throw new BotContentNotFoundError('memory');
    return readRegularText(join(directory, file), MAX_MEMORY_BYTES, 'memory');
  }

  async writeMemory(botId: string, name: unknown, content: unknown): Promise<void> {
    const file = memoryName(name);
    if (typeof content !== 'string' || content.includes('\0') || Buffer.byteLength(content, 'utf8') > MAX_MEMORY_BYTES) throw new Error('invalid memory content');
    const directory = await this.memoryDirectory(botId, true);
    if (directory === undefined) throw new Error('the memory folder is unavailable');
    writeFileAtomic(join(directory, file), content);
  }

  async deleteMemory(botId: string, name: unknown): Promise<void> {
    const file = memoryName(name);
    const directory = await this.memoryDirectory(botId, false);
    if (directory === undefined) throw new BotContentNotFoundError('memory');
    const path = join(directory, file);
    let info;
    try { info = await lstat(path); }
    catch (error) { throw missing(error) ? new BotContentNotFoundError('memory') : error; }
    if (!info.isFile()) throw new Error('memory is not a regular file');
    await unlink(path);
  }

  // Skills

  private skillsDirectory(botId: string): string {
    this.store.get(botId);
    return join(this.store.homeDir(botId), 'skills');
  }

  async listSkills(botId: string): Promise<BotSkillUi[]> {
    const directory = this.skillsDirectory(botId);
    let dirents;
    try { dirents = await readdir(directory, { withFileTypes: true }); }
    catch (error) {
      if (missing(error)) return [];
      throw error;
    }
    const skills: BotSkillUi[] = [];
    for (const dirent of dirents) {
      if (!dirent.isDirectory() || !SKILL_NAME.test(dirent.name) || dirent.name.includes('..')) continue;
      if (skills.length >= MAX_SKILLS) break;
      const fields = frontMatter(await readHead(join(directory, dirent.name, 'SKILL.md')));
      skills.push({ name: dirent.name, ...(fields['description'] ? { description: fields['description'] } : {}) });
    }
    return skills.sort((left, right) => left.name.localeCompare(right.name));
  }

  async deleteSkill(botId: string, name: unknown): Promise<void> {
    const skill = skillName(name);
    const path = join(this.skillsDirectory(botId), skill);
    let info;
    try { info = await lstat(path); }
    catch (error) { throw missing(error) ? new BotContentNotFoundError('skill') : error; }
    if (!info.isDirectory()) throw new Error('skill is not a directory');
    await rm(path, { recursive: true });
  }

  /**
   * Copy `<default configuration home>/skills/<name>` into the Bot, once. The
   * copy does not follow the original afterwards. A link inside the skill, a
   * file that is not a regular file, or a skill that is too large is refused:
   * a link would carry something from outside the skill into the Bot.
   */
  async importUserSkill(botId: string, name: unknown): Promise<BotSkillUi> {
    const skill = skillName(name);
    const skills = this.skillsDirectory(botId);
    const destination = join(skills, skill);
    const source = join(this.store.locations.defaultConfigHomes[0]!, 'skills', skill);
    let sourceInfo;
    try { sourceInfo = await stat(source); }
    catch (error) { throw missing(error) ? new BotContentNotFoundError('skill') : error; }
    if (!sourceInfo.isDirectory()) throw new Error('skill is not a directory');
    const exists = await lstat(destination).then(() => true, (error: unknown) => {
      if (missing(error)) return false;
      throw error;
    });
    if (exists) throw new Error(`The Bot already has a skill named ${skill}.`);
    const real = await realpath(source);
    const files: Array<{ from: string; relative: string; mode: number }> = [];
    const directories: string[] = [];
    let bytes = 0;
    const scan = async (directory: string, relative: string, depth: number): Promise<void> => {
      if (depth > MAX_SKILL_DEPTH) throw new Error('The skill is nested too deeply.');
      for (const dirent of await readdir(directory, { withFileTypes: true })) {
        const from = join(directory, dirent.name);
        const path = relative === '' ? dirent.name : join(relative, dirent.name);
        if (dirent.isDirectory()) {
          directories.push(path);
          await scan(from, path, depth + 1);
        } else if (dirent.isFile()) {
          const info = await lstat(from);
          bytes += info.size;
          if (files.length >= MAX_SKILL_FILES || bytes > MAX_SKILL_BYTES) throw new Error('The skill is too large to import.');
          files.push({ from, relative: path, mode: info.mode });
        } else {
          throw new Error(`The skill contains something other than files and folders: ${path}`);
        }
      }
    };
    await scan(real, '', 0);
    if (!files.some((file) => file.relative === 'SKILL.md')) throw new Error('The folder is not a skill: it has no SKILL.md.');
    const staging = join(skills, `.import-${randomUUID()}`);
    try {
      await mkdir(staging, { recursive: true, mode: 0o700 });
      for (const directory of directories) await mkdir(join(staging, directory), { recursive: true, mode: 0o700 });
      for (const file of files) {
        const target = join(staging, file.relative);
        await copyFile(file.from, target);
        await chmod(target, (file.mode & 0o700) | 0o600);
      }
      await rename(staging, destination);
    } catch (error) {
      await rm(staging, { recursive: true, force: true });
      throw error;
    }
    const fields = frontMatter(await readHead(join(destination, 'SKILL.md')));
    return { name: skill, ...(fields['description'] ? { description: fields['description'] } : {}) };
  }
}
