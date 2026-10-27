import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

import { BotContent, BotContentNotFoundError, MAX_MEMORY_BYTES, MAX_MEMORY_FILES, frontMatter, memoryName, skillName } from '../src/main/bots/content.js';
import { BotNotFoundError } from '../src/main/bots/store.js';
import { botFixture } from './bot-helpers.js';

function fixture(t: { after(fn: () => void): void }) {
  const f = botFixture(t);
  const bot = f.store.create({ name: 'Content', persona: 'You are Content.' });
  const content = new BotContent(f.store);
  const memory = f.store.memoryDir(bot.bot_id);
  const skills = join(f.store.homeDir(bot.bot_id), 'skills');
  const outside = join(f.root, 'outside');
  mkdirSync(outside, { recursive: true });
  writeFileSync(join(outside, 'secret.md'), 'not for the Bot');
  return { ...f, bot, id: bot.bot_id, content, memory, skills, outside };
}

const MISSING = 'Bot0000000000000000999';

// Persona

test('the persona is the text the Bot was made with, and can be replaced', async (t) => {
  const f = fixture(t);
  assert.equal(await f.content.persona(f.id), 'You are Content.');
  await f.content.setPersona(f.id, '# New\n\nBe brief.\n');
  assert.equal(await f.content.persona(f.id), '# New\n\nBe brief.\n');
  assert.equal(readFileSync(join(f.store.homeDir(f.id), 'LINGXI.md'), 'utf8'), '# New\n\nBe brief.\n');
  await f.content.setPersona(f.id, '');
  assert.equal(await f.content.persona(f.id), '');
});

test('the persona is bounded, text, and the persona of a Bot that exists', async (t) => {
  const f = fixture(t);
  for (const bad of ['x'.repeat(64 * 1024 + 1), 'nul\0', 5, null, undefined]) {
    await assert.rejects(f.content.setPersona(f.id, bad), /invalid persona/);
  }
  assert.equal(await f.content.persona(f.id), 'You are Content.');
  await f.content.setPersona(f.id, 'é'.repeat(32 * 1024));
  await assert.rejects(f.content.setPersona(f.id, 'é'.repeat(32 * 1024 + 1)), /invalid persona/, 'the bound is in bytes');
  await assert.rejects(f.content.persona(MISSING), BotNotFoundError);
  await assert.rejects(f.content.setPersona(MISSING, 'x'), BotNotFoundError);
  await assert.rejects(f.content.persona('../escape'), /invalid Bot id/);
  rmFile(join(f.store.homeDir(f.id), 'LINGXI.md'));
  assert.equal(await f.content.persona(f.id), '', 'no file is no persona');
});

test('a persona that is a link is not read, and writing a persona does not write through a link', async (t) => {
  const f = fixture(t);
  const persona = join(f.store.homeDir(f.id), 'LINGXI.md');
  rmFile(persona);
  symlinkSync(join(f.outside, 'secret.md'), persona);
  await assert.rejects(f.content.persona(f.id), /not a regular file/);
  await f.content.setPersona(f.id, 'Mine');
  assert.equal(readFileSync(join(f.outside, 'secret.md'), 'utf8'), 'not for the Bot');
  assert.equal(await f.content.persona(f.id), 'Mine');
});

// Memory

test('a Bot that remembers nothing has no memory folder to list', async (t) => {
  const f = fixture(t);
  assert.deepEqual(await f.content.listMemory(f.id), []);
  assert.ok(!existsSync(f.memory), 'listing does not make the folder');
  await assert.rejects(f.content.readMemory(f.id, 'x.md'), BotContentNotFoundError);
  await assert.rejects(f.content.deleteMemory(f.id, 'x.md'), BotContentNotFoundError);
});

