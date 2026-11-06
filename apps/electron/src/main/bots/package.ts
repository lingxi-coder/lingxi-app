import { constants } from 'node:fs';
import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type {
  BotExportSelection,
  BotPackageManifest,
  BotPackagePreview,
} from '../../shared/botMarket.js';
import {
  canonical,
  packagePath,
  packSource,
  unpackSource,
  sha256,
  type SourceFile,
} from '../local-apps/sourcePackage.js';
import { skillName } from './content.js';
import type { BotStore } from './store.js';

const forbidden =
  /-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----|"type"\s*:\s*"service_account"|\b(?:sk-[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,})\b|(?:credentialSlot|ownerUid|sessionId|bindingId)["']?\s*[=:]|(?:\/Users\/|\/home\/|[A-Z]:\\Users\\)/i;
function exact(value: unknown, keys: string[]): Record<string, unknown> {
  if (
    !value ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some((k) => !keys.includes(k)) ||
    keys.some((k) => !(k in value))
  )
    throw new Error('Bot package contains unsupported or missing fields');
  return value as Record<string, unknown>;
}
function text(value: unknown, max: number): asserts value is string {
  if (
    typeof value !== 'string' ||
    value.includes('\0') ||
    Buffer.byteLength(value) > max
  )
    throw new Error('Invalid Bot package text');
}
export function validatePublicBotText(path: string, value: string): void {
  packagePath(path);
  if (
    !(
      /\.(md|txt)$/i.test(path) || /^(LICENSE|NOTICE)(\.txt|\.md)?$/i.test(path)
    ) ||
    path
      .split('/')
      .some((p) =>
        /^(memory|memories|state|bindings|credentials?|sessions?|chats?|history|\.git|\.env)$/i.test(
          p,
        ),
      )
  )
    throw new Error('Private or executable Bot file is forbidden');
  text(value, 1024 * 1024);
  if (forbidden.test(value))
    throw new Error(
      'Bot export contains suspected credentials, private bindings or local paths',
    );
}
export function validateBotManifest(value: unknown): BotPackageManifest {
  const m = exact(value, [
    'schema',
    'kind',
    'name',
    'version',
    'license',
    'licenseFile',
    'entry',
    'skills',
    'resources',
    'capabilities',
    'runtime',
    'examples',
    'files',
  ]);
  if (m.schema !== 1 || m.kind !== 'bot')
    throw new Error('Unsupported Bot package');
  for (const [k, max] of [
    ['name', 60],
    ['license', 128],
    ['version', 128],
  ] as const) {
    text(m[k], max);
    if (!(m[k] as string).length)
      throw new Error('Missing Bot package metadata');
  }
  if (!/^\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?$/.test(m.version as string))
    throw new Error('Invalid Bot version');
  const list = (v: unknown, max: number): unknown[] => {
    if (!Array.isArray(v) || v.length > max)
      throw new Error('Invalid Bot package list');
    return v;
  };
  const files = list(m.files, 200).map((f) => {
    const row = exact(f, ['path', 'sizeBytes', 'sha256']);
    text(row.path, 240);
    packagePath(row.path);
    if (
      !Number.isSafeInteger(row.sizeBytes) ||
      (row.sizeBytes as number) < 0 ||
      (row.sizeBytes as number) > 1024 * 1024 ||
      typeof row.sha256 !== 'string' ||
      !/^[a-f0-9]{64}$/.test(row.sha256)
    )
      throw new Error('Invalid Bot file digest');
    return row;
  });
  const paths = files.map((f) => f.path as string);
  if (
    files.length < 2 ||
    new Set(paths.map((p) => p.toLowerCase())).size !== paths.length ||
    paths.includes('lingxi-bot.json')
  )
    throw new Error('Invalid Bot file allowlist');
  const refs = [
    m.entry,
    m.licenseFile,
    ...list(m.skills, 100).map((s) => exact(s, ['path']).path),
  ];
  for (const p of refs) {
    text(p, 240);
    packagePath(p);
    if (!paths.includes(p))
      throw new Error('Bot package reference is not explicitly declared');
  }
  const resources = list(m.resources, 100).map((r) =>
    exact(r, ['id', 'binding', 'required']),
  );
  if (
    resources.some(
      (r) =>
        typeof r.id !== 'string' ||
        !/^[A-Za-z0-9_-]{1,128}$/.test(r.id) ||
        r.binding !== 'installer' ||
        typeof r.required !== 'boolean',
    ) ||
    new Set(resources.map((r) => r.id)).size !== resources.length
  )
    throw new Error('Invalid public resource requirements');
  for (const c of list(m.capabilities, 2)) {
    const row = exact(c, ['tool', 'operations']);
    if (
      !['shared_resource.search', 'chat.read_messages'].includes(
        row.tool as string,
      ) ||
      !Array.isArray(row.operations) ||
      row.operations.length !== 1 ||
      row.operations[0] !== 'read'
    )
      throw new Error('Unsupported Bot capability');
  }
  const runtime = exact(m.runtime, [
    'minimumBotPackageVersion',
    'requiresTeamExecutionVersion',
    'hostingTargets',
    'memberClients',
  ]);
  if (
    runtime.minimumBotPackageVersion !== 1 ||
    runtime.requiresTeamExecutionVersion !== 2 ||
    !list(runtime.hostingTargets, 3).length ||
    (runtime.hostingTargets as unknown[]).some(
      (t) => !['macos', 'windows', 'linux'].includes(t as string),
    ) ||
    !list(runtime.memberClients, 4).length ||
    (runtime.memberClients as unknown[]).some(
      (t) => !['web', 'desktop', 'ios', 'android'].includes(t as string),
    )
  )
    throw new Error('Unsupported Bot runtime');
  for (const e of list(m.examples, 20)) text(e, 2000);
  if (forbidden.test(canonical(value)))
    throw new Error('Bot manifest contains suspected private material');
  return value as BotPackageManifest;
}
async function readSelected(root: string, path: string): Promise<string> {
  packagePath(path);
  let current = root;
  if (!(await fs.lstat(root)).isDirectory())
    throw new Error('Bot export refuses directory links');
  for (const part of path.split('/').slice(0, -1)) {
    current = join(current, part);
    if (!(await fs.lstat(current)).isDirectory())
      throw new Error('Bot export refuses directory links');
  }
  const handle = await fs.open(
    join(root, path),
    constants.O_RDONLY | constants.O_NOFOLLOW,
  );
  try {
    const info = await handle.stat();
    if (!info.isFile() || info.nlink !== 1 || info.size > 1024 * 1024)
      throw new Error(
        'Bot export requires bounded regular files without links',
      );
    const buffer = Buffer.alloc(1024 * 1024 + 1);
    let size = 0;
    while (size < buffer.length) {
      const { bytesRead } = await handle.read(
        buffer,
        size,
        buffer.length - size,
        null,
      );
      if (!bytesRead) break;
      size += bytesRead;
    }
    if (size > 1024 * 1024) throw new Error('Bot export file is too large');
    return new TextDecoder('utf-8', { fatal: true }).decode(
      buffer.subarray(0, size),
    );
  } finally {
    await handle.close();
  }
}
export interface PreparedBotPackage {
  preview: BotPackagePreview;
  directory: string;
  inventory: SourceFile[];
  archive: Buffer;
  sourceBotId: string;
  sourceRevision: number;
}
export async function prepareBotPackage(
  store: BotStore,
  selection: BotExportSelection,
  stagingRoot: string,
): Promise<PreparedBotPackage> {
  exact(selection, [
    'botId',
    'expectedRevision',
    'version',
    'license',
    'licenseText',
    'skills',
    'resources',
    'examples',
  ]);
  const bot = store.get(selection.botId);
  if (bot.revision !== selection.expectedRevision)
    throw new Error('Bot changed; review its latest revision before exporting');
  if (
    !Array.isArray(selection.skills) ||
    selection.skills.length > 100 ||
    new Set(selection.skills).size !== selection.skills.length
  )
    throw new Error('Choose explicit text Skills');
  if (!Array.isArray(selection.resources))
    throw new Error('Choose explicit read-only resource requirements');
  const files = [
    {
      path: 'persona.md',
      text: await readSelected(store.homeDir(bot.bot_id), 'LINGXI.md'),
    },
    { path: 'LICENSE', text: selection.licenseText },
  ];
  for (const name of selection.skills) {
    skillName(name);
    files.push({
      path: `skills/${name}/SKILL.md`,
      text: await readSelected(
        store.homeDir(bot.bot_id),
        `skills/${name}/SKILL.md`,
      ),
    });
  }
  for (const file of files) validatePublicBotText(file.path, file.text);
  const manifest = validateBotManifest({
    schema: 1,
    kind: 'bot',
    name: bot.name,
    version: selection.version,
    license: selection.license,
    licenseFile: 'LICENSE',
    entry: 'persona.md',
    skills: selection.skills.map((name) => ({
      path: `skills/${name}/SKILL.md`,
    })),
    resources: selection.resources.map((r) => ({
      ...exact(r, ['id', 'required']),
      binding: 'installer',
    })),
    capabilities: [
      { tool: 'chat.read_messages', operations: ['read'] },
      ...(selection.resources.length
        ? [{ tool: 'shared_resource.search', operations: ['read'] }]
        : []),
    ],
    runtime: {
      minimumBotPackageVersion: 1,
      requiresTeamExecutionVersion: 2,
      hostingTargets: ['macos'],
      memberClients: ['web', 'desktop', 'ios', 'android'],
    },
    examples: selection.examples,
    files: files.map((f) => ({
      path: f.path,
      sizeBytes: Buffer.byteLength(f.text),
      sha256: sha256(Buffer.from(f.text)),
    })),
  });
  const id = randomUUID(),
    directory = join(stagingRoot, id);
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  try {
    files.push({ path: 'lingxi-bot.json', text: canonical(manifest) });
    for (const file of files) {
      await fs.mkdir(join(directory, file.path, '..'), {
        recursive: true,
        mode: 0o700,
      });
      await fs.writeFile(join(directory, file.path), file.text, {
        flag: 'wx',
        mode: 0o600,
      });
    }
    const inventory = files
      .map((f) => ({
        path: f.path,
        sizeBytes: Buffer.byteLength(f.text),
        sha256: sha256(Buffer.from(f.text)),
        executable: false,
      }))
      .sort((a, b) => (a.path < b.path ? -1 : 1));
    const archive = await packSource(directory, inventory);
    const digest = sha256(
      Buffer.from(
        canonical({
          schema: 1,
          artifactSha256: sha256(archive),
          manifest,
          files: inventory,
          dependencies: [],
        }),
      ),
    );
    if (store.get(bot.bot_id).revision !== bot.revision)
      throw new Error('Bot changed during export');
    return {
      preview: { id, manifest, files, digest },
      directory,
      inventory,
      archive,
      sourceBotId: bot.bot_id,
      sourceRevision: bot.revision,
    };
  } catch (error) {
    await fs.rm(directory, { recursive: true, force: true });
    throw error;
  }
}

/** Recover the exact reviewed bytes after restart; no source files are scanned again. */
export async function restoreBotPackage(
  store: BotStore,
  botId: string,
  publicationDirectory: string,
  stagingDirectory: string,
): Promise<PreparedBotPackage> {
  const bot = store.get(botId);
  const review = JSON.parse(
    await fs.readFile(join(publicationDirectory, 'review.json'), 'utf8'),
  ) as BotPackagePreview;
  const manifest = validateBotManifest(review.manifest);
  if (
    !Array.isArray(review.files) ||
    review.files.length !== manifest.files.length + 1
  )
    throw new Error('Invalid reviewed package snapshot');
  const declared = new Map(manifest.files.map((file) => [file.path, file]));
  const inventory = review.files
    .map((file) => {
      if (file.path === 'lingxi-bot.json') {
        if (canonical(JSON.parse(file.text)) !== canonical(manifest))
          throw new Error('Reviewed manifest changed');
      } else {
        validatePublicBotText(file.path, file.text);
        const expected = declared.get(file.path);
        if (
          !expected ||
          expected.sizeBytes !== Buffer.byteLength(file.text) ||
          expected.sha256 !== sha256(Buffer.from(file.text))
        )
          throw new Error('Reviewed file changed');
      }
      return {
        path: file.path,
        sizeBytes: Buffer.byteLength(file.text),
        sha256: sha256(Buffer.from(file.text)),
        executable: false,
      };
    })
    .sort((a, b) => (a.path < b.path ? -1 : 1));
  const archive = await fs.readFile(join(publicationDirectory, 'source.zip'));
  const digest = sha256(
    Buffer.from(
      canonical({
        schema: 1,
        artifactSha256: sha256(archive),
        manifest,
        files: inventory,
        dependencies: [],
      }),
    ),
  );
  if (digest !== review.digest) throw new Error('Reviewed archive changed');
  // Existing bounded ZIP validator checks duplicates, complete inventory and every file digest.
  await fs.mkdir(join(stagingDirectory, '..'), {
    recursive: true,
    mode: 0o700,
  });
  try {
    await unpackSource(archive, inventory, stagingDirectory, 'lingxi-bot.json');
  } catch (error) {
    await fs.rm(stagingDirectory, { recursive: true, force: true });
    throw error;
  }
  return {
    preview: { ...review, id: randomUUID() },
    directory: stagingDirectory,
    inventory,
    archive,
    sourceBotId: botId,
    sourceRevision: bot.revision,
  };
}
