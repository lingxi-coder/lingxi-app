import { promises as fs } from 'node:fs';
import { join } from 'node:path';
import { randomUUID } from 'node:crypto';
import type { BotPackageManifest } from '../../shared/botMarket.js';
import type { MarketApiClient } from '../local-apps/marketClient.js';
import {
  canonical,
  sha256,
  unpackSource,
  type SourceFile,
} from '../local-apps/sourcePackage.js';
import { validateBotManifest, validatePublicBotText } from './package.js';
import { skillName } from './content.js';

/** A Bot release as the market describes it, before anything about it is trusted. */
export interface BotRelease {
  id: string;
  appId: string;
  resourceKind: 'bot';
  schema: 3;
  manifest: BotPackageManifest;
  files: SourceFile[];
  dependencies: unknown[];
  releaseDigest: string;
  sha256: string;
  sizeBytes: number;
  downloadPath: string;
}

/** A release whose description holds together: its manifest, its file list and its digest agree, and this computer can host it. */
export interface CheckedBotRelease {
  release: BotRelease;
  manifest: BotPackageManifest;
  digest: string;
}

/** The public texts of a downloaded release, each checked against the release it was approved as. */
export interface BotReleaseContent {
  /** Package path to text, for every file of the manifest. */
  content: Map<string, string>;
  skills: { name: string; path: string; text: string }[];
}

/** What an installed Bot's description says until its owner writes their own. */
export function installedDescription(version: string): string {
  return `Installed Bot ${version}. Configure your model and resource authorizations before sharing.`;
}

/** The name of the Skill a package path holds, or an error: a Bot package carries named text Skills only. */
export function packageSkillName(path: string): string {
  const match = /^skills\/([^/]+)\/SKILL\.md$/.exec(path);
  if (!match) throw new Error('Bot only supports named text Skills');
  return skillName(match[1]);
}

/**
 * Asks the market for one release of a listing and checks its description. Installing a Bot and moving an installed Bot to
 * another release both start here, so a release is never judged by two sets of rules.
 */
export async function checkedBotRelease(
  request: <T>(path: string) => Promise<T>,
  listingId: string,
  versionId: string,
): Promise<CheckedBotRelease> {
  const release = await request<BotRelease>(
    `bots/${listingId}/versions/${versionId}`,
  );
  if (
    release.resourceKind !== 'bot' ||
    release.schema !== 3 ||
    release.id !== versionId ||
    release.appId !== listingId
  )
    throw new Error('Invalid Bot market release');
  const manifest = validateBotManifest(release.manifest);
  const platform =
    process.platform === 'darwin'
      ? 'macos'
      : process.platform === 'win32'
        ? 'windows'
        : 'linux';
  if (!manifest.runtime.hostingTargets.includes(platform))
    throw new Error('This Bot requires another Desktop host');
  if (!Array.isArray(release.dependencies) || release.dependencies.length)
    throw new Error('This Bot has unsupported dependencies');
  if (
    !Array.isArray(release.files) ||
    release.files.length !== manifest.files.length + 1 ||
    release.files.some((f) => f.executable !== false)
  )
    throw new Error('Invalid Bot package inventory');
  const declared = new Map(manifest.files.map((f) => [f.path, f]));
  for (const f of release.files) {
    const expected = declared.get(f.path);
    if (
      f.path !== 'lingxi-bot.json' &&
      (!expected ||
        canonical({ ...expected, executable: false }) !== canonical(f))
    )
      throw new Error('Bot file inventory mismatch');
  }
  const size = release.sizeBytes;
  if (
    !Number.isSafeInteger(size) ||
    size <= 0 ||
    size > 25 * 1024 * 1024 ||
    !/^[a-f0-9]{64}$/.test(release.sha256)
  )
    throw new Error('Bot package exceeds download limits');
  const digest = sha256(
    Buffer.from(
      canonical({
        schema: 1,
        artifactSha256: release.sha256,
        manifest,
        files: release.files,
        dependencies: [],
      }),
    ),
  );
  if (digest !== release.releaseDigest)
    throw new Error('Bot release checksum mismatch');
  return { release, manifest, digest };
}

/**
 * Downloads a checked release, unpacks it under `directory` and reads its public texts. The bytes must be the ones the
 * release names, the manifest inside must be the approved one, and every text passes the public-text check. Nothing is
 * left under `directory` afterwards.
 */
export async function downloadBotRelease(
  api: MarketApiClient,
  checked: CheckedBotRelease,
  directory: string,
): Promise<BotReleaseContent> {
  const { release, manifest } = checked;
  // Authentication is restricted to the canonical same-origin v3 content proxy.
  const expected = new URL(
      `bots/${release.appId}/versions/${release.id}/download`,
      api.base,
    ),
    download = new URL(release.downloadPath, api.base);
  if (download.href !== expected.href)
    throw new Error('Invalid authenticated Bot download route');
  const response = await api.fetcher(download, {
    headers: { Authorization: `Bearer ${await api.token()}` },
    redirect: 'error',
    signal: AbortSignal.timeout(60_000),
  });
  if (!response.ok || !response.body) throw new Error('Bot download failed');
  const size = release.sizeBytes;
  const chunks: Buffer[] = [];
  let received = 0;
  for await (const chunk of response.body as unknown as AsyncIterable<Uint8Array>) {
    received += chunk.byteLength;
    if (received > size) throw new Error('Bot exceeds declared package size');
    chunks.push(Buffer.from(chunk));
  }
  const bytes = Buffer.concat(chunks);
  if (bytes.length !== size || sha256(bytes) !== release.sha256)
    throw new Error('Bot package checksum mismatch');
  const stage = join(directory, `stage-${randomUUID()}`);
  try {
    await unpackSource(bytes, release.files, stage, 'lingxi-bot.json');
    if (
      canonical(
        JSON.parse(await fs.readFile(join(stage, 'lingxi-bot.json'), 'utf8')),
      ) !== canonical(manifest)
    )
      throw new Error('Bot manifest differs from approved release');
    const content = new Map<string, string>();
    for (const file of manifest.files) {
      const value = new TextDecoder('utf-8', { fatal: true }).decode(
        await fs.readFile(join(stage, file.path)),
      );
      validatePublicBotText(file.path, value);
      content.set(file.path, value);
    }
    const skills = manifest.skills.map((s) => ({
      name: packageSkillName(s.path),
      path: s.path,
      text: content.get(s.path)!,
    }));
    if (new Set(skills.map((s) => s.name.toLowerCase())).size !== skills.length)
      throw new Error('Duplicate Bot Skills');
    return { content, skills };
  } finally {
    await fs.rm(stage, { recursive: true, force: true });
  }
}