test('memory is written, listed with what its front matter says, read back and deleted', async (t) => {
  const f = fixture(t);
  await f.content.writeMemory(f.id, 'user_role.md', '---\nname: user role\ndescription: "Who the owner is"\ntype: user\n---\nThe owner writes Rust.\n');
  await f.content.writeMemory(f.id, 'MEMORY.md', '- [user role](user_role.md)\n');
  await f.content.writeMemory(f.id, 'résumé 用户.md', '---\ndescription: unicode\n---\n');
  const listed = await f.content.listMemory(f.id);
  assert.deepEqual(listed.map((entry) => entry.name), ['MEMORY.md', 'résumé 用户.md', 'user_role.md'].sort((a, b) => a.localeCompare(b)));
  const role = listed.find((entry) => entry.name === 'user_role.md')!;
  assert.deepEqual({ description: role.description, type: role.type, size: role.size }, {
    description: 'Who the owner is', type: 'user', size: Buffer.byteLength('---\nname: user role\ndescription: "Who the owner is"\ntype: user\n---\nThe owner writes Rust.\n'),
  });
  assert.ok(Math.abs(role.modifiedAt - Date.now()) < 60_000);
  assert.ok(!('description' in listed.find((entry) => entry.name === 'MEMORY.md')!));
  assert.equal(await f.content.readMemory(f.id, 'user_role.md'), '---\nname: user role\ndescription: "Who the owner is"\ntype: user\n---\nThe owner writes Rust.\n');
  await f.content.writeMemory(f.id, 'user_role.md', 'replaced');
  assert.equal(await f.content.readMemory(f.id, 'user_role.md'), 'replaced');
  await f.content.deleteMemory(f.id, 'user_role.md');
  assert.deepEqual((await f.content.listMemory(f.id)).map((entry) => entry.name).includes('user_role.md'), false);
  await assert.rejects(f.content.readMemory(f.id, 'user_role.md'), BotContentNotFoundError);
  await assert.rejects(f.content.deleteMemory(f.id, 'user_role.md'), BotContentNotFoundError);
});

test('memory lives where the engine keeps it', async (t) => {
  const f = fixture(t);
  await f.content.writeMemory(f.id, 'where.md', 'here');
  assert.equal(readFileSync(join(f.memory, 'where.md'), 'utf8'), 'here');
  assert.ok(f.memory.startsWith(join(f.store.homeDir(f.id), 'projects') + '/'));
});

test('only the markdown files of the memory folder can be named, and a name is never a path', async (t) => {
  const f = fixture(t);
  writeFileSync(join(f.root, 'work-secret.md'), 'outside');
  for (const bad of [
    '../x.md', '../../LINGXI.md', 'a/b.md', 'a\\b.md', '/etc/passwd', '.hidden.md', '..md', 'x.txt', 'x', 'x.md.txt', '.md', ' .md', 'x.md\0',
    'a..b.md', '', 'x'.repeat(122) + '.md', 5, null, undefined, {}, ['x.md'],
  ]) {
    assert.throws(() => memoryName(bad), /invalid memory name/, JSON.stringify(bad));
    await assert.rejects(f.content.readMemory(f.id, bad), /invalid memory name/);
    await assert.rejects(f.content.writeMemory(f.id, bad, 'x'), /invalid memory name/);
    await assert.rejects(f.content.deleteMemory(f.id, bad), /invalid memory name/);
  }
  assert.equal(memoryName('x'.repeat(121) + '.md').length, 124);
  assert.deepEqual(readdirSync(join(f.root)).filter((name) => name.endsWith('.md')), ['work-secret.md']);
  assert.ok(!existsSync(f.memory), 'a refused name makes nothing');
});

test('memory is bounded, and what is not a regular markdown file is left out of the list', async (t) => {
  const f = fixture(t);
  await assert.rejects(f.content.writeMemory(f.id, 'big.md', 'x'.repeat(MAX_MEMORY_BYTES + 1)), /invalid memory content/);
  for (const bad of [1, null, undefined, 'nul\0']) await assert.rejects(f.content.writeMemory(f.id, 'bad.md', bad), /invalid memory content/);
  await f.content.writeMemory(f.id, 'limit.md', 'x'.repeat(MAX_MEMORY_BYTES));
  writeFileSync(join(f.memory, 'grown.md'), 'x'.repeat(MAX_MEMORY_BYTES + 1));
  await assert.rejects(f.content.readMemory(f.id, 'grown.md'), /too large/);
  writeFileSync(join(f.memory, 'notes.txt'), 'not markdown');
  mkdirSync(join(f.memory, 'folder.md'));
  mkdirSync(join(f.memory, 'sub'));
  writeFileSync(join(f.memory, 'sub', 'deep.md'), 'nested');
  symlinkSync(join(f.outside, 'secret.md'), join(f.memory, 'link.md'));
  assert.deepEqual((await f.content.listMemory(f.id)).map((entry) => entry.name).sort(), ['grown.md', 'limit.md']);
  await assert.rejects(f.content.deleteMemory(f.id, 'folder.md'), /not a regular file/);
});

test('the list of memory files is bounded', async (t) => {
  const f = fixture(t);
  mkdirSync(f.memory, { recursive: true });
  for (let index = 0; index < MAX_MEMORY_FILES + 5; index += 1) writeFileSync(join(f.memory, `m${String(index).padStart(4, '0')}.md`), '');
  assert.equal((await f.content.listMemory(f.id)).length, MAX_MEMORY_FILES);
});

test('a link in a memory file is neither followed nor written through', async (t) => {
  const f = fixture(t);
  mkdirSync(f.memory, { recursive: true });
  symlinkSync(join(f.outside, 'secret.md'), join(f.memory, 'evil.md'));
  await assert.rejects(f.content.readMemory(f.id, 'evil.md'), /not a regular file/);
  await assert.rejects(f.content.deleteMemory(f.id, 'evil.md'), /not a regular file/);
  assert.ok(existsSync(join(f.outside, 'secret.md')));
  await f.content.writeMemory(f.id, 'evil.md', 'now mine');
  assert.equal(readFileSync(join(f.outside, 'secret.md'), 'utf8'), 'not for the Bot');
  assert.equal(await f.content.readMemory(f.id, 'evil.md'), 'now mine');
});

test('a link in the way to the memory folder leads nowhere', async (t) => {
  const f = fixture(t);
  const home = f.store.homeDir(f.id);
  // The memory folder itself is a link.
  mkdirSync(join(home, 'projects', 'x'), { recursive: true });
  rmFile(join(home, 'projects'), true);
  mkdirSync(join(f.outside, 'projects-copy'), { recursive: true });
  symlinkSync(join(f.outside, 'projects-copy'), join(home, 'projects'));
  await assert.rejects(f.content.listMemory(f.id), /not a regular directory/);
  await assert.rejects(f.content.writeMemory(f.id, 'x.md', 'leak'), /not a regular directory/);
  assert.deepEqual(readdirSync(join(f.outside, 'projects-copy')), [], 'nothing was made outside');
  assert.deepEqual(readdirSync(f.outside).sort(), ['projects-copy', 'secret.md']);
});

test('memory of a Bot that does not exist cannot be reached', async (t) => {
  const f = fixture(t);
  await assert.rejects(f.content.listMemory(MISSING), BotNotFoundError);
  await assert.rejects(f.content.writeMemory(MISSING, 'x.md', 'x'), BotNotFoundError);
  await assert.rejects(f.content.readMemory('../..', 'x.md'), /invalid Bot id/);
});

test('front matter is the leading block of key: value lines', () => {
  assert.deepEqual(frontMatter('---\nname: a\ndescription: \'quoted\'\ntype: user\n---\nbody'), { name: 'a', description: 'quoted', type: 'user' });
  assert.deepEqual(frontMatter('---\r\ndescription: crlf\r\n---\r\n'), { description: 'crlf' });
  assert.deepEqual(frontMatter('no front matter\n---\ndescription: late\n---'), {});
  assert.deepEqual(frontMatter('---\ndescription: unterminated'), {});
  assert.equal(frontMatter(`---\ndescription: ${'x'.repeat(500)}\n---\n`)['description']!.length, 300);
});

// Skills

function writeSkill(root: string, name: string, files: Record<string, string> = {}): string {
  const directory = join(root, name);
  mkdirSync(directory, { recursive: true });
  const all = { 'SKILL.md': `---\nname: ${name}\ndescription: "Does ${name}"\n---\nSteps.\n`, ...files };
  for (const [path, body] of Object.entries(all)) {
    mkdirSync(join(directory, path, '..'), { recursive: true });
    writeFileSync(join(directory, path), body);
  }
  return directory;
}

test('skills are listed by name with their description, and only directories with a valid name', async (t) => {
  const f = fixture(t);
  assert.deepEqual(await f.content.listSkills(f.id), []);
  writeSkill(f.skills, 'review');
  writeSkill(f.skills, 'plain', { 'SKILL.md': 'no front matter' });
  mkdirSync(join(f.skills, '.import-leftover'));
  mkdirSync(join(f.skills, 'has space'));
  writeFileSync(join(f.skills, 'stray.md'), 'file');
  symlinkSync(f.outside, join(f.skills, 'linked'));
  assert.deepEqual(await f.content.listSkills(f.id), [{ name: 'plain' }, { name: 'review', description: 'Does review' }]);
});

test('a skill is deleted with everything in it, and a name is never a path', async (t) => {
  const f = fixture(t);
  writeSkill(f.skills, 'doomed', { 'scripts/run.sh': '#!/bin/sh\n' });
  await f.content.deleteSkill(f.id, 'doomed');
  assert.ok(!existsSync(join(f.skills, 'doomed')));
  await assert.rejects(f.content.deleteSkill(f.id, 'doomed'), BotContentNotFoundError);
  for (const bad of ['../x', '..', '.', 'a/b', '', '.hidden', 'a..b', 'x'.repeat(65), 5, null, undefined]) {
    assert.throws(() => skillName(bad), /invalid skill name/, JSON.stringify(bad));
    await assert.rejects(f.content.deleteSkill(f.id, bad), /invalid skill name/);
    await assert.rejects(f.content.importUserSkill(f.id, bad), /invalid skill name/);
  }
  assert.ok(existsSync(f.skills));
});

test('deleting a skill that is a link does not follow it', async (t) => {
  const f = fixture(t);
  symlinkSync(f.outside, join(f.skills, 'linked'));
  await assert.rejects(f.content.deleteSkill(f.id, 'linked'), /not a directory/);
  assert.ok(existsSync(join(f.outside, 'secret.md')));
});

test('a skill of the owner is copied into the Bot once, whole', async (t) => {
  const f = fixture(t);
  const source = writeSkill(join(f.defaultConfigHome, 'skills'), 'deploy', { 'scripts/run.sh': '#!/bin/sh\necho run\n', 'references/notes/one.md': 'one' });
  chmodSync(join(source, 'scripts', 'run.sh'), 0o755);
  const imported = await f.content.importUserSkill(f.id, 'deploy');
  assert.deepEqual(imported, { name: 'deploy', description: 'Does deploy' });
  for (const path of ['SKILL.md', 'scripts/run.sh', 'references/notes/one.md']) {
    assert.equal(readFileSync(join(f.skills, 'deploy', path), 'utf8'), readFileSync(join(source, path), 'utf8'), path);
  }
  assert.equal(statSync(join(f.skills, 'deploy', 'scripts', 'run.sh')).mode & 0o700, 0o700, 'a script stays runnable');
  assert.equal(statSync(join(f.skills, 'deploy', 'SKILL.md')).mode & 0o077, 0, 'and nothing is shared with others');
  assert.deepEqual(await f.content.listSkills(f.id), [{ name: 'deploy', description: 'Does deploy' }]);
  // One time: the copy does not follow the original, and a second import does not replace it.
  writeFileSync(join(source, 'SKILL.md'), '---\ndescription: changed\n---\n');
  await assert.rejects(f.content.importUserSkill(f.id, 'deploy'), /already has a skill named deploy/);
  assert.match(readFileSync(join(f.skills, 'deploy', 'SKILL.md'), 'utf8'), /Does deploy/);
  assert.equal(readFileSync(join(source, 'scripts', 'run.sh'), 'utf8'), '#!/bin/sh\necho run\n', 'the original is untouched');
  assert.deepEqual(readdirSync(f.skills), ['deploy'], 'no staging folder is left');
});

test('a skill that is not there, or is not a skill, is not imported', async (t) => {
  const f = fixture(t);
  await assert.rejects(f.content.importUserSkill(f.id, 'absent'), BotContentNotFoundError);
  writeFileSync(join(f.defaultConfigHome, 'skills', 'afile'), 'x');
  await assert.rejects(f.content.importUserSkill(f.id, 'afile'), /not a directory/);
  mkdirSync(join(f.defaultConfigHome, 'skills', 'empty'));
  writeFileSync(join(f.defaultConfigHome, 'skills', 'empty', 'readme.md'), 'x');
  await assert.rejects(f.content.importUserSkill(f.id, 'empty'), /no SKILL\.md/);
  await assert.rejects(f.content.importUserSkill(MISSING, 'empty'), BotNotFoundError);
  assert.deepEqual(readdirSync(f.skills), []);
});

test('a link inside a skill is refused, and so is anything that is not a plain file or folder', async (t) => {
  const f = fixture(t);
  const source = writeSkill(join(f.defaultConfigHome, 'skills'), 'linky');
  symlinkSync(join(f.outside, 'secret.md'), join(source, 'secret-link.md'));
  await assert.rejects(f.content.importUserSkill(f.id, 'linky'), /something other than files and folders: secret-link\.md/);
  rmFile(join(source, 'secret-link.md'));
  mkdirSync(join(source, 'docs'));
  symlinkSync(f.outside, join(source, 'docs', 'outside'));
  await assert.rejects(f.content.importUserSkill(f.id, 'linky'), /something other than files and folders/);
  assert.deepEqual(readdirSync(f.skills), [], 'nothing was copied');
});

test('a skill that the owner keeps behind a link of their own can be imported', async (t) => {
  const f = fixture(t);
  const real = writeSkill(join(f.root, 'dotfiles', 'skills'), 'shared');
  symlinkSync(real, join(f.defaultConfigHome, 'skills', 'shared'));
  assert.deepEqual(await f.content.importUserSkill(f.id, 'shared'), { name: 'shared', description: 'Does shared' });
  assert.ok(lstatSync(join(f.skills, 'shared')).isDirectory(), 'the copy is a folder of its own, not another link');
  assert.ok(existsSync(join(f.skills, 'shared', 'SKILL.md')));
});

test('a skill that is too large, too many files or too deep is refused', async (t) => {
  const f = fixture(t);
  const many = writeSkill(join(f.defaultConfigHome, 'skills'), 'many');
  for (let index = 0; index < 512; index += 1) writeFileSync(join(many, `f${index}.txt`), '');
  await assert.rejects(f.content.importUserSkill(f.id, 'many'), /too large to import/);
  const heavy = writeSkill(join(f.defaultConfigHome, 'skills'), 'heavy');
  writeFileSync(join(heavy, 'blob.bin'), Buffer.alloc(16 * 1024 * 1024 + 1));
  await assert.rejects(f.content.importUserSkill(f.id, 'heavy'), /too large to import/);
  const deep = writeSkill(join(f.defaultConfigHome, 'skills'), 'deep', { [`${'d/'.repeat(9)}leaf.txt`]: 'x' });
  await assert.rejects(f.content.importUserSkill(f.id, 'deep'), /nested too deeply/);
  const shallow = writeSkill(join(f.defaultConfigHome, 'skills'), 'shallow', { [`${'d/'.repeat(8)}leaf.txt`]: 'x' });
  assert.ok(shallow);
  assert.deepEqual(await f.content.importUserSkill(f.id, 'shallow'), { name: 'shallow', description: 'Does shallow' });
  assert.deepEqual(readdirSync(f.skills), ['shallow']);
});

function rmFile(path: string, recursive = false): void {
  rmSync(path, { recursive, force: true });
}
